use super::prelude::*;

use crate::system::SysKind;

use crate::{
    const_vars::{MOD_OPERATORS_ROOT, RESOLVED_VARS_YML},
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<ModelSTD>,
    #[serde(default, skip_serializing_if = "SysKind::is_gxl")]
    kind: SysKind,
    #[getset(set_with = "pub ")]
    vender: String,
}
impl SysDefine {
    pub fn new<S: Into<String>>(name: S, model: ModelSTD) -> Self {
        Self {
            name: name.into(),
            vender: String::new(),
            model: Some(model),
            kind: SysKind::Gxl,
        }
    }
    /// 无目标型号的系统（纯 docker-compose 系统，型号无意义）
    pub fn new_without_model<S: Into<String>>(name: S) -> Self {
        Self {
            name: name.into(),
            vender: String::new(),
            model: None,
            kind: SysKind::Gxl,
        }
    }
    pub fn with_kind(mut self, kind: SysKind) -> Self {
        self.kind = kind;
        self
    }
    pub fn is_docker_compose(&self) -> bool {
        self.kind == SysKind::DockerCompose
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

    /// 纯 docker-compose 系统的精简保存：只写 sys_model.yml + setting/vars.yml，
    /// 不写 mod_list.yml / workflows / setting/list.yml（这些对纯 compose 系统多余）。
    /// 已存在的文件不覆盖（幂等，用于 `sys new` 支持已存在的目录）。
    pub fn save_local_minimal(&self, path: &Path, name: &str) -> MainResult<()> {
        let root = path.join(name);
        let paths = SysTargetPaths::from(&root);
        std::fs::create_dir_all(paths.spec_path()).source_conf()?;
        if !paths.define_path().exists() {
            self.define
                .save_yaml(paths.define_path())
                .source_resource()?;
        }
        ensure_path(&paths.setting_path()).source_resource()?;
        self.setting()
            .save_local_vars_only_if_absent(paths.setting_path())?;
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
        // mod_list.yml 可选：缺失时视为空模块列表（例如纯 docker-compose 系统）
        let mut mod_list = if paths.modlist_path().exists() {
            ModulesList::load_yaml(paths.modlist_path())
                .with("load mod-list".to_string())
                .with(&ctx)
                .source_data()?
        } else {
            ModulesList::default()
        };
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
            let path = local.join(RESOLVED_VARS_YML);
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
        let setting = SysSetting::new(VarCollection::define(vec![
            VarDefinition::from(("SERVICE_IMAGE", "nginx:alpine")).with_mut_system(),
            VarDefinition::from(("SERVICE_PORT", 8080u64)).with_mut_system(),
            VarDefinition::from(("REPLICAS", 1u64)).with_mut_system(),
        ]));
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn minimal_spec() -> MainResult<SysModelSpec> {
        SysModelSpec::make_new(SysDefine::new_without_model("web-stack"))
    }

    #[test]
    fn test_save_local_minimal_writes_expected_files() -> MainResult<()> {
        let temp_dir = tempdir().unwrap();
        let spec = minimal_spec()?;
        spec.save_local_minimal(temp_dir.path(), "sys")?;

        let sys = temp_dir.path().join("sys");
        assert!(sys.join("sys_model.yml").exists());
        assert!(sys.join("setting/vars.yml").exists());
        // 纯 compose 不生成 GXL 相关文件
        assert!(!sys.join("mod_list.yml").exists());
        assert!(!sys.join("setting/list.yml").exists());
        assert!(!sys.join("workflows").exists());
        Ok(())
    }

    #[test]
    fn test_save_local_minimal_is_idempotent() -> MainResult<()> {
        let temp_dir = tempdir().unwrap();
        let spec = minimal_spec()?;
        spec.save_local_minimal(temp_dir.path(), "sys")?;

        // 模拟用户已有自定义内容
        let define_path = temp_dir.path().join("sys/sys_model.yml");
        let vars_path = temp_dir.path().join("sys/setting/vars.yml");
        std::fs::write(&define_path, "# user-custom define\n").unwrap();
        std::fs::write(&vars_path, "# user-custom vars\n").unwrap();

        // 再次保存不覆盖已有文件
        spec.save_local_minimal(temp_dir.path(), "sys")?;
        assert_eq!(
            std::fs::read_to_string(&define_path).unwrap(),
            "# user-custom define\n"
        );
        assert_eq!(
            std::fs::read_to_string(&vars_path).unwrap(),
            "# user-custom vars\n"
        );
        Ok(())
    }
}
