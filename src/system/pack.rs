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
/// - `use_git = false`（`--full`）：打当前目录**全部**内容（含制品与本地化产物，仅跳过 `.git/`）。
///
/// `ignore`（来自 `sys-prj.yml` 的 `ignore:` 节，glob 相对系统根）在**两种模式下都生效**。
/// 符号链接按 `git archive` 语义**保留为符号链接**（两种模式一致，不解引用）。
/// `deliver.lock` 作为交付清单始终随包分发（不受 `ignore` 影响）。
pub fn pack_system(root: &Path, out: &Path, use_git: bool, ignore: &[String]) -> MainResult<()> {
    let patterns = compile_path_patterns(ignore, "ignore")?;

    let mut files = if use_git {
        git_tracked_files(root)?
    } else {
        walk_files(root, &patterns)?
    };

    // 两种模式都应用 `sys-prj.yml` 的 ignore（匹配文件自身或其任一祖先目录）
    if !patterns.is_empty() {
        files.retain(|rel| !path_matches(rel, &patterns));
    }

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

/// 编译 ignore glob（去空白、去前导 `./`、去首尾 `/`；忽略空串）。
/// 编译一组路径模式（`sys-prj.yml` 的 `ignore:` / `preserve:` 等）为 glob。
///
/// `what` 只用于报错文案（如 `ignore` / `preserve`），让用户知道是哪一节写错了。
///
/// 归一化常见 gitignore 写法：`/build`、`./artifacts/` 等应等价于 `build`、`artifacts`。
/// 不归一化的话，glob 会把前导 `/`、`./` 当作字面量，模式静默失效。
pub(crate) fn compile_path_patterns(raw: &[String], what: &str) -> MainResult<Vec<glob::Pattern>> {
    let mut patterns = Vec::new();
    for raw in raw {
        let Some(pat) = normalize_path_pattern(raw) else {
            continue;
        };
        let pattern = glob::Pattern::new(&pat).map_err(|e| {
            MainReason::logic_detail(format!(
                "invalid {what} pattern in sys-prj.yml: `{raw}` ({e})"
            ))
        })?;
        patterns.push(pattern);
    }
    Ok(patterns)
}

/// 归一化单条路径模式：去空白、去前导 `./`、去首尾 `/`；空串返回 `None`。
/// 供 `compile_path_patterns` 与 `prj diagnose`（比对 `ignore` / `preserve` / `backup`）共用，
/// 避免两处归一化规则分叉。
pub(crate) fn normalize_path_pattern(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let no_dot = trimmed.strip_prefix("./").unwrap_or(trimmed);
    let pat = no_dot.trim_matches('/');
    if pat.is_empty() {
        None
    } else {
        Some(pat.to_string())
    }
}

/// `rel` 是否命中任一模式（匹配文件自身**或**其任一祖先目录，类 gitignore 的目录排除）。
///
/// 注意它返回的是“命中”，而不是“被忽略” —— 同一个判定既用于打包时的 `ignore`，
/// 也用于升级时的 `preserve`（后者是“不覆盖”，不是“不打包”），所以不能叫 `is_ignored`。
pub(crate) fn path_matches(rel: &Path, patterns: &[glob::Pattern]) -> bool {
    let opts = glob::MatchOptions {
        // `*` 不跨 `/`（需显式用 `**`），贴近 gitignore 直觉
        require_literal_separator: true,
        ..Default::default()
    };
    let mut cur: Option<&Path> = Some(rel);
    while let Some(p) = cur {
        // glob 在 Windows 已把 `\` 当作分隔符，故不做替换（否则会破坏 Unix 上合法的反斜杠名）
        let text = p.to_string_lossy();
        if patterns.iter().any(|pat| pat.matches_with(&text, opts)) {
            return true;
        }
        cur = p.parent().filter(|q| !q.as_os_str().is_empty());
    }
    false
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
                "run `git ls-files` failed: {e}（默认打包需要 git；若要打包整目录请用 `--full`）"
            ))
        })?;
    if !out.status.success() {
        return Err(MainReason::logic_detail(format!(
            "`git ls-files` 失败：默认打包需要系统根目录（{}）是 git 仓库；\
             如需打包整目录请用 `--full`",
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

/// 遍历目录：打全部内容（跳过 `.git/`，并**剪枝**被 ignore 的目录）；文件与符号链接都收。
fn walk_files(root: &Path, patterns: &[glob::Pattern]) -> MainResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_entry(|e| {
        if e.depth() == 0 {
            return true;
        }
        if e.file_name() == ".git" {
            return false;
        }
        // 剪枝：被 ignore 的目录不再深入（省 I/O，且忽略子树里的错误不会中断打包）
        if !patterns.is_empty()
            && e.file_type().is_dir()
            && let Ok(rel) = e.path().strip_prefix(root)
        {
            return !path_matches(rel, patterns);
        }
        true
    }) {
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
    fn test_pack_full_includes_everything_and_deliver_lock() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sys/arm-mac14-host/mods/m1/local")).unwrap();
        std::fs::write(root.join("sys/merged_vars.yml"), "a: 1\n").unwrap();
        std::fs::write(root.join("sys/arm-mac14-host/mods/m1/local/big.bin"), "x").unwrap();
        std::fs::write(root.join(".env"), "K=V\n").unwrap();
        std::fs::write(root.join("deliver.lock"), "lock").unwrap();

        let out = dir.path().join("out.tar.gz");
        pack_system(root, &out, false, &[]).unwrap();

        let names = tar_names(&out);
        assert!(names.iter().any(|n| n == "sys/merged_vars.yml"));
        assert!(
            names
                .iter()
                .any(|n| n == "sys/arm-mac14-host/mods/m1/local/big.bin")
        );
        assert!(
            names.iter().any(|n| n == ".env"),
            "--full 应含全部（含隐藏）"
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
        pack_system(root, &out, true, &[]).unwrap();

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
        pack_system(root, &out, true, &[]).unwrap();
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
            pack_system(root, &out, true, &[]).is_err(),
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
        pack_system(root, &out, false, &[]).unwrap();

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

    #[test]
    fn test_pack_ignore_excludes_subtree_in_full_mode() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sys/arm-mac14-host/mods/m1/local")).unwrap();
        std::fs::write(root.join("sys/merged_vars.yml"), "a: 1\n").unwrap();
        std::fs::write(root.join("sys/arm-mac14-host/mods/m1/local/big.bin"), "x").unwrap();
        std::fs::write(root.join("sys/arm-mac14-host/mods/m1/spec.yml"), "s").unwrap();
        // 两个组件在 sys 与 mods 之间：`sys/*/mods`（`*` 不跨 `/`）不应匹配它
        std::fs::create_dir_all(root.join("sys/a/b/mods")).unwrap();
        std::fs::write(root.join("sys/a/b/mods/deep.txt"), "d").unwrap();
        std::fs::write(root.join(".env"), "K=V\n").unwrap();
        std::fs::write(root.join("keep.txt"), "k").unwrap();

        let out = dir.path().join("out.tar.gz");
        // 目录级模式 `sys/*/mods` 应排除其下全部文件；`.env` 也排除
        let ignore = vec!["sys/*/mods".to_string(), ".env".to_string()];
        pack_system(root, &out, false, &ignore).unwrap();

        let names = tar_names(&out);
        assert!(
            !names.iter().any(|n| n.contains("mods/m1")),
            "ignore `sys/*/mods` 应排除其下所有文件: {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "sys/a/b/mods/deep.txt"),
            "`*` 不应跨 `/`，`sys/a/b/mods` 不应被 `sys/*/mods` 排除: {names:?}"
        );
        assert!(!names.iter().any(|n| n == ".env"), "ignore 应排除 .env");
        assert!(names.iter().any(|n| n == "sys/merged_vars.yml"));
        assert!(names.iter().any(|n| n == "keep.txt"));
    }

    #[test]
    fn test_pack_ignore_applies_in_git_mode() {
        if !git_ok(Path::new("."), &["--version"]) {
            return;
        }
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sys/arm-mac14-host/mods/m1")).unwrap();
        std::fs::write(root.join("sys/arm-mac14-host/mods/m1/spec.yml"), "s").unwrap();
        std::fs::write(root.join("tracked.txt"), "t").unwrap();
        assert!(git_ok(root, &["init", "-q"]));
        assert!(git_ok(
            root,
            &["add", "tracked.txt", "sys/arm-mac14-host/mods/m1/spec.yml"]
        ));

        let out = dir.path().join("out.tar.gz");
        let ignore = vec!["sys/*/mods".to_string()];
        pack_system(root, &out, true, &ignore).unwrap();

        let names = tar_names(&out);
        assert!(names.iter().any(|n| n == "tracked.txt"));
        assert!(
            !names.iter().any(|n| n.contains("mods/m1")),
            "git 模式下 ignore 也应生效: {names:?}"
        );
    }

    #[test]
    fn test_pack_invalid_ignore_pattern_errors() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a.txt"), "x").unwrap();
        let out = dir.path().join("out.tar.gz");
        let ignore = vec!["[".to_string()];
        let err = pack_system(root, &out, false, &ignore).expect_err("invalid pattern must fail");
        assert!(
            err.to_string().contains("invalid ignore pattern"),
            "错误信息应指出 ignore 模式非法: {err}"
        );
    }

    #[test]
    fn test_pack_ignore_normalizes_leading_slash_and_dot() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("artifacts")).unwrap();
        std::fs::write(root.join("artifacts/big.bin"), "x").unwrap();
        std::fs::write(root.join(".env"), "K=V\n").unwrap();
        std::fs::write(root.join("keep.txt"), "k").unwrap();

        let out = dir.path().join("out.tar.gz");
        // gitignore 风格写法：`/artifacts`、`./.env` 应等价于 `artifacts`、`.env`
        let ignore = vec!["/artifacts".to_string(), "./.env".to_string()];
        pack_system(root, &out, false, &ignore).unwrap();

        let names = tar_names(&out);
        assert!(!names.iter().any(|n| n.contains("artifacts")), "{names:?}");
        assert!(!names.iter().any(|n| n == ".env"), "{names:?}");
        assert!(names.iter().any(|n| n == "keep.txt"));
    }

    #[test]
    fn test_pack_deliver_lock_never_ignored() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("deliver.lock"), "lock").unwrap();
        std::fs::write(root.join("a.txt"), "a").unwrap();

        let out = dir.path().join("out.tar.gz");
        let ignore = vec!["deliver.lock".to_string()];
        pack_system(root, &out, false, &ignore).unwrap();

        let names = tar_names(&out);
        assert!(
            names.iter().any(|n| n == "deliver.lock"),
            "deliver.lock 不应被 ignore 排除: {names:?}"
        );
    }
}
