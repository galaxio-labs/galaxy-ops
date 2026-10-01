use super::prelude::*;

use crate::module::depend::DependencySet;
use crate::ops_prj::system::OpsSystem;
use crate::types::{Accessor, RefUpdateable};

/// 运维项目 manifest：项目身份 + 工作环境 + 已导入系统列表。
///
/// 由原来的 `ops-prj.yml`（ProjectConf：name + work_envs）与 `ops-systems.yml`
/// （OpsTarget：sys_models）合并为单个 `ops-prj.yml`。
#[derive(Getters, Clone, Debug, Serialize, Deserialize)]
#[getset(get = "pub")]
pub struct OpsProjectConf {
    name: String,
    work_envs: DependencySet,
    #[serde(default)]
    sys_models: Vec<OpsSystem>,
    /// 项目侧备份声明（落点 / 份数 / 收哪些系统）。缺省 = 未声明，`prj backup` 用内置默认。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backup: Option<ProjectBackupConf>,
}

/// 项目侧的备份决策：**系统侧**声明“哪些路径、什么代价”，**项目侧**声明“落到哪、留几份、收哪些”。
#[derive(Getters, Clone, Debug, Serialize, Deserialize)]
#[getset(get = "pub")]
pub struct ProjectBackupConf {
    /// 备份落点目录（可写 `${VAR}`）。
    target: String,
    /// 保留最近 N 份（按文件名里的时间戳排序）。
    #[serde(default = "default_backup_keep")]
    keep: usize,
    /// 要收的系统；为空 = 收 `sys_models` 里的全部系统（按 restore 档）。
    #[serde(default)]
    systems: Vec<SystemBackupSel>,
}

fn default_backup_keep() -> usize {
    10
}

/// 单个系统的备份选取。
#[derive(Getters, Clone, Debug, Serialize, Deserialize)]
#[getset(get = "pub")]
pub struct SystemBackupSel {
    name: String,
    /// 收哪一档：`restore`（默认）只收 restore；`rebuild` 连 rebuild 档一起收。
    #[serde(default)]
    level: BackupLevel,
    /// 现场增量：本项目要额外收的（在系统声明基础上追加）。
    #[serde(default)]
    include: Vec<String>,
    /// 现场减量：本项目不收的（从系统声明里剔除，前缀匹配）。
    #[serde(default)]
    exclude: Vec<String>,
}

/// 备份档位。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackupLevel {
    /// 丢了要重装 / 换身份
    #[default]
    Restore,
    /// 可重建（重新投放/生成）
    Rebuild,
}

impl BackupLevel {
    /// 是否要把系统声明的 `rebuild` 档也收进来。
    pub fn includes_rebuild(&self) -> bool {
        matches!(self, BackupLevel::Rebuild)
    }
}

impl OpsProjectConf {
    pub fn new<S: Into<String>>(name: S, local_res: DependencySet) -> Self {
        Self {
            name: name.into(),
            work_envs: local_res,
            sys_models: Vec::new(),
            backup: None,
        }
    }
    pub fn for_test() -> Self {
        Self {
            name: "example_sys".to_string(),
            work_envs: DependencySet::example(),
            sys_models: Vec::new(),
            backup: None,
        }
    }
    pub fn import_sys(&mut self, sys: OpsSystem) {
        if !self.sys_models.contains(&sys) {
            self.sys_models.push(sys);
        }
    }
    pub fn load(path: &Path) -> MainResult<Self> {
        let conf_file = path.join(OPS_PRJ_CONF_FILE);
        let ins = Self::load_conf(&conf_file).source_conf()?;
        Ok(ins)
    }
}
#[async_trait]
impl InsUpdateable<OpsProjectConf> for OpsProjectConf {
    async fn update_local(
        mut self,
        accessor: Accessor,
        path: &Path,
        options: &DownloadOptions,
    ) -> MainResult<Self> {
        let mut flag = auto_exit_log!(
            info!(
                target : "ops-prj/conf",
                "ins conf update from {} success!", path.display()
            ),
            error!(
                target : "ops-prj/conf",
                "ins conf update from {} fail!", path.display()
            )
        );
        self.work_envs
            .update_local(accessor, path, options)
            .await
            .with(("ops-conf", "update work envs"))?;
        flag.mark_suc();
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "name: cust\nwork_envs:\n  dep_root: ''\n  deps: []\n";

    #[test]
    fn test_backup_absent_by_default_and_omitted_on_save() {
        let conf = OpsProjectConf::new("cust", DependencySet::default());
        assert!(conf.backup().is_none());
        let yaml = serde_yaml::to_string(&conf).unwrap();
        assert!(!yaml.contains("backup"), "yaml={yaml}");
    }

    #[test]
    fn test_backup_declaration_roundtrip() {
        let yaml = format!(
            "{BASE}backup:\n  target: /Volumes/backup/gw\n  keep: 5\n  systems:\n    - name: web-stack\n      level: rebuild\n      include:\n        - packages\n      exclude:\n        - configs/logs\n"
        );
        let conf: OpsProjectConf = serde_yaml::from_str(&yaml).unwrap();
        let b = conf.backup().as_ref().unwrap();
        assert_eq!(b.target(), "/Volumes/backup/gw");
        assert_eq!(*b.keep(), 5);
        assert_eq!(b.systems().len(), 1);
        assert_eq!(b.systems()[0].name(), "web-stack");
        assert!(b.systems()[0].level().includes_rebuild());
        assert_eq!(b.systems()[0].include(), &["packages".to_string()]);
        assert_eq!(b.systems()[0].exclude(), &["configs/logs".to_string()]);
    }

    #[test]
    fn test_backup_defaults_keep_and_level() {
        let yaml = format!("{BASE}backup:\n  target: /tmp/b\n  systems:\n    - name: web-stack\n");
        let conf: OpsProjectConf = serde_yaml::from_str(&yaml).unwrap();
        let b = conf.backup().as_ref().unwrap();
        assert_eq!(*b.keep(), 10);
        assert_eq!(b.systems()[0].level(), &BackupLevel::Restore);
        assert!(!b.systems()[0].level().includes_rebuild());
    }
}
