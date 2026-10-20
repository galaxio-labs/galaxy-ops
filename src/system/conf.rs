use super::prelude::*;

use std::collections::BTreeMap;

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
    /// **密钥文件**：`gops run` / `gops sec` 运行时从这里加载 `SEC_*` 环境变量。
    /// 只指路、不含值；路径受系统根约束（见 `system::sec` 的安全校验）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secrets: Option<SecretsConf>,
}

/// `sys-prj.yml: secrets` —— 密钥文件声明。
///
/// **只声明路径，不声明变量名与值**：变量名由 `orion-sec` 的归一化规则自动派生
/// （大写 + `SEC_` 前缀，已是 `SEC_` 开头不重复加），所以 `center_admin_token`
/// 天然映射到 `${SEC_CENTER_ADMIN_TOKEN}`。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretsConf {
    /// 密钥文件路径：**必须是系统根内的相对路径**，不得含 `..`、不得是符号链接。
    file: String,
    /// 文件格式：`yaml`（默认）/ `toml`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    format: Option<SecFileFormat>,
    /// 注入范围（按消费方）。缺省策略见 [`ExposeConf::rule_for`]。
    ///
    /// 这里出现的只是**键名模式**（不是值）；用于落实最小权限：每个消费方只拿到自己需要的键。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expose: Option<ExposeConf>,
    /// 文件模式策略：缺省 `0600`（只属主可读）；与容器共享时设为 `0640`。
    ///
    /// 共享靠**属组**（部署账号:容器 gid 0640），不靠把属主改成容器 uid——
    /// 后者需要提权，gops 不做。
    #[serde(
        default,
        deserialize_with = "de_secret_mode",
        skip_serializing_if = "Option::is_none"
    )]
    mode: Option<String>,
}

/// `secrets.mode` 只接受**带引号的字符串**（`"0600"` / `"0640"`）。
///
/// YAML 里 `mode: 0640` 会被解析成整数（且容易被当成十进制 640），必须明确拒绝并提示加引号。
fn de_secret_mode<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Str(String),
        // 只为把“未加引号的整数”识别出来并给出可操作报错，值本身不用。
        #[allow(dead_code)]
        Int(i64),
    }
    match Option::<Raw>::deserialize(deserializer)? {
        None => Ok(None),
        Some(Raw::Str(s)) => Ok(Some(s)),
        Some(Raw::Int(_)) => Err(serde::de::Error::custom(
            "secrets.mode 必须是带引号的字符串：\"0600\"（默认）或 \"0640\"",
        )),
    }
}

impl SecretsConf {
    pub fn new(file: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            format: None,
            expose: None,
            mode: None,
        }
    }

    /// 声明的密钥文件（相对系统根）。空串是无效声明，加载时拒绝。
    pub fn file(&self) -> &str {
        &self.file
    }

    /// 文件格式；缺省 YAML。
    pub fn format(&self) -> SecFileFormat {
        self.format.unwrap_or_default()
    }

    /// 注入范围声明；缺省 = 各消费方自己的默认策略。
    pub fn expose(&self) -> Option<&ExposeConf> {
        self.expose.as_ref()
    }

    /// 解析后的文件模式：`0600`（默认）或 `0640`。
    ///
    /// 只允许这两种，避免把密钥误配成 `0644`。
    pub fn file_mode(&self) -> Result<u32, String> {
        match self.mode.as_deref().map(str::trim) {
            None | Some("") | Some("0600") | Some("600") => Ok(0o600),
            Some("0640") | Some("640") => Ok(0o640),
            Some(other) => Err(format!(
                "sys-prj.yml: secrets.mode = `{other}` 不支持；只允许 `0600`（默认）或 `0640`"
            )),
        }
    }
}

/// 消费方：决定“哪些密钥会被注入到这个入口”。
///
/// - `Run`：`gops run *` 拉起的 `docker compose` 子进程；
/// - `Localize`：`gops sys localize` 拉起的 `init-*.sh` 脚本；
/// - `Sec`：`gops sec run -- <cmd>` 显式执行。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subscriber {
    Run,
    Localize,
    Sec,
}

impl Subscriber {
    pub fn as_str(self) -> &'static str {
        match self {
            Subscriber::Run => "run",
            Subscriber::Localize => "localize",
            Subscriber::Sec => "sec",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("run") {
            Some(Subscriber::Run)
        } else if value.eq_ignore_ascii_case("localize") {
            Some(Subscriber::Localize)
        } else if value.eq_ignore_ascii_case("sec") {
            Some(Subscriber::Sec)
        } else {
            None
        }
    }

    /// 缺省策略：`run` / `sec` 全量（不改变现有行为）；`localize` **默认不注入**
    /// —— 新入口必须显式开权限（最小权限）。
    pub fn default_rule(self) -> ExposeRule {
        match self {
            Subscriber::Run | Subscriber::Sec => ExposeRule::All,
            Subscriber::Localize => ExposeRule::Only(Vec::new()),
        }
    }
}

/// `sys-prj.yml: secrets.expose` —— 按消费方的注入范围。
///
/// ```yaml
/// secrets:
///   file: .wist/sec-value.yml
///   expose:
///     run: all                      # 缺省 all
///     localize:                     # 缺省 none（必须显式开）
///       - SEC_CENTER_*
/// ```
///
/// 消费方名拼错（如 `loclize`）会在解析清单时直接报错，不会静默忽略。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExposeConf {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run: Option<ExposeRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    localize: Option<ExposeRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sec: Option<ExposeRule>,
}

impl ExposeConf {
    /// 取某个消费方的规则；未声明时用该消费方的缺省策略。
    pub fn rule_for(&self, subscriber: Subscriber) -> ExposeRule {
        let declared = match subscriber {
            Subscriber::Run => self.run.clone(),
            Subscriber::Localize => self.localize.clone(),
            Subscriber::Sec => self.sec.clone(),
        };
        declared.unwrap_or_else(|| subscriber.default_rule())
    }
}

/// 注入范围：`all` 或一组模式。
///
/// 模式只支持 `*`（任意序列，可匹配空串），匹配对象是**归一化后的环境变量名**
/// （`SEC_...`），大小写不敏感。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExposeRule {
    All,
    Only(Vec<String>),
}

impl<'de> serde::Deserialize<'de> for ExposeRule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Keyword(String),
            List(Vec<String>),
        }

        match Raw::deserialize(deserializer)? {
            Raw::Keyword(word) if word.eq_ignore_ascii_case("all") => Ok(ExposeRule::All),
            Raw::Keyword(word) => Err(serde::de::Error::custom(format!(
                "secrets.expose 只接受 `all` 或模式列表，收到 `{word}`"
            ))),
            Raw::List(patterns) => Ok(ExposeRule::Only(patterns)),
        }
    }
}

impl serde::Serialize for ExposeRule {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            ExposeRule::All => serializer.serialize_str("all"),
            ExposeRule::Only(patterns) => patterns.serialize(serializer),
        }
    }
}

/// 密钥文件格式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecFileFormat {
    #[default]
    Yaml,
    Toml,
}

impl From<SecFileFormat> for orion_sec::SecFileFmt {
    fn from(value: SecFileFormat) -> Self {
        match value {
            SecFileFormat::Yaml => orion_sec::SecFileFmt::Yaml,
            SecFileFormat::Toml => orion_sec::SecFileFmt::Toml,
        }
    }
}

/// `sys-prj.yml: perms.services.<service>` —— 单个服务的“有效容器身份”。
///
/// 典型用途：镜像 `Config.User` 为空（默认 root），但入口脚本会降权到非 root
/// （如官方 `postgres` 降权到 999 之后才读 `/docker-entrypoint-initdb.d`）。
/// **只影响 `gops sys perms` 的宿主属主/权限对齐，不改变容器运行时身份**；
/// 要改运行时请改 compose 的 `user:`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServicePerm {
    uid: u32,
    gid: u32,
}

impl ServicePerm {
    pub fn uid(&self) -> u32 {
        self.uid
    }
    pub fn gid(&self) -> u32 {
        self.gid
    }
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
    /// 按服务声明“有效容器身份”（仅用于对齐，不改运行时）。
    /// 全局 `container_uid/gid` 存在时本项被忽略（rootless 映射对所有服务一致）。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    services: BTreeMap<String, ServicePerm>,
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
    pub fn services(&self) -> &BTreeMap<String, ServicePerm> {
        &self.services
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
            secrets: None,
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
    /// 只由本列表决定收什么；不在其中的不收、不碰。
    pub fn backup(&self) -> &[String] {
        &self.backup
    }

    /// 宿主权限对齐的覆盖项（`sys-prj.yml` 的 `perms:` 节）；缺省 = 纯 compose 派生。
    pub fn perms(&self) -> Option<&PermsConf> {
        self.perms.as_ref()
    }

    /// 密钥文件声明（`sys-prj.yml` 的 `secrets:` 节）；缺省 = 未声明，走 `.galaxy` 旧约定。
    pub fn secrets(&self) -> Option<&SecretsConf> {
        self.secrets.as_ref()
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
            secrets: None,
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
            secrets: None,
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
            secrets: None,
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
            secrets: None,
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
            secrets: None,
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
            secrets: None,
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
