use crate::{
    error::MainReason,
    module::prelude::*,
    types::{Accessor, RefUpdateable},
};

use async_trait::async_trait;
use getset::Getters;
use orion_variate::{
    addr::{Address, GitRepository, LocalPath, types::PathTemplate},
    types::ResourceDownloader,
    update::DownloadOptions,
};

#[derive(Getters, Clone, Debug, Serialize, Deserialize)]
#[getset(get = "pub")]
pub struct Dependency {
    addr: Address,
    local: PathTemplate,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    rename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    enable: Option<bool>,
}

impl Dependency {
    pub fn new(addr: Address, local: PathTemplate) -> Self {
        Self {
            addr,
            local,
            rename: None,
            enable: None,
        }
    }
    pub fn with_rename<S: Into<String>>(mut self, name: S) -> Self {
        self.rename = Some(name.into());
        self
    }
}

#[async_trait]
impl RefUpdateable<()> for Dependency {
    async fn update_local(
        &self,
        accessor: Accessor,
        path: &Path,
        options: &DownloadOptions,
    ) -> MainResult<()> {
        let local = self.local().path(options.values());
        if local.is_absolute() {
            return Err(MainReason::conf_detail(format!(
                "dependency `local` must be relative to the owning directory, got absolute path: {}",
                local.display()
            )));
        }
        if has_parent_dir(&local) {
            return Err(MainReason::conf_detail(format!(
                "dependency `local` must not escape the owning directory via `..`, got: {}",
                local.display()
            )));
        }
        let path = path.join(local);
        // HTTP 下载**不会**为目标创建目录：目标位置的上级目录不存在时，底层会把
        // 「写半包文件失败」报成晦涩的 “configuration error << core config”，让人误以为
        // 是配置问题。这里提前给出可操作的提示。
        // （仅 HTTP 如此：git / local accessor 会自行 `create_dir_all` 目标目录。）
        if let Address::Http(http) = self.addr()
            && let Some(parent) = missing_parent_dir(&path)
        {
            return Err(MainReason::conf_detail(format!(
                "cannot download {} to {}: parent directory does not exist: {} \
                 (the dependency's `local`/`dep_root` is resolved relative to the owning \
                 directory)",
                http.url(),
                path.display(),
                parent.display()
            )));
        }
        if let Some(rename) = self.rename() {
            accessor
                .download_rename(self.addr(), &path, rename, options)
                .await
                .map_err(MainReason::from_addr_error)?;
        } else {
            accessor
                .download_to_local(self.addr(), &path, options)
                .await
                .map_err(MainReason::from_addr_error)?;
        }
        Ok(())
    }
}

/// 路径的**上级目录**不存在时返回它（相对所属实体目录解析后缺失）。
fn missing_parent_dir(path: &Path) -> Option<&Path> {
    let parent = path.parent()?;
    (!parent.as_os_str().is_empty() && !parent.exists()).then_some(parent)
}

/// 路径里是否含 `..`（相对逃逸）。
fn has_parent_dir(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

#[derive(Getters, Clone, Debug, Serialize, Deserialize, Default)]
#[getset(get = "pub")]
pub struct DependencySet {
    dep_root: PathTemplate,
    deps: Vec<Dependency>,
}

impl DependencySet {
    pub fn example() -> Self {
        let depends = vec![
            Dependency {
                addr: Address::from(LocalPath::from("./example/data")),
                local: PathTemplate::from("env_res".to_string()),
                rename: Some("mysql2".to_string()),
                enable: Some(false),
            },
            Dependency {
                addr: Address::from(GitRepository::from("https://github.com/xxx")),
                local: PathTemplate::from("env_res".to_string()),
                rename: Some("mylib".to_string()),
                enable: Some(false),
            },
        ];

        DependencySet {
            deps: depends,
            dep_root: PathTemplate::from("./depends".to_string()),
        }
    }
    pub fn for_test() -> Self {
        let depends = vec![Dependency {
            addr: Address::from(LocalPath::from("./example/knowlege/mysql")),
            local: PathTemplate::from("env_res".to_string()),
            rename: Some("mysql_x86".to_string()),
            enable: Some(true),
        }];

        DependencySet {
            deps: depends,
            dep_root: PathTemplate::from("./depends".to_string()),
        }
    }
    pub fn push(&mut self, item: Dependency) {
        self.deps.push(item);
    }
}
#[async_trait]
impl RefUpdateable<()> for DependencySet {
    async fn update_local(
        &self,
        accessor: Accessor,
        path: &Path,
        options: &DownloadOptions,
    ) -> MainResult<()> {
        // 依赖落点以**所属实体目录**（`path`）为基准，而不是进程 CWD：否则换个目录运行就会
        // 落到别处、多个实体还会互相覆盖。`dep_root` 必须是相对路径（绝对路径会顶替实体目录）。
        let dep_root = self.dep_root().path(options.values());
        if dep_root.is_absolute() {
            return Err(MainReason::conf_detail(format!(
                "dependency `dep_root` must be relative to the owning directory, got absolute path: {}",
                dep_root.display()
            )));
        }
        if has_parent_dir(&dep_root) {
            return Err(MainReason::conf_detail(format!(
                "dependency `dep_root` must not escape the owning directory via `..`, got: {}",
                dep_root.display()
            )));
        }
        let base = path.join(&dep_root);
        for dep in self.deps().iter() {
            if dep.is_enable() {
                dep.update_local(accessor.clone(), &base, options).await?;
            }
        }
        Ok(())
    }
}

impl Dependency {
    pub fn is_enable(&self) -> bool {
        self.enable.unwrap_or(true)
    }
}

#[cfg(test)]
pub mod tests {
    use std::path::PathBuf;

    use orion_error::dev::testing::TestAssertWithMsg;
    use orion_variate::{
        addr::{Address, LocalPath},
        update::DownloadOptions,
    };

    use crate::{
        accessor::accessor_for_test,
        module::depend::{Dependency, DependencySet, PathTemplate},
        types::RefUpdateable,
    };

    /// 取“配置错误”的 detail 文本（reason 应为 UVS `ConfigError`）。
    fn conf_detail(err: &orion_error::StructError<crate::error::MainReason>) -> String {
        use orion_error::UnifiedReason;
        match err.reason() {
            crate::error::MainReason::Uvs(UnifiedReason::ConfigError(_)) => {
                err.detail().clone().unwrap_or_default()
            }
            other => panic!("expected config error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_depend() {
        let prj_path = PathBuf::from("./test_data/temp/depend/");
        if prj_path.exists() {
            std::fs::remove_dir_all(&prj_path).assert("remove dir");
        }
        std::fs::create_dir_all(&prj_path).assert("create prj_path");
        let item = Dependency::new(
            Address::from(LocalPath::from("./example/knowlege/mysql")),
            PathTemplate::from("env_res".to_string()),
        )
        .with_rename("mysql2");
        let accessor = accessor_for_test();
        item.update_local(accessor, &prj_path, &DownloadOptions::for_test())
            .await
            .assert("update");
        assert!(prj_path.join("env_res").join("mysql2").exists())
    }

    #[tokio::test]
    async fn test_depend_set_lands_under_owning_dir_not_cwd() {
        // 依赖落点以**传入的实体目录**为基准，而不是进程 CWD。
        let owning = PathBuf::from("./test_data/temp/depend_owning");
        if owning.exists() {
            std::fs::remove_dir_all(&owning).assert("clean owning");
        }
        std::fs::create_dir_all(&owning).assert("create owning");
        let src = PathBuf::from("./test_data/temp/depend_src");
        if src.exists() {
            std::fs::remove_dir_all(&src).assert("clean src");
        }
        std::fs::create_dir_all(&src).assert("create src");
        std::fs::write(src.join("a.txt"), "x").assert("write src");

        let mut set = DependencySet::default();
        set.push(
            Dependency::new(
                Address::from(LocalPath::from(src.to_str().unwrap())),
                PathTemplate::from("payload".to_string()),
            )
            .with_rename("renamed"),
        );
        set.update_local(accessor_for_test(), &owning, &DownloadOptions::for_test())
            .await
            .assert("update");

        // 落在 owning 之下（而不是 CWD 下的 `./payload`）。
        assert!(
            owning.join("payload").join("renamed").exists(),
            "dependency should land under the owning dir: {}",
            owning.display()
        );
    }

    #[tokio::test]
    async fn test_depend_rejects_absolute_dep_root() {
        let owning = PathBuf::from("./test_data/temp/depend_abs_root");
        let set = DependencySet {
            dep_root: PathTemplate::from("/abs/depends".to_string()),
            deps: Vec::new(),
        };
        let err = set
            .update_local(accessor_for_test(), &owning, &DownloadOptions::for_test())
            .await
            .expect_err("absolute dep_root should be rejected");
        let msg = conf_detail(&err);
        assert!(msg.contains("dep_root"), "msg={msg}");
        assert!(msg.contains("absolute"), "msg={msg}");
    }

    #[tokio::test]
    async fn test_depend_rejects_absolute_local() {
        let owning = PathBuf::from("./test_data/temp/depend_abs_local");
        let mut set = DependencySet::default();
        set.push(
            Dependency::new(
                Address::from(LocalPath::from("./example/data")),
                PathTemplate::from("/abs/here".to_string()),
            )
            .with_rename("x"),
        );
        let err = set
            .update_local(accessor_for_test(), &owning, &DownloadOptions::for_test())
            .await
            .expect_err("absolute local should be rejected");
        let msg = conf_detail(&err);
        assert!(msg.contains("local"), "msg={msg}");
        assert!(msg.contains("absolute"), "msg={msg}");
    }

    #[tokio::test]
    async fn test_depend_rejects_parent_dir_escape() {
        // `..` 相对逃逸与绝对路径同类：不能写出所属实体目录。
        let owning = PathBuf::from("./test_data/temp/depend_escape");

        let mut set = DependencySet::default();
        set.push(
            Dependency::new(
                Address::from(LocalPath::from("./example/data")),
                PathTemplate::from("../outside"),
            )
            .with_rename("x"),
        );
        let err = set
            .update_local(accessor_for_test(), &owning, &DownloadOptions::for_test())
            .await
            .expect_err("`..` in local should be rejected");
        let msg = conf_detail(&err);
        assert!(msg.contains("local"), "msg={msg}");
        assert!(msg.contains(".."), "msg={msg}");

        let set2 = DependencySet {
            dep_root: PathTemplate::from("../up"),
            deps: Vec::new(),
        };
        let err2 = set2
            .update_local(accessor_for_test(), &owning, &DownloadOptions::for_test())
            .await
            .expect_err("`..` in dep_root should be rejected");
        assert!(conf_detail(&err2).contains("dep_root"));
    }

    #[tokio::test]
    async fn test_http_depend_missing_parent_dir_errors_clearly() {
        use orion_variate::addr::HttpResource;

        // 上级目录不存在时，HTTP 依赖应给出可操作的提示，
        // 而不是底层晦涩的 “configuration error << core config”。
        let base = PathBuf::from("./test_data/temp/depend_missing_parent");
        if base.exists() {
            std::fs::remove_dir_all(&base).assert("remove dir");
        }
        // 故意不创建 base —— 它就成为不存在的上级目录；此用例不涉及网络。
        let item = Dependency::new(
            Address::from(HttpResource::from("https://example.com/x.tar.gz")),
            PathTemplate::from("env_res".to_string()),
        )
        .with_rename("pkg");
        let accessor = accessor_for_test();
        let err = item
            .update_local(accessor, &base, &DownloadOptions::for_test())
            .await
            .expect_err("should fail when the parent directory is missing");

        let msg = conf_detail(&err);
        assert!(msg.contains("parent directory does not exist"), "msg={msg}");
        assert!(!msg.contains("core config"), "msg={msg}");
    }

    #[tokio::test]
    async fn test_depend_set_honors_dep_root_under_owning_dir() {
        // `dep_root` 相对**所属实体目录**拼接（不是 CWD）。
        let owning = PathBuf::from("./test_data/temp/depend_root_dir");
        if owning.exists() {
            std::fs::remove_dir_all(&owning).assert("clean owning");
        }
        std::fs::create_dir_all(&owning).assert("create owning");
        let src = PathBuf::from("./test_data/temp/depend_root_src");
        if src.exists() {
            std::fs::remove_dir_all(&src).assert("clean src");
        }
        std::fs::create_dir_all(&src).assert("create src");
        std::fs::write(src.join("a.txt"), "x").assert("write src");

        let mut set = DependencySet {
            dep_root: PathTemplate::from("./depends".to_string()),
            deps: Vec::new(),
        };
        set.push(
            Dependency::new(
                Address::from(LocalPath::from(src.to_str().unwrap())),
                PathTemplate::from("payload".to_string()),
            )
            .with_rename("renamed"),
        );
        set.update_local(accessor_for_test(), &owning, &DownloadOptions::for_test())
            .await
            .assert("update");

        assert!(
            owning
                .join("depends")
                .join("payload")
                .join("renamed")
                .exists(),
            "should land under <owning>/depends/payload"
        );
    }

    #[tokio::test]
    async fn test_depend_set_skips_disabled_deps() {
        let owning = PathBuf::from("./test_data/temp/depend_skip");
        if owning.exists() {
            std::fs::remove_dir_all(&owning).assert("clean");
        }
        std::fs::create_dir_all(&owning).assert("create");

        let mut set = DependencySet::default();
        set.push(Dependency {
            addr: Address::from(LocalPath::from("./example/data")),
            local: PathTemplate::from("payload".to_string()),
            rename: None,
            enable: Some(false),
        });
        set.update_local(accessor_for_test(), &owning, &DownloadOptions::for_test())
            .await
            .assert("update");
        assert!(
            !owning.join("payload").exists(),
            "disabled dep must be skipped"
        );
    }

    #[test]
    fn test_serialize_to_yaml() {
        let item = Dependency {
            addr: Address::from(LocalPath::from("./example/knowlege/mysql")),
            local: PathTemplate::from("env_res".to_string()),
            rename: Some("mysql2".to_string()),
            enable: Some(true),
        };

        let vec = DependencySet {
            deps: vec![item.clone(), item],
            dep_root: PathTemplate::from("./".to_string()),
        };
        let yaml_vec = serde_yaml::to_string(&vec).unwrap();
        println!("{yaml_vec:#}",);
        assert!(yaml_vec.contains("- addr:"));
        assert!(yaml_vec.contains("rename: mysql2"));
    }

    #[test]
    fn test_dependency_builder_pattern() {
        let dep = Dependency::new(
            Address::from(LocalPath::from("./example/data")),
            PathTemplate::from("env_res".to_string()),
        )
        .with_rename("test_dep");

        assert_eq!(dep.rename().as_ref().unwrap(), &"test_dep");
        // Check that the address is not empty (basic validation)
        assert!(!dep.addr().to_string().is_empty());
        assert_eq!(
            dep.local().path(&Default::default()).to_str(),
            Some("env_res")
        );
    }

    #[test]
    fn test_dependency_enable_flag() {
        let dep_enabled = Dependency {
            addr: Address::from(LocalPath::from("./example/data")),
            local: PathTemplate::from("env_res".to_string()),
            rename: None,
            enable: Some(true),
        };

        let dep_disabled = Dependency {
            addr: Address::from(LocalPath::from("./example/data")),
            local: PathTemplate::from("env_res".to_string()),
            rename: None,
            enable: Some(false),
        };

        let dep_default = Dependency::new(
            Address::from(LocalPath::from("./example/data")),
            PathTemplate::from("env_res".to_string()),
        );

        assert!(dep_enabled.is_enable());
        assert!(!dep_disabled.is_enable());
        assert!(dep_default.is_enable()); // Should default to true
    }
}
