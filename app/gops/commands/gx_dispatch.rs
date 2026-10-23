//! 调用外部 `gx`（galaxy-flow）的共用基础设施：二进制定位、版本校验、流程执行与子进程输出转发。
//!
//! 供 `gops sys`（`localize` 的可选阶段流程）与 `gops run`（算子流分派）共用，
//! 保证「如何调 gx」只有一处。

use std::path::Path;
use std::process::Command;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command as TokioCommand;

use galaxy_ops::error::MainResult;
use galaxy_ops::prelude::ErrorOwe;

/// 外部执行器 `gx`（galaxy-flow）的最低版本要求。
pub(crate) const GX_MIN_VERSION: (u32, u32, u32) = (0, 13, 0);

/// `gx` 可执行文件路径（`$HOME/bin/gx`）。
pub(crate) fn gx_bin_path() -> String {
    gx_bin_path_from(std::env::var_os("HOME"), std::env::var_os("PATH"))
}

/// 解析 `gx` 的可执行路径。
///
/// 顺序：`$HOME/bin/gx`（**存在才算**）→ `PATH` 里的 `gx` → 字面量 `gx`（交给系统再查一次）。
///
/// **为什么不能只拼 `$HOME/bin/gx`**：systemd 服务（如 `wist-gwlinkd.service`）默认
/// **不设 `HOME`**，`$HOME` 为空时会拼出 `/bin/gx` —— 一个不存在却看起来合理的路径，
/// 于是 `gx run --exists` 失败、localize 阶段流程被静默跳过
/// （2026-10-20 的网关升级事故：代码升级了，但配置没重渲、admin token 没生成）。
fn gx_bin_path_from(home: Option<std::ffi::OsString>, path: Option<std::ffi::OsString>) -> String {
    if let Some(home) = home.filter(|h| !h.is_empty()) {
        let candidate = std::path::Path::new(&home).join("bin").join("gx");
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    if let Some(path) = path {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("gx");
            if candidate.is_file() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    "gx".to_string()
}

pub(crate) fn check_gx_version() -> MainResult<()> {
    // 检查 gx 版本
    let gx_path = gx_bin_path();
    let output = Command::new(&gx_path)
        .arg("-V")
        .output()
        .map_err(|e| format!("无法执行 gx 命令 ({gx_path}): {e}"))
        .source_resource()?;

    if !output.status.success() {
        return Err("gx 命令执行失败").source_resource()?;
    }

    let version_str = String::from_utf8_lossy(&output.stdout);
    // 解析版本号，假设输出格式为 "gx x.y.z"
    let version_parts: Vec<&str> = version_str.split_whitespace().collect();
    if version_parts.len() < 2 {
        return Err(format!("无法解析 gx 版本: {version_str}")).source_resource()?;
    }

    let version = version_parts[1];
    let version_parts: Vec<&str> = version.split('.').collect();
    if version_parts.len() < 3 {
        return Err(format!("无效的 gx 版本格式: {version}")).source_resource()?;
    }

    // 解析主版本、次版本和修订版本
    let major: u32 = version_parts[0]
        .parse()
        .map_err(|_| format!("无效的主版本号: {}", version_parts[0]))
        .source_resource()?;
    let minor: u32 = version_parts[1]
        .parse()
        .map_err(|_| format!("无效的次版本号: {}", version_parts[1]))
        .source_resource()?;
    let patch: u32 = version_parts[2]
        .parse()
        .map_err(|_| format!("无效的修订版本号: {}", version_parts[2]))
        .source_resource()?;

    // 检查版本是否 >= GX_MIN_VERSION
    let (min_major, min_minor, min_patch) = GX_MIN_VERSION;
    if (major, minor, patch) >= (min_major, min_minor, min_patch) {
        Ok(())
    } else {
        Err(format!(
            "gx 版本过低，需要 >= {min_major}.{min_minor}.{min_patch}，当前版本: {version}"
        ))
        .source_resource()?
    }
}

/// 构造 `gx run` 的参数（不含可执行文件路径）。
///
/// 映射：`gops run <cmd> [-e ENV] [-d N] [--mod M]` -> `gx run -e ENV -d N [--cmd-arg M] <cmd>`
pub(crate) fn gx_run_args(
    cmd_name: &str,
    env: &str,
    debug: usize,
    module: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        "-e".to_string(),
        env.to_string(),
        "-d".to_string(),
        debug.to_string(),
    ];
    if let Some(module) = module {
        args.push("--cmd-arg".to_string());
        args.push(module.to_string());
    }
    args.push(cmd_name.to_string());
    args
}

/// 为 `gx` 子进程补 `PATH`：把**当前 gops 可执行文件所在目录**（以及调用它的目录、`gx`
/// 所在目录）前置。
///
/// 为什么要补：`gx.cmd` 的内容由 `gx` 原样交给 `sh -c`，流程里普遍按裸名字调用 gops
/// （例如 `gops sys perms --apply`）。而 `gx` 继承的是**发起 gops 那个环境**的 `PATH` ——
/// 交互式 shell 里 gops 常在 `~/bin`、`/root/bin`，非交互场景（gwlinkd / systemd / cron
/// 触发的升级）`PATH` 往往只有系统目录，裸调用会「找不到命令」退 127，整次升级被判定失败
/// 并回滚（2026-10-22 的网关升级事故）。
///
/// 为什么要两个来源：
///   · `current_exe()`：自更新把二进制装在**当前 exe 目录**且名字固定为 `gops`，最权威；
///   · `argv[0]` 的目录：手动软链到**改过名**的二进制时（`/usr/local/bin/gops ->
///     /opt/gops/gops-2.10.1`），exe 目录里并没有叫 `gops` 的文件，只有软链所在目录有。
///
/// 只**前置**不替换：原 `PATH` 全部保留在后面，不影响流程里的其它命令。
///
/// 代价（已知且接受）：gops 目录里如果同时放了别的同名命令（如 `docker`/`python3` 的
/// shim），流程里会优先用到它们。这与「交互式 shell 把 `~/bin` 放在 PATH 前面」一致，
/// 而正确的 gops 版本比避开这一点更重要（版本错配时 `sys perms` 一类子命令会直接不可用）。
fn apply_gx_child_path(cmd: &mut TokioCommand, gx_path: &str) {
    let base = std::env::var_os("PATH");
    apply_gx_child_path_with(
        cmd,
        gx_path,
        current_exe_dir(),
        argv0_dir(),
        base.as_deref(),
    );
}

/// 真正组装并写入 `PATH` 的那一步：候选来源作为参数传入，便于用合成值单测。
fn apply_gx_child_path_with(
    cmd: &mut TokioCommand,
    gx_path: &str,
    exe_dir: Option<std::path::PathBuf>,
    argv0_dir: Option<std::path::PathBuf>,
    base: Option<&std::ffi::OsStr>,
) {
    let gops_dirs = gops_search_dirs(exe_dir, argv0_dir);
    // 拼不出合法 PATH（目录名里带分隔符这类怪情况）就**不动**，交给子进程继承：
    // 写一个空 PATH 比不写更糟，会把流程里的 sh/docker 一起弄死。
    if let Some(path) = gx_child_path(&gops_dirs, gx_path, base) {
        cmd.env("PATH", path);
    }
}

/// 当前 gops 可执行文件所在目录。
fn current_exe_dir() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// `argv[0]` 所在目录（裸名字时为 `None` / 相对路径后续会被过滤）。
fn argv0_dir() -> Option<std::path::PathBuf> {
    std::env::args_os()
        .next()
        .map(std::path::PathBuf::from)
        .and_then(|argv0| argv0.parent().map(Path::to_path_buf))
}

/// 流程里要前置的 gops 目录：**exe 目录 → `argv[0]` 目录**。
///
/// 顺序有意义：`current_exe()` 最权威（自更新把二进制装在当前 exe 目录、名字固定 `gops`）；
/// `argv[0]` 目录只在「软链到改过名的二进制」时才补位。
fn gops_search_dirs(
    exe_dir: Option<std::path::PathBuf>,
    argv0_dir: Option<std::path::PathBuf>,
) -> Vec<std::path::PathBuf> {
    [exe_dir, argv0_dir].into_iter().flatten().collect()
}

/// [`apply_gx_child_path`] 的纯函数部分（便于单测）：
/// `gops_dirs`（exe 目录、argv[0] 目录）→ `gx` 目录 → 原 `PATH`，去重。
///
/// 只收**绝对路径**：相对目录进 `PATH` 后会按**流程 cwd** 解析，比不补更危险。
/// 返回 `None` = 「按原样继承」，调用方就不要设 `PATH`。
fn gx_child_path(
    gops_dirs: &[std::path::PathBuf],
    gx_path: &str,
    base: Option<&std::ffi::OsStr>,
) -> Option<std::ffi::OsString> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    let mut candidates: Vec<std::path::PathBuf> = gops_dirs.to_vec();
    // `gx_path` 是裸名字时 parent 为空串：交给下面的绝对路径过滤跳过。
    if let Some(parent) = Path::new(gx_path).parent() {
        candidates.push(parent.to_path_buf());
    }
    for dir in candidates {
        if dir.is_absolute() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    if let Some(base) = base {
        for dir in std::env::split_paths(base) {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    std::env::join_paths(dirs).ok()
}

/// 统一的 gx 流程执行入口：`gx run -e <env> -d <n> [--cmd-arg <mod>] <flow>`，
/// 并可按需把 `inject_env` 注入子进程环境。
///
/// `gops run` 的算子分派与 `docker-compose` 的可选阶段流程都走这里。
/// （GXL 内部的 `gx.run` 是另一种东西，gops 不直接使用。）
pub(crate) async fn run_gx_flow(
    gx_path: &str,
    env: &str,
    debug: usize,
    module: Option<&str>,
    flow: &str,
    inject_env: &[(String, String)],
    cwd: Option<&Path>,
) -> MainResult<()> {
    let mut cmd = TokioCommand::new(gx_path);
    cmd.args(gx_run_args(flow, env, debug, module));
    apply_gx_child_path(&mut cmd, gx_path);
    // `gx` 按**工作目录**解析工程：目录可变时（如 `prj upgrade` 对某系统目录）必须显式设置，
    // 否则会拿进程 CWD 去解析，探测与流程都会落错地方。
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    for (key, value) in inject_env {
        // 保留变量不被合并配置覆盖，否则可能破坏 gx 自身或其 shell（如 PATH/HOME）。
        if is_reserved_env(key) {
            continue;
        }
        cmd.env(key, value);
    }
    run_and_stream(cmd, "gx").await
}

/// 启动子进程并转发 stdout/stderr，非零退出返回错误。
///
/// 若当前任务绑定了 [`galaxy_ops::ops_prj::run_log`]（`prj upgrade` 的事务），
/// 同时把每行写进当前步骤日志；失败时错误里带**最后 40 行**（已脱敏）——
/// 不再只报“退出状态 1”。
pub(crate) async fn run_and_stream(mut cmd: TokioCommand, label: &str) -> MainResult<()> {
    use galaxy_ops::ops_prj::run_log::{self, Tail};
    use std::sync::{Arc, Mutex};

    let log = run_log::current();
    let tail = Arc::new(Mutex::new(Tail::default()));

    // 设置管道并启动进程
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("无法启动 {label} 命令: {}", e))
        .source_resource()?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("无法获取stdout"))
        .source_resource()?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("无法获取stderr"))
        .source_resource()?;

    // 创建异步读取器并并发处理stdout和stderr
    let stdout_handle = {
        let log = log.clone();
        let tail = Arc::clone(&tail);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                println!("{}", line);
                if let Some(log) = &log {
                    log.line("stdout", &line);
                }
                tail.lock().unwrap_or_else(|e| e.into_inner()).push(&line);
            }
        })
    };

    let stderr_handle = {
        let log = log.clone();
        let tail = Arc::clone(&tail);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                eprintln!("{}", line);
                if let Some(log) = &log {
                    log.line("stderr", &line);
                }
                tail.lock().unwrap_or_else(|e| e.into_inner()).push(&line);
            }
        })
    };

    // 等待输出处理完成
    let _ = tokio::try_join!(stdout_handle, stderr_handle);

    // 等待子进程完成并检查退出状态
    let exit_status = child
        .wait()
        .await
        .map_err(|e| anyhow::anyhow!("等待子进程失败: {}", e))
        .source_resource()?;

    if !exit_status.success() {
        let tail = tail.lock().unwrap_or_else(|e| e.into_inner()).render();
        let mut detail = format!("{label} 命令失败，退出状态: {exit_status}");
        if !tail.is_empty() {
            detail.push_str(&format!("；最后几行：\n{tail}"));
        }
        if let Some(log) = &log {
            detail = log.redact(&detail);
        }
        return Err(anyhow::anyhow!(detail)).source_resource()?;
    }

    Ok(())
}

/// 注入 gx 子进程时**保留**的环境变量：不被合并配置覆盖，否则可能破坏 gx 自身
/// （`~/.galaxy` 解析、动态库加载、`gx.*` 内部变量）或其 shell 的 `PATH`。
fn is_reserved_env(key: &str) -> bool {
    const RESERVED: &[&str] = &[
        "PATH", "HOME", "PWD", "OLDPWD", "SHELL", "TMPDIR", "USER", "LOGNAME", "LANG", "LC_ALL",
    ];
    const RESERVED_PREFIXES: &[&str] = &["LD_", "DYLD_", "GX_", "GXL_"];
    RESERVED.contains(&key) || RESERVED_PREFIXES.iter().any(|p| key.starts_with(p))
}

// ─────────────────────────── 诊断流程（gops run diagnose）专用 ───────────────────────────

/// `gx run --exists <flow>` 的探测结果。
///
/// 三态必须分开：`Absent` 是「没有这个流程」（compose 项目的正常情况），
/// `Unavailable` 是「gx 根本跑不起来」——后者若被当成前者，会静默跳过本该执行的流程。
pub(crate) enum FlowProbe {
    Exists,
    Absent(String),
    Unavailable(String),
}

/// 探测项目里是否有某个流程（`gx run --exists`）。
pub(crate) fn probe_flow(gx_path: &str, flow: &str, cwd: &Path) -> FlowProbe {
    match Command::new(gx_path).arg("-V").output() {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return FlowProbe::Unavailable(if stderr.is_empty() {
                format!("`{gx_path} -V` 退出码 {:?}", output.status.code())
            } else {
                format!("`{gx_path} -V` 失败：{stderr}")
            });
        }
        Err(err) => return FlowProbe::Unavailable(format!("无法执行 {gx_path}：{err}")),
    }
    match Command::new(gx_path)
        .args(["run", "--exists", flow])
        .current_dir(cwd)
        .output()
    {
        Ok(output) if output.status.success() => FlowProbe::Exists,
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            FlowProbe::Absent(if stderr.is_empty() {
                format!("gx --exists 退出码 {:?}", output.status.code())
            } else {
                stderr
            })
        }
        Err(err) => FlowProbe::Unavailable(format!("无法执行 gx：{err}")),
    }
}

/// 跑一个流程并把它的输出**全部收进 stderr**（stdout 留给 `gops run diagnose --json`）。
///
/// 带超时：超时先 `kill` 再返回错误 —— 体检不能卡在某个流程上。
/// 输出只记最后 40 行进错误里（与 [`run_and_stream`] 同口径），不把整段日志塞进报告。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_gx_flow_capture(
    gx_path: &str,
    env: &str,
    debug: usize,
    flow: &str,
    module: Option<&str>,
    inject_env: &[(String, String)],
    cwd: &Path,
    timeout: std::time::Duration,
) -> MainResult<()> {
    use std::sync::{Arc, Mutex};

    let mut cmd = TokioCommand::new(gx_path);
    cmd.args(gx_run_args(flow, env, debug, module))
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    apply_gx_child_path(&mut cmd, gx_path);
    for (key, value) in inject_env {
        if is_reserved_env(key) {
            continue;
        }
        cmd.env(key, value);
    }

    let mut child = cmd
        .spawn()
        .map_err(|err| anyhow::anyhow!("无法启动 gx 流程 `{flow}`：{err}"))
        .source_resource()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("无法获取 gx stdout"))
        .source_resource()?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("无法获取 gx stderr"))
        .source_resource()?;

    let tail = Arc::new(Mutex::new(Vec::<String>::new()));
    let stdout_task = {
        let tail = Arc::clone(&tail);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                eprintln!("{line}");
                push_tail(&tail, &line);
            }
        })
    };
    let stderr_task = {
        let tail = Arc::clone(&tail);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                eprintln!("{line}");
                push_tail(&tail, &line);
            }
        })
    };

    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(err)) => {
            let _ = child.kill().await;
            let _ = tokio::join!(stdout_task, stderr_task);
            return Err(anyhow::anyhow!("等待 gx 流程 `{flow}` 失败：{err}")).source_resource()?;
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = tokio::join!(stdout_task, stderr_task);
            return Err(anyhow::anyhow!(
                "gx 流程 `{flow}` 超时（{}s）已被终止",
                timeout.as_secs()
            ))
            .source_resource()?;
        }
    };
    let _ = tokio::join!(stdout_task, stderr_task);

    if status.success() {
        return Ok(());
    }
    let lines = tail.lock().map(|lines| lines.clone()).unwrap_or_default();
    let mut detail = format!("gx 流程 `{flow}` 失败，退出状态: {status}");
    if !lines.is_empty() {
        detail.push_str("；最后几行：\n");
        detail.push_str(&lines.join("\n"));
    }
    Err(anyhow::anyhow!(detail)).source_resource()?
}

/// 只留最后 40 行（与 `run_and_stream` 的错误口径一致）。
fn push_tail(tail: &std::sync::Arc<std::sync::Mutex<Vec<String>>>, line: &str) {
    const MAX: usize = 40;
    let mut guard = tail.lock().unwrap_or_else(|err| err.into_inner());
    guard.push(line.to_string());
    if guard.len() > MAX {
        let overflow = guard.len() - MAX;
        guard.drain(0..overflow);
    }
}

#[cfg(test)]
mod tests {
    use super::FlowProbe;
    use super::{
        apply_gx_child_path, apply_gx_child_path_with, gops_search_dirs, gx_bin_path_from,
        gx_child_path, gx_run_args, is_reserved_env, probe_flow, run_gx_flow_capture,
    };

    #[cfg(unix)]
    fn fake_gx(dir: &std::path::Path, body: &str) -> String {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-gx");
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "#!/bin/sh").unwrap();
        write!(file, "{body}").unwrap();
        drop(file);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    #[cfg(unix)]
    #[test]
    fn probe_flow_distinguishes_absent_from_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        // `-V` 成功，`run --exists` 失败 = 没有这个流程
        let gx = fake_gx(
            dir.path(),
            "if [ \"$1\" = \"-V\" ]; then exit 0; fi\n\
             if [ \"$1\" = \"run\" ] && [ \"$2\" = \"--exists\" ]; then echo 'no such flow' >&2; exit 1; fi\n\
             exit 0\n",
        );
        assert!(matches!(
            probe_flow(&gx, "diagnose", dir.path()),
            FlowProbe::Absent(_)
        ));

        // `-V` 失败 = gx 不可用（与「没有流程」不同）
        let other = tempfile::tempdir().unwrap();
        let broken = fake_gx(other.path(), "echo boom >&2\nexit 3\n");
        assert!(matches!(
            probe_flow(&broken, "diagnose", dir.path()),
            FlowProbe::Unavailable(_)
        ));

        // 全成功 = 存在
        let ok = fake_gx(dir.path(), "exit 0\n");
        assert!(matches!(
            probe_flow(&ok, "diagnose", dir.path()),
            FlowProbe::Exists
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn capture_reports_a_failing_flow_with_its_tail() {
        let dir = tempfile::tempdir().unwrap();
        let gx = fake_gx(dir.path(), "echo 'something broke' >&2\nexit 7\n");
        let err = run_gx_flow_capture(
            &gx,
            "default",
            0,
            "diagnose",
            None,
            &[],
            dir.path(),
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        let text = format!("{err}");
        assert!(text.contains("失败"), "{text}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn capture_times_out_and_kills_the_flow() {
        let dir = tempfile::tempdir().unwrap();
        let gx = fake_gx(dir.path(), "sleep 5\n");
        let err = run_gx_flow_capture(
            &gx,
            "default",
            0,
            "diagnose",
            None,
            &[],
            dir.path(),
            std::time::Duration::from_millis(200),
        )
        .await
        .unwrap_err();
        assert!(format!("{err}").contains("超时"), "{err}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn capture_passes_injected_env_to_the_flow() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("seen.txt");
        let gx = fake_gx(
            dir.path(),
            &format!(
                "printf '%s' \"$GOPS_DIAGNOSE_RESULT\" > {}\n",
                marker.display()
            ),
        );
        run_gx_flow_capture(
            &gx,
            "default",
            0,
            "diagnose",
            None,
            &[(
                "GOPS_DIAGNOSE_RESULT".to_string(),
                "/tmp/r.json".to_string(),
            )],
            dir.path(),
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "/tmp/r.json");
    }

    #[test]
    fn test_gx_run_args_basic() {
        // gops run start  ->  gx run -e default -d 0 start
        let args = gx_run_args("start", "default", 0, None);
        assert_eq!(args.join(" "), "run -e default -d 0 start");
    }

    #[test]
    fn test_gx_run_args_with_module_and_debug() {
        // gops run stop --mod nginx -e prod -d 2
        //   ->  gx run -e prod -d 2 --cmd-arg nginx stop
        let args = gx_run_args("stop", "prod", 2, Some("nginx"));
        assert_eq!(args.join(" "), "run -e prod -d 2 --cmd-arg nginx stop");
        // 流程名始终在末尾
        assert_eq!(args.last().map(String::as_str), Some("stop"));
    }

    #[test]
    fn test_gx_run_args_covers_all_dispatch_commands() {
        for cmd in [
            "download",
            "install",
            "uninstall",
            "start",
            "stop",
            "status",
            "diagnose",
        ] {
            let args = gx_run_args(cmd, "default", 0, None);
            assert_eq!(args.first().map(String::as_str), Some("run"));
            assert_eq!(args.last().map(String::as_str), Some(cmd));
        }
    }

    #[test]
    fn test_reserved_env_filter() {
        // 关键系统变量/内部前缀不得被合并配置覆盖
        for k in [
            "PATH",
            "HOME",
            "PWD",
            "LD_PRELOAD",
            "DYLD_LIBRARY_PATH",
            "GX_FOO",
            "GXL_PRJ_ROOT",
        ] {
            assert!(is_reserved_env(k), "{k} should be reserved");
        }
        for k in ["DOMAIN", "HTTP_PORT", "NGINX_TAG"] {
            assert!(!is_reserved_env(k), "{k} should not be reserved");
        }
    }

    fn touch_gx(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn gx_prefers_home_bin_when_it_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let gx = tmp.path().join("bin/gx");
        touch_gx(&gx);
        let got = gx_bin_path_from(Some(tmp.path().as_os_str().to_owned()), None);
        assert_eq!(got, gx.to_string_lossy());
    }

    #[test]
    fn gx_falls_back_to_path_when_home_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("usr/bin");
        let gx = dir.join("gx");
        touch_gx(&gx);
        let got = gx_bin_path_from(None, Some(dir.as_os_str().to_owned()));
        assert_eq!(got, gx.to_string_lossy());
    }

    #[test]
    fn gx_never_invents_slash_bin_gx_for_empty_home() {
        let got = gx_bin_path_from(Some(std::ffi::OsString::new()), None);
        assert_eq!(got, "gx", "HOME 空时不能拼出 /bin/gx");
    }

    #[test]
    fn gx_child_path_prepends_gops_dir_and_keeps_base() {
        // 非交互场景（systemd/gwlinkd）：base 只有系统目录，必须把 gops/gx 目录前置。
        let base = std::ffi::OsString::from("/usr/bin:/bin");
        let got = gx_child_path(
            &[std::path::PathBuf::from("/root/bin")],
            "/root/bin/gx",
            Some(&base),
        );
        let dirs: Vec<_> = std::env::split_paths(&got.unwrap()).collect();
        assert_eq!(dirs[0], std::path::Path::new("/root/bin"));
        assert_eq!(dirs[1], std::path::Path::new("/usr/bin"));
        assert_eq!(dirs[2], std::path::Path::new("/bin"));
        assert_eq!(dirs.len(), 3, "gops 与 gx 同目录时只保留一份");
    }

    #[test]
    fn gx_child_path_orders_exe_then_argv0_then_gx() {
        // 软链到改过名的二进制：exe 目录里没有 `gops`，argv[0] 目录（软链所在处）有。
        let got = gx_child_path(
            &[
                std::path::PathBuf::from("/opt/gops"),
                std::path::PathBuf::from("/usr/local/bin"),
            ],
            "/home/u/bin/gx",
            None,
        );
        let dirs: Vec<_> = std::env::split_paths(&got.unwrap()).collect();
        assert_eq!(
            dirs,
            vec![
                std::path::PathBuf::from("/opt/gops"),
                std::path::PathBuf::from("/usr/local/bin"),
                std::path::PathBuf::from("/home/u/bin"),
            ]
        );
    }

    #[test]
    fn gx_child_path_keeps_gx_dir_when_it_differs() {
        let got = gx_child_path(
            &[std::path::PathBuf::from("/opt/gops/bin")],
            "/home/u/bin/gx",
            None,
        );
        let dirs: Vec<_> = std::env::split_paths(&got.unwrap()).collect();
        assert_eq!(
            dirs,
            vec![
                std::path::PathBuf::from("/opt/gops/bin"),
                std::path::PathBuf::from("/home/u/bin"),
            ]
        );
    }

    #[test]
    fn gx_child_path_skips_empty_parent_of_bare_name() {
        let got = gx_child_path(&[std::path::PathBuf::from("/root/bin")], "gx", None);
        let dirs: Vec<_> = std::env::split_paths(&got.unwrap()).collect();
        assert_eq!(dirs, vec![std::path::PathBuf::from("/root/bin")]);
    }

    #[cfg(unix)]
    #[test]
    fn gx_child_path_gives_up_when_dirs_are_not_joinable() {
        // 目录名里带 PATH 分隔符（unix 的 ':'）：拼不出合法 PATH 就返回 None，
        // 调用方宁可不设 PATH，也不能写一个空/残缺的 PATH 把 sh、docker 一起弄死。
        assert!(gx_child_path(&[std::path::PathBuf::from("/a:b")], "/a:b/gx", None).is_none());
    }

    #[test]
    fn gx_child_path_skips_relative_dirs() {
        // 相对目录进 PATH 会按**流程 cwd** 解析（如 ./bin/gops），比不补更危险。
        let got =
            gx_child_path(&[std::path::PathBuf::from("/root/bin")], "./bin/gx", None).unwrap();
        let dirs: Vec<_> = std::env::split_paths(&got).collect();
        assert_eq!(dirs, vec![std::path::PathBuf::from("/root/bin")]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn spawned_flow_sees_gops_dir_first_in_path() {
        // 端到端：真的 spawn 一个 gx（假脚本），让它把看到的 PATH 写下来。
        // 这条比「检查 Command 的 env」强：它穿过 run_gx_flow_capture 的完整路径。
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("path.txt");
        let gx = fake_gx(
            dir.path(),
            &format!("printf '%s' \"$PATH\" > {}\n", out.display()),
        );
        run_gx_flow_capture(
            &gx,
            "default",
            0,
            "diagnose",
            None,
            &[],
            dir.path(),
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap();

        let seen = std::fs::read_to_string(&out).unwrap();
        let dirs: Vec<_> = std::env::split_paths(&seen).collect();
        let exe_dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert_eq!(
            dirs.first().map(|d| d.to_path_buf()),
            Some(exe_dir),
            "流程里拿到的 PATH 应把 gops 目录放在最前：{seen}"
        );
        assert_eq!(
            dirs.get(1).map(|d| d.to_path_buf()),
            Some(dir.path().to_path_buf()),
            "gx 目录紧随其后：{seen}"
        );
    }

    #[test]
    fn apply_gx_child_path_sets_gops_dir_first() {
        // 守住接线：纯函数写对了，但忘了/写错在 run_gx_flow 里生效也白搭。
        let mut cmd = tokio::process::Command::new("/bin/sh");
        apply_gx_child_path(&mut cmd, "/bin/sh");

        let path = cmd
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == std::ffi::OsStr::new("PATH"))
            .and_then(|(_, value)| value.map(std::ffi::OsStr::to_os_string))
            .expect("PATH 应被显式设置");
        let dirs: Vec<_> = std::env::split_paths(&path).collect();

        let exe_dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert_eq!(dirs.first(), Some(&exe_dir), "gops 自身目录必须在最前面");

        // 原 PATH 必须一项不少地跟在后面（不假设它是哪个目录，免得在精简环境的 CI 上误报）。
        if let Some(base) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&base) {
                assert!(
                    dirs.contains(&dir),
                    "原 PATH 项丢了：{}（{path:?}）",
                    dir.display()
                );
            }
        }
    }

    #[test]
    fn gops_search_dirs_orders_exe_then_argv0() {
        let dirs = gops_search_dirs(
            Some(std::path::PathBuf::from("/opt/gops")),
            Some(std::path::PathBuf::from("/usr/local/bin")),
        );
        assert_eq!(
            dirs,
            vec![
                std::path::PathBuf::from("/opt/gops"),
                std::path::PathBuf::from("/usr/local/bin"),
            ],
            "exe 目录在前，argv[0] 目录补位"
        );
    }

    #[test]
    fn gops_search_dirs_tolerates_missing_sources() {
        assert!(gops_search_dirs(None, None).is_empty());
        assert_eq!(
            gops_search_dirs(None, Some(std::path::PathBuf::from("/usr/local/bin"))),
            vec![std::path::PathBuf::from("/usr/local/bin")],
            "取不到 exe 目录时不能把 argv[0] 目录一起丢了"
        );
        assert_eq!(
            gops_search_dirs(Some(std::path::PathBuf::from("/root/bin")), None),
            vec![std::path::PathBuf::from("/root/bin")]
        );
    }

    #[test]
    fn apply_gx_child_path_with_puts_both_sources_before_base() {
        // 用合成目录：exe 目录与 argv[0] 目录不同，才能看出两个都写进去了（真实进程里二者相同）。
        // 这条能钉住「有没有把 argv[0] 目录传给 gops_search_dirs」—— 删掉那一项会直接红。
        let mut cmd = tokio::process::Command::new("/bin/sh");
        let base = std::ffi::OsString::from("/usr/bin:/bin");
        apply_gx_child_path_with(
            &mut cmd,
            "/home/u/bin/gx",
            Some(std::path::PathBuf::from("/opt/gops")),
            Some(std::path::PathBuf::from("/usr/local/bin")),
            Some(&base),
        );

        let path = cmd
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == std::ffi::OsStr::new("PATH"))
            .and_then(|(_, value)| value.map(std::ffi::OsStr::to_os_string))
            .expect("PATH 应被显式设置");
        assert_eq!(
            std::env::split_paths(&path).collect::<Vec<_>>(),
            vec![
                std::path::PathBuf::from("/opt/gops"),
                std::path::PathBuf::from("/usr/local/bin"),
                std::path::PathBuf::from("/home/u/bin"),
                std::path::PathBuf::from("/usr/bin"),
                std::path::PathBuf::from("/bin"),
            ]
        );
    }
}
