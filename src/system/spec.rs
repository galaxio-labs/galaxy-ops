use super::prelude::*;

use crate::{
    const_vars::{MOD_OPERATORS_ROOT, SYS_VARS_YML},
    error::ElementReason,
    module::operator::ModOperator,
    system::setting::ModSetting,
    types::SystemLocalizable,
    workflow::act::SysWorkflows,
};
use orion_variate::addr::{GitRepository, LocalPath};
use orion_vars::vars::VarDefinition;

use super::init::{SysIniter, sys_init_gitignore};
use crate::{
    error::{MainReason, MainResult},
    module::{CpuArch, ModelSTD, OsCPE, RunSPC, refs::ModuleSpecRef, spec::ModuleSpec},
};

#[derive(Clone, Debug, Serialize, Deserialize, Getters, WithSetters, PartialEq)]
#[getset(get = "pub ")]
pub struct SysDefine {
    name: String,
    model: ModelSTD,
    #[getset(set_with = "pub ")]
    vender: String,
}
impl SysDefine {
    pub fn new<S: Into<String>>(name: S, model: ModelSTD) -> Self {
        Self {
            name: name.into(),
            vender: String::new(),
            model,
        }
    }
}
#[derive(Getters, Clone, Debug, MutGetters)]
#[getset(get = "pub ", get_mut = "pub")]
pub struct SysModelSpec {
    define: SysDefine,
    mod_list: ModulesList,
    local: Option<PathBuf>,
    //#[serde(skip)]
    workflow: SysWorkflows,
    setting: SysSetting,
}

impl SysModelSpec {
    pub fn add_mod(&mut self, modx: ModuleSpec) {
        self.mod_list.add_mod(modx);
    }
    pub fn add_mod_ref(&mut self, modx: ModuleSpecRef) {
        self.mod_list.add_ref(modx)
    }
    pub fn save_to(&self, path: &Path) -> MainResult<()> {
        self.save_local(path, self.define.name())
    }
    pub fn save_local(&self, path: &Path, name: &str) -> MainResult<()> {
        let root = path.join(name);

        let mut flag = auto_exit_log!(
            info!(target: "sys", "save sys spec success!:{}", root.display()),
            error!(target: "sys", "save sys spec failed!:{}", root.display())
        );
        let paths = SysTargetPaths::from(&root);
        std::fs::create_dir_all(paths.spec_path()).source_conf()?;
        sys_init_gitignore(&root)?;
        self.define
            .save_yaml(paths.define_path())
            .source_resource()?;
        self.mod_list
            .save_yaml(paths.modlist_path())
            .source_resource()?;
        ensure_path(&paths.setting_path()).source_resource()?;
        self.setting().save_local(paths.setting_path())?;

        self.workflow
            .save_to(paths.workflow_path(), None)
            .source_logic()?;
        flag.mark_suc();
        Ok(())
    }

    pub fn load_from(root: &Path) -> MainResult<Self> {
        let mut ctx = WithContext::want("load syspec");
        let _name = root
            .file_name()
            .and_then(|f| f.to_str())
            .ok_or_else(|| MainReason::conf_detail("bad name"))?;

        let mut flag = auto_exit_log!(
            info!(target: "sys", "load sys spec success!:{}", root.display()),
            error!(target: "sys", "load sys spec failed!:{}", root.display())
        );
        let paths = SysTargetPaths::from(&root.to_path_buf());

        ctx.record("mod_list", paths.modlist_path().display());
        let define = if !paths.define_path().exists() {
            return Err(MainReason::logic_detail(format!(
                "miss define file: {}",
                paths.define_path().display()
            )));
        } else {
            SysDefine::load_yaml(paths.define_path())
                .with("load define".to_string())
                .with(&ctx)
                .source_data()?
        };
        let mut mod_list = ModulesList::load_yaml(paths.modlist_path())
            .with("load mod-list".to_string())
            .with(&ctx)
            .source_data()?;
        mod_list.set_mods_local(paths.spec_path().clone());
        let workflow = SysWorkflows::load_from(paths.workflow_path())
            .with(&ctx)
            .owe(SysReason::Load.into())?;
        let setting = SysSetting::load_from(paths.setting_path())?;
        flag.mark_suc();
        Ok(Self {
            define,
            mod_list,
            local: Some(root.to_path_buf()),
            workflow,
            setting,
        })
    }

    pub fn new(define: SysDefine, actions: SysWorkflows, setting: SysSetting) -> Self {
        Self {
            define,
            mod_list: ModulesList::default(),
            local: None,
            workflow: actions,
            //setting: SysSetting::example(),
            setting,
        }
    }
}
#[async_trait]
impl RefUpdateable<()> for SysModelSpec {
    async fn update_local(
        &self,
        accessor: Accessor,
        _path: &Path,
        options: &DownloadOptions,
    ) -> MainResult<()> {
        if let Some(local) = &self.local {
            let value = self.mod_list.update_local(accessor, local, options).await?;
            let path = local.join(SYS_VARS_YML);
            if path.exists() {
                std::fs::remove_file(&path).source_sys()?;
            }
            let sys_vars = value.vars.merge_system(self.setting().vars().clone());
            sys_vars.save_yaml(&path).source_resource()?;
            Ok(())
        } else {
            MainReason::from(ElementReason::Miss("local path".into())).err_result()
        }
    }
}

#[async_trait]
impl SystemLocalizable<SysValuePaths> for SysModelSpec {
    async fn sys_localize(
        &self,
        val_path: SysValuePaths,
        options: LocalizeOptions,
    ) -> MainResult<()> {
        if let Some(_local) = &self.local {
            self.mod_list
                .sys_localize(val_path.clone(), options.clone())
                .await?;
            self.setting
                .sys_localize(val_path.join("setting"), options)
                .await?;
            Ok(())
        } else {
            MainReason::from(ElementReason::Miss("local path".into())).err_result()
        }
    }
}
impl SysModelSpec {
    pub fn for_example(name: &str) -> MainResult<SysModelSpec> {
        ModOperator::make_test_prj("redis2_mock")?;
        ModOperator::make_test_prj("mysql2_mock")?;
        make_sys_spec_test(
            SysDefine::new(name, ModelSTD::from_cur_sys()),
            vec!["redis2_mock", "mysql2_mock"],
        )
    }

    pub fn make_new(define: SysDefine) -> MainResult<SysModelSpec> {
        let actions = SysWorkflows::sys_tpl_init();
        let setting = SysSetting::new(VarCollection::define(vec![]));
        let mut modul_spec = SysModelSpec::new(define.clone(), actions, setting);
        let mod_name = "you_mod1";

        modul_spec.add_mod_ref(
            ModuleSpecRef::from(
                mod_name,
                GitRepository::from("https://github.com/you-mod1").with_tag("0.1.0"),
                ModelSTD::new(CpuArch::Arm, OsCPE::MAC14, RunSPC::Host),
            )
            .with_enable(false),
        );
        modul_spec.add_mod_ref(
            ModuleSpecRef::from(
                "you_mod2",
                GitRepository::from("https://github.com/you-mod2").with_branch("beta"),
                ModelSTD::new(CpuArch::Arm, OsCPE::MAC14, RunSPC::Host),
            )
            .with_enable(false),
        );
        modul_spec.add_mod_ref(
            ModuleSpecRef::from(
                "you_mod3",
                GitRepository::from("https://github.com/you-mod3").with_tag("v1.0.0"),
                ModelSTD::new(CpuArch::X86, OsCPE::UBT22, RunSPC::K8S),
            )
            .with_enable(false),
        );
        Ok(modul_spec)
    }
}

pub fn make_sys_spec_test(define: SysDefine, mod_names: Vec<&str>) -> MainResult<SysModelSpec> {
    let actions = SysWorkflows::sys_tpl_init();
    let setting = SysSetting::new(VarCollection::define(vec![
        VarDefinition::from(("HOME", "${HOME}")).with_mut_immutable(),
        VarDefinition::from(("SYS_KEY1", "sys_value1")).with_mut_module(),
        VarDefinition::from(("SYS_KEY2", "sys_value2")).with_mut_system(),
    ]));
    let mut modul_spec = SysModelSpec::new(define, actions, setting);
    for mod_name in mod_names {
        //let mod_name = "postgresql";
        let model = ModelSTD::new(CpuArch::Arm, OsCPE::MAC14, RunSPC::Host);
        modul_spec.add_mod_ref(ModuleSpecRef::from(
            mod_name,
            LocalPath::from(format!("{MOD_OPERATORS_ROOT}/{mod_name}").as_str()),
            model.clone(),
        ));
        modul_spec.setting_mut().add_mod_setting(
            mod_name,
            ModSetting::enable_new(mod_name, model.to_string().as_str()),
        );
    }

    Ok(modul_spec)
}
