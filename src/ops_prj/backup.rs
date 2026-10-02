//! 现场态备份与还原。
//!
//! 备份选取来自两级声明（见 `docs/design/prj-backup-restore.md` §4）：
//! **系统侧** `sys-prj.yml: backup.{restore,rebuild}` 说清“哪些路径、丢了什么代价”，
//! **项目侧** `ops-prj.yml: backup.{target,keep,systems}` 说清“落到哪、留几份、收哪些”。
//!
//! 三条设计约束（与本模块直接相关）：
//! 1. **秘密显性**：含私钥/凭据的条目点名，并提示离机保管。
//! 2. **原子**：还原先解到临时目录，再合并回现场；中途失败不会留下半截目标。
//! 3. **没声明就保守**：不在任何档里的路径不收；也不删（收与删是两件事）。

use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::fs::File;

use crate::const_vars::SYS_PRJ_CONF_FILE_V2;
use crate::error::MainReason;
use crate::ops_prj::prelude::*;
use crate::ops_prj::project::OpsProject;
use crate::system::SysConf;
use crate::system::pack::{compile_path_patterns, path_matches};

/// 备份里的一个条目（相对**项目根**的路径）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// 内容里出现 `PRIVATE KEY` —— 提示离机保管
    pub secret: bool,
}

/// 一次备份的结果。
#[derive(Debug, Clone, Default)]
pub struct BackupReport {
    pub archive: PathBuf,
    pub entries: Vec<BackupEntry>,
    /// 因超过 `keep` 被清掉的旧份
    pub pruned: Vec<PathBuf>,
}

impl BackupReport {
    pub fn secret_entries(&self) -> impl Iterator<Item = &BackupEntry> {
        self.entries.iter().filter(|e| e.secret)
    }
    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.size).sum()
    }
}

/// 一个已解析好的系统选取：系统名 + 要收的模式（include 与 exclude 各自编译好）。
#[derive(Debug, Clone)]
pub struct ResolvedSystem {
    pub name: String,
    pub include: Vec<glob::Pattern>,
    pub exclude: Vec<glob::Pattern>,
}

/// 还原结果。
#[derive(Debug, Clone, Default)]
pub struct RestoreReport {
    pub root: PathBuf,
    pub entries: Vec<String>,
    pub dry_run: bool,
}

/// 项目侧声明解析出的单个系统选取（尚未编译成 glob）。
#[derive(Debug, Clone)]
struct SystemPlan {
    name: String,
    with_rebuild: bool,
    include: Vec<String>,
    exclude: Vec<String>,
}

impl OpsProject {
    /// `prj backup`：按项目/系统两级声明收集现场态，打成一个归档，并清理过期份。
    pub fn backup(&self, include_rebuild: bool) -> MainResult<BackupReport> {
        let root = self.paths().root();
        let (target, keep, sels) = self.backup_plan();
        let target = PathBuf::from(target.to_string().env_eval(&ValueDict::default()));

        let mut resolved = Vec::new();
        for sel in sels {
            let with_rebuild = sel.with_rebuild || include_rebuild;
            let include = self.resolve_include(root, &sel.name, with_rebuild, &sel.include)?;
            let exclude = compile_path_patterns(&sel.exclude, "backup.exclude")?;
            resolved.push(ResolvedSystem {
                name: sel.name,
                include,
                exclude,
            });
        }

        backup_systems(self.conf().name(), root, &resolved, &target, keep)
    }

    /// 解析备份计划：项目侧声明缺省时给保守默认（全系统、restore 档、`<root>/.backup`）。
    fn backup_plan(&self) -> (String, usize, Vec<SystemPlan>) {
        let root = self.paths().root().to_path_buf();
        let all_systems = |level_rebuild: bool| -> Vec<SystemPlan> {
            self.conf()
                .sys_models()
                .iter()
                .map(|s| SystemPlan {
                    name: s.sys().name().clone(),
                    with_rebuild: level_rebuild,
                    include: Vec::new(),
                    exclude: Vec::new(),
                })
                .collect()
        };
        match self.conf().backup() {
            Some(b) if !b.systems().is_empty() => {
                let sels = b
                    .systems()
                    .iter()
                    .map(|s| SystemPlan {
                        name: s.name().clone(),
                        with_rebuild: s.level().includes_rebuild(),
                        include: s.include().clone(),
                        exclude: s.exclude().clone(),
                    })
                    .collect();
                (b.target().clone(), *b.keep(), sels)
            }
            Some(b) => (b.target().clone(), *b.keep(), all_systems(false)),
            None => (
                root.join(".backup").to_string_lossy().to_string(),
                10,
                all_systems(false),
            ),
        }
    }

    /// 组装一个系统的 include 模式：系统声明的 backup 档 + 项目现场增量。
    fn resolve_include(
        &self,
        root: &Path,
        sys_name: &str,
        with_rebuild: bool,
        extra: &[String],
    ) -> MainResult<Vec<glob::Pattern>> {
        let conf_file = root.join(sys_name).join(SYS_PRJ_CONF_FILE_V2);
        let mut raw = if conf_file.exists() {
            let conf = SysConf::load_conf(&conf_file).source_resource()?;
            conf.backup().patterns_for(with_rebuild)
        } else {
            Vec::new()
        };
        raw.extend(extra.iter().cloned());
        compile_path_patterns(&raw, "backup")
    }
}

/// 收集并打包：把各系统里命中 `include` 且不命中 `exclude` 的文件收进归档。
///
/// 归档内路径形如 `<系统名>/<相对路径>`；落点在 `target/<项目名>-<时间戳>.tar.gz`。
pub fn backup_systems(
    project_name: &str,
    project_root: &Path,
    systems: &[ResolvedSystem],
    target: &Path,
    keep: usize,
) -> MainResult<BackupReport> {
    ensure_path(target).source_resource().with(target)?;
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let archive = target.join(format!("{project_name}-{ts}.tar.gz"));

    let file = File::create(&archive).source_resource().with(&archive)?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut tar = tar::Builder::new(encoder);

    let mut entries = Vec::new();
    for sys in systems {
        let sys_root = project_root.join(&sys.name);
        if !sys_root.is_dir() {
            println!("跳过 {}：未导入", sys.name);
            continue;
        }
        for rel in collect_matching(&sys_root, &sys.include, &sys.exclude)? {
            let abs = sys_root.join(&rel);
            let arc_name = format!("{}/{}", sys.name, rel.to_string_lossy());
            append_entry(&mut tar, &abs, Path::new(&arc_name))?;
            entries.push(entry_info(&arc_name, &abs)?);
        }
    }
    tar.finish()
        .map_err(|e| MainReason::logic_detail(format!("tar finish failed: {e}")))?;

    let pruned = prune_old(&archive, project_name, keep)?;
    Ok(BackupReport {
        archive,
        entries,
        pruned,
    })
}

/// 还原：把归档合并回现场目录（先解到临时目录，再覆盖）。
///
/// 归档条目形如 `<系统名>/<相对路径>`，合并落点为 `<project_root>/<条目路径>`。
pub fn restore(project_root: &Path, archive: &Path, dry_run: bool) -> MainResult<RestoreReport> {
    // 先打开归档：打不开就不建临时目录（否则会在现场留一个空的 .restore-tmp-*）
    let file = File::open(archive).source_resource().with(archive)?;

    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let tmp = project_root.join(format!(".restore-tmp-{ts}"));
    make_clean_path(&tmp).source_resource().with(&tmp)?;

    let mut ar = tar::Archive::new(GzDecoder::new(file));
    if let Err(e) = ar.unpack(&tmp) {
        // 解包失败：清掉半截临时目录，现场保持原样
        if let Err(re) = std::fs::remove_dir_all(&tmp) {
            println!("提示：临时目录未能删除 {}: {re}", tmp.display());
        }
        return Err(MainReason::logic_detail(format!(
            "解包失败（归档可能损坏）{}: {e}",
            archive.display()
        )));
    }

    let mut entries = Vec::new();
    for top in std::fs::read_dir(&tmp).source_resource().with(&tmp)? {
        let top = top.source_resource()?;
        entries.push(top.file_name().to_string_lossy().to_string());
    }
    entries.sort();

    if !dry_run {
        for name in &entries {
            merge_copy(&tmp.join(name), &project_root.join(name))?;
        }
    }
    // 无论 dry-run 与否，临时目录都不留
    if let Err(e) = std::fs::remove_dir_all(&tmp) {
        println!("提示：临时目录未能删除 {}: {e}", tmp.display());
    }

    Ok(RestoreReport {
        root: project_root.to_path_buf(),
        entries,
        dry_run,
    })
}

/// 列出归档里的条目（相对路径），用于 `prj restore --list`。
pub fn list_archive(archive: &Path) -> MainResult<Vec<String>> {
    let file = File::open(archive).source_resource().with(archive)?;
    let mut ar = tar::Archive::new(GzDecoder::new(file));
    let mut out = Vec::new();
    for e in ar.entries().source_resource().with(archive)? {
        let e = e.source_resource()?;
        if let Ok(p) = e.path() {
            out.push(p.to_string_lossy().to_string());
        }
    }
    out.sort();
    Ok(out)
}

/// 归档里 `path`（相对项目根的路径）在 `sys_root` 下对应的文件。
fn collect_matching(
    sys_root: &Path,
    include: &[glob::Pattern],
    exclude: &[glob::Pattern],
) -> MainResult<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(sys_root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || e.file_name() != ".git")
    {
        let entry = entry.map_err(|e| MainReason::logic_detail(format!("walk dir failed: {e}")))?;
        let ft = entry.file_type();
        if !(ft.is_file() || ft.is_symlink()) {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(sys_root)
            .map_err(|e| MainReason::logic_detail(format!("strip prefix failed: {e}")))?;
        // `values` 是项目层客户值（由 git 纳管），不在系统目录里，也不是本备份的对象
        if rel.components().next().map(|c| c.as_os_str() == "values") == Some(true) {
            continue;
        }
        if path_matches(rel, include) && !path_matches(rel, exclude) {
            out.push(rel.to_path_buf());
        }
    }
    out.sort();
    Ok(out)
}

/// 私钥扫描只读文件头这么多字节（身份材料都很小；只提醒，不求穷尽）。
const SECRET_SCAN_CAP: usize = 1024 * 1024;

/// 一个条目的信息：**流式计算** sha256（不把整个文件读进内存），私钥只在文件头判。
/// 符号链接不算目标文件内容，而是对**链接目标字符串**取摘要（与 `same_bytes` 同口径）。
fn entry_info(arc_name: &str, abs: &Path) -> MainResult<BackupEntry> {
    let meta = std::fs::symlink_metadata(abs).source_resource().with(abs)?;
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(abs).source_resource().with(abs)?;
        let bytes = target.to_string_lossy();
        return Ok(BackupEntry {
            path: arc_name.to_string(),
            size: 0,
            sha256: hex(&Sha256::digest(bytes.as_bytes())),
            secret: false,
        });
    }

    let mut f = File::open(abs).source_resource().with(abs)?;
    // 先读一段头部用于私钥扫描（连续一段，避免标记跨缓冲被切断），再流式哈希剩余部分
    let mut head = vec![0u8; SECRET_SCAN_CAP];
    let mut head_len = 0usize;
    while head_len < head.len() {
        let n = std::io::Read::read(&mut f, &mut head[head_len..])
            .source_resource()
            .with(abs)?;
        if n == 0 {
            break;
        }
        head_len += n;
    }
    head.truncate(head_len);
    let secret = looks_like_secret(&head);

    let mut hasher = Sha256::new();
    hasher.update(&head);
    let mut size = head_len as u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = std::io::Read::read(&mut f, &mut buf)
            .source_resource()
            .with(abs)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok(BackupEntry {
        path: arc_name.to_string(),
        size,
        sha256: hex(&hasher.finalize()),
        secret,
    })
}

/// 只看内容里有没有私钥标记，不做精确格式判断 —— 目的是提醒，不是证明。
fn looks_like_secret(bytes: &[u8]) -> bool {
    bytes
        .windows(b"PRIVATE KEY".len())
        .any(|w| w == b"PRIVATE KEY")
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 写入一个条目（文件 / 符号链接；目录不入归档，解包时按需创建）。
fn append_entry<W: std::io::Write>(
    tar: &mut tar::Builder<W>,
    abs: &Path,
    name: &Path,
) -> MainResult<()> {
    let meta = std::fs::symlink_metadata(abs).source_resource().with(abs)?;
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(abs).source_resource().with(abs)?;
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header
            .set_link_name(&target)
            .map_err(|e| MainReason::logic_detail(format!("set_link_name: {e}")))?;
        tar.append_data(&mut header, name, std::io::empty())
            .map_err(|e| MainReason::logic_detail(format!("tar add {}: {e}", name.display())))?;
    } else {
        tar.append_path_with_name(abs, name)
            .map_err(|e| MainReason::logic_detail(format!("tar add {}: {e}", name.display())))?;
    }
    Ok(())
}

/// 保留最新 `keep` 份 `<project_name>-*.tar.gz`，其余删除；返回被删的路径。
fn prune_old(archive: &Path, project_name: &str, keep: usize) -> MainResult<Vec<PathBuf>> {
    if keep == 0 {
        return Ok(Vec::new());
    }
    let Some(dir) = archive.parent() else {
        return Ok(Vec::new());
    };
    let prefix = format!("{project_name}-");
    let mut olds: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir).source_resource().with(dir)? {
        let entry = entry.source_resource()?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(&prefix) && name.ends_with(".tar.gz") {
            olds.push(entry.path());
        }
    }
    // 文件名内嵌时间戳（`%Y%m%d-%H%M%S`），字典序 = 时间序；保留末尾 keep 个
    olds.sort();
    let mut pruned = Vec::new();
    if olds.len() > keep {
        for old in &olds[..olds.len() - keep] {
            std::fs::remove_file(old).source_resource().with(old)?;
            pruned.push(old.clone());
        }
    }
    Ok(pruned)
}

/// 把 `src` 内容合并进 `dst`（递归；覆盖同名文件；符号链接按链接复制）。
fn merge_copy(src: &Path, dst: &Path) -> MainResult<()> {
    let meta = std::fs::symlink_metadata(src).source_resource().with(src)?;
    if meta.file_type().is_symlink() {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .source_resource()
                .with(parent)?;
        }
        if std::fs::symlink_metadata(dst).is_ok() {
            std::fs::remove_file(dst).source_resource().with(dst)?;
        }
        let link = std::fs::read_link(src).source_resource().with(src)?;
        std::os::unix::fs::symlink(&link, dst)
            .source_resource()
            .with(dst)?;
    } else if meta.is_dir() {
        std::fs::create_dir_all(dst).source_resource().with(dst)?;
        for entry in std::fs::read_dir(src).source_resource().with(src)? {
            let entry = entry.source_resource()?;
            merge_copy(&entry.path(), &dst.join(entry.file_name()))?;
        }
    } else {
        place_file(src, dst)?;
    }
    Ok(())
}

/// 把一个普通文件落到 `dst`：**同目录暂存 + `rename`**。
///
/// 为什么不是 `fs::copy(src, dst)` 原地覆盖：目标可能**不归当前用户**。现场真实一例 ——
/// `configs/gateway/state/wist-gateway-store.db*` 是**容器身份**（`999:999`）建的
/// （`align-host-perms.sh` 刻意不碰容器自建的库），而 `prj restore` 以**部署账号**跑：
/// 原地打开写入会被 `EACCES` 挡住，症状是「备份**收得进**、还原**写不回**」。
/// `rename(2)` 只要求**目录**可写、**不要求目标文件可写**，所以能把目标直接换掉 ——
/// 这也是不需要提权的唯一办法。附带好处：单个文件的落盘是原子的，不会留半截。
fn place_file(src: &Path, dst: &Path) -> MainResult<()> {
    if let Some(parent) = dst.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .source_resource()
            .with(parent)?;
    }
    // 目标已存在且是**目录**时，`rename` 只会给出 ENOTDIR/EISDIR 这种看不懂的话，先说人话。
    if std::fs::symlink_metadata(dst)
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
    {
        return Err(MainReason::logic_detail(format!(
            "还原时目标是一个目录，不能用文件覆盖：{}",
            dst.display()
        )));
    }
    let dir = dst.parent().unwrap_or_else(|| Path::new("."));
    let name = dst
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "restored".into());
    // 暂存文件与目标**同目录**：`rename` 才落在同一个文件系统上，且只需目录写权限。
    let staged = dir.join(format!(".{name}.restore-{}", std::process::id()));
    let _ = std::fs::remove_file(&staged);
    if let Err(err) = std::fs::copy(src, &staged) {
        let _ = std::fs::remove_file(&staged);
        return Err(MainReason::logic_detail(format!(
            "写入暂存文件 {} 失败：{err}",
            staged.display()
        )));
    }
    if let Err(err) = std::fs::rename(&staged, dst) {
        let _ = std::fs::remove_file(&staged);
        return Err(MainReason::logic_detail(format!(
            "换上 {} 失败：{err}\n  \
             目标可能归别的属主（现场典型：容器身份 999:999 建的文件），且它所在目录 {} 对当前用户不可写 —— \
             停掉容器后重试，或先 `sudo rm -f` 该文件。",
            dst.display(),
            dir.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// 造一个"现场"：configs/state/*.pem（含私钥）、store.db、一个不声明的 junk.log。
    fn make_site(root: &Path, sys: &str) {
        let s = root.join(sys);
        std::fs::create_dir_all(s.join("configs/state")).unwrap();
        std::fs::write(
            s.join("configs/state/gateway-ca.key.pem"),
            "-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----\n",
        )
        .unwrap();
        std::fs::write(s.join("configs/state/store.db"), b"db").unwrap();
        std::fs::write(s.join("configs/some.log"), b"huge log").unwrap();
        std::fs::create_dir_all(s.join("sys")).unwrap();
        std::fs::write(s.join("sys/sys_model.yml"), "name: web").unwrap();
    }

    fn sel(name: &str, include: &[&str]) -> ResolvedSystem {
        let include = include.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        ResolvedSystem {
            name: name.to_string(),
            include: compile_path_patterns(&include, "backup").unwrap(),
            exclude: Vec::new(),
        }
    }

    /// 造一个最小运维项目（`ops-prj.yml` + `_gal/work.gxl`），供 `OpsProject::load`。
    fn init_project(root: &Path, ops_yaml: &str) {
        std::fs::create_dir_all(root.join("_gal")).unwrap();
        std::fs::write(root.join("_gal/work.gxl"), "mod envs {}\nmod main {}\n").unwrap();
        std::fs::write(root.join("ops-prj.yml"), ops_yaml).unwrap();
    }

    #[test]
    fn test_backup_collects_only_declared_and_flags_secrets() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        make_site(root, "web-stack");
        let out = root.join(".backup");

        let report = backup_systems(
            "cust",
            root,
            &[sel(
                "web-stack",
                &["configs/state/*.pem", "configs/state/store.db"],
            )],
            &out,
            10,
        )
        .unwrap();

        let paths: Vec<&str> = report.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"web-stack/configs/state/gateway-ca.key.pem"));
        assert!(paths.contains(&"web-stack/configs/state/store.db"));
        // 未声明的不收
        assert!(!paths.iter().any(|p| p.ends_with("some.log")));
        // 私钥被点名
        assert_eq!(report.secret_entries().count(), 1);
        assert!(
            report
                .secret_entries()
                .next()
                .unwrap()
                .path
                .ends_with("gateway-ca.key.pem")
        );
        // sha256 是 64 位十六进制
        assert_eq!(report.entries[0].sha256.len(), 64);
    }

    #[test]
    fn test_backup_respects_exclude() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        make_site(root, "web-stack");
        let out = root.join(".backup");

        let mut s = sel("web-stack", &["configs"]);
        s.exclude = compile_path_patterns(&["configs/state/*.pem".to_string()], "backup").unwrap();
        let report = backup_systems("cust", root, &[s], &out, 10).unwrap();
        let paths: Vec<&str> = report.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"web-stack/configs/state/store.db"));
        assert!(!paths.iter().any(|p| p.ends_with(".pem")));
    }

    #[test]
    fn test_backup_prune_keeps_newest() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        make_site(root, "web-stack");
        let out = root.join(".backup");
        std::fs::create_dir_all(&out).unwrap();
        for ts in ["20260101-000000", "20260102-000000", "20260103-000000"] {
            std::fs::write(out.join(format!("cust-{ts}.tar.gz")), b"x").unwrap();
        }

        let report =
            backup_systems("cust", root, &[sel("web-stack", &["configs"])], &out, 2).unwrap();
        // 新打的 1 份 + 原有里保留最新 1 份 = 2 份；被删 2 份
        assert_eq!(report.pruned.len(), 2);
        let left: Vec<String> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(left.len(), 2, "left={left:?}");
    }

    #[test]
    fn test_backup_restore_round_trip() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("site");
        let out = tmp.path().join("backup");
        std::fs::create_dir_all(&root).unwrap();
        make_site(&root, "web-stack");

        let report = backup_systems(
            "cust",
            &root,
            &[sel(
                "web-stack",
                &["configs/state/*.pem", "configs/state/store.db"],
            )],
            &out,
            10,
        )
        .unwrap();

        // 模拟现场丢失：删掉 configs/state
        std::fs::remove_dir_all(root.join("web-stack/configs/state")).unwrap();
        assert!(!root.join("web-stack/configs/state/store.db").exists());

        let r = restore(&root, &report.archive, false).unwrap();
        assert_eq!(r.entries, vec!["web-stack".to_string()]);
        assert_eq!(
            std::fs::read(root.join("web-stack/configs/state/store.db")).unwrap(),
            b"db"
        );
        assert!(
            std::fs::read_to_string(root.join("web-stack/configs/state/gateway-ca.key.pem"))
                .unwrap()
                .contains("BEGIN PRIVATE KEY")
        );
        // 临时目录不留
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with(".restore-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "leftovers={leftovers:?}");
    }

    #[test]
    fn test_restore_replaces_a_target_the_user_cannot_write() {
        // 现场真实一例：`configs/gateway/state/wist-gateway-store.db*` 归**容器身份**
        // （999:999）所有，而 `prj restore` 以部署账号跑 —— 原地覆盖会 EACCES，
        // 于是「备份收得进、还原写不回」。工具得靠「同目录暂存 + rename」换掉它。
        // 这里用 0444 模拟「目标不可写」（rename 只要求目录可写）。
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("site");
        let out = tmp.path().join("backup");
        std::fs::create_dir_all(&root).unwrap();
        make_site(&root, "web-stack");

        let report = backup_systems(
            "cust",
            &root,
            &[sel("web-stack", &["configs/state/store.db"])],
            &out,
            10,
        )
        .unwrap();

        let live = root.join("web-stack/configs/state/store.db");
        let from_backup = std::fs::read(&live).unwrap();
        // 先把现场改成不同内容，再置为「不可写」——模拟「归别人所有、写不动」
        std::fs::write(&live, b"stale").unwrap();
        let mut perm = std::fs::metadata(&live).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&live, perm).unwrap();
        assert!(
            std::fs::write(&live, b"nope").is_err(),
            "前提：目标此时确实不可写"
        );

        restore(&root, &report.archive, false).unwrap();
        assert_eq!(
            std::fs::read(&live).unwrap(),
            from_backup,
            "不可写的目标也必须被换掉（否则就是「备份收得进、还原写不回」那个缺陷）"
        );
        // 不得留下暂存文件
        let leftovers: Vec<_> = std::fs::read_dir(live.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".restore-"))
            .collect();
        assert!(leftovers.is_empty(), "leftovers={leftovers:?}");
    }

    #[test]
    fn test_restore_dry_run_changes_nothing() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("site");
        let out = tmp.path().join("backup");
        std::fs::create_dir_all(&root).unwrap();
        make_site(&root, "web-stack");

        let report = backup_systems(
            "cust",
            &root,
            &[sel("web-stack", &["configs/state/store.db"])],
            &out,
            10,
        )
        .unwrap();
        std::fs::remove_dir_all(root.join("web-stack/configs")).unwrap();

        let r = restore(&root, &report.archive, true).unwrap();
        assert!(r.dry_run);
        assert_eq!(r.entries, vec!["web-stack".to_string()]);
        // dry-run 不落地
        assert!(!root.join("web-stack/configs/state/store.db").exists());
    }

    #[test]
    fn test_list_archive() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        make_site(root, "web-stack");
        let out = root.join(".backup");
        let report = backup_systems(
            "cust",
            root,
            &[sel("web-stack", &["configs/state/store.db"])],
            &out,
            10,
        )
        .unwrap();
        let listed = list_archive(&report.archive).unwrap();
        assert_eq!(listed, vec!["web-stack/configs/state/store.db".to_string()]);
    }

    #[test]
    fn test_entry_info_symlink_hashes_link_target() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("real.txt");
        std::fs::write(&target, "payload").unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink("real.txt", &link).unwrap();

        let info = entry_info("sys/link", &link).unwrap();
        assert_eq!(info.size, 0);
        assert!(!info.secret);
        // 与直接对链接目标字符串取摘要一致
        assert_eq!(info.sha256, hex(&Sha256::digest(b"real.txt")));
    }

    #[test]
    fn test_prune_only_touches_same_project() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        make_site(root, "web-stack");
        let out = root.join(".backup");
        std::fs::create_dir_all(&out).unwrap();
        for ts in ["20260101-000000", "20260102-000000", "20260103-000000"] {
            std::fs::write(out.join(format!("cust-{ts}.tar.gz")), b"x").unwrap();
        }
        // 另一个项目的归档：**不在**清理范围
        std::fs::write(out.join("other-20260101-000000.tar.gz"), b"x").unwrap();

        let report =
            backup_systems("cust", root, &[sel("web-stack", &["configs"])], &out, 1).unwrap();
        assert_eq!(
            report.pruned.len(),
            3,
            "旧 cust-* 应被清掉：{:?}",
            report.pruned
        );
        assert!(
            out.join("other-20260101-000000.tar.gz").exists(),
            "别家项目的归档不能被顺手删"
        );
        let cust_left: Vec<String> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("cust-"))
            .collect();
        assert_eq!(
            cust_left.len(),
            1,
            "keep=1：只在 cust 里留最新一份 {cust_left:?}"
        );
    }

    /// 项目侧 `include`（现场增量）要能收到系统声明之外的路径。
    #[test]
    fn test_ops_project_backup_honors_project_include() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("prj");
        let dest = tmp.path().join("bk");
        init_project(
            &root,
            &format!(
                "name: cust\nwork_envs:\n  dep_root: ''\n  deps: []\nsys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    url: http://example.com/web-stack.tar.gz\nbackup:\n  target: {}\n  systems:\n    - name: web-stack\n      include:\n        - extra/*.txt\n",
                dest.display()
            ),
        );
        let s = root.join("web-stack");
        std::fs::create_dir_all(s.join("configs/state")).unwrap();
        std::fs::write(s.join("configs/state/a.pem"), "pem").unwrap();
        std::fs::create_dir_all(s.join("extra")).unwrap();
        std::fs::write(s.join("extra/note.txt"), "note").unwrap();
        std::fs::write(
            s.join("sys-prj.yml"),
            "test_envs:\n  dep_root: ''\n  deps: []\nbackup:\n  restore:\n    - configs/state/*.pem\n",
        )
        .unwrap();

        let prj = OpsProject::load(&root).unwrap();
        let report = prj.backup(false).unwrap();
        let paths: Vec<&str> = report.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(
            paths.contains(&"web-stack/configs/state/a.pem"),
            "{paths:?}"
        );
        assert!(
            paths.contains(&"web-stack/extra/note.txt"),
            "项目侧 include 的现场增量应被收：{paths:?}"
        );
    }

    /// 项目侧 `systems` 点名了一个未导入/不存在的系统：跳过、不报错。
    #[test]
    fn test_ops_project_backup_skips_unimported_named_system() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("prj");
        let dest = tmp.path().join("bk");
        init_project(
            &root,
            &format!(
                "name: cust\nwork_envs:\n  dep_root: ''\n  deps: []\nsys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    url: http://example.com/web-stack.tar.gz\nbackup:\n  target: {}\n  systems:\n    - name: nope\n",
                dest.display()
            ),
        );

        let prj = OpsProject::load(&root).unwrap();
        let report = prj.backup(false).expect("未导入的系统应跳过而不是报错");
        assert!(report.entries.is_empty(), "{:?}", report.entries);
    }

    #[test]
    fn test_looks_like_secret() {
        assert!(looks_like_secret(b"-----BEGIN PRIVATE KEY-----"));
        assert!(!looks_like_secret(b"just a log line"));
        assert!(!looks_like_secret(b"public info"));
    }

    #[test]
    fn test_collect_matching_skips_top_level_values() {
        let tmp = TempDir::new().unwrap();
        let s = tmp.path().join("web-stack");
        std::fs::create_dir_all(s.join("configs")).unwrap();
        std::fs::write(s.join("configs/a.pem"), "x").unwrap();
        std::os::unix::fs::symlink("../values", s.join("values")).unwrap();

        let inc = compile_path_patterns(&["configs".to_string()], "backup").unwrap();
        let out = collect_matching(&s, &inc, &[]).unwrap();
        assert_eq!(out, vec![PathBuf::from("configs/a.pem")]);
    }

    #[test]
    fn test_prune_no_op_when_keep_zero_or_larger_than_count() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        make_site(root, "web-stack");
        let out = root.join(".backup");
        std::fs::create_dir_all(&out).unwrap();
        for ts in ["20260101-000000", "20260102-000000"] {
            std::fs::write(out.join(format!("cust-{ts}.tar.gz")), b"x").unwrap();
        }

        // keep=0：视为“不清理”（文档化行为），一份不删
        let r0 = backup_systems("cust", root, &[sel("web-stack", &["configs"])], &out, 0).unwrap();
        assert!(r0.pruned.is_empty());
        assert!(out.join("cust-20260101-000000.tar.gz").exists());

        // keep 大于总份数：也不删
        let r9 = backup_systems("cust", root, &[sel("web-stack", &["configs"])], &out, 9).unwrap();
        assert!(r9.pruned.is_empty());
        assert!(out.join("cust-20260101-000000.tar.gz").exists());
    }

    #[test]
    fn test_symlink_survives_backup_restore() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("site");
        let out = tmp.path().join("backup");
        let s = root.join("web-stack");
        std::fs::create_dir_all(s.join("configs")).unwrap();
        std::fs::create_dir_all(s.join("releases/v1")).unwrap();
        std::fs::write(s.join("releases/v1/app.conf"), "v1").unwrap();
        std::os::unix::fs::symlink("releases/v1", s.join("configs/current")).unwrap();

        let report =
            backup_systems("cust", &root, &[sel("web-stack", &["configs"])], &out, 10).unwrap();
        std::fs::remove_dir_all(&s).unwrap();
        restore(&root, &report.archive, false).unwrap();

        let link = s.join("configs/current");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "符号链接应按链接还原，不能解引用"
        );
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            PathBuf::from("releases/v1")
        );
    }

    #[test]
    fn test_restore_recreates_deleted_system_dir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("site");
        let out = tmp.path().join("backup");
        std::fs::create_dir_all(&root).unwrap();
        make_site(&root, "web-stack");

        let report = backup_systems(
            "cust",
            &root,
            &[sel("web-stack", &["configs/state/store.db"])],
            &out,
            10,
        )
        .unwrap();
        // 整目录被删（不只是文件）
        std::fs::remove_dir_all(root.join("web-stack")).unwrap();

        let r = restore(&root, &report.archive, false).unwrap();
        assert_eq!(r.entries, vec!["web-stack".to_string()]);
        assert_eq!(
            std::fs::read(root.join("web-stack/configs/state/store.db")).unwrap(),
            b"db"
        );
    }

    #[test]
    fn test_restore_errors_on_missing_archive() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("site");
        std::fs::create_dir_all(&root).unwrap();
        let missing = tmp.path().join("nope.tar.gz");

        let err = restore(&root, &missing, false).unwrap_err();
        assert!(
            err.detail().is_some() || err.reason().error_code() > 0,
            "应报错而非 panic"
        );
        // 失败路径也不应在现场留下临时目录
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with(".restore-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "leftovers={leftovers:?}");
    }

    #[test]
    fn test_restore_errors_on_corrupt_archive_and_cleans_tmp() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("site");
        std::fs::create_dir_all(&root).unwrap();
        let bad = tmp.path().join("bad.tar.gz");
        std::fs::write(&bad, b"this is not a gzip stream").unwrap();

        assert!(restore(&root, &bad, false).is_err());
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with(".restore-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "leftovers={leftovers:?}");
    }

    /// 项目侧未声明 `backup` 时的默认：落点 `<root>/.backup`、全系统、restore 档。
    #[test]
    fn test_ops_project_backup_default_plan_reads_sys_decl() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("prj");
        std::fs::create_dir_all(root.join("_gal")).unwrap();
        std::fs::write(root.join("_gal/work.gxl"), "mod envs {}\nmod main {}\n").unwrap();
        std::fs::write(
            root.join("ops-prj.yml"),
            "name: cust\nwork_envs:\n  dep_root: ''\n  deps: []\nsys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    url: http://example.com/web-stack.tar.gz\n",
        )
        .unwrap();
        let s = root.join("web-stack");
        std::fs::create_dir_all(s.join("configs/state")).unwrap();
        std::fs::write(s.join("configs/state/x.pem"), "pem").unwrap();
        std::fs::write(s.join("configs/other.log"), "log").unwrap();
        std::fs::write(
            s.join("sys-prj.yml"),
            "test_envs:\n  dep_root: ''\n  deps: []\nbackup:\n  restore:\n    - configs/state/*.pem\n",
        )
        .unwrap();

        let prj = OpsProject::load(&root).unwrap();
        let report = prj.backup(false).unwrap();

        assert!(
            report.archive.starts_with(root.join(".backup")),
            "默认落点应在 <项目根>/.backup：{:?}",
            report.archive
        );
        let paths: Vec<&str> = report.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["web-stack/configs/state/x.pem"]);
    }

    /// 项目侧声明 `level: rebuild` 时应连系统声明的 `rebuild` 档一起收。
    #[test]
    fn test_ops_project_backup_honors_level_and_target() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("prj");
        let dest = tmp.path().join("bk");
        std::fs::create_dir_all(root.join("_gal")).unwrap();
        std::fs::write(root.join("_gal/work.gxl"), "mod envs {}\nmod main {}\n").unwrap();
        std::fs::write(
            root.join("ops-prj.yml"),
            format!(
                "name: cust\nwork_envs:\n  dep_root: ''\n  deps: []\nsys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    url: http://example.com/web-stack.tar.gz\nbackup:\n  target: {}\n  keep: 3\n  systems:\n    - name: web-stack\n      level: rebuild\n",
                dest.display()
            ),
        )
        .unwrap();
        let s = root.join("web-stack");
        std::fs::create_dir_all(s.join("configs")).unwrap();
        std::fs::write(s.join("configs/a.pem"), "pem").unwrap();
        std::fs::create_dir_all(s.join("packages")).unwrap();
        std::fs::write(s.join("packages/pkg.tar.gz"), "pkg").unwrap();
        std::fs::write(
            s.join("sys-prj.yml"),
            "test_envs:\n  dep_root: ''\n  deps: []\nbackup:\n  restore:\n    - configs/*.pem\n  rebuild:\n    - packages\n",
        )
        .unwrap();

        let prj = OpsProject::load(&root).unwrap();
        let report = prj.backup(false).unwrap();

        assert!(
            report.archive.starts_with(&dest),
            "应落到项目声明的 target：{:?}",
            report.archive
        );
        let paths: Vec<&str> = report.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(
            paths.contains(&"web-stack/configs/a.pem"),
            "restore 档：{paths:?}"
        );
        assert!(
            paths.contains(&"web-stack/packages/pkg.tar.gz"),
            "level=rebuild 应带上 rebuild 档：{paths:?}"
        );
    }
}
