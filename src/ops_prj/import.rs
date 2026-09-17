use super::prelude::*;

use crate::{ops_prj::install::SystemPackageInstaller, system::operator::SysOperator};
use orion_variate::addr::Address;

use crate::{
    artifact::types::{build_pkg, convert_addr},
    const_vars::{SYS_VALUE_FILE, SYS_VARS_YML},
    error::MainResult,
    ops_prj::{project::OpsProject, system::OpsSystem},
    types::Accessor,
};

impl OpsProject {
    pub async fn import_sys(
        &mut self,
        accessor: Accessor,
        path: &str,
        up_opt: &DownloadOptions,
    ) -> MainResult<()> {
        // 1. 解析地址
        let addr = convert_addr(path);

        // 2. 更新到本地目录
        let work_path = PathBuf::from(
            "${HOME}/ds-package"
                .to_string()
                .env_eval(&ValueDict::default()),
        );

        let pkg_path = if let Address::Local(local) = addr.clone() {
            PathBuf::from(local.path())
        } else {
            let up_unit = accessor
                .download_to_local(&addr, &work_path, up_opt)
                .await
                .map_err(crate::error::MainReason::from_addr_error)?;
            up_unit.position().clone()
        };

        // 3. 创建安装器并准备包
        let installer = SystemPackageInstaller::new(self.paths().clone()).with_pkg_path(pkg_path);

        let package = build_pkg(path);
        let sys_src = installer.prepare_package(package)?;

        // 4. 导入到工作目录
        let ops_target_system = installer.install_system_package(&sys_src)?;

        let ops_sys = OpsSystem::new(ops_target_system.spec().define().clone(), addr);
        self.import_ops_sys(ops_sys);
        self.save()?;
        let sys_operator = SysOperator::load(&ops_target_system.installation_path)?;
        sys_operator.init_setting_value()?;
        // 5. 提供系统包的信息， 包组所有组件。
        Ok(())
    }
    pub fn ia_setting_interactive(&self) -> MainResult<()> {
        self.ia_setting(true)
    }

    pub fn process_system_vars(
        vars_path: &Path,
        value_path: &Path,
        system_name: &str,
        interactive: bool,
    ) -> MainResult<()> {
        use dialoguer::{Confirm, Input};

        let value_file = value_path.join(SYS_VALUE_FILE);

        let vars_vec = VarCollection::load_conf(vars_path).source_resource()?;
        let mut vals_dict = if value_file.exists() {
            ValueDict::load_conf(&value_file).source_resource()?
        } else {
            ValueDict::default()
        };

        // 通过交互模式设定vars的值
        println!("Setting variables for {system_name}");

        for var in vars_vec.system_vars() {
            if !var.is_mutable() {
                continue;
            }
            let prompt = if let Some(desp) = var.desc() {
                format!("{}\n{desp}", var.name())
            } else {
                var.name().to_string()
            };
            let mut default_value = var.value().clone();
            let value_str = if interactive {
                Input::new()
                    .with_prompt(&prompt)
                    .default(var.value().to_string())
                    .interact_text()
                    .source_data()?
            } else {
                // 非交互模式，如果已有值则保留，否则使用默认值
                if let Some(existing_value) = vals_dict.get(var.name()) {
                    existing_value.to_string()
                } else {
                    var.value().to_string()
                }
            };
            default_value
                .update_from_str(value_str.as_str())
                .source_data()?;
            vals_dict.insert(var.name().to_string(), default_value);
        }

        // 如果用户确认保存更改
        let should_save = if interactive {
            Confirm::new()
                .with_prompt("Do you want to save these changes?")
                .interact()
                .source_data()?
        } else {
            // 非交互模式，自动保存
            true
        };
        if should_save {
            // 保存修改后的vars到文件
            // vars.save_to_file(&vars_path)?; // 假设的方法
            println!("Changes saved to {}", value_file.display());
            orion_conf::ConfigIO::save_conf(&vals_dict, &value_file).source_resource()?;
        }
        Ok(())
    }

    pub fn ia_setting(&self, interactive: bool) -> MainResult<()> {
        for i in self.ops_target().iter() {
            let vars_path = self
                .root_local()
                .join(i.sys().name())
                .join("sys")
                .join(SYS_VARS_YML);

            let value_path = self.root_local().join("values").join(i.sys().name());
            ensure_path(&value_path).source_resource()?;

            Self::process_system_vars(&vars_path, &value_path, i.sys().name(), interactive)?;
        }
        Ok(())
    }
}
