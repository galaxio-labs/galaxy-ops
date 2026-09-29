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

/// 迁移已有系统的 `.gitignore`：补齐 1.4 的新布局忽略规则。
///
/// 1.3 及更早生成的 `.gitignore` 只有 `sys/mods`，它匹配不到
/// `sys/<model>/mods/`；这里只在缺失时追加一行，幂等且不改动其他内容。
pub fn sys_migrate_gitignore(path: &Path) -> MainResult<()> {
    let ignore_path = path.join(".gitignore");
    if !ignore_path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(&ignore_path)
        .source_resource()
        .with(&ignore_path)?;
    if content
        .lines()
        .any(|l| matches!(l.trim(), "sys/*/mods" | "sys/**/mods"))
    {
        return Ok(());
    }
    let mut updated = content;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str("# 迁移（gops >= 1.4）：模块按目标模型分组\nsys/*/mods\n");
    std::fs::write(&ignore_path, updated)
        .source_resource()
        .with(&ignore_path)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_init_gitignore_template_has_new_pattern() {
        let dir = tempdir().unwrap();
        sys_init_gitignore(dir.path()).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(content.contains("sys/*/mods"));
        assert!(!content.lines().any(|l| l.trim() == "sys/mods"));
    }

    #[test]
    fn test_migrate_gitignore_appends_new_pattern_keeping_legacy() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "sys/mods\nvalues/*\n").unwrap();

        sys_migrate_gitignore(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(content.contains("sys/*/mods"));
        assert!(content.contains("sys/mods"));
        assert!(content.contains("values/*"));
    }

    #[test]
    fn test_migrate_gitignore_is_idempotent() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "sys/*/mods\n").unwrap();
        let before = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();

        sys_migrate_gitignore(dir.path()).unwrap();

        let after = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn test_migrate_gitignore_no_file_is_noop() {
        let dir = tempdir().unwrap();
        sys_migrate_gitignore(dir.path()).unwrap();
        assert!(!dir.path().join(".gitignore").exists());
    }

    #[test]
    fn test_sys_operators_tpl_invariants() {
        // 系统侧算子统一指向权威仓库 galaxio-hub（旧模板曾用 galaxy-operators）
        assert!(SYS_OPS_GXL.contains("galaxio-hub/ops-gxl"));
        assert!(!SYS_OPS_GXL.contains("galaxy-operators/ops-gxl"));
        // 新布局（sys/<model>/mods/<mod>）需要 ops-gxl 的 2.0 线；main 保留给旧布局
        assert!(SYS_OPS_GXL.contains("${GXL_CHANNEL:2.0}"));
    }
}
