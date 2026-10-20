//! `gops run`：在目标系统上执行**标准运维动作**（算子流契约）。
//!
//! 这些动作与 `ops-gxl` 的算子流程一一对应：`gxl` 系统分派为 `gx run <cmd>`，
//! `docker-compose` 系统映射为 `docker compose` 子命令。
//!
//! 与 `gops sys`（系统**定义/交付/工件**：new/update/localize/package/setting/check）区分：
//! 此处只管「在环境里落地/运行」（取得制品、安装/卸载、启停、状态、诊断）。

use std::path::Path;

use clap::{Args, Parser};
use derive_getters::Getters;
use tokio::process::Command as TokioCommand;

use galaxy_ops::error::MainResult;
use galaxy_ops::infra::DfxArgsGetter;
use galaxy_ops::system::Subscriber;
use galaxy_ops::system::SysKind;
use galaxy_ops::system::compose::compose_invocation;
use galaxy_ops::system::operator::SysOperator;
use galaxy_ops::system::sec::{SECRET_MASK, SecFile};

use crate::commands::common::DebugLogArgs;
use crate::commands::gx_dispatch;

#[derive(Debug, Args, Getters)]
pub struct SysOpsArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,

    #[arg(long = "mod", help = "mod name")]
    pub module: Option<String>,

    #[arg(short, long = "env", help = "env name", default_value = "default")]
    pub env: String,

    #[arg(
        long = "no-perms-check",
        help = "start 时跳过宿主 bind 挂载权限 preflight（docker-compose 系统）"
    )]
    pub no_perms_check: bool,
}

impl DfxArgsGetter for SysOpsArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

#[derive(Debug, Parser)]
pub enum RunCmd {
    /// 下载系统组件 (Download System Components)
    #[command(
        about = "下载系统组件 (Download System Components)",
        long_about = "从指定源下载系统所需的组件、依赖或资源。支持指定特定的模块和环境，\
                     便于在不同配置下获取相应的系统组件。\n\
                     Download required components, dependencies, or resources for the system from specified sources. \
                     Supports specifying particular modules and environments for obtaining corresponding system components \
                     under different configurations."
    )]
    Download(SysOpsArgs),

    /// 安装系统组件 (Install System Components)
    #[command(
        about = "安装系统组件 (Install System Components)",
        long_about = "安装已下载的系统组件到目标环境。支持模块化安装和环境特定配置，\
                     确保系统组件正确部署到指定的环境中。\n\
                     Install downloaded system components to the target environment. Supports modular installation and \
                     environment-specific configurations to ensure system components are properly deployed to specified environments."
    )]
    Install(SysOpsArgs),

    /// 卸载系统组件 (Uninstall System Components)
    #[command(
        about = "卸载系统组件 (Uninstall System Components)",
        long_about = "从系统中移除已安装的组件。支持安全卸载指定模块的组件，\
                     清理相关配置和依赖，确保系统状态的完整性。\n\
                     Remove installed components from the system. Supports safe uninstallation of specified module components, \
                     cleaning up related configurations and dependencies to ensure system state integrity."
    )]
    Uninstall(SysOpsArgs),

    /// 启动系统服务 (Start System Services)
    #[command(
        about = "启动系统服务 (Start System Services)",
        long_about = "启动指定的系统服务或组件。支持按模块和环境启动服务，\
                     提供调试日志输出，便于监控启动过程和故障排除。\n\
                     Start specified system services or components. Supports starting services by module and environment, \
                     providing debug log output for monitoring the startup process and troubleshooting."
    )]
    Start(SysOpsArgs),

    /// 停止系统服务 (Stop System Services)
    #[command(
        about = "停止系统服务 (Stop System Services)",
        long_about = "停止正在运行的系统服务或组件。支持优雅停机过程，\
                     确保服务正常关闭并清理相关资源，维护系统稳定性。\n\
                     Stop running system services or components. Supports graceful shutdown processes to ensure \
                     services terminate normally and clean up related resources, maintaining system stability."
    )]
    Stop(SysOpsArgs),

    /// 查询系统状态 (Query System Status)
    #[command(
        about = "查询系统状态 (Query System Status)",
        long_about = "获取系统服务和组件的当前运行状态。支持按模块和环境过滤状态信息，\
                     提供详细的运行时状态和健康检查结果。\n\
                     Retrieve current runtime status of system services and components. Supports filtering status information \
                     by module and environment, providing detailed runtime status and health check results."
    )]
    Status(SysOpsArgs),

    /// 诊断系统问题 (Diagnose System Issues)
    #[command(
        about = "诊断系统问题 (Diagnose System Issues)",
        long_about = "对系统进行全面诊断和故障排除。支持针对特定模块和环境进行诊断，\
                     生成详细的诊断报告和建议解决方案。\n\
                     Perform comprehensive system diagnosis and troubleshooting. Supports targeted diagnosis for specific \
                     modules and environments, generating detailed diagnostic reports and suggested solutions."
    )]
    Diagnose(SysOpsArgs),
}

pub struct RunCommandHandler;

impl RunCommandHandler {
    pub async fn execute(cmd: RunCmd) -> MainResult<()> {
        match cmd {
            RunCmd::Download(args) => Self::handle_ops_cmd("download", args).await,
            RunCmd::Install(args) => Self::handle_ops_cmd("install", args).await,
            RunCmd::Uninstall(args) => Self::handle_ops_cmd("uninstall", args).await,
            RunCmd::Start(args) => Self::handle_ops_cmd("start", args).await,
            RunCmd::Stop(args) => Self::handle_ops_cmd("stop", args).await,
            RunCmd::Status(args) => Self::handle_ops_cmd("status", args).await,
            RunCmd::Diagnose(args) => Self::handle_ops_cmd("diagnose", args).await,
        }
    }

    pub async fn handle_ops_cmd(cmd_name: &str, args: SysOpsArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);

        let current_dir = std::env::current_dir().expect("无法获取当前目录");
        match SysOperator::load_kind(&current_dir) {
            SysKind::DockerCompose => {
                // preflight：权限未对齐时停在 `docker compose up` 之前（否则容器只会无限重启）。
                if cmd_name == "start" && !args.no_perms_check {
                    perms_preflight(&current_dir, false)?;
                }
                Self::run_compose_cmd(&current_dir, cmd_name, args.module().as_deref()).await
            }
            SysKind::Gxl => Self::run_gx_cmd(cmd_name, &args).await,
        }
    }

    /// 在**指定系统目录**上跑 docker-compose 运行时动作（`gops prj upgrade` 的事务用）。
    ///
    /// 只支持 `kind: docker-compose`：`gxl` 的分派（`gx run`）是按 **CWD** 解析工程的，
    /// 无法按目录重定向，另行设计（见 `docs/design/prj-upgrade.md` §3.5）。
    pub(crate) async fn compose_cmd_in(dir: &Path, cmd_name: &str) -> MainResult<()> {
        if !matches!(SysOperator::load_kind(dir), SysKind::DockerCompose) {
            return Err(galaxy_ops::error::MainReason::logic_detail(
                "gops prj upgrade 第一版只支持 kind: docker-compose 的系统",
            ));
        }
        // 升级的 `up` 直接修复权限：apply/regenerate/restore 之后，root 属主主要从这里来。
        if cmd_name == "start" {
            perms_preflight(dir, true)?;
        }
        Self::run_compose_cmd(dir, cmd_name, None).await
    }

    /// `kind: gxl` 的系统：把 `gops run <cmd>` 映射为 `gx run <cmd>`。
    ///
    /// 系统的 `start` / `stop` / `status` / `diagnose` 等即工作流里的同名流程
    /// （定义在 `_gal/work.gxl` 或其引用的模块中）。
    async fn run_gx_cmd(cmd_name: &str, args: &SysOpsArgs) -> MainResult<()> {
        gx_dispatch::check_gx_version()?;

        let gx_path = gx_dispatch::gx_bin_path();
        let module = args.module();
        if let Some(module) = module {
            println!("use module :{module}");
        }

        // 与 compose 的可选阶段流程共用同一入口 `run_gx_flow`。
        // gxl 分派按 CWD 解析工程（`gops run` 从系统目录执行），故传 `None`。
        gx_dispatch::run_gx_flow(
            &gx_path,
            args.env(),
            args.debug_level(),
            module.as_deref(),
            cmd_name,
            &[],
            None,
        )
        .await
    }

    async fn run_compose_cmd(dir: &Path, cmd_name: &str, module: Option<&str>) -> MainResult<()> {
        // 将 gops run 语义映射到 docker compose 子命令：
        //   download -> pull, install -> create, start -> up -d, stop -> stop,
        //   uninstall -> down, status -> ps, diagnose -> config
        let (subcommand, extra) = compose_subcommand(cmd_name);

        if module.is_some() {
            println!("note: docker-compose 类型系统忽略 --mod 参数");
        }

        let mut cmd = TokioCommand::new("docker");
        // compose 文件位置可变（默认 `sys/docker-compose.yaml`），但项目目录始终锚定到系统根，
        // 使相对挂载与 .env 的基准保持不变。
        let (_compose_file, global_args) = compose_invocation(dir);
        cmd.arg("compose")
            .args(global_args)
            .arg(subcommand)
            .args(extra);
        cmd.current_dir(dir);

        // 密钥按 `sys-prj.yml: secrets.file` 解析（路径限定 / 权限 / 值类型校验都在
        // `system::sec` 里做）；未声明时回退 `.galaxy/sec_value.yml` 旧约定。
        // 再按 `secrets.expose.run` 裁剪（缺省 all），只给 compose 它该看的键。
        // diagnose（docker compose config）只读校验，注入掩码值以免泄露明文。
        //
        // 只对“会落地/渲染配置”的子命令加载：密钥文件损坏时不应该连 `status` 都跑不了。
        if needs_secrets(cmd_name) {
            let sec = galaxy_ops::system::sec::load_for_system(dir)?.scoped(Subscriber::Run)?;
            for (key, value) in sec_env_pairs_for(cmd_name, &sec) {
                cmd.env(key, value);
            }
        }

        gx_dispatch::run_and_stream(cmd, "docker compose").await
    }
}

/// bind 挂载权限的运行时 preflight。
///
/// - `apply = false`：发现问题就拒绝启动，并给出显式修复命令；
/// - `apply = true`：先修复再启动（`prj upgrade` 的 `up` 用）。
///
/// 非 Linux / 无法派生（无 docker、无 compose）时**只告警**：这些环境本来也不需要这套对齐，
/// 而 `docker compose up` 自己会给出更直接的错误。
fn perms_preflight(dir: &Path, apply_fix: bool) -> MainResult<()> {
    if !cfg!(target_os = "linux") {
        return Ok(());
    }
    match galaxy_ops::system::perms::run_with_conf(dir, apply_fix) {
        Ok(report) if report.is_ok() => Ok(()),
        Ok(report) => Err(galaxy_ops::error::MainReason::logic_detail(format!(
            "宿主 bind 挂载权限未对齐（{} 项待修 / {} 项失败）；先运行 `gops sys perms --apply`\n{}",
            report.pending(),
            report.failed(),
            report.render_text()
        ))),
        Err(err) => {
            eprintln!("[WARN] 宿主权限 preflight 跳过：{err}");
            Ok(())
        }
    }
}

/// 把 gops run 命令名映射为 docker compose 子命令（及附加参数）。
fn compose_subcommand(cmd_name: &str) -> (&str, &'static [&'static str]) {
    match cmd_name {
        "download" => ("pull", &[]),
        "install" => ("create", &[]),
        "start" => ("up", &["-d"]),
        "stop" => ("stop", &[]),
        "uninstall" => ("down", &[]),
        "status" => ("ps", &[]),
        "diagnose" => ("config", &[]),
        other => (other, &[]),
    }
}

/// `gops run` 的哪些子命令需要密钥。
///
/// 只有会把变量真正插进配置/容器的才需要：`start`(up) / `install`(create) / `diagnose`(config)。
/// `download`(pull) / `stop` / `status`(ps) / `uninstall`(down) 不需要 ——
/// 否则密钥文件权限不对时，连 `gops run status` 都跑不了，反而破坏排障。
fn needs_secrets(cmd_name: &str) -> bool {
    matches!(cmd_name, "start" | "install" | "diagnose")
}

/// 把 secret dict（SEC_* → 明文值）转成要注入子进程的 (KEY, VALUE) 环境变量对。
/// `diagnose`（docker compose config）是只读校验，注入掩码值以免泄露明文；其余命令注入明文。
fn sec_env_pairs_for(cmd_name: &str, sec: &SecFile) -> Vec<(String, String)> {
    let mask = cmd_name == "diagnose";
    sec.env_pairs()
        .into_iter()
        .map(|(key, value)| {
            let value = if mask { SECRET_MASK.to_string() } else { value };
            (key, value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{compose_subcommand, needs_secrets, sec_env_pairs_for};
    use galaxy_ops::system::compose::compose_global_args;
    use galaxy_ops::system::sec::SecFile;
    use std::ffi::OsString;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn test_compose_subcommand() {
        fn check(cmd: &str, sub: &str, extra: &[&str]) {
            let (s, e) = compose_subcommand(cmd);
            assert_eq!(s, sub);
            assert_eq!(e, extra);
        }
        check("download", "pull", &[]);
        check("install", "create", &[]);
        check("start", "up", &["-d"]);
        check("stop", "stop", &[]);
        check("uninstall", "down", &[]);
        check("status", "ps", &[]);
        check("diagnose", "config", &[]);
        // 未知命令透传
        check("whatever", "whatever", &[]);
    }

    #[test]
    fn test_needs_secrets_only_for_materializing_commands() {
        for cmd in ["start", "install", "diagnose"] {
            assert!(needs_secrets(cmd), "{cmd} 应该需要密钥");
        }
        for cmd in ["download", "stop", "status", "uninstall"] {
            assert!(!needs_secrets(cmd), "{cmd} 不应该读密钥文件");
        }
    }

    #[test]
    fn test_compose_global_args_anchor_to_project_dir() {
        let root = Path::new("/srv/gateway");
        let file = root.join("sys/docker-compose.yaml");

        assert_eq!(
            compose_global_args(Some(&file), root),
            vec![
                OsString::from("-f"),
                OsString::from("/srv/gateway/sys/docker-compose.yaml"),
                OsString::from("--project-directory"),
                OsString::from("/srv/gateway"),
            ]
        );
    }

    #[test]
    fn test_compose_global_args_empty_for_root_layout() {
        // 旧布局（compose 就在系统根）：不传 global args，退回 docker 自动发现，
        // 以保留 docker-compose.override.yml 的自动合并与 COMPOSE_FILE 环境变量语义。
        let root = Path::new("/srv/gateway");
        assert!(compose_global_args(Some(&root.join("docker-compose.yml")), root).is_empty());
        assert!(compose_global_args(Some(&root.join("compose.yaml")), root).is_empty());
    }

    #[test]
    fn test_compose_global_args_merges_override_in_sys_layout() {
        // 显式 -f 会关闭 override 自动合并，因此内收布局要把同目录的 override 显式带上。
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sys")).unwrap();
        std::fs::write(root.join("sys/docker-compose.yaml"), "base").unwrap();
        std::fs::write(root.join("sys/docker-compose.override.yaml"), "override").unwrap();
        let file = root.join("sys/docker-compose.yaml");

        assert_eq!(
            compose_global_args(Some(&file), root),
            vec![
                OsString::from("-f"),
                root.join("sys/docker-compose.yaml").into_os_string(),
                OsString::from("-f"),
                root.join("sys/docker-compose.override.yaml")
                    .into_os_string(),
                OsString::from("--project-directory"),
                root.as_os_str().to_os_string(),
            ]
        );
    }

    #[test]
    fn test_compose_global_args_empty_without_compose_file() {
        // 未找到 compose 文件时不加全局参数，仍交由 docker 自行发现（兼容旧布局）
        assert!(compose_global_args(None, Path::new("/srv/gateway")).is_empty());
    }

    #[test]
    fn test_sec_env_pairs_for_diagnose_masks() {
        let sec = SecFile::from_pairs(
            "test.yml",
            vec![
                ("SEC_DB_PASSWORD".to_string(), "pw".to_string()),
                ("SEC_API_KEY".to_string(), "tok-123".to_string()),
            ],
        );
        // diagnose 注入掩码值，不泄露明文
        let pairs = sec_env_pairs_for("diagnose", &sec);
        assert!(pairs.contains(&("SEC_DB_PASSWORD".to_string(), "********".to_string())));
        assert!(pairs.contains(&("SEC_API_KEY".to_string(), "********".to_string())));
    }

    #[test]
    fn test_sec_env_pairs_for_start_plaintext() {
        let sec = SecFile::from_pairs(
            "test.yml",
            vec![
                ("SEC_DB_PASSWORD".to_string(), "pw".to_string()),
                ("SEC_API_KEY".to_string(), "tok-123".to_string()),
            ],
        );
        // 非 diagnose 注入明文
        let pairs = sec_env_pairs_for("start", &sec);
        assert!(pairs.contains(&("SEC_DB_PASSWORD".to_string(), "pw".to_string())));
        assert!(pairs.contains(&("SEC_API_KEY".to_string(), "tok-123".to_string())));
    }
}
