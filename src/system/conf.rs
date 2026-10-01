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
    /// 与 `ignore` 的分工：`ignore` 管“不进交付包”，`preserve` 管“升级不许碰”。
    /// 两者**不相等**（`.env`、`values/`、`deliver.lock` 都不进包，但都属于现场态），
    /// 但应满足不变式 `ignore ⊆ preserve` —— 不进包的按定义就不是包内容。
    /// 不一致由 `gops prj diagnose` 报出。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    preserve: Vec<String>,
    /// 备份选取分级（glob，相对系统根）。见 [`BackupConf`]。
    #[serde(default, skip_serializing_if = "BackupConf::is_empty")]
    backup: BackupConf,
}

/// 备份选取分级：只表达“丢失的代价”，**不触发任何删除**。
///
/// - `restore`：丢了要重装 / 换身份 → `prj backup` 默认**收**，`prj restore` 放回。
/// - `rebuild`：可重建（重新投放/生成即可）→ 默认**不收**，需显式 `--include-rebuild`，
///   收它的价值只是省一次重投。
///
/// 不在两档里的路径 = 不收、不碰（运行期巨量日志、`data-plane-run/` 之类）。
#[derive(Getters, Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[getset(get = "pub")]
pub struct BackupConf {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    restore: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rebuild: Vec<String>,
}

impl BackupConf {
    pub fn is_empty(&self) -> bool {
        self.restore.is_empty() && self.rebuild.is_empty()
    }

    /// 按级别取模式：`with_rebuild = true` 时把 `rebuild` 档也带上。
    pub fn patterns_for(&self, with_rebuild: bool) -> Vec<String> {
        let mut out = self.restore.clone();
        if with_rebuild {
            out.extend(self.rebuild.iter().cloned());
        }
        out
    }
}

impl SysConf {
    pub fn new(local_res: DependencySet) -> Self {
        Self {
            test_envs: local_res,
            ignore: Vec::new(),
            preserve: Vec::new(),
            backup: BackupConf::default(),
        }
    }

    /// `gops sys package` 打包时排除的路径模式（`sys-prj.yml` 的 `ignore:` 节）。
    /// 显式方法（非 `getset`）：`SysConf` 未加 `#[getset(get = "pub")]`，加后 `ignore()` 会与
    /// 派生同名 getter 冲突。
    pub fn ignore(&self) -> &[String] {
        &self.ignore
    }

    /// 现场态路径（升级不覆盖、不删除）。见字段注释里的不变式。
    pub fn preserve(&self) -> &[String] {
        &self.preserve
    }

    /// 备份选取分级。
    pub fn backup(&self) -> &BackupConf {
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
            preserve: Vec::new(),
            backup: BackupConf::default(),
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
    fn test_sys_conf_preserve_roundtrip_and_omitted_when_empty() {
        // 空 → 不写入 preserve 节
        let empty = SysConf::new(DependencySet::default());
        let yaml = serde_yaml::to_string(&empty).unwrap();
        assert!(!yaml.contains("preserve"), "yaml={yaml}");

        let with = SysConf {
            test_envs: DependencySet::default(),
            ignore: vec!["configs".to_string()],
            preserve: vec!["configs".to_string(), ".env".to_string()],
            backup: BackupConf::default(),
        };
        let yaml = serde_yaml::to_string(&with).unwrap();
        assert!(yaml.contains("preserve"), "yaml={yaml}");
        let back: SysConf = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(
            back.preserve(),
            &["configs".to_string(), ".env".to_string()][..]
        );
    }

    #[test]
    fn test_sys_conf_load_without_ignore_field() {
        // 旧 sys-prj.yml 无 ignore/preserve 节 → 默认空列表
        let conf: SysConf =
            serde_yaml::from_str("test_envs:\n  dep_root: ''\n  deps: []\n").unwrap();
        assert!(conf.ignore().is_empty());
        assert!(conf.preserve().is_empty());
        assert!(conf.backup().is_empty());
    }

    #[test]
    fn test_backup_conf_roundtrip_and_omitted_when_empty() {
        let empty = SysConf::new(DependencySet::default());
        let yaml = serde_yaml::to_string(&empty).unwrap();
        assert!(!yaml.contains("backup"), "yaml={yaml}");

        let yaml = "test_envs:\n  dep_root: ''\n  deps: []\nbackup:\n  restore:\n    - configs/gateway/state/*.pem\n  rebuild:\n    - packages\n";
        let conf: SysConf = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            conf.backup().restore(),
            &["configs/gateway/state/*.pem".to_string()]
        );
        assert_eq!(conf.backup().rebuild(), &["packages".to_string()]);
        assert!(!conf.backup().is_empty());
        // 默认只收 restore 档
        assert_eq!(
            conf.backup().patterns_for(false),
            vec!["configs/gateway/state/*.pem".to_string()]
        );
        assert_eq!(conf.backup().patterns_for(true).len(), 2);
    }
}
