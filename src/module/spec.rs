use super::prelude::*;
use crate::conf::{ConfFile, ConfSpec};
use crate::system::setting::Setting;
use crate::workflow::prj::GxlProject;

// 常量定义
const POSTGRESQL_URL: &str = "https://mirrors.aliyun.com/postgresql/latest/postgresql-17.4.tar.gz";
const POSTGRESQL_README_URL: &str = "https://mirrors.aliyun.com/postgresql/README";
const POSTGRESQL_ARCHIVE: &str = "postgresql-17.4.tar.gz";
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

/// k8s 模型的约定变量：供 Helm chart（`spec/confs`）与 `helm_ops` 流程共同使用。
pub fn k8s_var_init(name: &str, version: &str) -> VarCollection {
    VarCollection::define(vec![
        VarDefinition::from(("IMAGE_REPOSITORY", name)).with_mut_immutable(),
        VarDefinition::from(("IMAGE_TAG", version)).with_mut_module(),
        VarDefinition::from(("APP_NAME", name)).with_mut_module(),
        VarDefinition::from(("NAMESPACE", name)).with_mut_module(),
        VarDefinition::from(("IMAGE_REGISTRY", "your-registry.example.com")).with_mut_module(),
        VarDefinition::from(("IMAGE_PULL_SECRET", "")).with_mut_module(),
        VarDefinition::from(("REPLICA_COUNT", 1)).with_mut_module(),
        VarDefinition::from(("SERVICE_TYPE", "ClusterIP")).with_mut_module(),
        VarDefinition::from(("SERVICE_PORT", 8080)).with_mut_module(),
        VarDefinition::from(("RUNTIME", "containerd")).with_mut_system(),
        VarDefinition::from(("AIR_GAPPED", "false")).with_mut_system(),
        VarDefinition::from(("KUBECONFIG", "~/.kube/config")).with_mut_system(),
    ])
}
impl ModuleSpec {
    pub fn for_example() -> Self {
        let name = "postgresql";
        let k8s = MMOperator::init(
            ModelSTD::new(CpuArch::X86, OsCPE::UBT22, RunSPC::K8S),
            // k8s 构件是容器镜像（helm_ops 只处理 local == docker_image 的条目）
            ArtifactPackage::from(vec![Artifact::new(
                name,
                "0.1.0",
                HttpResource::from("your-registry.example.com"),
                "docker_image",
            )]),
            ModWorkflows::mod_k8s_tpl_init(),
            GxlProject::spec_k8s_tpl(),
            //conf.clone(),
            k8s_var_init(name, "0.1.0"),
            Some(Setting::k8s_module()),
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

        // 构件地址跟随模块名，避免硬编码到具体组件（如 postgresql）
        let artifact_version = "0.1.0";
        let artifact_local = format!("{name}-{artifact_version}.tar.gz");
        let artifact_url = format!("http://your-artifact-repo/{artifact_local}");
        let artifact = Artifact::new(
            name,
            artifact_version,
            HttpResource::from(artifact_url.as_str()),
            artifact_local.as_str(),
        )
        .with_cache_addr(Some(Address::from(HttpResource::from(
            "{{ART_CACHE_REPO}}",
        ))));

        // k8s 模型：构件是容器镜像 + 约定变量 + 带 Helm 支持的 setting
        let k8s_vars = k8s_var_init(name, artifact_version);
        let k8s_image_artifact = Artifact::new(
            name,
            artifact_version,
            HttpResource::from("your-registry.example.com"),
            "docker_image",
        );

        let x86_ubu22_k8s = MMOperator::init(
            ModelSTD::x86_ubt22_k8s(),
            ArtifactPackage::from(vec![k8s_image_artifact]),
            ModWorkflows::mod_k8s_tpl_init(),
            GxlProject::spec_k8s_tpl(),
            k8s_vars,
            Some(Setting::k8s_module()),
        );

        let arm_mac_host = MMOperator::init(
            ModelSTD::arm_mac14_host(),
            ArtifactPackage::from(vec![artifact.clone()]),
            ModWorkflows::mod_host_tpl_init(),
            GxlProject::spec_host_tpl(),
            vars.clone(),
            None,
        );
        let x86_ubt22_host = MMOperator::init(
            ModelSTD::x86_ubt22_host(),
            ArtifactPackage::from(vec![artifact.clone()]),
            ModWorkflows::mod_host_tpl_init(),
            GxlProject::spec_host_tpl(),
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
        // k8s 构件是容器镜像（helm_ops 只处理 local == docker_image 的条目）
        ArtifactPackage::from(vec![Artifact::new(
            name,
            "0.1.0",
            HttpResource::from("your-registry.example.com"),
            "docker_image",
        )]),
        ModWorkflows::mod_k8s_tpl_init(),
        GxlProject::spec_k8s_tpl(),
        //conf.clone(),
        k8s_var_init(name, "0.1.0"),
        Some(Setting::k8s_module()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_k8s_var_init_scopes_and_values() {
        let vars = k8s_var_init("demo", "1.2.3");

        let immutable: Vec<String> = vars
            .immutable_vars()
            .iter()
            .map(|v| v.name().to_string())
            .collect();
        assert_eq!(immutable, vec!["IMAGE_REPOSITORY".to_string()]);

        let system: Vec<String> = vars
            .system_vars()
            .iter()
            .map(|v| v.name().to_string())
            .collect();
        for key in ["RUNTIME", "AIR_GAPPED", "KUBECONFIG"] {
            assert!(
                system.contains(&key.to_string()),
                "system var {key} missing"
            );
        }

        let dict = vars.value_dict();
        assert_eq!(dict.get("IMAGE_REPOSITORY"), Some(&ValueType::from("demo")));
        assert_eq!(dict.get("IMAGE_TAG"), Some(&ValueType::from("1.2.3")));
        assert_eq!(dict.get("APP_NAME"), Some(&ValueType::from("demo")));
        assert_eq!(dict.get("NAMESPACE"), Some(&ValueType::from("demo")));

        // IMAGE_TAG 必须在模块层可变，否则客户无法升级镜像 tag
        let module: Vec<String> = vars
            .module_vars()
            .iter()
            .map(|v| v.name().to_string())
            .collect();
        assert!(module.contains(&"IMAGE_TAG".to_string()));
    }

    #[test]
    fn test_make_new_k8s_model_shape() -> MainResult<()> {
        let spec = ModuleSpec::make_new("demo")?;

        let k8s = spec
            .targets()
            .get(&ModelSTD::x86_ubt22_k8s())
            .expect("k8s target missing");
        assert!(k8s.model().is_k8s());
        // 构件是容器镜像，而非二进制归档
        assert!(k8s.artifact().iter().any(|a| a.local() == "docker_image"));

        let setting = k8s.setting().as_ref().expect("k8s setting missing");
        let localize = setting.localize().as_ref().expect("localize missing");
        let paths = localize
            .templatize_path()
            .as_ref()
            .expect("templatize_path missing");
        assert!(
            paths.excludes().iter().any(|e| e == "spec/confs/templates"),
            "templates/ must be excluded from gops localize"
        );
        let cust = localize
            .templatize_cust()
            .as_ref()
            .expect("templatize_cust missing");
        assert_eq!(cust.label_beg(), "[[");
        assert_eq!(cust.label_end(), "]]");

        // host 模型没有 k8s 专属 setting
        let host = spec
            .targets()
            .get(&ModelSTD::x86_ubt22_host())
            .expect("host target missing");
        assert!(!host.model().is_k8s());
        assert!(host.setting().is_none());
        Ok(())
    }

    #[test]
    fn test_save_writes_chart_for_k8s_only() -> MainResult<()> {
        let spec = ModuleSpec::make_new("demo")?;
        let tmp = TempDir::new().source_resource()?;
        spec.save_to(tmp.path(), None).source_logic()?;

        let mod_root = tmp.path().join("demo").join(MOD_DIR);
        let k8s_confs = mod_root.join("x86-ubt22-k8s").join(SPEC_DIR).join("confs");
        assert!(
            k8s_confs.join("Chart.yaml").exists(),
            "k8s chart missing: {}",
            k8s_confs.display()
        );
        assert!(k8s_confs.join("templates").join("deployment.yaml").exists());

        // host 模型不应生成 chart
        assert!(
            !mod_root
                .join("x86-ubt22-host")
                .join(SPEC_DIR)
                .join("confs")
                .exists(),
            "host model must not get a Helm chart"
        );
        Ok(())
    }

    #[test]
    fn test_example_k8s_model_is_localizable() -> MainResult<()> {
        // 回归：example / 4test 的 k8s 模型必须带 k8s 变量 + k8s_module setting，
        // 否则 mod localize 会因缺少 IMAGE_REGISTRY 等变量而失败。
        for spec in [ModuleSpec::for_example(), make_mod_spec_4test()?] {
            let k8s = spec
                .targets()
                .get(&ModelSTD::x86_ubt22_k8s())
                .expect("k8s target missing");
            let dict = k8s.vars().value_dict();
            for key in ["IMAGE_REGISTRY", "IMAGE_REPOSITORY", "IMAGE_TAG"] {
                assert!(dict.get(key).is_some(), "k8s var {key} missing");
            }
            assert!(k8s.artifact().iter().any(|a| a.local() == "docker_image"));
            let setting = k8s.setting().as_ref().expect("k8s setting missing");
            assert!(setting.localize().is_some());
        }
        Ok(())
    }
}
