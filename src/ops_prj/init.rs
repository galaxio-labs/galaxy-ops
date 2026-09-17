use std::path::Path;

use crate::internal_prelude::{ErrorOwe, ErrorWith};

use crate::{error::MainResult, workflow::gxl::GxlAction};

const SYS_SETUP_GXL: &str = include_str!("init/workflows/setup.gxl");
const SYS_UPDATE_GXL: &str = include_str!("init/workflows/update.gxl");
const SYS_GITIGNORE: &str = include_str!("init/.gitignore");
pub trait WorkOperatsIniter {
    fn sys_setup_tpl() -> Self;
    fn sys_update_tpl() -> Self;
}
pub trait SysPrjIniter {
    fn spec_tpl() -> Self;
}

impl WorkOperatsIniter for GxlAction {
    fn sys_setup_tpl() -> Self {
        Self::new("setup.gxl".into(), SYS_SETUP_GXL.to_string())
    }
    fn sys_update_tpl() -> Self {
        Self::new("update.gxl".into(), SYS_UPDATE_GXL.to_string())
    }
}

pub fn workins_init_gitignore(path: &Path) -> MainResult<()> {
    let ignore_path = path.join(".gitignore");
    if !ignore_path.exists() {
        std::fs::write(&ignore_path, SYS_GITIGNORE)
            .source_resource()
            .with(&ignore_path)?;
    }
    Ok(())
}
