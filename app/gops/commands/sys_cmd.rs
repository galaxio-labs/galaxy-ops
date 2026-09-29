use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Args, Parser};
use derive_getters::Getters;
use dialoguer::Select;
use galaxy_ops::const_vars::{SETTING_DIR, SPEC_DIR, USER_VALUE_FILE, VALUE_DIR};
use galaxy_ops::error::MainResult;
use galaxy_ops::infra::DfxArgsGetter;
use galaxy_ops::module::ModelSTD;
use galaxy_ops::module::model::MMOperator;
use galaxy_ops::prelude::{ErrorConv, ErrorOwe};
use galaxy_ops::project::load_value_file;
use galaxy_ops::report;
use galaxy_ops::system::drift::{DriftStatus, detect_drift};
use galaxy_ops::system::lock::DeliverLock;
use galaxy_ops::system::operator::SysOperator;
use galaxy_ops::system::pack::pack_system;
use galaxy_ops::system::setting::SysSetting;
use galaxy_ops::system::{SysKind, SysValuePaths};
use galaxy_ops::types::{LocalizeOptions, RefUpdateable};
use orion_infra::path::ensure_path;
use orion_variate::update::DownloadOptions;
use orion_vars::vars::{EnvDict, EnvEvalable, OriginDict, ValueDict};

use crate::commands::common::DebugLogArgs;
use crate::commands::gx_dispatch;

/// 解析当前系统的值目录：若系统属于某个运维项目（父目录有 `ops-prj.yml` 且列出该系统），
/// 使用项目为该系统维护的 `values/<sys_name>`（客户值）；否则使用 `<sys>/values`。
fn resolve_sys_value_path(sys_dir: &Path) -> SysValuePaths {
    match galaxy_ops::ops_prj::project::owner_project_value_dir(sys_dir) {
        Some(dir) => SysValuePaths::from(dir),
        None => SysValuePaths::from(sys_dir.to_path_buf()).join(VALUE_DIR),
    }
}

/// 已加载的模块视图：值比对与文件比对共用**一次**加载。
struct SysModuleView {
    name: String,
    /// 模块内容目录（`sys/<model>/mods/<mod>`，或旧布局）。
    root: PathBuf,
    mm: MMOperator,
}

/// 路径比较：忽略重复分隔符/尾随分隔符（`components()` 归一化）。
fn same_path(a: &Path, b: &Path) -> bool {
    a.components().eq(b.components())
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

    /// 打当前目录**全部**（含制品与本地化产物，用于隔离网络交付）；默认只打 git 入库文件
    #[arg(long = "full", alias = "no-git", default_value_t = false)]
    pub full: bool,
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
pub struct SysDiffArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,

    #[arg(long, help = "以 JSON 输出（便于脚本消费）")]
    pub json: bool,
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

    /// 检查系统配置漂移 (Check Configuration Drift)
    #[command(
        about = "检查系统配置漂移 (Check Configuration Drift)",
        long_about = "只读比对：重新计算本地化会产出的值与已生成的 .env，报告“值已变更但未重新 localize”。\
                     不做完整 reconcile；存在漂移时返回非零退出码，可用于 CI 卡口。\n\
                     Read-only: report values changed but not re-localized (drift).\n\
                     Usage: gops sys check"
    )]
    Check(SysCheckArgs),

    /// 展示系统值变更 (Show System Value Diff)
    #[command(
        about = "展示系统值变更 (Show System Value Diff)",
        long_about = "只读展示：比对系统默认值与客户覆盖后的生效值，列出每个键的初始值、生效值、来源与可变性。\n\
                     用于回答“哪些值被覆盖、被哪一层覆盖”。`--json` 输出机器可读结果。\n\
                     Usage: gops sys diff [--json]"
    )]
    Diff(SysDiffArgs),
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
impl DfxArgsGetter for SysCheckArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}
impl DfxArgsGetter for SysDiffArgs {
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

        // 3. 打包：默认只含 git 入库文件（不含制品）；`--full` 打整目录（含制品，用于隔离网络）。
        //    两种模式都排除 `sys-prj.yml` 的 `ignore:` 节。
        pack_system(
            &current_dir,
            &out_path,
            !args.full,
            operator.conf().ignore(),
        )?;
        println!("系统已打包: {}", out_path.display());
        Ok(())
    }

    pub async fn handle_localize(args: SysLocalizeArgs) -> MainResult<()> {
        let current_dir = std::env::current_dir().expect("无法获取当前目录");
        galaxy_ops::infra::configure_dfx_logging(&args);

        let spec = SysOperator::load(&current_dir).err_conv()?;
        let val_path = resolve_sys_value_path(&current_dir);

        // 默认：无条件先重解析变量（改 `sys/setting/vars.yml` 后一条命令即生效）。
        // `--only` 跳过解析，直接用现有 `sys/merged_vars.yml`。
        if !args.only {
            let options = DownloadOptions::from((false, ValueDict::default()));
            let accessor = galaxy_ops::accessor::accessor_for_default();
            spec.update_local(accessor, &current_dir, &options)
                .await
                .err_conv()?;
        }
        // 确保本地化值文件存在（生成 sys_value.yml 注释模板）；变量未解析时会报错
        spec.init_setting_value_in(val_path.clone())?;

        // 合并顺序与“初始层 / 生效层”的划分集中在此，供 diff 与 localize 共用。
        let (initial, dict) = Self::sys_value_layers(&spec, &val_path)?;
        // 系统层 + 各模块层（`sys/mod_list.yml`）：localize 会逐模块消费 `values/<mod>/`
        let sys_rows = Self::value_rows(&initial, &dict);
        let modules = Self::sys_modules(&spec);
        let mut module_rows = Self::module_value_changes(&modules, &val_path, &dict)?;
        if let Some(only) = &args.module {
            module_rows.retain(|(name, _)| name == only);
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
                &gx_dispatch::gx_bin_path(),
                "localize",
                debug,
                &stage_env,
            )
            .await?;
        }
        Self::print_value_changes(&sys_rows, &module_rows, false);
        Ok(())
    }

    /// 复现 `sys localize` 的层合并：
    /// - 初始层 = 系统默认值（`sys/merged_vars.yml` 的 `system:` 段，`origin=sys-defaults`）；
    /// - 生效层 = 初始层 ⊕ `values/sys_value.yml`(`sys-setting`) ⊕ `values/value.yml`(`customer`)。
    ///
    /// 返回的均为**未展开**值，直接可比对。
    fn sys_value_layers(
        spec: &SysOperator,
        val_path: &SysValuePaths,
    ) -> MainResult<(OriginDict, OriginDict)> {
        let initial =
            OriginDict::from(spec.system_default_values().err_conv()?).with_origin("sys-defaults");
        let mut dict = initial.clone();

        let value_file = val_path.sys_value_file();
        if value_file.exists() {
            let mut sys_dict = OriginDict::from(load_value_file(&value_file)?);
            sys_dict.set_source("sys-setting");
            dict.merge(&sys_dict);
        }

        let user_value_file = val_path.root().join(USER_VALUE_FILE);
        if user_value_file.exists() {
            let mut user_dict = OriginDict::from(load_value_file(&user_value_file)?);
            user_dict.set_source("customer");
            dict.merge(&user_dict);
        }
        Ok((initial, dict))
    }

    /// 统一的变更呈现：`json` 为真输出 JSON；否则无变更打 `[OK]`、有变更分范围打印表格。
    fn print_value_changes(
        sys_rows: &[report::ValueRow],
        modules: &[(String, Vec<report::ValueRow>)],
        json: bool,
    ) {
        Self::print_sys_diff(sys_rows, modules, &[], json);
    }

    /// 打印值变更（系统层 + 各模块）+ 文件覆盖。
    fn print_sys_diff(
        sys_rows: &[report::ValueRow],
        modules: &[(String, Vec<report::ValueRow>)],
        files: &[(String, Vec<report::FileRow>)],
        json: bool,
    ) {
        if json {
            println!("{}", report::render_sys_diff_json(sys_rows, modules, files));
            return;
        }
        let color = report::use_color();
        let has_module = modules.iter().any(|(_, rows)| !rows.is_empty());
        let has_file = files.iter().any(|(_, rows)| !rows.is_empty());
        if sys_rows.is_empty() && !has_module && !has_file {
            println!("[OK] 值无覆盖（全部取系统默认值）");
            return;
        }
        if !sys_rows.is_empty() {
            println!("[sys] 值变更 ({} 项):", sys_rows.len());
            print!("{}", report::render_table(sys_rows, color));
        }
        for (name, rows) in modules {
            if rows.is_empty() {
                continue;
            }
            println!("[mod: {name}] 值变更 ({} 项):", rows.len());
            print!("{}", report::render_table(rows, color));
        }
        for (target, rows) in files {
            report::print_file_rows(target, rows);
        }
    }

    /// 取「非未变更」的值行（供 diff / localize 复用）。
    fn value_rows(initial: &OriginDict, effective: &OriginDict) -> Vec<report::ValueRow> {
        report::diff_layers(initial, effective)
            .into_iter()
            .filter(|r| r.state() != report::ValueState::Same)
            .collect()
    }

    /// 加载系统内各模块（`sys/mod_list.yml`）的内容；值/文件比对共用，避免重复加载。
    fn sys_modules(spec: &SysOperator) -> Vec<SysModuleView> {
        let mut out = Vec::new();
        for mref in spec.sys_spec().mod_list().mods() {
            if !mref.is_enable() {
                continue;
            }
            let Some(root) = mref.content_dir().cloned() else {
                eprintln!(
                    "[WARN] 模块 {} 尚无内容（未下载？先 `gops sys update`）；跳过",
                    mref.name()
                );
                continue;
            };
            match mref.get_target_spec() {
                Ok(Some(mm)) => out.push(SysModuleView {
                    name: mref.name().clone(),
                    root,
                    mm,
                }),
                Ok(None) => {}
                Err(e) => eprintln!("[WARN] 模块 {} 加载失败：{e}；跳过", mref.name()),
            }
        }
        out
    }

    /// 各模块的值变更。
    ///
    /// 初始层 = 模块默认值（`sys/<model>/mods/<mod>/vars.yml`，`mod-default`）；
    /// 生效层 = 默认值 ⊕ `values/<mod>/value.yml`(`mod-cust`) ⊕ `values/<mod>/mod_value.yml`(`mod-setting`)
    ///          ⊕ 系统层 `sys_dict`（按其自身 origin 并入，只影响同名键）。
    ///
    /// 与 `ModuleSpecRef::sys_localize` 的消费路径一致（见 `src/module/refs.rs`）。
    fn module_value_changes(
        modules: &[SysModuleView],
        val_path: &SysValuePaths,
        sys_dict: &OriginDict,
    ) -> MainResult<Vec<(String, Vec<report::ValueRow>)>> {
        let mut out = Vec::new();
        for m in modules {
            let mod_root = val_path.root().join(&m.name);
            let (initial, effective) =
                galaxy_ops::project::mod_value_layers(m.mm.vars(), &mod_root, sys_dict.clone())?;
            out.push((m.name.clone(), Self::value_rows(&initial, &effective)));
        }
        Ok(out)
    }

    /// 文件覆盖层：`sys/setting/<mod>/**` 相对模块 `<mod>/spec/**` 的**新增 / 替换**。
    ///
    /// localize 会把 setting 层渲染进模块的 `local/`（模块自身的 `spec/` 也渲染进去），
    /// 所以「setting 覆盖了模块默认的哪些文件」就是这个系统的**文件级配置差异**。
    /// 纯路径 + 内容比对：不需要渲染，也不依赖上一次 localize 的磁盘状态。
    fn sys_file_overrides(
        modules: &[SysModuleView],
        spec: &SysOperator,
        evaled: &ValueDict,
    ) -> Vec<(String, Vec<report::FileRow>)> {
        let mut out = Vec::new();
        for (entry, ms) in spec.sys_spec().setting().list().dicts() {
            if !*ms.enable() {
                continue;
            }
            // 路径模板（`${GXL_PRJ_ROOT}` 等）需用展开后的值
            let lv = ms.localize().clone().env_eval(evaled);
            let src_dir = PathBuf::from(lv.src());
            let dst_dir = PathBuf::from(lv.dst());
            let Some(mod_root) = dst_dir.parent() else {
                continue;
            };
            if !src_dir.is_dir() {
                eprintln!(
                    "[WARN] setting 条目 {entry} 的源目录不存在：{}；跳过其文件覆盖",
                    src_dir.display()
                );
                continue;
            }
            // 与 localize 一致：只比较模块 `spec/` 中真正会被渲染的文件
            // （应用模块 `setting.yml` 的 include/exclude）
            let tpl = modules
                .iter()
                .find(|m| same_path(&m.root, mod_root))
                .and_then(|m| m.mm.setting().as_ref())
                .and_then(|s| s.localize().clone())
                .and_then(|x| x.templatize_path().clone())
                .map(|x| x.export_paths(mod_root))
                .unwrap_or_default();
            let keep = |p: &Path| tpl.is_include(p) && !tpl.is_exclude(p);
            let base = report::snapshot_tree_filtered(&mod_root.join(SPEC_DIR), &keep);
            let overlay = report::snapshot_tree(&src_dir);
            let rows = report::diff_files(&base, &overlay);
            out.push((
                format!(
                    "{} ← {}",
                    report::display_path(&dst_dir),
                    report::display_path(&src_dir)
                ),
                rows,
            ));
        }
        out
    }

    pub async fn handle_diff(args: SysDiffArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);
        let current_dir = std::env::current_dir().expect("无法获取当前目录");

        let spec = SysOperator::load(&current_dir).err_conv()?;
        let val_path = resolve_sys_value_path(&current_dir);
        let (initial, effective) = Self::sys_value_layers(&spec, &val_path)?;
        let sys_rows = Self::value_rows(&initial, &effective);
        let modules = Self::sys_modules(&spec);
        let module_rows = Self::module_value_changes(&modules, &val_path, &effective)?;
        // 文件覆盖的路径模板需展开（`${GXL_PRJ_ROOT}` 等）
        let evaled = effective
            .clone()
            .env_eval(&EnvDict::default())
            .export_dict();
        let file_rows = Self::sys_file_overrides(&modules, &spec, &evaled);
        Self::print_sys_diff(&sys_rows, &module_rows, &file_rows, args.json);
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
        gx_dispatch::run_gx_flow(gx_path, "default", debug, None, flow_name, env_pairs).await
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

    pub async fn handle_check(args: SysCheckArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);
        let current_dir = std::env::current_dir().source_resource()?;

        let op = SysOperator::load(&current_dir).err_conv()?;
        let report = detect_drift(&op, &current_dir).err_conv()?;

        // 变量定义比已解析结果更新（仅比对 `.env` 看不到）：先提醒。
        if report.vars_stale() {
            println!(
                "[WARN] 变量定义（sys/setting/vars.yml 等）比 sys/merged_vars.yml 更新：可能尚未重新解析；\n\
                 运行 `gops sys localize` 使其生效（或 `gops sys update`）。"
            );
        }

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
            SysCmd::Check(args) => Self::handle_check(args).await,
            SysCmd::Diff(args) => Self::handle_diff(args).await,
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

/// 把 `--kind` 的字符串解析为系统类型，未知值默认 Gxl。
fn parse_kind(s: &str) -> SysKind {
    match s {
        "docker-compose" | "compose" => SysKind::DockerCompose,
        _ => SysKind::Gxl,
    }
}

// === 测试 ===

#[cfg(test)]
mod tests {
    use super::*;
    use galaxy_ops::infra::{WorkDirWithLock, once_init_log};
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
            full: false,
        };

        assert_eq!(args.debug_level(), 1);
        assert_eq!(args.log_setting(), None);
        assert!(!args.force);
        assert!(args.output.is_none());
        assert!(!args.full);
    }

    #[test]
    fn test_sys_package_full_flag_parses() {
        let cmd = SysCmd::try_parse_from(["gops", "package", "--full"]).unwrap();
        match cmd {
            SysCmd::Package(a) => assert!(a.full),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn test_sys_package_legacy_no_git_alias_parses() {
        // 旧名 `--no-git` 保留为隐藏别名（兼容 2.0.2）
        let cmd = SysCmd::try_parse_from(["gops", "package", "--no-git"]).unwrap();
        match cmd {
            SysCmd::Package(a) => assert!(a.full),
            other => panic!("unexpected: {other:?}"),
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

        // orion-sec 归一化为大写并加 SEC_ 前缀；掩码/明文转换见 run_cmd 的 sec_env_pairs_for
        let dict = orion_sec::load_sec_dict().unwrap();
        assert!(dict.get("SEC_DB_PASSWORD").is_some());
        assert!(dict.get("SEC_POSTGRES_PASSWORD").is_some());
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

    #[tokio::test]
    async fn test_sys_diff_lists_overrides_from_value_file() {
        once_init_log();
        let temp_dir = tempdir().unwrap();

        // 项目下创建 docker-compose 系统并解析变量
        {
            let _wd = WorkDirWithLock::change(temp_dir.path()).unwrap();
            SysCommandHandler::handle_new(SysNewArgs {
                name: "diff_demo".to_string(),
                kind: Some("docker-compose".to_string()),
            })
            .await
            .unwrap();
        }

        let sys_dir = temp_dir.path().join("diff_demo");
        {
            let _wd = WorkDirWithLock::change(&sys_dir).unwrap();
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

        // 客户覆盖层只写需要改的项
        std::fs::write(
            sys_dir.join("values/value.yml"),
            "SERVICE_IMAGE: custom:1\n",
        )
        .unwrap();

        let spec = SysOperator::load(&sys_dir).unwrap();
        let val_path = resolve_sys_value_path(&sys_dir);
        let (initial, effective) = SysCommandHandler::sys_value_layers(&spec, &val_path).unwrap();
        let rows: Vec<_> = report::diff_layers(&initial, &effective)
            .into_iter()
            .filter(|r| r.state() != report::ValueState::Same)
            .collect();

        let row = rows
            .iter()
            .find(|r| r.key() == "SERVICE_IMAGE")
            .expect("SERVICE_IMAGE override row");
        assert_eq!(row.initial(), Some("nginx:alpine"));
        assert_eq!(row.effective(), Some("custom:1"));
        assert_eq!(row.origin(), Some("customer"));
        assert_eq!(row.state(), report::ValueState::Changed);

        // 未被覆盖的默认值不出现
        assert!(!rows.iter().any(|r| r.key() == "REPLICAS"));

        // handle_diff 端到端可跑通（含 --json 分支）
        {
            let _wd = WorkDirWithLock::change(&sys_dir).unwrap();
            SysCommandHandler::handle_diff(SysDiffArgs {
                debug_log: DebugLogArgs {
                    debug: 0,
                    log: None,
                },
                json: true,
            })
            .await
            .unwrap();
        }
    }
}
