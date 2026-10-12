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
    /// **现场态**：升级/重建时**不覆盖、也不删**的路径（glob，相对系统根，同 `ignore` 的写法）。
    ///
    /// 与 `ignore` 的分工：`ignore` 管“不进交付包”，`protect` 管“升级/重建不许碰”。
    /// 两者**不相等**（`.env`、`values/`、`deliver.lock` 都不进包，但都属于现场态），
    /// 但应满足不变式 `ignore ⊆ protect` —— 不进包的按定义就不是包内容。
    /// 不一致由 `gops prj diagnose` 报出。
    ///
    /// 旧名 `preserve` 仍可读（`#[serde(alias)]`），新写一律用 `protect`。
    #[serde(default, skip_serializing_if = "Vec::is_empty", alias = "preserve")]
    protect: Vec<String>,
    /// **对外契约**：承载“不可重建的公开现场事实”（对外域名 / 端点 / 公开证书）的文件或目录
    /// （glob，相对系统根）。语义**自足**：命中即 **不覆盖 + 必收（`backup`）+ 不进包**。
    /// 内容 site-owned、不在交付包内；`sys-prj.yml` 只指路。见 `docs/design/contract.md`。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    contract: Vec<String>,
    /// **备份选取**（glob，相对系统根）：**必收**的现场态（丢了要重装 / 换身份）。
    /// 只表达“丢失的代价”，不触发任何删除；不在这里的 = 不收、不碰。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    backup: Vec<String>,
}

impl SysConf {
    pub fn new(local_res: DependencySet) -> Self {
        Self {
            test_envs: local_res,
            ignore: Vec::new(),
            protect: Vec::new(),
            contract: Vec::new(),
            backup: Vec::new(),
        }
    }

    /// 设置现场态声明（builder；供示例 / 测试构造带声明的 `SysConf`）。
    pub fn with_site_state(
        mut self,
        ignore: Vec<String>,
        protect: Vec<String>,
        contract: Vec<String>,
        backup: Vec<String>,
    ) -> Self {
        self.ignore = ignore;
        self.protect = protect;
        self.contract = contract;
        self.backup = backup;
        self
    }

    /// `gops sys package` 打包时排除的路径模式（`sys-prj.yml` 的 `ignore:` 节）。
    /// 显式方法（非 `getset`）：`SysConf` 未加 `#[getset(get = "pub")]`，加后 `ignore()` 会与
    /// 派生同名 getter 冲突。
    pub fn ignore(&self) -> &[String] {
        &self.ignore
    }

    /// 现场态路径（升级/重建不覆盖、不删除）。见字段注释里的不变式。
    pub fn protect(&self) -> &[String] {
        &self.protect
    }

    /// 对外契约路径（不可重建的公开现场事实）。见字段注释。
    pub fn contract(&self) -> &[String] {
        &self.contract
    }

    /// **升级的保护集**：`protect` ∪ `contract`。
    ///
    /// `contract` 语义自足——声明即**自动纳入保护**（升级不碰），所以调用方（`prj sync` /
    /// `prj upgrade`）读的是这个并集，而不是裸的 `protect`。
    pub fn protection_paths(&self) -> Vec<String> {
        let mut out = self.protect.clone();
        for item in &self.contract {
            if !out.contains(item) {
                out.push(item.clone());
            }
        }
        out
    }

    /// **备份选取**：`backup` ∪ `contract`（**必收**）。
    ///
    /// `contract` 是不可重建的现场事实——声明即自动进备份选取，无需再写第二处。
    pub fn backup_paths(&self) -> Vec<String> {
        let mut out = self.backup.clone();
        for item in &self.contract {
            if !out.contains(item) {
                out.push(item.clone());
            }
        }
        out
    }

    /// 备份选取（`sys-prj.yml` 的 `backup:` 节，**不含** contract 派生项）。
    pub fn backup(&self) -> &[String] {
        &self.backup
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
            protect: Vec::new(),
            contract: Vec::new(),
            backup: Vec::new(),
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
    fn test_sys_conf_protect_roundtrip_and_omitted_when_empty() {
        // 空 → 不写入 protect 节
        let empty = SysConf::new(DependencySet::default());
        let yaml = serde_yaml::to_string(&empty).unwrap();
        assert!(!yaml.contains("protect"), "yaml={yaml}");

        let with = SysConf {
            test_envs: DependencySet::default(),
            ignore: vec!["configs".to_string()],
            protect: vec!["configs".to_string(), ".env".to_string()],
            contract: Vec::new(),
            backup: Vec::new(),
        };
        let yaml = serde_yaml::to_string(&with).unwrap();
        assert!(yaml.contains("protect"), "yaml={yaml}");
        let back: SysConf = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(
            back.protect(),
            &["configs".to_string(), ".env".to_string()][..]
        );
    }

    #[test]
    fn test_protection_paths_unions_contract() {
        let conf = SysConf {
            test_envs: DependencySet::default(),
            ignore: Vec::new(),
            protect: vec!["configs".to_string()],
            contract: vec!["configs/gateway/public".to_string()],
            backup: Vec::new(),
        };
        assert_eq!(
            conf.protection_paths(),
            vec!["configs".to_string(), "configs/gateway/public".to_string()]
        );
    }

    #[test]
    fn test_backup_paths_always_includes_contract() {
        // 不声明 `backup`，contract 也应被收（必收）。
        let conf = SysConf {
            test_envs: DependencySet::default(),
            ignore: Vec::new(),
            protect: Vec::new(),
            contract: vec!["configs/gateway/public".to_string()],
            backup: Vec::new(),
        };
        assert_eq!(
            conf.backup_paths(),
            vec!["configs/gateway/public".to_string()]
        );

        // contract 已在 `backup` 里时不重复（去重）。
        let conf2 = SysConf {
            test_envs: DependencySet::default(),
            ignore: Vec::new(),
            protect: Vec::new(),
            contract: vec!["configs/gateway/public".to_string()],
            backup: vec!["configs/gateway/public".to_string()],
        };
        assert_eq!(
            conf2.backup_paths(),
            vec!["configs/gateway/public".to_string()]
        );
    }

    #[test]
    fn test_sys_conf_reads_legacy_preserve_alias() {
        // 旧键 `preserve:` 仍可读，落到同一字段；新写一律 `protect`。
        let conf: SysConf = serde_yaml::from_str(
            "test_envs:\n  dep_root: ''\n  deps: []\npreserve:\n  - configs\n",
        )
        .unwrap();
        assert_eq!(conf.protect(), &["configs".to_string()][..]);
    }

    #[test]
    fn test_example_docker_compose_sys_prj_parses() {
        // 示例 `example/knowlege/docker-compose/sys-prj.yml` 的声明应能解析出来。
        let path = std::path::Path::new("example/knowlege/docker-compose/sys-prj.yml");
        let conf = SysConf::load_conf(path).unwrap();
        assert!(conf.ignore().iter().any(|p| p == ".env"));
        assert!(conf.protect().iter().any(|p| p == "web.conf"));
        assert_eq!(conf.contract(), &["configs".to_string()][..]);
        assert!(conf.backup().iter().any(|p| p == ".env"));
        // contract 语义自足：自动进保护集与备份选取
        assert!(conf.protection_paths().iter().any(|p| p == "configs"));
        assert!(conf.backup_paths().iter().any(|p| p == "configs"));
    }

    #[test]
    fn test_sys_conf_contract_roundtrip_and_omitted_when_empty() {
        // 空 → 不写入 contract 节
        let empty = SysConf::new(DependencySet::default());
        let yaml = serde_yaml::to_string(&empty).unwrap();
        assert!(!yaml.contains("contract"), "yaml={yaml}");

        let with = SysConf {
            test_envs: DependencySet::default(),
            ignore: Vec::new(),
            protect: Vec::new(),
            contract: vec!["configs/gateway/public".to_string()],
            backup: Vec::new(),
        };
        let yaml = serde_yaml::to_string(&with).unwrap();
        assert!(yaml.contains("contract"), "yaml={yaml}");
        let back: SysConf = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.contract(), &["configs/gateway/public".to_string()][..]);
    }

    #[test]
    fn test_sys_conf_load_without_ignore_field() {
        // 旧 sys-prj.yml 无 ignore/protect/contract/backup 节 → 默认空列表
        let conf: SysConf =
            serde_yaml::from_str("test_envs:\n  dep_root: ''\n  deps: []\n").unwrap();
        assert!(conf.ignore().is_empty());
        assert!(conf.protect().is_empty());
        assert!(conf.contract().is_empty());
        assert!(conf.backup().is_empty());
    }

    #[test]
    fn test_backup_roundtrip_and_omitted_when_empty() {
        // 空 → 不写入 backup 节
        let empty = SysConf::new(DependencySet::default());
        let yaml = serde_yaml::to_string(&empty).unwrap();
        assert!(!yaml.contains("backup"), "yaml={yaml}");

        // 非空 → 扁平列表，顺序保留
        let yaml = "test_envs:\n  dep_root: ''\n  deps: []\nbackup:\n  - configs/gateway/state/*.pem\n  - packages\n";
        let conf: SysConf = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            conf.backup(),
            &[
                "configs/gateway/state/*.pem".to_string(),
                "packages".to_string()
            ][..]
        );
        assert!(!conf.backup().is_empty());
    }
}
