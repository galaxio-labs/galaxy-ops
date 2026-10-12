use super::prelude::*;
use super::project::OpsProject;
use crate::const_vars::SYS_PRJ_CONF_FILE_V2;
use crate::system::SysConf;
use crate::system::pack::normalize_path_pattern;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

/// ANSI 样式码。只在 [`render_text`](DiagnoseReport::render_text) 的 `color=true` 时使用；
/// 与 agentd 侧 `diagnose` 同一套码，两边观感一致。
const BOLD: &str = "1";
const RED: &str = "31";
const GREEN: &str = "32";
const YELLOW: &str = "33";
const CYAN: &str = "36";
const RESET: &str = "\x1b[0m";

/// 检查结果级别。**不用日志等级词**（INFO/WARN/ERROR 是日志的，不是诊断的）：
/// 与 agentd `diagnose` 对齐为 OK / WARN / FAIL。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

impl CheckStatus {
    /// 展示用标签。
    pub fn tag(self) -> &'static str {
        match self {
            CheckStatus::Ok => "[OK]",
            CheckStatus::Warn => "[WARN]",
            CheckStatus::Fail => "[FAIL]",
        }
    }

    /// 机器可读名（JSON/脚本用）。
    pub fn as_str(self) -> &'static str {
        match self {
            CheckStatus::Ok => "ok",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
        }
    }

    fn ansi(self) -> &'static str {
        match self {
            CheckStatus::Ok => GREEN,
            CheckStatus::Warn => YELLOW,
            CheckStatus::Fail => RED,
        }
    }
}

/// 单条检查结果。`code` 稳定（供测试/脚本按项取用）；`hint` 仅在想给处置建议时给。
#[derive(Clone, Debug)]
pub struct CheckItem {
    status: CheckStatus,
    code: &'static str,
    title: String,
    detail: Option<String>,
    hint: Option<String>,
}

impl CheckItem {
    pub fn status(&self) -> CheckStatus {
        self.status
    }
    pub fn code(&self) -> &'static str {
        self.code
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }
}

/// `gops prj diagnose` 的检查报告。
#[derive(Clone, Debug, Default)]
pub struct DiagnoseReport {
    items: Vec<CheckItem>,
}

impl DiagnoseReport {
    fn push(
        &mut self,
        status: CheckStatus,
        code: &'static str,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) {
        let detail = detail.into();
        self.items.push(CheckItem {
            status,
            code,
            title: title.into(),
            detail: if detail.is_empty() {
                None
            } else {
                Some(detail)
            },
            hint: None,
        });
    }

    pub fn ok(&mut self, code: &'static str, title: impl Into<String>, detail: impl Into<String>) {
        self.push(CheckStatus::Ok, code, title, detail);
    }
    pub fn warn(
        &mut self,
        code: &'static str,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) {
        self.push(CheckStatus::Warn, code, title, detail);
    }
    pub fn fail(
        &mut self,
        code: &'static str,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) {
        self.push(CheckStatus::Fail, code, title, detail);
    }

    /// 给**最后一条**检查补上处置提示（无提示则不动）。
    pub fn hint_last(&mut self, hint: impl Into<String>) {
        if let Some(item) = self.items.last_mut() {
            item.hint = Some(hint.into());
        }
    }

    pub fn items(&self) -> &[CheckItem] {
        &self.items
    }
    pub fn count(&self, status: CheckStatus) -> usize {
        self.items.iter().filter(|i| i.status == status).count()
    }
    pub fn failures(&self) -> usize {
        self.count(CheckStatus::Fail)
    }
    pub fn warnings(&self) -> usize {
        self.count(CheckStatus::Warn)
    }
    /// 无 FAIL 即视为通过；WARN 是否折算成 FAIL 由调用方（strict）在构造时决定。
    pub fn is_ok(&self) -> bool {
        self.failures() == 0
    }
    /// `0` = 通过；`1` = 有 FAIL（便于 `gops prj diagnose || 处理`，与 agentd 同语义）。
    pub fn exit_code(&self) -> i32 {
        i32::from(self.failures() > 0)
    }

    fn counts(&self) -> (usize, usize, usize) {
        (
            self.count(CheckStatus::Ok),
            self.warnings(),
            self.failures(),
        )
    }

    /// 总体状态：有 FAIL 即 FAIL，否则有 WARN 即 WARN，再否则 OK。
    fn overall(&self) -> CheckStatus {
        if self.failures() > 0 {
            CheckStatus::Fail
        } else if self.warnings() > 0 {
            CheckStatus::Warn
        } else {
            CheckStatus::Ok
        }
    }

    /// 一句话总判定：先报计数，再点出第一个 FAIL（优先处理项）。
    pub fn verdict(&self) -> String {
        let (ok, warn, fail) = self.counts();
        if fail == 0 && warn == 0 {
            return format!("全部正常（{ok} 项）");
        }
        let mut verdict = format!("{ok} 项正常 / {warn} 项警告 / {fail} 项失败");
        if let Some(first) = self.items.iter().find(|i| i.status == CheckStatus::Fail) {
            let _ = write!(verdict, "；首要问题：{}", first.title);
        }
        verdict
    }

    /// 人读文本。`color = false`（重定向 / `NO_COLOR`）时输出**纯文本**，逐字节可断言。
    ///
    /// 版式与 agentd `diagnose` 一致：`[OK]/[WARN]/[FAIL]` 加粗着色 + 标题加粗，
    /// detail 缩进对齐，`→` 行给处置建议（青色），末尾 `结论:`。
    pub fn render_text(&self, color: bool) -> String {
        let paint = |code: &str, text: &str| -> String {
            if color {
                format!("\x1b[{code}m{text}{RESET}")
            } else {
                text.to_string()
            }
        };

        let mut out = String::new();
        let _ = writeln!(out, "{}", paint(BOLD, "prj diagnose"));
        for item in &self.items {
            let tag = paint(&format!("1;{}", item.status.ansi()), item.status.tag());
            let title = paint(BOLD, &item.title);
            let _ = writeln!(out, "{tag} {title}");
            if let Some(detail) = &item.detail {
                for line in detail.lines() {
                    let _ = writeln!(out, "       {line}");
                }
            }
            if let Some(hint) = &item.hint {
                // 提示可能多行：每行都缩进对齐，只有第一行带箭头。
                for (index, line) in hint.lines().enumerate() {
                    let marker = if index == 0 { "→ " } else { "  " };
                    let _ = writeln!(out, "       {}", paint(CYAN, &format!("{marker}{line}")));
                }
            }
        }
        let verdict = paint(&format!("1;{}", self.overall().ansi()), &self.verdict());
        let _ = writeln!(out, "\n{} {verdict}", paint(BOLD, "结论:"));
        out
    }
}

/// diagnose 选项。
#[derive(Clone, Copy, Debug, Default)]
pub struct DiagnoseRequest {
    strict: bool,
}

impl DiagnoseRequest {
    pub fn new(strict: bool) -> Self {
        Self { strict }
    }
    pub fn strict(&self) -> bool {
        self.strict
    }
}

/// 检查运维项目的现场态声明与客户值纳管情况。
///
/// 两件事：
/// 1. 客户值（`values/`）是否已被版本控制纳管（存在、每系统有值目录、未被 `.gitignore`、无未提交改动）；
/// 2. 现场态声明是否自洽：`sys-prj.yml` 的 `ignore ⊆ protect`、`backup` 是否声明、
///    `ops-prj.yml` 的 `sys_models` 是否重名。
pub fn project_diagnose(root: &Path, req: &DiagnoseRequest) -> MainResult<DiagnoseReport> {
    let mut report = DiagnoseReport::default();

    let project = OpsProject::load(root)?;
    let values_dir = project.paths().value_dir();

    // 去重后的系统名。`ops-prj.yml` 可能对同一系统写多条（重名单独报错），
    // 其余检查按**系统名各做一次**，不然重复条目会把同一条提示刷两遍。
    let mut names: Vec<&String> = Vec::new();
    for sys in project.conf().sys_models() {
        if !names.contains(&sys.sys().name()) {
            names.push(sys.sys().name());
        }
    }

    // 1) values/ 目录是否存在
    if values_dir.is_dir() {
        report.ok(
            "values.present",
            "客户值目录已纳管",
            values_dir.display().to_string(),
        );
    } else {
        report.warn(
            "values.missing",
            "客户值目录缺失",
            values_dir.display().to_string(),
        );
        report.hint_last("尚未 localize，或客户差异未落地");
    }

    // 2) 每个已导入系统是否维护了自己的值目录
    for sys_name in &names {
        let sys_values = values_dir.join(sys_name);
        if sys_values.is_dir() {
            report.ok(
                "values.sys.present",
                format!("系统 `{sys_name}` 的客户值目录存在"),
                sys_values.display().to_string(),
            );
        } else {
            report.warn(
                "values.sys.missing",
                format!("系统 `{sys_name}` 已导入，但缺客户值目录"),
                sys_values.display().to_string(),
            );
            report.hint_last("无覆盖时沿用系统默认值");
        }
    }

    // 3) sys_models 重名：sync 会对同一目录重复导入 / 覆盖，backup 会把同一系统收两遍。
    //    报错时**列出每条各自的 addr**，好判断该删哪条。
    let dup: Vec<String> = {
        let mut dup = Vec::new();
        for n in &names {
            let matching: Vec<String> = project
                .conf()
                .sys_models()
                .iter()
                .filter(|s| s.sys().name() == *n)
                .map(|s| s.resolved_addr().unwrap_or_else(|_| s.addr_template()))
                .collect();
            if matching.len() > 1 {
                dup.push(format!("`{n}` → {}", matching.join("\n         ")));
            }
        }
        dup
    };
    if !dup.is_empty() {
        report.fail(
            "sys_models.duplicate",
            "ops-prj.yml 的 sys_models 出现重名",
            dup.join("\n       "),
        );
        report.hint_last(
            "duplicate sys_models make the same system dir be synced and backed up twice; \
             remove the stale entry",
        );
    }

    // 4) 逐系统的现场态声明自洽（仅在系统目录已导入时）
    for sys_name in &names {
        let conf_file = root.join(sys_name).join(SYS_PRJ_CONF_FILE_V2);
        if !conf_file.exists() {
            continue;
        }
        let conf = SysConf::load_conf(&conf_file).source_resource()?;
        check_sys_conf(sys_name, root, &conf, &mut report);
    }

    // 5) git 提交检查（仅在 git work tree 内）
    if !is_git_work_tree(root) {
        report.ok("git.none", "非 git 仓库，跳过 values/ 提交检查", "");
        report.hint_last("建议把运维项目纳入版本控制");
        return Ok(report);
    }

    if git_ignores(root, &values_dir) {
        report.fail(
            "git.values.ignored",
            format!("{} 被 git 忽略，客户值不会入库", values_dir.display()),
            "",
        );
        report.hint_last("请调整 .gitignore");
    } else if values_dir.is_dir() {
        let dirty = git_dirty_under(root, &values_dir);
        if dirty.is_empty() {
            report.ok("git.values.clean", "values/ 已入库且无未提交改动", "");
        } else {
            let title = format!("values/ 有 {} 项未提交改动", dirty.len());
            if req.strict() {
                report.fail("git.values.dirty", title, dirty[0].clone());
            } else {
                report.warn("git.values.dirty", title, dirty[0].clone());
            }
            report.hint_last("提交后在 CI 可用 `--strict` 卡口");
        }
    }
    Ok(report)
}

/// 单个系统 `sys-prj.yml` 的自洽检查。
fn check_sys_conf(sys_name: &str, root: &Path, conf: &SysConf, report: &mut DiagnoseReport) {
    // 保护集 = protect ∪ contract（contract 语义自足：声明即自动纳入保护）。
    // ignore ⊆ protect：不进包的按定义就不是包内容，升级必须不动。
    // 判定放宽到前缀：`ignore: configs/foo` 被 `protect: configs` 覆盖也算合。
    let protect: Vec<String> = conf
        .protection_paths()
        .iter()
        .filter_map(|p| normalize_path_pattern(p))
        .collect();
    let uncovered: Vec<String> = conf
        .ignore()
        .iter()
        .filter_map(|p| normalize_path_pattern(p))
        .filter(|ign| {
            !protect
                .iter()
                .any(|p| p == ign || ign.starts_with(&format!("{p}/")))
        })
        .collect();
    if !uncovered.is_empty() {
        report.warn(
            "protect.covers_ignore",
            format!("系统 `{sys_name}`：ignore 未全部被 protect 覆盖"),
            uncovered.join("、"),
        );
        report.hint_last("不进包的东西升级时也不应被覆盖，建议补进 protect");
    }

    // 声明了保护集（有现场态）却没声明备份选取：丢了要重装/换身份，却无法从备份还原。
    // 没声明保护集的系统没有现场态要保护，这里不报（避免噪音）。
    // 备份选取 = backup ∪ contract（contract **必收**），所以有 contract 就不会误报。
    let backup_effective = conf.backup_paths();
    if !protect.is_empty() && backup_effective.is_empty() {
        report.warn(
            "backup.missing",
            format!("系统 `{sys_name}`：声明了 protect 但未声明 backup"),
            "",
        );
        report.hint_last("现场目录丢失后，身份材料/状态库无法从备份还原");
    } else if !backup_effective.is_empty() {
        // 保护集里“不在备份选取里”的路径：**不是问题**（那些本来就该重建），只作提示。
        let all: Vec<String> = backup_effective
            .iter()
            .filter_map(|p| normalize_path_pattern(p))
            .collect();
        let not_covered: Vec<String> = protect
            .iter()
            .filter(|p| p.as_str() != "values")
            .filter(|p| !all.iter().any(|b| covered_by(p, b)))
            .cloned()
            .collect();
        if !not_covered.is_empty() {
            report.ok(
                "backup.protect.uncovered",
                format!("系统 `{sys_name}`：以下 protect 未纳入备份"),
                not_covered.join("、"),
            );
            report.hint_last("丢失后只能重建；若不可接受，请加入 backup");
        }
    }

    check_contract(sys_name, root, conf, report);
}

/// 对外契约（`contract:`）的**自身**检查。不涉及与 `ignore`/`protect`/`backup` 的优先级——
/// 契约语义自足，只查：① 声明了但路径不在场（齐备）；② 与其它声明交叠（声明错误）。
fn check_contract(sys_name: &str, root: &Path, conf: &SysConf, report: &mut DiagnoseReport) {
    let contract: Vec<String> = conf
        .contract()
        .iter()
        .filter_map(|p| normalize_path_pattern(p))
        .collect();
    if contract.is_empty() {
        return;
    }

    // ① 齐备：契约路径的**最长字面前缀**应在场（契约条目可以是 glob）。
    let sys_dir = root.join(sys_name);
    let mut missing: Vec<String> = contract
        .iter()
        .filter(|p| {
            let prefix = literal_prefix(p);
            !prefix.is_empty() && !sys_dir.join(&prefix).exists()
        })
        .cloned()
        .collect();
    missing.sort();
    missing.dedup();
    if !missing.is_empty() {
        report.warn(
            "contract.missing",
            format!("系统 `{sys_name}`：契约路径不在场"),
            missing.join("、"),
        );
        report.hint_last(
            "契约是不可重建的现场事实，缺失可能意味着丢过；用 `prj contract import` 恢复",
        );
    }

    // ② 不交叠：一个路径（含父/子目录）只应写在一处。交叠当**声明错误**，不用优先级解决。
    let mut others: Vec<(&str, String)> = Vec::new();
    for p in conf.ignore() {
        others.push(("ignore", p.clone()));
    }
    for p in conf.protect() {
        others.push(("protect", p.clone()));
    }
    for p in conf.backup() {
        others.push(("backup", p.clone()));
    }
    let mut overlaps: Vec<String> = Vec::new();
    for c in &contract {
        for (name, other) in &others {
            let Some(on) = normalize_path_pattern(other) else {
                continue;
            };
            if covered_by(c, &on) {
                overlaps.push(format!("`{c}`（contract）↔ `{other}`（{name}）"));
            }
        }
    }
    if !overlaps.is_empty() {
        overlaps.sort();
        overlaps.dedup();
        report.warn(
            "contract.overlap",
            format!("系统 `{sys_name}`：contract 与其它声明交叠"),
            overlaps.join("\n       "),
        );
        report.hint_last("每个路径只写一处；contract 语义自足，不必再写进 ignore/protect/backup");
    }
}

/// 取 glob 模式的最长**字面前缀**（到第一个含 `*`/`?`/`[` 的路径分量为止）。
/// 例：`configs/*.pem` → `configs`；`configs/gateway/public` → `configs/gateway/public`。
fn literal_prefix(pattern: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in pattern.split('/') {
        if seg.chars().any(|c| matches!(c, '*' | '?' | '[')) {
            break;
        }
        parts.push(seg);
    }
    parts.join("/")
}

/// `protect` 模式 `p` 是否被备份模式 `b` 覆盖（任一方向的前缀/自身包含即可）。
/// 只做**路径前缀**判断，不做 glob 展开 —— 目的是提示，不是精确证明。
fn covered_by(p: &str, b: &str) -> bool {
    p == b || b.starts_with(&format!("{p}/")) || p.starts_with(&format!("{b}/"))
}

fn git_cmd(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root);
    cmd
}

fn is_git_work_tree(root: &Path) -> bool {
    match git_cmd(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim() == "true",
        _ => false,
    }
}

fn git_ignores(root: &Path, path: &Path) -> bool {
    // `git check-ignore -q <path>`：被忽略时退出码为 0。
    // 末尾带 '/' 才会匹配 `.gitignore` 中 `values/` 这类"仅目录"规则；
    // 否则当 values/ 尚未创建时 git 会当成文件，漏报。
    let mut arg = path.to_string_lossy().to_string();
    if !arg.ends_with('/') {
        arg.push('/');
    }
    git_cmd(root)
        .arg("check-ignore")
        .arg("-q")
        .arg(arg)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn git_dirty_under(root: &Path, path: &Path) -> Vec<String> {
    let rel = path.to_string_lossy().to_string();
    let out = match git_cmd(root)
        .args(["status", "--porcelain", "--", &rel])
        .output()
    {
        Ok(out) if out.status.success() => out,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_error::dev::testing::TestAssert;
    use orion_variate::tools::test_init;
    use tempfile::TempDir;

    const ONE_SYS: &str = "sys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    url: http://example.com/web-stack.tar.gz\n";

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn init_project(root: &Path, sys_models_yaml: &str) {
        std::fs::create_dir_all(root.join("_gal")).unwrap();
        std::fs::write(
            root.join("_gal").join("work.gxl"),
            "mod envs {}\nmod main {}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("ops-prj.yml"),
            format!("name: cust\nwork_envs:\n  dep_root: ''\n  deps: []\n{sys_models_yaml}"),
        )
        .unwrap();
    }

    fn sys_prj(root: &Path, sys: &str, body: &str) {
        let dir = root.join(sys);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sys-prj.yml"), body).unwrap();
    }

    fn codes(report: &DiagnoseReport) -> Vec<&'static str> {
        report.items().iter().map(|i| i.code()).collect()
    }

    fn count_code(report: &DiagnoseReport, code: &str) -> usize {
        report.items().iter().filter(|i| i.code() == code).count()
    }

    fn item<'a>(report: &'a DiagnoseReport, code: &str) -> &'a CheckItem {
        report
            .items()
            .iter()
            .find(|i| i.code() == code)
            .unwrap_or_else(|| panic!("no item {code}; codes={:?}", codes(report)))
    }

    #[test]
    fn test_diagnose_reports_contract_missing() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        // 声明了 contract，但路径不在场
        sys_prj(
            root,
            "web-stack",
            "test_envs:\n  dep_root: ''\n  deps: []\ncontract:\n  - configs/gateway/public\n",
        );

        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        assert!(codes(&report).contains(&"contract.missing"));
    }

    #[test]
    fn test_diagnose_reports_contract_overlap_as_declaration_error() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        // contract 与 protect 交叠：一个路径只应写在一处
        sys_prj(
            root,
            "web-stack",
            "test_envs:\n  dep_root: ''\n  deps: []\nprotect:\n  - configs\ncontract:\n  - configs/gateway/public\n",
        );

        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        assert!(codes(&report).contains(&"contract.overlap"));
    }

    #[test]
    fn test_diagnose_contract_present_omits_missing() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        sys_prj(
            root,
            "web-stack",
            "test_envs:\n  dep_root: ''\n  deps: []\ncontract:\n  - configs/gateway/public\n",
        );
        std::fs::create_dir_all(root.join("web-stack/configs/gateway/public")).unwrap();

        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        assert!(!codes(&report).contains(&"contract.missing"));
    }

    #[test]
    fn test_diagnose_contract_glob_prefix_is_not_reported_missing() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        // glob 契约：应检查最长字面前缀 `configs`，而不是字面 glob
        sys_prj(
            root,
            "web-stack",
            "test_envs:\n  dep_root: ''\n  deps: []\ncontract:\n  - configs/*.pem\n",
        );
        std::fs::create_dir_all(root.join("web-stack/configs")).unwrap();

        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        assert!(!codes(&report).contains(&"contract.missing"));
    }

    #[test]
    fn test_diagnose_contract_overlap_with_backup() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        sys_prj(
            root,
            "web-stack",
            "test_envs:\n  dep_root: ''\n  deps: []\ncontract:\n  - configs/gateway/public\nbackup:\n  - configs/gateway/public\n",
        );

        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        assert!(codes(&report).contains(&"contract.overlap"));
    }

    #[test]
    fn test_diagnose_contract_unrelated_declarations_do_not_overlap() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        sys_prj(
            root,
            "web-stack",
            "test_envs:\n  dep_root: ''\n  deps: []\nprotect:\n  - .env\ncontract:\n  - configs/gateway/public\n",
        );

        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        assert!(!codes(&report).contains(&"contract.overlap"));
    }

    #[test]
    fn test_diagnose_no_contract_declaration_skips_contract_checks() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        sys_prj(
            root,
            "web-stack",
            "test_envs:\n  dep_root: ''\n  deps: []\n",
        );

        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        assert!(
            !codes(&report).iter().any(|c| c.starts_with("contract.")),
            "codes: {:?}",
            codes(&report)
        );
    }

    #[test]
    fn test_status_tag_and_str() {
        assert_eq!(CheckStatus::Ok.tag(), "[OK]");
        assert_eq!(CheckStatus::Warn.tag(), "[WARN]");
        assert_eq!(CheckStatus::Fail.tag(), "[FAIL]");
        assert_eq!(CheckStatus::Ok.as_str(), "ok");
        assert_eq!(CheckStatus::Fail.as_str(), "fail");
    }

    #[test]
    fn test_report_counts_and_exit_code() {
        let mut report = DiagnoseReport::default();
        report.ok("a", "x", "");
        report.warn("b", "y", "");
        report.ok("c", "z", "");
        assert_eq!(report.count(CheckStatus::Ok), 2);
        assert_eq!(report.warnings(), 1);
        assert_eq!(report.failures(), 0);
        assert_eq!(report.exit_code(), 0);
        assert!(report.is_ok());

        report.fail("d", "w", "");
        assert_eq!(report.failures(), 1);
        assert_eq!(report.exit_code(), 1);
        assert!(!report.is_ok());
    }

    #[test]
    fn test_verdict_mentions_first_failure() {
        let mut report = DiagnoseReport::default();
        report.ok("a", "正常项", "");
        report.warn("b", "警告项", "");
        report.fail("c", "首要失败", "");
        let v = report.verdict();
        assert!(v.contains("1 项正常 / 1 项警告 / 1 项失败"), "{v}");
        assert!(v.contains("首要问题：首要失败"), "{v}");

        let mut clean = DiagnoseReport::default();
        clean.ok("a", "x", "");
        assert_eq!(clean.verdict(), "全部正常（1 项）");
    }

    #[test]
    fn test_render_text_is_plain_without_color_and_has_layout() {
        let mut report = DiagnoseReport::default();
        report.ok("a", "配置文件可读", "/etc/x.yml");
        report.warn("b", "缺客户值目录", "/p/values");
        report.hint_last("先 localize");
        report.fail("c", "凭据被拒", "401");

        let text = report.render_text(false);
        assert!(!text.contains("\x1b["), "无颜色时不应有 ANSI：{text:?}");
        assert!(text.contains("prj diagnose"), "{text}");
        // 日志等级词不应出现
        assert!(
            !text.contains("[INFO]") && !text.contains("[ERROR]"),
            "{text}"
        );
        // 标签 + 缩进 detail + → 提示
        assert!(text.contains("[OK] 配置文件可读"), "{text}");
        assert!(text.contains("       /etc/x.yml"), "{text}");
        assert!(text.contains("       → 先 localize"), "{text}");
        assert!(text.contains("[FAIL] 凭据被拒"), "{text}");
        // 结论在末尾，且点出首要问题
        assert!(
            text.contains("结论: 1 项正常 / 1 项警告 / 1 项失败；首要问题：凭据被拒"),
            "{text}"
        );
    }

    #[test]
    fn test_render_text_colors_statuses_only_when_asked() {
        let mut report = DiagnoseReport::default();
        report.ok("a", "ok 项", "");
        report.warn("b", "warn 项", "");
        report.fail("c", "fail 项", "");

        let plain = report.render_text(false);
        assert!(!plain.contains("\x1b["), "{plain:?}");

        let colored = report.render_text(true);
        assert!(colored.contains("\x1b[1;32m[OK]\x1b[0m"), "{colored:?}");
        assert!(colored.contains("\x1b[1;33m[WARN]\x1b[0m"), "{colored:?}");
        assert!(colored.contains("\x1b[1;31m[FAIL]\x1b[0m"), "{colored:?}");
    }

    #[test]
    fn test_diagnose_reports_missing_values() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);

        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        assert!(report.is_ok());
        let codes = codes(&report);
        assert!(codes.contains(&"values.missing"), "codes: {codes:?}");
        assert!(codes.contains(&"values.sys.missing"), "codes: {codes:?}");
        assert_eq!(item(&report, "values.missing").status(), CheckStatus::Warn);
        assert!(item(&report, "values.missing").hint().is_some());
    }

    #[test]
    fn test_diagnose_ok_when_values_present() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        std::fs::create_dir_all(root.join("values").join("web-stack")).unwrap();

        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        assert_eq!(report.failures(), 0, "unexpected: {:?}", codes(&report));
        let codes = codes(&report);
        assert!(codes.contains(&"values.present"));
        assert!(codes.contains(&"values.sys.present"));
    }

    #[test]
    fn test_diagnose_flags_duplicate_sys_models() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let two = "sys_models:\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    url: http://example.com/a.tar.gz\n- sys:\n    name: web-stack\n    kind: docker-compose\n    vender: ''\n  addr:\n    url: http://example.com/b.tar.gz\n";
        init_project(root, two);

        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        assert!(!report.is_ok());
        let it = item(&report, "sys_models.duplicate");
        assert_eq!(it.status(), CheckStatus::Fail);
        // 报错里要指明每条各自的 addr，好判断该删哪条
        let detail = it.detail().unwrap_or_default();
        assert!(
            detail.contains("a.tar.gz") && detail.contains("b.tar.gz"),
            "detail={detail}"
        );
        // 同一系统只检查一次：去重后 values.sys.missing 只出现一条
        assert_eq!(count_code(&report, "values.sys.missing"), 1, "应去重");
    }

    #[test]
    fn test_diagnose_checks_protect_covers_ignore() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        // ignore 里有 configs 但 protect 没有 → 警告
        sys_prj(
            root,
            "web-stack",
            "test_envs:\n  dep_root: ''\n  deps: []\nignore:\n  - configs\nprotect:\n  - .env\n",
        );

        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        let codes = codes(&report);
        assert!(codes.contains(&"protect.covers_ignore"), "codes: {codes:?}");
        assert!(codes.contains(&"backup.missing"), "codes: {codes:?}");
        // 都只是警告，不算失败
        assert!(report.is_ok());
    }

    #[test]
    fn test_diagnose_protect_prefix_covers_deeper_ignore() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        // protect 覆盖了更深的 ignore 项 → 不应报 covers_ignore
        sys_prj(
            root,
            "web-stack",
            "test_envs:\n  dep_root: ''\n  deps: []\nignore:\n  - configs/foo\nprotect:\n  - configs\nbackup:\n  - configs/*.pem\n",
        );

        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        assert!(
            !codes(&report).contains(&"protect.covers_ignore"),
            "codes: {:?}",
            codes(&report)
        );
    }

    #[test]
    fn test_diagnose_quiet_when_protect_covers_ignore_and_backup_declared() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        sys_prj(
            root,
            "web-stack",
            "test_envs:\n  dep_root: ''\n  deps: []\nignore:\n  - configs\nprotect:\n  - configs\nbackup:\n  - configs/state/*.pem\n",
        );

        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        let codes = codes(&report);
        assert!(
            !codes.contains(&"protect.covers_ignore"),
            "codes: {codes:?}"
        );
        assert!(!codes.contains(&"backup.missing"), "codes: {codes:?}");
    }

    #[test]
    fn test_diagnose_reports_protect_not_covered_by_backup() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        sys_prj(
            root,
            "web-stack",
            "test_envs:\n  dep_root: ''\n  deps: []\nprotect:\n  - configs\n  - runtime\nbackup:\n  - configs/state/*.pem\n",
        );

        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        let it = item(&report, "backup.protect.uncovered");
        // 只是提示（不是失败）：runtime 丢了大不了重建
        assert_eq!(it.status(), CheckStatus::Ok);
        assert!(
            it.detail().unwrap_or_default().contains("runtime"),
            "detail={:?}",
            it.detail()
        );
        assert!(it.hint().is_some());
        // 声明了 backup → 不该再报 missing
        assert!(!codes(&report).contains(&"backup.missing"));
        assert!(report.is_ok());
    }

    #[test]
    fn test_diagnose_notes_non_git_project() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        // 临时目录恰好落在某个 git 仓内时跳过（环境例外，不是被测行为）
        if is_git_work_tree(root) {
            return;
        }
        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        assert_eq!(item(&report, "git.none").status(), CheckStatus::Ok);
    }

    #[test]
    fn test_covered_by_prefix() {
        assert!(covered_by("configs", "configs/gateway/state/*.pem"));
        assert!(covered_by(
            "configs/gateway/state/wist-gateway-store.db",
            "configs"
        ));
        assert!(covered_by("packages", "packages"));
        assert!(!covered_by("configs", "data-plane-run"));
    }

    #[test]
    fn test_diagnose_errors_when_values_ignored_by_git() {
        test_init();
        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        std::fs::create_dir_all(root.join("values").join("web-stack")).unwrap();
        std::fs::write(root.join(".gitignore"), "values/\n").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["init", "-q"])
            .output()
            .unwrap();

        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        assert!(!report.is_ok());
        assert!(
            codes(&report).contains(&"git.values.ignored"),
            "codes: {:?}",
            codes(&report)
        );
    }

    #[test]
    fn test_diagnose_dirty_values_warns_then_strict_fails() {
        test_init();
        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        std::fs::create_dir_all(root.join("values").join("web-stack")).unwrap();
        std::fs::write(
            root.join("values").join("web-stack").join("sys_value.yml"),
            "# x\n",
        )
        .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["init", "-q"])
            .output()
            .unwrap();

        let report = project_diagnose(root, &DiagnoseRequest::new(false)).assert();
        assert!(report.is_ok());
        assert_eq!(
            item(&report, "git.values.dirty").status(),
            CheckStatus::Warn
        );

        let strict = project_diagnose(root, &DiagnoseRequest::new(true)).assert();
        assert!(!strict.is_ok());
        assert_eq!(
            item(&strict, "git.values.dirty").status(),
            CheckStatus::Fail
        );
    }
}
