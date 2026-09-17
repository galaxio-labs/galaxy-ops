use super::prelude::*;

use crate::module::depend::DependencySet;

/// 系统部署类型：决定 `gops sys` 的 download/install/start/stop/status/diagnose
/// 命令分派到 GXL 工作流还是 docker compose。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SysKind {
    /// 模块化 GXL 工作流系统（默认，向后兼容旧的 `sys-prj.yml`）
    #[default]
    Gxl,
    /// 纯 docker-compose 声明式系统
    DockerCompose,
}

#[derive(Getters, Clone, Debug, Serialize, Deserialize)]
pub struct SysConf {
    #[serde(default)]
    kind: SysKind,
    test_envs: DependencySet,
}

impl SysConf {
    pub fn new(local_res: DependencySet) -> Self {
        Self {
            kind: SysKind::default(),
            test_envs: local_res,
        }
    }
    pub fn with_kind(mut self, kind: SysKind) -> Self {
        self.kind = kind;
        self
    }
    pub fn kind(&self) -> SysKind {
        self.kind
    }
    pub fn is_docker_compose(&self) -> bool {
        self.kind == SysKind::DockerCompose
    }
}
#[async_trait]
impl RefUpdateable<()> for SysConf {
    async fn update_local(
        &self,
        accessor: Accessor,
        path: &Path,
        options: &DownloadOptions,
    ) -> MainResult<()> {
        self.test_envs
            .update_local(accessor, path, options)
            .await
            .with(("sys-conf", "update test envs"))
    }
}
#[async_trait]
impl SystemLocalizable<()> for SysConf {
    async fn sys_localize(&self, _val_path: (), _options: LocalizeOptions) -> MainResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sys_kind_serde_kebab_case() {
        let yaml = serde_yaml::to_string(&SysKind::DockerCompose).unwrap();
        assert!(yaml.trim().contains("docker-compose"), "yaml={yaml}");
        assert_eq!(
            serde_yaml::from_str::<SysKind>("docker-compose").unwrap(),
            SysKind::DockerCompose
        );
        assert_eq!(
            serde_yaml::from_str::<SysKind>("gxl").unwrap(),
            SysKind::Gxl
        );
    }

    #[test]
    fn test_sys_conf_defaults_to_gxl() {
        // 旧的 sys-prj.yml 没有 kind 字段时，应退回默认 Gxl（向后兼容）
        let yaml = "test_envs:\n  dep_root: ''\n  deps: []\n";
        let conf: SysConf = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(conf.kind(), SysKind::Gxl);
        assert!(!conf.is_docker_compose());
    }

    #[test]
    fn test_sys_conf_docker_compose_kind() {
        let yaml = "kind: docker-compose\ntest_envs:\n  dep_root: ''\n  deps: []\n";
        let conf: SysConf = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(conf.kind(), SysKind::DockerCompose);
        assert!(conf.is_docker_compose());
    }
}
