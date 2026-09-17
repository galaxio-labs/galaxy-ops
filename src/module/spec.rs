use super::prelude::*;
use crate::conf::{ConfFile, ConfSpec};
use crate::system::setting::Setting;
use crate::workflow::prj::GxlProject;

// 常量定义
const POSTGRESQL_URL: &str = "https://mirrors.aliyun.com/postgresql/latest/postgresql-17.4.tar.gz";
const POSTGRESQL_MD5_URL: &str =
    "https://mirrors.aliyun.com/postgresql/latest/postgresql-17.4.tar.gz.md5";
const POSTGRESQL_README_URL: &str = "https://mirrors.aliyun.com/postgresql/README";
const POSTGRESQL_ARCHIVE: &str = "postgresql-17.4.tar.gz";
const POSTGRESQL_MD5_ARCHIVE: &str = "postgresql-17.4.tar.gz.md5";
use crate::artifact::{Artifact, ArtifactPackage};
use indexmap::IndexMap;
use orion_variate::addr::HttpResource;

use super::{
    CpuArch, ModelSTD, OsCPE, RunSPC,
    depend::DependencySet,
    init::{ModIniter, ModPrjIniter, mod_init_gitignore},
    model::MMOperator,
};
use crate::types::{Accessor, LocalizeOptions, ModuleLocalizable, RefUpdateable};

#[derive(Getters, Clone, Debug)]
#[getset(get = "pub")]
pub struct ModuleSpec {
    name: String,
    targets: IndexMap<ModelSTD, MMOperator>,
    local: Option<PathBuf>,
}
impl ModuleSpec {
    pub fn init<S: Into<String>>(name: S, target_vec: Vec<MMOperator>) -> ModuleSpec {
        let mut targets = IndexMap::new();
        for node in target_vec {
            targets.insert(node.model().clone(), node);
        }
        Self {
            name: name.into(),
            targets,
            local: None,
        }
    }
    pub fn clean_other(&mut self, node: &ModelSTD) -> MainResult<()> {
        if let Some(local) = &self.local {
            let src_path = local.join(MOD_DIR);
            let subs = get_sub_dirs(&src_path).source_resource()?;
            for sub in subs {
                if !sub.ends_with(node.to_string().as_str()) {
                    Self::clean_path(&sub)?;
                }
            }
        }
        Ok(())
    }
    fn clean_path(path: &Path) -> MainResult<()> {
        if path.exists() {
            std::fs::remove_dir_all(path).source_resource().with(path)?;
        }
        Ok(())
    }
    pub fn save_main(&self, path: &Path, name: Option<String>) -> MainResult<()> {
        let mod_path = path.join(name.unwrap_or(self.name().clone()));
        std::fs::create_dir_all(&mod_path)
            .source_conf()
            .with(format!("path: {}", mod_path.display()))?;

        for node in self.targets.values() {
            node.save_main(&mod_path, Some("".into()))?;
        }
        Ok(())
    }
}

#[async_trait]
impl RefUpdateable<UpdateUnit> for ModuleSpec {
    async fn update_local(
        &self,
        accessor: Accessor,
        path: &Path,
        options: &DownloadOptions,
    ) -> MainResult<UpdateUnit> {
        for (target, node) in &self.targets {
            node.update_local(accessor.clone(), &path.join(target.to_string()), options)
                .await?;
        }
        Ok(UpdateUnit::from(path.to_path_buf()))
    }
}

impl FilePersist<ModuleSpec> for ModuleSpec {
    fn save_to(&self, path: &Path, name: Option<String>) -> SerdeResult<()> {
        let mod_path = path.join(name.unwrap_or(self.name().clone()));
        let src_path = mod_path.join(MOD_DIR);
        std::fs::create_dir_all(&mod_path)
            .source_conf()
            .with(format!("path: {}", mod_path.display()))?;

        mod_init_gitignore(&mod_path).source_resource()?;
        for node in self.targets.values() {
            node.save_to(&src_path, None)?;
        }

        Ok(())
    }

    fn load_from(path: &Path) -> SerdeResult<Self> {
        let name = path_file_name(path).source_logic()?;
        let name_copy = name.clone();
        let mut flag = auto_exit_log!(
            info!(target: "mod/spec", "load mod-spec {} success!", name_copy ),
            error!(target: "mod/spec", "load mod-spec {} fail!", name_copy)
        );
        let src_path = path.join(MOD_DIR);
        let subs = get_sub_dirs(&src_path).source_logic()?;
        let mut targets = IndexMap::new();
        for sub in subs {
            let node = MMOperator::load_from(&sub).with(&sub)?;
            targets.insert(node.model().clone(), node);
        }
        flag.mark_suc();
        Ok(Self {
            name,
            targets,
            local: Some(path.to_path_buf()),
        })
    }
}

#[async_trait]
impl ModuleLocalizable<ModValuePaths> for ModuleSpec {
    async fn mod_localize(
        &self,
        val_path: ModValuePaths,
        options: LocalizeOptions,
    ) -> MainResult<()> {
        for model in self.targets.values() {
            let mut ctx = OperationContext::want("model localize").with_auto_log();
            let model_path = val_path.clone().join(model.model().to_string());
            ctx.record("sys-value", model_path.sys_value_file().display());
            //let cur_options = if model_path.sys_value_file().exists() {
            let mut sys_vars = OriginDict::from(
                ValueDict::load_yaml(&model_path.sys_value_file()).source_resource()?,
            );
            sys_vars.set_source("sys-setting");
            let mut cur_dict = options.raw_value().clone();
            cur_dict.merge(&sys_vars);
            let cur_options = LocalizeOptions::new(cur_dict);
            //} else {
            //options.clone()
            //};
            model.mod_localize(model_path, cur_options).await?;
            ctx.mark_suc();
        }
        Ok(())
    }
}

fn pg_var_init() -> VarCollection {
    VarCollection::define(vec![
        VarDefinition::from(("app_name", "postgresql")).with_mut_immutable(),
        VarDefinition::from(("sys_domain", "http://test.galaxy.org/alpha")).with_mut_system(),
        VarDefinition::from(("cpu", 1000)).with_mut_module(),
        VarDefinition::from(("mem", 1048)).with_mut_module(),
    ])
}
impl ModuleSpec {
    pub fn for_example() -> Self {
        let name = "postgresql";
        let k8s = MMOperator::init(
            ModelSTD::new(CpuArch::X86, OsCPE::UBT22, RunSPC::K8S),
            ArtifactPackage::from(vec![Artifact::new(
                name,
                "0.1.0",
                HttpResource::from(POSTGRESQL_URL),
                POSTGRESQL_ARCHIVE,
            )]),
            ModWorkflows::mod_k8s_tpl_init(),
            GxlProject::spec_k8s_tpl(),
            //conf.clone(),
            pg_var_init(),
            Some(Setting::example()),
        )
        .with_depends(DependencySet::example());

        let host = MMOperator::init(
            ModelSTD::new(CpuArch::Arm, OsCPE::MAC14, RunSPC::Host),
            ArtifactPackage::from(vec![Artifact::new(
                name,
                "0.1.0",
                HttpResource::from(POSTGRESQL_URL),
                POSTGRESQL_ARCHIVE,
            )]),
            ModWorkflows::mod_host_tpl_init(),
            GxlProject::spec_host_tpl(),
            pg_var_init(),
            Some(Setting::example()),
        )
        .with_depends(DependencySet::example());
        ModuleSpec::init("postgresql", vec![k8s, host])
    }

    pub fn make_new(name: &str) -> MainResult<ModuleSpec> {
        let mut conf = ConfSpec::new("1.0.0", CONFS_DIR);
        conf.add(
            ConfFile::new("example.conf").with_addr(HttpResource::from(POSTGRESQL_README_URL)),
        );
        let vars = VarCollection::define(vec![
            VarDefinition::from(("app_name", name)).with_mut_immutable(),
            VarDefinition::from(("sys_domain", "http://test.galaxy.org/alpha")).with_mut_system(),
            VarDefinition::from(("ART_CACHE_REPO", "http://unknow.net")).with_mut_system(),
            VarDefinition::from(("cpu", 1000)).with_mut_module(),
            VarDefinition::from(("mem", 1048)).with_mut_module(),
        ]);

        let x86_ubu22_k8s = MMOperator::init(
            ModelSTD::x86_ubt22_k8s(),
            ArtifactPackage::from(vec![
                Artifact::new(
                    name,
                    "0.1.0",
                    HttpResource::from(POSTGRESQL_MD5_URL),
                    POSTGRESQL_MD5_ARCHIVE,
                )
                .with_cache_addr(Some(Address::from(HttpResource::from(
                    "{{ART_CACHE_REPO}}",
                )))),
            ]),
            ModWorkflows::mod_k8s_tpl_init(),
            GxlProject::spec_k8s_tpl(),
            //conf.clone(),
            vars.clone(),
            None,
        );

        let arm_mac_host = MMOperator::init(
            ModelSTD::arm_mac14_host(),
            ArtifactPackage::from(vec![
                Artifact::new(
                    name,
                    "0.1.0",
                    HttpResource::from(POSTGRESQL_MD5_URL),
                    POSTGRESQL_MD5_ARCHIVE,
                )
                .with_cache_addr(Some(Address::from(HttpResource::from(
                    "{{ART_CACHE_REPO}}",
                )))),
            ]),
            ModWorkflows::mod_host_tpl_init(),
            GxlProject::spec_host_tpl(),
            //conf.clone(),
            vars.clone(),
            None,
        );
        let x86_ubt22_host = MMOperator::init(
            ModelSTD::arm_mac14_host(),
            ArtifactPackage::from(vec![
                Artifact::new(
                    name,
                    "0.1.0",
                    HttpResource::from(POSTGRESQL_MD5_URL),
                    POSTGRESQL_MD5_ARCHIVE,
                )
                .with_cache_addr(Some(Address::from(HttpResource::from(
                    "{{ART_CACHE_REPO}}",
                )))),
            ]),
            ModWorkflows::mod_host_tpl_init(),
            GxlProject::spec_host_tpl(),
            //conf.clone(),
            vars.clone(),
            None,
        );

        Ok(ModuleSpec::init(
            name,
            vec![x86_ubu22_k8s, x86_ubt22_host, arm_mac_host],
        ))
    }
}

pub fn make_mod_spec_example() -> MainResult<ModuleSpec> {
    Ok(ModuleSpec::for_example())
}
pub fn make_mod_spec_4test() -> MainResult<ModuleSpec> {
    let name = "postgresql";
    let k8s = MMOperator::init(
        ModelSTD::new(CpuArch::X86, OsCPE::UBT22, RunSPC::K8S),
        ArtifactPackage::from(vec![Artifact::new(
            name,
            "0.1.0",
            HttpResource::from(POSTGRESQL_URL),
            POSTGRESQL_ARCHIVE,
        )]),
        ModWorkflows::mod_k8s_tpl_init(),
        GxlProject::spec_k8s_tpl(),
        //conf.clone(),
        pg_var_init(),
        Some(Setting::example()),
    )
    .with_depends(DependencySet::for_test());

    let host = MMOperator::init(
        ModelSTD::new(CpuArch::Arm, OsCPE::MAC14, RunSPC::Host),
        ArtifactPackage::from(vec![Artifact::new(
            name,
            "0.1.0",
            HttpResource::from(POSTGRESQL_URL),
            POSTGRESQL_ARCHIVE,
        )]),
        ModWorkflows::mod_host_tpl_init(),
        GxlProject::spec_host_tpl(),
        //conf.clone(),
        pg_var_init(),
        Some(Setting::example()),
    )
    .with_depends(DependencySet::for_test());
    Ok(ModuleSpec::init("postgresql", vec![k8s, host]))
}
