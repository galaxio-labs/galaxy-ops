use clap::{Args, Parser, Subcommand};
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
    FailurePolicy, ProjectRuntime, RuntimeDispatch, UpgradeOutcome, UpgradeTarget,
    imported_systems, record_path, run_upgrade,
};
use galaxy_ops::ops_prj::version::VersionSpec;
use galaxy_ops::self_update::ReleaseChannel;

fn parse_channel_arg(input: &str) -> Result<String, String> {
    ReleaseChannel::parse(input)
        .map(|channel| channel.as_str().to_string())
        .ok_or_else(|| format!("channel={input}, expected=stable|alpha|beta"))
}

fn parse_failure_policy_arg(input: &str) -> Result<String, String> {
    FailurePolicy::parse(input)
        .map(|policy| policy.as_str().to_string())
        .ok_or_else(|| format!("on-failure={input}, expected=rollback-all|halt"))
}

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
pub struct PrjSyncArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,
    #[clap(flatten)]
    pub force: ForceArgs,
}

#[derive(Debug, Args, Getters)]
pub struct PrjBackupArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,
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
pub struct PrjContractArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,
    #[command(subcommand)]
    pub cmd: PrjContractCmd,
}

#[derive(Debug, Subcommand)]
pub enum PrjContractCmd {
    #[command(
        about = "导出对外契约 (Export Contract)",
        long_about = "把本部署的对外契约（`sys-prj.yml: contract` 列出的文件 / 目录）打成归档，\
                     文件名带日期时间，用于交付 / 迁移 / 存档。"
    )]
    Export,
    #[command(
        about = "导入对外契约 (Import Contract)",
        long_about = "从归档把契约灌回本部署（写进 `contract:` 列出的文件），幂等。"
    )]
    Import {
        #[arg(help = "契约归档 (.tar.gz) 的路径")]
        archive: String,
    },
}

impl DfxArgsGetter for PrjContractArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

#[derive(Debug, Args, Getters)]
pub struct PrjUpgradeArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,

    #[arg(
        value_name = "SYS|SPEC",
        help = "系统名（缺省 = 全部）；若它不是已导入系统，则视为版本 spec（latest/patch/minor/major/X.Y.Z）"
    )]
    pub name: Option<String>,

    #[arg(
        value_name = "SPEC",
        help = "第二个位置参数：版本 spec（与 -v/--version 二选一）"
    )]
    pub spec: Option<String>,

    #[arg(
        short = 'v',
        long = "version",
        value_name = "latest|patch|minor|major|X.Y.Z",
        conflicts_with = "spec",
        help = "目标版本 spec（`feat` 是 `minor` 的别名）"
    )]
    pub version: Option<String>,

    #[arg(
        long = "to",
        value_name = "URL|PATH",
        conflicts_with_all = ["version", "spec", "channel"],
        help = "完整包地址（URL / 本机路径）；与版本 spec 互斥"
    )]
    pub to: Option<String>,

    #[arg(
        short = 'c',
        long = "channel",
        value_name = "stable|alpha|beta",
        value_parser = parse_channel_arg,
        help = "发布通道：仅对 latest/patch/minor/major 生效；缺省按当前部署版本后缀推断"
    )]
    pub channel: Option<String>,

    #[arg(long = "no-save", help = "升级成功后不回写 ops-prj.yml 的 `version:`")]
    pub no_save: bool,

    #[arg(
        long = "no-stop",
        help = "切换时不先停服务（默认 stop → up -d，确保 bind mount 的配置被重新读取；回滚同样生效）"
    )]
    pub no_stop: bool,

    #[arg(
        long = "on-failure",
        value_name = "rollback-all|halt",
        default_value = "rollback-all",
        value_parser = parse_failure_policy_arg,
        help = "失败处置：rollback-all = 整工程回滚（默认）；halt = 停在那里不回滚"
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
    #[command(
        about = "同步 / 补齐工程系统 (Sync Project Systems)",
        alias = "update",
        long_about = "按 ops-prj.yml 记录的系统逐个同步：**目录缺失的按包导入**（补齐被删的系统），\
                     **目录已在的**把包内内容**非破坏性**地覆盖到各自 `addr` 指向的版本 —— 仅覆盖**包内**文件，\
                     `sys-prj.yml` 的 `protect` 一律不碰，**不删**任何东西；\
                     同时刷新项目 `work_envs` 并回写 `ops-prj.yml`。\n\
                     与 `prj upgrade` 的区别：本命令**不是事务**（无备份、无健康检查、无回滚），\
                     也不同步运行时（不拉镜像、不切换、不探活）——要一次可回滚的发布升级请用 `gops prj upgrade`。\
                     （旧名 `update` 仍可用。）"
    )]
    Sync(PrjSyncArgs),
    #[command(
        about = "备份现场态 (Backup Site State)",
        long_about = "按系统侧 `sys-prj.yml: backup` 与项目侧 `ops-prj.yml: backup` 的声明，\
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
        about = "对外契约 (Contract)",
        long_about = "管理对外契约（不可重建的公开现场事实）：导出 / 导入。\
                     声明在 `sys-prj.yml: contract`（只列文件 / 目录）；契约内容是 site-owned 的现场事实。"
    )]
    Contract(PrjContractArgs),
    #[command(
        about = "升级发布态 (Upgrade Deployed Systems)",
        long_about = "对已导入的系统做一次**升级事务**：diagnose → backup → apply（覆盖包内内容）→ \
                     regenerate（sys resolve + localize）→ pull → up → health。\n\
                     `up` 默认先 `docker compose stop` 再 `up -d`：bind mount 里的配置内容变化不会改变 \
                     compose 配置哈希，单跑 `up -d` 不会让未变更的容器重读配置（`--no-stop` 关闭先停）。\n\
                     失败处置由 `--on-failure` 指定（rollback-all = 整工程回滚，**默认**；halt = 停在那里不回滚）。\n\
                     第 1 版只支持 `kind: docker-compose` 的系统。\n\
                     版本 spec 支持 `latest` / `patch` / `minor`（`feat` 别名）/ `major` / `X.Y.Z`；\
                     可写成 `upgrade [<sys>] <spec>` 或 `-v/--version <spec>`；缺省使用 `ops-prj.yml` 的 `version:`。\n\
                     `-c/--channel stable|alpha|beta` 只对 keyword spec 生效，缺省按当前部署版本后缀推断。\n\
                     `latest/patch/...` 从版本目录选择（GitHub Releases / git tag / 本地包目录）；\n\
                     普通 HTTP 源（COS/S3/OSS/nginx）没有清单时，`patch`/`minor`/`major` 会按版本规则\n\
                     生成候选地址并探测是否存在；`latest` 需要 `versions.json`/`versions.txt`，\n\
                     或用精确版本 / `--to <完整地址>`。\n\
                     成功后把解析出的具体版本回写 `ops-prj.yml`（`--no-save` 关闭）。\n\
                     只覆盖包内内容并保留现场态（`sys-prj.yml: protect`）；备份/回滚走 `prj backup`/`restore`。\
                     与 `prj sync` 的区别：本命令是**事务**（含备份 / 重渲 / 拉镜像 / 切换 / 探活，可回滚）；\
                     只同步磁盘内容用 `gops prj sync`。"
    )]
    Upgrade(PrjUpgradeArgs),
    #[command(
        about = "诊断工程现场态 (Diagnose Project)",
        alias = "doctor",
        long_about = "检查运维项目的现场态声明与客户值纳管：values/ 是否存在、每个已导入系统是否有值目录、\
                     values/ 是否被 .gitignore 忽略、是否有未提交改动；sys-prj.yml 的 `contract:` 是否齐备；\
                     ops-prj.yml 的 sys_models 是否重名。\
                     只做各声明**自身**的检查，不在 `pkg_include`/`pkg_exclude`/`protect`/`backup`/`contract`\
                     之间做任何跨声明推理。\
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

impl DfxArgsGetter for PrjSyncArgs {
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
struct ComposeDispatch {
    /// 切换前是否先 `docker compose stop`（默认 true，`--no-stop` 关闭）。
    stop_before_start: bool,
}

impl ComposeDispatch {
    fn new(stop_before_start: bool) -> Self {
        Self { stop_before_start }
    }
}

/// 切换阶段要跑的子命令序列。
///
/// 默认 `stop` → `start`：`docker compose up -d` 只重建**配置哈希变了**的容器；
/// bind mount 里的文件内容（WPL/OML、toml、证书）不在哈希里，
/// 不先停就会继续用进程启动时读到的旧内容。
fn switch_commands(stop_before_start: bool) -> &'static [&'static str] {
    if stop_before_start {
        &["stop", "start"]
    } else {
        &["start"]
    }
}

/// 按切换序列依次执行；任一步失败即停（不继续后面的命令）。
///
/// 拆成接受 runner 的形式是为了可测：生产路径传 `compose_cmd_in`，测试用记录序列的闭包
/// 验证「默认先 stop」「stop 失败不再 start」。
async fn run_switch_commands<F, Fut>(stop_before_start: bool, mut run: F) -> MainResult<()>
where
    F: FnMut(&'static str) -> Fut,
    Fut: std::future::Future<Output = MainResult<()>>,
{
    for cmd in switch_commands(stop_before_start) {
        run(cmd).await?;
    }
    Ok(())
}

/// `--dry-run` 里展示的切换方式文案。
fn switch_mode_label(stop_before_switch: bool) -> &'static str {
    if stop_before_switch {
        "stop → up -d（默认；确保 bind mount 的配置被重读）"
    } else {
        "up -d（--no-stop；不先停服务）"
    }
}

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
            no_perms_check: false,
        };
        SysCommandHandler::localize_in(sys_dir, &args, true).await
    }
    async fn pull(&self, sys_dir: &Path) -> MainResult<()> {
        RunCommandHandler::compose_cmd_in(sys_dir, "download").await
    }
    async fn up(&self, sys_dir: &Path) -> MainResult<()> {
        run_switch_commands(self.stop_before_start, |cmd| {
            RunCommandHandler::compose_cmd_in(sys_dir, cmd)
        })
        .await
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

fn system_names(project: &OpsProject) -> Vec<String> {
    project
        .conf()
        .sys_models()
        .iter()
        .map(|s| s.sys().name().clone())
        .collect()
}

/// 对单个系统做宿主 bind 挂载权限修复（`restore` / `upgrade` 完成后调用）。
///
/// 尽力而为：失败只告警，不让一个已成功的恢复/升级变成失败。
/// 输出走 **stderr**：`prj upgrade --json` 的 stdout 必须保持单行 JSON。
fn apply_sys_perms(sys_dir: &Path) {
    if !cfg!(target_os = "linux") {
        return;
    }
    // 只处理 compose 系统：`prj restore` 的条目里可能混有 gxl 系统。
    if !matches!(
        galaxy_ops::system::operator::SysOperator::load_kind(sys_dir),
        galaxy_ops::system::SysKind::DockerCompose
    ) {
        return;
    }
    match galaxy_ops::system::perms::run_with_conf(sys_dir, true) {
        Ok(report) if report.is_ok() => {
            if report.applied() > 0 {
                eprintln!(
                    "  宿主权限已对齐：{}（修复 {} 项）",
                    sys_dir.display(),
                    report.applied()
                );
            }
        }
        Ok(report) => eprintln!(
            "[WARN] {} 宿主权限仍有 {} 项待修 / {} 项失败；运行 `gops sys perms --apply` 查看明细",
            sys_dir.display(),
            report.pending(),
            report.failed()
        ),
        Err(err) => eprintln!("[WARN] {} 宿主权限对齐跳过：{err}", sys_dir.display()),
    }
}

/// 把位置参数拆成“系统名 + 目标”：
/// - 第一个位置参数命中已导入系统 → 系统名；否则视为版本 spec；
/// - 第二个位置参数、`-v/--version` 二选一，作为版本 spec；
/// - `--to` 是完整地址，与 spec / channel 互斥；
/// - 什么都没给 → 使用 `ops-prj.yml` 声明的 `version:`。
fn parse_upgrade_request(
    project: &OpsProject,
    args: &PrjUpgradeArgs,
) -> MainResult<(Option<String>, UpgradeTarget)> {
    let known = |name: &str| {
        project
            .conf()
            .sys_models()
            .iter()
            .any(|s| s.sys().name().as_str() == name)
    };

    let (sys, positional_spec) = match (args.name().as_deref(), args.spec().as_deref()) {
        (None, None) => (None, None),
        (None, Some(spec)) => (None, Some(spec.to_string())),
        (Some(first), second) => {
            if known(first) {
                (Some(first.to_string()), second.map(str::to_string))
            } else if second.is_some() {
                return Err(MainReason::logic_detail(format!(
                    "第一个位置参数 `{first}` 不是已导入系统，后面不能再跟版本 spec `{}`",
                    second.unwrap_or_default()
                )));
            } else if VersionSpec::looks_like(first) {
                (None, Some(first.to_string()))
            } else {
                return Err(MainReason::logic_detail(format!(
                    "未知系统或无效版本 spec：`{first}`。\n  已导入系统：{}",
                    system_names(project).join("、")
                )));
            }
        }
    };

    if args.version().is_some() && positional_spec.is_some() {
        return Err(MainReason::logic_detail(
            "版本 spec 只能给一次：位置参数 或 -v/--version",
        ));
    }
    let explicit_spec = args.version().as_deref().or(positional_spec.as_deref());

    if args.to().is_some() && explicit_spec.is_some() {
        return Err(MainReason::logic_detail(
            "--to 是完整地址，不能同时给版本 spec",
        ));
    }
    if args.to().is_some() && args.channel().is_some() {
        return Err(MainReason::logic_detail(
            "--channel 只用于 latest/patch/minor/major，不能和 --to 一起用",
        ));
    }

    match (args.to().as_deref(), explicit_spec) {
        (Some(addr), None) => Ok((sys, UpgradeTarget::Address(addr.to_string()))),
        (None, Some(spec)) => {
            let spec = VersionSpec::parse(spec)?;
            if args.channel().is_some() && !spec.is_keyword() {
                return Err(MainReason::logic_detail(format!(
                    "--channel 只用于 latest/patch/minor/major；`{}` 是精确/声明版本",
                    spec.as_str()
                )));
            }
            Ok((sys, UpgradeTarget::Spec(spec)))
        }
        (None, None) => {
            if args.channel().is_some() {
                return Err(MainReason::logic_detail(
                    "--channel 需要显式版本 spec：latest/patch/minor/major",
                ));
            }
            Ok((sys, UpgradeTarget::Spec(VersionSpec::Declared)))
        }
        (Some(_), Some(_)) => Err(MainReason::logic_detail(
            "--to 是完整地址，不能同时给版本 spec",
        )),
    }
}

/// 升级成功后把解析出的具体版本回写到 `ops-prj.yml` 的 `version:`。
/// 只处理有具体版本的目标（完整地址目标 / 非版本化系统不写）。
fn persist_upgrade_versions(
    root: &Path,
    outcome: &UpgradeOutcome,
    machine_output: bool,
) -> MainResult<()> {
    let mut project = OpsProject::load(root)?;
    let mut changed = Vec::new();
    for (sys, version) in outcome
        .plan
        .systems
        .iter()
        .zip(&outcome.plan.target_versions)
    {
        let Some(version) = version.as_deref().filter(|v| !v.is_empty()) else {
            continue;
        };
        if project.set_sys_version(sys, version)? {
            changed.push(format!("{sys}={version}"));
        }
    }
    if changed.is_empty() {
        return Ok(());
    }
    project.save()?;
    let line = format!("回写版本  {}", changed.join("、"));
    if machine_output {
        // `--json` 的 stdout 必须是单行 JSON。
        eprintln!("{line}");
    } else {
        println!("  {line}");
    }
    Ok(())
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

    pub async fn handle_sync(args: PrjSyncArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);

        let current_dir = std::env::current_dir().source_resource()?;
        let options = DownloadOptions::from((*args.force.force(), ValueDict::default()));
        let prj = load_project_from_cwd(&current_dir)?;
        let accessor = galaxy_ops::accessor::accessor_for_default();

        // 先做内容同步（缺失系统按包导入；已在系统非破坏覆盖，protect 不动），再更新项目 conf。
        prj.update_sys_content(accessor.clone(), &options)
            .await
            .err_conv()?;
        prj.update_local(accessor, &current_dir, &options)
            .await
            .err_conv()?;
        Ok(())
    }

    pub async fn handle_backup(args: PrjBackupArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);
        let current_dir = std::env::current_dir().source_resource()?;
        let prj = load_project_from_cwd(&current_dir)?;

        let report = prj.backup().err_conv()?;
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
            // 还原会把备份里的文件按**解包账号**落地；bind 挂载的非 root 容器可能读/写不了。
            // 恢复完成即对齐（幂等；非 Linux 自动跳过）。
            for name in &report.entries {
                apply_sys_perms(&current_dir.join(name));
            }
            // 备份到底收了什么，由各系统 `sys-prj.yml: backup` **声明**决定——设计上是现场态，
            // 但**不保证**一定不含交付包内容。所以这里说“备份里没有的”，而不断言“一定没有 sys/”。
            println!(
                "提示：restore 只放回**备份里有的**内容（= 各系统 `sys-prj.yml: backup` 声明命中的路径）。\n\
                 交付包内容（`sys/` 等）与 `contract:` 契约内容都**不在**其中；目录被删时请再跑 `gops prj sync` 把缺失系统按包铺回，\
                 契约内容用 `gops prj contract import` 恢复"
            );
        }
        Ok(())
    }

    pub async fn handle_upgrade(args: PrjUpgradeArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);
        let current_dir = std::env::current_dir().source_resource()?;
        // 先确认 CWD 是项目根：backup/restore 的落点就是它，倒错地方代价很大。
        let project = load_project_from_cwd(&current_dir)?;

        let (sys_name, target) = parse_upgrade_request(&project, &args)?;
        let plan_target = target.label();

        let policy = FailurePolicy::parse(args.on_failure()).ok_or_else(|| {
            MainReason::logic_detail(format!(
                "--on-failure 只接受 rollback-all / halt，收到：{}",
                args.on_failure()
            ))
        })?;

        let systems = imported_systems(&current_dir, sys_name.as_deref())?;
        // 早停：本版运行时只支持 docker-compose。非 compose 系统在**动现场之前**就拒，
        // 而不是先覆盖内容、localize 完到 pull 才报错（那就留下了中间态）。
        ensure_upgradeable(&systems, &current_dir)?;
        let state_path = record_path(&current_dir);

        let channel = args
            .channel()
            .as_deref()
            .map(|channel| ReleaseChannel::parse(channel).expect("channel is validated by clap"));

        let runtime = ProjectRuntime::new(
            current_dir.clone(),
            target,
            channel,
            *args.force().force() > 0,
            args.health_cmd().clone(),
            Duration::from_secs(*args.health_timeout()),
            Arc::new(ComposeDispatch::new(!*args.no_stop())),
        );

        let outcome = run_upgrade(
            &runtime,
            &systems,
            &plan_target,
            policy,
            args.health_cmd().is_some(),
            *args.dry_run(),
            &state_path,
        )
        .await?;

        if *args.json() {
            println!("{}", outcome.to_json());
        } else {
            Self::print_upgrade(&outcome, !*args.no_stop());
        }

        if outcome.exit_code() != 0 {
            std::process::exit(1);
        }

        // 升级成功后再对齐一次宿主权限（`up` 前已 apply；这里覆盖 no-op 等未走 `up` 的分支）。
        if !*args.dry_run() {
            for sys in &outcome.plan.systems {
                apply_sys_perms(&current_dir.join(sys));
            }
        }

        // 成功后才回写期望版本；失败 / 回滚 / dry-run 一律不写。
        if !*args.dry_run()
            && !*args.no_save()
            && let Err(err) = persist_upgrade_versions(&current_dir, &outcome, *args.json())
        {
            eprintln!("警告：升级成功，但回写 ops-prj.yml 的 version 失败：{err}");
        }
        Ok(())
    }

    fn print_upgrade(
        outcome: &galaxy_ops::ops_prj::upgrade::UpgradeOutcome,
        stop_before_switch: bool,
    ) {
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
            if outcome.plan.no_op {
                println!("  结果      no-op（已是目标版本，不执行事务）");
            } else {
                println!("  阶段序    diagnose → backup → apply → regenerate → pull → up → health");
                println!("  切换方式  {}", switch_mode_label(stop_before_switch));
            }
            for (idx, (sys, addr)) in outcome
                .plan
                .systems
                .iter()
                .zip(&outcome.plan.resolved)
                .enumerate()
            {
                match outcome
                    .plan
                    .target_versions
                    .get(idx)
                    .and_then(|v| v.as_deref())
                {
                    Some(version) => {
                        println!("  解析      {sys} → {addr}（版本 {version}）")
                    }
                    None => println!("  解析      {sys} → {addr}"),
                }
            }
            return;
        }
        if let Some(record) = &outcome.record {
            let label = match record.status.as_str() {
                "succeeded" if outcome.plan.no_op => "已是目标版本（no-op）",
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
            if let Some(log_dir) = &record.log_dir {
                println!("  日志  {log_dir}");
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

    pub async fn handle_contract(args: PrjContractArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);
        let current_dir = std::env::current_dir().source_resource()?;
        let prj = load_project_from_cwd(&current_dir)?;

        match args.cmd() {
            PrjContractCmd::Export => {
                let report = prj.contract_export().err_conv()?;
                println!("契约已导出 → {}", report.archive.display());
                if report.entries.is_empty() {
                    println!("提示：没有任何系统声明 `contract:`，归档为空");
                }
                for old in &report.pruned {
                    println!("  清理过期份：{}", old.display());
                }
            }
            PrjContractCmd::Import { archive } => {
                let report = prj.contract_import(Path::new(archive), false).err_conv()?;
                println!("契约已导入：{} 项", report.entries.len());
            }
        }
        Ok(())
    }

    pub async fn execute(cmd: PrjCmd) -> MainResult<()> {
        match cmd {
            PrjCmd::New(args) => Self::handle_new(args).await,
            PrjCmd::Import(args) => Self::handle_import(args).await,
            PrjCmd::Sync(args) => Self::handle_sync(args).await,
            PrjCmd::Backup(args) => Self::handle_backup(args).await,
            PrjCmd::Restore(args) => Self::handle_restore(args).await,
            PrjCmd::Upgrade(args) => Self::handle_upgrade(args).await,
            PrjCmd::Diagnose(args) => Self::handle_diagnose(args).await,
            PrjCmd::Contract(args) => Self::handle_contract(args).await,
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
    fn test_prj_contract_parses_export_and_import() {
        let export = GInsCmd::try_parse_from(["gops", "prj", "contract", "export"]).unwrap();
        assert!(matches!(export, GInsCmd::Prj(PrjCmd::Contract(_))));

        let import =
            GInsCmd::try_parse_from(["gops", "prj", "contract", "import", "c.tar.gz"]).unwrap();
        match import {
            GInsCmd::Prj(PrjCmd::Contract(a)) => match a.cmd() {
                PrjContractCmd::Import { archive } => assert_eq!(archive, "c.tar.gz"),
                other => panic!("expected import, got {other:?}"),
            },
            other => panic!("expected contract, got {other:?}"),
        }

        // 子命令必填
        assert!(GInsCmd::try_parse_from(["gops", "prj", "contract"]).is_err());
    }

    #[test]
    fn test_prj_update_is_sync_alias() {
        // 主名 `sync`；旧名 `update` 仍可用（隐藏别名）。
        assert!(matches!(
            GInsCmd::try_parse_from(["gops", "prj", "sync"]).unwrap(),
            GInsCmd::Prj(PrjCmd::Sync(_))
        ));
        assert!(matches!(
            GInsCmd::try_parse_from(["gops", "prj", "update"]).unwrap(),
            GInsCmd::Prj(PrjCmd::Sync(_))
        ));
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
    fn test_prj_upgrade_on_failure_defaults_to_rollback_all() {
        // 缺省 → rollback-all（安全优先）。
        let cmd = GInsCmd::try_parse_from(["gops", "prj", "upgrade", "latest"]).unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(a)) => {
                assert_eq!(a.on_failure(), FailurePolicy::default().as_str());
                assert_eq!(a.name().as_deref(), Some("latest"));
            }
            other => panic!("expected upgrade, got {other:?}"),
        }

        // 显式 halt 仍可覆盖。
        let cmd =
            GInsCmd::try_parse_from(["gops", "prj", "upgrade", "latest", "--on-failure", "halt"])
                .unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(a)) => assert_eq!(a.on_failure(), "halt"),
            other => panic!("expected upgrade, got {other:?}"),
        }

        // 地址逃生口：--to 不再必填，但仍可用。
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
                assert_eq!(a.to().as_deref(), Some("http://x/p.tar.gz"));
                assert_eq!(a.on_failure(), "rollback-all");
                assert!(*a.dry_run());
                assert!(*a.json());
                assert_eq!(*a.health_timeout(), 60);
                assert!(a.name().is_none());
                assert!(a.spec().is_none());
                assert!(a.version().is_none());
                assert!(a.channel().is_none());
            }
            other => panic!("expected upgrade, got {other:?}"),
        }
    }

    #[test]
    fn test_prj_upgrade_no_stop_defaults_off_and_can_be_enabled() {
        // 缺省：先停再起（bind mount 的配置才会被重读）。
        let cmd = GInsCmd::try_parse_from(["gops", "prj", "upgrade", "latest"]).unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(a)) => assert!(!*a.no_stop()),
            other => panic!("expected upgrade, got {other:?}"),
        }

        let cmd =
            GInsCmd::try_parse_from(["gops", "prj", "upgrade", "latest", "--no-stop"]).unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(a)) => assert!(*a.no_stop()),
            other => panic!("expected upgrade, got {other:?}"),
        }

        // 与其它 flag 共存：不破坏 --to / --dry-run / --json。
        let cmd = GInsCmd::try_parse_from([
            "gops",
            "prj",
            "upgrade",
            "--to",
            "http://x/p.tar.gz",
            "--no-stop",
            "--dry-run",
            "--json",
        ])
        .unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(a)) => {
                assert!(*a.no_stop());
                assert!(*a.dry_run());
                assert!(*a.json());
                assert_eq!(a.to().as_deref(), Some("http://x/p.tar.gz"));
            }
            other => panic!("expected upgrade, got {other:?}"),
        }
    }

    #[test]
    fn test_switch_commands_stop_then_start_by_default() {
        assert_eq!(switch_commands(true), &["stop", "start"]);
        assert_eq!(switch_commands(false), &["start"]);
    }

    #[tokio::test]
    async fn test_run_switch_commands_stops_then_starts_by_default() {
        let seen = std::sync::Mutex::new(Vec::new());
        run_switch_commands(true, |cmd| {
            seen.lock().unwrap().push(cmd);
            async { Ok(()) }
        })
        .await
        .expect("switch sequence");
        assert_eq!(*seen.lock().unwrap(), vec!["stop", "start"]);
    }

    #[tokio::test]
    async fn test_run_switch_commands_skips_stop_with_no_stop() {
        let seen = std::sync::Mutex::new(Vec::new());
        run_switch_commands(false, |cmd| {
            seen.lock().unwrap().push(cmd);
            async { Ok(()) }
        })
        .await
        .expect("switch sequence");
        assert_eq!(*seen.lock().unwrap(), vec!["start"]);
    }

    #[tokio::test]
    async fn test_run_switch_commands_short_circuits_when_stop_fails() {
        let seen = std::sync::Mutex::new(Vec::new());
        let err = run_switch_commands(true, |cmd| {
            seen.lock().unwrap().push(cmd);
            async { Err(MainReason::logic_detail("stop failed")) }
        })
        .await
        .expect_err("stop failure must propagate");
        assert!(err.to_string().contains("stop failed"), "{err}");
        // stop 失败后**不能**继续 start：让上层按 --on-failure 处置。
        assert_eq!(*seen.lock().unwrap(), vec!["stop"]);
    }

    #[test]
    fn test_switch_mode_label_matches_flag() {
        assert!(switch_mode_label(true).contains("stop → up -d"));
        assert!(switch_mode_label(false).contains("--no-stop"));
    }

    #[test]
    fn test_prj_upgrade_parses_version_spec_and_channel() {
        // 位置参数 spec：`upgrade latest -c alpha`
        let cmd = GInsCmd::try_parse_from([
            "gops",
            "prj",
            "upgrade",
            "latest",
            "-c",
            "alpha",
            "--on-failure",
            "halt",
        ])
        .unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(a)) => {
                assert_eq!(a.name().as_deref(), Some("latest"));
                assert!(a.spec().is_none());
                assert_eq!(a.channel().as_deref(), Some("alpha"));
                assert!(a.to().is_none());
            }
            other => panic!("expected upgrade, got {other:?}"),
        }

        // 系统名 + 第二个位置参数 spec：`upgrade web-stack patch`
        let cmd = GInsCmd::try_parse_from([
            "gops",
            "prj",
            "upgrade",
            "web-stack",
            "patch",
            "--on-failure",
            "halt",
        ])
        .unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(a)) => {
                assert_eq!(a.name().as_deref(), Some("web-stack"));
                assert_eq!(a.spec().as_deref(), Some("patch"));
            }
            other => panic!("expected upgrade, got {other:?}"),
        }

        // `-v/--version` 与位置 spec 互斥；与 --to 也互斥。
        assert!(
            GInsCmd::try_parse_from([
                "gops",
                "prj",
                "upgrade",
                "web-stack",
                "patch",
                "-v",
                "latest",
                "--on-failure",
                "halt",
            ])
            .is_err()
        );
        assert!(
            GInsCmd::try_parse_from([
                "gops",
                "prj",
                "upgrade",
                "--to",
                "http://x/p.tar.gz",
                "-c",
                "alpha",
                "--on-failure",
                "halt",
            ])
            .is_err()
        );
        // 非法 channel 由 clap value_parser 拒绝。
        assert!(
            GInsCmd::try_parse_from([
                "gops",
                "prj",
                "upgrade",
                "latest",
                "-c",
                "gamma",
                "--on-failure",
                "halt",
            ])
            .is_err()
        );
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

    fn make_project_with_sys(root: &std::path::Path) {
        make_project(root);
        std::fs::write(
            root.join("ops-prj.yml"),
            "name: cust\nwork_envs:\n  dep_root: ''\n  deps: []\nsys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    path: ./s-v{version}.tar.gz\n",
        )
        .unwrap();
    }

    #[test]
    fn test_parse_upgrade_request_disambiguates_system_and_spec() {
        let tmp = TempDir::new().unwrap();
        make_project_with_sys(tmp.path());
        let project = load_project_from_cwd(tmp.path()).unwrap();

        // 位置参数 spec。
        let cmd = GInsCmd::try_parse_from([
            "gops",
            "prj",
            "upgrade",
            "latest",
            "-c",
            "alpha",
            "--on-failure",
            "halt",
        ])
        .unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(args)) => {
                let (sys, target) = parse_upgrade_request(&project, &args).unwrap();
                assert!(sys.is_none());
                assert!(matches!(target, UpgradeTarget::Spec(VersionSpec::Latest)));
            }
            other => panic!("expected upgrade, got {other:?}"),
        }

        // 系统名 + 第二个位置参数 spec。
        let cmd = GInsCmd::try_parse_from([
            "gops",
            "prj",
            "upgrade",
            "web-stack",
            "patch",
            "--on-failure",
            "halt",
        ])
        .unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(args)) => {
                let (sys, target) = parse_upgrade_request(&project, &args).unwrap();
                assert_eq!(sys.as_deref(), Some("web-stack"));
                assert!(matches!(target, UpgradeTarget::Spec(VersionSpec::Patch)));
            }
            other => panic!("expected upgrade, got {other:?}"),
        }

        // 精确版本不能配 --channel。
        let cmd = GInsCmd::try_parse_from([
            "gops",
            "prj",
            "upgrade",
            "web-stack",
            "-v",
            "0.10.0",
            "-c",
            "alpha",
            "--on-failure",
            "halt",
        ])
        .unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(args)) => {
                assert!(parse_upgrade_request(&project, &args).is_err());
            }
            other => panic!("expected upgrade, got {other:?}"),
        }

        // 未知系统名 / 非法 spec：明确报错，而不是当成版本去下载。
        let cmd =
            GInsCmd::try_parse_from(["gops", "prj", "upgrade", "typo", "--on-failure", "halt"])
                .unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(args)) => {
                assert!(parse_upgrade_request(&project, &args).is_err());
            }
            other => panic!("expected upgrade, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_upgrade_request_defaults_and_negative_cases() {
        let tmp = TempDir::new().unwrap();
        make_project_with_sys(tmp.path());
        let project = load_project_from_cwd(tmp.path()).unwrap();

        // 什么都没给 → 声明版本。
        let cmd =
            GInsCmd::try_parse_from(["gops", "prj", "upgrade", "--on-failure", "halt"]).unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(args)) => {
                let (sys, target) = parse_upgrade_request(&project, &args).unwrap();
                assert!(sys.is_none());
                assert!(matches!(target, UpgradeTarget::Spec(VersionSpec::Declared)));
            }
            other => panic!("expected upgrade, got {other:?}"),
        }

        // 只给系统名 → 声明版本。
        let cmd = GInsCmd::try_parse_from([
            "gops",
            "prj",
            "upgrade",
            "web-stack",
            "--on-failure",
            "halt",
        ])
        .unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(args)) => {
                let (sys, target) = parse_upgrade_request(&project, &args).unwrap();
                assert_eq!(sys.as_deref(), Some("web-stack"));
                assert!(matches!(target, UpgradeTarget::Spec(VersionSpec::Declared)));
            }
            other => panic!("expected upgrade, got {other:?}"),
        }

        // `-v latest` 与 `--to` 都可以不带位置 spec。
        let cmd = GInsCmd::try_parse_from([
            "gops",
            "prj",
            "upgrade",
            "-v",
            "latest",
            "--on-failure",
            "halt",
        ])
        .unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(args)) => {
                let (sys, target) = parse_upgrade_request(&project, &args).unwrap();
                assert!(sys.is_none());
                assert!(matches!(target, UpgradeTarget::Spec(VersionSpec::Latest)));
            }
            other => panic!("expected upgrade, got {other:?}"),
        }

        let cmd = GInsCmd::try_parse_from([
            "gops",
            "prj",
            "upgrade",
            "web-stack",
            "--to",
            "./pkg.tar.gz",
            "--on-failure",
            "halt",
        ])
        .unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(args)) => {
                let (sys, target) = parse_upgrade_request(&project, &args).unwrap();
                assert_eq!(sys.as_deref(), Some("web-stack"));
                assert!(matches!(target, UpgradeTarget::Address(_)));
            }
            other => panic!("expected upgrade, got {other:?}"),
        }

        // 声明版本 / 精确版本 + channel → 拒绝。
        let cmd = GInsCmd::try_parse_from([
            "gops",
            "prj",
            "upgrade",
            "web-stack",
            "-c",
            "alpha",
            "--on-failure",
            "halt",
        ])
        .unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(args)) => {
                assert!(parse_upgrade_request(&project, &args).is_err());
            }
            other => panic!("expected upgrade, got {other:?}"),
        }

        // 位置 spec 与 -v 重复 → 拒绝。
        let cmd = GInsCmd::try_parse_from([
            "gops",
            "prj",
            "upgrade",
            "latest",
            "-v",
            "patch",
            "--on-failure",
            "halt",
        ])
        .unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(args)) => {
                assert!(parse_upgrade_request(&project, &args).is_err());
            }
            other => panic!("expected upgrade, got {other:?}"),
        }

        // 第一个位置参数不是系统时，不能再跟第二个 spec。
        let cmd = GInsCmd::try_parse_from([
            "gops",
            "prj",
            "upgrade",
            "latest",
            "patch",
            "--on-failure",
            "halt",
        ])
        .unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(args)) => {
                assert!(parse_upgrade_request(&project, &args).is_err());
            }
            other => panic!("expected upgrade, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_upgrade_request_accepts_bump_aliases_and_no_save() {
        let tmp = TempDir::new().unwrap();
        make_project_with_sys(tmp.path());
        let project = load_project_from_cwd(tmp.path()).unwrap();

        for (input, expected) in [
            ("patch", VersionSpec::Patch),
            ("minor", VersionSpec::Minor),
            ("feat", VersionSpec::Minor),
            ("major", VersionSpec::Major),
        ] {
            let cmd =
                GInsCmd::try_parse_from(["gops", "prj", "upgrade", input, "--on-failure", "halt"])
                    .unwrap();
            match cmd {
                GInsCmd::Prj(PrjCmd::Upgrade(args)) => {
                    let (sys, target) = parse_upgrade_request(&project, &args).unwrap();
                    assert!(sys.is_none());
                    assert!(matches!(target, UpgradeTarget::Spec(spec) if spec == expected));
                }
                other => panic!("expected upgrade, got {other:?}"),
            }
        }

        // 显式 halt 仍可覆盖（大小写/空白归一化）。
        let cmd = GInsCmd::try_parse_from([
            "gops",
            "prj",
            "upgrade",
            "latest",
            "--on-failure",
            "  HALT ",
        ])
        .unwrap();
        match cmd {
            GInsCmd::Prj(PrjCmd::Upgrade(a)) => assert_eq!(a.on_failure(), "halt"),
            other => panic!("expected upgrade, got {other:?}"),
        }

        // 非法值在 CLI 解析阶段就拒绝。
        assert!(
            GInsCmd::try_parse_from([
                "gops",
                "prj",
                "upgrade",
                "latest",
                "--on-failure",
                "continue",
            ])
            .is_err()
        );
    }

    #[test]
    fn test_persist_upgrade_versions_writes_concrete_version() {
        let tmp = TempDir::new().unwrap();
        make_project_with_sys(tmp.path());
        let outcome = UpgradeOutcome {
            plan: galaxy_ops::ops_prj::upgrade::UpgradePlan {
                systems: vec!["web-stack".to_string()],
                target: "latest".to_string(),
                policy: "halt".to_string(),
                health: false,
                resolved: vec!["./s-v0.10.0.tar.gz".to_string()],
                target_versions: vec![Some("0.10.0".to_string())],
                no_op: false,
            },
            dry_run: false,
            record: None,
            record_path: tmp.path().join("upgrade.json"),
        };

        persist_upgrade_versions(tmp.path(), &outcome, true).unwrap();

        let reloaded = OpsProject::load(tmp.path()).unwrap();
        let sys = &reloaded.conf().sys_models()[0];
        assert_eq!(sys.declared_version(), Some("0.10.0"));
    }

    #[test]
    fn test_persist_upgrade_versions_skips_address_targets_and_keeps_other_conf() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());
        std::fs::write(
            tmp.path().join("ops-prj.yml"),
            "name: cust\nwork_envs:\n  dep_root: ''\n  deps: []\nbackup:\n  target: ./backup\nsys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    path: ./s-v{version}.tar.gz\n- sys:\n    name: api-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    path: ./api-v{version}.tar.gz\n",
        )
        .unwrap();
        let outcome = UpgradeOutcome {
            plan: galaxy_ops::ops_prj::upgrade::UpgradePlan {
                systems: vec!["web-stack".to_string(), "api-stack".to_string()],
                target: "latest".to_string(),
                policy: "halt".to_string(),
                health: false,
                resolved: vec!["./s-v0.10.0.tar.gz".to_string(), "./api.tar.gz".to_string()],
                target_versions: vec![Some("0.10.0".to_string()), None],
                no_op: false,
            },
            dry_run: false,
            record: None,
            record_path: tmp.path().join("upgrade.json"),
        };

        persist_upgrade_versions(tmp.path(), &outcome, true).unwrap();

        let reloaded = OpsProject::load(tmp.path()).unwrap();
        let web = &reloaded.conf().sys_models()[0];
        let api = &reloaded.conf().sys_models()[1];
        assert_eq!(web.declared_version(), Some("0.10.0"));
        assert_eq!(api.declared_version(), None);
        assert_eq!(
            reloaded.conf().backup().as_ref().unwrap().target(),
            "./backup",
            "persist must not drop other ops-prj.yml sections"
        );
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
