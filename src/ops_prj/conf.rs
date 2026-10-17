use super::prelude::*;

use crate::error::MainReason;
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
    /// 项目侧契约归档声明（落点 / 份数）。缺省 = 未声明，`prj contract export` 用内置默认。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    contract: Option<ProjectContractConf>,
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
    /// 要收的系统；为空 = 收 `sys_models` 里的全部系统（按各系统 `backup:` 声明）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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
    /// 现场增量：本项目要额外收的（在系统声明基础上追加）。
    #[serde(default)]
    include: Vec<String>,
    /// 现场减量：本项目不收的（从系统声明里剔除，前缀匹配）。
    #[serde(default)]
    exclude: Vec<String>,
}

/// 项目侧的契约归档决策：落点 / 份数（对称于 [`ProjectBackupConf`]）。
#[derive(Getters, Clone, Debug, Serialize, Deserialize)]
#[getset(get = "pub")]
pub struct ProjectContractConf {
    /// 归档落点目录（可写 `${VAR}`）。
    target: String,
    /// 保留最近 N 份（按文件名里的时间戳排序）。
    #[serde(default = "default_contract_keep")]
    keep: usize,
}

fn default_contract_keep() -> usize {
    10
}

impl OpsProjectConf {
    pub fn new<S: Into<String>>(name: S, local_res: DependencySet) -> Self {
        Self {
            name: name.into(),
            work_envs: local_res,
            sys_models: Vec::new(),
            backup: None,
            contract: None,
        }
    }
    pub fn for_test() -> Self {
        Self {
            name: "example_sys".to_string(),
            work_envs: DependencySet::example(),
            sys_models: Vec::new(),
            backup: None,
            contract: None,
        }
    }

    /// 设置项目侧备份决策（builder；供示例 / 测试）。
    pub fn with_backup(mut self, target: impl Into<String>, keep: usize) -> Self {
        self.backup = Some(ProjectBackupConf {
            target: target.into(),
            keep,
            systems: Vec::new(),
        });
        self
    }

    /// 设置项目侧契约归档决策（builder；供示例 / 测试）。
    pub fn with_contract(mut self, target: impl Into<String>, keep: usize) -> Self {
        self.contract = Some(ProjectContractConf {
            target: target.into(),
            keep,
        });
        self
    }
    pub fn import_sys(&mut self, sys: OpsSystem) {
        if !self.sys_models.contains(&sys) {
            self.sys_models.push(sys);
        }
    }

    /// 按系统名 upsert：`prj import` 重复导入同名系统时更新原记录，而不是追加重复项。
    pub fn upsert_sys(&mut self, sys: OpsSystem) {
        if let Some(existing) = self
            .sys_models
            .iter_mut()
            .find(|s| s.sys().name() == sys.sys().name())
        {
            *existing = sys;
        } else {
            self.sys_models.push(sys);
        }
    }

    /// 更新某系统声明的 `version:`；返回值表示是否真的发生变化。
    pub fn set_sys_version(&mut self, sys: &str, version: &str) -> MainResult<bool> {
        let version = version.trim();
        if version.is_empty() {
            return Err(MainReason::logic_detail(format!(
                "系统 `{sys}` 的版本不能为空"
            )));
        }
        let Some(entry) = self
            .sys_models
            .iter_mut()
            .find(|s| s.sys().name().as_str() == sys)
        else {
            return Err(MainReason::logic_detail(format!(
                "ops-prj.yml 里没有系统 `{sys}`"
            )));
        };
        if entry.declared_version() == Some(version) {
            return Ok(false);
        }
        entry.set_version(version);
        Ok(true)
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
        let mut ctx = OperationContext::doing("update ops conf")
            .with_auto_log()
            .with_mod_path("ops-prj/conf");
        ctx.record("path", path.display());
        self.work_envs
            .update_local(accessor, path, options)
            .await
            .with(("ops-conf", "update work envs"))?;
        ctx.mark_suc();
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
            "{BASE}backup:\n  target: /Volumes/backup/gw\n  keep: 5\n  systems:\n    - name: web-stack\n      include:\n        - packages\n      exclude:\n        - configs/logs\n"
        );
        let conf: OpsProjectConf = serde_yaml::from_str(&yaml).unwrap();
        let b = conf.backup().as_ref().unwrap();
        assert_eq!(b.target(), "/Volumes/backup/gw");
        assert_eq!(*b.keep(), 5);
        assert_eq!(b.systems().len(), 1);
        assert_eq!(b.systems()[0].name(), "web-stack");
        assert_eq!(b.systems()[0].include(), &["packages".to_string()]);
        assert_eq!(b.systems()[0].exclude(), &["configs/logs".to_string()]);
    }

    #[test]
    fn test_backup_defaults_keep() {
        let yaml = format!("{BASE}backup:\n  target: /tmp/b\n  systems:\n    - name: web-stack\n");
        let conf: OpsProjectConf = serde_yaml::from_str(&yaml).unwrap();
        let b = conf.backup().as_ref().unwrap();
        assert_eq!(*b.keep(), 10);
    }

    #[test]
    fn test_contract_absent_by_default_and_omitted_on_save() {
        let conf = OpsProjectConf::new("cust", DependencySet::default());
        assert!(conf.contract().is_none());
        let yaml = serde_yaml::to_string(&conf).unwrap();
        assert!(!yaml.contains("contract"), "yaml={yaml}");
    }

    #[test]
    fn test_contract_declaration_roundtrip_and_default_keep() {
        let yaml = format!("{BASE}contract:\n  target: ./contract\n");
        let conf: OpsProjectConf = serde_yaml::from_str(&yaml).unwrap();
        let c = conf.contract().as_ref().unwrap();
        assert_eq!(c.target(), "./contract");
        assert_eq!(*c.keep(), 10); // 缺省 10
    }

    #[test]
    fn test_upsert_sys_replaces_same_name() {
        let yaml = |version: &str, path: &str| {
            format!(
                "sys:\n  name: web-stack\n  kind: docker-compose\n  vender: ''\nversion: {version}\naddr:\n  path: {path}\n"
            )
        };
        let a: OpsSystem = serde_yaml::from_str(&yaml("0.9.0", "./old-v{version}.tar.gz")).unwrap();
        let b: OpsSystem =
            serde_yaml::from_str(&yaml("0.10.0", "./new-v{version}.tar.gz")).unwrap();

        let mut conf = OpsProjectConf::new("cust", DependencySet::default());
        conf.upsert_sys(a);
        conf.upsert_sys(b);

        assert_eq!(conf.sys_models().len(), 1, "same name must be upserted");
        assert_eq!(conf.sys_models()[0].declared_version(), Some("0.10.0"));
        assert_eq!(
            conf.sys_models()[0].addr_template(),
            "./new-v{version}.tar.gz",
            "upsert must replace the whole entry"
        );
    }

    #[test]
    fn test_set_sys_version_updates_and_roundtrips() {
        let yaml = format!(
            "{BASE}sys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    path: ./s-v{{version}}.tar.gz\n"
        );
        let mut conf: OpsProjectConf = serde_yaml::from_str(&yaml).unwrap();
        assert!(conf.set_sys_version("web-stack", "0.10.0").unwrap());
        // 同值不重复写。
        assert!(!conf.set_sys_version("web-stack", " 0.10.0 ").unwrap());
        assert!(conf.set_sys_version("nope", "1.0.0").is_err());
        assert!(
            conf.set_sys_version("web-stack", "  ").is_err(),
            "empty version must be rejected"
        );

        let saved = serde_yaml::to_string(&conf).unwrap();
        assert!(saved.contains("version: 0.10.0"), "saved={saved}");
    }
}
