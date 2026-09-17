use crate::internal_prelude::*;
use crate::types::Accessor;
use orion_variate::addr::AddrResult;
use orion_variate::types::{ResourceDownloader, UpdateUnit};

#[derive(Getters, Clone, Debug, Deserialize, Serialize, Setters, WithSetters)]
#[getset(get = "pub")]
pub struct Artifact {
    name: String,
    version: String,
    #[serde(alias = "addr")]
    origin_addr: Address,
    #[getset(set_with = "pub", set = "pub")]
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_addr: Option<Address>,
    #[getset(set_with = "pub", set = "pub")]
    #[serde(default = "default_cache_enable")]
    cache_enable: bool,
    local: String,
}
fn default_cache_enable() -> bool {
    false
}

impl Artifact {
    pub fn new<S: Into<String>, A: Into<Address>>(name: S, version: S, addr: A, local: S) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            origin_addr: addr.into(),
            cache_addr: None,
            cache_enable: false,
            local: local.into(),
        }
    }

    // 直接从远程仓库下载
    pub async fn deploy_repo_to_local(
        &self,
        accessor: Accessor,
        dest_path: &Path,
        options: &DownloadOptions,
    ) -> AddrResult<UpdateUnit> {
        std::fs::create_dir_all(dest_path).source_resource()?;
        let result = accessor
            .download_rename(self.origin_addr(), dest_path, &self.name, options)
            .await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use home::home_dir;

    use orion_variate::addr::{GitRepository, HttpResource};

    use crate::accessor::accessor_for_test;

    use super::*;

    #[ignore = "not run in ci"]
    #[tokio::test]
    async fn test_http_artifact_v1() -> AddrResult<()> {
        let artifact = Artifact::new(
            "hello-word",
            "0.1.0",
            HttpResource::from("https://github.com/galaxio-labs/hello-word.git"),
            "hello-word",
        );
        let path = home_dir()
            .unwrap_or("UNKOWN".into())
            .join(".cache")
            .join("v1");
        let accessor = accessor_for_test();
        artifact
            .deploy_repo_to_local(accessor, &path, &DownloadOptions::default())
            .await?;

        assert!(path.join("hello-word").exists());
        Ok(())
    }

    #[ignore = "not run in ci"]
    #[tokio::test]
    async fn test_http_artifact_v2() -> AddrResult<()> {
        let cache_addr = Address::Http(HttpResource::from(
            "https://dy-sec-generic.pkg.coding.net/galaxy-open/generic/galaxy-init.sh?version=latest",
        ));
        let deploy_type = Address::Git(
            GitRepository::from("git@github.com:galaxio-labs/spec_test.git").with_branch("main"),
        );
        let _artifact = Artifact {
            name: "galaxy-init".to_string(),
            version: "0.1.0".to_string(),
            origin_addr: deploy_type,
            cache_addr: Some(cache_addr),
            cache_enable: false,
            local: "galaxy-init".to_string(),
        }
        .with_cache_enable(true);
        Ok(())
    }

    #[tokio::test]
    async fn test_artifact_with_cache_enabled() {
        let artifact = Artifact::new(
            "test-artifact",
            "1.0.0",
            HttpResource::from("https://example.com/repo"),
            "test-artifact",
        );

        // Test with cache enabled
        let artifact_with_cache = artifact.clone().with_cache_enable(true);
        assert!(artifact_with_cache.cache_enable());

        // Test with cache disabled
        let artifact_without_cache = artifact.clone().with_cache_enable(false);
        assert!(!artifact_without_cache.cache_enable());
    }

    #[tokio::test]
    async fn test_artifact_with_cache_addr() {
        let cache_addr = Address::Http(HttpResource::from("https://example.com/cache.zip"));

        let artifact = Artifact {
            name: "cached-artifact".to_string(),
            version: "3.0.0".to_string(),
            origin_addr: HttpResource::from("https://github.com/example/main.git").into(),
            cache_addr: Some(cache_addr),
            cache_enable: true,
            local: "cached-artifact-local".to_string(),
        };

        assert!(artifact.cache_addr.is_some());
        assert!(artifact.cache_enable);

        if let Some(addr) = artifact.cache_addr {
            match addr {
                Address::Http(http_addr) => {
                    // Use Debug trait to compare HTTP resources
                    let addr_debug = format!("{http_addr:?}");
                    assert!(addr_debug.contains("example.com"));
                }
                _ => panic!("Expected HTTP address"),
            }
        }
    }

    #[tokio::test]
    async fn test_artifact_basic_properties() {
        let artifact = Artifact::new(
            "test-artifact",
            "1.0.0",
            HttpResource::from("https://example.com/repo"),
            "test-artifact-local",
        );

        assert_eq!(artifact.name, "test-artifact");
        assert_eq!(artifact.version, "1.0.0");
        assert_eq!(artifact.local, "test-artifact-local");
        assert!(!artifact.cache_enable);
        assert!(artifact.cache_addr.is_none());
    }
}
