use super::prelude::*;

use orion_variate::types::ResourceDownloader;

use super::ModelSTD;
use crate::types::{Accessor, RefUpdateable, SystemLocalizable};
use crate::{
    const_vars::{MOD_DIR, MODS_DIR},
    module::model::MMOperator,
};

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
    /// 旧布局内容目录 `sys/mods/<mod>/<model>`（1.3 及更早），仅供读取回退。
    #[serde(skip)]
    legacy_local: Option<PathBuf>,
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
            legacy_local: None,
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
    /// 模块内容目录：`<sys_root>/<model>/mods/<name>`。
    ///
    /// 自 1.4 起按目标模型分组（`sys/<model>/mods/<mod>`），
    /// 取代旧的 `<sys_root>/mods/<name>/<model>`；同一模型的模块因此集中在一处。
    pub fn mod_dir(&self, sys_root: &Path) -> PathBuf {
        sys_root
            .join(self.model().to_string())
            .join(MODS_DIR)
            .join(self.name.as_str())
    }
    /// 旧布局内容目录：`<sys_root>/mods/<name>/<model>`（1.3 及更早）。
    pub fn legacy_mod_dir(&self, sys_root: &Path) -> PathBuf {
        sys_root
            .join(MODS_DIR)
            .join(self.name.as_str())
            .join(self.model().to_string())
    }
    /// 同时记录新布局（写入/优先读取）与旧布局（读取回退）目录。
    pub fn set_local_with_legacy(&mut self, local: PathBuf, legacy: PathBuf) {
        self.local = Some(local);
        self.legacy_local = Some(legacy);
    }
    /// 读取模块内容目录：优先新布局 `sys/<model>/mods/<mod>`，
    /// 缺失时回退旧布局 `sys/mods/<mod>/<model>`（两者皆无时返回 `None`）。
    pub fn content_dir(&self) -> Option<&PathBuf> {
        self.local
            .as_ref()
            .filter(|p| p.exists())
            .or_else(|| self.legacy_local.as_ref().filter(|p| p.exists()))
    }
    pub fn get_target_spec(&self) -> MainResult<Option<MMOperator>> {
        if self.is_enable()
            && let Some(local) = self.content_dir()
        {
            let spec = MMOperator::load_from_model(local, self.model())
                .with(local)
                .owe(MainReason::from(ModReason::Load))?;
            return Ok(Some(spec));
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
            // local 即模块内容目录 `sys/<model>/mods/<mod>`；下载和解包在它的父目录进行。
            let target_root = local.clone();
            let work_root = target_root
                .parent()
                .ok_or_else(|| MainReason::logic_detail("bad module local path"))?
                .to_path_buf();
            std::fs::create_dir_all(&work_root)
                .source_resource()
                .with(&work_root)?;
            if !target_root.exists() || options.clean_cache() {
                let tmp_name = "__mod";
                let prj_path = accessor
                    .download_rename(self.addr(), &work_root, tmp_name, options)
                    .await
                    .map_err(MainReason::from_addr_error)?;
                // 模块包里 `mod/<model>/` 才是本模型的产物，只取这一份。
                let model_path = prj_path
                    .position()
                    .join(MOD_DIR)
                    .join(self.model().to_string());
                let tmp_path = work_root.join(tmp_name);
                make_clean_path(&target_root).source_resource()?;

                std::fs::rename(&model_path, &target_root)
                    .source_logic()
                    .with(("from", &model_path))
                    .with(("to", &target_root))?;
                if tmp_path.exists() {
                    std::fs::remove_dir_all(tmp_path).source_sys()?;
                }
            }

            debug!(target: "mod/ref",  "update target success!" );
            let spec = MMOperator::load_from_model(&target_root, self.model())
                .with(&target_root)
                .owe(MainReason::from(ModReason::Load))?;
            let unit = spec
                .update_local(accessor, &target_root, options)
                .await
                .with(("module", self.name().to_string()))?;
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
                // 读取优先新布局，缺失时回退旧布局；两者皆无时按新路径报错。
                let target_path = self.content_dir().unwrap_or(local);
                let spec = MMOperator::load_from_model(target_path, self.model())
                    .owe(MainReason::from(ModReason::Load))?;
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
    use std::path::{Path, PathBuf};

    use crate::module::{CpuArch, ModelSTD, OsCPE, RunSPC, refs::ModuleSpecRef};

    fn copy_dir_all(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let to = dst.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir_all(&entry.path(), &to);
            } else {
                std::fs::copy(entry.path(), &to).unwrap();
            }
        }
    }

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
    async fn test_module_spec_ref_mod_dir() {
        let model_std = ModelSTD::x86_ubt22_k8s();
        let module_ref = ModuleSpecRef::from("test-module", "https://example.com", model_std);

        let root = PathBuf::from("/project/root");
        let spec_path = module_ref.mod_dir(&root);

        assert_eq!(
            spec_path,
            root.join("x86-ubt22-k8s").join("mods").join("test-module")
        );
    }

    #[test]
    fn test_content_dir_prefers_new_then_legacy() {
        let tmp = tempfile::tempdir().unwrap();
        let sys_root = tmp.path().join("sys");
        let mut r = ModuleSpecRef::from(
            "m",
            "https://example.com",
            ModelSTD::new(CpuArch::Arm, OsCPE::MAC14, RunSPC::Host),
        );
        let new = r.mod_dir(&sys_root);
        let legacy = r.legacy_mod_dir(&sys_root);
        assert_eq!(new, sys_root.join("arm-mac14-host/mods/m"));
        assert_eq!(legacy, sys_root.join("mods/m/arm-mac14-host"));
        r.set_local_with_legacy(new.clone(), legacy.clone());

        // 两者皆无 → None
        assert_eq!(r.content_dir(), None);
        // 仅旧布局 → 回退旧布局
        std::fs::create_dir_all(&legacy).unwrap();
        assert_eq!(r.content_dir(), Some(&legacy));
        // 新布局存在 → 优先新布局
        std::fs::create_dir_all(&new).unwrap();
        assert_eq!(r.content_dir(), Some(&new));
    }

    #[test]
    fn test_get_target_spec_falls_back_to_legacy_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let sys_root = tmp.path().join("sys");
        let mut r = ModuleSpecRef::from(
            "redis2_mock",
            "https://example.com",
            ModelSTD::new(CpuArch::Arm, OsCPE::MAC14, RunSPC::Host),
        );
        // 内容只放在旧布局
        let legacy = r.legacy_mod_dir(&sys_root);
        copy_dir_all(
            Path::new("./example/mod-operators/redis2_mock/mod/arm-mac14-host"),
            &legacy,
        );
        let new = r.mod_dir(&sys_root);
        r.set_local_with_legacy(new, legacy);

        let spec = r.get_target_spec().unwrap();
        assert!(spec.is_some(), "应能从旧布局回退加载模块");
    }

    #[test]
    fn test_get_target_spec_prefers_new_layout_when_both_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let sys_root = tmp.path().join("sys");
        let mut r = ModuleSpecRef::from(
            "redis2_mock",
            "https://example.com",
            ModelSTD::new(CpuArch::Arm, OsCPE::MAC14, RunSPC::Host),
        );
        let new = r.mod_dir(&sys_root);
        let legacy = r.legacy_mod_dir(&sys_root);
        copy_dir_all(
            Path::new("./example/mod-operators/redis2_mock/mod/arm-mac14-host"),
            &new,
        );
        copy_dir_all(
            Path::new("./example/mod-operators/redis2_mock/mod/arm-mac14-host"),
            &legacy,
        );
        r.set_local_with_legacy(new.clone(), legacy);

        let spec = r.get_target_spec().unwrap().unwrap();
        assert_eq!(spec.root().as_ref(), Some(&new), "应优先加载新布局");
    }
}
