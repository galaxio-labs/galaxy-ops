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
    use super::{gx_bin_path_from, gx_run_args, is_reserved_env, probe_flow, run_gx_flow_capture};

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
}
