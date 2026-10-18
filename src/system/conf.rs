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
    /// **打包纳入**（glob，相对系统根）：在默认的“git 入库文件”（或 `--full` 的整目录）之外
    /// **额外补入**的路径——用于让**未被 git 跟踪**的生成物 / 制品随包走（可覆盖 `.gitignore`）。
    /// `--full` 已“全收”，故它主要作用于默认模式。空则不写入。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pkg_include: Vec<String>,
    /// `gops sys package` 打包时排除的路径（glob，相对系统根）。
    /// 两种模式（默认 / `--full`）都生效；空则不写入。
    /// 旧名 `ignore` 仍可读（`#[serde(alias)]`），新写一律用 `pkg_exclude`。
    /// 与 `pkg_include` 相交时**以排除为准**（`pkg_exclude` 胜出）。
    #[serde(default, skip_serializing_if = "Vec::is_empty", alias = "ignore")]
    pkg_exclude: Vec<String>,
    /// **升级保护**（glob，相对系统根）：`gops sys resolve` / `prj sync` / `prj upgrade`
    /// **不覆盖、也不删**的路径。与 `pkg_include` / `pkg_exclude` / `contract` / `backup` **互相独立**，各自管各的。
    ///
    /// 旧名 `preserve` 仍可读（`#[serde(alias)]`），新写一律用 `protect`。
    #[serde(default, skip_serializing_if = "Vec::is_empty", alias = "preserve")]
    protect: Vec<String>,
    /// **对外契约**：承载“不可重建的公开现场事实”（对外域名 / 端点 / 证书 / 身份材料）的文件或目录
    /// （glob，相对系统根）。由 **`gops prj contract export` / `import`** 归档与回填。
    /// 与 `pkg_include` / `pkg_exclude` / `protect` / `backup` **互相独立**：声明它**不**隐式影响打包/覆盖/备份。
    /// 内容 site-owned；`sys-prj.yml` 只指路。见 `docs/design/contract.md`。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    contract: Vec<String>,
    /// **备份选取**（glob，相对系统根）：`gops prj backup` 收集的现场态（丢了要重装 / 换身份）。
    /// 只表达“收什么”，不触发任何删除；不在这里的 = 不收、不碰。与其它声明**互相独立**。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    backup: Vec<String>,
    /// **宿主权限对齐**（`gops sys perms`）：从 compose 派生 bind 挂载的属主/权限时使用的覆盖项。
    /// 缺省 = 全部按 compose 的 `user:` / `source` / `read_only` 派生。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    perms: Option<PermsConf>,
}

/// `sys-prj.yml: perms` —— 派生不出容器身份，或容器内 uid 与宿主 uid 不等价
/// （rootless / userns-remap）时的显式覆盖。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PermsConf {
    /// 宿主上应属容器的 uid（userns-remap 后是映射后的宿主 uid）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    container_uid: Option<u32>,
    /// 宿主上应属容器的 gid。必须与 `container_uid` 同时配置。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    container_gid: Option<u32>,
    /// 允许处理系统根之外的 bind `source`（绝对路径或相对系统根的路径）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    allow_external: Vec<String>,
}

impl PermsConf {
    pub fn container_uid(&self) -> Option<u32> {
        self.container_uid
    }
    pub fn container_gid(&self) -> Option<u32> {
        self.container_gid
    }
    pub fn allow_external(&self) -> &[String] {
        &self.allow_external
    }
}

impl SysConf {
    pub fn new(local_res: DependencySet) -> Self {
        Self {
            test_envs: local_res,
            pkg_include: Vec::new(),
            pkg_exclude: Vec::new(),
            protect: Vec::new(),
            contract: Vec::new(),
            backup: Vec::new(),
            perms: None,
        }
    }

    /// 设置现场态声明（builder；供示例 / 测试构造带声明的 `SysConf`）。
    pub fn with_site_state(
        mut self,
        pkg_include: Vec<String>,
        pkg_exclude: Vec<String>,
        protect: Vec<String>,
        contract: Vec<String>,
        backup: Vec<String>,
    ) -> Self {
        self.pkg_include = pkg_include;
        self.pkg_exclude = pkg_exclude;
        self.protect = protect;
        self.contract = contract;
        self.backup = backup;
        self
    }

    /// `gops sys package` 打包时**额外纳入**的路径模式（`sys-prj.yml` 的 `pkg_include:` 节）。
    pub fn pkg_include(&self) -> &[String] {
        &self.pkg_include
    }

    /// `gops sys package` 打包时排除的路径模式（`sys-prj.yml` 的 `pkg_exclude:` 节）。
    /// 显式方法（非 `getset`）：`SysConf` 未加 `#[getset(get = "pub")]`，加后 `pkg_exclude()` 会与
    /// 派生同名 getter 冲突。
    pub fn pkg_exclude(&self) -> &[String] {
        &self.pkg_exclude
    }

    /// 升级保护路径（`sys resolve` / `prj sync` / `prj upgrade` 不覆盖、不删除）。
    pub fn protect(&self) -> &[String] {
        &self.protect
    }

    /// 对外契约路径（不可重建的公开现场事实，走 `prj contract export/import`）。
    pub fn contract(&self) -> &[String] {
        &self.contract
    }

    /// 备份选取（`sys-prj.yml` 的 `backup:` 节）。与 `contract` 等**互相独立**。
    ///
    /// 只由本列表决定收什么；不在其中的不收、不碰。
    pub fn backup(&self) -> &[String] {
        &self.backup
    }

    /// 宿主权限对齐的覆盖项（`sys-prj.yml` 的 `perms:` 节）；缺省 = 纯 compose 派生。
    pub fn perms(&self) -> Option<&PermsConf> {
        self.perms.as_ref()
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
    fn test_sys_conf_pkg_exclude_roundtrip_and_omitted_when_empty() {
        // 空 → 不写入 pkg_exclude 节（不污染现有 sys-prj.yml）
        let empty = SysConf::new(DependencySet::default());
        let yaml = serde_yaml::to_string(&empty).unwrap();
        assert!(!yaml.contains("pkg_exclude"), "yaml={yaml}");

        // 非空 → 写入并可回读
        let with = SysConf {
            test_envs: DependencySet::default(),
            pkg_include: Vec::new(),
            pkg_exclude: vec!["sys/*/mods".to_string(), ".env".to_string()],
            protect: Vec::new(),
            contract: Vec::new(),
            backup: Vec::new(),
            perms: None,
        };
        let yaml = serde_yaml::to_string(&with).unwrap();
        assert!(yaml.contains("pkg_exclude"), "yaml={yaml}");
        let back: SysConf = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(
            back.pkg_exclude(),
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
            pkg_include: Vec::new(),
            pkg_exclude: vec!["configs".to_string()],
            protect: vec!["configs".to_string(), ".env".to_string()],
            contract: Vec::new(),
            backup: Vec::new(),
            perms: None,
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
    fn test_backup_is_independent_of_contract() {
        // 声明 contract 但未声明 backup → 备份选取为空（契约走独立归档，**不进** backup）。
        let conf = SysConf {
            test_envs: DependencySet::default(),
            pkg_include: Vec::new(),
            pkg_exclude: Vec::new(),
            protect: Vec::new(),
            contract: vec!["configs/gateway/public".to_string()],
            backup: Vec::new(),
            perms: None,
        };
        assert!(conf.backup().is_empty(), "contract 不应被并入 backup");

        // backup 与 contract 各归各，互不并入。
        let conf2 = SysConf {
            test_envs: DependencySet::default(),
            pkg_include: Vec::new(),
            pkg_exclude: Vec::new(),
            protect: Vec::new(),
            contract: vec!["configs/gateway/public".to_string()],
            backup: vec![".wist".to_string()],
            perms: None,
        };
        assert_eq!(conf2.backup(), &[".wist".to_string()][..]);
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
    fn test_sys_conf_reads_legacy_ignore_alias() {
        // 旧键 `ignore:` 仍可读，落到同一字段；新写一律 `pkg_exclude`。
        let conf: SysConf = serde_yaml::from_str(
            "test_envs:\n  dep_root: ''\n  deps: []\nignore:\n  - sys/*/mods\n",
        )
        .unwrap();
        assert_eq!(conf.pkg_exclude(), &["sys/*/mods".to_string()][..]);
    }

    #[test]
    fn test_sys_conf_pkg_include_roundtrip_and_omitted_when_empty() {
        // 空 → 不写入 pkg_include 节
        let empty = SysConf::new(DependencySet::default());
        let yaml = serde_yaml::to_string(&empty).unwrap();
        assert!(!yaml.contains("pkg_include"), "yaml={yaml}");

        // 非空 → 写入并可回读
        let with = SysConf {
            test_envs: DependencySet::default(),
            pkg_include: vec!["artifacts/app.bin".to_string()],
            pkg_exclude: Vec::new(),
            protect: Vec::new(),
            contract: Vec::new(),
            backup: Vec::new(),
            perms: None,
        };
        let yaml = serde_yaml::to_string(&with).unwrap();
        assert!(yaml.contains("pkg_include"), "yaml={yaml}");
        let back: SysConf = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.pkg_include(), &["artifacts/app.bin".to_string()][..]);
    }

    #[test]
    fn test_example_docker_compose_sys_prj_parses() {
        // 示例 `example/knowlege/docker-compose/sys-prj.yml` 的声明应能解析出来。
        let path = std::path::Path::new("example/knowlege/docker-compose/sys-prj.yml");
        let conf = SysConf::load_conf(path).unwrap();
        assert!(conf.pkg_exclude().iter().any(|p| p == ".env"));
        assert!(conf.protect().iter().any(|p| p == "web.conf"));
        assert_eq!(conf.contract(), &["configs".to_string()][..]);
        assert!(conf.backup().iter().any(|p| p == ".env"));
        // 各声明互相独立：contract 不进 backup。
        assert!(!conf.backup().iter().any(|p| p == "configs"));
    }

    #[test]
    fn test_sys_conf_contract_roundtrip_and_omitted_when_empty() {
        // 空 → 不写入 contract 节
        let empty = SysConf::new(DependencySet::default());
        let yaml = serde_yaml::to_string(&empty).unwrap();
        assert!(!yaml.contains("contract"), "yaml={yaml}");

        let with = SysConf {
            test_envs: DependencySet::default(),
            pkg_include: Vec::new(),
            pkg_exclude: Vec::new(),
            protect: Vec::new(),
            contract: vec!["configs/gateway/public".to_string()],
            backup: Vec::new(),
            perms: None,
        };
        let yaml = serde_yaml::to_string(&with).unwrap();
        assert!(yaml.contains("contract"), "yaml={yaml}");
        let back: SysConf = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.contract(), &["configs/gateway/public".to_string()][..]);
    }

    #[test]
    fn test_sys_conf_load_without_pkg_exclude_field() {
        // 旧 sys-prj.yml 无 pkg_exclude/protect/contract/backup 节 → 默认空列表
        let conf: SysConf =
            serde_yaml::from_str("test_envs:\n  dep_root: ''\n  deps: []\n").unwrap();
        assert!(conf.pkg_exclude().is_empty());
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
