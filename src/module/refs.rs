use super::prelude::*;

use orion_variate::types::ResourceDownloader;

use super::ModelSTD;
use crate::types::{Accessor, RefUpdateable, SystemLocalizable};
use crate::{const_vars::MOD_DIR, module::model::MMOperator};

#[derive(Getters, Clone, Debug, Serialize, Deserialize)]
#[getset(get = "pub")]
pub struct ModuleSpecRef {
    name: String,
    addr: Address,
    #[serde(alias = "node")]
    model: ModelSTD,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    enable: Option<bool>,
    #[serde(skip)]
    local: Option<PathBuf>,
}

impl ModuleSpecRef {
    pub fn from<S: Into<String>, A: Into<Address>>(
        name: S,
        addr: A,
        node: ModelSTD,
    ) -> ModuleSpecRef {
        Self {
            name: name.into(),
            addr: addr.into(),
            model: node,
            enable: None,
            local: None,
        }
    }
    pub fn with_enable(mut self, effective: bool) -> Self {
        self.enable = Some(effective);
        self
    }

    pub fn with_local(mut self, local: PathBuf) -> Self {
        self.local = Some(local);
        self
    }

    pub fn is_enable(&self) -> bool {
        self.enable.unwrap_or(true)
    }
    pub fn spec_path(&self, root: &Path) -> PathBuf {
        root.join("mods").join(self.name.as_str())
    }
    pub fn set_local(&mut self, local: PathBuf) {
        self.local = Some(local);
    }
    pub fn get_target_spec(&self) -> MainResult<Option<MMOperator>> {
        if self.is_enable()
            && let Some(local) = &self.local
        {
            let target_root = local.join(self.name());
            let target_path = target_root.join(self.model().to_string());
            if target_path.exists() {
                let spec = MMOperator::load_from(&target_path)
                    .with(&target_root)
                    .owe(MainReason::from(ModReason::Load))?;
                return Ok(Some(spec));
            }
        }
        Ok(None)
    }
}
#[async_trait]
impl RefUpdateable<UpdateUnit> for ModuleSpecRef {
    //#[requires(self.local.is_some())]
    async fn update_local(
        &self,
        accessor: Accessor,
        _sys_root: &Path,
        options: &DownloadOptions,
    ) -> MainResult<UpdateUnit> {
        //trace!(target: "spec/mod/",  "{:?}",self );
        if let Some(local) = &self.local {
            let mut flag = auto_exit_log!(
                info!(target: "/mod/ref",  "update mod ref {} success!", self.name ),
                error!(target: "/mod/ref", "update mod ref {} fail!", self.name )
            );
            std::fs::create_dir_all(local)
                .source_resource()
                .with(local)?;
            let target_root = local.join(self.name());
            let target_path = target_root.join(self.model().to_string());
            if !target_path.exists() || options.clean_cache() {
                let tmp_name = "__mod";
                let prj_path = accessor
                    .download_rename(self.addr(), local, tmp_name, options)
                    .await
                    .map_err(MainReason::from_addr_error)?;
                let mod_path = prj_path.position().join(MOD_DIR);
                let tmp_path = local.join(tmp_name);
                make_clean_path(&target_root).source_resource()?;

                std::fs::rename(&mod_path, &target_root)
                    .source_logic()
                    .with(("from", &mod_path))
                    .with(("to", &target_root))?;
                if tmp_path.exists() {
                    std::fs::remove_dir_all(tmp_path).source_sys()?;
                }
            }

            debug!(target: "mod/ref",  "update target success!" );
            //let target_path = target_root.join(self.node().to_string());
            let spec = MMOperator::load_from(&target_path)
                .with(&target_root)
                .owe(MainReason::from(ModReason::Load))?;
            let unit = spec
                .update_local(accessor, &target_path, options)
                .await
                .with(("module", self.name().to_string()))?;
            MMOperator::clean_other(&target_root, self.model())?;
            flag.mark_suc();
            return Ok(unit);
        } else {
            Err(MainReason::logic_detail("no local value in ModuleSpecRef"))
        }
    }
}

impl ModuleSpecRef {
    pub fn spec_value_path(&self, parent: ValuePath) -> ValuePath {
        let value = PathBuf::from(self.name());
        parent.join(value)
    }
}
use crate::system::SysValuePaths;
#[async_trait]
impl SystemLocalizable<SysValuePaths> for ModuleSpecRef {
    async fn sys_localize(
        &self,
        val_path: SysValuePaths,
        options: LocalizeOptions,
    ) -> MainResult<()> {
        if self.enable.is_none_or(|x| x) {
            if let Some(local) = &self.local {
                let mut ctx = OperationContext::want("mod ref localize")
                    .with_auto_log()
                    .with_mod_path("mod");
                ctx.record("name", self.name.as_str());
                let mod_val_path = val_path.join(self.name.as_str());
                let mod_path = local.join(self.name.as_str());
                let target_path = mod_path.join(self.model().to_string());
                let spec =
                    MMOperator::load_from(&target_path).owe(MainReason::from(ModReason::Load))?;
                //let value = PathBuf::from(self.name());
                let cur_md_path = ModValuePaths::from(mod_val_path.root().clone());
                spec.mod_localize(cur_md_path.clone(), options.clone())
                    .await
                    .with(&ctx)?;
                ctx.mark_suc();
            }
            Ok(())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::module::{ModelSTD, refs::ModuleSpecRef};

    #[test]
    fn test_module_spec_ref_builder() {
        let model_std = ModelSTD::x86_ubt22_k8s();
        let module_ref = ModuleSpecRef::from(
            "test-module",
            "https://github.com/example/test-module.git",
            model_std.clone(),
        )
        .with_enable(true)
        .with_local(PathBuf::from("/tmp/test"));

        assert_eq!(module_ref.name(), "test-module");
        assert!(!module_ref.addr().to_string().is_empty());
        assert_eq!(module_ref.model(), &model_std);
        assert!(module_ref.is_enable());
        assert!(module_ref.local().is_some());
    }

    #[test]
    fn test_module_spec_ref_enable_flag() {
        let model_std = ModelSTD::x86_ubt22_k8s();

        let enabled_ref =
            ModuleSpecRef::from("test", "https://example.com", model_std.clone()).with_enable(true);
        let disabled_ref = ModuleSpecRef::from("test", "https://example.com", model_std.clone())
            .with_enable(false);
        let default_ref = ModuleSpecRef::from("test", "https://example.com", model_std);

        assert!(enabled_ref.is_enable());
        assert!(!disabled_ref.is_enable());
        assert!(default_ref.is_enable()); // Default should be true
    }

    #[tokio::test]
    async fn test_module_spec_ref_spec_path() {
        let model_std = ModelSTD::x86_ubt22_k8s();
        let module_ref = ModuleSpecRef::from("test-module", "https://example.com", model_std);

        let root = PathBuf::from("/project/root");
        let spec_path = module_ref.spec_path(&root);

        assert_eq!(spec_path, root.join("mods").join("test-module"));
    }
}
