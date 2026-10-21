//! `gops run diagnose`：运行时体检。
//!
//! 设计见 `docs/design/run-diagnose.md`。分工：
//! - 本模块（gops 侧）：采集事实（[`context`]）、通用检查（[`checks`]）、报告与退出码（[`report`]）；
//! - 系统的 `diagnose` 流程（`sys/workflows/operators.gxl`）：本系统专属的**期望**，
//!   经 [`Bundle`] 交接的 JSON 契约（§5 / §6）与本模块对话。
//!
//! 关键约束：`diagnose` **只读** —— 不 `cp`、不 `chmod`、不重启；修复建议只以 `hint` 呈现。

pub mod checks;
pub mod context;
pub mod report;

use std::path::{Path, PathBuf};

use crate::error::{MainReason, MainResult};

pub use checks::DEFAULT_LOG_LINES;
pub use context::{Context, SystemInfo};
pub use report::{Check, CheckStatus, Report};

/// 传给流程的环境变量名（契约 §5）。
pub const ENV_CONTEXT: &str = "GOPS_DIAGNOSE_CONTEXT";
pub const ENV_RESULT: &str = "GOPS_DIAGNOSE_RESULT";
pub const ENV_ROOT: &str = "GOPS_SYSTEM_ROOT";
pub const ENV_NAME: &str = "GOPS_SYSTEM_NAME";
pub const ENV_KIND: &str = "GOPS_SYSTEM_KIND";
pub const ENV_ENV: &str = "GOPS_DIAGNOSE_ENV";

/// `diagnose` 流程名（`gx run diagnose`）。
pub const FLOW_NAME: &str = "diagnose";

/// 一次体检的选项（CLI 直接映射）。
#[derive(Clone, Debug)]
pub struct DiagnoseOptions {
    /// 抓多少行容器日志（`0` = 不抓）。
    pub log_lines: usize,
    /// 日志里包含这些子串的行不当错误线索（站点自行屏蔽已知噪声，大小写不敏感）。
    pub log_ignore: Vec<String>,
    /// 流程超时（秒）。
    pub timeout_secs: u64,
    /// `warn` 是否也算失败（`--fail-on warn`）。
    pub fail_on_warn: bool,
    /// 跳过系统的 `diagnose` 流程（`--no-ext`）。
    pub no_ext: bool,
    /// `--json` 里是否带完整上下文。
    pub include_context: bool,
    /// 保留临时目录（排障用）。
    pub keep_context: bool,
    pub env: String,
}

impl Default for DiagnoseOptions {
    fn default() -> Self {
        Self {
            log_lines: DEFAULT_LOG_LINES,
            log_ignore: Vec::new(),
            timeout_secs: 60,
            fail_on_warn: false,
            no_ext: false,
            include_context: false,
            keep_context: false,
            env: "default".to_string(),
        }
    }
}

/// 一次诊断的交接目录：`context.json`（gops → 流程）+ `result.json`（流程 → gops）。
///
/// 目录权限 `0700`、文件 `0600`：上下文里可能有渲染后的配置（密钥已掩码，仍按敏感处理）。
/// 运行结束删除，`--keep-context` 时保留并在 stderr 打印路径。
pub struct Bundle {
    dir: PathBuf,
    keep: bool,
}

impl Bundle {
    /// 建目录并写入 `context.json`。
    pub fn create(context: &Context, keep: bool) -> MainResult<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        // 同进程内并发（测试 / 并行诊断）也必须各不相同：时间戳精度不够就补序号。
        let dir = std::env::temp_dir().join(format!(
            "gops-diagnose-{}-{}-{}",
            std::process::id(),
            chrono::Local::now().format("%Y%m%d%H%M%S%3f"),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).map_err(|err| {
            MainReason::logic_detail(format!("无法创建诊断临时目录 {}：{err}", dir.display()))
        })?;
        restrict(&dir, true)?;

        let bundle = Self { dir, keep };
        let text = serde_json::to_string_pretty(&context.to_json()).unwrap_or_else(|_| "{}".into());
        bundle.write(&bundle.context_path(), &text)?;
        Ok(bundle)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn context_path(&self) -> PathBuf {
        self.dir.join("context.json")
    }

    pub fn result_path(&self) -> PathBuf {
        self.dir.join("result.json")
    }

    /// 注入流程的环境变量（契约 §5）。
    pub fn env_pairs(&self, context: &Context) -> Vec<(String, String)> {
        vec![
            (
                ENV_CONTEXT.to_string(),
                self.context_path().display().to_string(),
            ),
            (
                ENV_RESULT.to_string(),
                self.result_path().display().to_string(),
            ),
            (ENV_ROOT.to_string(), context.system.root.clone()),
            (ENV_NAME.to_string(), context.system.name.clone()),
            (ENV_KIND.to_string(), context.system.kind.clone()),
            (ENV_ENV.to_string(), context.system.env.clone()),
        ]
    }

    /// 读流程交回的结果；没写返回 `None`。
    pub fn read_result(&self) -> MainResult<Option<String>> {
        match std::fs::read_to_string(self.result_path()) {
            Ok(text) => Ok(Some(text)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(MainReason::logic_detail(format!(
                "读取 {} 失败：{err}",
                self.result_path().display()
            ))),
        }
    }

    /// 运行结束的收尾：默认删除（由 [`Drop`] 完成）；`keep` 时把路径打到 stderr
    /// （stdout 留给 `--json`）。
    ///
    /// 用 `Drop` 而不是只靠这个函数：中间任何早退（`?` / `exit`）也不会把临时目录留在 `/tmp`。
    pub fn finish(self) {
        if self.keep {
            eprintln!("diagnose context kept: {}", self.dir.display());
        }
    }

    fn write(&self, path: &Path, text: &str) -> MainResult<()> {
        // 先写临时名再 rename：流程永远读不到半个 JSON。
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|err| {
            MainReason::logic_detail(format!("写入 {} 失败：{err}", tmp.display()))
        })?;
        restrict(&tmp, false)?;
        std::fs::rename(&tmp, path).map_err(|err| {
            MainReason::logic_detail(format!("落盘 {} 失败：{err}", path.display()))
        })?;
        Ok(())
    }
}

/// 目录的默认归宿是删除；`--keep-context` 时保留（排障需要 `context.json` / `result.json`）。
impl Drop for Bundle {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// `dir = true` 时设目录权限（0700），否则设文件权限（0600）。
#[cfg(unix)]
fn restrict(path: &Path, dir: bool) -> MainResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if dir { 0o700 } else { 0o600 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|err| MainReason::logic_detail(format!("设置 {} 权限失败：{err}", path.display())))
}

#[cfg(not(unix))]
fn restrict(_path: &Path, _dir: bool) -> MainResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        Context::minimal(SystemInfo {
            name: "stack".into(),
            kind: "docker-compose".into(),
            root: "/srv/stack".into(),
            env: "default".into(),
        })
    }

    #[test]
    fn bundle_writes_context_and_exposes_the_contract_env() {
        let bundle = Bundle::create(&context(), false).unwrap();
        let text = std::fs::read_to_string(bundle.context_path()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema_version"], "1");

        let pairs = bundle.env_pairs(&context());
        let keys: Vec<&str> = pairs.iter().map(|(key, _)| key.as_str()).collect();
        for expected in [
            ENV_CONTEXT,
            ENV_RESULT,
            ENV_ROOT,
            ENV_NAME,
            ENV_KIND,
            ENV_ENV,
        ] {
            assert!(keys.contains(&expected), "缺 {expected}");
        }
        bundle.finish();
    }

    #[test]
    fn missing_result_is_none_not_an_error() {
        let bundle = Bundle::create(&context(), false).unwrap();
        assert!(bundle.read_result().unwrap().is_none());
        bundle.finish();
    }

    #[test]
    fn result_is_read_back_verbatim() {
        let bundle = Bundle::create(&context(), false).unwrap();
        std::fs::write(bundle.result_path(), r#"{"checks":[]}"#).unwrap();
        assert_eq!(
            bundle.read_result().unwrap().as_deref(),
            Some(r#"{"checks":[]}"#)
        );
        bundle.finish();
    }

    #[cfg(unix)]
    #[test]
    fn files_and_dir_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let bundle = Bundle::create(&context(), false).unwrap();
        let dir_mode = std::fs::metadata(bundle.dir())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let file_mode = std::fs::metadata(bundle.context_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
        bundle.finish();
    }

    #[test]
    fn dropping_the_bundle_removes_the_directory() {
        let dir = {
            let bundle = Bundle::create(&context(), false).unwrap();
            bundle.dir().to_path_buf()
        };
        assert!(!dir.exists(), "未 keep 时必须清理临时目录");
    }

    #[test]
    fn keeping_the_bundle_leaves_the_directory_behind() {
        let dir = {
            let bundle = Bundle::create(&context(), true).unwrap();
            bundle.dir().to_path_buf()
        };
        assert!(dir.exists(), "--keep-context 时必须保留");
        assert!(dir.join("context.json").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn early_return_also_cleans_up() {
        // 模拟中间出错：只 create、不 finish 就 drop。
        let dir = Bundle::create(&context(), false)
            .unwrap()
            .dir()
            .to_path_buf();
        assert!(!dir.exists());
    }
}
