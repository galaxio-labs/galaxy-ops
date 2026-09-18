use crate::internal_prelude::*;

use crate::system::SysValuePaths;
use crate::types::{LocalizeOptions, SystemLocalizable};
use crate::{
    const_vars::VARS_YML,
    localize::{LocalizeExecPath, LocalizeVarPath},
    project::mix_used_value,
    types::ModuleLocalizable,
};

use async_trait::async_trait;
use indexmap::IndexMap;
use orion_vars::vars::EnvEvalable;

use crate::{error::MainResult, module::ModelSTD};

#[derive(Getters, Clone, Debug, Serialize, Deserialize)]
#[getset(get = "pub ")]
pub struct ModSetting {
    enable: bool,
    localize: LocalizeVarPath,
}
impl ModSetting {
    pub fn disaple_new(module: &str, model: &str) -> Self {
        Self {
            enable: false,
            localize: LocalizeVarPath::of_module(module, model),
        }
    }
    pub fn enable_new(module: &str, model: &str) -> Self {
        Self {
            enable: true,
            localize: LocalizeVarPath::of_module(module, model),
        }
    }
}
#[derive(Getters, Clone, Debug, Default, Serialize, Deserialize)]
#[getset(get = "pub ")]
#[serde(transparent)]
pub struct LocalizeDict {
    dicts: IndexMap<String, ModSetting>,
}
impl LocalizeDict {
    pub fn example() -> Self {
        let mut dicts = IndexMap::new();
        dicts.insert(
            "example".to_string(),
            ModSetting::disaple_new("example", ModelSTD::x86_ubt22_host().to_string().as_str()),
        );

        Self { dicts }
    }
}
impl LoadHook for LocalizeDict {
    fn loaded_event_do(&mut self) {}
}

#[derive(Getters, Clone, Debug)]
#[getset(get = "pub ")]
pub struct SysSetting {
    vars: VarCollection,
    list: LocalizeDict,
    root: Option<PathBuf>,
}
impl LoadHook for SysSetting {
    fn loaded_event_do(&mut self) {
        self.vars.mark_vars_scope();
    }
}
impl SysSetting {
    fn finalize_loaded(mut self) -> Self {
        self.loaded_event_do();
        self
    }

    pub fn new(vars: VarCollection) -> Self {
        SysSetting {
            vars,
            list: LocalizeDict::example(),
            root: None,
        }
    }
    pub fn add_mod_setting<S: Into<String>>(&mut self, mod_name: S, mod_setting: ModSetting) {
        self.list.dicts.insert(mod_name.into(), mod_setting);
    }

    pub fn example() -> Self {
        SysSetting {
            vars: VarCollection::define(vec![
                VarDefinition::from(("HOME", "${HOME}")).with_mut_immutable(),
                VarDefinition::from(("SYS_KEY1", "sys_value1")).with_mut_module(),
                VarDefinition::from(("SYS_KEY2", "sys_value2")).with_mut_system(),
            ]),
            list: LocalizeDict::example(),
            root: None,
        }
    }
    pub fn save_local(&self, path: &Path) -> MainResult<()> {
        self.save_local_with(path, true)
    }
    /// 只写 system 变量定义（vars.yml），不写按模块的本地化列表（list.yml）。
    /// 纯 docker-compose 系统没有模块，list.yml 多余。
    pub fn save_local_vars_only(&self, path: &Path) -> MainResult<()> {
        self.save_local_with(path, false)
    }
    /// 同 `save_local_vars_only`，但已存在的 vars.yml 不覆盖（幂等）。
    pub fn save_local_vars_only_if_absent(&self, path: &Path) -> MainResult<()> {
        let vars_file = path.join(VARS_YML);
        if vars_file.exists() {
            return Ok(());
        }
        self.save_local_with(path, false)
    }
    fn save_local_with(&self, path: &Path, with_list: bool) -> MainResult<()> {
        let vars_file_name = path.join(VARS_YML);
        self.vars.save_yaml(&vars_file_name).source_resource()?;
        if with_list {
            let list_file_name = path.join("list.yml");
            self.list.save_yaml(&list_file_name).source_resource()?;
        }
        Ok(())
    }
    pub fn load_from(root: &Path) -> MainResult<Self> {
        let vars_file_name = root.join(VARS_YML);
        let list_file_name = root.join("list.yml");
        let vars = VarCollection::load_yaml(&vars_file_name).source_resource()?;
        // list.yml 可选：缺失时视为空本地化列表（纯 docker-compose 等场景）
        let list = if list_file_name.exists() {
            LocalizeDict::load_yaml(&list_file_name).source_resource()?
        } else {
            LocalizeDict::default()
        };
        let root = Some(root.to_path_buf());
        Ok(SysSetting { vars, list, root }.finalize_loaded())
    }
}

#[async_trait]
impl SystemLocalizable<SysValuePaths> for SysSetting {
    async fn sys_localize(
        &self,
        val_path: SysValuePaths,
        options: LocalizeOptions,
    ) -> MainResult<()> {
        let used_value_file = self
            .root()
            .clone()
            .expect("setting root miss")
            .join("_used.json");
        for (_k, v) in self.list.dicts() {
            if !v.enable {
                continue;
            }
            let cur_used_file = used_value_file.clone();
            let mut ctx = OperationContext::want("sys-setting localize")
                .with_auto_log()
                .with_mod_path("sys/setting");
            let dict = options.clone().evaled_value().export_dict();
            let exe_setting = LocalizeExecPath::from(v.localize.clone().env_eval(&dict));

            let used = mix_used_value(options.clone(), &self.vars, &val_path.mod_value_file())?;
            used.export_origin()
                .save_yaml(&val_path.used_with_origon())
                .source_resource()?;
            ctx.record("value_file", cur_used_file.display());
            orion_conf::JsonIO::save_json(&used.export_value(), &cur_used_file)
                .source_resource()?;

            exe_setting
                .mod_localize(cur_used_file, options.clone())
                .await?;
            ctx.mark_suc();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_error::dev::testing::TestAssert;
    use orion_vars::vars::Mutability;
    use tempfile::tempdir;

    #[test]
    fn test_load_from_marks_var_scopes() {
        let temp_dir = tempdir().unwrap();
        let setting = SysSetting::example();

        setting.save_local(temp_dir.path()).assert();
        let loaded = SysSetting::load_from(temp_dir.path()).assert();

        assert_eq!(
            loaded.vars().immutable_vars()[0].mutability(),
            &Mutability::Immutable
        );
        assert_eq!(
            loaded.vars().system_vars()[0].mutability(),
            &Mutability::System
        );
        assert_eq!(
            loaded.vars().module_vars()[0].mutability(),
            &Mutability::Module
        );
    }

    #[test]
    fn test_save_local_vars_only_if_absent_writes_when_missing() {
        let temp_dir = tempdir().unwrap();
        let setting = SysSetting::example();

        setting
            .save_local_vars_only_if_absent(temp_dir.path())
            .assert();

        // vars.yml 已生成，list.yml 不生成
        assert!(temp_dir.path().join("vars.yml").exists());
        assert!(!temp_dir.path().join("list.yml").exists());
    }

    #[test]
    fn test_save_local_vars_only_if_absent_preserves_existing() {
        let temp_dir = tempdir().unwrap();
        // 预置一个“已存在”的 vars.yml
        std::fs::write(temp_dir.path().join("vars.yml"), "# user custom\n").unwrap();

        let setting = SysSetting::example();
        setting
            .save_local_vars_only_if_absent(temp_dir.path())
            .assert();

        let content = std::fs::read_to_string(temp_dir.path().join("vars.yml")).unwrap();
        assert_eq!(content, "# user custom\n");
    }
}
