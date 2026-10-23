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
use galaxy_ops::system::diagnose::{
    self, Bundle, Context, DiagnoseOptions, Report, SystemInfo, checks,
};
use galaxy_ops::system::operator::SysOperator;
use galaxy_ops::system::sec::{SECRET_MASK, SecFile};

use crate::commands::common::DebugLogArgs;
use crate::commands::gx_dispatch::{self, FlowProbe};

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

/// `gops run diagnose` 的入参（设计：`docs/design/run-diagnose.md`）。
#[derive(Debug, Args)]
pub struct RunDiagnoseArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,

    #[arg(long = "mod", help = "mod name")]
    pub module: Option<String>,

    #[arg(short, long = "env", help = "env name", default_value = "default")]
    pub env: String,

    #[arg(long, help = "机器可读输出（stdout 只有 JSON，人读信息走 stderr）")]
    pub json: bool,

    #[arg(long = "no-ext", help = "跳过系统的 diagnose 流程（只跑内置检查）")]
    pub no_ext: bool,

    #[arg(
        long,
        default_value_t = 200,
        help = "每个容器抓多少行日志找错误线索（0 = 不抓）"
    )]
    pub logs: usize,

    #[arg(
        long = "log-ignore",
        value_name = "SUBSTR",
        help = "日志里包含该子串的行不算错误线索（可重复；大小写不敏感）"
    )]
    pub log_ignore: Vec<String>,

    #[arg(
        long,
        default_value_t = 60,
        help = "diagnose 流程超时（秒，钳在 5–3600）"
    )]
    pub timeout: u64,

    #[arg(
        long,
        value_name = "fail|warn",
        default_value = "fail",
        help = "失败门限：fail（默认，只看 FAIL）| warn（WARN 也算失败）"
    )]
    pub fail_on: String,

    #[arg(long, help = "JSON 里附上完整上下文（默认不带，避免变成配置转储）")]
    pub include_context: bool,

    #[arg(long, help = "保留诊断临时目录（context.json / result.json）")]
    pub keep_context: bool,

    #[arg(long, help = "不上色（等价 NO_COLOR）")]
    pub no_color: bool,
}

impl DfxArgsGetter for RunDiagnoseArgs {
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

    /// 打印渲染后的 compose 模型 (Show Rendered Compose Model)
    #[command(
        about = "打印渲染后的 compose 模型 (Show Rendered Compose Model)",
        long_about = "把 `docker compose config` 的结果打印出来：环境变量、挂载、端口、卷的**声明态**\
                     （密钥掩码）。它回答「实际交给 compose 的是什么」，**不是诊断**。\
                     以前这个行为藏在 `run diagnose` 后面（语义错位），现改为独立命令。"
    )]
    Config(SysOpsArgs),

    /// 诊断系统问题 (Diagnose System Issues)
    #[command(
        about = "诊断系统问题 (Diagnose System Issues)",
        long_about = "运行时体检（设计：docs/design/run-diagnose.md）：\
                     采集容器/挂载/端口/证书/日志的**实际事实** → 跑内置检查 → 执行系统的 \
                     `diagnose` 流程（`sys/workflows/operators.gxl`）拿业务结论 → 归并呈现。\
                     默认人读（`[OK]/[WARN]/[FAIL]` + 结论）；`--json` 出机器可读报告；\
                     退出码：0 = 无 FAIL，1 = 有 FAIL，2 = 诊断自身跑不起来。\
                     只读：不修任何东西，修复建议只以 `→` 提示给出。\
                     要看渲染后的配置用 `gops run config`。"
    )]
    Diagnose(RunDiagnoseArgs),
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
            RunCmd::Config(args) => Self::handle_ops_cmd("config", args).await,
            RunCmd::Diagnose(args) => Self::handle_diagnose(args).await,
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

    /// `gops run diagnose`：运行时体检（设计：`docs/design/run-diagnose.md`）。
    ///
    /// 流水线：采集事实 → 内置检查 → 系统 `diagnose` 流程 → 归并/呈现/退出码。
    /// 全程只读；**不**把流程失败当异常抛出（那是「结果」，用报告表达）。
    async fn handle_diagnose(args: RunDiagnoseArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);
        let dir = std::env::current_dir().expect("无法获取当前目录");

        let fail_on_warn = match args.fail_on.as_str() {
            "fail" => false,
            "warn" => true,
            other => {
                // 参数错 = 诊断没跑起来（契约 §8.3 的退出码 `2`），不能被当成“有 FAIL”。
                eprintln!("gops run diagnose: --fail-on 只接受 fail|warn，收到 `{other}`");
                std::process::exit(2);
            }
        };
        let options = DiagnoseOptions {
            log_lines: args.logs,
            log_ignore: args.log_ignore.clone(),
            timeout_secs: args.timeout.clamp(5, 3_600),
            fail_on_warn,
            no_ext: args.no_ext,
            include_context: args.include_context,
            keep_context: args.keep_context,
            env: args.env.clone(),
        };

        // 只读：**不**用 `SysOperator::load`（它会跑配置迁移）。`load_kind` 只读、且足以判 kind。
        let is_compose = matches!(SysOperator::load_kind(&dir), SysKind::DockerCompose);

        // 采集（只读）。`config` 要注入**掩码**后的密钥：上下文可能被流程读取。
        // 这里失败 = 诊断无法进行（退出码 2，与“有 FAIL”区分开）。
        let collected: MainResult<Context> = (|| {
            let system = SystemInfo {
                name: system_name(&dir),
                kind: if is_compose {
                    "docker-compose".to_string()
                } else {
                    "gxl".to_string()
                },
                root: dir.display().to_string(),
                env: options.env.clone(),
            };
            if is_compose {
                let sec =
                    galaxy_ops::system::sec::load_for_system(&dir)?.scoped(Subscriber::Run)?;
                let secret_env = sec_env_pairs_for("config", &sec);
                Context::collect(&dir, system, &secret_env)
            } else {
                Ok(Context::minimal(system))
            }
        })();
        let context = match collected {
            Ok(context) => context,
            Err(err) => {
                // 采集失败不是“体检结论”，不走报告（也不应该被当成一条可修复的检查）。
                eprintln!("[FAIL] gops.compose.render");
                eprintln!("       {err}");
                std::process::exit(2);
            }
        };

        let mut report = Report::default();
        if is_compose {
            report.extend(checks::builtin(
                &context,
                options.log_lines,
                &options.log_ignore,
            ));
        }

        // 系统的 `diagnose` 流程：两个 kind 都走同一个契约（§5 / §6）。
        // 交接目录建不出来 = 诊断无法进行（同采集失败：退出码 `2`）。
        let bundle = match Bundle::create(&context, options.keep_context) {
            Ok(bundle) => bundle,
            Err(err) => {
                eprintln!("[FAIL] gops.ext.diagnose");
                eprintln!("       {err}");
                std::process::exit(2);
            }
        };
        if !options.no_ext {
            Self::run_diagnose_flow(
                FlowRun {
                    dir: &dir,
                    context: &context,
                    bundle: &bundle,
                    options: &options,
                    is_compose,
                    module: args.module.as_deref(),
                    debug: args.debug_level(),
                },
                &mut report,
            )
            .await;
        } else if !is_compose {
            // gxl 的体检主体就是流程；显式关掉时要说清楚（不能报“全部正常”）。
            report.flow_skipped(diagnose::FLOW_NAME);
        }

        report.sort();
        let header = format!(
            "{} · {} · env={}",
            context.system.name, context.system.kind, context.system.env
        );
        if args.json {
            let system_json = serde_json::json!({
                "name": context.system.name,
                "kind": context.system.kind,
                "env": context.system.env,
                "root": context.system.root,
            });
            let context_json = options.include_context.then(|| context.to_json());
            println!(
                "{}",
                report.render_json(&system_json, context_json.as_ref(), options.fail_on_warn)
            );
        } else {
            let color = !args.no_color && galaxy_ops::report::use_color();
            print!(
                "{}",
                report.render_text(color, &header, options.fail_on_warn)
            );
        }
        // 有 FAIL 时下面直接 exit：先把 stdout 落盘（管道下是块缓冲）。
        use std::io::Write as _;
        let _ = std::io::stdout().flush();

        bundle.finish();
        if report.exit_code(options.fail_on_warn) != 0 {
            // 诊断的「失败」是**结果**，不是异常：退 1 让 `gops run diagnose || 处理` 可用。
            std::process::exit(1);
        }
        Ok(())
    }

    /// 跑系统的 `diagnose` 流程并把结论并进报告。
    ///
    /// 三种情形分开处理（不能混）：
    /// - 流程不存在：compose 项目正常（`-d 1` 提示）；gxl 项目说明体检没做（WARN）。
    /// - gx 不可用：FAIL（「可能声明了没跑」与「本来就没有」不是一回事）。
    /// - 流程失败 / 结果不合契约：FAIL，但**不掩盖**内置检查。
    async fn run_diagnose_flow(run: FlowRun<'_>, report: &mut Report) {
        let FlowRun {
            dir,
            context,
            bundle,
            options,
            is_compose,
            module,
            debug,
        } = run;
        let gx = gx_dispatch::gx_bin_path();
        match gx_dispatch::probe_flow(&gx, diagnose::FLOW_NAME, dir) {
            FlowProbe::Exists => {}
            FlowProbe::Absent(reason) => {
                if is_compose {
                    // compose 项目本来就可以没有流程：默认静默，`-d 1` 给出细节。
                    if debug >= 1 {
                        eprintln!("skip diagnose flow: {reason}");
                    }
                } else {
                    report.flow_absent(diagnose::FLOW_NAME, &short_reason(&reason));
                }
                return;
            }
            FlowProbe::Unavailable(reason) => {
                report.flow_failure(diagnose::FLOW_NAME, &short_reason(&reason));
                return;
            }
        }

        let mut env_pairs = bundle.env_pairs(context);
        // 流程要读 `WEB_DOMAIN` 这类合并后的值：`.env` 就是 localize 的生效结果。
        env_pairs = merge_flow_env(env_pairs, read_dotenv(dir));
        let timeout = std::time::Duration::from_secs(options.timeout_secs);
        if let Err(err) = gx_dispatch::run_gx_flow_capture(
            &gx,
            &options.env,
            0,
            diagnose::FLOW_NAME,
            module,
            &env_pairs,
            dir,
            timeout,
        )
        .await
        {
            report.flow_failure(diagnose::FLOW_NAME, &format!("{err}"));
            return;
        }

        match bundle.read_result() {
            Ok(Some(raw)) => match report.merge_flow_result(&raw, diagnose::FLOW_NAME) {
                Ok(0) => report.flow_empty(diagnose::FLOW_NAME),
                Ok(_) => {}
                Err(err) => report.flow_failure(
                    diagnose::FLOW_NAME,
                    &format!("result.json 不符合契约：{err}"),
                ),
            },
            Ok(None) => report.flow_empty(diagnose::FLOW_NAME),
            Err(err) => report.flow_failure(diagnose::FLOW_NAME, &format!("{err}")),
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
        // 配置错误（密钥策略 / 身份冲突）或宿主预置缺失（不在容器 gid 组里）
        // 都不能靠改权限修复，也不引导提权。
        Ok(report) if !report.errors.is_empty() => {
            Err(galaxy_ops::error::MainReason::logic_detail(format!(
                "宿主 bind 挂载有 {} 项无法靠改权限解决（`--apply` 不会修），按下面的 [FAIL] 处理\n{}",
                report.errors.len(),
                report.render_text()
            )))
        }
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

/// 只读地取系统名：`sys/sys_model.yml`（或旧布局 `sys_model.yml`）的 `name`；取不到就用目录名。
///
/// 为什么不用 `SysOperator::load`：它会跑配置迁移（写盘），而 `diagnose` 必须全程只读。
fn system_name(root: &Path) -> String {
    #[derive(serde_derive::Deserialize)]
    struct Head {
        name: String,
    }
    for candidate in ["sys/sys_model.yml", "sys_model.yml"] {
        let Ok(text) = std::fs::read_to_string(root.join(candidate)) else {
            continue;
        };
        if let Ok(head) = serde_yaml::from_str::<Head>(&text)
            && !head.name.trim().is_empty()
        {
            return head.name;
        }
    }
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "system".to_string())
}

/// 一次 `diagnose` 流程调用的输入（参数较多，打包成结构体）。
struct FlowRun<'a> {
    dir: &'a Path,
    context: &'a Context,
    bundle: &'a Bundle,
    options: &'a DiagnoseOptions,
    is_compose: bool,
    module: Option<&'a str>,
    debug: usize,
}

/// gx 的报错常带多行 trace；报告里只留前几行（完整输出用 `-d 1` / 单独跑流程）。
fn short_reason(reason: &str) -> String {
    const MAX_LINES: usize = 3;
    const MAX_CHARS: usize = 300;
    let mut out = reason
        .lines()
        .take(MAX_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    if out.chars().count() > MAX_CHARS {
        out = out.chars().take(MAX_CHARS).collect::<String>();
        out.push('…');
    }
    out
}

/// 把 `.env` 的现场值并进流程环境。
///
/// `GOPS_*` 是**契约变量**，不允许被现场值覆盖（否则一个 `.env` 就能劫持交接路径）；
/// 其余键后写覆盖前写（与 dotenv 的直觉一致）。
fn merge_flow_env(
    mut contract: Vec<(String, String)>,
    dotenv: Vec<(String, String)>,
) -> Vec<(String, String)> {
    contract.extend(
        dotenv
            .into_iter()
            .filter(|(key, _)| !key.starts_with("GOPS_")),
    );
    contract
}

/// 读 `<系统根>/.env`（localize 的生效结果）给流程注入；解析失败的行直接跳过。
fn read_dotenv(root: &Path) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(root.join(".env")) else {
        return Vec::new();
    };
    let mut pairs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        // 合法环境变量名：首字符不能是数字（`1KEY=x` 注入了也读不到）。
        let valid = !key.is_empty()
            && key
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
            && key
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
        if !valid {
            continue;
        }
        let value = value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        pairs.push((key.to_string(), value));
    }
    pairs
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
        "config" => ("config", &[]),
        other => (other, &[]),
    }
}

/// `gops run` 的哪些子命令需要密钥。
///
/// 只有会把变量真正插进配置/容器的才需要：`start`(up) / `install`(create) / `diagnose`(config)。
/// `download`(pull) / `stop` / `status`(ps) / `uninstall`(down) 不需要 ——
/// 否则密钥文件权限不对时，连 `gops run status` 都跑不了，反而破坏排障。
fn needs_secrets(cmd_name: &str) -> bool {
    matches!(cmd_name, "start" | "install" | "config")
}

/// 把 secret dict（SEC_* → 明文值）转成要注入子进程的 (KEY, VALUE) 环境变量对。
/// `diagnose`（docker compose config）是只读校验，注入掩码值以免泄露明文；其余命令注入明文。
fn sec_env_pairs_for(cmd_name: &str, sec: &SecFile) -> Vec<(String, String)> {
    // `config` 是只读校验（渲染结果给人看 / 给诊断看），注入掩码值以免泄露明文。
    let mask = cmd_name == "config";
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
    use super::{
        compose_subcommand, merge_flow_env, needs_secrets, read_dotenv, sec_env_pairs_for,
        short_reason, system_name,
    };
    use galaxy_ops::system::compose::compose_global_args;
    use galaxy_ops::system::sec::SecFile;
    use std::ffi::OsString;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn diagnose_flags_parse_with_defaults() {
        use crate::commands::GInsCmd;
        use clap::Parser;

        let parsed = GInsCmd::try_parse_from(["gops", "run", "diagnose"]).unwrap();
        match parsed {
            GInsCmd::Run(super::RunCmd::Diagnose(args)) => {
                assert_eq!(args.logs, 200);
                assert_eq!(args.timeout, 60);
                assert_eq!(args.fail_on, "fail");
                assert_eq!(args.env, "default");
                assert!(!args.json && !args.no_ext && !args.no_color && !args.keep_context);
            }
            _ => panic!("run diagnose 应解析为 RunCmd::Diagnose"),
        }

        let parsed = GInsCmd::try_parse_from([
            "gops",
            "run",
            "diagnose",
            "--json",
            "--no-ext",
            "--logs",
            "0",
            "--fail-on",
            "warn",
            "--timeout",
            "15",
            "-e",
            "prod",
        ])
        .unwrap();
        match parsed {
            GInsCmd::Run(super::RunCmd::Diagnose(args)) => {
                assert!(args.json && args.no_ext);
                assert_eq!(args.logs, 0);
                assert_eq!(args.fail_on, "warn");
                assert_eq!(args.timeout, 15);
                assert_eq!(args.env, "prod");
            }
            _ => panic!("run diagnose 应解析为 RunCmd::Diagnose"),
        }
    }

    #[test]
    fn config_is_its_own_subcommand() {
        use crate::commands::GInsCmd;
        use clap::Parser;
        let parsed = GInsCmd::try_parse_from(["gops", "run", "config"]).unwrap();
        assert!(matches!(parsed, GInsCmd::Run(super::RunCmd::Config(_))));
    }

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
        check("config", "config", &[]);
        // 未知命令透传
        check("whatever", "whatever", &[]);
    }

    #[test]
    fn test_needs_secrets_only_for_materializing_commands() {
        for cmd in ["start", "install", "config"] {
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
        // config 注入掩码值，不泄露明文
        let pairs = sec_env_pairs_for("config", &sec);
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
        // 非 config 注入明文
        let pairs = sec_env_pairs_for("start", &sec);
        assert!(pairs.contains(&("SEC_DB_PASSWORD".to_string(), "pw".to_string())));
        assert!(pairs.contains(&("SEC_API_KEY".to_string(), "tok-123".to_string())));
    }

    #[test]
    fn test_system_name_reads_sys_model_yml_and_falls_back_to_dir_name() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("my-stack");
        std::fs::create_dir_all(root.join("sys")).unwrap();
        assert_eq!(
            system_name(&root),
            "my-stack",
            "无 sys_model.yml 时用目录名"
        );

        std::fs::write(
            root.join("sys/sys_model.yml"),
            "name: real-name\nkind: docker-compose\n",
        )
        .unwrap();
        assert_eq!(system_name(&root), "real-name");
    }

    #[test]
    fn test_system_name_supports_legacy_root_layout() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("sys_model.yml"), "name: legacy-name\n").unwrap();
        assert_eq!(system_name(dir.path()), "legacy-name");
    }

    #[test]
    fn test_merge_flow_env_never_lets_dotenv_hijack_the_contract() {
        let contract = vec![
            (
                "GOPS_DIAGNOSE_RESULT".to_string(),
                "/tmp/r.json".to_string(),
            ),
            ("GOPS_SYSTEM_NAME".to_string(), "s".to_string()),
        ];
        let dotenv = vec![
            ("GOPS_DIAGNOSE_RESULT".to_string(), "/evil".to_string()),
            ("WEB_DOMAIN".to_string(), "gw.example".to_string()),
        ];
        let merged = merge_flow_env(contract, dotenv);
        assert_eq!(
            merged
                .iter()
                .find(|(key, _)| key == "GOPS_DIAGNOSE_RESULT")
                .map(|(_, value)| value.as_str()),
            Some("/tmp/r.json")
        );
        assert!(
            merged
                .iter()
                .any(|(key, value)| key == "WEB_DOMAIN" && value == "gw.example")
        );
    }

    #[test]
    fn test_short_reason_keeps_a_few_lines_and_bounds_length() {
        let multi = "line1\nline2\nline3\nline4\n";
        assert_eq!(short_reason(multi), "line1\nline2\nline3");
        let long = "x".repeat(400);
        let out = short_reason(&long);
        assert_eq!(out.chars().count(), 301);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn test_read_dotenv_missing_file_is_empty() {
        let dir = tempdir().unwrap();
        assert!(read_dotenv(dir.path()).is_empty());
    }

    #[test]
    fn test_read_dotenv_skips_comments_and_quotes_values() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".env"),
            "# comment\nWEB_DOMAIN=gw.example\nAPI=\"https://api\"\nBAD LINE\n1KEY=x\n",
        )
        .unwrap();
        let pairs = read_dotenv(root);
        assert!(pairs.contains(&("WEB_DOMAIN".to_string(), "gw.example".to_string())));
        assert!(pairs.contains(&("API".to_string(), "https://api".to_string())));
        assert_eq!(pairs.len(), 2, "注释 / 非法行不注入：{pairs:?}");
    }
}
