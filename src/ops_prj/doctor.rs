use super::prelude::*;
use super::project::OpsProject;
use std::path::Path;
use std::process::Command;

/// 检查严重级别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CheckLevel {
    Info,
    Warn,
    Error,
}

/// 单条检查结果。
#[derive(Clone, Debug)]
pub struct CheckItem {
    level: CheckLevel,
    code: &'static str,
    message: String,
}

impl CheckItem {
    pub fn level(&self) -> CheckLevel {
        self.level
    }
    pub fn code(&self) -> &'static str {
        self.code
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// `gops prj doctor` 的检查报告。
#[derive(Clone, Debug, Default)]
pub struct DoctorReport {
    items: Vec<CheckItem>,
}

impl DoctorReport {
    fn push(&mut self, level: CheckLevel, code: &'static str, message: impl Into<String>) {
        self.items.push(CheckItem {
            level,
            code,
            message: message.into(),
        });
    }
    pub fn info(&mut self, code: &'static str, message: impl Into<String>) {
        self.push(CheckLevel::Info, code, message);
    }
    pub fn warn(&mut self, code: &'static str, message: impl Into<String>) {
        self.push(CheckLevel::Warn, code, message);
    }
    pub fn error(&mut self, code: &'static str, message: impl Into<String>) {
        self.push(CheckLevel::Error, code, message);
    }
    pub fn items(&self) -> &[CheckItem] {
        &self.items
    }
    pub fn count(&self, level: CheckLevel) -> usize {
        self.items.iter().filter(|i| i.level == level).count()
    }
    pub fn errors(&self) -> usize {
        self.count(CheckLevel::Error)
    }
    pub fn warnings(&self) -> usize {
        self.count(CheckLevel::Warn)
    }
    /// 无 error 即视为通过；warn 是否折算成 error 由调用方（strict）决定。
    pub fn is_ok(&self) -> bool {
        self.errors() == 0
    }
}

/// doctor 选项。
#[derive(Clone, Copy, Debug, Default)]
pub struct DoctorRequest {
    strict: bool,
}

impl DoctorRequest {
    pub fn new(strict: bool) -> Self {
        Self { strict }
    }
    pub fn strict(&self) -> bool {
        self.strict
    }
}

/// 检查运维项目的客户值（`values/`）是否已被版本控制纳管。
///
/// 目标是问题 1 的对应解药：把"靠 git 纪律"变成"靠工具提醒"——
/// `values/` 是否存在、每个已导入系统是否有值目录、是否被 `.gitignore` 忽略、是否有未提交改动。
pub fn project_doctor(root: &Path, req: &DoctorRequest) -> MainResult<DoctorReport> {
    let mut report = DoctorReport::default();

    let project = OpsProject::load(root)?;
    let values_dir = project.paths().value_dir();

    // 1) values/ 目录是否存在
    if values_dir.is_dir() {
        report.info(
            "values.present",
            format!("客户值目录存在: {}", values_dir.display()),
        );
    } else {
        report.warn(
            "values.missing",
            format!(
                "客户值目录缺失: {}（尚未 localize，或客户差异未落地）",
                values_dir.display()
            ),
        );
    }

    // 2) 每个已导入系统是否维护了自己的值目录
    for ops_sys in project.conf().sys_models() {
        let sys_name = ops_sys.sys().name();
        let sys_values = values_dir.join(sys_name);
        if sys_values.is_dir() {
            report.info(
                "values.sys.present",
                format!("系统 `{sys_name}` 的客户值目录存在"),
            );
        } else {
            report.warn(
                "values.sys.missing",
                format!(
                    "系统 `{sys_name}` 已由 ops-prj.yml 导入，但缺少 {}（无覆盖时沿用系统默认值）",
                    sys_values.display()
                ),
            );
        }
    }

    // 3) git 提交检查（仅在 git work tree 内）
    if !is_git_work_tree(root) {
        report.info(
            "git.none",
            "非 git 仓库，跳过 values/ 提交检查；建议把运维项目纳入版本控制",
        );
        return Ok(report);
    }

    if git_ignores(root, &values_dir) {
        report.error(
            "git.values.ignored",
            format!(
                "{} 被 git 忽略，客户值不会入库；请调整 .gitignore",
                values_dir.display()
            ),
        );
    } else if values_dir.is_dir() {
        let dirty = git_dirty_under(root, &values_dir);
        if dirty.is_empty() {
            report.info("git.values.clean", "values/ 已入库且无未提交改动");
        } else {
            let msg = format!(
                "values/ 有 {} 项未提交改动（示例: {}）",
                dirty.len(),
                dirty[0]
            );
            if req.strict() {
                report.error("git.values.dirty", msg);
            } else {
                report.warn("git.values.dirty", msg);
            }
        }
    }
    Ok(report)
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

    fn codes(report: &DoctorReport) -> Vec<&'static str> {
        report.items().iter().map(|i| i.code()).collect()
    }

    #[test]
    fn test_report_counts_by_level() {
        let mut report = DoctorReport::default();
        report.info("a", "x");
        report.warn("b", "y");
        report.info("c", "z");
        assert_eq!(report.count(CheckLevel::Info), 2);
        assert_eq!(report.warnings(), 1);
        assert_eq!(report.errors(), 0);
        assert!(report.is_ok());

        report.error("d", "w");
        assert_eq!(report.errors(), 1);
        assert!(!report.is_ok());
    }

    #[test]
    fn test_doctor_reports_missing_values() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);

        let report = project_doctor(root, &DoctorRequest::new(false)).assert();
        // 缺失只是警告，不阻断
        assert!(report.is_ok());
        let codes = codes(&report);
        assert!(codes.contains(&"values.missing"), "codes: {codes:?}");
        assert!(codes.contains(&"values.sys.missing"), "codes: {codes:?}");
    }

    #[test]
    fn test_doctor_ok_when_values_present() {
        test_init();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        std::fs::create_dir_all(root.join("values").join("web-stack")).unwrap();

        let report = project_doctor(root, &DoctorRequest::new(false)).assert();
        assert_eq!(report.errors(), 0, "unexpected: {:?}", codes(&report));
        let codes = codes(&report);
        assert!(codes.contains(&"values.present"));
        assert!(codes.contains(&"values.sys.present"));
    }

    #[test]
    fn test_doctor_errors_when_values_ignored_by_git() {
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

        let report = project_doctor(root, &DoctorRequest::new(false)).assert();
        assert!(!report.is_ok());
        assert!(
            codes(&report).contains(&"git.values.ignored"),
            "codes: {:?}",
            codes(&report)
        );
    }

    #[test]
    fn test_doctor_errors_when_values_ignored_even_if_absent() {
        test_init();
        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_project(root, ONE_SYS);
        // values/ 尚未创建，但已被 .gitignore 忽略——仍应报错
        std::fs::write(root.join(".gitignore"), "values/\n").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["init", "-q"])
            .output()
            .unwrap();

        let report = project_doctor(root, &DoctorRequest::new(false)).assert();
        assert!(!report.is_ok());
        assert!(
            codes(&report).contains(&"git.values.ignored"),
            "codes: {:?}",
            codes(&report)
        );
    }

    #[test]
    fn test_doctor_dirty_values_warns_then_strict_errors() {
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

        // 未提交：默认只是警告
        let report = project_doctor(root, &DoctorRequest::new(false)).assert();
        assert!(report.is_ok());
        assert!(
            codes(&report).contains(&"git.values.dirty"),
            "codes: {:?}",
            codes(&report)
        );

        // strict：折算为错误
        let strict = project_doctor(root, &DoctorRequest::new(true)).assert();
        assert!(!strict.is_ok());
    }
}
