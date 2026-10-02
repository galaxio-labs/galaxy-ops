use super::prelude::*;

use crate::ops_prj::system::OpsTargetSystem;
use crate::types::Accessor;
use fs_extra::dir::CopyOptions;
use orion_variate::addr::Address;
use orion_variate::archive::decompress;
use pathdiff::diff_paths;

use crate::{
    artifact::types::{PackageType, build_pkg, convert_addr},
    error::{MainError, MainReason, MainResult},
    infra::{ensure_download_dir, package_work_dir},
    ops_prj::path::ProjectPath,
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

/// 清空并重建解包目录。
///
/// 不用 `make_clean_path`：那个只报 “system error / create_dir_all”，把底层 io 原文吞了 ——
/// 在客户机上无法分辨是**权限 / 只读 / 磁盘满**，还是路径被同名文件（或符号链接）占着。
/// 这里把 io 原文带出来，并把“同名文件占位”也当作可处理的输入（删掉重建，而不是失败）。
fn prepare_clean_dir(dir: &Path) -> MainResult<()> {
    if let Ok(meta) = std::fs::symlink_metadata(dir) {
        let removed = if meta.is_dir() {
            std::fs::remove_dir_all(dir)
        } else {
            // 同名文件 / 符号链接：直接删（原实现会因 remove_dir_all(非目录) 而失败）
            std::fs::remove_file(dir)
        };
        removed.map_err(|e| clean_dir_err("清理解包目录", dir, &e))?;
    }
    std::fs::create_dir_all(dir).map_err(|e| clean_dir_err("创建解包目录", dir, &e))
}

fn clean_dir_err(action: &str, dir: &Path, e: &std::io::Error) -> MainError {
    MainReason::logic_detail(format!(
        "{action}失败：{}（{e}）—— 检查是否只读 / 磁盘满，或该路径被同名文件占用",
        dir.display()
    ))
}

/// 取包并解开：本地包直接解，远端包先下载到工作区再解。
///
/// 返回解包后的**包内容根目录**（内含 `sys/`、`sys-prj.yml` 等）。
/// 与 `import_sys` 的第一步完全一致，抽出来供非破坏性更新（`prj update`）与重建复用 ——
/// 三处各抄一遍取包逻辑，迟早会分叉。
///
/// 工作区取 [`package_work_dir`]（`GOPS_PACKAGE_DIR` 或平台缓存目录），不再硬编 `$HOME/ds-package`。
pub async fn fetch_and_prepare(
    project_paths: ProjectPath,
    addr: &str,
    accessor: Accessor,
    options: &DownloadOptions,
) -> MainResult<PathBuf> {
    let address = convert_addr(addr)?;
    let work_path = package_work_dir();
    // 落一行日志：客户报文里最常缺的就是“它到底往哪个目录写”
    debug!("package work dir: {}", work_path.display());
    // 先保证工作目录**真的是目录**：下载器在“目标不是已存在目录”时会把整条路径当文件名写，
    // 于是一个 `~/ds-package` 会被写成一个文件，之后所有解包都 ENOTDIR（见 `ensure_download_dir` 注释）。
    ensure_download_dir(&work_path)?;
    let pkg_path = if let Address::Local(local) = address.clone() {
        PathBuf::from(local.path())
    } else {
        let up_unit = accessor
            .download_to_local(&address, &work_path, options)
            .await
            .map_err(crate::error::MainReason::from_addr_error)?;
        up_unit.position().clone()
    };
    let installer = SystemPackageInstaller::new(project_paths).with_pkg_path(pkg_path);
    let package = build_pkg(addr)?;
    installer.prepare_package(package)
}

impl SystemPackageInstaller {
    pub fn new(project_paths: ProjectPath) -> Self {
        let work_dir = package_work_dir();
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
                // 防御式：万一调用方没走 `fetch_and_prepare`，工作目录也必须是目录
                ensure_download_dir(&self.work_paths.work_dir)?;
                let out_path = self.work_paths.work_dir.join(bin_package.name());
                prepare_clean_dir(&out_path)?;
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
        // 系统包可能自带空的 values/ 目录，需先移除，否则下面的符号链接会因目录已存在而失败
        if let Ok(meta) = std::fs::symlink_metadata(&sys_value) {
            if meta.file_type().is_symlink() || meta.is_file() {
                std::fs::remove_file(&sys_value).source_resource()?;
            } else if meta.is_dir() {
                std::fs::remove_dir_all(&sys_value).source_resource()?;
            }
        }
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

    use super::prepare_clean_dir;

    #[test]
    fn test_prepare_clean_dir_wipes_dir_and_replaces_file() {
        let tmp = TempDir::new().unwrap();

        // 已存在的目录 → 清空重建
        let d = tmp.path().join("out");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("stale.txt"), "x").unwrap();
        prepare_clean_dir(&d).unwrap();
        assert!(d.is_dir());
        assert!(!d.join("stale.txt").exists());

        // 同名**文件** → 删掉重建（原 `make_clean_path` 会失败）
        let f = tmp.path().join("asfile");
        std::fs::write(&f, "x").unwrap();
        prepare_clean_dir(&f).unwrap();
        assert!(f.is_dir());
    }

    #[test]
    fn test_prepare_clean_dir_error_carries_io_detail() {
        let tmp = TempDir::new().unwrap();
        // 父路径是文件 → create_dir_all ENOTDIR；报错必须带 io 原文与处置提示
        let parent = tmp.path().join("pfile");
        std::fs::write(&parent, "x").unwrap();

        let err = prepare_clean_dir(&parent.join("sub")).unwrap_err();
        let detail = err.detail().as_deref().unwrap_or_default();
        assert!(detail.contains("创建解包目录失败"), "detail={detail}");
        assert!(detail.contains("只读"), "应给出排查提示：{detail}");
    }

    /// 目标是**指向目录的符号链接**：只删链接、重建为目录，**不动链接指向的内容**。
    /// （否则一次清理就会把别人目录里的东西删光。）
    #[test]
    fn test_prepare_clean_dir_symlink_only_removes_the_link() {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("keep.txt"), "keep").unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        prepare_clean_dir(&link).unwrap();

        // link 现在是**真目录**（不再是链接）
        assert!(std::fs::symlink_metadata(&link).unwrap().is_dir());
        assert!(
            !std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        // 链接原来指向的目录内容完好
        assert_eq!(
            std::fs::read_to_string(real.join("keep.txt")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn test_prepare_clean_dir_creates_nested_from_scratch() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("a/b/c");
        prepare_clean_dir(&nested).unwrap();
        assert!(nested.is_dir());
    }

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
