//! 变更呈现（供 `gops sys diff` / `gops mod diff` 与 `localize` 结尾复用）：
//!
//! - **值变更表**：比对「初始层」（系统 / 模块默认值）与「生效层」（合并覆盖后的值），
//!   逐键列出 `KEY` / `INITIAL` / `EFFECTIVE` / `ORIGIN` / `MUTABILITY` / `STATE`；
//! - **文件变更表**：localize 渲染文件前后各取一次内容指纹快照（[`snapshot_tree`]），
//!   只列出**新增 / 替换**的文件（`FILE` / `STATE`）。

use crate::internal_prelude::*;
use orion_vars::vars::{Mutability, ValueType};

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

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
    out.push_str(&join_row(
        &HEADERS.map(String::from),
        &widths,
        None,
        false,
        5,
    ));
    out.push('\n');
    out.push_str(&sep);
    out.push('\n');
    for (row, raw) in rows.iter().zip(cells.iter()) {
        out.push_str(&join_row(raw, &widths, row.state.ansi(), color, 5));
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
/// 只含**有变更**的分组（与人类可读输出一致）。
pub fn render_json_models(changes: &[(String, Vec<ValueRow>)]) -> String {
    let arr: Vec<serde_json::Value> = changes
        .iter()
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(model, rows)| {
            serde_json::json!({
                "model": model,
                "changes": rows.iter().map(ValueRow::to_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".to_string())
}

/// `gops sys diff --json`：`{ "system": [...], "modules": [{ "module": ..., "changes": [...] }] }`。
///
/// 系统层与各模块分组分开，便于脚本按范围消费。
pub fn render_sys_diff_json(system: &[ValueRow], modules: &[(String, Vec<ValueRow>)]) -> String {
    let modules_json: Vec<serde_json::Value> = modules
        .iter()
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(name, rows)| {
            serde_json::json!({
                "module": name,
                "changes": rows.iter().map(ValueRow::to_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    let out = serde_json::json!({
        "system": system.iter().map(ValueRow::to_json).collect::<Vec<_>>(),
        "modules": modules_json,
    });
    serde_json::to_string_pretty(&out).unwrap_or_else(|_| "{}".to_string())
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

/// 文件级变更状态（只区分「新增 / 替换」）。
///
/// localize 会**重建**输出树（`mod/<model>/local/` 先清空再渲染），所以“删除”是常态而非信号，
/// 不报；只报内容真正新增或改变的文件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileState {
    /// 输出树中新增（此前快照无此文件）。
    Created,
    /// 已存在但内容变化（替换）。
    Replaced,
}

impl FileState {
    pub fn as_str(&self) -> &'static str {
        match self {
            FileState::Created => "created",
            FileState::Replaced => "replaced",
        }
    }

    fn ansi(&self) -> &'static str {
        match self {
            FileState::Created => "\x1b[32m",  // 绿：新增
            FileState::Replaced => "\x1b[33m", // 黄：替换
        }
    }
}

impl std::fmt::Display for FileState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一行的文件变更记录（`path` 为相对快照根、以 `/` 分隔）。
#[derive(Clone, Debug)]
pub struct FileRow {
    path: String,
    state: FileState,
}

impl FileRow {
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn state(&self) -> FileState {
        self.state
    }

    /// 机器可读形式（用于 `--json`）。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "path": self.path, "state": self.state.as_str() })
    }
}

/// 输出树快照：相对路径（`/` 分隔）→ 内容 sha256（十六进制）。
///
/// 根不存在或不是目录时返回空。用于 localize 前后比对，得知哪些文件**新增/替换**。
pub fn snapshot_tree(root: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if !root.is_dir() {
        return map;
    }
    for entry in walkdir::WalkDir::new(root) {
        let Ok(entry) = entry else {
            continue;
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if let Ok(bytes) = std::fs::read(path) {
            map.insert(rel, hash_bytes(&bytes));
        }
    }
    map
}

/// 比对两次快照，返回**新增 / 替换**的文件（内容相同不报；“删除”不报，见 [`FileState`]）。
pub fn diff_files(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<FileRow> {
    let mut rows = Vec::new();
    for (rel, hash) in after {
        match before.get(rel) {
            Some(b) if b == hash => {}
            Some(_) => rows.push(FileRow {
                path: rel.clone(),
                state: FileState::Replaced,
            }),
            None => rows.push(FileRow {
                path: rel.clone(),
                state: FileState::Created,
            }),
        }
    }
    rows
}

/// 渲染文件变更表（表头 + 各行）。`color` 为真时按状态给 `STATE` 单元格上色。
pub fn render_file_table(rows: &[FileRow], color: bool) -> String {
    const HEADERS: [&str; 2] = ["FILE", "STATE"];
    let cells: Vec<[String; 2]> = rows
        .iter()
        .map(|r| [truncate(&r.path), r.state.as_str().to_string()])
        .collect();

    let mut widths: [usize; 2] = HEADERS.map(char_len);
    for row in &cells {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(char_len(cell));
        }
    }

    let sep: String = "-".repeat(widths.iter().sum::<usize>() + 2 * (HEADERS.len() - 1));
    let mut out = String::new();
    out.push_str(&join_row(
        &HEADERS.map(String::from),
        &widths,
        None,
        false,
        1,
    ));
    out.push('\n');
    out.push_str(&sep);
    out.push('\n');
    for (row, raw) in rows.iter().zip(cells.iter()) {
        out.push_str(&join_row(raw, &widths, Some(row.state.ansi()), color, 1));
        out.push('\n');
    }
    out
}

/// 文件变更的平铺 JSON 数组。
pub fn render_files_json(rows: &[FileRow]) -> String {
    let arr: Vec<serde_json::Value> = rows.iter().map(FileRow::to_json).collect();
    serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".to_string())
}

/// 在 localize 前后各取一次快照，打印文件变更表；返回变更项数（0 则不打任何东西）。
///
/// `label` 非空时拼在表头；建议形如 `<输出目录> ← <源模板>`（用 [`display_path`] 缩短），
/// 以便同一次 localize 多个渲染目标（模块 `spec/`、`sys/setting/` 写入同一 `local/`）时能区分来源。
pub fn print_file_changes(
    label: &str,
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> usize {
    let rows = diff_files(before, after);
    if rows.is_empty() {
        return 0;
    }
    if label.is_empty() {
        println!("文件变更 ({} 项):", rows.len());
    } else {
        println!("文件变更 ({} 项) @ {label}:", rows.len());
    }
    print!("{}", render_file_table(&rows, use_color()));
    rows.len()
}

/// 便于阅读的路径显示：若位于项目根（`GXL_PRJ_ROOT`，`gops` 启动时设置）或当前目录下，去掉前缀。
pub fn display_path(path: &Path) -> String {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(env_root) = std::env::var_os("GXL_PRJ_ROOT") {
        roots.push(PathBuf::from(env_root));
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    for root in roots {
        if let Ok(rel) = path.strip_prefix(&root) {
            return rel.display().to_string();
        }
    }
    path.display().to_string()
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let out = hasher.finalize();
    let mut s = String::with_capacity(out.len() * 2);
    for b in out.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s
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

/// 把一行单元格按宽度用两空格拼接；`color_ansi` 非空且 `color` 为真时给 `color_col` 列上色。
fn join_row(
    cells: &[String],
    widths: &[usize],
    color_ansi: Option<&'static str>,
    color: bool,
    color_col: usize,
) -> String {
    let mut line = String::new();
    for i in 0..cells.len() {
        if i > 0 {
            line.push_str("  ");
        }
        let pad = " ".repeat(widths[i].saturating_sub(char_len(&cells[i])));
        match (color, color_ansi, i == color_col) {
            (true, Some(code), true) => {
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
    fn json_models_filters_empty_groups() {
        let empty = vec![
            ("m1".to_string(), Vec::new()),
            (
                "m2".to_string(),
                vec![ValueRow {
                    key: "K".into(),
                    initial: Some("1".into()),
                    effective: Some("2".into()),
                    origin: None,
                    mutability: Some("module".into()),
                    state: ValueState::Changed,
                }],
            ),
        ];
        let parsed: serde_json::Value = serde_json::from_str(&render_json_models(&empty)).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 1);
        assert_eq!(parsed[0]["model"], "m2");
    }

    #[test]
    fn sys_diff_json_groups_system_and_modules() {
        let system = vec![ValueRow {
            key: "A".into(),
            initial: None,
            effective: Some("1".into()),
            origin: Some("customer".into()),
            mutability: Some("module".into()),
            state: ValueState::Added,
        }];
        let modules = vec![
            ("clean-mod".to_string(), Vec::new()),
            (
                "warp-parse".to_string(),
                vec![ValueRow {
                    key: "CPU".into(),
                    initial: Some("1000".into()),
                    effective: Some("2000".into()),
                    origin: Some("mod-setting".into()),
                    mutability: Some("module".into()),
                    state: ValueState::Changed,
                }],
            ),
        ];
        let parsed: serde_json::Value =
            serde_json::from_str(&render_sys_diff_json(&system, &modules)).unwrap();
        assert_eq!(parsed["system"][0]["key"], "A");
        // 空分组被过滤，只留 warp-parse
        let mods = parsed["modules"].as_array().unwrap();
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0]["module"], "warp-parse");
        assert_eq!(mods[0]["changes"][0]["key"], "CPU");
    }

    #[test]
    fn empty_rows_render_header_only() {
        let table = render_table(&[], false);
        assert_eq!(table.lines().count(), 2);
    }

    #[test]
    fn snapshot_of_missing_root_is_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let missing = tmp.path().join("nope");
        assert!(snapshot_tree(&missing).is_empty());
    }

    #[test]
    fn diff_files_reports_created_and_replaced_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.yml"), "1").unwrap();
        std::fs::write(root.join("sub/b.yml"), "2").unwrap();
        std::fs::write(root.join("gone.yml"), "x").unwrap();
        let before = snapshot_tree(root);

        // 改 a、删 gone、新增 c（含子目录）
        std::fs::write(root.join("a.yml"), "1-changed").unwrap();
        std::fs::remove_file(root.join("gone.yml")).unwrap();
        std::fs::write(root.join("sub/c.yml"), "3").unwrap();
        let after = snapshot_tree(root);

        let rows = diff_files(&before, &after);
        let by: std::collections::BTreeMap<&str, FileState> =
            rows.iter().map(|r| (r.path(), r.state())).collect();
        assert_eq!(by.get("a.yml"), Some(&FileState::Replaced));
        assert_eq!(by.get("sub/c.yml"), Some(&FileState::Created));
        // 内容未变的文件不报；被删除的文件不报
        assert!(!by.contains_key("sub/b.yml"));
        assert!(!by.contains_key("gone.yml"));
        assert_eq!(rows.len(), 2);
        // 输出按路径排序
        assert_eq!(rows[0].path(), "a.yml");
        assert_eq!(rows[1].path(), "sub/c.yml");
    }

    #[test]
    fn identical_tree_produces_no_rows() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a"), "same").unwrap();
        let before = snapshot_tree(tmp.path());
        let after = snapshot_tree(tmp.path());
        assert!(diff_files(&before, &after).is_empty());
    }

    #[test]
    fn file_table_has_header_and_colors() {
        let rows = vec![
            FileRow {
                path: "local/a.yml".into(),
                state: FileState::Created,
            },
            FileRow {
                path: "local/b.yml".into(),
                state: FileState::Replaced,
            },
        ];
        let plain = render_file_table(&rows, false);
        let lines: Vec<&str> = plain.lines().collect();
        assert!(lines[0].starts_with("FILE"));
        assert!(lines[0].contains("STATE"));
        assert!(!plain.contains('\x1b'));
        assert!(lines[2].contains("created"));
        assert!(lines[3].contains("replaced"));

        let colored = render_file_table(&rows, true);
        assert!(colored.contains("\x1b[32m"));
        assert!(colored.contains("\x1b[33m"));
    }

    #[test]
    fn files_json_has_path_and_state() {
        let rows = vec![FileRow {
            path: "local/a.yml".into(),
            state: FileState::Created,
        }];
        let parsed: serde_json::Value = serde_json::from_str(&render_files_json(&rows)).unwrap();
        assert_eq!(parsed[0]["path"], "local/a.yml");
        assert_eq!(parsed[0]["state"], "created");
    }
}
