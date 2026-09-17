use clap::{Args, Parser};
use derive_getters::Getters;
use galaxy_ops::error::MainResult;
use galaxy_ops::infra::DfxArgsGetter;
use galaxy_ops::ops_prj::project::OpsProject;
use galaxy_ops::prelude::{ErrorConv, ErrorOwe};
use galaxy_ops::types::InsUpdateable;
use orion_infra::path::make_new_path;
use orion_variate::update::DownloadOptions;
use orion_vars::vars::ValueDict;

use crate::commands::common::{DebugLogArgs, ForceArgs};

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

#[derive(Debug, Parser)]
pub enum PrjCmd {
    #[command(about = "创建维护工程 (Create Maintenance Project)")]
    New(PrjNewArgs),
    #[command(about = "导入系统到工程 (Import System to Project)")]
    Import(PrjImportArgs),
    #[command(about = "维护工程 (Maintain Project)")]
    Update(PrjUpdateArgs),
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

pub struct PrjCommandHandler;

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
        let mut prj = OpsProject::load(&current_dir).err_conv()?;
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
        let prj = OpsProject::load(&current_dir).err_conv()?;
        let accessor = galaxy_ops::accessor::accessor_for_default();

        prj.update_local(accessor, &current_dir, &options)
            .await
            .err_conv()?;
        Ok(())
    }

    pub async fn execute(cmd: PrjCmd) -> MainResult<()> {
        match cmd {
            PrjCmd::New(args) => Self::handle_new(args).await,
            PrjCmd::Import(args) => Self::handle_import(args).await,
            PrjCmd::Update(args) => Self::handle_update(args).await,
        }
    }
}
