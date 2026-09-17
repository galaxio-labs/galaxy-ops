use clap::{Args, Parser};
use derive_getters::Getters;
use galaxy_ops::const_vars::VALUE_DIR;
use galaxy_ops::infra::DfxArgsGetter;
use galaxy_ops::module::operator::ModOperator;
use galaxy_ops::module::spec::make_mod_spec_example;
use galaxy_ops::prelude::{ErrorConv, ErrorOwe};
use galaxy_ops::types::{LocalizeOptions, ModuleLocalizable, RefUpdateable};
use galaxy_ops::{error::MainResult, module::operator::ModValuePaths};
use orion_conf::FilePersist;
use orion_variate::update::DownloadOptions;
use orion_vars::vars::{OriginDict, ValueDict};

use crate::commands::common::{DebugLogArgs, ForceArgs, LocalizeArgs};

// === 参数定义 ===

#[derive(Debug, Args, Getters)]
pub struct ModExampleArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,
}

#[derive(Debug, Args, Getters)]
pub struct ModNewArgs {
    #[arg(
        short,
        long,
        help = "模块名称 (Module name): 字母数字，可包含连字符和下划线\nalphanumeric with hyphens/underscores"
    )]
    pub(crate) name: String,

    #[clap(flatten)]
    pub debug_log: DebugLogArgs,
}

#[derive(Debug, Args, Getters)]
pub struct ModUpdateArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,

    #[clap(flatten)]
    pub force: ForceArgs,
}

#[derive(Debug, Args, Getters)]
pub struct ModLocalizeArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,

    #[clap(flatten)]
    pub localize: LocalizeArgs,
}

#[derive(Debug, Parser)]
pub enum ModCmd {
    /// 创建示例模块结构 (Create Example Module Structure)
    #[command(
        about = "创建示例模块结构 (Create Example Module Structure)",
        long_about = "创建一个完整的示例模块结构，包含示例配置和工作流，以展示模块组织和最佳实践。\n\
                     Create a complete example module structure with sample configurations and workflows to demonstrate module organization and best practices."
    )]
    Example(ModExampleArgs),

    /// 定义新的模块操作符 (Define New Module Operator)
    #[command(
        about = "定义新的模块操作符 (Define New Module Operator)",
        long_about = "使用给定的名称创建新的模块规范。这将初始化一个新的模块目录结构，其中包含所有必要的配置文件。\n\
                     Create a new module specification with the given name. This will initialize a new module directory structure with all necessary configuration files."
    )]
    New(ModNewArgs),

    /// 更新现有模块操作符 (Update Existing Module Operator)
    #[command(
        about = "更新现有模块操作符 (Update Existing Module Operator)",
        long_about = "更新现有模块的配置、依赖关系或规范。支持强制更新以覆盖现有配置。\n\
                     Update an existing module's configuration, dependencies, or specifications. Supports force updates to override existing configurations."
    )]
    Update(ModUpdateArgs),

    /// 本地化模块配置 (Localize Module Configuration)
    #[command(
        about = "本地化模块配置 (Localize Module Configuration)",
        long_about = "基于环境特定值为模块生成本地化配置文件。适用于将模块适配到不同的部署环境。\n\
                     Generate localized configuration files for the module based on environment-specific values. Useful for adapting modules to different deployment environments."
    )]
    Localize(ModLocalizeArgs),
}

// === DfxArgsGetter 实现 ===

impl DfxArgsGetter for ModExampleArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

impl DfxArgsGetter for ModNewArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

impl DfxArgsGetter for ModUpdateArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

impl DfxArgsGetter for ModLocalizeArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

// === 命令处理器 ===

pub struct ModCommandHandler;

impl ModCommandHandler {
    pub async fn handle_example(args: ModExampleArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);

        let spec = make_mod_spec_example().err_conv()?;
        spec.save_to(&std::path::PathBuf::from("./"), None)
            .source_resource()?;
        Ok(())
    }

    pub async fn handle_new(args: ModNewArgs) -> MainResult<()> {
        let current_dir = std::env::current_dir().expect("无法获取当前目录");
        let project_dir = current_dir.join(args.name());
        std::fs::create_dir(&project_dir).source_resource()?;

        galaxy_ops::infra::configure_dfx_logging(&args);
        let spec = ModOperator::make_new(&project_dir, args.name.as_str()).err_conv()?;
        spec.save().err_conv()?;
        Ok(())
    }

    pub async fn handle_update(args: ModUpdateArgs) -> MainResult<()> {
        let current_dir = std::env::current_dir().expect("无法获取当前目录");
        galaxy_ops::infra::configure_dfx_logging(&args);

        let operator = ModOperator::load(&current_dir).err_conv()?;
        let options = DownloadOptions::from((*args.force.force(), ValueDict::default()));
        let accessor = galaxy_ops::accessor::accessor_for_default();

        operator
            .update_local(accessor, &current_dir, &options)
            .await
            .err_conv()?;
        operator.init_setting_value()?;
        Ok(())
    }

    pub async fn handle_localize(args: ModLocalizeArgs) -> MainResult<()> {
        let current_dir = std::env::current_dir().expect("无法获取当前目录");
        galaxy_ops::infra::configure_dfx_logging(&args);

        let operator = ModOperator::load(&current_dir).err_conv()?;
        let val_path = ModValuePaths::from(current_dir).join(VALUE_DIR);
        operator
            .mod_localize(val_path, LocalizeOptions::new(OriginDict::default()))
            .await
            .err_conv()?;
        Ok(())
    }

    pub async fn execute(cmd: ModCmd) -> MainResult<()> {
        match cmd {
            ModCmd::Example(args) => Self::handle_example(args).await,
            ModCmd::New(args) => Self::handle_new(args).await,
            ModCmd::Update(args) => Self::handle_update(args).await,
            ModCmd::Localize(args) => Self::handle_localize(args).await,
        }
    }
}

// === 测试 ===

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[ignore = "reason"]
    #[tokio::test]
    async fn test_mod_new_command() {
        let temp_dir = tempdir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let args = ModNewArgs {
            name: "test_module".to_string(),
            debug_log: DebugLogArgs {
                debug: 0,
                log: None,
            },
        };

        let result = ModCommandHandler::handle_new(args).await;
        assert!(result.is_ok());
    }

    #[ignore = "reason"]
    #[tokio::test]
    async fn test_mod_example_command() {
        let temp_dir = tempdir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let args = ModExampleArgs {
            debug_log: DebugLogArgs {
                debug: 0,
                log: None,
            },
        };

        let result = ModCommandHandler::handle_example(args).await;
        assert!(result.is_ok());
    }

    #[ignore = "reason"]
    #[tokio::test]
    async fn test_execute_mod_commands() {
        let temp_dir = tempdir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        // 测试 example 命令
        let example_cmd = ModCmd::Example(ModExampleArgs {
            debug_log: DebugLogArgs {
                debug: 0,
                log: None,
            },
        });
        let result = ModCommandHandler::execute(example_cmd).await;
        assert!(result.is_ok());

        // 测试 new 命令
        let new_cmd = ModCmd::New(ModNewArgs {
            name: "test_module".to_string(),
            debug_log: DebugLogArgs {
                debug: 0,
                log: None,
            },
        });
        let result = ModCommandHandler::execute(new_cmd).await;
        assert!(result.is_ok());
    }

    #[ignore = "reason"]
    #[test]
    fn test_debug_args_getter() {
        let args = ModExampleArgs {
            debug_log: DebugLogArgs {
                debug: 2,
                log: Some("info".to_string()),
            },
        };

        assert_eq!(args.debug_level(), 2);
        assert_eq!(args.log_setting(), Some("info".to_string()));
    }

    #[ignore = "reason"]
    #[test]
    fn test_new_args_getter() {
        let args = ModNewArgs {
            name: "test_module".to_string(),
            debug_log: DebugLogArgs {
                debug: 1,
                log: Some("debug".to_string()),
            },
        };

        assert_eq!(args.debug_level(), 1);
        assert_eq!(args.log_setting(), Some("debug".to_string()));
        assert_eq!(args.name(), "test_module");
    }
}
