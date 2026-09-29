use std::ffi::OsString;
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
use galaxy_ops::project::load_value_file;
use galaxy_ops::system::drift::{DriftStatus, detect_drift};
use galaxy_ops::system::lock::DeliverLock;
use galaxy_ops::system::operator::SysOperator;
use galaxy_ops::system::pack::pack_system;
use galaxy_ops::system::setting::SysSetting;
use galaxy_ops::system::{SysKind, SysOperatorPath, SysValuePaths};
use galaxy_ops::types::{LocalizeOptions, RefUpdateable};
use orion_infra::path::ensure_path;
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

    /// 不按 git 入库文件打包，而是打包当前目录全部（默认只打 git 跟踪的入库文件）
    #[arg(long = "no-git", default_value_t = false)]
    pub no_git: bool,
}

#[derive(Debug, Args, Getters)]
pub struct SysLocalizeArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,

    #[arg(long = "mod", help = "mod name")]
    pub module: Option<String>,

    #[arg(long, help = "只 localize，跳过 update（不解析/下载模块）")]
    pub only: bool,

    #[arg(
        long = "no-flow",
        help = "跳过可选的阶段 gx 流程（如 localize）；默认在 docker-compose 系统里按需执行"
    )]
    pub no_flow: bool,
}

#[derive(Debug, Args, Getters)]
pub struct SysSettingArgs {
    #[arg(long, help = "init sys setting")]
    pub init: bool,
}

#[derive(Debug, Args, Getters)]
pub struct SysCheckArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,
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

    /// 检查系统配置漂移 (Check Configuration Drift)
    #[command(
        about = "检查系统配置漂移 (Check Configuration Drift)",
        long_about = "只读比对：重新计算本地化会产出的值与已生成的 .env，报告“值已变更但未重新 localize”。\
                     不做完整 reconcile；存在漂移时返回非零退出码，可用于 CI 卡口。\n\
                     Read-only: report values changed but not re-localized (drift).\n\
                     Usage: gops sys check"
    )]
    Check(SysCheckArgs),
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
impl DfxArgsGetter for SysCheckArgs {
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
        let val_path = resolve_sys_value_path(&current_dir);

        operator
            .update_local(accessor, &current_dir, &options)
            .await
            .err_conv()?;
        // 初始化辅助值文件；同时生成 values/sys_value.yml 注释模板（可用变量已注释）
        operator.init_setting_value_in(val_path)?;
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

        // 生成交付锁：记录系统版本、模块引用与值指纹（随交付包分发，可复现、可回滚）
        let lock = DeliverLock::build(&current_dir).err_conv()?;
        lock.save(&current_dir).err_conv()?;
        println!(
            "交付锁已生成: {}",
            DeliverLock::path(&current_dir).display()
        );

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

        // 3. 打包：默认只含 git 入库文件；`--no-git` 打整目录
        pack_system(&current_dir, &out_path, !args.no_git)?;
        println!("系统已打包: {}", out_path.display());
        Ok(())
    }

    pub async fn handle_localize(args: SysLocalizeArgs) -> MainResult<()> {
        let current_dir = std::env::current_dir().expect("无法获取当前目录");
        galaxy_ops::infra::configure_dfx_logging(&args);

        let spec = SysOperator::load(&current_dir).err_conv()?;
        let val_path = resolve_sys_value_path(&current_dir);

        // 默认：系统变量未解析时先 update（解析变量）。--only 跳过 update，直接用现有数据。
        if !args.only && !spec.has_resolved_vars() {
            let options = DownloadOptions::from((false, ValueDict::default()));
            let accessor = galaxy_ops::accessor::accessor_for_default();
            spec.update_local(accessor, &current_dir, &options)
                .await
                .err_conv()?;
        }
        // 确保本地化值文件存在（生成 sys_value.yml 注释模板）；变量未解析时会报错
        spec.init_setting_value_in(val_path.clone())?;

        // 基线：系统解析出的默认值（sys/merged_vars.yml 的 system 段），
        // 使值文件只需写“需要修改的项”，其余取系统默认值。
        let mut dict = OriginDict::from(spec.system_default_values().err_conv()?);
        dict.set_source("sys-defaults");

        // 叠加值文件（可选，可为部分覆盖；全注释模板等价于空覆盖）
        let value_file = val_path.sys_value_file();
        if value_file.exists() {
            let mut sys_dict = OriginDict::from(load_value_file(&value_file)?);
            sys_dict.set_source("sys-setting");
            dict.merge(&sys_dict);
        }

        // 叠加客户覆盖值 values/value.yml（若存在），优先于系统默认与值文件
        let user_value_file = val_path.root().join(USER_VALUE_FILE);
        if user_value_file.exists() {
            let mut user_dict = OriginDict::from(load_value_file(&user_value_file)?);
            user_dict.set_source("customer");
            dict.merge(&user_dict);
        }

        let no_flow = args.no_flow;
        let debug = args.debug_level();

        // 与写 `.env` **完全一致**的值（同一份 evaled 字典，含 `${VAR}` 展开）：先算好，
        // 供随后的阶段流程注入子进程。
        let options = LocalizeOptions::new(dict).with_only_mod(args.module);
        let stage_env = galaxy_ops::project::env_pairs(options.evaled_value());

        spec.localize(val_path, options).await.err_conv()?;

        // 可选阶段扩展点：内置 localize（写 .env）完成后，若项目定义了同名 gx 流程则执行。
        // 当前只对 docker-compose 生效；机制与阶段名无关，见 `run_stage_flow`。
        if !no_flow {
            Self::run_stage_flow(
                spec.kind(),
                &Self::gx_bin_path(),
                "localize",
                debug,
                &stage_env,
            )
            .await?;
        }
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

    /// 外部执行器 `gx`（galaxy-flow）的最低版本要求。
    const GX_MIN_VERSION: (u32, u32, u32) = (0, 13, 0);

    /// `gx` 可执行文件路径（`$HOME/bin/gx`）。
    fn gx_bin_path() -> String {
        format!(
            "{}/bin/gx",
            std::env::var("HOME").unwrap_or_else(|_| "".to_string())
        )
    }

    fn check_gx_version() -> MainResult<()> {
        // 检查 gx 版本
        let gx_path = Self::gx_bin_path();
        let output = Command::new(&gx_path)
            .arg("-V")
            .output()
            .map_err(|e| format!("无法执行 gx 命令 ({gx_path}): {e}"))
            .source_resource()?;

        if !output.status.success() {
            return Err("gx 命令执行失败").source_resource()?;
        }

        let version_str = String::from_utf8_lossy(&output.stdout);
        // 解析版本号，假设输出格式为 "gx x.y.z"
        let version_parts: Vec<&str> = version_str.split_whitespace().collect();
        if version_parts.len() < 2 {
            return Err(format!("无法解析 gx 版本: {version_str}")).source_resource()?;
        }

        let version = version_parts[1];
        let version_parts: Vec<&str> = version.split('.').collect();
        if version_parts.len() < 3 {
            return Err(format!("无效的 gx 版本格式: {version}")).source_resource()?;
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

        // 检查版本是否 >= GX_MIN_VERSION
        let (min_major, min_minor, min_patch) = Self::GX_MIN_VERSION;
        if (major, minor, patch) >= (min_major, min_minor, min_patch) {
            Ok(())
        } else {
            Err(format!(
                "gx 版本过低，需要 >= {min_major}.{min_minor}.{min_patch}，当前版本: {version}"
            ))
            .source_resource()?
        }
    }

    pub async fn handle_ops_cmd(cmd_name: &str, args: SysOpsArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);

        let current_dir = std::env::current_dir().expect("无法获取当前目录");
        match SysOperator::load_kind(&current_dir) {
            SysKind::DockerCompose => Self::run_compose_cmd(cmd_name, &args).await,
            SysKind::Gxl => Self::run_gx_cmd(cmd_name, &args).await,
        }
    }

    /// 构造 `gx run` 的参数（不含可执行文件路径）。
    ///
    /// 映射：`gops sys <cmd> [-e ENV] [-d N] [--mod M]` -> `gx run -e ENV -d N [--cmd-arg M] <cmd>`
    fn gx_run_args(cmd_name: &str, env: &str, debug: usize, module: Option<&str>) -> Vec<String> {
        let mut args = vec![
            "run".to_string(),
            "-e".to_string(),
            env.to_string(),
            "-d".to_string(),
            debug.to_string(),
        ];
        if let Some(module) = module {
            args.push("--cmd-arg".to_string());
            args.push(module.to_string());
        }
        args.push(cmd_name.to_string());
        args
    }

    /// 统一的 gx 流程执行入口：`gx run -e <env> -d <n> [--cmd-arg <mod>] <flow>`，
    /// 并可按需把 `inject_env` 注入子进程环境。
    ///
    /// `gxl` 的分派（`gops sys start` 等）与 `docker-compose` 的可选阶段流程都走这里，
    /// 保证「如何调 gx」只有一处。（GXL 内部的 `gx.run` 是另一种东西，gops 不直接使用。）
    async fn run_gx_flow(
        gx_path: &str,
        env: &str,
        debug: usize,
        module: Option<&str>,
        flow: &str,
        inject_env: &[(String, String)],
    ) -> MainResult<()> {
        let mut cmd = TokioCommand::new(gx_path);
        cmd.args(Self::gx_run_args(flow, env, debug, module));
        for (key, value) in inject_env {
            // 保留变量不被合并配置覆盖，否则可能破坏 gx 自身或其 shell（如 PATH/HOME）。
            if is_reserved_env(key) {
                continue;
            }
            cmd.env(key, value);
        }
        Self::run_and_stream(cmd, "gx").await
    }

    /// 阶段扩展点：内置动作完成后，若项目定义了**同名 gx 流程**则执行。
    ///
    /// 机制与阶段名无关（`flow_name` 由调用方给出，如 `localize`）；当前**只对
    /// `kind: docker-compose` 开放**——策略集中在此处，后续要对 `install`/`start`/…
    /// 或 gxl 放开，只改这里的判断与调用点即可。
    ///
    /// 判定依赖 galaxy-flow `>= 0.14` 的 `gx run --exists`（**确定性**，不靠试跑猜
    /// 退出码）：存在则 `gx run <flow>`；不存在/不可判定则**跳过**（零行为变化、gx 可选）。
    /// `env_pairs` 里是合并后的配置，作为进程环境变量注入子进程，使流程能读到刚合并的值。
    async fn run_stage_flow(
        kind: SysKind,
        gx_path: &str,
        flow_name: &str,
        debug: usize,
        env_pairs: &[(String, String)],
    ) -> MainResult<()> {
        if !matches!(kind, SysKind::DockerCompose) {
            return Ok(());
        }
        match Self::gx_flow_probe(gx_path, flow_name) {
            StageProbe::Exists => {}
            StageProbe::Skipped(reason) => {
                // 默认静默（保持「compose 无需 gx」）；`-d 1` 给出跳过原因，避免
                // “写了流程却未跑”时完全不可发现（conf 解析错 / gx 过旧 等）。
                if debug >= 1 {
                    eprintln!("skip stage flow '{flow_name}': {reason}");
                }
                return Ok(());
            }
        }
        println!("run stage flow: gx run {flow_name}");
        // 与 gxl 分派共用同一入口（`-e default`，因为 compose 系统没有可选的 env）。
        Self::run_gx_flow(gx_path, "default", debug, None, flow_name, env_pairs).await
    }

    /// `gx run --exists <flow>`：退出 `0` = 存在；其余（不存在 / conf 不可加载 /
    /// gx 缺失或过旧）一律视为不可用，并保留原因供 `-d 1` 提示。
    ///
    /// 捕获并丢弃输出：探测不应污染 gops 输出（例如 compose 项目没有 `_gal/work.gxl`
    /// 时 gx 会往 stderr 打 `conf not exists`）。
    fn gx_flow_probe(gx_path: &str, flow_name: &str) -> StageProbe {
        match Command::new(gx_path)
            .args(["run", "--exists", flow_name])
            .output()
        {
            Ok(out) if out.status.success() => StageProbe::Exists,
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                StageProbe::Skipped(if stderr.is_empty() {
                    format!("gx --exists 退出码 {:?}", out.status.code())
                } else {
                    stderr
                })
            }
            Err(e) => StageProbe::Skipped(format!("无法执行 gx: {e}")),
        }
    }

    /// `kind: gxl` 的系统：把 `gops sys <cmd>` 映射为 `gx run <cmd>`。
    ///
    /// 系统的 `start` / `stop` / `status` / `diagnose` 等即工作流里的同名流程
    /// （定义在 `_gal/work.gxl` 或其引用的模块中）。
    async fn run_gx_cmd(cmd_name: &str, args: &SysOpsArgs) -> MainResult<()> {
        Self::check_gx_version()?;

        let gx_path = Self::gx_bin_path();
        let module = args.module();
        if let Some(module) = module {
            println!("use module :{module}");
        }

        // 与 compose 的可选阶段流程共用同一入口 `run_gx_flow`。
        Self::run_gx_flow(
            &gx_path,
            args.env(),
            args.debug_level(),
            module.as_deref(),
            cmd_name,
            &[],
        )
        .await
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
        // compose 文件位置可变（默认 `sys/docker-compose.yaml`），但项目目录始终锚定到系统根，
        // 使相对挂载与 .env 的基准保持不变。
        let sys_paths = SysOperatorPath::new(&current_dir);
        let candidates = sys_paths.compose_candidates();
        let compose_file = candidates.first().cloned();
        if let Some(file) = &compose_file {
            if candidates.len() > 1 {
                eprintln!(
                    "warn: 检测到 {} 个 compose 文件，使用 {}（其余忽略）",
                    candidates.len(),
                    file.display()
                );
            }
        } else {
            eprintln!(
                "warn: 未找到 compose 文件（已按 sys/ 与系统根的 {{compose,docker-compose}}.{{yaml,yml}} 查找），交由 docker compose 自行发现"
            );
        }
        cmd.arg("compose")
            .args(compose_global_args(compose_file.as_deref(), &current_dir))
            .arg(subcommand)
            .args(extra);
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

    pub async fn handle_check(args: SysCheckArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);
        let current_dir = std::env::current_dir().source_resource()?;

        let op = SysOperator::load(&current_dir).err_conv()?;
        let report = detect_drift(&op, &current_dir).err_conv()?;

        match report.status() {
            DriftStatus::NoBaseline => {
                println!("[INFO] 尚无 .env 基线（未 localize）；跳过漂移检查");
                Ok(())
            }
            DriftStatus::Clean => {
                println!("[OK] 值与 {} 一致，无漂移", report.env_path().display());
                Ok(())
            }
            DriftStatus::Drifted => {
                println!(
                    "[DRIFT] 检测到 {} 处值变更，但 {} 未更新：",
                    report.changes().len(),
                    report.env_path().display()
                );
                for change in report.changes() {
                    println!("  {}", change.describe());
                }
                println!("提示：运行 `gops sys localize` 重新生成本地化配置。");
                Err(format!(
                    "sys check: 检测到 {} 处漂移",
                    report.changes().len()
                ))
                .source_resource()?
            }
        }
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
            SysCmd::Check(args) => Self::handle_check(args).await,
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

/// `gx run --exists` 的探测结果。
enum StageProbe {
    /// 存在（退出 0）。
    Exists,
    /// 不可用 / 不存在；带原因（供 `-d 1` 提示）。
    Skipped(String),
}

/// 注入 gx 子进程时**保留**的环境变量：不被合并配置覆盖，否则可能破坏 gx 自身
/// （`~/.galaxy` 解析、动态库加载、`gx.*` 内部变量）或其 shell 的 `PATH`。
fn is_reserved_env(key: &str) -> bool {
    const RESERVED: &[&str] = &[
        "PATH", "HOME", "PWD", "OLDPWD", "SHELL", "TMPDIR", "USER", "LOGNAME", "LANG", "LC_ALL",
    ];
    const RESERVED_PREFIXES: &[&str] = &["LD_", "DYLD_", "GX_", "GXL_"];
    RESERVED.contains(&key) || RESERVED_PREFIXES.iter().any(|p| key.starts_with(p))
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

/// 构造 `docker compose` 的全局参数（必须位于子命令之前）。
///
/// 分两种布局：
/// - **内收布局**（compose 在 `sys/` 下）：显式 `-f <file> --project-directory <系统根>`，
///   把项目目录锚回系统根，避免项目名与相对挂载基准漂到 `sys/`。显式 `-f` 会关闭 docker
///   对 override 的自动合并，因此这里同时显式合并同目录的 `*.override.{yaml,yml}`。
/// - **旧布局**（compose 就在系统根）：**不传任何全局参数**，退回 docker 自动发现。
///   这样能完整保留 docker 的 override 自动合并与 `COMPOSE_FILE` 环境变量语义，
///   保证旧仓不加改动照样跑。此时项目目录本就等于系统根，语义与内收布局一致。
///
/// 未找到 compose 文件时同样返回空参数，交由 docker 自行发现。
fn compose_global_args(compose_file: Option<&Path>, project_dir: &Path) -> Vec<OsString> {
    let Some(file) = compose_file else {
        return Vec::new();
    };
    // 旧布局：文件就在系统根，交给 docker 自动发现（保留 override 合并 / COMPOSE_FILE）。
    if file.parent() == Some(project_dir) {
        return Vec::new();
    }

    let mut args = vec![OsString::from("-f"), file.as_os_str().to_os_string()];
    for override_file in compose_override_files(file) {
        args.push(OsString::from("-f"));
        args.push(override_file.into_os_string());
    }
    args.push(OsString::from("--project-directory"));
    args.push(project_dir.as_os_str().to_os_string());
    args
}

/// 与 compose 主文件同目录、按 docker 约定命名的 override 文件（存在者）：
/// `<stem>.override.yaml` → `<stem>.override.yml`。
fn compose_override_files(base: &Path) -> Vec<PathBuf> {
    let (Some(stem), Some(dir)) = (base.file_stem().and_then(|s| s.to_str()), base.parent()) else {
        return Vec::new();
    };
    ["yaml", "yml"]
        .into_iter()
        .map(|ext| dir.join(format!("{stem}.override.{ext}")))
        .filter(|path| path.is_file())
        .collect()
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
            no_git: false,
        };

        assert_eq!(args.debug_level(), 1);
        assert_eq!(args.log_setting(), None);
        assert!(!args.force);
        assert!(args.output.is_none());
    }

    #[test]
    fn test_gx_run_args_basic() {
        // gops sys start  ->  gx run -e default -d 0 start
        let args = SysCommandHandler::gx_run_args("start", "default", 0, None);
        assert_eq!(args.join(" "), "run -e default -d 0 start");
    }

    #[test]
    fn test_gx_run_args_with_module_and_debug() {
        // gops sys stop --mod nginx -e prod -d 2
        //   ->  gx run -e prod -d 2 --cmd-arg nginx stop
        let args = SysCommandHandler::gx_run_args("stop", "prod", 2, Some("nginx"));
        assert_eq!(args.join(" "), "run -e prod -d 2 --cmd-arg nginx stop");
        // 流程名始终在末尾
        assert_eq!(args.last().map(String::as_str), Some("stop"));
    }

    #[test]
    fn test_gx_run_args_covers_all_dispatch_commands() {
        for cmd in [
            "download",
            "install",
            "uninstall",
            "start",
            "stop",
            "status",
            "diagnose",
        ] {
            let args = SysCommandHandler::gx_run_args(cmd, "default", 0, None);
            assert_eq!(args.first().map(String::as_str), Some("run"));
            assert_eq!(args.last().map(String::as_str), Some(cmd));
        }
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
            no_flow: false,
        };

        assert_eq!(args.debug_level(), 1);
        assert_eq!(args.log_setting(), Some("debug".to_string()));
        assert!(!args.only);
    }

    #[test]
    fn test_parse_no_flow_flag() {
        use clap::Parser;
        #[derive(Parser)]
        struct Cli {
            #[command(flatten)]
            args: SysLocalizeArgs,
        }
        let cli = Cli::try_parse_from(["x", "--no-flow"]).unwrap();
        assert!(cli.args.no_flow);
        let cli = Cli::try_parse_from(["x"]).unwrap();
        assert!(!cli.args.no_flow);
    }

    #[test]
    fn test_gx_flow_probe() {
        // `true` 忽略参数退出 0（视为存在）；`false` 退出 1；不存在路径 → 不可用
        assert!(matches!(
            SysCommandHandler::gx_flow_probe("true", "localize"),
            StageProbe::Exists
        ));
        assert!(matches!(
            SysCommandHandler::gx_flow_probe("false", "localize"),
            StageProbe::Skipped(_)
        ));
        assert!(matches!(
            SysCommandHandler::gx_flow_probe("/nonexistent/gx-xyz", "localize"),
            StageProbe::Skipped(_)
        ));
    }

    #[test]
    fn test_reserved_env_filter() {
        // 关键系统变量/内部前缀不得被合并配置覆盖
        for k in [
            "PATH",
            "HOME",
            "PWD",
            "LD_PRELOAD",
            "DYLD_LIBRARY_PATH",
            "GX_FOO",
            "GXL_PRJ_ROOT",
        ] {
            assert!(is_reserved_env(k), "{k} should be reserved");
        }
        for k in ["DOMAIN", "HTTP_PORT", "NGINX_TAG"] {
            assert!(!is_reserved_env(k), "{k} should not be reserved");
        }
    }

    #[tokio::test]
    async fn test_run_stage_flow_gates_on_kind() {
        // 非 docker-compose 是 no-op：即使 gx 路径不存在也不报错
        SysCommandHandler::run_stage_flow(SysKind::Gxl, "/nonexistent/gx", "localize", 0, &[])
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_stage_flow_skips_when_flow_absent() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("ran.txt");
        let pairs = vec![("OUT".to_string(), out.display().to_string())];
        // 探测失败（`false`）→ 跳过，不执行流程
        SysCommandHandler::run_stage_flow(SysKind::DockerCompose, "false", "localize", 0, &pairs)
            .await
            .unwrap();
        assert!(!out.exists(), "absent flow must be skipped");
    }

    #[tokio::test]
    async fn test_run_stage_flow_runs_and_injects_env() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let gx = dir.path().join("fake-gx");
        std::fs::write(
            &gx,
            r#"#!/bin/sh
case " $* " in
  *" --exists "*) exit 0 ;;
esac
printf '%s' "$DOMAIN" > "$OUT"
exit 0
"#,
        )
        .unwrap();
        std::fs::set_permissions(&gx, std::fs::Permissions::from_mode(0o755)).unwrap();

        let out = dir.path().join("ran.txt");
        let pairs = vec![
            ("DOMAIN".to_string(), "example.com".to_string()),
            ("OUT".to_string(), out.display().to_string()),
        ];
        SysCommandHandler::run_stage_flow(
            SysKind::DockerCompose,
            gx.to_str().unwrap(),
            "localize",
            0,
            &pairs,
        )
        .await
        .unwrap();

        // 流程被执行，且合并后的配置（DOMAIN）作为环境变量注入子进程
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "example.com");
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
        // 旧布局已存在 compose 时，不再额外生成 sys/ 下的新位置
        assert!(!prj.join("sys/docker-compose.yaml").exists());
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
    async fn test_sys_localize_resolves_vars_when_unresolved() {
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
                no_flow: false,
            };
            SysCommandHandler::handle_localize(localize_args)
                .await
                .unwrap();
        }

        let env = std::fs::read_to_string(temp_dir.path().join("compose_demo/.env")).unwrap();
        assert!(
            env.contains("SERVICE_IMAGE=nginx:alpine"),
            "unexpected .env: {env}"
        );
        assert!(env.contains("REPLICAS=1"), "unexpected .env: {env}");

        // 值文件是注释模板：存在但默认不生效（不钉住系统默认值）
        let value_file = temp_dir.path().join("compose_demo/values/sys_value.yml");
        let text = std::fs::read_to_string(&value_file).unwrap();
        assert!(
            text.lines()
                .all(|l| l.trim().is_empty() || l.trim().starts_with('#')),
            "template should be fully commented: {text}"
        );
        assert!(
            text.contains("# REPLICAS: 1"),
            "template missing vars: {text}"
        );
    }

    #[tokio::test]
    async fn test_sys_localize_merges_partial_ops_project_values() {
        once_init_log();
        let temp_dir = tempdir().unwrap();
        let prj = temp_dir.path().join("proj");
        std::fs::create_dir_all(&prj).unwrap();

        // 1. 项目下创建 docker-compose 系统
        {
            let _wd = WorkDirWithLock::change(&prj).unwrap();
            SysCommandHandler::handle_new(SysNewArgs {
                name: "my-sys".to_string(),
                kind: Some("docker-compose".to_string()),
            })
            .await
            .unwrap();
        }

        // 2. 项目声明该系统
        std::fs::write(
            prj.join("ops-prj.yml"),
            "name: proj\nwork_envs:\n  dep_root: ''\n  deps: []\nsys_models:\n- sys:\n    name: my-sys\n    kind: docker-compose\n    vender: ''\n  addr:\n    url: http://example.com/my-sys.tar.gz\n",
        )
        .unwrap();

        // 3. 解析变量（生成 sys/merged_vars.yml，并初始化项目值文件）
        {
            let _wd = WorkDirWithLock::change(prj.join("my-sys")).unwrap();
            SysCommandHandler::handle_update(SysUpdateArgs {
                debug_log: DebugLogArgs {
                    debug: 0,
                    log: None,
                },
                force: false,
            })
            .await
            .unwrap();
        }

        // 4. 项目值文件只写需要修改的项（部分覆盖）
        let prj_values = prj.join("values/my-sys");
        std::fs::write(prj_values.join("sys_value.yml"), "SERVICE_PORT: 9090\n").unwrap();

        // 5. 在系统目录内 localize：未列出的项取系统默认值
        {
            let _wd = WorkDirWithLock::change(prj.join("my-sys")).unwrap();
            SysCommandHandler::handle_localize(SysLocalizeArgs {
                debug_log: DebugLogArgs {
                    debug: 0,
                    log: None,
                },
                module: None,
                only: false,
                no_flow: false,
            })
            .await
            .unwrap();
        }

        let env = std::fs::read_to_string(prj.join("my-sys/.env")).unwrap();
        assert!(env.contains("SERVICE_PORT=9090"), "unexpected .env: {env}");
        assert!(
            env.contains("SERVICE_IMAGE=nginx:alpine"),
            "unexpected .env: {env}"
        );
        assert!(env.contains("REPLICAS=1"), "unexpected .env: {env}");
    }

    #[tokio::test]
    async fn test_sys_localize_ops_template_only_uses_defaults() {
        once_init_log();
        let temp_dir = tempdir().unwrap();
        let prj = temp_dir.path().join("proj");
        std::fs::create_dir_all(&prj).unwrap();

        // 项目下创建 docker-compose 系统并声明到 ops-prj.yml
        {
            let _wd = WorkDirWithLock::change(&prj).unwrap();
            SysCommandHandler::handle_new(SysNewArgs {
                name: "my-sys".to_string(),
                kind: Some("docker-compose".to_string()),
            })
            .await
            .unwrap();
        }
        std::fs::write(
            prj.join("ops-prj.yml"),
            "name: proj\nwork_envs:\n  dep_root: ''\n  deps: []\nsys_models:\n- sys:\n    name: my-sys\n    kind: docker-compose\n    vender: ''\n  addr:\n    url: http://example.com/my-sys.tar.gz\n",
        )
        .unwrap();

        // 首次 localize：自动解析变量，并在**项目值目录**生成注释模板
        {
            let _wd = WorkDirWithLock::change(prj.join("my-sys")).unwrap();
            SysCommandHandler::handle_localize(SysLocalizeArgs {
                debug_log: DebugLogArgs {
                    debug: 0,
                    log: None,
                },
                module: None,
                only: false,
                no_flow: false,
            })
            .await
            .unwrap();
        }

        // 模板落在项目值目录，且全为注释
        let tpl = prj.join("values/my-sys/sys_value.yml");
        let text = std::fs::read_to_string(&tpl).unwrap();
        assert!(
            text.lines()
                .all(|l| l.trim().is_empty() || l.trim().starts_with('#')),
            "template should be fully commented: {text}"
        );

        // 模板未取消注释 -> .env 完全取系统默认值
        let env = std::fs::read_to_string(prj.join("my-sys/.env")).unwrap();
        assert!(
            env.contains("SERVICE_IMAGE=nginx:alpine"),
            "unexpected .env: {env}"
        );
        assert!(env.contains("REPLICAS=1"), "unexpected .env: {env}");
    }

    #[tokio::test]
    async fn test_sys_localize_only_errors_when_vars_unresolved() {
        once_init_log();
        let temp_dir = tempdir().unwrap();

        // 新建 docker-compose 系统（尚未 update，无 merged_vars.yml）
        {
            let _wd = WorkDirWithLock::change(temp_dir.path()).unwrap();
            SysCommandHandler::handle_new(SysNewArgs {
                name: "only_demo".to_string(),
                kind: Some("docker-compose".to_string()),
            })
            .await
            .unwrap();
        }

        // --only 跳过 update：变量未解析时应给出明确错误
        {
            let _wd = WorkDirWithLock::change(temp_dir.path().join("only_demo")).unwrap();
            let result = SysCommandHandler::handle_localize(SysLocalizeArgs {
                debug_log: DebugLogArgs {
                    debug: 0,
                    log: None,
                },
                module: None,
                only: true,
                no_flow: false,
            })
            .await;
            assert!(result.is_err(), "--only should fail without resolved vars");
            let err = format!("{:?}", result.unwrap_err());
            assert!(err.contains("系统变量未解析"), "unexpected error: {err}");
        }
    }
}
