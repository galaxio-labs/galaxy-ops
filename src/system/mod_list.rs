use super::prelude::*;
use std::net::Ipv4Addr;

use crate::types::{SysUpdateValue, ValuePath};
use derive_more::Deref;
use orion_vars::vars::{ValueDict, ValueType, VarCollection};

use crate::const_vars::MODS_DIR;
use crate::error::MainResult;
use crate::module::refs::ModuleSpecRef;
use crate::module::spec::ModuleSpec;

#[derive(Getters, Clone, Debug, Default, Serialize, Deserialize, Deref)]
#[getset(get = "pub")]
#[serde(transparent)]
pub struct ModulesList {
    mods: Vec<ModuleSpecRef>,
    //#[serde(skip)]
    //mod_map: HashMap<String, ModuleSpec>,
}
impl ModulesList {
    pub fn add_ref(&mut self, spec_ref: ModuleSpecRef) {
        self.mods.push(spec_ref);
    }
    pub fn export(&self) -> ValueDict {
        let mut dict = ValueDict::new();
        for item in self.mods().iter() {
            if item.is_enable() {
                dict.insert(item.name(), ValueType::from(item.name().as_str()));
            }
        }
        dict
    }

    /// 注入各模块的内容目录：新布局 `<sys_root>/<model>/mods/<mod>`（写入/优先读取），
    /// 并记录旧布局 `<sys_root>/mods/<mod>/<model>` 供读取回退。
    ///
    /// `sys_root` 是系统目录（`sys/`）；模型取自各模块 ref 自身，
    /// 因此不同模型的模块会落在不同的 `sys/<model>/mods/` 下。
    pub fn set_mods_local(&mut self, sys_root: PathBuf) {
        for x in self.mods.iter_mut() {
            let local = x.mod_dir(&sys_root);
            let legacy = x.legacy_mod_dir(&sys_root);
            x.set_local_with_legacy(local, legacy);
        }
    }

    pub fn find(&self, arg: &str) -> Option<&ModuleSpecRef> {
        self.mods.iter().find(|x| x.name() == arg)
    }
}

#[async_trait]
impl RefUpdateable<SysUpdateValue> for ModulesList {
    async fn update_local(
        &self,
        accessor: Accessor,
        sys_root: &Path,
        options: &DownloadOptions,
    ) -> MainResult<SysUpdateValue> {
        let mut vars = VarCollection::default();
        for m in &self.mods {
            if m.is_enable() {
                let update_v = m.update_local(accessor.clone(), sys_root, options).await?;
                if let Some(v) = update_v.vars {
                    vars = vars.merge_system(v);
                }
            }
        }
        // 迁移清理：模块已落到新布局 `sys/<model>/mods/`，删除旧布局 `sys/mods` 遗留。
        // 无模块的系统不动 `sys/mods`，缩小影响面。
        let legacy_root = sys_root.join(MODS_DIR);
        if !self.mods.is_empty() && legacy_root.is_dir() {
            std::fs::remove_dir_all(&legacy_root).source_sys()?;
        }
        Ok(SysUpdateValue::new(vars))
    }
}

impl ModulesList {
    pub fn value_path(&self, parent: ValuePath) -> ValuePath {
        parent.join_all("mods")
    }
}
#[async_trait]
impl SystemLocalizable<SysValuePaths> for ModulesList {
    async fn sys_localize(
        &self,
        val_path: SysValuePaths,
        options: LocalizeOptions,
    ) -> MainResult<()> {
        //let root = val_path.join("mods");
        for m in &self.mods {
            if m.is_enable() && options.allow_module(m.name()) {
                m.sys_localize(val_path.clone(), options.clone()).await?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum NoneValue<T> {
    None,
    Value(T),
}
impl ModulesList {
    pub fn add_mod(&mut self, _modx: ModuleSpec) {
        todo!();
        //self.mod_map.insert(modx.name().clone(), modx);
    }
}

#[derive(Getters, Clone, Debug, Serialize, Deserialize)]
#[getset(get = "pub")]
pub struct NetResSpace {
    master: Ipv4Addr,
    node_scope: (Ipv4Addr, Ipv4Addr),
}
impl NetResSpace {
    pub fn new(master: Ipv4Addr, node_scope: (Ipv4Addr, Ipv4Addr)) -> Self {
        Self { master, node_scope }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::{CpuArch, ModelSTD, OsCPE, RunSPC};
    use crate::types::RefUpdateable;
    use orion_variate::update::DownloadOptions;

    #[test]
    fn test_set_mods_local_groups_by_model() {
        let mut list = ModulesList::default();
        let arm = ModelSTD::new(CpuArch::Arm, OsCPE::MAC14, RunSPC::Host);
        let k8s = ModelSTD::x86_ubt22_k8s();
        list.add_ref(ModuleSpecRef::from(
            "mod-a",
            "https://example.com/a",
            arm.clone(),
        ));
        list.add_ref(ModuleSpecRef::from(
            "mod-b",
            "https://example.com/b",
            arm.clone(),
        ));
        list.add_ref(ModuleSpecRef::from(
            "mod-c",
            "https://example.com/c",
            k8s.clone(),
        ));

        list.set_mods_local(PathBuf::from("/sys"));

        // 按目标模型分组：同名模型的模块集中在同一个 `mods/` 下
        assert_eq!(
            list.find("mod-a").unwrap().local(),
            &Some(PathBuf::from("/sys/arm-mac14-host/mods/mod-a"))
        );
        assert_eq!(
            list.find("mod-b").unwrap().local(),
            &Some(PathBuf::from("/sys/arm-mac14-host/mods/mod-b"))
        );
        assert_eq!(
            list.find("mod-c").unwrap().local(),
            &Some(PathBuf::from("/sys/x86-ubt22-k8s/mods/mod-c"))
        );
        // 同时记录旧布局供读取回退
        assert_eq!(
            list.find("mod-a").unwrap().legacy_local(),
            &Some(PathBuf::from("/sys/mods/mod-a/arm-mac14-host"))
        );
    }

    #[tokio::test]
    async fn test_update_local_removes_legacy_mods_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let sys_root = tmp.path().join("sys");
        let legacy = sys_root.join("mods/old-mod/arm-mac14-host");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("vars.yml"), "x").unwrap();

        let mut list = ModulesList::default();
        // 禁用模块，避免真实下载；仅为触发迁移清理
        list.add_ref(
            ModuleSpecRef::from(
                "mod-a",
                "https://example.com/a",
                ModelSTD::new(CpuArch::Arm, OsCPE::MAC14, RunSPC::Host),
            )
            .with_enable(false),
        );

        let accessor = crate::accessor::accessor_for_test();
        list.update_local(accessor, &sys_root, &DownloadOptions::for_test())
            .await
            .unwrap();
        assert!(!sys_root.join("mods").exists(), "旧布局 sys/mods 应被清理");
    }

    #[tokio::test]
    async fn test_update_local_keeps_sys_mods_when_no_modules() {
        let tmp = tempfile::tempdir().unwrap();
        let sys_root = tmp.path().join("sys");
        std::fs::create_dir_all(sys_root.join("mods/x")).unwrap();

        let list = ModulesList::default();
        let accessor = crate::accessor::accessor_for_test();
        list.update_local(accessor, &sys_root, &DownloadOptions::for_test())
            .await
            .unwrap();

        // 无模块的系统不应被误删
        assert!(sys_root.join("mods").exists());
    }
}
