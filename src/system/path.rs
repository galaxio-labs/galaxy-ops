use crate::const_vars::{
    EFFECTIVE_VARS_YML, MOD_VALUE_FILE, SYS_MODLE_DEF_YML, SYS_VARS_YML, SYS_VALUE_FILE,
    USED_READABLE_FILE,
};
use std::path::{Path, PathBuf};

use crate::const_vars::{MOD_LIST_YML, VARS_YML};
use crate::error::MainResult;
use crate::internal_prelude::ErrorOwe;
use crate::types::ValuePath;
use getset::Getters;
use orion_infra::path::{PathResult, ensure_path};

#[derive(Getters, Clone, Debug)]
#[getset(get = "pub ")]
pub struct SysTargetPaths {
    #[allow(dead_code)]
    target_root: PathBuf,
    define_path: PathBuf,
    spec_path: PathBuf,
    #[allow(dead_code)]
    sys_vars_path: PathBuf,
    modlist_path: PathBuf,
    workflow_path: PathBuf,
    setting_path: PathBuf,
}
impl From<&PathBuf> for SysTargetPaths {
    fn from(target_root: &PathBuf) -> Self {
        //let spec_path = target_root.join(SPEC_DIR);
        Self {
            target_root: target_root.to_path_buf(),
            define_path: target_root.join(SYS_MODLE_DEF_YML),
            sys_vars_path: target_root.join(VARS_YML),
            modlist_path: target_root.join(MOD_LIST_YML),
            workflow_path: target_root.to_path_buf(),
            spec_path: target_root.clone(),
            setting_path: target_root.join("setting"),
        }
    }
}

#[derive(Getters, Clone, Debug)]
pub struct SysOperatorPath {
    #[getset(get = "pub")]
    root: PathBuf,
}

impl SysOperatorPath {
    /// 创建新的 SysOperatorPath 实例
    pub fn new<P: AsRef<Path>>(root: P) -> Self {
        Self {
            root: PathBuf::from(root.as_ref()),
        }
    }

    /// 获取系统配置文件 v1 路径 (sys_prj.yml)
    pub fn conf_file_v1(&self) -> PathBuf {
        self.root.join("sys_prj.yml")
    }

    /// 获取系统配置文件 v2 路径 (sys-prj.yml)
    pub fn conf_file_v2(&self) -> PathBuf {
        self.root.join("sys-prj.yml")
    }

    /// 获取系统目录路径 (sys/)
    pub fn sys_dir(&self) -> PathBuf {
        self.root.join("sys")
    }
    pub fn effective_vars_file(&self) -> PathBuf {
        self.root.join("sys").join(EFFECTIVE_VARS_YML)
    }

    /// 旧名路径（`sys_vars.yml`，1.2.0 及更早版本）
    pub fn legacy_vars_file(&self) -> PathBuf {
        self.root.join("sys").join(SYS_VARS_YML)
    }

    /// 解析生效变量文件：`effective_vars.yml` 优先，缺失时回退旧名 `sys_vars.yml`。
    /// 两者都不存在时返回新名路径（供报错信息显示）。
    pub fn resolve_effective_vars_file(&self) -> PathBuf {
        let new_path = self.effective_vars_file();
        if new_path.exists() {
            return new_path;
        }
        let legacy = self.legacy_vars_file();
        if legacy.exists() {
            return legacy;
        }
        new_path
    }

    /// 获取值目录路径 (values/)
    pub fn value_dir(&self) -> PathBuf {
        self.root.join("values")
    }

    /// 获取系统值文件路径 (values/sys_value.yml)
    pub fn sys_value_file(&self) -> PathBuf {
        self.value_dir().join("sys_value.yml")
    }

    /// 检查是否需要配置文件迁移
    pub fn needs_conf_migration(&self) -> bool {
        self.conf_file_v1().exists() && !self.conf_file_v2().exists()
    }

    /// 执行配置文件迁移（如果需要）
    pub fn migrate_conf_file(&self) -> MainResult<()> {
        if self.needs_conf_migration() {
            std::fs::rename(self.conf_file_v1(), self.conf_file_v2()).source_resource()?;
        }
        Ok(())
    }

    /// 转换为 ValuePath，与现有 API 兼容
    pub fn to_value_path(&self) -> ValuePath {
        ValuePath::from_root(self.value_dir())
    }

    /// 确保项目根目录存在
    pub fn ensure_root_exists(&self) -> MainResult<()> {
        ensure_path(&self.root).source_logic()?;
        Ok(())
    }
}

#[derive(Getters, Clone, Debug)]
#[getset(get = "pub")]
pub struct SysValuePaths {
    root: PathBuf,
}
impl From<PathBuf> for SysValuePaths {
    fn from(value: PathBuf) -> Self {
        Self { root: value }
    }
}

impl SysValuePaths {
    pub fn sys_value_file(&self) -> PathBuf {
        self.root.join(SYS_VALUE_FILE)
    }
    pub fn mod_value_file(&self) -> PathBuf {
        self.root.join(MOD_VALUE_FILE)
    }
    pub fn used_with_origon(&self) -> PathBuf {
        self.root.join(USED_READABLE_FILE)
    }
    pub fn join<S: AsRef<str>>(self, path: S) -> Self {
        Self {
            root: self.root.join(path.as_ref()),
        }
    }
    pub fn ensure_join<S: AsRef<str>>(self, path: S) -> PathResult<Self> {
        Ok(Self {
            root: ensure_path(self.root.join(path.as_ref()))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_resolve_effective_vars_prefers_new_name() {
        let dir = tempdir().unwrap();
        let paths = SysOperatorPath::new(dir.path());
        std::fs::create_dir_all(dir.path().join("sys")).unwrap();
        std::fs::write(dir.path().join("sys/effective_vars.yml"), "new").unwrap();
        std::fs::write(dir.path().join("sys/sys_vars.yml"), "legacy").unwrap();

        assert_eq!(
            paths.resolve_effective_vars_file(),
            dir.path().join("sys/effective_vars.yml")
        );
    }

    #[test]
    fn test_resolve_effective_vars_falls_back_to_legacy_name() {
        let dir = tempdir().unwrap();
        let paths = SysOperatorPath::new(dir.path());
        std::fs::create_dir_all(dir.path().join("sys")).unwrap();
        std::fs::write(dir.path().join("sys/sys_vars.yml"), "legacy").unwrap();

        assert_eq!(
            paths.resolve_effective_vars_file(),
            dir.path().join("sys/sys_vars.yml")
        );
    }

    #[test]
    fn test_resolve_effective_vars_defaults_to_new_when_both_missing() {
        let dir = tempdir().unwrap();
        let paths = SysOperatorPath::new(dir.path());

        assert_eq!(
            paths.resolve_effective_vars_file(),
            dir.path().join("sys/effective_vars.yml")
        );
    }
}
