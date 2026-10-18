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
//! 与 `prj sync` 的分工：`update` 只覆盖**内容**；本模块在其上补事务边界（备份、拉取、切换、
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
use super::version::{self, VersionSpec, infer_channel};
use crate::accessor::accessor_for_default;
use crate::error::{MainError, MainReason};
use crate::self_update::ReleaseChannel;
use crate::system::lock::DeliverLock;

/// 失败发生时**做什么**（`--on-failure`）。
///
/// 默认值**尚未确定**，所以 CLI 目前要求显式给出 —— 见设计稿 §3.3。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FailurePolicy {
    /// 整工程原子：任一系统失败 → 全部回滚（含已成功的）。
    #[default]
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

/// 一个系统解析后的目标：地址 + 具体版本（地址型目标 / 非版本化目标没有版本）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub version: Option<String>,
    pub addr: String,
}

/// 本次事务的静态计划（`--dry-run` 也返回它）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UpgradePlan {
    pub systems: Vec<String>,
    pub target: String,
    pub policy: String,
    pub health: bool,
    /// 每系统解析出的目标地址（与 `systems` 同序）。
    pub resolved: Vec<String>,
    /// 每系统解析出的具体版本（与 `systems` 同序）；地址型目标为 `None`。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_versions: Vec<Option<String>>,
    /// 目标版本全部等于现场版本（且未 `--force`）→ 不执行事务。
    #[serde(default)]
    pub no_op: bool,
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
            "resolved": self.plan.resolved,
            "target_versions": self.plan.target_versions,
            "no_op": self.plan.no_op,
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
    /// 是否强制重跑：`--force` 时即使目标版本与现场相同，也不做 no-op。
    fn force(&self) -> bool {
        false
    }
    /// 前置体检（失败即拒，**不动现场**）。
    async fn diagnose(&self) -> MainResult<()>;
    /// 备份现场态，返回交给 [`UpgradeRuntime::restore`] 的备份标识（归档路径）。
    async fn backup(&self) -> MainResult<String>;
    /// 取包并覆盖到指定系统目录（只动磁盘内容，**不停服务**）。
    async fn apply(&self, sys: &str, target: &ResolvedTarget) -> MainResult<()>;
    /// 重渲染值 / `.env`（`sys resolve` + `localize`），为切换做准备。
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
    /// 读某系统当前版本（优先 `version.txt`，缺失时退回 `deliver.lock`）。
    async fn version_of(&self, sys: &str) -> MainResult<String>;
    /// 解析某系统的目标（`--to` 是地址则原样；版本 spec 用该系统 ref 的 `{version}` 模板渲染）。
    /// **只读**：在 diagnose/backup 之前调用，避免「计划一片绿、真跑才在 apply 炸」。
    async fn resolve_target(&self, sys: &str) -> MainResult<ResolvedTarget>;
}

/// 运行时动作（按 `kind` 分派到 docker compose / gx）—— app 层实现。
#[async_trait]
pub trait RuntimeDispatch: Send + Sync {
    /// `sys resolve` + `localize`（含渲染与 localize 阶段流程）。
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
    targets: &[ResolvedTarget],
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

    for (sys, target) in systems.iter().zip(targets) {
        step(UpgradeStep::Apply, sys, runtime.apply(sys, target)).await?;
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

/// 跑一次升级事务。**事务阶段失败**都落状态文件并返回 `Ok`（结局在 `record.status` 里）；
/// 目标解析 / 包存在性预检失败属于 setup error，直接返回 `Err`（不写状态）；
/// 无可升级系统、状态文件写失败也返回 `Err`。
pub async fn run_upgrade(
    runtime: &dyn UpgradeRuntime,
    systems: &[String],
    target: &str,
    policy: FailurePolicy,
    expect_health: bool,
    dry_run: bool,
    record_path: &Path,
) -> MainResult<UpgradeOutcome> {
    let mut plan = UpgradePlan {
        systems: systems.to_vec(),
        target: target.to_string(),
        policy: policy.as_str().to_string(),
        health: expect_health,
        resolved: Vec::new(),
        target_versions: Vec::new(),
        no_op: false,
    };

    if systems.is_empty() {
        return Err(MainReason::logic_detail(
            "没有可升级的系统：ops-prj.yml 里没有该系统，或它还没导入（先 `gops prj import`）",
        ));
    }

    // 目标解析放在**所有副作用之前**：`latest` 可能要访问版本目录，失败时不能已经备份/覆盖。
    let mut targets = Vec::with_capacity(systems.len());
    for sys in systems {
        targets.push(runtime.resolve_target(sys).await?);
    }
    plan.resolved = targets.iter().map(|t| t.addr.clone()).collect();
    plan.target_versions = targets.iter().map(|t| t.version.clone()).collect();

    // 先读一次现场版本：既用于状态文件的 from，也用于同版本 no-op 判定。
    let mut current_versions = Vec::with_capacity(systems.len());
    for sys in systems {
        current_versions.push(runtime.version_of(sys).await.unwrap_or_default());
    }
    let from_version = systems
        .iter()
        .zip(&current_versions)
        .map(|(sys, version)| format!("{sys}={version}"))
        .collect::<Vec<_>>()
        .join(",");

    let all_up_to_date = !runtime.force()
        && targets
            .iter()
            .zip(&current_versions)
            .all(|(target, current)| {
                target
                    .version
                    .as_deref()
                    .is_some_and(|version| version::same_version(current, version))
            });
    plan.no_op = all_up_to_date;

    if dry_run {
        return Ok(UpgradeOutcome {
            plan,
            dry_run: true,
            record: None,
            record_path: record_path.to_path_buf(),
        });
    }

    // 同版本 no-op：不备份、不覆盖、不拉取、不切换。
    if all_up_to_date {
        let record = make_record(
            UpgradeStep::Done,
            UpgradeStatus::Succeeded,
            None,
            from_version.clone(),
            from_version,
            format!("已是目标版本（no-op）：{}", plan.target),
        );
        write_record(record_path, &record)?;
        return Ok(UpgradeOutcome {
            plan,
            dry_run: false,
            record: Some(record),
            record_path: record_path.to_path_buf(),
        });
    }

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
    match run_phases(runtime, systems, &targets, expect_health).await {
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

/// 读某系统目录当前的**部署版本**（供状态文件记 `from`/`to`）。
///
/// **优先 `version.txt`**：它随交付包走、升级时会被覆盖更新。
/// 回退 `deliver.lock`：它是**交付时**生成的 git-ignored 产物、**不在包里** —— 升级不会更新它，
/// 因此它常常是**陈旧**的（本次现场实测：栈包已升到 0.1.23，`deliver.lock` 仍是 0.1.17），只作最后兜底。
pub fn read_sys_version(sys_dir: &Path) -> String {
    if let Some(version) = std::fs::read_to_string(sys_dir.join("version.txt"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return version;
    }
    if DeliverLock::path(sys_dir).exists()
        && let Ok(lock) = DeliverLock::load(sys_dir)
    {
        return lock.version().to_string();
    }
    String::new()
}

/// 事务开始前确认目标包存在。
///
/// 对本地路径是 `is_file`；对 HTTP 先 `HEAD`（部分服务器不支持时退回带 `Range` 的 `GET`）；
/// 对 git 地址用 `git ls-remote`。这一检查发生在 diagnose/backup 之前，避免「备份做完了、
/// apply 才发现包不存在」，多系统时也避免一部分系统已被覆盖。
async fn verify_target_exists(project_root: &Path, addr: &str) -> MainResult<()> {
    let addr = addr.trim();
    if addr.is_empty() {
        return Err(MainReason::logic_detail("目标包地址为空"));
    }
    if addr.starts_with("http://") || addr.starts_with("https://") {
        return verify_http_target(addr).await;
    }
    if addr.starts_with("git@") || addr.ends_with(".git") {
        return verify_git_target(addr).await;
    }

    let path = Path::new(addr);
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };
    if full.is_file() {
        return Ok(());
    }
    if full.exists() {
        return Err(MainReason::resource_detail(format!(
            "目标包不是文件：{}\n  addr={addr}",
            full.display()
        )));
    }
    Err(MainReason::resource_detail(format!(
        "目标包不存在：{addr}\n  本地路径按项目根解析：{}",
        full.display()
    )))
}

async fn verify_http_target(url: &str) -> MainResult<()> {
    let client = reqwest::Client::builder()
        .user_agent("gops")
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| MainReason::resource_detail(format!("创建 HTTP 客户端失败：{e}")))?;

    let mut head = client.head(url);
    if let Some(token) = github_token_for(url) {
        head = head.bearer_auth(token);
    }
    let resp = head
        .send()
        .await
        .map_err(|e| MainReason::resource_detail(format!("检查目标包失败（HEAD）{url}: {e}")))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }

    // 一些静态服务器/对象存储不接受 HEAD（405/501），或有边缘策略返回 403；
    // 用最小 Range GET 再确认一次，避免把“不支持 HEAD”误判成“包不存在”。
    if matches!(status.as_u16(), 403 | 405 | 501) {
        let mut get = client.get(url).header(reqwest::header::RANGE, "bytes=0-0");
        if let Some(token) = github_token_for(url) {
            get = get.bearer_auth(token);
        }
        let resp = get.send().await.map_err(|e| {
            MainReason::resource_detail(format!("检查目标包失败（GET Range）{url}: {e}"))
        })?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        return Err(http_status_error(url, status, "GET Range"));
    }

    Err(http_status_error(url, status, "HEAD"))
}

fn http_status_error(url: &str, status: reqwest::StatusCode, method: &str) -> MainError {
    let hint = match status.as_u16() {
        401 | 403 => "（可能需要鉴权；GitHub 可设置 GITHUB_TOKEN）",
        404 => "（包不存在或无权访问）",
        _ => "",
    };
    MainReason::resource_detail(format!(
        "目标包不存在或不可访问：{url}\n  {method} → HTTP {status}{hint}"
    ))
}

fn github_token_for(url: &str) -> Option<String> {
    let is_github = url.starts_with("https://github.com/")
        || url.starts_with("http://github.com/")
        || url.starts_with("https://api.github.com/");
    if !is_github {
        return None;
    }
    version::github_token()
}

async fn verify_git_target(addr: &str) -> MainResult<()> {
    let mut command = tokio::process::Command::new("git");
    command
        .args(["ls-remote", "--heads", "--tags", addr])
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .map_err(|_| MainReason::resource_detail(format!("检查 git 目标超时（30s）：{addr}")))?
        .map_err(|e| MainReason::resource_detail(format!("检查 git 目标失败：{e}")))?;
    if !out.status.success() {
        return Err(MainReason::resource_detail(format!(
            "git 目标不可访问：{addr}\n  {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    if out.stdout.is_empty() {
        return Err(MainReason::resource_detail(format!(
            "git 目标没有任何 heads/tags：{addr}"
        )));
    }
    Ok(())
}

/// 本次升级目标：完整地址，或版本 spec（`latest` / `patch` / ... / 精确版本）。
#[derive(Clone, Debug)]
pub enum UpgradeTarget {
    Address(String),
    Spec(VersionSpec),
}

impl UpgradeTarget {
    /// 供计划 / 状态文件展示的原文。
    pub fn label(&self) -> String {
        match self {
            Self::Address(addr) => addr.clone(),
            Self::Spec(spec) => spec.as_str().to_string(),
        }
    }
}

/// 真实运行时：接本 crate 既有实现 + 注入的 [`RuntimeDispatch`]。
///
/// `target` 是 `--to` 的地址或版本 spec；关键字 spec 会先解析成**具体版本**，
/// 再按该系统 addr 模板 `{version}` 渲染。
pub struct ProjectRuntime {
    project_root: PathBuf,
    target: UpgradeTarget,
    channel: Option<ReleaseChannel>,
    force: bool,
    health_cmd: Option<String>,
    health_timeout: Duration,
    dispatch: Arc<dyn RuntimeDispatch>,
}

impl ProjectRuntime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project_root: PathBuf,
        target: UpgradeTarget,
        channel: Option<ReleaseChannel>,
        force: bool,
        health_cmd: Option<String>,
        health_timeout: Duration,
        dispatch: Arc<dyn RuntimeDispatch>,
    ) -> Self {
        Self {
            project_root,
            target,
            channel,
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
    fn force(&self) -> bool {
        self.force
    }

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
        let report = project.backup()?;
        Ok(report.archive.display().to_string())
    }

    async fn resolve_target(&self, sys: &str) -> MainResult<ResolvedTarget> {
        let project = OpsProject::load(&self.project_root)?;
        let sys_ref = project
            .conf()
            .sys_models()
            .iter()
            .find(|s| s.sys().name().as_str() == sys)
            .ok_or_else(|| MainReason::logic_detail(format!("ops-prj.yml 里没有系统 `{sys}`")))?;

        let resolved = match &self.target {
            UpgradeTarget::Address(addr) => ResolvedTarget {
                version: None,
                addr: addr.trim().to_string(),
            },
            UpgradeTarget::Spec(spec) => {
                let current = read_sys_version(&self.sys_dir(sys));
                match spec {
                    VersionSpec::Declared => {
                        let version = sys_ref.declared_version().map(str::to_string);
                        let addr = sys_ref.resolved_addr()?;
                        ResolvedTarget { version, addr }
                    }
                    VersionSpec::Exact(version) => {
                        let addr = sys_ref.resolved_addr_with(version)?;
                        ResolvedTarget {
                            version: Some(version.clone()),
                            addr,
                        }
                    }
                    _ => {
                        let channel = self.channel.unwrap_or_else(|| infer_channel(&current));
                        // 有清单/API 直接枚举；普通 HTTP 无清单时，按 patch/minor/major
                        // 规则生成候选 URL 并 HEAD 探测。
                        let entries = match version::try_list_versions(&self.project_root, sys_ref)
                            .await?
                        {
                            Some(entries) => entries,
                            None => {
                                version::probe_versions(sys_ref, spec, &current, channel).await?
                            }
                        };
                        let entry =
                            version::select_version(&entries, spec, Some(&current), channel)?;
                        let version = entry.version.to_string();
                        let addr = sys_ref.resolved_addr_with(&version)?;
                        ResolvedTarget {
                            version: Some(version),
                            addr,
                        }
                    }
                }
            }
        };

        // 目标包必须在**事务开始前**存在：否则要等到 backup 之后的 apply 才报错，
        // 多系统时还可能已经覆盖了一部分。
        verify_target_exists(&self.project_root, &resolved.addr).await?;
        Ok(resolved)
    }

    async fn apply(&self, sys: &str, target: &ResolvedTarget) -> MainResult<()> {
        let project = OpsProject::load(&self.project_root)?;
        let options = DownloadOptions::from((self.force, ValueDict::default()));
        let sys_src = fetch_and_prepare(
            project.paths().clone(),
            &target.addr,
            accessor_for_default(),
            &options,
        )
        .await?;
        let target_dir = self.sys_dir(sys);
        let _ = apply_overlay(sys, &sys_src, &target_dir)?;
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
        Ok(read_sys_version(&self.sys_dir(sys)))
    }
}

/// `--health-cmd` 缺省超时（秒）。
pub const DEFAULT_HEALTH_TIMEOUT_SECS: u64 = 60;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tempfile::TempDir;

    #[test]
    fn read_sys_version_prefers_version_txt_over_stale_deliver_lock() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        // 升级后现场的真实形状：version.txt 是新的，deliver.lock 是交付时的旧的。
        std::fs::write(dir.join("version.txt"), "0.1.23\n").unwrap();
        std::fs::write(
            dir.join("deliver.lock"),
            "lockfile_version: 1\nname: x\nversion: 0.1.17\nkind: docker-compose\ngenerated_at: t\nmodules: []\nhashes: {}\n",
        )
        .unwrap();
        assert_eq!(
            read_sys_version(dir),
            "0.1.23",
            "must not trust the stale deliver.lock"
        );
        // 没有 version.txt 才回退 deliver.lock。
        std::fs::remove_file(dir.join("version.txt")).unwrap();
        assert_eq!(read_sys_version(dir), "0.1.17");
        // 都没有 → 空串（不编造）。
        std::fs::remove_file(dir.join("deliver.lock")).unwrap();
        assert_eq!(read_sys_version(dir), "");
    }

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
    fn failure_policy_defaults_to_rollback_all() {
        assert_eq!(FailurePolicy::default(), FailurePolicy::RollbackAll);
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

    struct NoopDispatch;

    #[async_trait]
    impl RuntimeDispatch for NoopDispatch {
        async fn localize(&self, _sys_dir: &Path) -> MainResult<()> {
            Ok(())
        }
        async fn pull(&self, _sys_dir: &Path) -> MainResult<()> {
            Ok(())
        }
        async fn up(&self, _sys_dir: &Path) -> MainResult<()> {
            Ok(())
        }
        async fn status(&self, _sys_dir: &Path) -> MainResult<()> {
            Ok(())
        }
    }

    fn runtime(
        root: &Path,
        target: UpgradeTarget,
        channel: Option<ReleaseChannel>,
    ) -> ProjectRuntime {
        ProjectRuntime::new(
            root.to_path_buf(),
            target,
            channel,
            false,
            None,
            Duration::from_secs(1),
            Arc::new(NoopDispatch),
        )
    }

    #[tokio::test]
    async fn project_runtime_resolves_exact_declared_latest_and_bump() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        for name in [
            "s-v0.9.0.tar.gz",
            "s-v0.9.1.tar.gz",
            "s-v0.10.0-alpha.tar.gz",
            "s-v0.10.0.tar.gz",
            "pkg.tar.gz",
        ] {
            std::fs::write(root.join(name), "x").unwrap();
        }
        project_with_sys(
            root,
            "  version: 0.9.0\n  addr:\n    path: ./s-v{version}.tar.gz\n",
        );
        std::fs::create_dir_all(root.join("web-stack")).unwrap();
        std::fs::write(root.join("web-stack/version.txt"), "0.9.0\n").unwrap();

        // 精确版本：直接渲染 addr。
        let target = runtime(
            root,
            UpgradeTarget::Spec(VersionSpec::Exact("0.10.0".to_string())),
            None,
        )
        .resolve_target("web-stack")
        .await
        .unwrap();
        assert_eq!(target.version.as_deref(), Some("0.10.0"));
        assert_eq!(target.addr, "./s-v0.10.0.tar.gz");

        // 声明版本：使用 ops-prj.yml 的 version。
        let target = runtime(root, UpgradeTarget::Spec(VersionSpec::Declared), None)
            .resolve_target("web-stack")
            .await
            .unwrap();
        assert_eq!(target.version.as_deref(), Some("0.9.0"));
        assert_eq!(target.addr, "./s-v0.9.0.tar.gz");

        // latest：按当前版本后缀推断 stable。
        let target = runtime(root, UpgradeTarget::Spec(VersionSpec::Latest), None)
            .resolve_target("web-stack")
            .await
            .unwrap();
        assert_eq!(target.version.as_deref(), Some("0.10.0"));

        // patch：同 minor 的下一个可用版本（0.9.0 → 0.9.1）。
        let target = runtime(root, UpgradeTarget::Spec(VersionSpec::Patch), None)
            .resolve_target("web-stack")
            .await
            .unwrap();
        assert_eq!(target.version.as_deref(), Some("0.9.1"));

        // 地址：原样透传，不带版本。
        let target = runtime(
            root,
            UpgradeTarget::Address("./pkg.tar.gz".to_string()),
            None,
        )
        .resolve_target("web-stack")
        .await
        .unwrap();
        assert!(target.version.is_none());
        assert_eq!(target.addr, "./pkg.tar.gz");
    }

    #[tokio::test]
    async fn verify_target_exists_checks_local_files() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("pkg.tar.gz"), "x").unwrap();
        verify_target_exists(tmp.path(), "./pkg.tar.gz")
            .await
            .unwrap();

        let err = verify_target_exists(tmp.path(), "./missing.tar.gz")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("目标包不存在"), "{err}");

        std::fs::create_dir_all(tmp.path().join("dir.tar.gz")).unwrap();
        let err = verify_target_exists(tmp.path(), "./dir.tar.gz")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("不是文件"), "{err}");
    }

    #[tokio::test]
    async fn verify_target_exists_rejects_empty_and_missing_git() {
        let err = verify_target_exists(Path::new("."), "  ")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("为空"), "{err}");

        let err = verify_target_exists(Path::new("."), "/definitely/missing/repo.git")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("git 目标不可访问")
                || err.to_string().contains("git 目标没有任何"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn verify_target_exists_checks_http_status_and_head_fallback() {
        let mut server = mockito::Server::new_async().await;
        let ok = server
            .mock("HEAD", "/pkg.tar.gz")
            .with_status(200)
            .create_async()
            .await;
        let missing = server
            .mock("HEAD", "/missing.tar.gz")
            .with_status(404)
            .create_async()
            .await;
        let no_head = server
            .mock("HEAD", "/no-head.tar.gz")
            .with_status(405)
            .create_async()
            .await;
        let range = server
            .mock("GET", "/no-head.tar.gz")
            .match_header("range", "bytes=0-0")
            .with_status(206)
            .create_async()
            .await;

        verify_target_exists(Path::new("."), &format!("{}/pkg.tar.gz", server.url()))
            .await
            .unwrap();
        let err = verify_target_exists(Path::new("."), &format!("{}/missing.tar.gz", server.url()))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("HTTP 404"), "{err}");
        verify_target_exists(Path::new("."), &format!("{}/no-head.tar.gz", server.url()))
            .await
            .unwrap();

        ok.assert_async().await;
        missing.assert_async().await;
        no_head.assert_async().await;
        range.assert_async().await;
    }

    #[tokio::test]
    async fn project_runtime_rejects_missing_package_and_unresolvable_specs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("s-v0.9.0.tar.gz"), "x").unwrap();
        project_with_sys(
            root,
            "  version: 0.9.0\n  addr:\n    path: ./s-v{version}.tar.gz\n",
        );
        std::fs::create_dir_all(root.join("web-stack")).unwrap();
        std::fs::write(root.join("web-stack/version.txt"), "0.9.0\n").unwrap();

        // 精确版本存在时通过。
        runtime(
            root,
            UpgradeTarget::Spec(VersionSpec::Exact("0.9.0".to_string())),
            None,
        )
        .resolve_target("web-stack")
        .await
        .unwrap();

        // 精确版本不存在 → 预检失败。
        let err = runtime(
            root,
            UpgradeTarget::Spec(VersionSpec::Exact("0.10.0".to_string())),
            None,
        )
        .resolve_target("web-stack")
        .await
        .unwrap_err();
        assert!(err.to_string().contains("目标包不存在"), "{err}");

        // 地址不存在 → 预检失败。
        let err = runtime(
            root,
            UpgradeTarget::Address("./missing.tar.gz".to_string()),
            None,
        )
        .resolve_target("web-stack")
        .await
        .unwrap_err();
        assert!(err.to_string().contains("目标包不存在"), "{err}");

        // 关键字 spec + 没有 {version} 占位 → 明确拒绝。
        let tmp2 = TempDir::new().unwrap();
        project_with_sys(tmp2.path(), "  addr:\n    path: ./s.tar.gz\n");
        std::fs::create_dir_all(tmp2.path().join("web-stack")).unwrap();
        std::fs::write(tmp2.path().join("web-stack/version.txt"), "0.9.0\n").unwrap();
        std::fs::write(tmp2.path().join("s.tar.gz"), "x").unwrap();
        let err = runtime(tmp2.path(), UpgradeTarget::Spec(VersionSpec::Latest), None)
            .resolve_target("web-stack")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("没有 {version} 占位"), "{err}");

        // 声明版本缺失、但 addr 有占位 → 明确拒绝。
        let tmp3 = TempDir::new().unwrap();
        project_with_sys(tmp3.path(), "  addr:\n    path: ./s-v{version}.tar.gz\n");
        std::fs::create_dir_all(tmp3.path().join("web-stack")).unwrap();
        let err = runtime(
            tmp3.path(),
            UpgradeTarget::Spec(VersionSpec::Declared),
            None,
        )
        .resolve_target("web-stack")
        .await
        .unwrap_err();
        assert!(err.to_string().contains("没写 `version`"), "{err}");
    }

    #[tokio::test]
    async fn project_runtime_probes_patch_for_generic_http() {
        let mut server = mockito::Server::new_async().await;
        let manifest_json = server
            .mock("GET", "/wist-center-stack/versions.json")
            .with_status(404)
            .create_async()
            .await;
        let manifest_txt = server
            .mock("GET", "/wist-center-stack/versions.txt")
            .with_status(404)
            .create_async()
            .await;
        let package = server
            .mock(
                "HEAD",
                "/wist-center-stack/v0.1.10-alpha/wist-center-stack-v0.1.10-alpha.tar.gz",
            )
            .with_status(200)
            .expect(2)
            .create_async()
            .await;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let addr = format!(
            "url: {}/wist-center-stack/v{{version}}/wist-center-stack-v{{version}}.tar.gz",
            server.url()
        );
        project_with_sys(
            root,
            &format!("  version: 0.1.9-alpha\n  addr:\n    {addr}\n"),
        );
        std::fs::create_dir_all(root.join("web-stack")).unwrap();
        std::fs::write(root.join("web-stack/version.txt"), "0.1.9-alpha\n").unwrap();

        let target = runtime(
            root,
            UpgradeTarget::Spec(VersionSpec::Patch),
            Some(ReleaseChannel::Alpha),
        )
        .resolve_target("web-stack")
        .await
        .unwrap();
        assert_eq!(target.version.as_deref(), Some("0.1.10-alpha"));
        assert!(target.addr.contains("v0.1.10-alpha"));

        manifest_json.assert_async().await;
        manifest_txt.assert_async().await;
        package.assert_async().await;
    }

    #[tokio::test]
    async fn project_runtime_latest_still_requires_catalog_for_generic_http() {
        let mut server = mockito::Server::new_async().await;
        let manifest_json = server
            .mock("GET", "/wist-center-stack/versions.json")
            .with_status(404)
            .create_async()
            .await;
        let manifest_txt = server
            .mock("GET", "/wist-center-stack/versions.txt")
            .with_status(404)
            .create_async()
            .await;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let addr = format!(
            "url: {}/wist-center-stack/v{{version}}/wist-center-stack-v{{version}}.tar.gz",
            server.url()
        );
        project_with_sys(
            root,
            &format!("  version: 0.1.9-alpha\n  addr:\n    {addr}\n"),
        );
        std::fs::create_dir_all(root.join("web-stack")).unwrap();
        std::fs::write(root.join("web-stack/version.txt"), "0.1.9-alpha\n").unwrap();

        let err = runtime(
            root,
            UpgradeTarget::Spec(VersionSpec::Latest),
            Some(ReleaseChannel::Alpha),
        )
        .resolve_target("web-stack")
        .await
        .unwrap_err();
        assert!(err.to_string().contains("latest"), "{err}");

        manifest_json.assert_async().await;
        manifest_txt.assert_async().await;
    }

    #[tokio::test]
    async fn project_runtime_refuses_implicit_downgrade() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("s-v0.10.0.tar.gz"), "x").unwrap();
        project_with_sys(root, "  addr:\n    path: ./s-v{version}.tar.gz\n");
        std::fs::create_dir_all(root.join("web-stack")).unwrap();
        std::fs::write(root.join("web-stack/version.txt"), "0.11.0\n").unwrap();

        let err = runtime(root, UpgradeTarget::Spec(VersionSpec::Latest), None)
            .resolve_target("web-stack")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("拒绝隐式降级"), "{err}");
    }

    #[tokio::test]
    async fn run_upgrade_dry_run_reports_resolved_versions_and_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("s-v0.9.0.tar.gz"), "x").unwrap();
        std::fs::write(root.join("s-v0.10.0.tar.gz"), "x").unwrap();
        project_with_sys(root, "  addr:\n    path: ./s-v{version}.tar.gz\n");
        std::fs::create_dir_all(root.join("web-stack")).unwrap();
        std::fs::write(root.join("web-stack/version.txt"), "0.9.0\n").unwrap();
        let record = root.join("upgrade.json");

        let outcome = run_upgrade(
            &runtime(root, UpgradeTarget::Spec(VersionSpec::Latest), None),
            &["web-stack".to_string()],
            "latest",
            FailurePolicy::Halt,
            false,
            true,
            &record,
        )
        .await
        .unwrap();

        assert!(outcome.dry_run);
        assert_eq!(
            outcome.plan.target_versions,
            vec![Some("0.10.0".to_string())]
        );
        assert!(outcome.plan.resolved[0].contains("s-v0.10.0.tar.gz"));
        assert!(!record.exists(), "dry-run must not write the state file");

        // `--json` 也带解析出的具体版本，编排器可直接消费。
        let json: serde_json::Value = serde_json::from_str(&outcome.to_json()).unwrap();
        assert_eq!(json["target_versions"][0], "0.10.0");
    }

    #[tokio::test]
    async fn run_upgrade_dry_run_reports_no_op_with_project_runtime() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("s-v0.9.0.tar.gz"), "x").unwrap();
        project_with_sys(
            root,
            "  version: 0.9.0\n  addr:\n    path: ./s-v{version}.tar.gz\n",
        );
        std::fs::create_dir_all(root.join("web-stack")).unwrap();
        std::fs::write(root.join("web-stack/version.txt"), "0.9.0\n").unwrap();
        let record = root.join("upgrade.json");

        let outcome = run_upgrade(
            &runtime(root, UpgradeTarget::Spec(VersionSpec::Declared), None),
            &["web-stack".to_string()],
            "declared",
            FailurePolicy::Halt,
            false,
            true,
            &record,
        )
        .await
        .unwrap();

        assert!(outcome.plan.no_op);
        assert_eq!(
            outcome.plan.target_versions,
            vec![Some("0.9.0".to_string())]
        );
        assert!(!record.exists());
    }

    #[tokio::test]
    async fn project_runtime_infers_alpha_channel_and_honors_override() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        for name in [
            "s-v0.10.0-alpha.tar.gz",
            "s-v0.10.0-beta.tar.gz",
            "s-v0.10.0.tar.gz",
        ] {
            std::fs::write(root.join(name), "x").unwrap();
        }
        project_with_sys(root, "  addr:\n    path: ./s-v{version}.tar.gz\n");
        std::fs::create_dir_all(root.join("web-stack")).unwrap();
        std::fs::write(root.join("web-stack/version.txt"), "0.10.0-alpha\n").unwrap();

        // 缺省 channel 从当前版本后缀推断为 alpha。
        let target = runtime(root, UpgradeTarget::Spec(VersionSpec::Latest), None)
            .resolve_target("web-stack")
            .await
            .unwrap();
        assert_eq!(target.version.as_deref(), Some("0.10.0-alpha"));

        // 显式 --channel beta 覆盖推断。
        let target = runtime(
            root,
            UpgradeTarget::Spec(VersionSpec::Latest),
            Some(ReleaseChannel::Beta),
        )
        .resolve_target("web-stack")
        .await
        .unwrap();
        assert_eq!(target.version.as_deref(), Some("0.10.0-beta"));
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
        version_by_sys: Mutex<HashMap<String, String>>,
        resolved_version: Option<String>,
        force: bool,
        resolve_err: bool,
    }

    impl FakeRuntime {
        fn new(fail_once: Option<(UpgradeStep, Option<String>)>) -> Self {
            Self {
                fail_once: Mutex::new(fail_once),
                calls: Mutex::new(Vec::new()),
                version: "0.1.0".to_string(),
                version_by_sys: Mutex::new(HashMap::new()),
                resolved_version: None,
                force: false,
                resolve_err: false,
            }
        }
        fn with_current_version(mut self, version: &str) -> Self {
            self.version = version.to_string();
            self
        }
        fn with_sys_version(self, sys: &str, version: &str) -> Self {
            self.version_by_sys
                .lock()
                .unwrap()
                .insert(sys.to_string(), version.to_string());
            self
        }
        /// 让 `resolve_target` 报错（验证 dry-run 会因目标不可解析而拒）。
        fn resolving_err(mut self) -> Self {
            self.resolve_err = true;
            self
        }
        /// 目标解析出的具体版本（用于 no-op 测试）。
        fn with_resolved_version(mut self, version: &str) -> Self {
            self.resolved_version = Some(version.to_string());
            self
        }
        fn with_force(mut self, force: bool) -> Self {
            self.force = force;
            self
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
        fn force(&self) -> bool {
            self.force
        }
        async fn diagnose(&self) -> MainResult<()> {
            self.maybe_fail(UpgradeStep::Diagnose, "")?;
            self.record("diagnose")
        }
        async fn backup(&self) -> MainResult<String> {
            self.maybe_fail(UpgradeStep::Backup, "")?;
            self.record("backup")?;
            Ok("/tmp/fake-backup.tar.gz".to_string())
        }
        async fn apply(&self, sys: &str, _target: &ResolvedTarget) -> MainResult<()> {
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
        async fn version_of(&self, sys: &str) -> MainResult<String> {
            Ok(self
                .version_by_sys
                .lock()
                .unwrap()
                .get(sys)
                .cloned()
                .unwrap_or_else(|| self.version.clone()))
        }
        async fn resolve_target(&self, _sys: &str) -> MainResult<ResolvedTarget> {
            // 只读查询，**不记入 calls**（dry-run 会调它，但不能因此算「动了现场」）。
            if self.resolve_err {
                return Err(MainReason::logic_detail("cannot resolve target (test)"));
            }
            Ok(ResolvedTarget {
                version: self.resolved_version.clone(),
                addr: "https://example.com/resolved.tar.gz".to_string(),
            })
        }
    }

    fn systems() -> Vec<String> {
        vec!["s1".to_string(), "s2".to_string()]
    }

    #[tokio::test]
    async fn dry_run_fails_when_target_cannot_be_resolved() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("upgrade.json");
        let runtime = FakeRuntime::new(None).resolving_err();
        let err = run_upgrade(
            &runtime,
            &systems(),
            "0.1.24",
            FailurePolicy::Halt,
            false,
            true,
            &path,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("cannot resolve target"), "{err}");
        assert!(!path.exists(), "dry-run must not write the record");
    }

    #[tokio::test]
    async fn target_resolution_failure_is_preflight_for_real_run() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("upgrade.json");
        let runtime = FakeRuntime::new(None).resolving_err();
        let err = run_upgrade(
            &runtime,
            &systems(),
            "0.1.24",
            FailurePolicy::RollbackAll,
            false,
            false,
            &path,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("cannot resolve target"), "{err}");
        assert!(
            runtime.calls().is_empty(),
            "resolution happens before diagnose/backup"
        );
        assert!(!path.exists(), "no state file for a preflight failure");
    }

    #[tokio::test]
    async fn no_op_when_all_targets_already_deployed() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("upgrade.json");
        let runtime = FakeRuntime::new(None).with_resolved_version("0.1.0");

        let outcome = run_upgrade(
            &runtime,
            &systems(),
            "0.1.0",
            FailurePolicy::RollbackAll,
            true,
            false,
            &path,
        )
        .await
        .unwrap();

        assert!(outcome.plan.no_op);
        assert_eq!(outcome.exit_code(), 0);
        let json: serde_json::Value = serde_json::from_str(&outcome.to_json()).unwrap();
        assert_eq!(json["no_op"], true);
        let record = outcome.record.unwrap();
        assert_eq!(record.status, "succeeded");
        assert_eq!(record.step, "done");
        assert_eq!(record.from_version, "s1=0.1.0,s2=0.1.0");
        assert_eq!(record.to_version, "s1=0.1.0,s2=0.1.0");
        assert!(record.backup_id.is_none());
        assert!(record.detail.contains("no-op"), "{}", record.detail);
        assert!(
            runtime.calls().is_empty(),
            "no-op must not run diagnose/backup/apply: {:?}",
            runtime.calls()
        );
        assert!(path.exists(), "no-op should still record the outcome");
    }

    #[tokio::test]
    async fn dry_run_reports_no_op_without_side_effects() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("upgrade.json");
        let runtime = FakeRuntime::new(None).with_resolved_version("0.1.0");

        let outcome = run_upgrade(
            &runtime,
            &systems(),
            "0.1.0",
            FailurePolicy::RollbackAll,
            false,
            true,
            &path,
        )
        .await
        .unwrap();

        assert!(outcome.dry_run);
        assert!(outcome.plan.no_op);
        assert!(outcome.record.is_none());
        assert!(runtime.calls().is_empty());
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn force_bypasses_no_op() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("upgrade.json");
        let runtime = FakeRuntime::new(None)
            .with_resolved_version("0.1.0")
            .with_force(true);

        let outcome = run_upgrade(
            &runtime,
            &systems(),
            "0.1.0",
            FailurePolicy::RollbackAll,
            false,
            false,
            &path,
        )
        .await
        .unwrap();

        assert!(!outcome.plan.no_op);
        assert!(
            runtime.calls().contains(&"diagnose".to_string()),
            "force must run the transaction: {:?}",
            runtime.calls()
        );
    }

    #[tokio::test]
    async fn no_op_requires_versioned_targets_and_all_systems() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("upgrade.json");

        // 地址型目标没有具体版本 → 不能判定 no-op。
        let outcome = run_upgrade(
            &FakeRuntime::new(None),
            &systems(),
            "http://example.com/pkg.tar.gz",
            FailurePolicy::Halt,
            false,
            true,
            &path,
        )
        .await
        .unwrap();
        assert!(!outcome.plan.no_op);

        // 只要有一个系统版本不同 → 不做整工程 no-op。
        let runtime = FakeRuntime::new(None)
            .with_resolved_version("0.1.0")
            .with_sys_version("s2", "0.2.0");
        let outcome = run_upgrade(
            &runtime,
            &systems(),
            "0.1.0",
            FailurePolicy::Halt,
            false,
            true,
            &path,
        )
        .await
        .unwrap();
        assert!(!outcome.plan.no_op);

        // 现场版本带 `v` 前缀时仍应识别为同版本。
        let runtime = FakeRuntime::new(None)
            .with_current_version("v0.1.0")
            .with_resolved_version("0.1.0");
        let outcome = run_upgrade(
            &runtime,
            &systems(),
            "0.1.0",
            FailurePolicy::Halt,
            false,
            true,
            &path,
        )
        .await
        .unwrap();
        assert!(outcome.plan.no_op);

        // `--force` + dry-run 也应显示将执行事务。
        let runtime = FakeRuntime::new(None)
            .with_resolved_version("0.1.0")
            .with_force(true);
        let outcome = run_upgrade(
            &runtime,
            &systems(),
            "0.1.0",
            FailurePolicy::Halt,
            false,
            true,
            &path,
        )
        .await
        .unwrap();
        assert!(!outcome.plan.no_op);
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
        // dry-run 会**解析**目标（只读，不记入 calls），并把结果放进计划。
        assert_eq!(
            outcome.plan.resolved,
            vec!["https://example.com/resolved.tar.gz".to_string(); 2],
            "dry-run must resolve each system's target"
        );
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
        assert_eq!(json["no_op"], false);
    }
}
