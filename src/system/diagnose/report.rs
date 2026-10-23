//! `gops run diagnose` 的报告模型、渲染与退出码。
//!
//! 契约见 `docs/design/run-diagnose.md` §6 / §8：
//! - 内置检查与流程（`diagnose`）交回的 check **用同一套词汇**（`ok|warn|fail`）；
//!   渲染与退出码**只由本模块**决定，流程不参与。
//! - `code` 稳定且带命名空间（`gops.` / `wparse.` / …），脚本按它取用。
//!
//! 刻意不复用 `ops_prj::diagnose` 的 `CheckItem`：那边没有 `source` 与 `evidence`，
//! 且构造器是私有的。词汇（`[OK]/[WARN]/[FAIL]`、OK/WARN/FAIL 排序）与它保持一致。

use std::fmt::Write as _;

use serde_json::{Value, json};

/// ANSI 样式码（与 `prj diagnose` / agentd `diagnose` 同一套）。
const BOLD: &str = "1";
const RED: &str = "31";
const GREEN: &str = "32";
const YELLOW: &str = "33";
const CYAN: &str = "36";
const RESET: &str = "\x1b[0m";

/// 单条 `detail` 的字符上限（防止流程把整份文件塞进来）。
const MAX_DETAIL_CHARS: usize = 8 * 1024;
/// `evidence` 序列化后的字节上限（契约 §6）。
const MAX_EVIDENCE_BYTES: usize = 16 * 1024;

/// 检查结果级别。与 agentd / `prj diagnose` 对齐为 OK / WARN / FAIL。
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

    /// 机器可读名（JSON / 脚本用）。
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

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "ok" => Some(CheckStatus::Ok),
            "warn" => Some(CheckStatus::Warn),
            "fail" => Some(CheckStatus::Fail),
            _ => None,
        }
    }
}

/// 一条检查结果。`code` 稳定；`source` 标明它是谁给的（`gops` / 流程名）。
#[derive(Clone, Debug)]
pub struct Check {
    pub code: String,
    pub source: String,
    pub status: CheckStatus,
    pub title: String,
    pub detail: Option<String>,
    pub hint: Option<String>,
    pub evidence: Option<Value>,
}

impl Check {
    pub fn new(
        source: impl Into<String>,
        status: CheckStatus,
        code: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            source: source.into(),
            status,
            title: title.into(),
            detail: None,
            hint: None,
            evidence: None,
        }
    }

    pub fn ok(
        source: impl Into<String>,
        code: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        Self::new(source, CheckStatus::Ok, code, title)
    }

    pub fn warn(
        source: impl Into<String>,
        code: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        Self::new(source, CheckStatus::Warn, code, title)
    }

    pub fn fail(
        source: impl Into<String>,
        code: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        Self::new(source, CheckStatus::Fail, code, title)
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        if !detail.is_empty() {
            self.detail = Some(truncate_chars(&detail, MAX_DETAIL_CHARS));
        }
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        let hint = hint.into();
        if !hint.is_empty() {
            self.hint = Some(hint);
        }
        self
    }

    pub fn with_evidence(mut self, evidence: Value) -> Self {
        // 超限就整块换成「省略」标记，不截断 —— 半个 JSON 比没有更误导。
        let too_big = serde_json::to_string(&evidence)
            .map(|text| text.len() > MAX_EVIDENCE_BYTES)
            .unwrap_or(true);
        self.evidence = Some(if too_big {
            json!({ "omitted": true, "reason": "evidence too large" })
        } else {
            evidence
        });
        self
    }

    fn to_json(&self) -> Value {
        let mut out = json!({
            "code": self.code,
            "source": self.source,
            "status": self.status.as_str(),
            "title": self.title,
        });
        if let Some(detail) = &self.detail {
            out["detail"] = json!(detail);
        }
        if let Some(hint) = &self.hint {
            out["hint"] = json!(hint);
        }
        if let Some(evidence) = &self.evidence {
            out["evidence"] = evidence.clone();
        }
        out
    }
}

/// 一条 source 的计数。
#[derive(Clone, Copy, Debug, Default)]
struct SourceCounts {
    ok: usize,
    warn: usize,
    fail: usize,
}

/// 分组统计里的一条：**为 0 的段不打印**。
///
/// `0 fail` 在日志查看器 / CI 的关键词高亮里会被涂红，看起来像“有失败”；
/// 分组名已经限定了上下文，省掉零段不丢信息。
fn count_segments(counts: &SourceCounts) -> String {
    let mut parts = Vec::new();
    if counts.ok > 0 {
        parts.push(format!("{} ok", counts.ok));
    }
    if counts.warn > 0 {
        parts.push(format!("{} warn", counts.warn));
    }
    if counts.fail > 0 {
        parts.push(format!("{} fail", counts.fail));
    }
    if parts.is_empty() {
        "无检查".to_string()
    } else {
        parts.join(" / ")
    }
}

/// 一次诊断的全部检查结果。
#[derive(Clone, Debug, Default)]
pub struct Report {
    checks: Vec<Check>,
}

impl Report {
    pub fn push(&mut self, check: Check) {
        self.checks.push(check);
    }

    pub fn extend(&mut self, checks: impl IntoIterator<Item = Check>) {
        self.checks.extend(checks);
    }

    pub fn checks(&self) -> &[Check] {
        &self.checks
    }

    /// 各状态计数：`(ok, warn, fail)`。
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut ok = 0;
        let mut warn = 0;
        let mut fail = 0;
        for check in &self.checks {
            match check.status {
                CheckStatus::Ok => ok += 1,
                CheckStatus::Warn => warn += 1,
                CheckStatus::Fail => fail += 1,
            }
        }
        (ok, warn, fail)
    }

    /// `0` = 无 fail（`fail_on_warn` 时 warn 也算失败）；`1` = 至少一条 fail。
    ///
    /// 「诊断本身跑不起来」用 `2`，由 CLI 层在采集失败时给出（设计稿 §8.3）。
    pub fn exit_code(&self, fail_on_warn: bool) -> i32 {
        let (_, warn, fail) = self.counts();
        if fail > 0 || (fail_on_warn && warn > 0) {
            1
        } else {
            0
        }
    }

    /// 排序：`fail → warn → ok`；同级按 `source`、`code`（输出稳定，便于 diff）。
    pub fn sort(&mut self) {
        self.checks.sort_by(|a, b| {
            b.status
                .cmp(&a.status)
                .then_with(|| a.source.cmp(&b.source))
                .then_with(|| a.code.cmp(&b.code))
        });
    }

    /// 按 `source` 汇总（保持首次出现顺序）。
    fn by_source(&self) -> Vec<(&str, SourceCounts)> {
        let mut out: Vec<(&str, SourceCounts)> = Vec::new();
        for check in &self.checks {
            let index = match out.iter().position(|(name, _)| *name == check.source) {
                Some(index) => index,
                None => {
                    out.push((check.source.as_str(), SourceCounts::default()));
                    out.len() - 1
                }
            };
            let counts = &mut out[index].1;
            match check.status {
                CheckStatus::Ok => counts.ok += 1,
                CheckStatus::Warn => counts.warn += 1,
                CheckStatus::Fail => counts.fail += 1,
            }
        }
        out
    }

    /// 合并流程交回的 `Flow Result v1`（契约 §6）。
    ///
    /// 返回合并条数；格式不合法返回 `Err(可读原因)`（调用方记成 `gops.ext.<flow>` 失败）。
    pub fn merge_flow_result(&mut self, raw: &str, fallback_source: &str) -> Result<usize, String> {
        let value: Value =
            serde_json::from_str(raw).map_err(|err| format!("result.json 不是合法 JSON：{err}"))?;
        let items = value
            .get("checks")
            .and_then(Value::as_array)
            .ok_or_else(|| "result.json 缺 `checks` 数组".to_string())?;
        let source = value
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or(fallback_source)
            .to_string();

        let mut merged = 0;
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (index, item) in items.iter().enumerate() {
            let status_raw = item
                .get("status")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("checks[{index}] 缺 `status`"))?;
            let status = CheckStatus::parse(status_raw).ok_or_else(|| {
                format!("checks[{index}] 的 status=`{status_raw}` 非法（只接受 ok|warn|fail）")
            })?;
            let code = item
                .get("code")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("checks[{index}] 缺 `code`"))?;
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("checks[{index}] 缺 `title`"))?;
            // 契约 §6：同一次运行内 `code` 不得重复（否则脚本按 code 取项会拿到两条）。
            if !seen.insert(code.to_string()) {
                return Err(format!("checks[{index}] 的 code=`{code}` 重复"));
            }
            let mut check = Check::new(source.clone(), status, code, title);
            if let Some(detail) = item.get("detail").and_then(Value::as_str) {
                check = check.with_detail(detail);
            }
            if let Some(hint) = item.get("hint").and_then(Value::as_str) {
                check = check.with_hint(hint);
            }
            if let Some(evidence) = item.get("evidence") {
                check = check.with_evidence(evidence.clone());
            }
            self.push(check);
            merged += 1;
        }
        Ok(merged)
    }

    /// 流程崩溃 / 结果不可读：记一条 FAIL（不掩盖内置结果）。
    pub fn flow_failure(&mut self, source: &str, detail: &str) {
        self.push(
            Check::fail(
                "gops",
                format!("gops.ext.{source}"),
                format!("`{source}` 流程执行失败"),
            )
            .with_detail(detail)
            .with_hint("单独跑该流程看完整输出；确认 `gx -V` 可用、流程读写 GOPS_DIAGNOSE_* 路径"),
        );
    }

    /// 流程跑完但没交结果：WARN（不判 FAIL —— 可能只是没有结论）。
    pub fn flow_empty(&mut self, source: &str) {
        self.push(
            Check::warn(
                "gops",
                format!("gops.ext.{source}"),
                format!("`{source}` 流程未输出结果"),
            )
            .with_detail("流程已执行，但没有写 GOPS_DIAGNOSE_RESULT（或 checks 为空）")
            .with_hint("按契约 §6 写 result.json；确实没有结论就不必写该文件"),
        );
    }

    /// 项目里根本就没有这个流程（`gx run --exists` 失败）：gxl 系统要说明「体检没做」。
    pub fn flow_absent(&mut self, source: &str, reason: &str) {
        self.push(
            Check::warn(
                "gops",
                format!("gops.ext.{source}"),
                format!("系统未定义 `{source}` 流程"),
            )
            .with_detail(reason)
            .with_hint(
                "在 `sys/workflows/operators.gxl` 的 `mod operators` 里加 `flow diagnose`；\
                 并确认 gx 支持 `run --exists`（galaxy-flow >= 0.14）",
            ),
        );
    }

    /// 被 `--no-ext` 显式跳过：对 gxl 系统等于「本次没有任何结论」，必须说出来。
    pub fn flow_skipped(&mut self, source: &str) {
        self.push(
            Check::warn(
                "gops",
                format!("gops.ext.{source}"),
                format!("已跳过 `{source}` 流程（--no-ext）"),
            )
            .with_detail("gxl 系统的体检主体就是该流程，本次没有任何检查结论")
            .with_hint("去掉 --no-ext；只想要内置检查时，这属于预期行为"),
        );
    }

    fn verdict(&self, fail_on_warn: bool) -> String {
        let (ok, warn, fail) = self.counts();
        if fail == 0 && warn == 0 {
            return format!("全部正常（{ok} 项）");
        }
        let mut text = format!("{ok} 项正常 / {warn} 项警告 / {fail} 项失败");
        if let Some(first) = self
            .checks
            .iter()
            .find(|check| check.status == CheckStatus::Fail)
        {
            let _ = write!(text, "；首要问题：{}", first.title);
        } else if fail_on_warn {
            let _ = write!(text, "（--fail-on warn）");
        }
        text
    }

    /// 人读文本。`color = false` 时**逐字节可断言**（重定向 / `--no-color` / NO_COLOR）。
    ///
    /// 版式：`[FAIL] <code>` + 标题 + detail 缩进 + `→` 提示（青色）+ 分组统计 + 结论。
    pub fn render_text(&self, color: bool, header: &str, fail_on_warn: bool) -> String {
        let paint = |code: &str, text: &str| -> String {
            if color {
                format!("\x1b[{code}m{text}{RESET}")
            } else {
                text.to_string()
            }
        };

        let mut out = String::new();
        let _ = writeln!(out, "{}", paint(BOLD, header));
        for check in &self.checks {
            let tag = paint(&format!("1;{}", check.status.ansi()), check.status.tag());
            let _ = writeln!(out, "{tag} {}", check.code);
            let _ = writeln!(out, "       {}", paint(BOLD, &check.title));
            if let Some(detail) = &check.detail {
                for line in detail.lines() {
                    let _ = writeln!(out, "       {line}");
                }
            }
            if let Some(hint) = &check.hint {
                for (index, line) in hint.lines().enumerate() {
                    let marker = if index == 0 { "→ " } else { "  " };
                    let _ = writeln!(out, "       {}", paint(CYAN, &format!("{marker}{line}")));
                }
            }
        }

        // 分组统计（按 source）：一眼看出是环境问题，还是流程没跑。
        let sources = self.by_source();
        if sources.len() > 1 {
            let parts: Vec<String> = sources
                .iter()
                .map(|(name, counts)| format!("{name}: {}", count_segments(counts)))
                .collect();
            let _ = writeln!(out, "{}", paint(BOLD, &parts.join("   ")));
        }

        let verdict = self.verdict(fail_on_warn);
        let overall = if self.exit_code(fail_on_warn) == 0 {
            CheckStatus::Ok
        } else {
            CheckStatus::Fail
        };
        let _ = writeln!(
            out,
            "\n{} {}",
            paint(BOLD, "结论:"),
            paint(&format!("1;{}", overall.ansi()), &verdict)
        );
        out
    }

    /// `--json`：stdout 上**只有** JSON（人读与调试走 stderr）。
    pub fn render_json(
        &self,
        system: &Value,
        context: Option<&Value>,
        fail_on_warn: bool,
    ) -> String {
        let (ok, warn, fail) = self.counts();
        let sources: Vec<Value> = self
            .by_source()
            .into_iter()
            .map(|(name, counts)| {
                json!({
                    "name": name,
                    "checks": counts.ok + counts.warn + counts.fail,
                    "ok": counts.ok,
                    "warn": counts.warn,
                    "fail": counts.fail,
                    "ran": true,
                })
            })
            .collect();
        let checks: Vec<Value> = self.checks.iter().map(Check::to_json).collect();
        let mut out = json!({
            "schema_version": "1",
            "system": system,
            "generated_at": chrono::Local::now().to_rfc3339(),
            "summary": {
                "ok": ok,
                "warn": warn,
                "fail": fail,
                "exit_code": self.exit_code(fail_on_warn),
            },
            "sources": sources,
            "checks": checks,
        });
        if let Some(context) = context {
            out["context"] = context.clone();
        }
        serde_json::to_string_pretty(&out).unwrap_or_else(|_| "{}".to_string())
    }
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Report {
        let mut report = Report::default();
        report.push(Check::ok("gops", "gops.a", "fine"));
        report.push(
            Check::fail("diagnose", "wparse.work_drift", "drift")
                .with_detail("clone 多 3 个文件")
                .with_hint("先看 show-wparse-state.sh"),
        );
        report.push(Check::warn("gops", "gops.b", "meh"));
        report
    }

    #[test]
    fn counts_and_exit_codes_follow_the_contract() {
        let report = sample();
        assert_eq!(report.counts(), (1, 1, 1));
        assert_eq!(report.exit_code(false), 1);

        let mut ok_only = Report::default();
        ok_only.push(Check::ok("gops", "gops.c", "fine"));
        assert_eq!(ok_only.exit_code(false), 0);

        let mut warn_only = Report::default();
        warn_only.push(Check::warn("gops", "gops.c", "meh"));
        assert_eq!(warn_only.exit_code(false), 0, "默认 warn 不改变退出码");
        assert_eq!(warn_only.exit_code(true), 1, "--fail-on warn 收紧");
    }

    #[test]
    fn sorting_puts_failures_first_and_is_stable() {
        let mut report = sample();
        report.push(Check::fail("gops", "gops.z", "z"));
        report.sort();
        let order: Vec<&str> = report.checks().iter().map(|c| c.code.as_str()).collect();
        // 两条 fail：先按 source（diagnose < gops），再按 code。
        assert_eq!(
            order,
            vec!["wparse.work_drift", "gops.z", "gops.b", "gops.a"]
        );
    }

    #[test]
    fn text_rendering_without_color_is_byte_stable() {
        let mut report = sample();
        report.sort();
        let text = report.render_text(false, "sys · docker-compose", false);
        assert!(text.starts_with("sys · docker-compose\n"));
        assert!(text.contains("[FAIL] wparse.work_drift"));
        assert!(text.contains("       drift"));
        assert!(text.contains("→ 先看 show-wparse-state.sh"));
        assert!(text.contains("1 项正常 / 1 项警告 / 1 项失败"));
        assert!(!text.contains('\u{1b}'), "无色输出里不能有 ANSI 码");
    }

    #[test]
    fn text_rendering_with_color_paints_tags() {
        let text = sample().render_text(true, "h", false);
        assert!(text.contains("\u{1b}[1;31m[FAIL]"));
        assert!(text.contains(RESET));
    }

    #[test]
    fn flow_result_merges_checks_and_source() {
        let mut report = Report::default();
        let raw = r#"{
            "schema_version": "1",
            "source": "diagnose",
            "checks": [
                {"code": "wparse.work_drift", "status": "fail", "title": "drift",
                 "detail": "d", "hint": "h", "evidence": {"only_source": ["a"]}}
            ]
        }"#;
        assert_eq!(report.merge_flow_result(raw, "diagnose").unwrap(), 1);
        let check = &report.checks()[0];
        assert_eq!(check.source, "diagnose");
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.evidence.is_some());
    }

    #[test]
    fn flow_result_rejects_bad_status_and_missing_fields() {
        let mut report = Report::default();
        let err = report
            .merge_flow_result(
                r#"{"checks":[{"code":"x","status":"error","title":"t"}]}"#,
                "d",
            )
            .unwrap_err();
        assert!(err.contains("非法"), "{err}");
        assert!(report.checks().is_empty());

        let err = report
            .merge_flow_result(r#"{"checks":[{"status":"ok","title":"t"}]}"#, "d")
            .unwrap_err();
        assert!(err.contains("缺 `code`"), "{err}");
    }

    #[test]
    fn oversized_evidence_is_replaced_not_truncated() {
        let big = json!({ "blob": "x".repeat(MAX_EVIDENCE_BYTES + 16) });
        let check = Check::ok("gops", "gops.x", "t").with_evidence(big);
        let evidence = check.evidence.unwrap();
        assert_eq!(evidence["omitted"], json!(true));
    }

    #[test]
    fn json_output_has_summary_and_one_entry_per_source() {
        let mut report = sample();
        report.sort();
        let text = report.render_json(&json!({"name": "s"}), None, false);
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["summary"]["fail"], json!(1));
        assert_eq!(value["summary"]["exit_code"], json!(1));
        assert_eq!(value["checks"][0]["code"], json!("wparse.work_drift"));
        let sources = value["sources"].as_array().unwrap();
        assert_eq!(sources.len(), 2, "内置与流程各一组");
    }

    #[test]
    fn flow_result_rejects_duplicate_codes() {
        let mut report = Report::default();
        let raw = r#"{"checks":[
            {"code":"x.y","status":"ok","title":"a"},
            {"code":"x.y","status":"warn","title":"b"}
        ]}"#;
        let err = report.merge_flow_result(raw, "d").unwrap_err();
        assert!(err.contains("重复"), "{err}");
    }

    #[test]
    fn flow_failure_and_empty_use_the_gops_source() {
        let mut report = Report::default();
        report.flow_failure("diagnose", "gx 超时");
        report.flow_empty("diagnose");
        report.flow_absent("diagnose", "no such flow");
        report.flow_skipped("diagnose");
        let sources: Vec<&str> = report
            .checks()
            .iter()
            .map(|check| check.source.as_str())
            .collect();
        assert_eq!(sources, vec!["gops", "gops", "gops", "gops"]);
        let (_, warn, fail) = report.counts();
        assert_eq!((warn, fail), (3, 1));
    }

    #[test]
    fn json_carries_context_only_when_asked() {
        let mut report = Report::default();
        report.push(Check::ok("gops", "gops.a", "fine"));
        let without = report.render_json(&json!({"name": "s"}), None, false);
        assert!(!without.contains("\"context\""));

        let context = json!({"schema_version": "1"});
        let with = report.render_json(&json!({"name": "s"}), Some(&context), false);
        let value: Value = serde_json::from_str(&with).unwrap();
        assert_eq!(value["context"]["schema_version"], json!("1"));
    }

    #[test]
    fn multi_source_counts_appear_in_text_and_json() {
        let mut report = Report::default();
        report.push(Check::ok("gops", "gops.a", "a"));
        report.push(Check::warn("diagnose", "w.parse", "b"));
        let text = report.render_text(false, "h", false);
        assert!(text.contains("gops: 1 ok"), "{text}");
        assert!(text.contains("diagnose: 1 warn"), "{text}");
        assert!(
            !text.contains("0 fail"),
            "为 0 的段不该打印（会被日志高亮涂红）：{text}"
        );

        let value: Value =
            serde_json::from_str(&report.render_json(&json!({}), None, false)).unwrap();
        let sources = value["sources"].as_array().unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[1]["name"], json!("diagnose"));
        assert_eq!(sources[1]["checks"], json!(1));
        assert_eq!(value["summary"]["exit_code"], json!(0), "warn 不改变退出码");
    }

    #[test]
    fn empty_detail_and_hint_are_dropped() {
        let check = Check::ok("gops", "gops.a", "a")
            .with_detail("")
            .with_hint("");
        assert!(check.detail.is_none());
        assert!(check.hint.is_none());
    }

    #[test]
    fn long_detail_is_truncated() {
        let check = Check::ok("gops", "gops.x", "t").with_detail("y".repeat(MAX_DETAIL_CHARS + 10));
        let detail = check.detail.unwrap();
        assert_eq!(detail.chars().count(), MAX_DETAIL_CHARS + 1);
        assert!(detail.ends_with('…'));
    }
}
