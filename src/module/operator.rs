use super::prelude::*;
use core::str;
use orion_infra::path::PathResult;
use orion_variate::addr::HttpResource;
use orion_vars::vars::VarToValue;

use crate::const_vars::{
    EXAMPLE_GIT_URL, MOD_PRJ_CONF_FILE_V1, MOD_PRJ_CONF_FILE_V2, MOD_PRJ_TEST_ROOT, MOD_VALUE_FILE,
    SYS_VALUE_FILE, USED_READABLE_FILE, VALUE_DIR,
};
use crate::module::init::MOD_PRJ_ROOT_FILE;
use crate::types::{Accessor, RefUpdateable};

use super::init::{MOD_PRJ_ADM_GXL, MOD_PRJ_WORK_GXL, mod_init_gitignore};
use crate::const_vars::MOD_OPERATORS_ROOT;
use crate::module::{
    depend::{Dependency, DependencySet},
    spec::ModuleSpec,
};

#[derive(Getters, Clone, Debug, Serialize, Deserialize)]
pub struct ModConf {
    test_envs: DependencySet,
}

#[derive(Getters, Clone, Debug, Serialize, Deserialize)]
#[getset(get = "pub")]
pub struct ModCodePaths {
    root: PathBuf,
}
impl From<PathBuf> for ModCodePaths {
    fn from(value: PathBuf) -> Self {
        Self { root: value }
    }
}

#[derive(Getters, Clone, Debug, Serialize, Deserialize)]
#[getset(get = "pub")]
pub struct ModValuePaths {
    root: PathBuf,
}
impl From<PathBuf> for ModValuePaths {
    fn from(value: PathBuf) -> Self {
        Self { root: value }
    }
}

impl From<String> for ModValuePaths {
    fn from(value: String) -> Self {
        Self {
            root: PathBuf::from(value),
        }
    }
}
impl ModValuePaths {
    pub fn sys_value_file(&self) -> PathBuf {
        self.root.join(SYS_VALUE_FILE)
    }
    pub fn mod_value_file(&self) -> PathBuf {
        self.root.join(MOD_VALUE_FILE)
    }
    pub fn used_with_origon(&self) -> PathBuf {
        self.root.join(USED_READABLE_FILE)
    }
    pub fn join<S: AsRef<str>>(self, path: S) -> Self {
        Self {
            root: self.root.join(path.as_ref()),
        }
    }
    pub fn ensure_join<S: AsRef<str>>(self, path: S) -> PathResult<Self> {
        Ok(Self {
            root: ensure_path(self.root.join(path.as_ref()))?,
        })
    }
}
#[derive(Getters, Clone, Debug)]
#[getset(get = "pub")]
pub struct ModOperator {
    conf: ModConf,
    mod_spec: ModuleSpec,
    project: GxlProject,
    root_local: PathBuf,
}
impl ModConf {
    pub fn new(local_res: DependencySet) -> Self {
        Self {
            test_envs: local_res,
        }
    }
}
impl ModOperator {
    pub fn new(spec: ModuleSpec, local_res: DependencySet, root_local: PathBuf) -> Self {
        let conf = ModConf::new(local_res);
        let mut val_dict = ValueDict::default();
        val_dict.insert("TEST_WORK_ROOT", ValueType::from(MOD_PRJ_TEST_ROOT));
        Self {
            conf,
            mod_spec: spec,
            project: GxlProject::from((MOD_PRJ_WORK_GXL, MOD_PRJ_ADM_GXL, MOD_PRJ_ROOT_FILE)),
            root_local,
        }
    }

    pub fn init_setting_value(&self) -> MainResult<ModValuePaths> {
        let value_root = ModValuePaths::from(self.root_local().clone());
        let value_root = value_root.ensure_join(VALUE_DIR).source_logic()?;
        for (name, model) in self.mod_spec.targets() {
            let model_value_path = value_root
                .clone()
                .ensure_join(name.to_string())
                .source_logic()?;
            let model_sys_value = model_value_path.sys_value_file();
            let model_mod_value = model_value_path.mod_value_file();
            if !model_sys_value.exists() {
                let sys_vars = model.vars().system_vars().to_val();
                orion_conf::ConfigIO::save_conf(&sys_vars, &model_sys_value).source_resource()?;
                let mod_vars = model.vars().module_vars().to_val();
                orion_conf::ConfigIO::save_conf(&mod_vars, &model_mod_value).source_resource()?;
            }
        }
        Ok(value_root)
    }
    pub fn load(root_local: &Path) -> MainResult<Self> {
        let mut flag = auto_exit_log!(
            info!(
                target : "/mod_prj",
                 "load mod-prj  to {} success!", root_local.display()
            ),
            error!(
                target : "/mod_prj",
                "load mod-prj  to {} fail!", root_local.display()
            )
        );

        let conf_file_v1 = root_local.join(MOD_PRJ_CONF_FILE_V1);
        let conf_file_v2 = root_local.join(MOD_PRJ_CONF_FILE_V2);
        if conf_file_v1.exists() {
            std::fs::rename(&conf_file_v1, &conf_file_v2).source_resource()?;
        };
        let conf = ModConf::load_conf(&conf_file_v2).source_logic()?;
        let root_local = root_local.to_path_buf();
        let mod_spec = ModuleSpec::load_from(&root_local).owe(ModReason::Load.into())?;
        let project = GxlProject::load_from(&root_local).owe(ModReason::Load.into())?;
        flag.mark_suc();
        Ok(Self {
            conf,
            mod_spec,
            project,
            root_local,
        })
    }
    pub fn save(&self) -> MainResult<()> {
        let mut flag = auto_exit_log!(
            info!(
                target : "spec/local/modprj",
                 "save modprj  to {} success!", self.root_local().display()
            ),
            error!(
               target : "spec/local/modprj",
               "save modprj  to {} fail!", self.root_local().display()
            )
        );
        let conf_file = self.root_local().join("mod-prj.yml");
        orion_conf::ConfigIO::save_conf(&self.conf, &conf_file).source_resource()?;
        self.mod_spec
            .save_to(self.root_local(), Some("./".into()))
            .owe(ModReason::Save.into())?;
        self.project
            .save_to(self.root_local(), None)
            .owe(ModReason::Save.into())?;
        mod_init_gitignore(self.root_local())?;
        flag.mark_suc();
        Ok(())
    }
}

#[async_trait]
impl RefUpdateable<()> for ModConf {
    async fn update_local(
        &self,
        accessor: Accessor,
        _path: &Path,
        options: &DownloadOptions,
    ) -> MainResult<()> {
        self.test_envs
            .update_local(accessor, _path, options)
            .await
            .with(("mod-conf", "update test envs"))
    }
}

#[async_trait]
impl RefUpdateable<()> for ModOperator {
    async fn update_local(
        &self,
        accessor: Accessor,
        _path: &Path,
        options: &DownloadOptions,
    ) -> MainResult<()> {
        self.conf
            .update_local(accessor.clone(), _path, options)
            .await?;
        self.mod_spec()
            .update_local(accessor, self.root_local(), options)
            .await
            .with(("mod-operator", "update module spec"))?;
        Ok(())
    }
}

#[async_trait]
impl ModuleLocalizable<ModValuePaths> for ModConf {
    async fn mod_localize(
        &self,
        _dst_path: ModValuePaths,
        _options: LocalizeOptions,
    ) -> MainResult<()> {
        Ok(())
    }
}

#[async_trait]
impl ModuleLocalizable<ModValuePaths> for ModOperator {
    async fn mod_localize(
        &self,
        val_path: ModValuePaths,
        options: LocalizeOptions,
    ) -> MainResult<()> {
        self.conf
            .mod_localize(val_path.clone(), options.clone())
            .await?;
        self.mod_spec().mod_localize(val_path, options).await?;
        Ok(())
    }
}
impl ModOperator {
    pub fn make_new(prj_path: &Path, name: &str) -> MainResult<Self> {
        let mod_spec = ModuleSpec::make_new(name)?;
        let res = DependencySet::default();
        Ok(ModOperator::new(mod_spec, res, prj_path.to_path_buf()))
    }
    pub fn make_test_prj(name: &str) -> MainResult<Self> {
        let prj_path = PathBuf::from(MOD_OPERATORS_ROOT).join(name);
        make_clean_path(&prj_path).source_logic()?;
        let proj = ModOperator::make_new(&prj_path, name)?;
        proj.save()?;
        Ok(proj)
    }
}

pub fn make_mod_prj_testins(prj_path: &Path) -> MainResult<ModOperator> {
    let mod_spec = ModuleSpec::for_example();
    let mut res = DependencySet::default();
    res.push(
        Dependency::new(
            Address::from(HttpResource::from(EXAMPLE_GIT_URL)),
            PathTemplate::from(prj_path.join("test_res")),
        )
        .with_rename("bit-common"),
    );
    Ok(ModOperator::new(mod_spec, res, prj_path.to_path_buf()))
}

#[cfg(test)]
pub mod tests {
    use super::super::prelude::*;
    use crate::{
        accessor::accessor_for_test,
        module::{
            depend::{Dependency, DependencySet},
            spec::ModuleSpec,
        },
        types::{LocalizeOptions, ModuleLocalizable, RefUpdateable},
    };
    use std::path::PathBuf;

    use orion_error::dev::testing::TestAssertWithMsg;
    use orion_infra::path::make_clean_path;
    use orion_variate::{
        addr::{Address, LocalPath, types::PathTemplate},
        tools::test_init,
        update::DownloadOptions,
    };
    use orion_vars::vars::OriginDict;

    use crate::{const_vars::MOD_OPERATORS_ROOT, module::operator::ModOperator};
    #[tokio::test]
    async fn test_mod_prj_new() -> MainResult<()> {
        test_init();
        let prj_path = PathBuf::from(MOD_OPERATORS_ROOT).join("mod-new");
        make_clean_path(&prj_path).source_logic()?;
        let proj = ModOperator::make_new(&prj_path, "mod_new")?;
        proj.save()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_mod_prj_example() -> MainResult<()> {
        test_init();

        let prj_path = PathBuf::from(MOD_OPERATORS_ROOT).join("postgresql");
        if prj_path.exists() {
            std::fs::remove_dir_all(&prj_path).assert("ok");
        }
        std::fs::create_dir_all(&prj_path).assert("yes");
        let dep_src = prj_path.join("fixture-source");
        std::fs::create_dir_all(&dep_src).assert("create fixture dir");
        std::fs::write(dep_src.join("README.md"), "fixture").assert("write fixture");

        let mut res = DependencySet::default();
        res.push(
            Dependency::new(
                Address::from(LocalPath::from(dep_src.to_str().unwrap())),
                PathTemplate::from(prj_path.join("test_res")),
            )
            .with_rename("bit-common"),
        );
        let project = ModOperator::new(ModuleSpec::for_example(), res, prj_path.clone());
        project.save().assert("save dss_prj");
        let operator = ModOperator::load(&prj_path).assert("dss-project");
        let accessor = accessor_for_test();
        operator
            .update_local(accessor, &prj_path, &DownloadOptions::default())
            .await
            .assert("spec.update_local");
        assert!(prj_path.join("test_res").join("bit-common").exists());

        let value_path = operator.init_setting_value()?;
        operator
            .mod_localize(value_path, LocalizeOptions::new(OriginDict::new()))
            .await
            .assert("spec.localize");
        Ok(())
    }
}
