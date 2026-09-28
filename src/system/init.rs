use std::path::Path;

use crate::internal_prelude::{ErrorOwe, ErrorWith};

use crate::{
    error::MainResult,
    system::SysOperatorPath,
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
const SYS_DOCKER_COMPOSE: &str = include_str!("init/docker-compose.yaml");

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

/// 生成 compose 脚手架。
///
/// compose 属于「系统定义」，默认落在 `sys/docker-compose.yaml`（见 `SysOperatorPath::compose_file`
/// 的查找链）。已有 compose（`sys/` 内或旧布局的根目录）时不覆盖，
/// 保证 `sys new` 对已存在的目录幂等。
pub fn sys_init_docker_compose(path: &Path) -> MainResult<()> {
    let paths = SysOperatorPath::new(path);
    if paths.compose_file().is_some() {
        return Ok(());
    }

    let compose_path = paths.sys_compose_file();
    if let Some(parent) = compose_path.parent() {
        std::fs::create_dir_all(parent)
            .source_resource()
            .with(&compose_path)?;
    }
    std::fs::write(&compose_path, SYS_DOCKER_COMPOSE)
        .source_resource()
        .with(&compose_path)?;
    Ok(())
}
