use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command as TokioCommand;

use clap::{Args, Parser};
use derive_getters::Getters;
use dialoguer::Select;
use galaxy_ops::const_vars::{SETTING_DIR, USER_VALUE_FILE, VALUE_DIR};
use galaxy_ops::error::MainResult;
use galaxy_ops::infra::DfxArgsGetter;
use galaxy_ops::module::ModelSTD;
use galaxy_ops::prelude::{ErrorConv, ErrorOwe};
use galaxy_ops::system::operator::SysOperator;
use galaxy_ops::system::setting::SysSetting;
use galaxy_ops::system::{SysKind, SysValuePaths};
use galaxy_ops::types::{LocalizeOptions, RefUpdateable};
use orion_conf::YamlIO;
use orion_infra::path::ensure_path;
use orion_variate::archive::compress;
use orion_variate::update::DownloadOptions;
use orion_vars::vars::{OriginDict, ValueDict};

use crate::commands::common::DebugLogArgs;

/// 解析当前系统的值目录：若系统属于某个运维项目（父目录有 `ops-prj.yml` 且列出该系统），
/// 使用项目为该系统维护的 `values/<sys_name>`（客户值）；否则使用 `<sys>/values`。
fn resolve_sys_value_path(sys_dir: &Path) -> SysValuePaths {
    match galaxy_ops::ops_prj::project::owner_project_value_dir(sys_dir) {
        Some(dir) => SysValuePaths::from(dir),
        None => SysValuePaths::from(sys_dir.to_path_buf()).join(VALUE_DIR),
    }
}

// === 参数定义 ===

#[derive(Debug, Args, Getters)]
pub struct SysNewArgs {
    #[arg(
        short,
        long,
        help = "系统名称 (System name): 字母数字，可包含连字符和下划线\nalphanumeric with hyphens/underscores"
    )]
    pub(crate) name: String,

    #[arg(
        long,
        help = "系统部署类型 (System kind): gxl | docker-compose\n不指定时交互式选择。gxl=模块化 GXL 工作流系统; docker-compose=声明式 docker compose 系统"
    )]
    pub(crate) kind: Option<String>,
}

#[derive(Debug, Args, Getters)]
pub struct SysUpdateArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,

    #[arg(short, long, help = "update force", default_value = "false")]
    pub force: bool,
}

#[derive(Debug, Args, Getters)]
pub struct SysPackageArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,

    #[arg(short, long, help = "update force", default_value = "false")]
    pub force: bool,

    #[arg(
        short,
        long = "output",
        help = "输出 tar.gz 路径 (默认: ../<name>-<version>.tar.gz)"
    )]
    pub output: Option<String>,
}

#[derive(Debug, Args, Getters)]
pub struct SysLocalizeArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,

    #[arg(long = "mod", help = "mod name")]
    pub module: Option<String>,

    #[arg(long, help = "只 localize，跳过 update（不解析/下载模块）")]
    pub only: bool,
}

#[derive(Debug, Args, Getters)]
pub struct SysSettingArgs {
    #[arg(long, help = "init sys setting")]
    pub init: bool,
}

#[derive(Debug, Args, Getters)]
pub struct SysOpsArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,

    #[arg(long = "mod", help = "mod name")]
    pub module: Option<String>,

    #[arg(short, long = "env", help = "env name", default_value = "default")]
    pub env: String,
}

#[derive(Debug, Parser)]
pub enum SysCmd {
    /// 创建新的系统操作符 (Create New System Operator)
    #[command(
        about = "创建新的系统维护器 (Create New System Operator)",
        long_about = "使用给定的名称创建新的系统规范。这将初始化一个新的系统目录结构，其中包含所有必要的配置文件和模板。\n\
                     Create a new system specification with the given name. This will initialize a new system directory structure with all necessary configuration files and templates."
    )]
    New(SysNewArgs),

    /// 更新系统配置 (Update System Configuration)
    #[command(
        about = "更新系统配置 (Update System Configuration)",
        long_about = "更新现有系统的配置、规范或依赖关系。支持强制更新以在不确认的情况下覆盖现有配置。\n\
                     Update an existing system's configuration, specifications, or dependencies. Supports force updates to override existing configurations without confirmation."
    )]
    Update(SysUpdateArgs),

    /// 打包系统 (Package System)
    #[command(
        about = "打包系统 (Package System)",
        long_about = "先更新系统（解析模块变量并生成 merged_vars.yml），再打包为可交付的 .tar.gz。\n\
                     Update the system first (resolve module variables and generate merged_vars.yml), then package it into a deliverable .tar.gz."
    )]
    Package(SysPackageArgs),

    /// 为环境本地化系统配置 (Localize System Configuration for Environment)
    #[command(
        about = "为环境本地化系统配置 (Localize System Configuration for Environment)",
        long_about = "基于环境特定值为系统生成本地化配置文件。适用于将系统配置适配到不同的部署环境。\n\
                     Generate localized configuration files for the system based on environment-specific values. Useful for adapting system configurations to different deployment environments."
    )]
    Localize(SysLocalizeArgs),

    /// 初始化系统设置 (Initialize System Settings)
    #[command(
        about = "初始化系统设置 (Initialize System Settings)",
        long_about = "在当前目录下创建系统设置文件。这会生成一个包含默认配置的系统设置示例文件，\
                     可作为系统配置的基础模板。\n\
                     Create system settings files in the current directory. This generates a sample system settings \
                     file with default configurations that can serve as a base template for system configuration."
    )]
    Setting(SysSettingArgs),

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

// === DfxArgsGetter 实现 ===

impl DfxArgsGetter for SysNewArgs {
    fn debug_level(&self) -> usize {
        0
    }
    fn log_setting(&self) -> Option<String> {
        None
    }
}

impl DfxArgsGetter for SysUpdateArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

impl DfxArgsGetter for SysPackageArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

impl DfxArgsGetter for SysLocalizeArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}
impl DfxArgsGetter for SysOpsArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

// === 命令处理器 ===

pub struct SysCommandHandler;

impl SysCommandHandler {
    /// 交互式选择系统部署类型；测试环境默认 gxl，避免交互卡住。
    fn ia_kind() -> MainResult<SysKind> {
        if std::env::var("TEST_MODE").is_ok() {
            return Ok(SysKind::Gxl);
        }
        let options = vec!["gxl".to_string(), "docker-compose".to_string()];
        let index = Select::new()
            .with_prompt("请选择部署类型:")
            .items(&options)
            .interact()
            .unwrap();
        Ok(if index == 0 {
            SysKind::Gxl
        } else {
            SysKind::DockerCompose
        })
    }

    fn ia_model_std() -> MainResult<ModelSTD> {
        let support_models = ModelSTD::support();
        let options: Vec<String> = support_models
            .iter()
            .map(|model| format!("{model}"))
            .collect();

        // 检查是否在测试环境中
        if std::env::var("TEST_MODE").is_ok() {
            // 在测试环境中，自动选择第一个支持的模式
            if let Some(first_model) = support_models.first() {
                return Ok(first_model.clone());
            } else {
                return Ok(ModelSTD::from_cur_sys());
            }
        }

        let index = Select::new()
            .with_prompt("请选择系统型号配置:")
            .items(&options)
            .interact()
            .unwrap();
        if index < support_models.len() {
            Ok(support_models[index].clone())
        } else {
            Ok(ModelSTD::from_cur_sys()) // 兜底处理
        }
    }

    pub async fn handle_new(args: SysNewArgs) -> MainResult<()> {
        let current_dir = std::env::current_dir().expect("无法获取当前目录");
        let new_prj = current_dir.join(args.name());
        // 支持目录已存在：只补齐缺失的骨架文件，不覆盖已有文件
        ensure_path(&new_prj).source_resource()?;

        // 部署类型：显式 --kind 优先，否则交互式选择
        let kind = match args.kind() {
            Some(k) => parse_kind(k.as_str()),
            None => Self::ia_kind()?,
        };
        // docker-compose 无目标型号；gxl 才交互选择型号
        let spec = match kind {
            SysKind::DockerCompose => {
                SysOperator::make_new_docker(&new_prj, args.name()).err_conv()?
            }
            SysKind::Gxl => {
                let model = Self::ia_model_std()?;
                SysOperator::make_new(&new_prj, args.name(), model).err_conv()?
            }
        };
        spec.with_kind(kind).save().err_conv()?;
        Ok(())
    }

    pub async fn handle_update(args: SysUpdateArgs) -> MainResult<()> {
        let current_dir = std::env::current_dir().expect("无法获取当前目录");
        galaxy_ops::infra::configure_dfx_logging(&args);

        let options = DownloadOptions::from((args.force, ValueDict::default()));
        let operator = SysOperator::load(&current_dir).err_conv()?;
        let accessor = galaxy_ops::accessor::accessor_for_default();

        operator
            .update_local(accessor, &current_dir, &options)
            .await
            .err_conv()?;
        operator.init_setting_value_in(resolve_sys_value_path(&current_dir))?;
        Ok(())
    }

    pub async fn handle_package(args: SysPackageArgs) -> MainResult<()> {
        let current_dir = std::env::current_dir().expect("无法获取当前目录");
        galaxy_ops::infra::configure_dfx_logging(&args);

        // 1. 先解析变量（生成 sys/merged_vars.yml），保证交付包可被 prj import 完整导入
        let options = DownloadOptions::from((args.force, ValueDict::default()));
        let operator = SysOperator::load(&current_dir).err_conv()?;
        let accessor = galaxy_ops::accessor::accessor_for_default();
        operator
            .update_local(accessor, &current_dir, &options)
            .await
            .err_conv()?;

        // 2. 确定输出路径与版本
        let name = operator.sys_spec().define().name().clone();
        let version = read_version(&current_dir);
        let out_path = match &args.output {
            Some(p) => PathBuf::from(p),
            None => current_dir
                .parent()
                .map(|p| p.join(format!("{name}-{version}.tar.gz")))
                .unwrap_or_else(|| PathBuf::from(format!("{name}-{version}.tar.gz"))),
        };

        // 3. 打包
        compress(&current_dir, &out_path).source_sys()?;
        println!("系统已打包: {}", out_path.display());
        Ok(())
    }

    pub async fn handle_localize(args: SysLocalizeArgs) -> MainResult<()> {
        let current_dir = std::env::current_dir().expect("无法获取当前目录");
        galaxy_ops::infra::configure_dfx_logging(&args);

        let spec = SysOperator::load(&current_dir).err_conv()?;
        let val_path = resolve_sys_value_path(&current_dir);

        // 默认：值文件缺失时先 update（解析变量 + 初始化值），一条 localize 即可。
        // --only 跳过 update，直接用现有值（值缺失会报错）。
        if !args.only && !val_path.sys_value_file().exists() {
            let options = DownloadOptions::from((false, ValueDict::default()));
            let accessor = galaxy_ops::accessor::accessor_for_default();
            spec.update_local(accessor, &current_dir, &options)
                .await
                .err_conv()?;
            spec.init_setting_value_in(val_path.clone())?;
        }

        let mut dict =
            OriginDict::from(ValueDict::load_yaml(&val_path.sys_value_file()).source_resource()?);
        dict.set_source("sys-setting");

        // 合并客户覆盖值 values/value.yml（若存在），覆盖系统默认值
        let user_value_file = val_path.root().join(USER_VALUE_FILE);
        if user_value_file.exists() {
            let mut user_dict =
                OriginDict::from(ValueDict::load_yaml(&user_value_file).source_resource()?);
            user_dict.set_source("customer");
            dict.merge(&user_dict);
        }

        spec.localize(
            val_path,
            LocalizeOptions::new(dict).with_only_mod(args.module),
        )
        .await
        .err_conv()?;
        Ok(())
    }

    pub async fn handle_setting(args: SysSettingArgs) -> MainResult<()> {
        let current_dir = std::env::current_dir().expect("无法获取当前目录");
        if args.init {
            let setting = SysSetting::example();
            let setting_path =
                ensure_path(current_dir.join("sys").join(SETTING_DIR)).source_resource()?;
            setting.save_local(&setting_path)?;
        }
        Ok(())
    }

    fn check_gflow_version() -> MainResult<()> {
        // 检查 gflow 版本
        let gflow_path = format!(
            "{}/bin/gflow",
            std::env::var("HOME").unwrap_or_else(|_| "".to_string())
        );
        let output = Command::new(&gflow_path)
            .arg("-V")
            .output()
            .map_err(|e| format!("无法执行 gflow 命令: {}", e))
            .source_resource()?;

        if !output.status.success() {
            return Err("gflow 命令执行失败").source_resource()?;
        }

        let version_str = String::from_utf8_lossy(&output.stdout);
        // 解析版本号，假设输出格式为 "gflow x.y.z"
        let version_parts: Vec<&str> = version_str.split_whitespace().collect();
        if version_parts.len() < 2 {
            return Err(format!("无法解析 gflow 版本: {}", version_str)).source_resource()?;
        }

        let version = version_parts[1];
        let version_parts: Vec<&str> = version.split('.').collect();
        if version_parts.len() < 3 {
            return Err(format!("无效的 gflow 版本格式: {}", version)).source_resource()?;
        }

        // 解析主版本、次版本和修订版本
        let major: u32 = version_parts[0]
            .parse()
            .map_err(|_| format!("无效的主版本号: {}", version_parts[0]))
            .source_resource()?;
        let minor: u32 = version_parts[1]
            .parse()
            .map_err(|_| format!("无效的次版本号: {}", version_parts[1]))
            .source_resource()?;
        let patch: u32 = version_parts[2]
            .parse()
            .map_err(|_| format!("无效的修订版本号: {}", version_parts[2]))
            .source_resource()?;

        // 检查版本是否 >= 0.11.2
        if major > 0 || (major == 0 && minor > 11) || (major == 0 && minor == 11 && patch >= 2) {
            Ok(())
        } else {
            Err(format!(
                "gflow 版本过低，需要 >= 0.11.2，当前版本: {}",
                version
            ))
            .source_resource()?
        }
    }

    pub async fn handle_ops_cmd(cmd_name: &str, args: SysOpsArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);

        let current_dir = std::env::current_dir().expect("无法获取当前目录");
        match SysOperator::load_kind(&current_dir) {
            SysKind::DockerCompose => Self::run_compose_cmd(cmd_name, &args).await,
            SysKind::Gxl => Self::run_gflow_cmd(cmd_name, &args).await,
        }
    }

    async fn run_gflow_cmd(cmd_name: &str, args: &SysOpsArgs) -> MainResult<()> {
        // 检查 gflow 版本
        Self::check_gflow_version()?;

        // 构建并执行命令
        let gflow_path = format!(
            "{}/bin/gflow",
            std::env::var("HOME").unwrap_or_else(|_| "".to_string())
        );

        // 直接执行 gflow 命令
        let mut cmd = TokioCommand::new(&gflow_path);
        cmd.arg("-e").arg(args.env());
        cmd.arg(cmd_name);
        cmd.arg("-d").arg(args.debug_level().to_string());

        if let Some(module) = args.module() {
            println!("use module :{module}");
            cmd.arg("--").arg(module);
        }

        Self::run_and_stream(cmd, "gflow").await
    }

    async fn run_compose_cmd(cmd_name: &str, args: &SysOpsArgs) -> MainResult<()> {
        let current_dir = std::env::current_dir().expect("无法获取当前目录");

        // 将 gops sys 语义映射到 docker compose 子命令：
        //   download -> pull, install -> create, start -> up -d, stop -> stop,
        //   uninstall -> down, status -> ps, diagnose -> config
        let (subcommand, extra) = compose_subcommand(cmd_name);

        if args.module().is_some() {
            println!("note: docker-compose 类型系统忽略 --mod 参数");
        }

        let mut cmd = TokioCommand::new("docker");
        cmd.arg("compose").arg(subcommand).args(extra);
        cmd.current_dir(&current_dir);

        // 运行时注入密钥：从 ~/.galaxy/sec_value.yml（或 ./.galaxy/sec_value.yml）读取 SEC_* 变量，
        // 只进入 docker compose 子进程环境，不落盘、不进 .env。
        // diagnose（docker compose config）只读校验，注入掩码值以免泄露明文。
        let sec_dict = orion_sec::load_sec_dict().source_resource()?;
        for (key, value) in sec_env_pairs_for(cmd_name, &sec_dict) {
            cmd.env(key, value);
        }

        Self::run_and_stream(cmd, "docker compose").await
    }

    /// 启动子进程并转发 stdout/stderr，非零退出返回错误。
    async fn run_and_stream(mut cmd: TokioCommand, label: &str) -> MainResult<()> {
        // 设置管道并启动进程
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("无法启动 {label} 命令: {}", e))
            .source_resource()?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("无法获取stdout"))
            .source_resource()?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("无法获取stderr"))
            .source_resource()?;

        // 创建异步读取器并并发处理stdout和stderr
        let stdout_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                println!("{}", line);
            }
        });

        let stderr_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                eprintln!("{}", line);
            }
        });

        // 等待输出处理完成
        let _ = tokio::try_join!(stdout_handle, stderr_handle);

        // 等待子进程完成并检查退出状态
        let exit_status = child
            .wait()
            .await
            .map_err(|e| anyhow::anyhow!("等待子进程失败: {}", e))
            .source_resource()?;

        if !exit_status.success() {
            return Err(anyhow::anyhow!("命令执行失败，退出状态: {}", exit_status))
                .source_resource()?;
        }

        Ok(())
    }

    pub async fn execute(cmd: SysCmd) -> MainResult<()> {
        match cmd {
            SysCmd::New(args) => Self::handle_new(args).await,
            SysCmd::Update(args) => Self::handle_update(args).await,
            SysCmd::Package(args) => Self::handle_package(args).await,
            SysCmd::Localize(args) => Self::handle_localize(args).await,
            SysCmd::Setting(args) => Self::handle_setting(args).await,
            SysCmd::Download(sys_ops_args) => Self::handle_ops_cmd("download", sys_ops_args).await,
            SysCmd::Install(sys_ops_args) => Self::handle_ops_cmd("install", sys_ops_args).await,
            SysCmd::Start(sys_ops_args) => Self::handle_ops_cmd("start", sys_ops_args).await,
            SysCmd::Stop(sys_ops_args) => Self::handle_ops_cmd("stop", sys_ops_args).await,
            SysCmd::Uninstall(sys_ops_args) => {
                Self::handle_ops_cmd("uninstall", sys_ops_args).await
            }
            SysCmd::Status(sys_ops_args) => Self::handle_ops_cmd("status", sys_ops_args).await,
            SysCmd::Diagnose(sys_ops_args) => Self::handle_ops_cmd("diagnose", sys_ops_args).await,
        }
    }
}

fn read_version(root: &Path) -> String {
    let version_file = root.join("version.txt");
    let version = std::fs::read_to_string(&version_file)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "0.1.0".to_string());
    if version.is_empty() {
        "0.1.0".to_string()
    } else {
        version
    }
}

/// 把 `--kind` 的字符串解析为系统类型，未知值默认 Gxl。
fn parse_kind(s: &str) -> SysKind {
    match s {
        "docker-compose" | "compose" => SysKind::DockerCompose,
        _ => SysKind::Gxl,
    }
}

/// 把 gops sys 命令名映射为 docker compose 子命令（及附加参数）。
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

/// 密钥掩码值，与 `orion-sec` 的 `SECRET_MASK` 保持一致。
const SECRET_MASK: &str = "********";

/// 把 secret dict（SEC_* → 明文值）转成要注入子进程的 (KEY, VALUE) 环境变量对。
/// `diagnose`（docker compose config）是只读校验，注入掩码值以免泄露明文；其余命令注入明文。
fn sec_env_pairs_for(cmd_name: &str, dict: &ValueDict) -> Vec<(String, String)> {
    let mask = cmd_name == "diagnose";
    dict.iter()
        .map(|(k, v)| {
            let value = if mask {
                SECRET_MASK.to_string()
            } else {
                v.to_string()
            };
            (k.as_str().to_string(), value)
        })
        .collect()
}

// === 测试 ===

#[cfg(test)]
mod tests {
    use super::*;
    use galaxy_ops::infra::{WorkDirWithLock, once_init_log};
    use orion_vars::vars::ValueType;
    use tempfile::tempdir;
    #[tokio::test]
    async fn test_sys_new_command() {
        once_init_log();
        let temp_dir = tempdir().unwrap();
        let _wd = WorkDirWithLock::change(temp_dir.path());

        unsafe {
            std::env::set_var("TEST_MODE", "true");
        }

        let args = SysNewArgs {
            name: "test_system".to_string(),
            kind: Some("gxl".to_string()),
        };

        let result = SysCommandHandler::handle_new(args).await;
        unsafe {
            std::env::remove_var("TEST_MODE");
        }

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_ia_model_std() {
        once_init_log();
        unsafe {
            std::env::set_var("TEST_MODE", "true");
        }

        let result = SysCommandHandler::ia_model_std();
        unsafe {
            std::env::remove_var("TEST_MODE");
        }

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_sys_commands() {
        once_init_log();
        let temp_dir = tempdir().unwrap();
        let _wd = WorkDirWithLock::change(temp_dir.path());
        unsafe {
            std::env::set_var("TEST_MODE", "true");
        }

        // 测试 new 命令
        let new_cmd = SysCmd::New(SysNewArgs {
            name: "test_system".to_string(),
            kind: Some("gxl".to_string()),
        });
        let result = SysCommandHandler::execute(new_cmd).await;
        assert!(result.is_ok());

        unsafe {
            std::env::remove_var("TEST_MODE");
        }
    }

    #[test]
    fn test_sys_new_args_getter() {
        once_init_log();
        let args = SysNewArgs {
            name: "test_system".to_string(),
            kind: Some("docker-compose".to_string()),
        };

        assert_eq!(args.debug_level(), 0);
        assert_eq!(args.log_setting(), None);
        assert_eq!(args.name(), "test_system");
        assert_eq!(args.kind(), &Some("docker-compose".to_string()));
    }

    #[test]
    fn test_sys_update_args_getter() {
        once_init_log();
        let args = SysUpdateArgs {
            debug_log: DebugLogArgs {
                debug: 2,
                log: Some("info".to_string()),
            },
            force: false,
        };

        assert_eq!(args.debug_level(), 2);
        assert_eq!(args.log_setting(), Some("info".to_string()));
        assert!(!args.force);
    }

    #[test]
    fn test_sys_package_args_getter() {
        once_init_log();
        let args = SysPackageArgs {
            debug_log: DebugLogArgs {
                debug: 1,
                log: None,
            },
            force: false,
            output: None,
        };

        assert_eq!(args.debug_level(), 1);
        assert_eq!(args.log_setting(), None);
        assert!(!args.force);
        assert!(args.output.is_none());
    }

    #[test]
    fn test_sys_localize_args_getter() {
        once_init_log();
        let args = SysLocalizeArgs {
            debug_log: DebugLogArgs {
                debug: 1,
                log: Some("debug".to_string()),
            },
            module: None,
            only: false,
        };

        assert_eq!(args.debug_level(), 1);
        assert_eq!(args.log_setting(), Some("debug".to_string()));
        assert!(!args.only);
    }

    #[test]
    fn test_parse_kind() {
        assert_eq!(parse_kind("gxl"), SysKind::Gxl);
        assert_eq!(parse_kind("docker-compose"), SysKind::DockerCompose);
        assert_eq!(parse_kind("compose"), SysKind::DockerCompose);
        assert_eq!(parse_kind("unknown"), SysKind::Gxl);
        assert_eq!(parse_kind(""), SysKind::Gxl);
    }

    #[test]
    fn test_ia_kind_test_mode_defaults_to_gxl() {
        unsafe {
            std::env::set_var("TEST_MODE", "true");
        }
        let kind = SysCommandHandler::ia_kind().unwrap();
        unsafe {
            std::env::remove_var("TEST_MODE");
        }
        assert_eq!(kind, SysKind::Gxl);
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
        check("diagnose", "config", &[]);
        // 未知命令透传
        check("whatever", "whatever", &[]);
    }

    #[tokio::test]
    async fn test_sys_new_writes_kind() {
        once_init_log();
        let temp_dir = tempdir().unwrap();
        let _wd = WorkDirWithLock::change(temp_dir.path());
        unsafe {
            std::env::set_var("TEST_MODE", "true");
        }

        let args = SysNewArgs {
            name: "compose_demo".to_string(),
            kind: Some("docker-compose".to_string()),
        };
        SysCommandHandler::handle_new(args).await.unwrap();

        unsafe {
            std::env::remove_var("TEST_MODE");
        }

        let prj = temp_dir.path().join("compose_demo");
        assert_eq!(SysOperator::load_kind(&prj), SysKind::DockerCompose);
        let define = std::fs::read_to_string(prj.join("sys/sys_model.yml")).unwrap();
        assert!(define.contains("kind: docker-compose"));
    }

    #[tokio::test]
    async fn test_sys_new_existing_dir_preserves_files() {
        once_init_log();
        let temp_dir = tempdir().unwrap();
        let _wd = WorkDirWithLock::change(temp_dir.path());
        unsafe {
            std::env::set_var("TEST_MODE", "true");
        }

        // 预先创建同名目录，并放一个用户已有的 docker-compose.yml，模拟“已存在”的场景
        let prj = temp_dir.path().join("gateway");
        std::fs::create_dir_all(&prj).unwrap();
        let existing_compose = "services:\n  app:\n    image: nginx\n";
        std::fs::write(prj.join("docker-compose.yml"), existing_compose).unwrap();

        let args = SysNewArgs {
            name: "gateway".to_string(),
            kind: Some("docker-compose".to_string()),
        };
        SysCommandHandler::handle_new(args).await.unwrap();

        unsafe {
            std::env::remove_var("TEST_MODE");
        }

        // 不覆盖用户已有的 docker-compose.yml
        let compose = std::fs::read_to_string(prj.join("docker-compose.yml")).unwrap();
        assert_eq!(compose, existing_compose);
        // 补齐缺失的骨架文件
        assert_eq!(SysOperator::load_kind(&prj), SysKind::DockerCompose);
        assert!(prj.join("sys-prj.yml").exists());
        assert!(prj.join("sys/sys_model.yml").exists());
        assert!(prj.join("sys/setting/vars.yml").exists());
    }

    #[test]
    fn test_sec_env_pairs_for_diagnose_masks() {
        let mut dict = ValueDict::new();
        dict.insert("SEC_DB_PASSWORD", ValueType::from("pw"));
        dict.insert("SEC_API_KEY", ValueType::from("tok-123"));
        // diagnose 注入掩码值，不泄露明文
        let pairs = sec_env_pairs_for("diagnose", &dict);
        assert!(pairs.contains(&("SEC_DB_PASSWORD".to_string(), "********".to_string())));
        assert!(pairs.contains(&("SEC_API_KEY".to_string(), "********".to_string())));
    }

    #[test]
    fn test_sec_env_pairs_for_start_plaintext() {
        let mut dict = ValueDict::new();
        dict.insert("SEC_DB_PASSWORD", ValueType::from("pw"));
        dict.insert("SEC_API_KEY", ValueType::from("tok-123"));
        // 非 diagnose 注入明文
        let pairs = sec_env_pairs_for("start", &dict);
        assert!(pairs.contains(&("SEC_DB_PASSWORD".to_string(), "pw".to_string())));
        assert!(pairs.contains(&("SEC_API_KEY".to_string(), "tok-123".to_string())));
    }

    #[test]
    fn test_load_sec_dict_normalizes_keys() {
        let temp_dir = tempdir().unwrap();
        let _wd = WorkDirWithLock::change(temp_dir.path()).unwrap();
        let dot_dir = temp_dir.path().join(".galaxy");
        std::fs::create_dir_all(&dot_dir).unwrap();
        std::fs::write(
            dot_dir.join("sec_value.yml"),
            "db_password: secretpw\npostgres_password: secretpgpw\n",
        )
        .unwrap();

        // orion-sec 归一化为大写并加 SEC_ 前缀
        let dict = orion_sec::load_sec_dict().unwrap();
        let pairs = sec_env_pairs_for("start", &dict);
        assert!(pairs.contains(&("SEC_DB_PASSWORD".to_string(), "secretpw".to_string())));
        assert!(pairs.contains(&(
            "SEC_POSTGRES_PASSWORD".to_string(),
            "secretpgpw".to_string()
        )));
    }

    #[tokio::test]
    async fn test_sys_localize_auto_updates_when_values_missing() {
        once_init_log();
        let temp_dir = tempdir().unwrap();

        // 先在 temp_dir 下创建一个 docker-compose 系统
        {
            let _wd = WorkDirWithLock::change(temp_dir.path()).unwrap();
            let new_args = SysNewArgs {
                name: "compose_demo".to_string(),
                kind: Some("docker-compose".to_string()),
            };
            SysCommandHandler::handle_new(new_args).await.unwrap();
        }

        // 进入系统目录，默认 localize：应自动 update 并生成 .env
        {
            let _wd = WorkDirWithLock::change(temp_dir.path().join("compose_demo")).unwrap();
            let localize_args = SysLocalizeArgs {
                debug_log: DebugLogArgs {
                    debug: 0,
                    log: None,
                },
                module: None,
                only: false,
            };
            SysCommandHandler::handle_localize(localize_args)
                .await
                .unwrap();
        }

        assert!(temp_dir.path().join("compose_demo/.env").exists());
        assert!(
            temp_dir
                .path()
                .join("compose_demo/values/sys_value.yml")
                .exists()
        );
    }

    #[tokio::test]
    async fn test_sys_localize_prefers_ops_project_values() {
        once_init_log();
        let temp_dir = tempdir().unwrap();
        let prj = temp_dir.path().join("proj");
        std::fs::create_dir_all(&prj).unwrap();

        // 1. 在项目下创建 docker-compose 系统
        {
            let _wd = WorkDirWithLock::change(&prj).unwrap();
            SysCommandHandler::handle_new(SysNewArgs {
                name: "my-sys".to_string(),
                kind: Some("docker-compose".to_string()),
            })
            .await
            .unwrap();
        }

        // 2. 项目声明该系统并为它维护客户值（系统目录内的 values 并非符号链接）
        std::fs::write(
            prj.join("ops-prj.yml"),
            "name: proj\nwork_envs:\n  dep_root: ''\n  deps: []\nsys_models:\n- sys:\n    name: my-sys\n    kind: docker-compose\n    vender: ''\n  addr:\n    url: http://example.com/my-sys.tar.gz\n",
        )
        .unwrap();
        let prj_values = prj.join("values/my-sys");
        std::fs::create_dir_all(&prj_values).unwrap();
        std::fs::write(prj_values.join("sys_value.yml"), "HTTP_PORT: 9090\n").unwrap();

        // 3. 在系统目录内 localize：应使用项目值（客户值）
        {
            let _wd = WorkDirWithLock::change(prj.join("my-sys")).unwrap();
            SysCommandHandler::handle_localize(SysLocalizeArgs {
                debug_log: DebugLogArgs {
                    debug: 0,
                    log: None,
                },
                module: None,
                only: false,
            })
            .await
            .unwrap();
        }

        let env = std::fs::read_to_string(prj.join("my-sys/.env")).unwrap();
        assert!(env.contains("HTTP_PORT=9090"), "unexpected .env: {env}");
        // 项目值已存在，不应在系统目录另生成一份派生值文件
        assert!(!prj.join("my-sys/values/sys_value.yml").exists());
    }
}
