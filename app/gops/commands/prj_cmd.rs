use clap::{Args, Parser};
use derive_getters::Getters;
use galaxy_ops::error::MainResult;
use galaxy_ops::infra::DfxArgsGetter;
use galaxy_ops::ops_prj::backup::{list_archive, restore};
use galaxy_ops::ops_prj::diagnose::{DiagnoseRequest, project_diagnose};
use galaxy_ops::ops_prj::project::OpsProject;
use galaxy_ops::prelude::{ErrorConv, ErrorOwe};
use galaxy_ops::types::InsUpdateable;
use orion_infra::path::make_new_path;
use orion_variate::update::DownloadOptions;
use orion_vars::vars::ValueDict;

use crate::commands::common::{DebugLogArgs, ForceArgs};
use crate::commands::run_cmd::RunCommandHandler;
use crate::commands::sys_cmd::{SysCommandHandler, SysLocalizeArgs};

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use galaxy_ops::error::MainReason;
use galaxy_ops::ops_prj::upgrade::{
    FailurePolicy, ProjectRuntime, RuntimeDispatch, imported_systems, record_path, run_upgrade,
};

#[derive(Debug, Args, Getters)]
pub struct PrjNewArgs {
    #[arg(short, long, help = "工程配置名称 (Project configuration name)")]
    pub(crate) name: String,
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,
}

#[derive(Debug, Args, Getters)]
pub struct PrjImportArgs {
    #[arg(short = 'p', long = "path", help = "系统导入路径 (System import path)")]
    pub path: String,
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,
    #[clap(flatten)]
    pub force: ForceArgs,
}

#[derive(Debug, Args, Getters)]
pub struct PrjUpdateArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,
    #[clap(flatten)]
    pub force: ForceArgs,
}

#[derive(Debug, Args, Getters)]
pub struct PrjReimportArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,
    #[clap(flatten)]
    pub force: ForceArgs,
}

#[derive(Debug, Args, Getters)]
pub struct PrjRebuildArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,
    #[clap(flatten)]
    pub force: ForceArgs,
    #[arg(help = "只重建该系统（缺省 = ops-prj.yml 里的全部系统）")]
    pub name: Option<String>,
}

#[derive(Debug, Args, Getters)]
pub struct PrjBackupArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,
    #[arg(long, help = "连系统声明的 rebuild 档一起收（默认只收 restore 档）")]
    pub include_rebuild: bool,
}

#[derive(Debug, Args, Getters)]
pub struct PrjRestoreArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,
    #[arg(help = "备份归档 (.tar.gz) 的路径")]
    pub archive: String,
    #[arg(long, help = "只列出归档内容，不落地")]
    pub list: bool,
    #[arg(long, help = "预演：解包校验但不改动现场")]
    pub dry_run: bool,
}

#[derive(Debug, Args, Getters)]
pub struct PrjDiagnoseArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,
    #[arg(long, help = "将警告视为错误（用于 CI 卡口）")]
    pub strict: bool,
}

#[derive(Debug, Args, Getters)]
pub struct PrjUpgradeArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,

    #[arg(help = "只升级该系统（缺省 = ops-prj.yml 里已导入的全部系统）")]
    pub name: Option<String>,

    #[arg(
        long = "to",
        value_name = "VERSION|URL|PATH",
        help = "目标版本（按 ref 的 addr 模板 `{version}` 解析）或完整地址（URL / 本机路径）"
    )]
    pub to: String,

    #[arg(
        long = "on-failure",
        value_name = "rollback-all|halt",
        help = "失败处置：rollback-all = 整工程回滚；halt = 停在那里不回滚。默认未定，现阶段必填"
    )]
    pub on_failure: String,

    #[arg(long, help = "只出计划，不动现场")]
    pub dry_run: bool,

    #[arg(
        long = "health-cmd",
        help = "栈外健康检查命令（给 sh -c）；给了才做回滚判定"
    )]
    pub health_cmd: Option<String>,

    #[arg(
        long = "health-timeout",
        default_value_t = galaxy_ops::ops_prj::upgrade::DEFAULT_HEALTH_TIMEOUT_SECS,
        help = "健康检查超时（秒）"
    )]
    pub health_timeout: u64,

    #[arg(long, help = "机读结果（单行 JSON，给编排器判成败）")]
    pub json: bool,

    #[clap(flatten)]
    pub force: ForceArgs,
}

#[derive(Debug, Parser)]
pub enum PrjCmd {
    #[command(about = "创建维护工程 (Create Maintenance Project)")]
    New(PrjNewArgs),
    #[command(about = "导入系统到工程 (Import System to Project)")]
    Import(PrjImportArgs),
    #[command(about = "维护工程 (Maintain Project)")]
    Update(PrjUpdateArgs),
    #[command(
        about = "补装缺失的系统 (Reimport Missing Systems)",
        long_about = "按 ops-prj.yml 里记录的 sys_models，把**不在场的**系统重新导入。\
                     已存在的目录一律不动（只补、不删），并在结束时拒绝并指向 `prj update` / `prj rebuild`。\
                     适用于删除了已导入系统目录、但保留了 values/ + ops-prj.yml 的场景。"
    )]
    Reimport(PrjReimportArgs),
    #[command(
        about = "重建系统 (Rebuild Systems)",
        long_about = "重建系统目录：现场态（`sys-prj.yml: preserve`）先搬走、旧目录改名保留、铺新内容、再搬回；\
                     中途失败回滚。目录不在场的直接导入。**这是唯一会丢“包外未声明内容”的操作**，\
                     所以不做成默认；只想升级请用 `gops prj update`。\
                     可选 `<系统名>` 只重建一个，缺省重建 ops-prj.yml 里的全部系统。"
    )]
    Rebuild(PrjRebuildArgs),
    #[command(
        about = "备份现场态 (Backup Site State)",
        long_about = "按系统侧 `sys-prj.yml: backup.{restore,rebuild}` 与项目侧 `ops-prj.yml: backup` 的声明，\
                     收集现场态到一个归档（含清单 + sha256），并保留最近 `keep` 份。\
                     含私钥/凭据的条目会被点名，请离机保管。"
    )]
    Backup(PrjBackupArgs),
    #[command(
        about = "还原现场态 (Restore Site State)",
        long_about = "把备份归档合并回现场目录（先解到临时目录再覆盖）。\
                     `--list` 只列内容；`--dry-run` 解包校验但不改动现场。"
    )]
    Restore(PrjRestoreArgs),
    #[command(
        about = "升级发布态 (Upgrade Deployed Systems)",
        long_about = "对已导入的系统做一次**升级事务**：diagnose → backup → apply（覆盖包内内容）→ \
                     regenerate（sys update + localize）→ pull → up → health。\n\
                     失败处置由 `--on-failure` 指定（rollback-all = 整工程回滚；halt = 停在那里不回滚）；\
                     默认值未定，现阶段**必填**。\n\
                     第 1 版只支持 `kind: docker-compose` 的系统；`--to` 收**版本**（按 ref 的 `addr` 模板 \
                     `{version}` 解析）或**完整地址**（URL / 本机路径）。\n\
                     只覆盖包内内容并保留现场态（`sys-prj.yml: preserve`）；备份/回滚走 `prj backup`/`restore`。"
    )]
    Upgrade(PrjUpgradeArgs),
    #[command(
        about = "诊断工程现场态 (Diagnose Project)",
        alias = "doctor",
        long_about = "检查运维项目的现场态声明与客户值纳管：values/ 是否存在、每个已导入系统是否有值目录、\
                     values/ 是否被 .gitignore 忽略、是否有未提交改动；以及 sys-prj.yml 的\
                     `ignore ⊆ preserve`、`backup.restore` 是否声明、ops-prj.yml 的 sys_models 是否重名。\
                     （旧名 `doctor` 仍可用。）"
    )]
    Diagnose(PrjDiagnoseArgs),
}

impl DfxArgsGetter for PrjNewArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

impl DfxArgsGetter for PrjImportArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

impl DfxArgsGetter for PrjUpdateArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

impl DfxArgsGetter for PrjReimportArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

impl DfxArgsGetter for PrjRebuildArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

impl DfxArgsGetter for PrjBackupArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

impl DfxArgsGetter for PrjRestoreArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

impl DfxArgsGetter for PrjDiagnoseArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

impl DfxArgsGetter for PrjUpgradeArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

pub struct PrjCommandHandler;

/// `prj upgrade` 的运行时能力闸门：本版只支持 `kind: docker-compose`。
///
/// 放在**动现场之前**（backup/apply 前）：否则非 compose 系统会先被覆盖内容 + localize，
/// 再在 `pull` 才失败，留下中间态。读 `sys/sys_model.yml` 的 `kind`，不碰任何文件。
fn ensure_upgradeable(systems: &[String], project_root: &Path) -> MainResult<()> {
    for sys in systems {
        let dir = project_root.join(sys);
        let kind = galaxy_ops::system::operator::SysOperator::load_kind(&dir);
        if !matches!(kind, galaxy_ops::system::SysKind::DockerCompose) {
            return Err(MainReason::logic_detail(format!(
                "系统 `{sys}` 不是 kind: docker-compose；`gops prj upgrade` 第一版只支持 compose\
                 （见 docs/design/prj-upgrade.md §3.5）"
            )));
        }
    }
    Ok(())
}

/// `prj upgrade` 第一版的运行时：docker-compose 的 localize / pull / up / status。
///
/// 与栈隔离的核心：它只碰指定系统目录，不依赖任何全局状态。
struct ComposeDispatch;

#[async_trait::async_trait]
impl RuntimeDispatch for ComposeDispatch {
    async fn localize(&self, sys_dir: &Path) -> MainResult<()> {
        let args = SysLocalizeArgs {
            debug_log: DebugLogArgs {
                debug: 0,
                log: None,
            },
            module: None,
            only: false,
            no_flow: false,
        };
        SysCommandHandler::localize_in(sys_dir, &args).await
    }
    async fn pull(&self, sys_dir: &Path) -> MainResult<()> {
        RunCommandHandler::compose_cmd_in(sys_dir, "download").await
    }
    async fn up(&self, sys_dir: &Path) -> MainResult<()> {
        RunCommandHandler::compose_cmd_in(sys_dir, "start").await
    }
    async fn status(&self, sys_dir: &Path) -> MainResult<()> {
        RunCommandHandler::compose_cmd_in(sys_dir, "status").await
    }
}

/// 从当前目录加载运维项目，并在“不在项目根”时给出可操作的提示。
///
/// `gops prj` 全部以 CWD 为项目根（`ops-prj.yml` 所在）。常见误用是在**系统目录**里执行
/// （如 `<项目>/wist-gateway-stack/`）——那里没有 `ops-prj.yml`，默认报错是一串
/// “CONF ERROR / read file”，看不出该怎么办。
fn load_project_from_cwd(current_dir: &std::path::Path) -> MainResult<OpsProject> {
    if current_dir.join("ops-prj.yml").exists() {
        return OpsProject::load(current_dir).err_conv();
    }
    // 父目录是项目根？—— 多半是在系统子目录里执行的
    if let Some(parent) = current_dir.parent()
        && parent.join("ops-prj.yml").exists()
    {
        return Err(format!(
            "当前目录不是运维项目根：{}\n  → 它下面没有 ops-prj.yml，父目录 {} 才有。\n  → 请到项目根目录重跑，例如：cd {} && gops prj backup",
            current_dir.display(),
            parent.display(),
            parent.display()
        ))
        .source_resource()?;
    }
    Err(format!(
        "当前目录没有 ops-prj.yml（运维项目标志）：{}\n  → `gops prj` 需要在项目根目录运行；新建项目用 `gops prj new <name>`。",
        current_dir.display()
    ))
    .source_resource()?
}

impl PrjCommandHandler {
    pub async fn handle_new(args: PrjNewArgs) -> MainResult<()> {
        let current_dir = std::env::current_dir().source_resource()?;
        let new_prj = current_dir.join(args.name());
        make_new_path(&new_prj).source_resource()?;

        let spec = OpsProject::make_new(&new_prj, args.name()).err_conv()?;
        spec.save().err_conv()?;
        Ok(())
    }

    pub async fn handle_import(args: PrjImportArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);
        let current_dir = std::env::current_dir().source_resource()?;
        let options = DownloadOptions::from((*args.force.force(), ValueDict::default()));
        let mut prj = load_project_from_cwd(&current_dir)?;
        let accessor = galaxy_ops::accessor::accessor_for_default();

        prj.import_sys(accessor, args.path(), &options)
            .await
            .err_conv()?;
        Ok(())
    }

    pub async fn handle_update(args: PrjUpdateArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);

        let current_dir = std::env::current_dir().source_resource()?;
        let options = DownloadOptions::from((*args.force.force(), ValueDict::default()));
        let prj = load_project_from_cwd(&current_dir)?;
        let accessor = galaxy_ops::accessor::accessor_for_default();

        // 先做**非破坏性内容更新**（包内覆盖、preserve 不动），再更新项目 conf。
        prj.update_sys_content(accessor.clone(), &options)
            .await
            .err_conv()?;
        prj.update_local(accessor, &current_dir, &options)
            .await
            .err_conv()?;
        Ok(())
    }

    pub async fn handle_reimport(args: PrjReimportArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);

        let current_dir = std::env::current_dir().source_resource()?;
        let options = DownloadOptions::from((*args.force.force(), ValueDict::default()));
        let mut prj = load_project_from_cwd(&current_dir)?;
        let accessor = galaxy_ops::accessor::accessor_for_default();

        prj.reimport(accessor, &options).await.err_conv()?;
        Ok(())
    }

    pub async fn handle_rebuild(args: PrjRebuildArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);

        let current_dir = std::env::current_dir().source_resource()?;
        let options = DownloadOptions::from((*args.force.force(), ValueDict::default()));
        let mut prj = load_project_from_cwd(&current_dir)?;
        let accessor = galaxy_ops::accessor::accessor_for_default();

        prj.rebuild(args.name().as_deref(), accessor, &options)
            .await
            .err_conv()?;
        Ok(())
    }

    pub async fn handle_backup(args: PrjBackupArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);
        let current_dir = std::env::current_dir().source_resource()?;
        let prj = load_project_from_cwd(&current_dir)?;

        let report = prj.backup(*args.include_rebuild()).err_conv()?;
        println!(
            "备份完成 → {}（{} 项，共 {} 字节）",
            report.archive.display(),
            report.entries.len(),
            report.total_bytes()
        );
        for e in &report.entries {
            println!("  {}  {}  {}", e.sha256, e.size, e.path);
        }
        let secrets: Vec<&str> = report.secret_entries().map(|e| e.path.as_str()).collect();
        if !secrets.is_empty() {
            println!("  ⚠ 含私钥/凭据 {} 项，请离机安全保管：", secrets.len());
            for s in &secrets {
                println!("    {s}");
            }
        }
        for old in &report.pruned {
            println!("  清理过期份：{}", old.display());
        }
        Ok(())
    }

    pub async fn handle_restore(args: PrjRestoreArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);
        let current_dir = std::env::current_dir().source_resource()?;
        let archive = std::path::PathBuf::from(args.archive());

        // 还原落点就是项目根：先确认 CWD 是项目根，免得把现场态倒进系统子目录
        let _ = load_project_from_cwd(&current_dir)?;

        if *args.list() {
            for p in list_archive(&archive).err_conv()? {
                println!("{p}");
            }
            return Ok(());
        }
        let report = restore(&current_dir, &archive, *args.dry_run()).err_conv()?;
        if report.dry_run {
            println!(
                "预演（未改动现场）→ {}（将还原 {} 个系统目录：{}）",
                report.root.display(),
                report.entries.len(),
                report.entries.join("、")
            );
        } else {
            println!(
                "已还原 → {}（{} 个系统目录：{}）",
                report.root.display(),
                report.entries.len(),
                report.entries.join("、")
            );
            // 备份只收**现场态**，不含交付包内容：目录是被删掉的场景还要把系统铺回来
            println!(
                "提示：备份不含交付包内容（sys/ 等）；若系统目录是被删掉的，请再跑 `gops prj import` 或 `gops prj reimport`"
            );
        }
        Ok(())
    }

    pub async fn handle_upgrade(args: PrjUpgradeArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);
        let current_dir = std::env::current_dir().source_resource()?;
        // 先确认 CWD 是项目根：backup/restore 的落点就是它，倒错地方代价很大。
        let _ = load_project_from_cwd(&current_dir)?;

        let policy = FailurePolicy::parse(args.on_failure()).ok_or_else(|| {
            MainReason::logic_detail(format!(
                "--on-failure 只接受 rollback-all / halt，收到：{}",
                args.on_failure()
            ))
        })?;

        let systems = imported_systems(&current_dir, args.name().as_deref())?;
        // 早停：本版运行时只支持 docker-compose。非 compose 系统在**动现场之前**就拒，
        // 而不是先覆盖内容、localize 完到 pull 才报错（那就留下了中间态）。
        ensure_upgradeable(&systems, &current_dir)?;
        let state_path = record_path(&current_dir);

        let runtime = ProjectRuntime::new(
            current_dir.clone(),
            args.to().clone(),
            *args.force().force() > 0,
            args.health_cmd().clone(),
            Duration::from_secs(*args.health_timeout()),
            Arc::new(ComposeDispatch),
        );

        let outcome = run_upgrade(
            &runtime,
            &systems,
            args.to(),
            policy,
            args.health_cmd().is_some(),
            *args.dry_run(),
            &state_path,
        )
        .await?;

        if *args.json() {
            println!("{}", outcome.to_json());
        } else {
            Self::print_upgrade(&outcome);
        }

        if outcome.exit_code() != 0 {
            std::process::exit(1);
        }
        Ok(())
    }

    fn print_upgrade(outcome: &galaxy_ops::ops_prj::upgrade::UpgradeOutcome) {
        if outcome.dry_run {
            println!("升级计划（--dry-run，未动现场）");
            println!("  目标      {}", outcome.plan.target);
            println!("  系统      {}", outcome.plan.systems.join("、"));
            println!("  失败处置  {}", outcome.plan.policy);
            println!(
                "  健康检查  {}",
                if outcome.plan.health {
                    "有（失败会触发回滚判定）"
                } else {
                    "无（不探活）"
                }
            );
            println!("  阶段序    diagnose → backup → apply → regenerate → pull → up → health");
            return;
        }
        if let Some(record) = &outcome.record {
            let label = match record.status.as_str() {
                "succeeded" => "升级成功",
                "rolled_back" => "已回滚（整工程）",
                _ => "升级失败",
            };
            println!("{label}（status={}）", record.status);
            println!(
                "  版本  {} → {}",
                record.from_version,
                if record.to_version.is_empty() {
                    "—"
                } else {
                    &record.to_version
                }
            );
            println!("  步骤  {}", record.step);
            if let Some(backup) = &record.backup_id {
                println!("  备份  {backup}");
            }
            if !record.detail.is_empty() {
                println!("  详情  {}", record.detail);
            }
        }
        println!("  状态文件  {}", outcome.record_path.display());
    }

    pub async fn handle_diagnose(args: PrjDiagnoseArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);
        let current_dir = std::env::current_dir().source_resource()?;

        // 提前给“不在项目根”的可操作提示（project_diagnose 内部自己会再 load 一次）
        let _ = load_project_from_cwd(&current_dir)?;

        let report =
            project_diagnose(&current_dir, &DiagnoseRequest::new(*args.strict())).err_conv()?;

        // 版式与 agentd `diagnose` 一致（[OK]/[WARN]/[FAIL] + 结论），着色由 NO_COLOR / TTY 决定。
        let text = report.render_text(galaxy_ops::report::use_color());
        print!("{text}");
        // 有 FAIL 时下面直接 exit，先把 stdout 落盘（管道下是块缓冲）
        use std::io::Write as _;
        let _ = std::io::stdout().flush();

        if report.exit_code() != 0 {
            // 诊断的“失败”是**结果**，不是异常：退 1 让 `gops prj diagnose || 处理` 可用，
            // 但不走框架的 Run Error 块（那会把报告重复一遍、还盖在结论后面）。
            std::process::exit(1);
        }
        Ok(())
    }

    pub async fn execute(cmd: PrjCmd) -> MainResult<()> {
        match cmd {
            PrjCmd::New(args) => Self::handle_new(args).await,
            PrjCmd::Import(args) => Self::handle_import(args).await,
            PrjCmd::Update(args) => Self::handle_update(args).await,
            PrjCmd::Reimport(args) => Self::handle_reimport(args).await,
            PrjCmd::Rebuild(args) => Self::handle_rebuild(args).await,
            PrjCmd::Backup(args) => Self::handle_backup(args).await,
            PrjCmd::Restore(args) => Self::handle_restore(args).await,
            PrjCmd::Upgrade(args) => Self::handle_upgrade(args).await,
            PrjCmd::Diagnose(args) => Self::handle_diagnose(args).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{GInsCmd, PrjCmd};
    use clap::Parser;
    use tempfile::TempDir;

    #[test]
    fn test_prj_rebuild_parses_with_and_without_name() {
        let with = GInsCmd::try_parse_from(["gops", "prj", "rebuild", "web-stack"]).unwrap();
        match with {
            GInsCmd::Prj(PrjCmd::Rebuild(a)) => {
                assert_eq!(a.name().as_deref(), Some("web-stack"))
            }
            other => panic!("expected rebuild, got {other:?}"),
        }

        let without = GInsCmd::try_parse_from(["gops", "prj", "rebuild"]).unwrap();
        match without {
            GInsCmd::Prj(PrjCmd::Rebuild(a)) => assert!(a.name().is_none()),
            other => panic!("expected rebuild, got {other:?}"),
        }
    }

    #[test]
    fn test_prj_reimport_no_longer_accepts_recreate_flag() {
        assert!(GInsCmd::try_parse_from(["gops", "prj", "reimport"]).is_ok());
        // 改名后旧旗标必须被拒（不做隐藏别名，因为从未发布）
        assert!(GInsCmd::try_parse_from(["gops", "prj", "reimport", "--recreate"]).is_err());
    }

    #[test]
    fn test_prj_doctor_is_diagnose_alias() {
        let cmd = GInsCmd::try_parse_from(["gops", "prj", "doctor"]).unwrap();
        assert!(matches!(cmd, GInsCmd::Prj(PrjCmd::Diagnose(_))));
    }

    #[test]
    fn test_ensure_upgradeable_rejects_non_compose() {
        let tmp = TempDir::new().unwrap();
        // compose 系统：放一份 kind: docker-compose 的 sys_model.yml
        let compose = tmp.path().join("compose-sys");
        std::fs::create_dir_all(compose.join("sys")).unwrap();
        std::fs::write(
            compose.join("sys/sys_model.yml"),
            "name: compose-sys\nkind: docker-compose\nvender: ''\n",
        )
        .unwrap();
        assert!(ensure_upgradeable(&["compose-sys".to_string()], tmp.path()).is_ok());

        // gxl 系统（缺 kind）→ 拒
        let gxl = tmp.path().join("gxl-sys");
        std::fs::create_dir_all(gxl.join("sys")).unwrap();
        std::fs::write(gxl.join("sys/sys_model.yml"), "name: gxl-sys\nvender: ''\n").unwrap();
        let err = ensure_upgradeable(&["gxl-sys".to_string()], tmp.path()).unwrap_err();
        assert!(err.to_string().contains("docker-compose"), "{err}");

        // 空清单：无可升级（上层 `run_upgrade` 会拒），这里不报错
        assert!(ensure_upgradeable(&[], tmp.path()).is_ok());
    }

    #[test]
    fn test_prj_upgrade_requires_to_and_on_failure() {
        // 两个都必填：缺一即拒（默认值未定，不替人选）。
        assert!(
            GInsCmd::try_parse_from(["gops", "prj", "upgrade", "--to", "http://x/p.tar.gz"])
                .is_err()
        );
        assert!(
            GInsCmd::try_parse_from(["gops", "prj", "upgrade", "--on-failure", "halt"]).is_err()
        );

        let cmd = GInsCmd::try_parse_from([
            "gops",
            "prj",
            "upgrade",
            "--to",
            "http://x/p.tar.gz",
            "--on-failure",
            "rollback-all",
            "--dry-run",
            "--json",
        ])
        .unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(a)) => {
                assert_eq!(a.to(), "http://x/p.tar.gz");
                assert_eq!(a.on_failure(), "rollback-all");
                assert!(*a.dry_run());
                assert!(*a.json());
                assert_eq!(*a.health_timeout(), 60);
                assert!(a.name().is_none());
            }
            other => panic!("expected upgrade, got {other:?}"),
        }
    }

    fn make_project(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("_gal")).unwrap();
        std::fs::write(root.join("_gal/work.gxl"), "mod envs {}\nmod main {}\n").unwrap();
        std::fs::write(
            root.join("ops-prj.yml"),
            "name: cust\nwork_envs:\n  dep_root: ''\n  deps: []\n",
        )
        .unwrap();
    }

    #[test]
    fn test_load_project_from_cwd_ok_at_root() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());
        assert!(load_project_from_cwd(tmp.path()).is_ok());
    }

    #[test]
    fn test_load_project_from_cwd_points_to_parent_when_in_system_dir() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());
        // 在系统子目录里执行（该目录没有 ops-prj.yml）
        let sys = tmp.path().join("wist-gateway-stack");
        std::fs::create_dir_all(&sys).unwrap();

        let err = load_project_from_cwd(&sys).unwrap_err();
        let msg = err.detail().as_deref().unwrap_or_default();
        assert!(msg.contains("不是运维项目根"), "msg={msg}");
        assert!(msg.contains("cd"), "msg={msg}");
        assert!(msg.contains("wist-gateway-stack"), "msg={msg}");
    }

    #[test]
    fn test_load_project_from_cwd_without_project() {
        let tmp = TempDir::new().unwrap();
        let err = load_project_from_cwd(tmp.path()).unwrap_err();
        let msg = err.detail().as_deref().unwrap_or_default();
        assert!(msg.contains("ops-prj.yml"), "msg={msg}");
    }
}
