//! 系统内容同步：把**交付包的内容**覆盖到已导入的系统目录上，现场态一律不碰；
//! 系统目录缺失时则按包**导入**（补齐被删的系统）。
//!
//! 它补的是这条缺口：`prj sync` 以前只更新项目 conf（`src/ops_prj/project.rs`），
//! 而 `prj import`（以及已移除的 `prj reimport`）都是"删了重装" —— 于是"给已交付的系统
//! 换个版本"没有安全出路，只能靠人手工搬文件。
//!
//! 这个模块存在的全部理由：**不删现场态**。
//!   · `sys-prj.yml: protect:` 命中的路径：不覆盖、不删（身份材料、运行态、客户值…）
//!   · 包外且**未声明**的存量：也不删（保守）
//!   · 包内文件：用新版覆盖
//!   · **上一版包内有、这一版包外**：仅当旧 `deliver.lock` 能证明它未被本地改动时才删
//!     （见 `apply_overlay` 第 5 步）；`prj sync` 默认不清理（需 `--prune`），
//!     `prj upgrade` 因有备份/回滚语境而固定清理。旧锁是 v1（无文件清单）则本轮不删，
//!     由新版锁建立基线。
//!
//! 白名单**就是交付包本身**（包里有什么就覆盖什么）—— 选取逻辑复用 `gops sys package`
//! 那一套（git 入库文件 − `pkg_exclude:` + `pkg_include:`），不在这里重写 git/glob 判定。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::ops_prj::prelude::*;
use crate::system::lock::{DeliverLock, LockedFile};
use crate::system::pack::{compile_path_patterns, path_matches};

use crate::const_vars::SYS_PRJ_CONF_FILE_V2;
use crate::error::MainReason;
use crate::ops_prj::install::fetch_and_prepare;
use crate::ops_prj::project::OpsProject;
use crate::system::SysConf;
use crate::system::spec::SysModelSpec;
use crate::types::Accessor;

/// 一次系统内容更新的结果（供调用方打印与测试断言）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SysContentUpdate {
    pub name: String,
    /// 目标目录原本不存在 → 整包**导入**（不再逐文件覆盖；各计数保持 0）
    pub imported: bool,
    /// 目标里原本没有 → 写入
    pub added: usize,
    /// 目标里已有且内容不同 → 覆盖
    pub replaced: usize,
    /// 目标里已有且内容一致 → 没动
    pub unchanged: usize,
    /// 因命中 `protect:` 而跳过（**不覆盖、不删**）
    pub skipped_protect: usize,
    /// 包外且未声明的顶层条目（**不删**，只列给人看）
    pub undeclared: Vec<String>,
    /// 上一版包内有、这一版包内没有，且**内容未被本地改动** → 已删除的旧文件数
    pub removed: usize,
    /// 上一版包内、这一版包外，但**本地改过** → 保留（列给人看）
    pub kept_modified: Vec<String>,
}

/// 覆盖完成后是否清理「上一版包内、这一版包外」的残渣。
///
/// `prj sync` 默认 `Keep`（保持“非破坏性更新”的原有契约）；需要清理时显式 `--prune`。
/// `prj upgrade` 是升级事务（有备份/回滚语境），固定 `RemoveStale`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PruneMode {
    /// 只覆盖，不删任何东西（`prj sync` 默认）。
    Keep,
    /// 删除旧清单里、新包外、且**未被本地改动**且不在 `protect` 的文件。
    RemoveStale,
}

/// 覆盖计划：由「包内文件清单 + protect 模式」算出的纯函数结果（可单测）。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct OverlayPlan {
    /// 要写到目标里的相对路径（按原顺序）
    pub write: Vec<PathBuf>,
    /// 因命中 `protect:` 被跳过的相对路径
    pub skipped: Vec<PathBuf>,
}

/// 纯函数：把包内文件分成“要写”与“因 protect 跳过”。
pub fn plan_overlay(files: &[PathBuf], patterns: &[glob::Pattern]) -> OverlayPlan {
    let mut plan = OverlayPlan::default();
    for rel in files {
        if path_matches(rel, patterns) {
            plan.skipped.push(rel.clone());
        } else {
            plan.write.push(rel.clone());
        }
    }
    plan
}

impl OpsProject {
    /// 按 `ops-prj.yml` 记录的系统逐个同步：
    ///
    /// · 目录**缺失** → 按包**导入**（补齐被删的系统，不再是静默跳过）；
    /// · 目录**已在** → 把包内内容**非破坏**地覆盖上去（只跳过 `protect` 声明的路径，不删任何东西）。
    pub async fn update_sys_content(
        &self,
        accessor: Accessor,
        options: &DownloadOptions,
        prune: PruneMode,
    ) -> MainResult<Vec<SysContentUpdate>> {
        let mut reports = Vec::new();

        for (name, addr) in self.sys_targets()? {
            let target = self.paths().root().join(&name);
            if !target.is_dir() {
                println!("导入缺失的系统 {name} …");
                self.install_sys_files(&addr, accessor.clone(), options)
                    .await?;
                reports.push(SysContentUpdate {
                    name,
                    imported: true,
                    ..Default::default()
                });
                continue;
            }
            let report = self
                .overlay_one_system(&name, &target, &addr, accessor.clone(), options, prune)
                .await?;
            reports.push(report);
        }
        Ok(reports)
    }

    async fn overlay_one_system(
        &self,
        name: &str,
        target: &Path,
        addr: &str,
        accessor: Accessor,
        options: &DownloadOptions,
        prune: PruneMode,
    ) -> MainResult<SysContentUpdate> {
        // 取包并解开（与 import 同一条路；取不到就在这里失败，**动都没动过目标**）
        let sys_src = fetch_and_prepare(self.paths().clone(), addr, accessor, options).await?;
        apply_overlay(name, &sys_src, target, prune)
    }
}

/// 把已解开的包内容（`sys_src`）覆盖到目标目录（`target`），返回报告。
///
/// 单独成函数是为了可测：取包（要联网/动 `$HOME`）与覆盖分开，测试只需造两个目录。
pub(crate) fn apply_overlay(
    name: &str,
    sys_src: &Path,
    target: &Path,
    prune: PruneMode,
) -> MainResult<SysContentUpdate> {
    // 2) 护栏：包里的系统名必须与目标目录一致，免得把别的系统倒进来
    let pkg_name = SysModelSpec::load_from(&sys_src.join("sys"))?
        .define()
        .name()
        .clone();
    if pkg_name != name {
        return Err(MainReason::logic_detail(format!(
            "包内系统名（{pkg_name}）与 ops-prj.yml 记录的（{name}）不一致：拒绝覆盖 {}",
            target.display()
        )));
    }

    // 3) protect：**包内与目标取并集**（保守 —— 任一方声明过就不碰）
    let mut protect = load_protect(&sys_src.join(SYS_PRJ_CONF_FILE_V2))?;
    for item in load_protect(&target.join(SYS_PRJ_CONF_FILE_V2))? {
        if !protect.contains(&item) {
            protect.push(item);
        }
    }
    let patterns = compile_path_patterns(&protect, "protect")?;

    // 旧版包内清单：用于清理「上一版包内有、这一版包外」的残渣。
    // 必须在覆盖 `deliver.lock` **之前**读：覆盖后就只剩新版清单了。
    let old_files = read_old_manifest(target);

    // 4) 覆盖（逐文件；父目录按需创建；权限沿用源文件）
    let files = walk_files(sys_src)?;
    let plan = plan_overlay(&files, &patterns);
    let mut report = SysContentUpdate {
        name: name.to_string(),
        skipped_protect: plan.skipped.len(),
        ..Default::default()
    };
    for rel in &plan.write {
        let from = sys_src.join(rel);
        let to = target.join(rel);
        let existed = to.symlink_metadata().is_ok();
        if existed && same_bytes(&from, &to) {
            report.unchanged += 1;
            continue;
        }
        copy_entry(&from, &to)?;
        if existed {
            report.replaced += 1;
        } else {
            report.added += 1;
        }
    }

    // 5) 清理「上一版包内、这一版包外」的残渣（仅 `PruneMode::RemoveStale`）。
    //
    // 安全边界（少一条就可能删到现场态）：
    //   · 只认旧清单里的相对路径，拒绝绝对路径 / `..` / `.`（防被篡改的锁删到系统根外）；
    //   · 顶层 `values` 是客户值链接，不碰；`values` 内部可能在链接另一头；
    //   · 祖先目录是符号链接的不碰（否则 `target/link/x` 会写到链接目标）；
    //   · 非普通文件/符号链接（目录、FIFO、socket…）不碰；
    //   · protect 命中的、本地改过的都保留。
    // 旧锁是 v1（无清单）时 `old_files` 为空 → 本轮不清理。
    if prune == PruneMode::RemoveStale {
        let new_files = new_package_files(sys_src, &files);
        let mut removed = 0usize;
        let mut kept_modified = Vec::new();
        for old in &old_files {
            let Some(rel) = safe_manifest_rel(old.path()) else {
                continue;
            };
            if is_top_level_values(&rel) || rel == Path::new(crate::system::lock::DELIVER_LOCK_FILE)
            {
                continue;
            }
            if new_files.contains(&rel) || path_matches(&rel, &patterns) {
                continue;
            }
            let abs = target.join(&rel);
            let Ok(meta) = std::fs::symlink_metadata(&abs) else {
                continue;
            };
            // 目录不能被 `remove_file` 删；FIFO/socket/设备节点也不在我们的管理范围。
            // 直接跳过（而不是让 `old.matches()` 报错拖垮整次更新）。
            if !(meta.is_file() || meta.file_type().is_symlink())
                || has_symlink_ancestor(target, &rel)
            {
                continue;
            }
            if old.matches(&abs) {
                std::fs::remove_file(&abs).source_resource().with(&abs)?;
                removed += 1;
                prune_empty_dirs(target, rel.parent());
            } else {
                kept_modified.push(rel.to_string_lossy().to_string());
            }
        }
        report.removed = removed;
        report.kept_modified = kept_modified;
    }

    // 6) 包外且未声明的**顶层条目**：只列出来（不删）—— 让人看见 configs/、data-plane-run/
    //    这类"这次没被碰"的东西，正是它们构成了"为什么不能盲删"的理由。
    //    放在清理**之后**统计：刚被清掉的旧文件不应再被报成"未改动"。
    report.undeclared = undeclared_top_entries(target, sys_src, &patterns, &protect)?;

    print_report(&report);
    Ok(report)
}

/// 新包**实际提供**的文件集合：解开目录里真实存在的文件 ∪ 新包 `deliver.lock.files`。
///
/// 必须是并集而不是二选一：清单是打包时算的，包到了现场仍可能被解包工具/人工补文件；
/// 旧清单里的同名文件若**新版包也提供**（哪怕清单漏记），一律不能删。
/// 多收只会让清理更保守，不会误删。目录不是文件，`walk_files` 已排除顶层 `values`。
fn new_package_files(sys_src: &Path, walked: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut out: BTreeSet<PathBuf> = walked.iter().cloned().collect();
    if let Ok(lock) = DeliverLock::load(sys_src) {
        out.extend(
            lock.files()
                .iter()
                .filter_map(|file| safe_manifest_rel(file.path())),
        );
    }
    out
}

/// 锁里的路径只接受「纯相对、无 `.` / `..` / 根」的形状。
fn safe_manifest_rel(raw: &str) -> Option<PathBuf> {
    let rel = Path::new(raw);
    if rel.is_absolute() {
        return None;
    }
    if !rel
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(rel.to_path_buf())
}

fn is_top_level_values(rel: &Path) -> bool {
    rel.components().next().map(|c| c.as_os_str() == "values") == Some(true)
}

/// `rel` 的**祖先**（不含最后一级）里是否有符号链接。最后一级允许是链接 —— 要删的就是它本身。
fn has_symlink_ancestor(root: &Path, rel: &Path) -> bool {
    let Some(parent) = rel.parent() else {
        return false;
    };
    let mut cur = root.to_path_buf();
    for comp in parent.components() {
        let std::path::Component::Normal(seg) = comp else {
            return true;
        };
        cur.push(seg);
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => return true,
            Ok(_) => {}
            // 祖先不存在 / 读不了：不碰（保守）
            Err(_) => return true,
        }
    }
    false
}

/// 读目标里上一版包内清单（`deliver.lock`）。
///
/// 无锁 / v1 无清单 → 空（这一轮不清理，由新版锁建立基线）；
/// 有锁但读不出来（YAML 损坏、权限）→ 空 + 告警：不能假装“没有清单”而静默跳过。
fn read_old_manifest(target: &Path) -> Vec<LockedFile> {
    let path = DeliverLock::path(target);
    if !path.exists() {
        return Vec::new();
    }
    match DeliverLock::load(target) {
        Ok(lock) => lock.files().to_vec(),
        Err(e) => {
            eprintln!(
                "  告警：{} 读取失败，本轮不清理上一版残渣（{e}）",
                path.display()
            );
            Vec::new()
        }
    }
}

/// 删文件后自下而上清空目录（只删**空**目录；遇到非空立即停，不越过系统根）。
fn prune_empty_dirs(root: &Path, mut dir: Option<&Path>) {
    while let Some(rel) = dir {
        if rel.as_os_str().is_empty() {
            break;
        }
        if std::fs::remove_dir(root.join(rel)).is_err() {
            break;
        }
        dir = rel.parent().filter(|p| !p.as_os_str().is_empty());
    }
}

/// 读一份 `sys-prj.yml` 的 **`protect`** 声明（升级不覆盖/不删的路径）；文件不存在时为空。
pub(crate) fn load_protect(conf_file: &Path) -> MainResult<Vec<String>> {
    if !conf_file.exists() {
        return Ok(Vec::new());
    }
    let conf = SysConf::load_conf(conf_file).source_resource()?;
    Ok(conf.protect().to_vec())
}

/// 目标里"既不在包内、也不在 protect"的**顶层条目**名（只列一层：够用且不会扫 900M 的目录）。
pub(crate) fn undeclared_top_entries(
    target: &Path,
    sys_src: &Path,
    patterns: &[glob::Pattern],
    raw_protect: &[String],
) -> MainResult<Vec<String>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(target).source_resource().with(target)? {
        let entry = entry.source_resource()?;
        let name = entry.file_name();
        let rel = PathBuf::from(&name);
        // 包内有同名顶层条目 → 归"包内"，不算未声明
        if sys_src.join(&rel).exists() {
            continue;
        }
        if path_matches(&rel, patterns) {
            continue;
        }
        let shown = name.to_string_lossy().to_string();
        // protect 声明可能比顶层更深（如 `configs/gateway/state/*.pem`）：
        // 顶层目录出现在任一声明的路径前缀里，就说明它已是现场态，不算"未声明存量"。
        if raw_protect.iter().any(|p| declared_under(p, &shown)) {
            continue;
        }
        // `values` 是导入时建立的客户值链接、`deliver.lock` 是交付清单自身：
        // 都不是“未声明存量”。
        if shown != "values" && shown != crate::system::lock::DELIVER_LOCK_FILE {
            out.push(shown);
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// 模式文本的归一化形态是否落在顶层条目 `top` 之下（或就是它）。
fn declared_under(pattern: &str, top: &str) -> bool {
    let base = pattern.trim().trim_start_matches("./").trim_matches('/');
    base == top || base.starts_with(&format!("{top}/"))
}

fn print_report(report: &SysContentUpdate) {
    println!(
        "更新 {}：新增 {} / 覆盖 {} / 已是最新 {} / 因 protect 跳过 {}",
        report.name, report.added, report.replaced, report.unchanged, report.skipped_protect
    );
    if report.removed > 0 {
        println!(
            "  清理上一版残渣（包内已移除且未本地改动）：{} 个文件",
            report.removed
        );
    }
    if !report.kept_modified.is_empty() {
        println!(
            "  上一版包内、新版已移除，但**本地改过** → 保留：{}",
            report.kept_modified.join("、")
        );
    }
    if !report.undeclared.is_empty() {
        println!(
            "  包外且未声明（**未改动**，如现场生成物）：{}",
            report.undeclared.join("、")
        );
    }
}

/// 列出包根下所有文件/符号链接的相对路径（跳过 `.git`；目录不入清单）。
pub(crate) fn walk_files(root: &Path) -> MainResult<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root)
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
            .strip_prefix(root)
            .map_err(|e| MainReason::logic_detail(format!("strip prefix failed: {e}")))?;
        // 顶层 `values` 是项目客户值在系统目录里的符号链接（由导入/重建维护），不是包内容：
        // 覆盖它会把客户值指丢。与 backup/undeclared 同一个口径，统一跳过。
        if rel.components().next().map(|c| c.as_os_str() == "values") == Some(true) {
            continue;
        }
        out.push(rel.to_path_buf());
    }
    out.sort();
    Ok(out)
}

/// 覆盖一个条目：父目录按需创建；符号链接按链接复制（不跟随），其余按内容复制（沿用权限）。
pub(crate) fn copy_entry(from: &Path, to: &Path) -> MainResult<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .source_resource()
            .with(parent)?;
    }
    let meta = std::fs::symlink_metadata(from)
        .source_resource()
        .with(from)?;
    if meta.file_type().is_symlink() {
        // 目标已存在（可能是文件）先移除，symlink(2) 不会覆盖已有路径
        remove_existing(to)?;
        let link = std::fs::read_link(from).source_resource().with(from)?;
        std::os::unix::fs::symlink(&link, to)
            .source_resource()
            .with(to)?;
    } else {
        // **关键**：目标若是符号链接，必须先移除 —— 否则 `fs::copy` 会顺着链接
        // 写到链接指向的那个文件上（可能是不相干的现场文件），而不是替换链接本身。
        if std::fs::symlink_metadata(to)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            remove_existing(to)?;
        }
        std::fs::copy(from, to).source_resource().with(to)?;
    }
    Ok(())
}

/// 移除一个已存在的**非目录**条目（文件或符号链接）。
fn remove_existing(path: &Path) -> MainResult<()> {
    if std::fs::symlink_metadata(path).is_ok() {
        std::fs::remove_file(path).source_resource().with(path)?;
    }
    Ok(())
}

/// 内容是否一致。
///
/// 两侧都是符号链接时比**链接目标**（不比目标文件内容 —— 否则指向不同目标、
/// 但目标内容恰好相同的两个链接会被误判为未变化）；否则比大小 + 逐字节。
/// 任一侧读不了就当"不一致"（交给覆盖去报错）。
fn same_bytes(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) else {
        return false;
    };
    let (la, lb) = (ma.file_type().is_symlink(), mb.file_type().is_symlink());
    if la != lb {
        return false;
    }
    if la {
        return std::fs::read_link(a).ok() == std::fs::read_link(b).ok();
    }
    if ma.len() != mb.len() {
        return false;
    }
    let (Ok(ca), Ok(cb)) = (std::fs::read(a), std::fs::read(b)) else {
        return false;
    };
    ca == cb
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns(items: &[&str]) -> Vec<glob::Pattern> {
        let raw: Vec<String> = items.iter().map(|s| s.to_string()).collect();
        compile_path_patterns(&raw, "protect").unwrap()
    }

    #[test]
    fn test_plan_overlay_splits_write_and_skipped() {
        let files = vec![
            PathBuf::from("sys/sys_model.yml"),
            PathBuf::from("configs/gateway/wist-gateway.toml"),
            PathBuf::from("state/wist-gateway-store.db"),
        ];
        let pats = patterns(&["configs", ".env"]);
        let plan = plan_overlay(&files, &pats);

        assert_eq!(
            plan.write,
            vec![
                PathBuf::from("sys/sys_model.yml"),
                PathBuf::from("state/wist-gateway-store.db"),
            ]
        );
        assert_eq!(
            plan.skipped,
            vec![PathBuf::from("configs/gateway/wist-gateway.toml")]
        );
    }

    #[test]
    fn test_plan_overlay_no_patterns_writes_all() {
        let files = vec![PathBuf::from("a"), PathBuf::from("b/c")];
        let plan = plan_overlay(&files, &[]);
        assert_eq!(plan.write.len(), 2);
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn test_declared_under_handles_normalization() {
        assert!(declared_under("configs", "configs"));
        assert!(declared_under("./configs/", "configs"));
        assert!(declared_under("configs/gateway/state/*.pem", "configs"));
        assert!(!declared_under("configs-web", "configs"));
        assert!(!declared_under("data-plane-run", "configs"));
    }

    #[test]
    fn test_apply_overlay_covers_package_keeps_protect_leaves_undeclared() {
        use crate::system::spec::{SysDefine, SysModelSpec};

        let tmp = tempfile::TempDir::new().unwrap();
        // 包侧：用库生成一个合法的 sys/ 目录，再补上 sys-prj.yml 与内容文件
        let spec = SysModelSpec::make_new(SysDefine::new_without_model("web-stack")).unwrap();
        spec.save_local_minimal(&tmp.path().join("pkg"), "sys")
            .unwrap();
        let sys_src = tmp.path().join("pkg");
        std::fs::write(
            sys_src.join("sys-prj.yml"),
            "test_envs:\n  dep_root: ''\n  deps: []\nprotect:\n  - configs\n",
        )
        .unwrap();
        std::fs::write(sys_src.join("app.txt"), "new").unwrap();
        std::fs::create_dir_all(sys_src.join("configs")).unwrap();
        std::fs::write(sys_src.join("configs/default.toml"), "pkg-default").unwrap();

        // 目标侧：app.txt 旧值、configs 现场值（要保住）、junk.txt（包外未声明）
        let target = tmp.path().join("site");
        std::fs::create_dir_all(target.join("configs")).unwrap();
        std::fs::write(target.join("app.txt"), "old").unwrap();
        std::fs::write(target.join("configs/default.toml"), "site-value").unwrap();
        std::fs::write(target.join("junk.txt"), "junk").unwrap();

        let report = apply_overlay("web-stack", &sys_src, &target, PruneMode::Keep).unwrap();

        // 包内覆盖
        assert_eq!(
            std::fs::read_to_string(target.join("app.txt")).unwrap(),
            "new"
        );
        assert!(target.join("sys/sys_model.yml").exists());
        assert!(report.added >= 1);
        assert_eq!(report.replaced, 1);
        // protect 命中：包里的 configs/default.toml 不覆盖现场值
        assert_eq!(report.skipped_protect, 1);
        assert_eq!(
            std::fs::read_to_string(target.join("configs/default.toml")).unwrap(),
            "site-value"
        );
        // 包外未声明：列出但不删
        assert_eq!(report.undeclared, vec!["junk.txt".to_string()]);
        assert_eq!(
            std::fs::read_to_string(target.join("junk.txt")).unwrap(),
            "junk"
        );
    }

    #[test]
    fn test_apply_overlay_rejects_name_mismatch() {
        use crate::system::spec::{SysDefine, SysModelSpec};

        let tmp = tempfile::TempDir::new().unwrap();
        let spec = SysModelSpec::make_new(SysDefine::new_without_model("other")).unwrap();
        spec.save_local_minimal(&tmp.path().join("pkg"), "sys")
            .unwrap();
        let sys_src = tmp.path().join("pkg");
        let target = tmp.path().join("site");
        std::fs::create_dir_all(&target).unwrap();

        let err = apply_overlay("web-stack", &sys_src, &target, PruneMode::Keep).unwrap_err();
        assert!(
            err.detail()
                .as_deref()
                .is_some_and(|d| d.contains("不一致")),
            "detail={:?}",
            err.detail()
        );
    }

    #[test]
    fn test_walk_files_skips_top_level_values() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("pkg");
        std::fs::create_dir_all(root.join("sys")).unwrap();
        std::fs::write(root.join("sys/sys_model.yml"), "x").unwrap();
        std::fs::write(root.join("app.txt"), "y").unwrap();
        // 包自带一个 values 符号链接（某些包会带）——不应被当成包内容
        std::os::unix::fs::symlink("../values", root.join("values")).unwrap();

        let files = walk_files(&root).unwrap();
        assert!(
            !files.iter().any(|p| p.starts_with("values")),
            "files={files:?}"
        );
        assert!(files.contains(&PathBuf::from("sys/sys_model.yml")));
        assert!(files.contains(&PathBuf::from("app.txt")));
    }

    #[test]
    fn test_copy_entry_does_not_write_through_symlink() {
        let tmp = tempfile::TempDir::new().unwrap();
        let victim = tmp.path().join("victim.txt");
        std::fs::write(&victim, "important").unwrap();
        let dst = tmp.path().join("dst.txt");
        // 现场 dst 是指向 victim 的符号链接
        std::os::unix::fs::symlink(&victim, &dst).unwrap();
        // 包内 dst 是普通文件
        let src = tmp.path().join("src.txt");
        std::fs::write(&src, "new").unwrap();

        copy_entry(&src, &dst).unwrap();

        // victim 未被写穿；dst 已变成普通文件
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "important");
        assert!(std::fs::symlink_metadata(&dst).unwrap().is_file());
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "new");
    }

    #[test]
    fn test_same_bytes_compares_symlink_targets_not_contents() {
        let tmp = tempfile::TempDir::new().unwrap();
        let t1 = tmp.path().join("t1");
        let t2 = tmp.path().join("t2");
        std::fs::write(&t1, "same").unwrap();
        std::fs::write(&t2, "same").unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::os::unix::fs::symlink("t1", &a).unwrap();
        std::os::unix::fs::symlink("t2", &b).unwrap();
        // 目标内容相同但链接目标不同 → 不得判为一致
        assert!(!same_bytes(&a, &b));

        // 同样目标的链接 → 一致
        let c = tmp.path().join("c");
        std::os::unix::fs::symlink("t1", &c).unwrap();
        assert!(same_bytes(&a, &c));

        // 类型不同（链接 vs 普通文件）→ 不一致
        assert!(!same_bytes(&a, &t1));
    }

    #[test]
    fn test_walk_files_skips_git_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("pkg");
        std::fs::create_dir_all(root.join(".git/objects")).unwrap();
        std::fs::write(root.join(".git/config"), "x").unwrap();
        std::fs::write(root.join("app.txt"), "y").unwrap();

        let files = walk_files(&root).unwrap();
        assert_eq!(files, vec![PathBuf::from("app.txt")]);
    }

    #[test]
    fn test_load_protect_errors_on_bad_yaml() {
        let tmp = tempfile::TempDir::new().unwrap();
        let f = tmp.path().join("sys-prj.yml");
        std::fs::write(&f, "protect: [oops\n").unwrap();
        assert!(load_protect(&f).is_err(), "坏 YAML 不能静默当空声明");
    }

    #[test]
    fn test_copy_entry_errors_when_target_is_a_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src.txt");
        std::fs::write(&src, "new").unwrap();
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(dst.join("keep.txt"), "keep").unwrap();

        // 目标是目录：报错，且**不能**损害目录里的东西
        assert!(copy_entry(&src, &dst).is_err());
        assert_eq!(
            std::fs::read_to_string(dst.join("keep.txt")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn test_undeclared_top_entries_lists_only_undeclared() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sys_src = tmp.path().join("pkg");
        let target = tmp.path().join("site");
        // 包内有 sys/；目标里另有 configs/（protect 声明）与 junk/
        std::fs::create_dir_all(sys_src.join("sys")).unwrap();
        std::fs::write(sys_src.join("sys/sys_model.yml"), "x").unwrap();
        std::fs::create_dir_all(target.join("sys")).unwrap();
        std::fs::create_dir_all(target.join("configs/gateway/state")).unwrap();
        std::fs::create_dir_all(target.join("junk")).unwrap();
        std::os::unix::fs::symlink("../values/x", target.join("values")).unwrap();

        let raw = vec!["configs/gateway/state/*.pem".to_string()];
        let pats = compile_path_patterns(&raw, "protect").unwrap();
        let out = undeclared_top_entries(&target, &sys_src, &pats, &raw).unwrap();
        // configs 被 protect 声明覆盖，values 是客户值链接，sys 在包内 → 只剩 junk
        assert_eq!(out, vec!["junk".to_string()]);
    }

    /// 用库生成一个合法的 `<root>/sys/` 目录（`apply_overlay` 的取名护栏需要它）。
    fn make_pkg(root: &Path, define_name: &str) {
        use crate::system::spec::{SysDefine, SysModelSpec};
        let spec = SysModelSpec::make_new(SysDefine::new_without_model(define_name)).unwrap();
        spec.save_local_minimal(root, "sys").unwrap();
    }

    /// 造一个「能过 `DeliverLock::build`」的完整系统目录（`save_local_minimal` 不够）。
    fn make_full_pkg(root: &Path, name: &str) {
        use crate::system::spec::{SysDefine, SysModelSpec};
        let spec = SysModelSpec::make_new(SysDefine::new_without_model(name)).unwrap();
        spec.save_local(root, "sys").unwrap();
    }

    /// 把当前目标树的文件清单写进 `deliver.lock`（模拟「上一版包」建立基线）。
    fn write_manifest_lock(target: &Path) {
        let files = walk_files(target).unwrap();
        let lock = crate::system::lock::DeliverLock::build_with_files(target, &files).unwrap();
        lock.save(target).unwrap();
    }

    /// 直接写一份含**指定清单**的 v2 锁（构造被篡改/异常锁的场景，绕过实际文件）。
    fn write_lock_with_manifest(target: &Path, entries: &[(&str, String)]) {
        let mut yaml = String::from(
            "lockfile_version: 2\nname: web-stack\nversion: 1.0.0\nkind: gxl\n\
             generated_at: '2026-01-01T00:00:00Z'\nmodules: []\nhashes: {}\nfiles:\n",
        );
        for (path, sha) in entries {
            yaml.push_str(&format!("  - path: {path:?}\n    sha256: {sha:?}\n"));
        }
        std::fs::write(target.join("deliver.lock"), yaml).unwrap();
    }

    fn sha_of(path: &Path) -> String {
        crate::system::lock::hash_entry(path).unwrap()
    }

    #[test]
    fn test_load_protect_missing_is_empty_and_present_parses() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(
            load_protect(&tmp.path().join("nope.yml"))
                .unwrap()
                .is_empty()
        );

        let f = tmp.path().join("sys-prj.yml");
        std::fs::write(
            &f,
            "test_envs:\n  dep_root: ''\n  deps: []\nprotect:\n  - .env\n  - configs\n",
        )
        .unwrap();
        assert_eq!(
            load_protect(&f).unwrap(),
            vec![".env".to_string(), "configs".to_string()]
        );
    }

    #[test]
    fn test_load_protect_reads_legacy_preserve_alias() {
        // 回到旧名：存量站点 `sys-prj.yml` 里的 `preserve:` 仍要生效
        // （`prj sync` / `prj upgrade` 都走 `load_protect`）。
        let tmp = tempfile::TempDir::new().unwrap();
        let f = tmp.path().join("sys-prj.yml");
        std::fs::write(
            &f,
            "test_envs:\n  dep_root: ''\n  deps: []\npreserve:\n  - .env\n",
        )
        .unwrap();
        assert_eq!(load_protect(&f).unwrap(), vec![".env".to_string()]);
    }

    #[test]
    fn test_load_protect_is_protect_only() {
        // 各声明互相独立：`load_protect` 只读 `protect`，不含 `contract`。
        let tmp = tempfile::TempDir::new().unwrap();
        let f = tmp.path().join("sys-prj.yml");
        std::fs::write(
            &f,
            "test_envs:\n  dep_root: ''\n  deps: []\nprotect:\n  - .env\ncontract:\n  - configs/gateway/public\n",
        )
        .unwrap();
        assert_eq!(load_protect(&f).unwrap(), vec![".env".to_string()]);
    }

    #[test]
    fn test_apply_overlay_counts_added_replaced_unchanged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sys_src = tmp.path().join("pkg");
        make_pkg(&sys_src, "web-stack");
        std::fs::write(sys_src.join("same.txt"), "S").unwrap();
        std::fs::write(sys_src.join("diff.txt"), "NEW").unwrap();

        let target = tmp.path().join("site");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("same.txt"), "S").unwrap();
        std::fs::write(target.join("diff.txt"), "OLD").unwrap();

        let report = apply_overlay("web-stack", &sys_src, &target, PruneMode::Keep).unwrap();
        assert_eq!(report.replaced, 1, "diff.txt 内容不同 → 覆盖");
        assert_eq!(report.unchanged, 1, "same.txt 内容相同 → 不动");
        assert!(report.added >= 2, "sys/ 下的文件是新增：{}", report.added);
        assert_eq!(report.skipped_protect, 0);
        assert_eq!(
            std::fs::read_to_string(target.join("diff.txt")).unwrap(),
            "NEW"
        );
    }

    #[test]
    fn test_apply_overlay_protect_is_union_of_package_and_target() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sys_src = tmp.path().join("pkg");
        make_pkg(&sys_src, "web-stack");
        std::fs::write(
            sys_src.join("sys-prj.yml"),
            "test_envs:\n  dep_root: ''\n  deps: []\nprotect:\n  - configs\n",
        )
        .unwrap();
        std::fs::create_dir_all(sys_src.join("configs")).unwrap();
        std::fs::write(sys_src.join("configs/pkg.toml"), "pkg-default").unwrap();
        std::fs::create_dir_all(sys_src.join("site_only")).unwrap();
        std::fs::write(sys_src.join("site_only/y.toml"), "pkg-site").unwrap();

        // 目标自己声明了 site_only 为现场态（包侧没声明）→ 并集后也应跳过
        let target = tmp.path().join("site");
        std::fs::create_dir_all(target.join("configs")).unwrap();
        std::fs::create_dir_all(target.join("site_only")).unwrap();
        std::fs::write(
            target.join("sys-prj.yml"),
            "test_envs:\n  dep_root: ''\n  deps: []\nprotect:\n  - site_only\n",
        )
        .unwrap();
        std::fs::write(target.join("configs/pkg.toml"), "site-configs").unwrap();

        let report = apply_overlay("web-stack", &sys_src, &target, PruneMode::Keep).unwrap();
        assert_eq!(report.skipped_protect, 2, "包侧 configs + 目标侧 site_only");
        // 两侧现场值都保住
        assert_eq!(
            std::fs::read_to_string(target.join("configs/pkg.toml")).unwrap(),
            "site-configs"
        );
        assert!(!target.join("site_only/y.toml").exists());
    }

    #[test]
    fn test_apply_overlay_recreates_package_symlink() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sys_src = tmp.path().join("pkg");
        make_pkg(&sys_src, "web-stack");
        std::os::unix::fs::symlink("sys/sys_model.yml", sys_src.join("link")).unwrap();

        let target = tmp.path().join("site");
        std::fs::create_dir_all(&target).unwrap();

        apply_overlay("web-stack", &sys_src, &target, PruneMode::Keep).unwrap();
        let meta = std::fs::symlink_metadata(target.join("link")).unwrap();
        assert!(meta.file_type().is_symlink(), "包内符号链接应按链接复制");
        assert_eq!(
            std::fs::read_link(target.join("link")).unwrap(),
            PathBuf::from("sys/sys_model.yml")
        );
    }

    /// 系统目录缺失 → `sync` 会去**取包并导入**（不再静默跳过）；包不可达时就地报错。
    #[tokio::test]
    async fn test_update_sys_content_imports_missing_system() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("prj");
        std::fs::create_dir_all(root.join("_gal")).unwrap();
        std::fs::write(root.join("_gal/work.gxl"), "mod envs {}\nmod main {}\n").unwrap();
        // addr 指向不存在的本地包：目录缺失 → 现在会去导入，因而取包失败（而不是被跳过）
        std::fs::write(
            root.join("ops-prj.yml"),
            "name: cust\nwork_envs:\n  dep_root: ''\n  deps: []\nsys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    path: /nonexistent/web-stack-0.1.0.tar.gz\n",
        )
        .unwrap();

        let prj = OpsProject::load(&root).unwrap();
        let accessor = crate::accessor::accessor_for_test();
        let opts = DownloadOptions::from((0usize, ValueDict::default()));
        let err = prj
            .update_sys_content(accessor, &opts, PruneMode::Keep)
            .await
            .unwrap_err();
        // 证明“缺失系统”走了取包这条路（包不存在 → 报错），而非旧行为“静默跳过”
        assert!(
            err.to_string().to_lowercase().contains("nonexistent") || err.detail().is_some(),
            "err={err:?}"
        );
        // 未落地系统目录
        assert!(!root.join("web-stack").exists());
    }

    #[test]
    fn test_apply_overlay_removes_unmodified_file_dropped_by_new_version() {
        let tmp = tempfile::TempDir::new().unwrap();
        // 旧版包（= 当前现场）：app.txt + removed.txt，并写下一版清单
        let target = tmp.path().join("site");
        make_full_pkg(&target, "web-stack");
        std::fs::write(target.join("app.txt"), "old").unwrap();
        std::fs::write(target.join("removed.txt"), "gone").unwrap();
        write_manifest_lock(&target);

        // 新版包：没有 removed.txt，app.txt 更新
        let sys_src = tmp.path().join("pkg");
        make_full_pkg(&sys_src, "web-stack");
        std::fs::write(sys_src.join("app.txt"), "new").unwrap();

        let report = apply_overlay("web-stack", &sys_src, &target, PruneMode::RemoveStale).unwrap();
        assert_eq!(
            report.removed, 1,
            "removed.txt 应被清掉（旧包内、新包外、未改动）"
        );
        assert!(!target.join("removed.txt").exists());
        assert!(
            report.undeclared.is_empty(),
            "清掉的残渣与交付锁都不应再被列为未声明: {:?}",
            report.undeclared
        );
        assert_eq!(
            std::fs::read_to_string(target.join("app.txt")).unwrap(),
            "new"
        );
    }

    #[test]
    fn test_apply_overlay_keeps_modified_and_protected_stale_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("site");
        make_full_pkg(&target, "web-stack");
        std::fs::create_dir_all(target.join("configs")).unwrap();
        std::fs::write(target.join("configs/keep.toml"), "site-value").unwrap();
        std::fs::write(target.join("local.txt"), "v1").unwrap();
        std::fs::write(target.join("gone.txt"), "gone").unwrap();
        write_manifest_lock(&target);
        // 清单建立后本地改了 local.txt：不能被当成「旧包残渣」删掉
        std::fs::write(target.join("local.txt"), "hand-edited").unwrap();

        // 新版包：三个文件都不再提供；configs 声明为 protect
        let sys_src = tmp.path().join("pkg");
        make_full_pkg(&sys_src, "web-stack");
        std::fs::write(
            sys_src.join("sys-prj.yml"),
            "test_envs:\n  dep_root: ''\n  deps: []\nprotect:\n  - configs\n",
        )
        .unwrap();

        let report = apply_overlay("web-stack", &sys_src, &target, PruneMode::RemoveStale).unwrap();
        assert_eq!(report.removed, 1, "只应清掉未改动的 gone.txt");
        assert!(!target.join("gone.txt").exists());
        assert_eq!(
            std::fs::read_to_string(target.join("local.txt")).unwrap(),
            "hand-edited",
            "本地改过的文件必须保留"
        );
        assert_eq!(
            report.kept_modified,
            vec!["local.txt".to_string()],
            "保留原因要报出来"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("configs/keep.toml")).unwrap(),
            "site-value",
            "protect 命中的旧文件不删"
        );
    }

    #[test]
    fn test_apply_overlay_v1_lock_without_manifest_does_not_prune() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("site");
        make_full_pkg(&target, "web-stack");
        std::fs::write(target.join("old.txt"), "keep").unwrap();
        // v1 锁：没有 files 清单 → 无法证明 old.txt 是旧包内容
        std::fs::write(
            target.join("deliver.lock"),
            "lockfile_version: 1\nname: web-stack\nversion: 1.0.0\nkind: gxl\n\
             generated_at: '2026-01-01T00:00:00Z'\nmodules: []\nhashes: {}\n",
        )
        .unwrap();

        let sys_src = tmp.path().join("pkg");
        make_full_pkg(&sys_src, "web-stack");

        let report = apply_overlay("web-stack", &sys_src, &target, PruneMode::RemoveStale).unwrap();
        assert_eq!(report.removed, 0, "无清单的 v1 锁不得清理任何文件");
        assert!(target.join("old.txt").exists());
    }

    #[test]
    fn test_apply_overlay_corrupt_lock_does_not_prune() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("site");
        make_full_pkg(&target, "web-stack");
        std::fs::write(target.join("old.txt"), "keep").unwrap();
        std::fs::write(target.join("deliver.lock"), "lockfile_version: [broken").unwrap();

        let sys_src = tmp.path().join("pkg");
        make_full_pkg(&sys_src, "web-stack");

        let report = apply_overlay("web-stack", &sys_src, &target, PruneMode::RemoveStale).unwrap();
        assert_eq!(report.removed, 0, "锁读不出来时不能凭空猜清单去删");
        assert!(target.join("old.txt").exists());
    }

    #[test]
    fn test_apply_overlay_prune_rejects_traversal_and_absolute_manifest_paths() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("site");
        make_full_pkg(&target, "web-stack");
        // 系统根外的“靶子”：穿越/绝对路径若没被拒绝就会被删掉
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        let sha = sha_of(&outside);
        let abs = outside.to_string_lossy().to_string();
        write_lock_with_manifest(
            &target,
            &[("../outside.txt", sha.clone()), (abs.as_str(), sha)],
        );

        let sys_src = tmp.path().join("pkg");
        make_full_pkg(&sys_src, "web-stack");

        let report = apply_overlay("web-stack", &sys_src, &target, PruneMode::RemoveStale).unwrap();
        assert_eq!(report.removed, 0);
        assert!(outside.exists(), "不得删到系统根之外");
    }

    #[test]
    fn test_apply_overlay_prune_skips_symlinked_ancestor() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("site");
        make_full_pkg(&target, "web-stack");
        // 现场把 data 指到系统根外（数据盘挂载/软链都有先例）
        let outside_dir = tmp.path().join("outside");
        std::fs::create_dir_all(&outside_dir).unwrap();
        let victim = outside_dir.join("victim.txt");
        std::fs::write(&victim, "pkg-copy").unwrap();
        std::os::unix::fs::symlink(&outside_dir, target.join("data")).unwrap();
        write_lock_with_manifest(&target, &[("data/victim.txt", sha_of(&victim))]);

        let sys_src = tmp.path().join("pkg");
        make_full_pkg(&sys_src, "web-stack");

        let report = apply_overlay("web-stack", &sys_src, &target, PruneMode::RemoveStale).unwrap();
        assert_eq!(report.removed, 0);
        assert!(victim.exists(), "祖先为符号链接时不得顺着链接删");
    }

    #[test]
    fn test_apply_overlay_prune_skips_non_file_stale_entries() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("site");
        make_full_pkg(&target, "web-stack");
        // 旧清单里的文件位置在现场变成了目录：不能 remove_file，跳过即可
        std::fs::create_dir_all(target.join("became-dir")).unwrap();
        std::fs::write(target.join("became-dir/inner.txt"), "x").unwrap();
        write_lock_with_manifest(&target, &[("became-dir", "sha256:00".to_string())]);

        let sys_src = tmp.path().join("pkg");
        make_full_pkg(&sys_src, "web-stack");

        let report = apply_overlay("web-stack", &sys_src, &target, PruneMode::RemoveStale).unwrap();
        assert_eq!(report.removed, 0, "非普通文件不参与清理，更不能让更新报错");
        assert!(target.join("became-dir/inner.txt").exists());
    }

    #[test]
    fn test_apply_overlay_prune_removes_empty_dirs_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("site");
        make_full_pkg(&target, "web-stack");
        std::fs::create_dir_all(target.join("extra/deep")).unwrap();
        std::fs::write(target.join("extra/deep/gone.txt"), "gone").unwrap();
        std::fs::create_dir_all(target.join("mixed/sub")).unwrap();
        std::fs::write(target.join("mixed/sub/also-gone.txt"), "gone").unwrap();
        write_manifest_lock(&target);
        // 清单建立后现场新增的文件：不属于旧包，但会让父目录非空 → 清目录必须停在它上面
        std::fs::write(target.join("mixed/keep.txt"), "site-local").unwrap();

        let sys_src = tmp.path().join("pkg");
        make_full_pkg(&sys_src, "web-stack");

        let report = apply_overlay("web-stack", &sys_src, &target, PruneMode::RemoveStale).unwrap();
        assert_eq!(report.removed, 2);
        assert!(!target.join("extra").exists(), "空目录链应自下而上回收");
        assert!(!target.join("mixed/sub").exists());
        assert!(
            target.join("mixed/keep.txt").exists(),
            "父目录非空时必须停下，且本地文件不得被删"
        );
        assert!(target.join("mixed").exists());
    }

    #[test]
    fn test_apply_overlay_prune_keeps_file_new_package_ships_but_manifest_missed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("site");
        make_full_pkg(&target, "web-stack");
        std::fs::write(target.join("late.txt"), "same").unwrap();
        write_manifest_lock(&target); // 旧清单含 late.txt

        // 新包：先写清单（不含 late.txt），再补上内容与之相同的 late.txt
        // （模拟打包后补文件 / 清单与包内容短暂不一致）
        let sys_src = tmp.path().join("pkg");
        make_full_pkg(&sys_src, "web-stack");
        write_manifest_lock(&sys_src);
        std::fs::write(sys_src.join("late.txt"), "same").unwrap();

        let report = apply_overlay("web-stack", &sys_src, &target, PruneMode::RemoveStale).unwrap();
        assert_eq!(report.removed, 0);
        assert!(
            target.join("late.txt").exists(),
            "新版包实际提供的文件，即使清单漏记也不得当成残渣删除"
        );
    }
}
