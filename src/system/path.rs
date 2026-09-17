use crate::const_vars::{
    MOD_VALUE_FILE, SYS_MODLE_DEF_YML, SYS_VALUE_FILE, SYS_VARS_YML, USED_READABLE_FILE,
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
    pub fn sys_vars_file(&self) -> PathBuf {
        self.root.join("sys").join(SYS_VARS_YML)
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
