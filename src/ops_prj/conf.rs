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
}

impl OpsProjectConf {
    pub fn new<S: Into<String>>(name: S, local_res: DependencySet) -> Self {
        Self {
            name: name.into(),
            work_envs: local_res,
            sys_models: Vec::new(),
        }
    }
    pub fn for_test() -> Self {
        Self {
            name: "example_sys".to_string(),
            work_envs: DependencySet::example(),
            sys_models: Vec::new(),
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
