use super::prelude::*;

use crate::module::depend::DependencySet;

/// 系统部署类型：决定 `gops sys` 的 download/install/start/stop/status/diagnose
/// 命令分派到 GXL 工作流还是 docker compose。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SysKind {
    /// 模块化 GXL 工作流系统（默认，向后兼容旧的 `sys_model.yml`）
    #[default]
    Gxl,
    /// 纯 docker-compose 声明式系统
    DockerCompose,
}
impl SysKind {
    /// 是否是默认类型（gxl），用于序列化时省略默认值
    pub fn is_gxl(&self) -> bool {
        matches!(self, SysKind::Gxl)
    }
}

#[derive(Getters, Clone, Debug, Serialize, Deserialize)]
pub struct SysConf {
    test_envs: DependencySet,
    /// `gops sys package` 打包时排除的路径（glob，相对系统根）。
    /// 两种模式（默认 / `--full`）都生效；空则不写入。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ignore: Vec<String>,
}

impl SysConf {
    pub fn new(local_res: DependencySet) -> Self {
        Self {
            test_envs: local_res,
            ignore: Vec::new(),
        }
    }

    /// `gops sys package` 打包时排除的路径模式（`sys-prj.yml` 的 `ignore:` 节）。
    /// 显式方法（非 `getset`）：`SysConf` 未加 `#[getset(get = "pub")]`，加后 `ignore()` 会与
    /// 派生同名 getter 冲突。
    pub fn ignore(&self) -> &[String] {
        &self.ignore
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
    fn test_sys_kind_is_gxl() {
        assert!(SysKind::Gxl.is_gxl());
        assert!(!SysKind::DockerCompose.is_gxl());
    }

    #[test]
    fn test_sys_conf_ignore_roundtrip_and_omitted_when_empty() {
        // 空 → 不写入 ignore 节（不污染现有 sys-prj.yml）
        let empty = SysConf::new(DependencySet::default());
        let yaml = serde_yaml::to_string(&empty).unwrap();
        assert!(!yaml.contains("ignore"), "yaml={yaml}");

        // 非空 → 写入并可回读
        let with = SysConf {
            test_envs: DependencySet::default(),
            ignore: vec!["sys/*/mods".to_string(), ".env".to_string()],
        };
        let yaml = serde_yaml::to_string(&with).unwrap();
        assert!(yaml.contains("ignore"), "yaml={yaml}");
        let back: SysConf = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(
            back.ignore(),
            &["sys/*/mods".to_string(), ".env".to_string()][..]
        );
    }

    #[test]
    fn test_sys_conf_load_without_ignore_field() {
        // 旧 sys-prj.yml 无 ignore 节 → 默认空列表
        let conf: SysConf =
            serde_yaml::from_str("test_envs:\n  dep_root: ''\n  deps: []\n").unwrap();
        assert!(conf.ignore().is_empty());
    }
}
