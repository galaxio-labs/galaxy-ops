//! 升级事务的**执行记录**（P0）：一次 `prj upgrade` 一个目录。
//!
//! 为什么需要：`upgrade.json` 只有一句 `detail`（`resource error / exit status 1`），
//! 真正的输出散在 journald 里、和心跳混流、没有退出码/耗时/边界 —— "有日志"不等于"能定位"。
//!
//! 落点（与 `upgrade.json` 同级）：
//!
//! ```text
//! ~/.galaxy/upgrade/<prj>/
//!   upgrade.json                 # 事务结局（已有）
//!   runs/<YYYYmmdd-HHMMSS>/
//!     run.json                   # 本次是谁/为什么/从哪到哪
//!     events.jsonl               # 每步 start/end + 错误（append-only，崩溃也留得下）
//!     04-apply-wist-center-stack.log
//!     05-regenerate-wist-center-stack.log
//!     06-pull-….log
//! ```
//!
//! 三条约束：
//! 1. **有界**：单步日志 4 MiB 截断；`runs/` 只留最近 [`KEEP_RUNS`] 次；
//! 2. **脱敏在写入时**：环境里的 `*TOKEN*` / `*SECRET*` / `*PASSWORD*` 值，以及
//!    `Bearer xxx`、`token=xxx` 之类模式，一律替换成 `***`；
//! 3. **不影响主流程**：日志写失败只告警，不能让一次升级因为日志而失败。

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use regex::Regex;
use serde_json::json;

use crate::error::MainReason;
use crate::internal_prelude::*;

/// 保留最近多少次运行（`runs/` 下按名字排序，旧的删）。
const KEEP_RUNS: usize = 10;
/// 单个步骤日志上限。
const STEP_LOG_LIMIT: u64 = 4 * 1024 * 1024;
/// 错误里带出的尾部行数上限。
const TAIL_LINES: usize = 40;

/// 事务级元信息（写进 `run.json`，方便事后只看目录就知道"这是什么"）。
#[derive(Debug, Clone)]
pub struct RunMeta {
    pub project: String,
    pub systems: Vec<String>,
    pub target: String,
    pub on_failure: String,
    pub from_version: String,
}

struct StepFile {
    name: String,
    file: File,
    bytes: u64,
    started: Instant,
    truncated: bool,
}

struct Inner {
    step: Option<StepFile>,
}

/// 一次升级事务的执行记录。
///
/// 通过 [`scope`] 绑定到当前 tokio 任务；命令执行器（`run_and_stream`）用 [`current`]
/// 取到它，把子进程输出写进当前步骤的日志。
pub struct RunLog {
    dir: PathBuf,
    events: Mutex<File>,
    inner: Mutex<Inner>,
    next_seq: AtomicUsize,
    secrets: Vec<String>,
    bearer: Regex,
    key_value: Regex,
}

tokio::task_local! {
    static CURRENT: Arc<RunLog>;
}

/// 在本任务内绑定一份执行记录（在该 future 里 [`current`] 才能取到）。
pub async fn scope<F: std::future::Future>(log: Arc<RunLog>, fut: F) -> F::Output {
    CURRENT.scope(log, fut).await
}

/// 当前任务绑定的执行记录（`prj upgrade` 之外为 `None`）。
pub fn current() -> Option<Arc<RunLog>> {
    CURRENT.try_with(Arc::clone).ok()
}

impl RunLog {
    /// 新建一次运行目录并清理过期运行。
    ///
    /// `runs_root` = `record_path(<prj>).parent()/runs`（与 `upgrade.json` 同级）。
    pub fn create(runs_root: &Path, meta: &RunMeta) -> MainResult<Arc<Self>> {
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let dir = runs_root.join(&ts);
        std::fs::create_dir_all(&dir).map_err(|e| {
            MainReason::resource_detail(format!("创建运行目录失败 {}: {e}", dir.display()))
        })?;

        let events_path = dir.join("events.jsonl");
        let events = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&events_path)
            .map_err(|e| {
                MainReason::resource_detail(format!(
                    "创建事件日志失败 {}: {e}",
                    events_path.display()
                ))
            })?;

        let run_json = json!({
            "run_id": ts,
            "project": meta.project,
            "systems": meta.systems,
            "target": meta.target,
            "on_failure": meta.on_failure,
            "from_version": meta.from_version,
            "started_at": chrono::Utc::now().to_rfc3339(),
            "log_dir": dir.display().to_string(),
        });
        // run.json 写失败不阻断：events.jsonl 仍能说明发生了什么。
        if let Err(err) = std::fs::write(
            dir.join("run.json"),
            serde_json::to_string_pretty(&run_json).unwrap_or_default(),
        ) {
            eprintln!("[WARN] 写 run.json 失败 {}: {err}", dir.display());
        }

        let log = Arc::new(Self {
            dir,
            events: Mutex::new(events),
            inner: Mutex::new(Inner { step: None }),
            next_seq: AtomicUsize::new(1),
            secrets: collect_secret_values(),
            bearer: Regex::new(r"(?i)(bearer\s+)([A-Za-z0-9._~+/=-]{8,})").expect("regex"),
            key_value: Regex::new(
                r#"(?i)((?:token|secret|password|passwd|credential)s?\s*[=:]\s*)(["']?)([^\s"',}]{4,})"#,
            )
            .expect("regex"),
        });

        prune_old_runs(runs_root);
        Ok(log)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 进入一个步骤：关闭上一步日志、开新文件、记 `start` 事件。
    pub fn begin_step(&self, step: &str, sys: &str) {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let suffix = if sys.is_empty() {
            String::new()
        } else {
            format!("-{}", sys.replace(['/', '\\'], "_"))
        };
        let name = format!("{seq:02}-{step}{suffix}.log");
        let path = self.dir.join(&name);
        let file = match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => file,
            Err(err) => {
                eprintln!("[WARN] 创建步骤日志失败 {}: {err}", path.display());
                return;
            }
        };
        {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.step = Some(StepFile {
                name,
                file,
                bytes: 0,
                started: Instant::now(),
                truncated: false,
            });
        }
        self.event(step, sys, "start", None);
    }

    /// 结束当前步骤：记 `end` 事件（含耗时与错误摘要）。
    pub fn end_step(&self, step: &str, sys: &str, error: Option<&str>) {
        let (elapsed_ms, name) = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            match inner.step.take() {
                Some(step_file) => (
                    step_file.started.elapsed().as_millis() as u64,
                    step_file.name.clone(),
                ),
                None => (0, String::new()),
            }
        };
        self.event_with(step, sys, "end", error, Some(elapsed_ms), Some(&name));
    }

    /// 子进程的一行输出（已脱敏；超出上限后只记一次截断标记）。
    ///
    /// P0 不区分 stdout/stderr（两者本来就交错，行序更重要）；`_stream` 保留给后续标记。
    pub fn line(&self, _stream: &str, line: &str) {
        let redacted = self.redact(line);
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(step) = inner.step.as_mut() else {
            return;
        };
        if step.truncated {
            return;
        }
        let payload = format!("{redacted}\n");
        if step.bytes + payload.len() as u64 > STEP_LOG_LIMIT {
            let _ = writeln!(
                step.file,
                "… [truncated: 步骤日志超过 {} MiB]",
                STEP_LOG_LIMIT / 1024 / 1024
            );
            step.truncated = true;
            return;
        }
        if let Err(err) = step.file.write_all(payload.as_bytes()) {
            eprintln!("[WARN] 写步骤日志失败 {}: {err}", step.name);
            return;
        }
        step.bytes += payload.len() as u64;
    }

    /// 脱敏：先按值替换环境里的密钥，再处理 `Bearer xxx` / `token=xxx` 之类模式。
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.secrets {
            if !secret.is_empty() && out.contains(secret) {
                out = out.replace(secret, "***");
            }
        }
        out = self.bearer.replace_all(&out, "${1}***").into_owned();
        self.key_value
            .replace_all(&out, "${1}${2}***${2}")
            .into_owned()
    }

    fn event(&self, step: &str, sys: &str, phase: &str, error: Option<&str>) {
        self.event_with(step, sys, phase, error, None, None);
    }

    fn event_with(
        &self,
        step: &str,
        sys: &str,
        phase: &str,
        error: Option<&str>,
        duration_ms: Option<u64>,
        file: Option<&str>,
    ) {
        let mut value = json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "step": step,
            "sys": sys,
            "phase": phase,
        });
        if let Some(duration_ms) = duration_ms {
            value["duration_ms"] = json!(duration_ms);
        }
        if let Some(file) = file {
            value["file"] = json!(file);
        }
        if let Some(error) = error {
            value["error"] = json!(self.redact(error));
        }
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(err) = writeln!(events, "{value}") {
            eprintln!("[WARN] 写事件日志失败: {err}");
        }
    }
}

/// 错误里带出的"最后一屏"：有界、已脱敏。
#[derive(Default)]
pub struct Tail {
    lines: VecDeque<String>,
    bytes: usize,
}

impl Tail {
    /// 追加一行（保留最后 [`TAIL_LINES`] 行、至多 4 KiB）。
    pub fn push(&mut self, line: &str) {
        self.lines.push_back(line.to_string());
        self.bytes += line.len();
        while self.lines.len() > TAIL_LINES || self.bytes > 4096 {
            if let Some(front) = self.lines.pop_front() {
                self.bytes = self.bytes.saturating_sub(front.len());
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn render(&self) -> String {
        self.lines
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// 收集环境里看起来像密钥的值（只用于**替换输出中的明文**）。
fn collect_secret_values() -> Vec<String> {
    let marker =
        Regex::new(r"(?i)(token|secret|password|passwd|credential|api[_-]?key)").expect("regex");
    let mut values: Vec<String> = std::env::vars()
        .filter(|(key, _)| marker.is_match(key))
        .map(|(_, value)| value)
        .filter(|value| value.len() >= 8)
        .collect();
    // 长的先替换，避免短值把长值的片段先打碎。
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values.dedup();
    values
}

/// 只保留最近 [`KEEP_RUNS`] 次运行（按目录名排序，时间戳天然有序）。
fn prune_old_runs(runs_root: &Path) {
    let Ok(entries) = std::fs::read_dir(runs_root) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    let excess = dirs.len().saturating_sub(KEEP_RUNS);
    for dir in dirs.into_iter().take(excess) {
        if let Err(err) = std::fs::remove_dir_all(&dir) {
            eprintln!("[WARN] 清理旧运行目录失败 {}: {err}", dir.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> RunMeta {
        RunMeta {
            project: "p".into(),
            systems: vec!["s".into()],
            target: "v".into(),
            on_failure: "rollback-all".into(),
            from_version: "s=1".into(),
        }
    }

    #[test]
    fn records_steps_and_redacts_secrets() {
        let tmp = tempfile::tempdir().unwrap();
        let log = RunLog::create(&tmp.path().join("runs"), &meta()).unwrap();
        log.begin_step("apply", "demo");
        log.line("stderr", "Authorization: Bearer abcdefghijklmnop");
        log.line("stdout", "token=deadbeefcafe");
        log.line("stdout", "all good");
        log.end_step("apply", "demo", Some("token=deadbeefcafe"));

        let events = std::fs::read_to_string(log.dir().join("events.jsonl")).unwrap();
        assert!(events.contains("\"phase\":\"start\""), "{events}");
        assert!(events.contains("\"phase\":\"end\""), "{events}");
        assert!(events.contains("\"duration_ms\""), "{events}");
        assert!(
            !events.contains("deadbeefcafe"),
            "事件里的错误也要脱敏：{events}"
        );

        let step = std::fs::read_to_string(log.dir().join("01-apply-demo.log")).unwrap();
        assert!(step.contains("all good"));
        assert!(!step.contains("abcdefghijklmnop"), "Bearer 要脱敏：{step}");
        assert!(!step.contains("deadbeefcafe"), "key=value 要脱敏：{step}");
        assert!(step.contains("***"));
        assert!(log.dir().join("run.json").exists());
    }

    #[test]
    fn keeps_only_the_latest_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("runs");
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..(KEEP_RUNS + 3) {
            std::fs::create_dir_all(root.join(format!("202601010000{i:02}"))).unwrap();
        }
        prune_old_runs(&root);
        let count = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().is_dir())
            .count();
        assert_eq!(count, KEEP_RUNS);
    }

    #[test]
    fn tail_is_bounded() {
        let mut tail = Tail::default();
        for i in 0..200 {
            tail.push(&format!("line {i}"));
        }
        assert!(tail.render().lines().count() <= TAIL_LINES);
        assert!(tail.render().contains("line 199"));
    }
}
