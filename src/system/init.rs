use std::path::Path;

use crate::internal_prelude::{ErrorOwe, ErrorWith};

use crate::{
    error::MainResult,
    workflow::{
        act::{Workflow, Workflows},
        gxl::GxlAction,
        prj::GxlProject,
    },
};

const SYS_OPS_GXL: &str = include_str!("init/workflows/operators.gxl");
pub const SYS_PRJ_WORK: &str = include_str!("init/_gal/work.gxl");
pub const SYS_PRJ_ADM: &str = include_str!("init/_gal/adm.gxl");
const SYS_GITIGNORE: &str = include_str!("init/.gitignore");
const SYS_DOCKER_COMPOSE: &str = include_str!("init/docker-compose.yml");

pub trait SysActIniter {
    fn sys_operators_tpl() -> Self;
}
pub trait SysPrjIniter {
    fn spec_tpl() -> Self;
}

impl SysActIniter for GxlAction {
    fn sys_operators_tpl() -> Self {
        Self::new("operators.gxl".into(), SYS_OPS_GXL.to_string())
    }
}
impl SysPrjIniter for GxlProject {
    fn spec_tpl() -> Self {
        Self::from(SYS_PRJ_WORK)
    }
}

pub trait SysIniter {
    fn sys_tpl_init() -> Self;
}

impl SysIniter for Workflows {
    fn sys_tpl_init() -> Self {
        let actions = vec![Workflow::Gxl(GxlAction::sys_operators_tpl())];
        Self::new(actions)
    }
}

pub fn sys_init_gitignore(path: &Path) -> MainResult<()> {
    let ignore_path = path.join(".gitignore");
    if !ignore_path.exists() {
        std::fs::write(&ignore_path, SYS_GITIGNORE)
            .source_resource()
            .with(&ignore_path)?;
    }
    Ok(())
}

pub fn sys_init_docker_compose(path: &Path) -> MainResult<()> {
    let compose_path = path.join("docker-compose.yml");
    if !compose_path.exists() {
        std::fs::write(&compose_path, SYS_DOCKER_COMPOSE)
            .source_resource()
            .with(&compose_path)?;
    }
    Ok(())
}
