//! 值变更表格：比对「初始层」（系统 / 模块默认值）与「生效层」（合并覆盖后的值），
//! 以等宽文字表呈现每个键的默认值、生效值、来源（origin）与可变性（mutability）。
//!
//! 供 `gops sys diff` / `gops mod diff`，以及 `localize` 结尾的变更提示复用。

use crate::internal_prelude::*;
use orion_vars::vars::{Mutability, ValueType};

use crate::project::env_raw_value;

/// 单元格最大字符数，超出以 `…` 截断，避免长值撑破表格。
const MAX_CELL: usize = 48;

/// 单个键的变更状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueState {
    /// 未覆盖：初始层与生效层一致。
    Same,
    /// 已覆盖：两层都有但取值不同。
    Changed,
    /// 初始层没有、生效层新增。
    Added,
    /// 生效层已移除（初始层仍有）。
    Removed,
}

impl ValueState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ValueState::Same => "same",
            ValueState::Changed => "changed",
            ValueState::Added => "added",
            ValueState::Removed => "removed",
        }
    }

    /// STATE 单元格的着色（错开不可见转义，仅在 `use_color()` 为真时使用）。
    fn ansi(&self) -> Option<&'static str> {
        match self {
            ValueState::Changed => Some("\x1b[33m"), // 黄：被覆盖
            ValueState::Added => Some("\x1b[32m"),   // 绿：新增
            ValueState::Removed => Some("\x1b[31m"), // 红：移除
            ValueState::Same => None,
        }
    }
}

impl std::fmt::Display for ValueState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一行的值变更记录。
#[derive(Clone, Debug)]
pub struct ValueRow {
    key: String,
    initial: Option<String>,
    effective: Option<String>,
    origin: Option<String>,
    mutability: Option<String>,
    state: ValueState,
}

impl ValueRow {
    pub fn key(&self) -> &str {
        &self.key
    }
    /// 初始层的取值（`Added` 时为 `None`）。
    pub fn initial(&self) -> Option<&str> {
        self.initial.as_deref()
    }
    /// 生效层的取值（`Removed` 时为 `None`）。
    pub fn effective(&self) -> Option<&str> {
        self.effective.as_deref()
    }
    /// 生效值来自哪一层（如 `sys-defaults` / `customer` / `mod-setting`）。
    pub fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }
    /// 生效值的可变性（`immutable` / `system` / `module`）。
    pub fn mutability(&self) -> Option<&str> {
        self.mutability.as_deref()
    }
    pub fn state(&self) -> ValueState {
        self.state
    }

    /// 机器可读形式（用于 `--json`）。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "key": self.key,
            "initial": self.initial,
            "effective": self.effective,
            "origin": self.origin,
            "mutability": self.mutability,
            "state": self.state.as_str(),
        })
    }
}

/// 逐键比对「初始层」与「生效层」，返回**全部**键（含未变更项，由调用方按需过滤）。
///
/// - 生效层有、初始层无 → [`ValueState::Added`]；
/// - 两层都有、取值不同 → [`ValueState::Changed`]；
/// - 两层都有、取值相同 → [`ValueState::Same`]；
/// - 初始层有、生效层无 → [`ValueState::Removed`]。
pub fn diff_layers(initial: &OriginDict, effective: &OriginDict) -> Vec<ValueRow> {
    let mut rows = Vec::with_capacity(effective.len());
    for (key, value) in effective.iter() {
        let base = initial.get_case_insensitive(key.as_str());
        let (state, initial_val) = match base {
            None => (ValueState::Added, None),
            Some(b) if b.value() == value.value() => {
                (ValueState::Same, Some(display_value(b.value())))
            }
            Some(b) => (ValueState::Changed, Some(display_value(b.value()))),
        };
        rows.push(ValueRow {
            key: key.as_str().to_string(),
            initial: initial_val,
            effective: Some(display_value(value.value())),
            origin: value.origin().clone(),
            mutability: Some(mutability_str(value.mutability()).to_string()),
            state,
        });
    }
    for (key, value) in initial.iter() {
        if effective.get_case_insensitive(key.as_str()).is_none() {
            rows.push(ValueRow {
                key: key.as_str().to_string(),
                initial: Some(display_value(value.value())),
                effective: None,
                origin: value.origin().clone(),
                mutability: Some(mutability_str(value.mutability()).to_string()),
                state: ValueState::Removed,
            });
        }
    }
    rows
}

/// 渲染等宽表格（表头 + 各行）。`color` 为真时按状态给 `STATE` 单元格上色。
pub fn render_table(rows: &[ValueRow], color: bool) -> String {
    const HEADERS: [&str; 6] = [
        "KEY",
        "INITIAL",
        "EFFECTIVE",
        "ORIGIN",
        "MUTABILITY",
        "STATE",
    ];
    let cells: Vec<[String; 6]> = rows
        .iter()
        .map(|r| {
            [
                truncate(&r.key),
                truncate(r.initial.as_deref().unwrap_or("-")),
                truncate(r.effective.as_deref().unwrap_or("-")),
                truncate(r.origin.as_deref().unwrap_or("-")),
                truncate(r.mutability.as_deref().unwrap_or("-")),
                r.state.as_str().to_string(),
            ]
        })
        .collect();

    let mut widths: [usize; 6] = HEADERS.map(char_len);
    for row in &cells {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(char_len(cell));
        }
    }

    let sep: String = "-".repeat(widths.iter().sum::<usize>() + 2 * (HEADERS.len() - 1));
    let mut out = String::new();
    out.push_str(&join_cells(&HEADERS.map(String::from), &widths));
    out.push('\n');
    out.push_str(&sep);
    out.push('\n');
    for (row, raw) in rows.iter().zip(cells.iter()) {
        out.push_str(&render_row(raw, &widths, row.state(), color));
        out.push('\n');
    }
    out
}

/// 平铺 JSON 数组（`gops sys diff --json`）。
pub fn render_json(rows: &[ValueRow]) -> String {
    let arr: Vec<serde_json::Value> = rows.iter().map(ValueRow::to_json).collect();
    serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".to_string())
}

/// 分组 JSON：`[{ "model": ..., "changes": [ ... ] }]`（`gops mod diff --json`）。
pub fn render_json_models(changes: &[(String, Vec<ValueRow>)]) -> String {
    let arr: Vec<serde_json::Value> = changes
        .iter()
        .map(|(model, rows)| {
            serde_json::json!({
                "model": model,
                "changes": rows.iter().map(ValueRow::to_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".to_string())
}

/// 是否给终端输出上色：仅当 stdout 是终端且 `NO_COLOR` 为空/未设置（遵循 NO_COLOR spec）。
pub fn use_color() -> bool {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return false;
    }
    match std::env::var_os("NO_COLOR") {
        Some(v) => v.is_empty(),
        None => true,
    }
}

fn display_value(v: &ValueType) -> String {
    env_raw_value(v)
}

fn mutability_str(m: &Mutability) -> &'static str {
    match m {
        Mutability::Immutable => "immutable",
        Mutability::System => "system",
        Mutability::Module => "module",
    }
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

fn truncate(s: &str) -> String {
    if char_len(s) <= MAX_CELL {
        return s.to_string();
    }
    let mut out: String = s.chars().take(MAX_CELL - 1).collect();
    out.push('…');
    out
}

fn join_cells(cells: &[String; 6], widths: &[usize; 6]) -> String {
    let mut line = String::new();
    for i in 0..cells.len() {
        if i > 0 {
            line.push_str("  ");
        }
        line.push_str(&cells[i]);
        line.push_str(&" ".repeat(widths[i].saturating_sub(char_len(&cells[i]))));
    }
    line.trim_end().to_string()
}

fn render_row(cells: &[String; 6], widths: &[usize; 6], state: ValueState, color: bool) -> String {
    let mut line = String::new();
    for i in 0..cells.len() {
        if i > 0 {
            line.push_str("  ");
        }
        let pad = " ".repeat(widths[i].saturating_sub(char_len(&cells[i])));
        match (color, i == 5, state.ansi()) {
            (true, true, Some(code)) => {
                line.push_str(&format!("{code}{}\x1b[0m{pad}", cells[i]));
            }
            _ => {
                line.push_str(&cells[i]);
                line.push_str(&pad);
            }
        }
    }
    line.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(pairs: &[(&str, &str)]) -> OriginDict {
        let mut d = OriginDict::new();
        for (k, v) in pairs {
            d.insert(*k, ValueType::from(*v));
        }
        d
    }

    fn row_of<'a>(rows: &'a [ValueRow], key: &str) -> &'a ValueRow {
        rows.iter().find(|r| r.key() == key).expect("row exists")
    }

    #[test]
    fn diff_classifies_same_changed_added_removed() {
        let mut initial = dict(&[("A", "1"), ("B", "2"), ("GONE", "x")]);
        initial.set_source("sys-defaults");
        let mut effective = dict(&[("A", "1"), ("B", "9"), ("NEW", "n")]);
        effective.set_source("customer");

        let rows = diff_layers(&initial, &effective);
        assert_eq!(row_of(&rows, "A").state(), ValueState::Same);
        assert_eq!(row_of(&rows, "B").state(), ValueState::Changed);
        assert_eq!(row_of(&rows, "NEW").state(), ValueState::Added);
        assert_eq!(row_of(&rows, "GONE").state(), ValueState::Removed);

        let b = row_of(&rows, "B");
        assert_eq!(b.initial(), Some("2"));
        assert_eq!(b.effective(), Some("9"));
        assert_eq!(b.origin(), Some("customer"));
        assert_eq!(b.mutability(), Some("module"));

        let new = row_of(&rows, "NEW");
        assert_eq!(new.initial(), None);
        assert_eq!(new.state(), ValueState::Added);
    }

    #[test]
    fn origin_and_mutability_follow_effective_layer() {
        let initial = dict(&[("K", "default")]);
        // 生效层的 origin/mutability 被保留
        let mut effective = OriginDict::new();
        effective.insert("K", ValueType::from("override"));
        effective.set_source("customer");

        let rows = diff_layers(&initial, &effective);
        let r = row_of(&rows, "K");
        assert_eq!(r.state(), ValueState::Changed);
        assert_eq!(r.origin(), Some("customer"));
    }

    #[test]
    fn table_has_header_columns_and_state_token() {
        let initial = dict(&[("PORT", "8080")]);
        let mut effective = dict(&[("PORT", "9090")]);
        effective.set_source("customer");
        let rows: Vec<_> = diff_layers(&initial, &effective)
            .into_iter()
            .filter(|r| r.state() != ValueState::Same)
            .collect();

        let table = render_table(&rows, false);
        let lines: Vec<&str> = table.lines().collect();
        assert!(lines[0].starts_with("KEY"));
        for col in ["INITIAL", "EFFECTIVE", "ORIGIN", "MUTABILITY", "STATE"] {
            assert!(lines[0].contains(col), "header missing {col}: {}", lines[0]);
        }
        assert!(lines[1].chars().all(|c| c == '-'));
        let data = lines[2];
        assert!(data.contains("PORT"));
        assert!(data.contains("8080"));
        assert!(data.contains("9090"));
        assert!(data.contains("changed"));
    }

    #[test]
    fn table_renders_no_ansi_without_color() {
        let rows = vec![ValueRow {
            key: "K".into(),
            initial: Some("1".into()),
            effective: Some("2".into()),
            origin: Some("customer".into()),
            mutability: Some("module".into()),
            state: ValueState::Changed,
        }];
        let plain = render_table(&rows, false);
        assert!(!plain.contains('\x1b'));
        let colored = render_table(&rows, true);
        assert!(colored.contains("\x1b[33m"));
    }

    #[test]
    fn long_values_are_truncated() {
        let long = "x".repeat(200);
        let mut initial = OriginDict::new();
        initial.insert("K", ValueType::from("short"));
        let mut effective = OriginDict::new();
        effective.insert("K", ValueType::from(long.as_str()));
        let rows = diff_layers(&initial, &effective);
        let table = render_table(&rows, false);
        assert!(table.contains('…'));
        assert!(!table.contains(&long));
    }

    #[test]
    fn json_contains_all_fields() {
        let initial = dict(&[("PORT", "8080")]);
        let mut effective = dict(&[("PORT", "9090")]);
        effective.set_source("customer");
        let rows = diff_layers(&initial, &effective);
        let json = render_json(&rows);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let first = &parsed.as_array().unwrap()[0];
        assert_eq!(first["key"], "PORT");
        assert_eq!(first["initial"], "8080");
        assert_eq!(first["effective"], "9090");
        assert_eq!(first["origin"], "customer");
        assert_eq!(first["state"], "changed");
    }

    #[test]
    fn json_models_groups_by_model() {
        let rows = vec![ValueRow {
            key: "K".into(),
            initial: Some("1".into()),
            effective: Some("2".into()),
            origin: None,
            mutability: Some("module".into()),
            state: ValueState::Changed,
        }];
        let groups = vec![("arm-mac14-host".to_string(), rows)];
        let json = render_json_models(&groups);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed[0]["model"], "arm-mac14-host");
        assert_eq!(parsed[0]["changes"][0]["key"], "K");
    }

    #[test]
    fn empty_rows_render_header_only() {
        let table = render_table(&[], false);
        assert_eq!(table.lines().count(), 2);
    }
}
