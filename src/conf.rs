use crate::error::MainReason;
use crate::internal_prelude::*;

use crate::{
    const_vars::CONFS_DIR,
    types::{Accessor, RefUpdateable},
};
use orion_variate::{
    addr::{Address, accessor::path_file_name},
    types::{ResourceDownloader, UpdateUnit},
};
// 由于 `crate::tools::log_flag` 未定义，移除该导入
#[derive(Clone, Debug, Getters, Deserialize, Serialize)]
#[getset(get = "pub")]
pub struct ConfSpec {
    version: String,
    #[serde(default = "default_local_root")]
    local_root: String,
    files: Vec<ConfFile>,
}
fn default_local_root() -> String {
    CONFS_DIR.to_string()
}

#[derive(Clone, Debug, Getters, Deserialize, Serialize)]
#[getset(get = "pub")]
pub struct ConfFile {
    path: String,
    addr: Option<Address>,
}

#[derive(Getters, Clone, Debug, Serialize)]
pub struct ConfSpecRef {
    path: String,
    #[serde(skip_serializing)] // 序列化时跳过
    obj: ConfSpec,
}

impl ConfSpecRef {
    pub fn files(&self) -> &Vec<ConfFile> {
        self.obj.files()
    }
}

impl<'de> serde::Deserialize<'de> for ConfSpecRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // 定义临时结构体用于反序列化
        #[derive(Deserialize)]
        struct RawRef {
            path: String,
        }
        // 先执行标准反序列化
        let raw = RawRef::deserialize(deserializer)?;
        // 构建实例
        let config = ConfSpecRef {
            obj: ConfSpecRef::load_ref(raw.path.as_str()).unwrap(),
            path: raw.path,
        };
        Ok(config)
    }
}

impl ConfSpecRef {
    pub fn new<S: Into<String>>(path: S) -> MainResult<Self> {
        let path = path.into();
        let file_path = PathBuf::from(path.as_str());
        let obj = ConfSpec::load_conf(&file_path).source_conf()?;
        Ok(Self { path, obj })
    }
    fn load_ref(path: &str) -> MainResult<ConfSpec> {
        let path = PathBuf::from(path);
        ConfSpec::load_conf(&path).source_conf()
    }
}

impl ConfFile {
    pub fn new<S: Into<String>>(path: S) -> Self {
        Self {
            path: path.into(),
            addr: None,
        }
    }
    pub fn with_addr<A: Into<Address>>(mut self, addr: A) -> Self {
        self.addr = Some(addr.into());
        self
    }
}
impl ConfSpec {
    pub fn save(&self, path: &PathBuf) -> MainResult<()> {
        let mut ctx = WithContext::want("save conf spec");
        ctx.record("path", format!("path: {}", path.display()));
        let data_content = toml::to_string(self).source_data().with(&ctx)?;
        fs::write(path, data_content).source_resource().with(&ctx)?;
        Ok(())
    }

    pub fn new<S: Into<String>>(version: S, local_root: S) -> Self {
        Self {
            version: version.into(),
            local_root: local_root.into(),
            files: Vec::new(),
        }
    }
    pub fn add(&mut self, file: ConfFile) {
        self.files.push(file);
    }
    pub fn default_from_files(values: Vec<&str>) -> Self {
        let mut ins = ConfSpec::new("1.0", CONFS_DIR);
        for item in values {
            ins.add(ConfFile::new(item));
        }
        ins
    }
}

#[async_trait]
impl RefUpdateable<UpdateUnit> for ConfSpec {
    async fn update_local(
        &self,
        accessor: Accessor,
        path: &Path,
        options: &DownloadOptions,
    ) -> MainResult<UpdateUnit> {
        let mut ctx = OperationContext::want("upload local confspec")
            .with_auto_log()
            .with_mod_path("spec/conf");
        ctx.record("path", path.display());
        ctx.debug("begin");

        let root = path.join(self.local_root());
        std::fs::create_dir_all(&root).source_resource()?;
        for f in &self.files {
            if let Some(addr) = f.addr() {
                let filename = path_file_name(&PathBuf::from(f.path.as_str())).err_conv()?;

                let x = accessor
                    .download_rename(addr, &root, filename.as_str(), options)
                    .await
                    .map_err(MainReason::from_addr_error)?;
                ctx.mark_suc();
                return Ok(x);
            }
        }
        Ok(UpdateUnit::from(root))
    }
}

#[cfg(test)]
mod tests {
    use crate::accessor::accessor_for_test;

    use super::*;
    use mockito::Server;
    use orion_error::dev::testing::TestAssert;
    use orion_variate::{
        addr::{HttpResource, LocalPath},
        tools::test_init,
    };
    use tokio::fs;

    #[test]
    fn test_conf_spec_new() {
        let spec = ConfSpec::new("1.0", CONFS_DIR);
        assert_eq!(spec.version(), "1.0");
        assert!(spec.files().is_empty());
    }

    #[test]
    fn test_conf_file_creation() {
        let file = ConfFile::new("config.yml");
        assert_eq!(file.path(), "config.yml");
        assert!(file.addr().is_none());

        let with_addr = file.with_addr(Address::Local(LocalPath::from("/tmp")));
        assert!(with_addr.addr().is_some());
    }
    #[tokio::test]
    async fn test_async_update() -> MainResult<()> {
        test_init();
        let src_dir = PathBuf::from("./temp/src");
        let dst_dir = PathBuf::from("./temp/dst");

        // 创建带地址的配置
        let mut spec = ConfSpec::new("3.0", CONFS_DIR);
        spec.add(
            ConfFile::new("db.yml").with_addr(Address::Local(LocalPath::from("./temp/src/db.yml"))),
        );

        // 模拟本地文件

        fs::create_dir_all(&src_dir).await.source_resource()?;
        fs::create_dir_all(&dst_dir).await.source_resource()?;
        fs::write(src_dir.join("db.yml"), "[database]\nurl=\"localhost\"")
            .await
            .source_resource()?;

        let accessor = accessor_for_test();
        // 执行更新
        let _ = spec
            .update_local(accessor, &dst_dir, &DownloadOptions::for_test())
            .await
            .source_logic()?;
        assert!(dst_dir.join("confs/db.yml").exists());

        // 清理
        fs::remove_dir_all(dst_dir).await.source_resource()?;
        fs::remove_dir_all(src_dir).await.source_resource()?;
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    /// Test HTTP configuration loading with mock HTTP server
    /// This test verifies the async update_local functionality with HTTP resources
    /// using mockito 1.7 for HTTP mocking
    async fn test_conf_with_http_addr() -> MainResult<()> {
        // 使用 mockito 1.7 的异步兼容API - 通过线程避免 runtime 嵌套
        let server = std::thread::spawn(|| {
            let mut server = Server::new();
            let _mock = server
                .mock("GET", "/global.yml")
                .with_status(200)
                .with_body("[settings]\nenv=\"test\"")
                .create();
            server
        })
        .join()
        .expect("Failed to create mock server");

        // 创建包含HttpResource的配置
        let mut conf = ConfSpec::new("1.0", CONFS_DIR);
        conf.add(
            ConfFile::new("remote.yml").with_addr(HttpResource::from(server.url() + "/global.yml")),
        );

        // 测试更新
        //let src_dir = PathBuf::from("./temp/src");
        //let dst_dir = PathBuf::from("./temp/dst");
        //let temp_dir = temp_dir();
        let temp_dir = PathBuf::from("./test_data/temp/http");
        if temp_dir.exists() {
            std::fs::remove_dir_all(&temp_dir).assert();
        }
        std::fs::create_dir_all(&temp_dir).assert();

        let accessor = accessor_for_test();
        let updated_v = conf
            .update_local(accessor, &temp_dir, &DownloadOptions::for_test())
            .await
            .assert();

        assert_eq!(
            updated_v.position(),
            &temp_dir.join(CONFS_DIR).join("remote.yml")
        );
        // 验证下载的文件
        let content = fs::read_to_string(updated_v.position())
            .await
            .source_resource()
            .with(format!("path: {}", updated_v.position().display()))?;
        assert!(content.contains("env=\"test\""));
        //fs::remove_dir_all(dst_dir).await.owe_res()?;
        Ok(())
    }
    #[tokio::test(flavor = "current_thread")]
    async fn test_conf_with_addr_addr() -> MainResult<()> {
        let server = std::thread::spawn(|| {
            let mut server = Server::new();
            let _mock = server
                .mock("GET", "/bitnami")
                .with_status(200)
                .with_body("name=bitnami")
                .create();
            server
        })
        .join()
        .expect("Failed to create mock server");

        // 创建包含HttpResource的配置
        let mut conf = ConfSpec::new("1.0", CONFS_DIR);
        conf.add(ConfFile::new("bitnami").with_addr(HttpResource::from(server.url() + "/bitnami")));

        // 测试更新
        //let src_dir = PathBuf::from("./temp/src");
        //let dst_dir = PathBuf::from("./temp/dst");
        //let temp_dir = temp_dir();
        let temp_dir = PathBuf::from("./test_data/temp/conf_dst");
        if temp_dir.exists() {
            std::fs::remove_dir_all(&temp_dir).assert();
        }
        std::fs::create_dir_all(&temp_dir).assert();
        let accessor = accessor_for_test();
        let updated_v = conf
            .update_local(accessor, &temp_dir, &DownloadOptions::for_test())
            .await
            .assert();
        assert_eq!(
            updated_v.position(),
            &temp_dir.join(CONFS_DIR).join("bitnami")
        );
        let content = fs::read_to_string(updated_v.position())
            .await
            .source_resource()
            .with(format!("path: {}", updated_v.position().display()))?;
        assert_eq!(content, "name=bitnami");

        Ok(())
    }
}
