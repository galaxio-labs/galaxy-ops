use super::prelude::*;

use crate::ops_prj::system::OpsTargetSystem;
use fs_extra::dir::CopyOptions;
use orion_variate::archive::decompress;
use pathdiff::diff_paths;

use crate::{
    artifact::types::PackageType, error::MainResult, ops_prj::path::ProjectPath,
    system::spec::SysModelSpec,
};
#[derive(Debug, Clone)]
pub struct PackageWorkingPaths {
    pub work_dir: PathBuf,
    pub pkg_path: PathBuf,
}

impl PackageWorkingPaths {
    pub fn new(work_dir: PathBuf, pkg_path: PathBuf) -> Self {
        Self { work_dir, pkg_path }
    }
}

#[derive(Clone)]
pub struct SystemPackageInstaller {
    project_paths: ProjectPath,
    work_paths: PackageWorkingPaths,
    copy_options: CopyOptions,
}

impl SystemPackageInstaller {
    pub fn new(project_paths: ProjectPath) -> Self {
        let work_dir = PathBuf::from(
            "${HOME}/ds-package"
                .to_string()
                .env_eval(&ValueDict::default()),
        );
        Self {
            project_paths,
            work_paths: PackageWorkingPaths::new(work_dir, PathBuf::new()),
            copy_options: CopyOptions::new(),
        }
    }

    pub fn with_pkg_path(mut self, pkg_path: PathBuf) -> Self {
        self.work_paths.pkg_path = pkg_path;
        self
    }

    pub fn prepare_package(&self, package: PackageType) -> MainResult<PathBuf> {
        match package {
            PackageType::Bin(bin_package) => {
                let out_path = self.work_paths.work_dir.join(bin_package.name());
                make_clean_path(&out_path).source_resource()?;
                decompress(&self.work_paths.pkg_path, out_path.clone())
                    .source_sys()
                    .want("decompress tar.gz")
                    .with(self.work_paths.pkg_path.display().to_string())?;
                Ok(out_path)
            }
            PackageType::Git(_git_package) => Ok(self.work_paths.pkg_path.to_path_buf()),
        }
    }

    pub fn install_system_package(&self, sys_src: &Path) -> MainResult<OpsTargetSystem> {
        let sys_spec = SysModelSpec::load_from(&sys_src.join("sys"))?;

        let paths = self.prepare_installation_paths(sys_src, sys_spec.define().name())?;
        self.move_and_rename_system(sys_src, &paths)?;
        Ok(crate::ops_prj::system::OpsTargetSystem::new(
            paths.final_target_path,
            sys_spec.clone(),
        ))
    }

    fn prepare_installation_paths(
        &self,
        sys_src: &Path,
        sys_name: &str,
    ) -> MainResult<crate::ops_prj::path::InstallationPaths> {
        if let Some(last_name) = sys_src.iter().next_back() {
            let sys_dst_path = self.project_paths.root().join(last_name);
            let sys_new_path = self.project_paths.root().join(sys_name);
            Ok(crate::ops_prj::path::InstallationPaths {
                source_path: sys_src.to_path_buf(),
                temp_target_path: sys_dst_path,
                final_target_path: sys_new_path,
                project_root: self.project_paths.root().to_path_buf(),
                value_path: self.project_paths.value_dir().join(sys_name),
            })
        } else {
            Err(crate::error::MainReason::conf_detail(format!(
                "import package failed, bad path: {}",
                sys_src.display()
            )))
        }
    }

    fn move_and_rename_system(
        &self,
        sys_src: &Path,
        paths: &crate::ops_prj::path::InstallationPaths,
    ) -> MainResult<()> {
        let mut ctx = OperationContext::want("move&rename sys").with_auto_log();
        ctx.record("src", sys_src.display());
        ctx.record("src", paths.project_root.display());
        fs_extra::dir::move_dir(sys_src, &paths.project_root, &self.copy_options)
            .source_resource()?;

        ctx.record("temp", paths.temp_target_path.display());
        ctx.record("fianl", paths.final_target_path.display());
        std::fs::rename(&paths.temp_target_path, &paths.final_target_path).source_resource()?;
        ctx.record("prj-values", paths.value_path.display());
        std::fs::create_dir_all(&paths.value_path)
            .source_resource()
            .want("crate")?;
        let sys_value = paths.final_target_path.join("values");
        ctx.record("sys-values", sys_value.display());
        if let Some(link_target) = diff_paths(&paths.value_path, &paths.final_target_path) {
            std::os::unix::fs::symlink(&link_target, &sys_value).source_resource()?;
        }
        ctx.mark_suc();
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use crate::ops_prj::path::InstallationPaths;

    #[test]
    fn test_installation_paths() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let sys_src = root.join("source");
        let sys_dst_path = root.join("temp_name");
        let sys_new_path = root.join("final_name");

        let paths = InstallationPaths {
            source_path: sys_src.clone(),
            temp_target_path: sys_dst_path.clone(),
            final_target_path: sys_new_path.clone(),
            project_root: root.to_path_buf(),
            value_path: root.join("values").join("final_name"),
        };

        // Test that all paths are accessible
        assert_eq!(paths.source_path, sys_src);
        assert_eq!(paths.temp_target_path, sys_dst_path);
        assert_eq!(paths.final_target_path, sys_new_path);
        assert_eq!(paths.project_root, root);
    }
}
