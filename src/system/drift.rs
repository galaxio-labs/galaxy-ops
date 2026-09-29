use super::prelude::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::const_vars::{ENV_FILE, SYS_VALUE_FILE, USER_VALUE_FILE, VALUE_DIR};
use crate::project::{load_value_file, render_env};
use crate::system::operator::SysOperator;

/// 漂移检查结论。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriftStatus {
    /// 尚无 `.env` 基线（从未 localize）。
    NoBaseline,
    /// 当前值与已生成的 `.env` 一致。
    Clean,
    /// 值已变更，但 `.env` 未随之更新（未重新 localize）。
    Drifted,
}

/// 单项变更类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    /// 当前值里有、`.env` 里没有（新增或未 localize）。
    Added,
    /// 两边都有但值不同。
    Changed,
    /// `.env` 里有、当前值里没有（已从值文件移除）。
    Removed,
}

/// 单项值变更。
#[derive(Clone, Debug)]
pub struct ValueChange {
    key: String,
    kind: ChangeKind,
    expected: Option<String>,
    actual: Option<String>,
}

impl ValueChange {
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn kind(&self) -> ChangeKind {
        self.kind
    }
    /// 当前合并值（`Added` / `Changed` 时有值）。
    pub fn expected(&self) -> Option<&str> {
        self.expected.as_deref()
    }
    /// 已生成的 `.env` 里的值（`Removed` / `Changed` 时有值）。
    pub fn actual(&self) -> Option<&str> {
        self.actual.as_deref()
    }

    /// 人类可读的变更描述。
    pub fn describe(&self) -> String {
        let short = |s: Option<&str>| s.unwrap_or("").chars().take(60).collect::<String>();
        match self.kind {
            ChangeKind::Added => format!("+ {} = {}", self.key, short(self.expected())),
            ChangeKind::Removed => format!("- {} (原值 {})", self.key, short(self.actual())),
            ChangeKind::Changed => format!(
                "{}: {} -> {}",
                self.key,
                short(self.actual()),
                short(self.expected())
            ),
        }
    }
}

/// 轻量漂移报告：只比对"当前合并值 vs 已生成的 `.env`"，不做完整 reconcile。
#[derive(Clone, Debug)]
pub struct DriftReport {
    status: DriftStatus,
    env_path: PathBuf,
    changes: Vec<ValueChange>,
    /// 变量定义（`sys/setting/vars.yml` 等）比 `sys/merged_vars.yml` 更新：
    /// 即"改了定义但未重新解析"，仅靠 `.env` 比对看不到。
    vars_stale: bool,
}

impl DriftReport {
    pub fn status(&self) -> DriftStatus {
        self.status
    }
    pub fn env_path(&self) -> &Path {
        &self.env_path
    }
    pub fn changes(&self) -> &[ValueChange] {
        &self.changes
    }
    pub fn is_drifted(&self) -> bool {
        self.status == DriftStatus::Drifted
    }
    /// 变量定义比已解析结果更新（需要重跑解析 / localize）。
    pub fn vars_stale(&self) -> bool {
        self.vars_stale
    }
}

/// 检测漂移：重新计算本地化会产出的值，与已落盘的 `.env` 比对。
///
/// 只读（不写任何输出文件）；但会像其它 `gops sys` 命令一样先 `load` 系统
/// （因此可能触发配置迁移、并确保 `values/` 目录存在）。
pub fn detect_drift(op: &SysOperator, sys_root: &Path) -> MainResult<DriftReport> {
    let env_path = sys_root.join(ENV_FILE);
    let expected = expected_env(op, sys_root)?;
    // 已解析但定义输入更新 → 变量陈旧（仅在 merged 存在时有意义）
    let vars_stale = op.has_resolved_vars() && op.vars_need_resolve()?;

    if !env_path.exists() {
        return Ok(DriftReport {
            status: DriftStatus::NoBaseline,
            env_path,
            changes: Vec::new(),
            vars_stale,
        });
    }

    let actual_text = std::fs::read_to_string(&env_path)
        .source_resource()
        .with(&env_path)?;
    let actual = parse_env(&actual_text);
    let changes = diff_env(&expected, &actual);
    let status = if changes.is_empty() {
        DriftStatus::Clean
    } else {
        DriftStatus::Drifted
    };
    Ok(DriftReport {
        status,
        env_path,
        changes,
        vars_stale,
    })
}

/// 复现 `sys localize` 的合并顺序：系统默认值 ⊕ `values/sys_value.yml` ⊕ `values/value.yml`。
fn expected_env(op: &SysOperator, sys_root: &Path) -> MainResult<BTreeMap<String, String>> {
    let value_root = resolve_value_root(sys_root);

    let mut dict = OriginDict::from(op.system_default_values()?);
    dict.set_source("sys-defaults");

    let sys_value = value_root.join(SYS_VALUE_FILE);
    if sys_value.exists() {
        let mut d = OriginDict::from(load_value_file(&sys_value)?);
        d.set_source("sys-setting");
        dict.merge(&d);
    }

    let user_value = value_root.join(USER_VALUE_FILE);
    if user_value.exists() {
        let mut d = OriginDict::from(load_value_file(&user_value)?);
        d.set_source("customer");
        dict.merge(&d);
    }

    Ok(parse_env(&render_env(&dict)))
}

/// 与 `sys_cmd::resolve_sys_value_path` 一致：优先用运维项目维护的值目录，否则 `<sys>/values`。
fn resolve_value_root(sys_root: &Path) -> PathBuf {
    crate::ops_prj::project::owner_project_value_dir(sys_root)
        .unwrap_or_else(|| sys_root.join(VALUE_DIR))
}

/// 解析 dotenv 文本为 `KEY -> VALUE`（跳过空行与注释；`VALUE` 原样保留）。
fn parse_env(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            map.insert(key.trim().to_string(), value.to_string());
        }
    }
    map
}

fn diff_env(
    expected: &BTreeMap<String, String>,
    actual: &BTreeMap<String, String>,
) -> Vec<ValueChange> {
    let mut changes = Vec::new();
    for (key, exp) in expected {
        match actual.get(key) {
            None => changes.push(ValueChange {
                key: key.clone(),
                kind: ChangeKind::Added,
                expected: Some(exp.clone()),
                actual: None,
            }),
            Some(act) if act != exp => changes.push(ValueChange {
                key: key.clone(),
                kind: ChangeKind::Changed,
                expected: Some(exp.clone()),
                actual: Some(act.clone()),
            }),
            Some(_) => {}
        }
    }
    for (key, act) in actual {
        if !expected.contains_key(key) {
            changes.push(ValueChange {
                key: key.clone(),
                kind: ChangeKind::Removed,
                expected: None,
                actual: Some(act.clone()),
            });
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::depend::DependencySet;
    use crate::system::spec::{SysDefine, SysModelSpec};
    use orion_error::dev::testing::TestAssert;
    use orion_vars::vars::VarDefinition;
    use tempfile::TempDir;

    /// 造一个最小 gxl 系统：`sys/merged_vars.yml`（系统默认值）+ 空的 SysModelSpec。
    fn setup(root: &Path) -> SysOperator {
        std::fs::create_dir_all(root.join("sys")).unwrap();
        let vars = VarCollection::define(vec![
            VarDefinition::from(("SERVICE_PORT", 8080u64)).with_mut_system(),
            VarDefinition::from(("REPLICAS", 1u64)).with_mut_system(),
        ]);
        orion_conf::ConfigIO::save_conf(&vars, &root.join("sys").join("merged_vars.yml")).assert();

        let spec = SysModelSpec::make_new(SysDefine::new_without_model("demo")).assert();
        SysOperator::new(spec, DependencySet::default(), root.to_path_buf())
    }

    fn describe(report: &DriftReport) -> Vec<String> {
        report.changes().iter().map(|c| c.describe()).collect()
    }

    fn set_mtime(path: &Path, t: std::time::SystemTime) {
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(t).unwrap();
    }

    #[test]
    fn test_vars_stale_when_definition_newer_than_merged() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let op = setup(root);
        std::fs::write(root.join(".env"), "SERVICE_PORT=8080\nREPLICAS=1\n").unwrap();

        // 变量定义比 merged 更新 → 陈旧（仅比对 .env 看不到）
        let vars = root.join("sys/setting/vars.yml");
        std::fs::create_dir_all(root.join("sys/setting")).unwrap();
        std::fs::write(&vars, "system: []\n").unwrap();
        let base =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        set_mtime(root.join("sys/merged_vars.yml").as_path(), base);
        set_mtime(&vars, base + std::time::Duration::from_secs(60));

        let report = detect_drift(&op, root).assert();
        assert!(report.vars_stale(), "definition newer → stale");
        // .env 本身没变，状态仍是 Clean（漂移只在定义层）
        assert_eq!(report.status(), DriftStatus::Clean);

        // 定义回退到不新于 merged → 不再陈旧
        set_mtime(&vars, base);
        let report = detect_drift(&op, root).assert();
        assert!(!report.vars_stale());
    }

    #[test]
    fn test_no_baseline_without_env() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let op = setup(root);

        let report = detect_drift(&op, root).assert();
        assert_eq!(report.status(), DriftStatus::NoBaseline);
        assert!(report.changes().is_empty());
    }

    #[test]
    fn test_clean_when_env_matches() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let op = setup(root);
        std::fs::write(root.join(".env"), "SERVICE_PORT=8080\nREPLICAS=1\n").unwrap();

        let report = detect_drift(&op, root).assert();
        assert_eq!(
            report.status(),
            DriftStatus::Clean,
            "{:?}",
            describe(&report)
        );
    }

    #[test]
    fn test_drift_when_value_file_changes_after_localize() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let op = setup(root);
        // 假设已 localize（旧值 8080）
        std::fs::write(root.join(".env"), "SERVICE_PORT=8080\nREPLICAS=1\n").unwrap();
        // 客户随后改了值文件，但没重新 localize
        std::fs::create_dir_all(root.join("values")).unwrap();
        std::fs::write(root.join("values/sys_value.yml"), "SERVICE_PORT: 9090\n").unwrap();

        let report = detect_drift(&op, root).assert();
        assert_eq!(report.status(), DriftStatus::Drifted);
        let changed: Vec<_> = report
            .changes()
            .iter()
            .filter(|c| c.key() == "SERVICE_PORT")
            .collect();
        assert_eq!(changed.len(), 1, "{:?}", describe(&report));
        assert_eq!(changed[0].kind(), ChangeKind::Changed);
        assert_eq!(changed[0].expected(), Some("9090"));
        assert_eq!(changed[0].actual(), Some("8080"));
    }

    #[test]
    fn test_drift_added_and_removed() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let op = setup(root);
        // .env 缺 REPLICAS（added），多了一个已废弃键（removed）
        std::fs::write(root.join(".env"), "SERVICE_PORT=8080\nOLD_KEY=x\n").unwrap();

        let report = detect_drift(&op, root).assert();
        assert_eq!(report.status(), DriftStatus::Drifted);
        let kinds: BTreeMap<&str, ChangeKind> = report
            .changes()
            .iter()
            .map(|c| (c.key(), c.kind()))
            .collect();
        assert_eq!(kinds.get("REPLICAS"), Some(&ChangeKind::Added));
        assert_eq!(kinds.get("OLD_KEY"), Some(&ChangeKind::Removed));
    }

    #[test]
    fn test_customer_override_takes_precedence() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let op = setup(root);
        std::fs::create_dir_all(root.join("values")).unwrap();
        std::fs::write(root.join("values/sys_value.yml"), "SERVICE_PORT: 9090\n").unwrap();
        std::fs::write(root.join("values/value.yml"), "SERVICE_PORT: 7070\n").unwrap();
        // .env 反映最终生效值（客户覆盖优先）
        std::fs::write(root.join(".env"), "SERVICE_PORT=7070\nREPLICAS=1\n").unwrap();

        let report = detect_drift(&op, root).assert();
        assert_eq!(
            report.status(),
            DriftStatus::Clean,
            "{:?}",
            describe(&report)
        );
    }

    #[test]
    fn test_parse_env_ignores_blank_and_comments() {
        let map = parse_env("# comment\n\nA=1\nB=2\n");
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("A").map(String::as_str), Some("1"));
        assert_eq!(map.get("B").map(String::as_str), Some("2"));
    }
}
