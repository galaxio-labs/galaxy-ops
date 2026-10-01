//! 非破坏性更新：把**交付包的内容**覆盖到已导入的系统目录上，现场态一律不碰。
//!
//! 它补的是这条缺口：`prj update` 以前只更新项目 conf（`src/ops_prj/project.rs`），
//! `prj import`/`reimport` 则是"删了重装" —— 于是"给已交付的系统换个版本"没有安全出路，
//! 只能靠人手工搬文件。
//!
//! 与 `reimport` 的差别就是这个模块存在的全部理由：**不删任何东西**。
//!   · `sys-prj.yml: preserve:` 命中的路径：不覆盖、不删（身份材料、运行态、客户值…）
//!   · 包外且**未声明**的存量：也不删（保守；只有 `prj rebuild` 才会动它们）
//!   · 包内文件：用新版覆盖
//!
//! 白名单**就是交付包本身**（包里有什么就覆盖什么）—— 选取逻辑复用 `gops sys package`
//! 那一套（git 入库文件 − `ignore:`），不在这里重写 git/glob 判定。

use std::path::{Path, PathBuf};

use crate::ops_prj::prelude::*;
use crate::system::pack::{compile_path_patterns, path_matches};

use crate::const_vars::SYS_PRJ_CONF_FILE_V2;
use crate::error::MainReason;
use crate::ops_prj::import::addr_to_path_string;
use crate::ops_prj::install::fetch_and_prepare;
use crate::ops_prj::project::OpsProject;
use crate::system::SysConf;
use crate::system::spec::SysModelSpec;
use crate::types::Accessor;

/// 一次系统内容更新的结果（供调用方打印与测试断言）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SysContentUpdate {
    pub name: String,
    /// 目标里原本没有 → 写入
    pub added: usize,
    /// 目标里已有且内容不同 → 覆盖
    pub replaced: usize,
    /// 目标里已有且内容一致 → 没动
    pub unchanged: usize,
    /// 因命中 `preserve:` 而跳过（**不覆盖、不删**）
    pub skipped_preserve: usize,
    /// 包外且未声明的顶层条目（**不删**，只列给人看）
    pub undeclared: Vec<String>,
}

/// 覆盖计划：由「包内文件清单 + preserve 模式」算出的纯函数结果（可单测）。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct OverlayPlan {
    /// 要写到目标里的相对路径（按原顺序）
    pub write: Vec<PathBuf>,
    /// 因命中 `preserve:` 被跳过的相对路径
    pub skipped: Vec<PathBuf>,
}

/// 纯函数：把包内文件分成“要写”与“因 preserve 跳过”。
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
    /// 按 `ops-prj.yml` 记录的系统，逐个把包内内容覆盖到 `<项目根>/<系统名>/`。
    ///
    /// 未导入的系统跳过（提示用 `prj import`），不报错 —— 这个命令的语义是"更新已有的"。
    pub async fn update_sys_content(
        &self,
        accessor: Accessor,
        options: &DownloadOptions,
    ) -> MainResult<Vec<SysContentUpdate>> {
        let mut reports = Vec::new();
        let targets: Vec<(String, String)> = self
            .conf()
            .sys_models()
            .iter()
            .map(|sys| (sys.sys().name().clone(), addr_to_path_string(sys.addr())))
            .collect();

        for (name, addr) in targets {
            let target = self.paths().root().join(&name);
            if !target.is_dir() {
                println!("跳过 {name}：未导入（先 `gops prj import`）");
                continue;
            }
            let report = self
                .overlay_one_system(&name, &target, &addr, accessor.clone(), options)
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
    ) -> MainResult<SysContentUpdate> {
        // 取包并解开（与 import 同一条路；取不到就在这里失败，**动都没动过目标**）
        let sys_src = fetch_and_prepare(self.paths().clone(), addr, accessor, options).await?;
        apply_overlay(name, &sys_src, target)
    }
}

/// 把已解开的包内容（`sys_src`）覆盖到目标目录（`target`），返回报告。
///
/// 单独成函数是为了可测：取包（要联网/动 `$HOME`）与覆盖分开，测试只需造两个目录。
pub(crate) fn apply_overlay(
    name: &str,
    sys_src: &Path,
    target: &Path,
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

    // 3) preserve：**包内与目标取并集**（保守 —— 任一方声明过就不碰）
    let mut preserve = load_preserve(&sys_src.join(SYS_PRJ_CONF_FILE_V2))?;
    for item in load_preserve(&target.join(SYS_PRJ_CONF_FILE_V2))? {
        if !preserve.contains(&item) {
            preserve.push(item);
        }
    }
    let patterns = compile_path_patterns(&preserve, "preserve")?;

    // 4) 覆盖（逐文件；父目录按需创建；权限沿用源文件）
    let files = walk_files(sys_src)?;
    let plan = plan_overlay(&files, &patterns);
    let mut report = SysContentUpdate {
        name: name.to_string(),
        skipped_preserve: plan.skipped.len(),
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

    // 5) 包外且未声明的**顶层条目**：只列出来（不删）—— 让人看见 configs/、data-plane-run/
    //    这类"这次没被碰"的东西，正是它们构成了"为什么不能盲删"的理由。
    report.undeclared = undeclared_top_entries(target, sys_src, &patterns, &preserve)?;

    print_report(&report);
    Ok(report)
}

/// 读一份 `sys-prj.yml` 的 `preserve:`；文件不存在时为空。
pub(crate) fn load_preserve(conf_file: &Path) -> MainResult<Vec<String>> {
    if !conf_file.exists() {
        return Ok(Vec::new());
    }
    let conf = SysConf::load_conf(conf_file).source_resource()?;
    Ok(conf.preserve().to_vec())
}

/// 目标里"既不在包内、也不在 preserve"的**顶层条目**名（只列一层：够用且不会扫 900M 的目录）。
pub(crate) fn undeclared_top_entries(
    target: &Path,
    sys_src: &Path,
    patterns: &[glob::Pattern],
    raw_preserve: &[String],
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
        // preserve 声明可能比顶层更深（如 `configs/gateway/state/*.pem`）：
        // 顶层目录出现在任一声明的路径前缀里，就说明它已是现场态，不算"未声明存量"。
        if raw_preserve.iter().any(|p| declared_under(p, &shown)) {
            continue;
        }
        // `values` 是导入时建立的客户值符号链接，不是"未声明存量"
        if shown != "values" {
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
        "更新 {}：新增 {} / 覆盖 {} / 已是最新 {} / 因 preserve 跳过 {}",
        report.name, report.added, report.replaced, report.unchanged, report.skipped_preserve
    );
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
fn copy_entry(from: &Path, to: &Path) -> MainResult<()> {
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
        compile_path_patterns(&raw, "preserve").unwrap()
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
    fn test_apply_overlay_covers_package_keeps_preserve_leaves_undeclared() {
        use crate::system::spec::{SysDefine, SysModelSpec};

        let tmp = tempfile::TempDir::new().unwrap();
        // 包侧：用库生成一个合法的 sys/ 目录，再补上 sys-prj.yml 与内容文件
        let spec = SysModelSpec::make_new(SysDefine::new_without_model("web-stack")).unwrap();
        spec.save_local_minimal(&tmp.path().join("pkg"), "sys")
            .unwrap();
        let sys_src = tmp.path().join("pkg");
        std::fs::write(
            sys_src.join("sys-prj.yml"),
            "test_envs:\n  dep_root: ''\n  deps: []\npreserve:\n  - configs\n",
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

        let report = apply_overlay("web-stack", &sys_src, &target).unwrap();

        // 包内覆盖
        assert_eq!(
            std::fs::read_to_string(target.join("app.txt")).unwrap(),
            "new"
        );
        assert!(target.join("sys/sys_model.yml").exists());
        assert!(report.added >= 1);
        assert_eq!(report.replaced, 1);
        // preserve 命中：包里的 configs/default.toml 不覆盖现场值
        assert_eq!(report.skipped_preserve, 1);
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

        let err = apply_overlay("web-stack", &sys_src, &target).unwrap_err();
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
    fn test_load_preserve_errors_on_bad_yaml() {
        let tmp = tempfile::TempDir::new().unwrap();
        let f = tmp.path().join("sys-prj.yml");
        std::fs::write(&f, "preserve: [oops\n").unwrap();
        assert!(load_preserve(&f).is_err(), "坏 YAML 不能静默当空声明");
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
        // 包内有 sys/；目标里另有 configs/（preserve 声明）与 junk/
        std::fs::create_dir_all(sys_src.join("sys")).unwrap();
        std::fs::write(sys_src.join("sys/sys_model.yml"), "x").unwrap();
        std::fs::create_dir_all(target.join("sys")).unwrap();
        std::fs::create_dir_all(target.join("configs/gateway/state")).unwrap();
        std::fs::create_dir_all(target.join("junk")).unwrap();
        std::os::unix::fs::symlink("../values/x", target.join("values")).unwrap();

        let raw = vec!["configs/gateway/state/*.pem".to_string()];
        let pats = compile_path_patterns(&raw, "preserve").unwrap();
        let out = undeclared_top_entries(&target, &sys_src, &pats, &raw).unwrap();
        // configs 被 preserve 声明覆盖，values 是客户值链接，sys 在包内 → 只剩 junk
        assert_eq!(out, vec!["junk".to_string()]);
    }

    /// 用库生成一个合法的 `<root>/sys/` 目录（`apply_overlay` 的取名护栏需要它）。
    fn make_pkg(root: &Path, define_name: &str) {
        use crate::system::spec::{SysDefine, SysModelSpec};
        let spec = SysModelSpec::make_new(SysDefine::new_without_model(define_name)).unwrap();
        spec.save_local_minimal(root, "sys").unwrap();
    }

    #[test]
    fn test_load_preserve_missing_is_empty_and_present_parses() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(
            load_preserve(&tmp.path().join("nope.yml"))
                .unwrap()
                .is_empty()
        );

        let f = tmp.path().join("sys-prj.yml");
        std::fs::write(
            &f,
            "test_envs:\n  dep_root: ''\n  deps: []\npreserve:\n  - .env\n  - configs\n",
        )
        .unwrap();
        assert_eq!(
            load_preserve(&f).unwrap(),
            vec![".env".to_string(), "configs".to_string()]
        );
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

        let report = apply_overlay("web-stack", &sys_src, &target).unwrap();
        assert_eq!(report.replaced, 1, "diff.txt 内容不同 → 覆盖");
        assert_eq!(report.unchanged, 1, "same.txt 内容相同 → 不动");
        assert!(report.added >= 2, "sys/ 下的文件是新增：{}", report.added);
        assert_eq!(report.skipped_preserve, 0);
        assert_eq!(
            std::fs::read_to_string(target.join("diff.txt")).unwrap(),
            "NEW"
        );
    }

    #[test]
    fn test_apply_overlay_preserve_is_union_of_package_and_target() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sys_src = tmp.path().join("pkg");
        make_pkg(&sys_src, "web-stack");
        std::fs::write(
            sys_src.join("sys-prj.yml"),
            "test_envs:\n  dep_root: ''\n  deps: []\npreserve:\n  - configs\n",
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
            "test_envs:\n  dep_root: ''\n  deps: []\npreserve:\n  - site_only\n",
        )
        .unwrap();
        std::fs::write(target.join("configs/pkg.toml"), "site-configs").unwrap();

        let report = apply_overlay("web-stack", &sys_src, &target).unwrap();
        assert_eq!(
            report.skipped_preserve, 2,
            "包侧 configs + 目标侧 site_only"
        );
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

        apply_overlay("web-stack", &sys_src, &target).unwrap();
        let meta = std::fs::symlink_metadata(target.join("link")).unwrap();
        assert!(meta.file_type().is_symlink(), "包内符号链接应按链接复制");
        assert_eq!(
            std::fs::read_link(target.join("link")).unwrap(),
            PathBuf::from("sys/sys_model.yml")
        );
    }

    #[tokio::test]
    async fn test_update_sys_content_skips_unimported_without_fetch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("prj");
        std::fs::create_dir_all(root.join("_gal")).unwrap();
        std::fs::write(root.join("_gal/work.gxl"), "mod envs {}\nmod main {}\n").unwrap();
        // addr 指向不存在的本地包：因为系统未导入，应在取包前就跳过
        std::fs::write(
            root.join("ops-prj.yml"),
            "name: cust\nwork_envs:\n  dep_root: ''\n  deps: []\nsys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    path: /nonexistent/web-stack-0.1.0.tar.gz\n",
        )
        .unwrap();

        let prj = OpsProject::load(&root).unwrap();
        let accessor = crate::accessor::accessor_for_test();
        let opts = DownloadOptions::from((0usize, ValueDict::default()));
        let reports = prj.update_sys_content(accessor, &opts).await.unwrap();
        assert!(reports.is_empty(), "未导入 → 跳过，且不应去取包");
    }
}
