//! `gops prj upgrade`：发布态的**升级事务**。
//!
//! 设计见 `docs/design/prj-upgrade.md`。本模块做两件事，故意分开：
//!
//! 1. [`run_upgrade`] —— **纯编排**：阶段序（diagnose → backup → apply → regenerate → pull →
//!    up → health）+ 失败处置（[`FailurePolicy`]）+ 状态文件。它只依赖 [`UpgradeRuntime`] 抽象，
//!    所以状态机可以脱离 docker / gx 单测（见 `mod tests` 里的 `FakeRuntime`）。
//! 2. [`ProjectRuntime`] —— 真实实现：把 `diagnose`/`backup`/`apply`/`restore` 接到本 crate 既有
//!    实现上，把 `localize`/`pull`/`up`/`status` 交给注入的 [`RuntimeDispatch`]（docker compose /
//!    gx 在 app 层，本 crate 不认得）。
//!
//! 与 `prj update` 的分工：`update` 只覆盖**内容**；本模块在其上补事务边界（备份、拉取、切换、
//! 健康、回滚），也是「发布态升级」唯一的动词。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::backup::restore;
use super::diagnose::{DiagnoseRequest, project_diagnose};
use super::install::fetch_and_prepare;
use super::prelude::*;
use super::project::OpsProject;
use super::update::apply_overlay;
use crate::accessor::accessor_for_default;
use crate::error::MainReason;
use crate::system::lock::DeliverLock;

/// 失败发生时**做什么**（`--on-failure`）。
///
/// 默认值**尚未确定**，所以 CLI 目前要求显式给出 —— 见设计稿 §3.3。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailurePolicy {
    /// 整工程原子：任一系统失败 → 全部回滚（含已成功的）。
    RollbackAll,
    /// 停在那里：不继续，已切换的不回滚；如实报告停在哪个系统/哪一步。
    Halt,
}

impl FailurePolicy {
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "rollback-all" => Some(Self::RollbackAll),
            "halt" => Some(Self::Halt),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::RollbackAll => "rollback-all",
            Self::Halt => "halt",
        }
    }
}

/// `--to` 的取值是不是「地址」而不是「版本」。
pub fn looks_like_address(value: &str) -> bool {
    let v = value.trim();
    v.contains("://")
        || v.starts_with('/')
        || v.starts_with("./")
        || v.starts_with("../")
        || v.starts_with("git@")
}

/// 把 `--to` 解析成某系统要下载的地址。
///
/// - `target` 看着像地址（`scheme://`、`/abs`、`./rel`、`git@`）→ 原样用于所有系统；
/// - 否则当**版本**：用该系统 ref 的 addr 模板（`{version}`）渲染；模板缺占位则报错。
pub fn resolve_target_addr(project: &OpsProject, sys: &str, target: &str) -> MainResult<String> {
    if looks_like_address(target) {
        return Ok(target.trim().to_string());
    }
    let sys_ref = project
        .conf()
        .sys_models()
        .iter()
        .find(|s| s.sys().name().as_str() == sys)
        .ok_or_else(|| MainReason::logic_detail(format!("ops-prj.yml 里没有系统 `{sys}`")))?;
    sys_ref.resolved_addr_with(target)
}

/// 事务走到的步骤（写进状态文件，供「上次停在哪」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeStep {
    Diagnose,
    Backup,
    Apply,
    Regenerate,
    Pull,
    Up,
    Health,
    Rollback,
    Done,
}

impl UpgradeStep {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Diagnose => "diagnose",
            Self::Backup => "backup",
            Self::Apply => "apply",
            Self::Regenerate => "regenerate",
            Self::Pull => "pull",
            Self::Up => "up",
            Self::Health => "health",
            Self::Rollback => "rollback",
            Self::Done => "done",
        }
    }
}

/// 事务结局。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeStatus {
    Succeeded,
    Failed,
    /// `rollback-all` 且回退成功。
    RolledBack,
}

impl UpgradeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::RolledBack => "rolled_back",
        }
    }
}

/// 状态文件（与 agentd 的 `state/upgrade.json` 同形）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeRecord {
    pub from_version: String,
    pub to_version: String,
    pub step: String,
    pub status: String,
    #[serde(default)]
    pub backup_id: Option<String>,
    pub updated_at: String,
    #[serde(default)]
    pub detail: String,
}

/// 本次事务的静态计划（`--dry-run` 也返回它）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UpgradePlan {
    pub systems: Vec<String>,
    pub target: String,
    pub policy: String,
    pub health: bool,
}

/// 事务结果。
#[derive(Clone, Debug)]
pub struct UpgradeOutcome {
    pub plan: UpgradePlan,
    pub dry_run: bool,
    pub record: Option<UpgradeRecord>,
    pub record_path: PathBuf,
}

impl UpgradeOutcome {
    /// 成功（含 dry-run）= 0；失败与已回滚 = 1。回滚是「失败的一种结局」，不算成功。
    pub fn exit_code(&self) -> i32 {
        match &self.record {
            None => 0,
            Some(record) if record.status == UpgradeStatus::Succeeded.as_str() => 0,
            Some(_) => 1,
        }
    }

    /// `--json` 的单行输出（给编排器判成败）。
    pub fn to_json(&self) -> String {
        let record = self.record.as_ref().map(|r| {
            serde_json::json!({
                "from_version": r.from_version,
                "to_version": r.to_version,
                "step": r.step,
                "status": r.status,
                "backup_id": r.backup_id,
                "updated_at": r.updated_at,
                "detail": r.detail,
            })
        });
        serde_json::json!({
            "dry_run": self.dry_run,
            "systems": self.plan.systems,
            "target": self.plan.target,
            "on_failure": self.plan.policy,
            "health": self.plan.health,
            "record": record,
            "record_path": self.record_path.display().to_string(),
        })
        .to_string()
    }
}

/// 升级事务的真实副作用接口。
///
/// `diagnose`/`backup`/`apply`/`restore`/`version_of` 由 [`ProjectRuntime`] 接本 crate 既有实现；
/// `localize`/`pull`/`up`/`status` 走 [`RuntimeDispatch`]（docker compose / gx，在 app 层）。
#[async_trait]
pub trait UpgradeRuntime: Send + Sync {
    /// 前置体检（失败即拒，**不动现场**）。
    async fn diagnose(&self) -> MainResult<()>;
    /// 备份现场态，返回交给 [`UpgradeRuntime::restore`] 的备份标识（归档路径）。
    async fn backup(&self) -> MainResult<String>;
    /// 取包并覆盖到指定系统目录（只动磁盘内容，**不停服务**）。
    async fn apply(&self, sys: &str) -> MainResult<()>;
    /// 重渲染值 / `.env`（`sys update` + `localize`），为切换做准备。
    async fn regenerate(&self, sys: &str) -> MainResult<()>;
    /// 拉制品（`docker compose pull` / `gx run download`）。
    async fn pull(&self, sys: &str) -> MainResult<()>;
    /// 切换（`docker compose up -d` / `gx run start`）。
    async fn up(&self, sys: &str) -> MainResult<()>;
    /// 状态快照（`docker compose ps` / `gx run status`）。
    async fn status(&self, sys: &str) -> MainResult<()>;
    /// 栈外健康检查（未配置时直接 `Ok`）。
    async fn health(&self) -> MainResult<()>;
    /// 回滚：把备份还原回现场（**状态级**，含库）。
    async fn restore(&self, backup: &str) -> MainResult<()>;
    /// 读某系统当前版本（`deliver.lock`，缺失时退回 `version.txt`）。
    async fn version_of(&self, sys: &str) -> MainResult<String>;
}

/// 运行时动作（按 `kind` 分派到 docker compose / gx）—— app 层实现。
#[async_trait]
pub trait RuntimeDispatch: Send + Sync {
    /// `sys update` + `localize`（含渲染与 localize 阶段流程）。
    async fn localize(&self, sys_dir: &Path) -> MainResult<()>;
    /// 拉制品。
    async fn pull(&self, sys_dir: &Path) -> MainResult<()>;
    /// 切换。
    async fn up(&self, sys_dir: &Path) -> MainResult<()>;
    /// 状态。
    async fn status(&self, sys_dir: &Path) -> MainResult<()>;
}

struct PhaseFailure {
    step: UpgradeStep,
    sys: String,
    detail: String,
}

fn make_record(
    step: UpgradeStep,
    status: UpgradeStatus,
    backup_id: Option<String>,
    from_version: String,
    to_version: String,
    detail: String,
) -> UpgradeRecord {
    UpgradeRecord {
        from_version,
        to_version,
        step: step.as_str().to_string(),
        status: status.as_str().to_string(),
        backup_id,
        updated_at: chrono::Utc::now().to_rfc3339(),
        detail,
    }
}

async fn join_versions(runtime: &dyn UpgradeRuntime, systems: &[String]) -> String {
    let mut out = Vec::with_capacity(systems.len());
    for sys in systems {
        let version = runtime.version_of(sys).await.unwrap_or_default();
        out.push(format!("{sys}={version}"));
    }
    out.join(",")
}

/// 阶段 3~7：apply → regenerate → pull → up → health(+status)。按**阶段**跨全部系统推进，
/// 保证「先拉后停」——第 5 步 `pull` 全做完，才允许第 6 步 `up`。
async fn run_phases(
    runtime: &dyn UpgradeRuntime,
    systems: &[String],
    expect_health: bool,
) -> Result<(), PhaseFailure> {
    async fn step(
        step: UpgradeStep,
        sys: &str,
        fut: impl std::future::Future<Output = MainResult<()>>,
    ) -> Result<(), PhaseFailure> {
        fut.await.map_err(|err| PhaseFailure {
            step,
            sys: sys.to_string(),
            detail: err.to_string(),
        })
    }

    for sys in systems {
        step(UpgradeStep::Apply, sys, runtime.apply(sys)).await?;
    }
    for sys in systems {
        step(UpgradeStep::Regenerate, sys, runtime.regenerate(sys)).await?;
    }
    for sys in systems {
        step(UpgradeStep::Pull, sys, runtime.pull(sys)).await?;
    }
    for sys in systems {
        step(UpgradeStep::Up, sys, runtime.up(sys)).await?;
    }
    if expect_health {
        step(UpgradeStep::Health, "", runtime.health()).await?;
    }
    for sys in systems {
        step(UpgradeStep::Health, sys, runtime.status(sys)).await?;
    }
    Ok(())
}

/// 跑一次升级事务。**任何失败都落状态文件并返回 `Ok`**（结局在 `record.status` 里）；
/// 只有「系统性错误」（无可升级系统、状态文件写失败）才返回 `Err`。
pub async fn run_upgrade(
    runtime: &dyn UpgradeRuntime,
    systems: &[String],
    target: &str,
    policy: FailurePolicy,
    expect_health: bool,
    dry_run: bool,
    record_path: &Path,
) -> MainResult<UpgradeOutcome> {
    let plan = UpgradePlan {
        systems: systems.to_vec(),
        target: target.to_string(),
        policy: policy.as_str().to_string(),
        health: expect_health,
    };

    if systems.is_empty() {
        return Err(MainReason::logic_detail(
            "没有可升级的系统：ops-prj.yml 里没有该系统，或它还没导入（先 `gops prj import`）",
        ));
    }

    if dry_run {
        return Ok(UpgradeOutcome {
            plan,
            dry_run: true,
            record: None,
            record_path: record_path.to_path_buf(),
        });
    }

    let from_version = join_versions(runtime, systems).await;

    // 1) 前置体检：失败则**未动现场**，不回滚。
    if let Err(err) = runtime.diagnose().await {
        let record = make_record(
            UpgradeStep::Diagnose,
            UpgradeStatus::Failed,
            None,
            from_version,
            String::new(),
            format!("前置体检未通过（未动现场）：{err}"),
        );
        write_record(record_path, &record)?;
        return Ok(UpgradeOutcome {
            plan,
            dry_run: false,
            record: Some(record),
            record_path: record_path.to_path_buf(),
        });
    }

    // 2) 备份：回滚点。失败则**未动现场**，不回滚。
    let backup = match runtime.backup().await {
        Ok(backup) => backup,
        Err(err) => {
            let record = make_record(
                UpgradeStep::Backup,
                UpgradeStatus::Failed,
                None,
                from_version,
                String::new(),
                format!("备份失败（未动现场）：{err}"),
            );
            write_record(record_path, &record)?;
            return Ok(UpgradeOutcome {
                plan,
                dry_run: false,
                record: Some(record),
                record_path: record_path.to_path_buf(),
            });
        }
    };

    // 3~7) 阶段序。
    match run_phases(runtime, systems, expect_health).await {
        Ok(()) => {
            let to_version = join_versions(runtime, systems).await;
            let record = make_record(
                UpgradeStep::Done,
                UpgradeStatus::Succeeded,
                Some(backup),
                from_version,
                to_version,
                String::new(),
            );
            write_record(record_path, &record)?;
            Ok(UpgradeOutcome {
                plan,
                dry_run: false,
                record: Some(record),
                record_path: record_path.to_path_buf(),
            })
        }
        Err(failure) => {
            let failure_detail = format!(
                "步骤 {} 失败（系统 {}）：{}",
                failure.step.as_str(),
                if failure.sys.is_empty() {
                    "-"
                } else {
                    &failure.sys
                },
                failure.detail
            );
            match policy {
                FailurePolicy::Halt => {
                    let record = make_record(
                        failure.step,
                        UpgradeStatus::Failed,
                        Some(backup),
                        from_version,
                        String::new(),
                        format!(
                            "{failure_detail}；按 --on-failure halt 停在中间态，未回滚（现场可能部分新、部分旧）"
                        ),
                    );
                    write_record(record_path, &record)?;
                    Ok(UpgradeOutcome {
                        plan,
                        dry_run: false,
                        record: Some(record),
                        record_path: record_path.to_path_buf(),
                    })
                }
                FailurePolicy::RollbackAll => {
                    let (rolled_back, rollback_detail) = rollback(runtime, systems, &backup).await;
                    let status = if rolled_back {
                        UpgradeStatus::RolledBack
                    } else {
                        // 回滚自身失败：也算失败，但要把备份在哪、卡在哪说清楚。
                        UpgradeStatus::Failed
                    };
                    let record = make_record(
                        UpgradeStep::Rollback,
                        status,
                        Some(backup),
                        from_version,
                        String::new(),
                        format!("{failure_detail}；{rollback_detail}"),
                    );
                    write_record(record_path, &record)?;
                    Ok(UpgradeOutcome {
                        plan,
                        dry_run: false,
                        record: Some(record),
                        record_path: record_path.to_path_buf(),
                    })
                }
            }
        }
    }
}

/// `rollback-all`：还原备份 → 逐系统重新拉起 → 探活。返回 (是否全部成功, 描述)。
async fn rollback(
    runtime: &dyn UpgradeRuntime,
    systems: &[String],
    backup: &str,
) -> (bool, String) {
    let mut ok = true;
    let mut detail = format!("已按 --on-failure rollback-all 回滚（备份 {backup}）");

    if let Err(err) = runtime.restore(backup).await {
        ok = false;
        detail.push_str(&format!("；还原失败：{err}"));
    }
    for sys in systems {
        if let Err(err) = runtime.up(sys).await {
            ok = false;
            detail.push_str(&format!("；系统 {sys} 拉起失败：{err}"));
        }
    }
    if let Err(err) = runtime.health().await {
        ok = false;
        detail.push_str(&format!("；回滚后探活失败：{err}"));
    }
    if !ok {
        detail.push_str("；⚠ 回滚未完全成功，现场可能停在中间态，请人工介入");
    }
    (ok, detail)
}

/// 状态文件落点：`~/.galaxy/upgrade/<prj>/upgrade.json`（与 agentd 的升级状态同形）。
pub fn record_path(project_root: &Path) -> PathBuf {
    let name = project_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    let home = home::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".galaxy")
        .join("upgrade")
        .join(name)
        .join("upgrade.json")
}

/// 写状态文件（父目录按需创建）。
pub fn write_record(path: &Path, record: &UpgradeRecord) -> MainResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            MainReason::resource_detail(format!("创建状态目录失败 {}: {e}", parent.display()))
        })?;
    }
    let json = serde_json::to_string_pretty(record)
        .map_err(|e| MainReason::logic_detail(format!("序列化升级状态失败: {e}")))?;
    std::fs::write(path, json).map_err(|e| {
        MainReason::resource_detail(format!("写升级状态失败 {}: {e}", path.display()))
    })?;
    Ok(())
}

/// 读状态文件；不存在时 `None`。
pub fn read_record(path: &Path) -> MainResult<Option<UpgradeRecord>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).map_err(|e| {
        MainReason::resource_detail(format!("读升级状态失败 {}: {e}", path.display()))
    })?;
    let record = serde_json::from_str(&text).map_err(|e| {
        MainReason::logic_detail(format!("解析升级状态失败 {}: {e}", path.display()))
    })?;
    Ok(Some(record))
}

/// `ops-prj.yml` 里**已导入**的系统名（可选只取一个）。未导入的跳过并在返回值里体现。
pub fn imported_systems(project_root: &Path, only: Option<&str>) -> MainResult<Vec<String>> {
    let project = OpsProject::load(project_root)?;
    let mut out = Vec::new();
    for sys in project.conf().sys_models() {
        let name = sys.sys().name();
        if let Some(only) = only
            && name.as_str() != only
        {
            continue;
        }
        if project_root.join(name).is_dir() {
            out.push(name.clone());
        }
    }
    Ok(out)
}

/// 真实运行时：接本 crate 既有实现 + 注入的 [`RuntimeDispatch`]。
///
/// `target` 是 `--to` 的原文：地址（直接用于所有系统）或版本（每个系统各自用
/// addr 模板 `{version}` 渲染，见 [`resolve_target_addr`]）。
pub struct ProjectRuntime {
    project_root: PathBuf,
    target: String,
    force: bool,
    health_cmd: Option<String>,
    health_timeout: Duration,
    dispatch: Arc<dyn RuntimeDispatch>,
}

impl ProjectRuntime {
    pub fn new(
        project_root: PathBuf,
        target: String,
        force: bool,
        health_cmd: Option<String>,
        health_timeout: Duration,
        dispatch: Arc<dyn RuntimeDispatch>,
    ) -> Self {
        Self {
            project_root,
            target,
            force,
            health_cmd,
            health_timeout,
            dispatch,
        }
    }

    fn sys_dir(&self, sys: &str) -> PathBuf {
        self.project_root.join(sys)
    }
}

#[async_trait]
impl UpgradeRuntime for ProjectRuntime {
    async fn diagnose(&self) -> MainResult<()> {
        let report = project_diagnose(&self.project_root, &DiagnoseRequest::new(false))?;
        if report.exit_code() != 0 {
            return Err(MainReason::logic_detail(
                "gops prj diagnose 有 FAIL 项，拒绝升级（先处理上面的问题）",
            ));
        }
        Ok(())
    }

    async fn backup(&self) -> MainResult<String> {
        let project = OpsProject::load(&self.project_root)?;
        let report = project.backup(false)?;
        Ok(report.archive.display().to_string())
    }

    async fn apply(&self, sys: &str) -> MainResult<()> {
        let project = OpsProject::load(&self.project_root)?;
        let addr = resolve_target_addr(&project, sys, &self.target)?;
        let options = DownloadOptions::from((self.force, ValueDict::default()));
        let sys_src = fetch_and_prepare(
            project.paths().clone(),
            &addr,
            accessor_for_default(),
            &options,
        )
        .await?;
        let target = self.sys_dir(sys);
        let _ = apply_overlay(sys, &sys_src, &target)?;
        Ok(())
    }

    async fn regenerate(&self, sys: &str) -> MainResult<()> {
        let dir = self.sys_dir(sys);
        self.dispatch.localize(&dir).await
    }

    async fn pull(&self, sys: &str) -> MainResult<()> {
        let dir = self.sys_dir(sys);
        self.dispatch.pull(&dir).await
    }

    async fn up(&self, sys: &str) -> MainResult<()> {
        let dir = self.sys_dir(sys);
        self.dispatch.up(&dir).await
    }

    async fn status(&self, sys: &str) -> MainResult<()> {
        let dir = self.sys_dir(sys);
        self.dispatch.status(&dir).await
    }

    async fn health(&self) -> MainResult<()> {
        let Some(cmd) = self.health_cmd.as_deref() else {
            return Ok(());
        };
        let deadline = Instant::now() + self.health_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            // 每次尝试都**限时**：探活命令自己挂住时，不能把整个升级事务拖死。
            let attempt = tokio::time::timeout(remaining, async {
                tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(cmd)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await
                    .map(|status| status.success())
                    .unwrap_or(false)
            })
            .await;
            match attempt {
                Ok(true) => return Ok(()),
                Ok(false) => {}  // 这次没通过，退避后重试
                Err(_) => break, // 单次尝试就吃满预算 → 判定超时
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Err(MainReason::logic_detail(format!(
            "健康检查在 {:?} 内未通过：{cmd}",
            self.health_timeout
        )))
    }

    async fn restore(&self, backup: &str) -> MainResult<()> {
        let _ = restore(&self.project_root, Path::new(backup), false)?;
        Ok(())
    }

    async fn version_of(&self, sys: &str) -> MainResult<String> {
        let dir = self.sys_dir(sys);
        if DeliverLock::path(&dir).exists() {
            return Ok(DeliverLock::load(&dir)?.version().to_string());
        }
        Ok(std::fs::read_to_string(dir.join("version.txt"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_default())
    }
}

/// `--health-cmd` 缺省超时（秒）。
pub const DEFAULT_HEALTH_TIMEOUT_SECS: u64 = 60;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    #[test]
    fn failure_policy_parses_known_values_only() {
        assert_eq!(
            FailurePolicy::parse("rollback-all"),
            Some(FailurePolicy::RollbackAll)
        );
        assert_eq!(FailurePolicy::parse(" HALT "), Some(FailurePolicy::Halt));
        assert_eq!(FailurePolicy::parse("continue"), None);
        assert_eq!(FailurePolicy::parse(""), None);
    }

    #[test]
    fn failure_policy_roundtrips() {
        for p in [FailurePolicy::RollbackAll, FailurePolicy::Halt] {
            assert_eq!(FailurePolicy::parse(p.as_str()), Some(p));
        }
    }

    #[test]
    fn looks_like_address_distinguishes_version_from_address() {
        for addr in [
            "https://x/y.tar.gz",
            "http://x/y.tar.gz",
            "/opt/pkg/s.tar.gz",
            "./packages/s.tar.gz",
            "../packages/s.tar.gz",
            "git@github.com:o/r.git",
        ] {
            assert!(looks_like_address(addr), "{addr} should be an address");
        }
        for version in ["0.1.24", "v0.1.24-alpha", "1.2.3-rc.1"] {
            assert!(
                !looks_like_address(version),
                "{version} should be a version"
            );
        }
    }

    fn project_with_sys(root: &std::path::Path, sys_yaml: &str) {
        std::fs::create_dir_all(root.join("_gal")).unwrap();
        std::fs::write(root.join("_gal/work.gxl"), "mod envs {}\nmod main {}\n").unwrap();
        std::fs::write(
            root.join("ops-prj.yml"),
            format!(
                "name: smoke\nwork_envs:\n  dep_root: ''\n  deps: []\nsys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n{sys_yaml}"
            ),
        )
        .unwrap();
    }

    #[test]
    fn resolve_target_addr_passes_address_through_or_renders_version_template() {
        let tmp = TempDir::new().unwrap();
        project_with_sys(
            tmp.path(),
            "  version: 0.1.22\n  addr:\n    url: https://gh/o/r/releases/download/v{version}/s-v{version}.tar.gz\n",
        );
        let project = OpsProject::load(tmp.path()).unwrap();

        // 地址：原样透传（所有系统同一份）。
        assert_eq!(
            resolve_target_addr(&project, "web-stack", "https://mirror/s.tar.gz").unwrap(),
            "https://mirror/s.tar.gz"
        );
        // 版本：按该系统自己的 addr 模板渲染（`--to` 覆盖声明版本）。
        assert_eq!(
            resolve_target_addr(&project, "web-stack", "0.1.24").unwrap(),
            "https://gh/o/r/releases/download/v0.1.24/s-v0.1.24.tar.gz"
        );
        // 未知系统：报错，不静默。
        assert!(resolve_target_addr(&project, "nope", "0.1.24").is_err());
    }

    #[test]
    fn resolve_target_addr_errors_when_version_given_but_no_placeholder() {
        let tmp = TempDir::new().unwrap();
        project_with_sys(tmp.path(), "  addr:\n    url: https://gh/o/r/s.tar.gz\n");
        let project = OpsProject::load(tmp.path()).unwrap();

        // 给了版本但 addr 没占位：报错（而不是拿字面量去下载）。
        let err = resolve_target_addr(&project, "web-stack", "0.1.24").unwrap_err();
        assert!(err.to_string().contains("没有 {version} 占位"), "{err}");
        // 同一系统仍可传完整地址。
        assert_eq!(
            resolve_target_addr(&project, "web-stack", "https://x/y.tar.gz").unwrap(),
            "https://x/y.tar.gz"
        );
    }

    #[test]
    fn record_roundtrips_through_disk() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nested/upgrade.json");
        let record = make_record(
            UpgradeStep::Pull,
            UpgradeStatus::RolledBack,
            Some("/tmp/backup.tar.gz".into()),
            "sys=0.1.0".into(),
            String::new(),
            "boom".into(),
        );
        write_record(&path, &record).unwrap();
        assert_eq!(read_record(&path).unwrap().unwrap(), record);
        assert!(
            read_record(&tmp.path().join("missing.json"))
                .unwrap()
                .is_none()
        );
    }

    /// 可编排的假运行时：记录调用顺序，在**首次**命中 `(步骤[, 系统])` 时注入一次失败。
    struct FakeRuntime {
        fail_once: Mutex<Option<(UpgradeStep, Option<String>)>>,
        calls: Mutex<Vec<String>>,
        version: String,
    }

    impl FakeRuntime {
        fn new(fail_once: Option<(UpgradeStep, Option<String>)>) -> Self {
            Self {
                fail_once: Mutex::new(fail_once),
                calls: Mutex::new(Vec::new()),
                version: "0.1.0".to_string(),
            }
        }
        fn record(&self, name: &str) -> MainResult<()> {
            self.calls.lock().unwrap().push(name.to_string());
            Ok(())
        }
        fn maybe_fail(&self, step: UpgradeStep, sys: &str) -> MainResult<()> {
            let mut guard = self.fail_once.lock().unwrap();
            let hit = match guard.as_ref() {
                Some((wanted, who)) if *wanted == step => {
                    who.as_deref().map(|w| w == sys).unwrap_or(true)
                }
                _ => false,
            };
            if hit {
                *guard = None;
                return Err(MainReason::logic_detail(format!(
                    "injected failure at {}",
                    step.as_str()
                )));
            }
            Ok(())
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl UpgradeRuntime for FakeRuntime {
        async fn diagnose(&self) -> MainResult<()> {
            self.maybe_fail(UpgradeStep::Diagnose, "")?;
            self.record("diagnose")
        }
        async fn backup(&self) -> MainResult<String> {
            self.maybe_fail(UpgradeStep::Backup, "")?;
            self.record("backup")?;
            Ok("/tmp/fake-backup.tar.gz".to_string())
        }
        async fn apply(&self, sys: &str) -> MainResult<()> {
            self.maybe_fail(UpgradeStep::Apply, sys)?;
            self.record(&format!("apply:{sys}"))
        }
        async fn regenerate(&self, sys: &str) -> MainResult<()> {
            self.maybe_fail(UpgradeStep::Regenerate, sys)?;
            self.record(&format!("regenerate:{sys}"))
        }
        async fn pull(&self, sys: &str) -> MainResult<()> {
            self.maybe_fail(UpgradeStep::Pull, sys)?;
            self.record(&format!("pull:{sys}"))
        }
        async fn up(&self, sys: &str) -> MainResult<()> {
            self.maybe_fail(UpgradeStep::Up, sys)?;
            self.record(&format!("up:{sys}"))
        }
        async fn status(&self, sys: &str) -> MainResult<()> {
            self.record(&format!("status:{sys}"))
        }
        async fn health(&self) -> MainResult<()> {
            self.maybe_fail(UpgradeStep::Health, "")?;
            self.record("health")
        }
        async fn restore(&self, backup: &str) -> MainResult<()> {
            self.record(&format!("restore:{backup}"))
        }
        async fn version_of(&self, _sys: &str) -> MainResult<String> {
            Ok(self.version.clone())
        }
    }

    fn systems() -> Vec<String> {
        vec!["s1".to_string(), "s2".to_string()]
    }

    #[tokio::test]
    async fn dry_run_makes_no_calls_and_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("upgrade.json");
        let runtime = FakeRuntime::new(None);

        let outcome = run_upgrade(
            &runtime,
            &systems(),
            "http://example.com/pkg.tar.gz",
            FailurePolicy::RollbackAll,
            true,
            true,
            &path,
        )
        .await
        .unwrap();

        assert!(outcome.dry_run);
        assert!(outcome.record.is_none());
        assert_eq!(outcome.exit_code(), 0);
        assert!(
            runtime.calls().is_empty(),
            "dry-run must not touch anything"
        );
        assert!(!path.exists(), "dry-run must not write the record");
        // 计划里带上目标与策略，供人核对。
        assert_eq!(outcome.plan.systems, systems());
        assert_eq!(outcome.plan.policy, "rollback-all");
        assert!(outcome.plan.health);
    }

    #[tokio::test]
    async fn success_runs_phases_in_order_and_records_succeeded() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("upgrade.json");
        let runtime = FakeRuntime::new(None);

        let outcome = run_upgrade(
            &runtime,
            &systems(),
            "http://example.com/pkg.tar.gz",
            FailurePolicy::RollbackAll,
            true,
            false,
            &path,
        )
        .await
        .unwrap();

        assert_eq!(outcome.exit_code(), 0);
        let record = outcome.record.unwrap();
        assert_eq!(record.status, "succeeded");
        assert_eq!(record.step, "done");
        assert_eq!(record.backup_id.as_deref(), Some("/tmp/fake-backup.tar.gz"));
        assert_eq!(record.from_version, "s1=0.1.0,s2=0.1.0");
        // 阶段序：apply 全做完再 regenerate，…，pull 全做完再 up（先拉后停）。
        assert_eq!(
            runtime.calls(),
            vec![
                "diagnose",
                "backup",
                "apply:s1",
                "apply:s2",
                "regenerate:s1",
                "regenerate:s2",
                "pull:s1",
                "pull:s2",
                "up:s1",
                "up:s2",
                "health",
                "status:s1",
                "status:s2",
            ]
        );
        // 状态文件也落了。
        assert_eq!(read_record(&path).unwrap().unwrap().status, "succeeded");
    }

    #[tokio::test]
    async fn halt_on_pull_failure_stops_without_restore() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("upgrade.json");
        let runtime = FakeRuntime::new(Some((UpgradeStep::Pull, None)));

        let outcome = run_upgrade(
            &runtime,
            &systems(),
            "http://example.com/pkg.tar.gz",
            FailurePolicy::Halt,
            true,
            false,
            &path,
        )
        .await
        .unwrap();

        assert_eq!(outcome.exit_code(), 1, "halt is a failure outcome");
        let record = outcome.record.unwrap();
        assert_eq!(record.status, "failed");
        assert_eq!(record.step, "pull");
        assert!(record.detail.contains("halt"));
        let calls = runtime.calls();
        assert!(
            !calls.iter().any(|c| c.starts_with("restore")),
            "halt must not restore"
        );
        assert!(
            !calls.iter().any(|c| c.starts_with("up:")),
            "halt must not switch"
        );
        // 但备份已经做过（回滚点已存在，交人处理）。
        assert!(calls.contains(&"backup".to_string()));
    }

    #[tokio::test]
    async fn rollback_all_restores_and_restarts_when_a_switch_fails() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("upgrade.json");
        // 只在第一次 `up s2` 失败；之后的 up（属于回滚）应成功。
        let runtime = FakeRuntime::new(Some((UpgradeStep::Up, Some("s2".to_string()))));

        let outcome = run_upgrade(
            &runtime,
            &systems(),
            "http://example.com/pkg.tar.gz",
            FailurePolicy::RollbackAll,
            true,
            false,
            &path,
        )
        .await
        .unwrap();

        let record = outcome.record.clone().unwrap();
        assert_eq!(record.status, "rolled_back");
        assert_eq!(record.step, "rollback");
        let calls = runtime.calls();
        assert!(calls.contains(&"restore:/tmp/fake-backup.tar.gz".to_string()));
        // 两次 up：切换（s1 成功 / s2 失败）+ 回滚后把 s1、s2 都重新拉起。
        assert_eq!(
            calls.iter().filter(|c| c.starts_with("up:")).count(),
            3,
            "switch(s1) + rollback(s1,s2)；失败的 up:s2 不记：{calls:?}"
        );
        // 回滚后重新探活（FakeRuntime 的 health 不再失败）。
        let at = calls
            .iter()
            .position(|c| c == "restore:/tmp/fake-backup.tar.gz")
            .unwrap();
        assert!(calls[at..].contains(&"health".to_string()));
    }

    #[tokio::test]
    async fn preflight_diagnose_failure_records_failed_without_backup() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("upgrade.json");
        let runtime = FakeRuntime::new(Some((UpgradeStep::Diagnose, None)));

        let outcome = run_upgrade(
            &runtime,
            &systems(),
            "http://example.com/pkg.tar.gz",
            FailurePolicy::RollbackAll,
            true,
            false,
            &path,
        )
        .await
        .unwrap();

        assert_eq!(outcome.exit_code(), 1);
        let record = outcome.record.unwrap();
        assert_eq!(record.step, "diagnose");
        assert!(record.backup_id.is_none());
        let calls = runtime.calls();
        assert!(!calls.contains(&"backup".to_string()));
    }

    #[tokio::test]
    async fn empty_systems_is_a_setup_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("upgrade.json");
        let runtime = FakeRuntime::new(None);
        let err = run_upgrade(
            &runtime,
            &[],
            "http://example.com/pkg.tar.gz",
            FailurePolicy::RollbackAll,
            false,
            false,
            &path,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("没有可升级的系统"), "{err}");
        assert!(runtime.calls().is_empty());
    }

    #[tokio::test]
    async fn json_contract_carries_record_and_exit_code_is_nonzero_on_failure() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("upgrade.json");
        let runtime = FakeRuntime::new(Some((UpgradeStep::Up, None)));
        let outcome = run_upgrade(
            &runtime,
            &systems(),
            "http://example.com/pkg.tar.gz",
            FailurePolicy::Halt,
            true,
            false,
            &path,
        )
        .await
        .unwrap();
        assert_eq!(outcome.exit_code(), 1);
        let json: serde_json::Value = serde_json::from_str(&outcome.to_json()).unwrap();
        assert_eq!(json["on_failure"], "halt");
        assert_eq!(json["record"]["status"], "failed");
        assert_eq!(json["dry_run"], false);
    }
}
