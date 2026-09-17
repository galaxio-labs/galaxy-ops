use super::prelude::*;

use crate::{ops_prj::install::SystemPackageInstaller, system::operator::SysOperator};
use orion_variate::addr::Address;

use crate::{
    artifact::types::{build_pkg, convert_addr},
    const_vars::{MERGED_VARS_YML, SYS_VALUE_FILE, SYS_VARS_YML},
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
        let addr = convert_addr(path)?;

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

        let package = build_pkg(path)?;
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

    /// 重新导入：按 `ops-prj.yml` 里记录的 `sys_models` 重新导入系统，保留 `values/` 客户值。
    ///
    /// 适用于“删除了已导入的系统目录，但保留了 values/ + ops-prj.yml”的场景。
    pub async fn reimport(
        &mut self,
        accessor: Accessor,
        options: &DownloadOptions,
    ) -> MainResult<()> {
        // 先收集（系统名，addr 反推的路径字符串），避免迭代借用与 &mut self 冲突
        let targets: Vec<(String, String)> = self
            .conf()
            .sys_models()
            .iter()
            .map(|sys| (sys.sys().name().clone(), addr_to_path_string(sys.addr())))
            .collect();

        for (name, path) in targets {
            // 移除已有系统目录（values/ 在外层，不受影响）
            let sys_dir = self.paths().root().join(&name);
            if sys_dir.exists() {
                std::fs::remove_dir_all(&sys_dir).source_resource()?;
            }
            self.import_sys(accessor.clone(), &path, options).await?;
        }
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
        for i in self.conf().sys_models().iter() {
            let sys_dir = self.root_local().join(i.sys().name()).join("sys");
            // 兼容旧名：merged_vars.yml 优先，缺失时回退 sys_vars.yml
            let new_vars_path = sys_dir.join(MERGED_VARS_YML);
            let legacy_vars_path = sys_dir.join(SYS_VARS_YML);
            let vars_path = if new_vars_path.exists() {
                new_vars_path
            } else if legacy_vars_path.exists() {
                legacy_vars_path
            } else {
                new_vars_path
            };

            let value_path = self.root_local().join("values").join(i.sys().name());
            ensure_path(&value_path).source_resource()?;

            Self::process_system_vars(&vars_path, &value_path, i.sys().name(), interactive)?;
        }
        Ok(())
    }
}

/// 把 `Address` 反推回可用于 `convert_addr` / `build_pkg` 的路径字符串。
/// 用于 reimport：从 ops-prj.yml 记录的 addr 重新导入系统。
fn addr_to_path_string(addr: &Address) -> String {
    match addr {
        Address::Local(local) => local.path().clone(),
        Address::Git(git) => git.repo().clone(),
        Address::Http(http) => http.url().clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_variate::addr::{GitRepository, HttpResource, LocalPath};

    #[test]
    fn test_addr_to_path_string() {
        let local = Address::Local(LocalPath::from("/tmp/foo.tar.gz"));
        assert_eq!(addr_to_path_string(&local), "/tmp/foo.tar.gz");

        let git = Address::Git(GitRepository::from("https://github.com/x/y.git"));
        assert_eq!(addr_to_path_string(&git), "https://github.com/x/y.git");

        let http = Address::Http(HttpResource::from("https://x.com/y.tar.gz"));
        assert_eq!(addr_to_path_string(&http), "https://x.com/y.tar.gz");
    }
}
