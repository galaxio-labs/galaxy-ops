use crate::internal_prelude::OperationContext;

use super::prelude::*;
use crate::localize::{LocalizeTemplate, TemplateConfig};
use crate::system::setting::Setting;
use crate::{
    artifact::ArtifactPackage,
    const_vars::{
        DEFAULT_VALUE_FILE, LOCAL_DIR, MOD_VALUE_FILE, USED_JSON, USED_READABLE_FILE,
        USER_VALUE_FILE,
    },
    module::operator::ModValuePaths,
    types::{Accessor, ModuleLocalizable, RefUpdateable, ValuePath},
};
use std::{fs::read_to_string, str::FromStr};

use super::{ModelSTD, depend::DependencySet};

#[derive(Getters, Clone, Debug, Serialize)]
#[getset(get = "pub")]
pub struct MMOperator {
    model: ModelSTD,
    artifact: ArtifactPackage,
    workflow: ModWorkflows,
    gxl_prj: GxlProject,
    vars: VarCollection,
    root: Option<PathBuf>,
    setting: Option<Setting>,
    depends: DependencySet,
}

impl MMOperator {
    pub fn with_depends(mut self, depends: DependencySet) -> Self {
        self.depends = depends;
        self
    }

    fn build_used_value(
        &self,
        options: LocalizeOptions,
        mod_value: &Path,
    ) -> Result<OriginDict, StructError<MainReason>> {
        crate::project::mix_used_value(options, &self.vars, mod_value)
    }
}

#[async_trait]
impl RefUpdateable<UpdateUnit> for MMOperator {
    async fn update_local(
        &self,
        accessor: Accessor,
        path: &Path,
        options: &DownloadOptions,
    ) -> MainResult<UpdateUnit> {
        //self.conf_spec.update_local(path, options).await?;
        self.depends.update_local(accessor, path, options).await?;
        Ok(UpdateUnit::new(path.to_path_buf(), self.vars.clone()))
    }
}
impl MMOperator {
    pub fn save_main(&self, root: &Path, name: Option<String>) -> MainResult<()> {
        let target_path = root.join(name.unwrap_or(self.model().to_string()));
        std::fs::create_dir_all(&target_path)
            .source_conf()
            .with(format!("path: {}", target_path.display()))?;
        self.workflow.save_to(&target_path, None).source_logic()?;
        Ok(())
    }

    pub fn clean_other(root: &Path, node: &ModelSTD) -> MainResult<()> {
        let subs = get_sub_dirs(root).source_logic()?;
        for sub in subs {
            if !sub.ends_with(node.to_string().as_str()) {
                Self::clean_path(&sub)?;
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
}

#[derive(Getters, Clone, Debug)]
#[getset(get = "pub")]
pub struct ModTargetPaths {
    target_root: PathBuf,
    spec_path: PathBuf,
    conf_path: PathBuf,
    vars_path: PathBuf,
    setting_path: PathBuf,
    artifact_path: PathBuf,
    workflow_path: PathBuf,
    depends_path: PathBuf,
}
impl From<&PathBuf> for ModTargetPaths {
    fn from(target_root: &PathBuf) -> Self {
        let spec_path = target_root.join(SPEC_DIR);
        Self {
            target_root: target_root.to_path_buf(),
            conf_path: spec_path.join(CONF_SPEC_YML),
            vars_path: target_root.join(VARS_YML),
            setting_path: target_root.join(SETTING_YML),
            artifact_path: spec_path.join(ARTIFACT_YML),
            depends_path: spec_path.join(DEPENDS_YML),
            workflow_path: target_root.to_path_buf(),
            spec_path,
        }
    }
}

#[derive(Getters, Clone, Debug)]
#[getset(get = "pub")]
pub struct TargetValuePaths {
    used_readable: PathBuf,
    default_value_file: PathBuf,
    user_value_file: PathBuf,
    mod_value_file: PathBuf,
    used_json_path: PathBuf,
}
impl From<&PathBuf> for TargetValuePaths {
    fn from(value_root: &PathBuf) -> Self {
        Self {
            used_readable: value_root.join(USED_READABLE_FILE),
            default_value_file: value_root.join(DEFAULT_VALUE_FILE),
            user_value_file: value_root.join(USER_VALUE_FILE),
            mod_value_file: value_root.join(MOD_VALUE_FILE),
            used_json_path: value_root.join(crate::const_vars::USED_JSON),
        }
    }
}

impl FilePersist<MMOperator> for MMOperator {
    fn save_to(&self, root: &Path, name: Option<String>) -> SerdeResult<()> {
        let target_path = root.join(name.unwrap_or(self.model().to_string()));

        let mut ctx = OperationContext::want("save target").with_auto_log();
        ctx.record("target", target_path.display());
        let paths = ModTargetPaths::from(&target_path);
        std::fs::create_dir_all(paths.spec_path())
            .source_conf()
            .with(&ctx)
            .with(format!("path: {}", paths.spec_path().display()))?;

        if let Some(setting) = &self.setting {
            orion_conf::ConfigIO::save_conf(setting, paths.setting_path())
                .source_logic()
                .with(&ctx)?;
        }
        self.workflow.save_to(paths.workflow_path(), None)?;
        orion_conf::ConfigIO::save_conf(&self.artifact, paths.artifact_path())
            .source_logic()
            .with(&ctx)?;

        orion_conf::ConfigIO::save_conf(&self.depends, paths.depends_path())
            .source_logic()
            .with(&ctx)?;
        orion_conf::ConfigIO::save_conf(&self.vars, paths.vars_path())
            .source_logic()
            .with(&ctx)?;
        self.gxl_prj.save_to(&paths.target_root, None).with(&ctx)?;
        //flag.mark_suc();
        ctx.mark_suc();
        Ok(())
    }

    fn load_from(target_root: &Path) -> SerdeResult<Self> {
        let mut ctx = WithContext::want("load target mod spec");

        let mut flag = auto_exit_log!(
            info!(target: "spec/mod/target", "load target  success!:{}", target_root.display()),
            error!(target: "spec/mod/target", "load target failed!:{}", target_root.display())
        );
        let paths = ModTargetPaths::from(&target_root.to_path_buf());
        ctx.record("root", target_root.display());
        let target = ModelSTD::from_str(path_file_name(target_root).source_logic()?.as_str())
            .source_resource()
            .with(&ctx)?;
        let actions = ModWorkflows::load_from(paths.workflow_path()).with(&ctx)?;

        let setting = if paths.setting_path().exists() {
            Some(Setting::load_conf(paths.setting_path()).source_logic()?)
        } else {
            None
        };
        ctx.record("artifact", paths.artifact_path().display());
        let artifact = ArtifactPackage::load_conf(paths.artifact_path())
            .with(&ctx)
            .source_logic()?;

        //ctx.record("conf_spec", paths.conf_path());
        //let conf_spec = ConfSpec::load_conf(paths.conf_path()).with(&ctx)?;

        ctx.record("depends", paths.depends_path().display());
        let depends = DependencySet::load_conf(paths.depends_path())
            .with(&ctx)
            .source_logic()?;
        ctx.record("vars", paths.vars_path().display());
        //let vars = VarCollection::eval_from_file(&ValueDict::default(), paths.vars_path())
        let vars = VarCollection::load_conf(paths.vars_path())
            .with(&ctx)
            .source_logic()?;

        let gxl_prj = GxlProject::load_from(paths.target_root()).with(&ctx)?;
        flag.mark_suc();
        Ok(Self {
            model: target,
            artifact,
            workflow: actions,
            //conf_spec,
            root: Some(target_root.to_path_buf()),
            vars,
            setting,
            depends,
            gxl_prj,
        })
    }
}
impl MMOperator {
    pub fn init(
        target: ModelSTD,
        artifact: ArtifactPackage,
        workflow: ModWorkflows,
        gxl_prj: GxlProject,
        //conf_spec: ConfSpec,
        vars: VarCollection,
        setting: Option<Setting>,
    ) -> Self {
        Self {
            model: target,
            workflow,
            gxl_prj,
            artifact,
            root: None,
            vars,
            setting,
            depends: DependencySet::default(),
        }
    }
    pub fn get_local_values(&self, parent: ValuePath) -> MainResult<Option<String>> {
        let value_paths = TargetValuePaths::from(parent.path());
        if value_paths.used_readable().exists() {
            let data = read_to_string(value_paths.used_readable()).source_sys()?;
            return Ok(Some(data));
        }
        Ok(None)
    }
    pub fn used_value_path(&self) -> MainResult<PathBuf> {
        let root = self
            .root
            .clone()
            .ok_or(MainReason::from(ElementReason::Miss("local-path".into())).to_err())?;
        //let value_path = ensure_path(local.join(VALUE_DIR)).owe_logic()?;
        let value_file = root.join(USED_JSON);
        Ok(value_file)
    }
}

#[async_trait]
impl ModuleLocalizable<ModValuePaths> for MMOperator {
    async fn mod_localize(
        &self,
        val_path: ModValuePaths,
        options: LocalizeOptions,
    ) -> MainResult<()> {
        let mut ctx = OperationContext::want("mod1 localize")
            .with_auto_log()
            .with_mod_path("mod");
        let mod_root = self
            .root
            .clone()
            .ok_or(MainReason::from(ElementReason::Miss("local-path".into())).to_err())?;
        ctx.record("model", self.model().to_string());

        //let value_paths = TargetValuePaths::from(&val_path);
        let local_path = mod_root.join(LOCAL_DIR);
        ctx.record("local", local_path.display());
        debug!( target:"spec/mod/target", "localize mod-target begin: {}" ,local_path.display() );
        make_clean_path(&local_path).source_logic()?;

        let used = self.build_used_value(options, &val_path.mod_value_file())?;
        orion_conf::TextConfigIO::save_valconf(&used.export_origin(), &val_path.used_with_origon())
            .source_resource()?;
        let used_value_file = self.used_value_path()?;
        ctx.record("value_file", used_value_file.display());
        orion_conf::JsonIO::save_json(&used.export_value(), &used_value_file).source_resource()?;

        debug!(target : "/mod/target/loc", "use value: {}", used_value_file.display());
        let tpl_path_opt = self
            .setting
            .as_ref()
            .and_then(|x| x.localize().clone())
            .and_then(|x| x.templatize_path().clone())
            .map(|x| x.export_paths(&mod_root));

        let tpl_path = tpl_path_opt.unwrap_or_default();
        let tpl_custom = self
            .setting
            .as_ref()
            .and_then(|x| x.localize().clone())
            .and_then(|x| x.templatize_cust().clone())
            .map(TemplateConfig::from);

        let localizer = if let Some(cust) = tpl_custom {
            LocalizeTemplate::new(cust)
        } else {
            LocalizeTemplate::default()
        };
        let spec_tpl = mod_root.join(crate::const_vars::SPEC_DIR);
        localizer
            .render_path(&spec_tpl, &local_path, &used_value_file, &tpl_path)
            .with(&ctx)?;
        ctx.mark_suc();
        Ok(())
    }
}
