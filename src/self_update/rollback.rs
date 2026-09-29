use super::prelude::*;
use std::fs;
use std::path::Path;

/// 自升级涉及的可执行文件（回滚/健康检查用）。
pub(crate) const SELF_UPDATE_BINARIES: &[&str] = &["gops"];

/// 用备份文件覆盖安装目录下的可执行文件（原子替换，可在二进制运行时回滚）。
pub(crate) fn rollback(install_dir: &Path, backup_dir: &Path) -> MainResult<()> {
    for bin in SELF_UPDATE_BINARIES {
        let live_path = install_dir.join(bin_name(bin));
        let backup_path = backup_dir.join(bin_name(bin));
        if !backup_path.exists() {
            return Err(MainReason::resource_detail(format!(
                "backup is incomplete: backup_dir={}, missing={}",
                backup_dir.display(),
                backup_path.display()
            )));
        }
        copy_file(&backup_path, &live_path)?;
    }
    Ok(())
}

/// 回滚后运行 `--version` 校验可执行文件可用。
pub(crate) fn health_check(install_dir: &Path) -> MainResult<()> {
    for bin in SELF_UPDATE_BINARIES {
        exec_version(&install_dir.join(bin_name(bin)))?;
    }
    Ok(())
}

fn exec_version(bin: &Path) -> MainResult<()> {
    let status = std::process::Command::new(bin)
        .arg("--version")
        .status()
        .source_resource()
        .doing("run binary version command")
        .with_context(("bin", bin))?;
    if status.success() {
        return Ok(());
    }
    Err(MainReason::resource_detail(format!(
        "health check failed: {} --version exit={status}",
        bin.display()
    )))
}

fn copy_file(src: &Path, dst: &Path) -> MainResult<()> {
    let parent = dst.parent().ok_or_else(|| {
        MainReason::resource_detail(format!("cannot resolve parent of {}", dst.display()))
    })?;
    fs::create_dir_all(parent)
        .source_resource()
        .doing("create copy destination parent")
        .with_context(("path", parent))?;
    // 原子替换：先复制到同目录临时文件，再 rename 覆盖目标。
    // 注意：不能直接 `fs::copy` 到正在运行的二进制——Linux 会报 ETXTBSY。
    let file_name = dst
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .ok_or_else(|| {
            MainReason::resource_detail(format!("cannot resolve file name of {}", dst.display()))
        })?;
    let tmp = parent.join(format!(".{file_name}.new.{}", std::process::id()));
    fs::copy(src, &tmp)
        .source_resource()
        .doing("stage backup binary")
        .with_context(("src", src))
        .with_context(("dst", tmp.as_path()))?;
    let perm = fs::metadata(src)
        .source_resource()
        .doing("read source file permissions")
        .with_context(("src", src))?
        .permissions();
    fs::set_permissions(&tmp, perm)
        .source_resource()
        .doing("set backup file permissions")
        .with_context(("path", tmp.as_path()))?;
    fs::rename(&tmp, dst)
        .source_resource()
        .doing("replace binary atomically")
        .with_context(("dst", dst))?;
    Ok(())
}

pub(crate) fn bin_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{bin_name, rollback};

    #[test]
    fn bin_name_appends_exe_on_windows_only() {
        if cfg!(windows) {
            assert_eq!(bin_name("gops"), "gops.exe");
        } else {
            assert_eq!(bin_name("gops"), "gops");
        }
    }

    #[test]
    fn rollback_fails_when_backup_missing() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let install_dir = tmp.path().join("install");
        let backup_dir = tmp.path().join("backup");
        std::fs::create_dir_all(&install_dir).expect("install dir");
        std::fs::create_dir_all(&backup_dir).expect("backup dir");

        let err = rollback(&install_dir, &backup_dir).expect_err("missing backup must fail");
        assert!(err.to_string().contains("backup is incomplete"));
    }

    #[test]
    fn rollback_restores_binary_from_backup() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let install_dir = tmp.path().join("install");
        let backup_dir = tmp.path().join("backup");
        std::fs::create_dir_all(&install_dir).expect("install dir");
        std::fs::create_dir_all(&backup_dir).expect("backup dir");
        std::fs::write(install_dir.join("gops"), b"new").expect("write live");
        std::fs::write(backup_dir.join("gops"), b"old").expect("write backup");

        rollback(&install_dir, &backup_dir).expect("rollback ok");
        let restored = std::fs::read(install_dir.join("gops")).expect("read restored");
        assert_eq!(restored, b"old");
        // 原子替换：不应残留临时文件
        let leftovers: Vec<String> = std::fs::read_dir(&install_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".new."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "no temp files should remain: {leftovers:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rollback_preserves_executable_permission() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().expect("tmpdir");
        let install_dir = tmp.path().join("install");
        let backup_dir = tmp.path().join("backup");
        std::fs::create_dir_all(&install_dir).expect("install dir");
        std::fs::create_dir_all(&backup_dir).expect("backup dir");
        std::fs::write(install_dir.join("gops"), b"new").expect("write live");
        let backup = backup_dir.join("gops");
        std::fs::write(&backup, b"old").expect("write backup");
        std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        rollback(&install_dir, &backup_dir).expect("rollback ok");
        let mode = std::fs::metadata(install_dir.join("gops"))
            .expect("stat restored")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755, "exec bit must survive rollback");
    }
}
