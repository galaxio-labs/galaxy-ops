use crate::internal_prelude::*;

use crate::{
    localize::exec::LocalizeExecPath,
    system::setting::Setting,
    types::{Accessor, LocalizeOptions, ModuleLocalizable, RefUpdateable},
};
use derive_more::Deref;

#[derive(Getters, Clone, Debug, Default, Serialize, Deserialize, Deref)]
#[serde(transparent)]
pub struct LocalizeSet {
    items: Vec<LocalizeExecPath>,
}

impl LocalizeSet {
    pub fn example() -> Self {
        Self {
            items: vec![
                LocalizeExecPath::new(
                    PathBuf::from("/opt/galaxy/templates/nginx.conf"),
                    PathBuf::from("/etc/nginx/nginx.conf"),
                    Setting::example(),
                ),
                LocalizeExecPath::simple_new(
                    PathBuf::from("/opt/galaxy/static/logo.png"),
                    PathBuf::from("/var/www/html/assets/logo.png"),
                ),
            ],
        }
    }
}

#[async_trait]
impl RefUpdateable<()> for LocalizeSet {
    async fn update_local(
        &self,
        _accessor: Accessor,
        _path: &Path,
        _options: &DownloadOptions,
    ) -> MainResult<()> {
        // For now, template paths are handled as local files
        Ok(())
    }
}

#[async_trait]
impl ModuleLocalizable<PathBuf> for LocalizeSet {
    async fn mod_localize(&self, val_path: PathBuf, options: LocalizeOptions) -> MainResult<()> {
        let mut flag = auto_exit_log!(
            info!(target: "sys-localize", "Localizing {} paths for sys_local", self.items.len()),
            error!(target: "sys-localize", "Failed to localize sys_local paths")
        );

        for item in &self.items {
            item.mod_localize(val_path.clone(), options.clone()).await?;
        }

        flag.mark_suc();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use orion_error::dev::testing::TestAssert;
    use orion_vars::vars::ValueDict;
    use tempfile::tempdir;

    use crate::{
        localize::{
            LocalizeSet,
            exec::{LocalizeExecPath, assert_file_content},
        },
        types::{LocalizeOptions, ModuleLocalizable},
    };

    // LocalizeSet 测试保持不变，但使用新的辅助函数
    #[tokio::test]
    async fn test_localize_set_multiple_files() {
        let temp_dir = tempdir().unwrap();

        // Create source files
        let file1 = temp_dir.path().join("source1.txt");
        let file2 = temp_dir.path().join("source2.txt");

        std::fs::write(&file1, "content1").unwrap();
        std::fs::write(&file2, "content2").unwrap();

        let localize_set = LocalizeSet {
            items: vec![
                LocalizeExecPath::simple_new(file1.clone(), temp_dir.path().join("dest1.txt")),
                LocalizeExecPath::simple_new(file2.clone(), temp_dir.path().join("dest2.txt")),
            ],
        };
        let value_path = temp_dir.path().join("used.json");
        orion_conf::JsonIO::save_json(&ValueDict::default(), &value_path).assert();
        let result = localize_set
            .mod_localize(value_path, LocalizeOptions::default())
            .await;
        assert!(result.is_ok());

        // Verify both files were localized
        assert!(temp_dir.path().join("dest1.txt").exists());
        assert!(temp_dir.path().join("dest2.txt").exists());

        assert_file_content(&temp_dir.path().join("dest1.txt"), "content1");
        assert_file_content(&temp_dir.path().join("dest2.txt"), "content2");
    }
}
