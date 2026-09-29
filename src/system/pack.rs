use super::prelude::*;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::File;
use walkdir::WalkDir;

use crate::error::MainReason;
use crate::system::lock::DELIVER_LOCK_FILE;

/// 打包系统根目录为 `.tar.gz`。
///
/// - `use_git = true`（默认）：只含 `git ls-files` 列出的**入库文件**（等价 `git archive` 的
///   “只含入库文件”，需在 git 仓库内运行），自然排除 `.gitignore` 忽略的产物；
/// - `use_git = false`（`--no-git`）：打当前目录**全部**内容（仅跳过 `.git/`）。
///
/// 符号链接按 `git archive` 语义**保留为符号链接**（两种模式一致，不解引用）。
/// `deliver.lock` 作为交付清单始终随包分发。
pub fn pack_system(root: &Path, out: &Path, use_git: bool) -> MainResult<()> {
    let mut files = if use_git {
        git_tracked_files(root)?
    } else {
        walk_files(root)?
    };

    // 交付清单必须随包分发
    let lock_rel = PathBuf::from(DELIVER_LOCK_FILE);
    if root.join(&lock_rel).is_file() && !files.iter().any(|f| f == &lock_rel) {
        files.push(lock_rel);
    }

    files.sort();
    files.dedup();
    write_archive(root, out, &files)?;
    Ok(())
}

/// `git ls-files`：列出系统根目录里入库（被跟踪）的相对路径。
fn git_tracked_files(root: &Path) -> MainResult<Vec<PathBuf>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
        .output()
        .map_err(|e| {
            MainReason::logic_detail(format!(
                "run `git ls-files` failed: {e}（默认打包需要 git；若要打包整目录请用 `--no-git`）"
            ))
        })?;
    if !out.status.success() {
        return Err(MainReason::logic_detail(format!(
            "`git ls-files` 失败：默认打包需要系统根目录（{}）是 git 仓库；\
             如需打包整目录请用 `--no-git`",
            root.display()
        )));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// 遍历目录：打全部内容（仅跳过 `.git/`）；文件与符号链接都收（目录不单独列）。
fn walk_files(root: &Path) -> MainResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || e.file_name() != ".git")
    {
        let entry = entry.map_err(|e| MainReason::logic_detail(format!("walk dir failed: {e}")))?;
        let ft = entry.file_type();
        if ft.is_file() || ft.is_symlink() {
            let rel = entry
                .path()
                .strip_prefix(root)
                .map_err(|e| MainReason::logic_detail(format!("strip prefix failed: {e}")))?;
            files.push(rel.to_path_buf());
        }
    }
    Ok(files)
}

/// 估算归档总量（字节，用于进度）；符号链接不计内容。
fn total_bytes(root: &Path, files: &[PathBuf]) -> u64 {
    files
        .iter()
        .filter_map(|rel| std::fs::symlink_metadata(root.join(rel)).ok())
        .map(|m| if m.is_file() { m.len() } else { 0 })
        .sum()
}

fn write_archive(root: &Path, out: &Path, files: &[PathBuf]) -> MainResult<()> {
    let pb = ProgressBar::new(total_bytes(root, files));
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} {msg}",
        )
        .unwrap()
        .progress_chars("#>-"),
    );
    pb.set_message("打包中");

    let file = File::create(out).source_sys().with(out)?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut tar = tar::Builder::new(encoder);
    for rel in files {
        if append_entry(&mut tar, root, rel)? {
            let size = std::fs::symlink_metadata(root.join(rel))
                .map(|m| if m.is_file() { m.len() } else { 0 })
                .unwrap_or(0);
            pb.inc(size);
        }
    }
    pb.finish_and_clear();
    tar.finish()
        .map_err(|e| MainReason::logic_detail(format!("tar finish failed: {e}")))?;
    Ok(())
}

/// 写入一个条目；返回是否真的写了（不存在/目录返回 `false`）。
///
/// 符号链接保留为符号链接（不解引用），与 `git archive` 一致。
fn append_entry(
    tar: &mut tar::Builder<GzEncoder<File>>,
    root: &Path,
    rel: &Path,
) -> MainResult<bool> {
    let abs = root.join(rel);
    let meta = match std::fs::symlink_metadata(&abs) {
        Ok(m) => m,
        // 已入库但工作区已删除的文件：跳过
        Err(_) => return Ok(false),
    };
    let ft = meta.file_type();
    if ft.is_symlink() {
        let target = std::fs::read_link(&abs)
            .map_err(|e| MainReason::logic_detail(format!("read_link {}: {e}", abs.display())))?;
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_link_name(&target).map_err(|e| {
            MainReason::logic_detail(format!("set_link_name {}: {e}", abs.display()))
        })?;
        tar.append_data(&mut header, rel, std::io::empty())
            .map_err(|e| MainReason::logic_detail(format!("tar add {}: {e}", rel.display())))?;
        Ok(true)
    } else if ft.is_file() {
        tar.append_path_with_name(&abs, rel)
            .map_err(|e| MainReason::logic_detail(format!("tar add {}: {e}", rel.display())))?;
        Ok(true)
    } else {
        Ok(false) // 目录：不写条目（解压时按需创建）
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use tempfile::tempdir;

    fn tar_names(path: &Path) -> Vec<String> {
        let file = File::open(path).unwrap();
        let mut ar = tar::Archive::new(GzDecoder::new(file));
        ar.entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().to_string())
            .collect()
    }

    fn tar_file_content(path: &Path, name: &str) -> Option<String> {
        let file = File::open(path).unwrap();
        let mut ar = tar::Archive::new(GzDecoder::new(file));
        for entry in ar.entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.path().unwrap().to_string_lossy() == name {
                let mut buf = String::new();
                std::io::Read::read_to_string(&mut entry, &mut buf).unwrap();
                return Some(buf);
            }
        }
        None
    }

    fn tar_entry_type(path: &Path, name: &str) -> Option<tar::EntryType> {
        let file = File::open(path).unwrap();
        let mut ar = tar::Archive::new(GzDecoder::new(file));
        for entry in ar.entries().unwrap() {
            let entry = entry.unwrap();
            if entry.path().unwrap().to_string_lossy() == name {
                return Some(entry.header().entry_type());
            }
        }
        None
    }

    fn git_ok(root: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn test_pack_no_git_includes_everything_and_deliver_lock() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sys/arm-mac14-host/mods/m1/local")).unwrap();
        std::fs::write(root.join("sys/merged_vars.yml"), "a: 1\n").unwrap();
        std::fs::write(root.join("sys/arm-mac14-host/mods/m1/local/big.bin"), "x").unwrap();
        std::fs::write(root.join(".env"), "K=V\n").unwrap();
        std::fs::write(root.join("deliver.lock"), "lock").unwrap();

        let out = dir.path().join("out.tar.gz");
        pack_system(root, &out, false).unwrap();

        let names = tar_names(&out);
        assert!(names.iter().any(|n| n == "sys/merged_vars.yml"));
        assert!(
            names
                .iter()
                .any(|n| n == "sys/arm-mac14-host/mods/m1/local/big.bin")
        );
        assert!(
            names.iter().any(|n| n == ".env"),
            "--no-git 应含全部（含隐藏）"
        );
        assert!(names.iter().any(|n| n == "deliver.lock"));
    }

    #[test]
    fn test_pack_git_only_tracked_when_git_available() {
        if !git_ok(Path::new("."), &["--version"]) {
            return; // 无 git
        }
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("tracked.txt"), "t").unwrap();
        std::fs::write(root.join("untracked.txt"), "u").unwrap();
        std::fs::write(root.join("deliver.lock"), "lock").unwrap();
        assert!(git_ok(root, &["init", "-q"]));
        assert!(git_ok(root, &["add", "tracked.txt"]));

        let out = dir.path().join("out.tar.gz");
        pack_system(root, &out, true).unwrap();

        let names = tar_names(&out);
        assert!(names.iter().any(|n| n == "tracked.txt"));
        assert!(
            !names.iter().any(|n| n == "untracked.txt"),
            "默认只打入库文件: {names:?}"
        );
        // 交付清单即使未跟踪也必须随包
        assert!(names.iter().any(|n| n == "deliver.lock"));
    }

    #[test]
    fn test_pack_git_takes_working_tree_content() {
        if !git_ok(Path::new("."), &["--version"]) {
            return;
        }
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("vars.yml"), "v1\n").unwrap();
        assert!(git_ok(root, &["init", "-q"]));
        assert!(git_ok(root, &["add", "vars.yml"]));
        // 已入库但工作区内容被改成 v2（未提交）——必须打进 v2，否则 update 后的值会丢
        std::fs::write(root.join("vars.yml"), "v2\n").unwrap();

        let out = dir.path().join("out.tar.gz");
        pack_system(root, &out, true).unwrap();
        assert_eq!(tar_file_content(&out, "vars.yml").as_deref(), Some("v2\n"));
    }

    #[test]
    fn test_pack_default_errors_when_not_a_git_repo() {
        if !git_ok(Path::new("."), &["--version"]) {
            return; // 无 git
        }
        let dir = tempdir().unwrap();
        let root = dir.path();
        // 若临时目录恰在某个仓库内则跳过（git ls-files 会成功）
        if git_ok(root, &["rev-parse", "--is-inside-work-tree"]) {
            return;
        }
        std::fs::write(root.join("a.txt"), "x").unwrap();
        let out = dir.path().join("out.tar.gz");
        assert!(
            pack_system(root, &out, true).is_err(),
            "非 git 仓库时默认打包应报错"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_pack_preserves_symlinks() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("real.txt"), "data").unwrap();
        std::os::unix::fs::symlink("real.txt", root.join("link.txt")).unwrap();

        let out = dir.path().join("out.tar.gz");
        pack_system(root, &out, false).unwrap();

        assert_eq!(
            tar_entry_type(&out, "link.txt"),
            Some(tar::EntryType::Symlink),
            "符号链接应保留为链接，而不是解引用"
        );
        assert_ne!(
            tar_entry_type(&out, "real.txt"),
            Some(tar::EntryType::Symlink)
        );
    }
}
