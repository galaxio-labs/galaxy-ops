//! HTTP 客户端与传输层错误诊断。
//!
//! 场景：`wget` 能下、`gops` 却报 `error sending request for url (...)`。
//! 常见原因是 reqwest 默认会使用**环境代理**（`HTTP(S)_PROXY` / `ALL_PROXY`），
//! 而 `wget` 不使用 `ALL_PROXY`，两者行为并不一致；另外 reqwest 错误的
//! `Display` 只有顶层一句，真正原因（DNS / 代理 / TLS / 超时）在 `source()` 链里。
//!
//! 本模块提供：
//! 1. 统一的客户端构造（`gops` UA + 超时；[`direct_client`] 显式绕过环境代理）；
//! 2. [`send_with_direct_retry`]：传输层失败时用直连再试一次；
//! 3. [`error_chain`]：把 source 链拼成一句话，便于用户/工单直接定位；
//! 4. 直连成功而走代理失败时，把该主机加进 `NO_PROXY`，让后续下载
//!    （包括 `orion-accessor` 内部的客户端）也跳过代理 —— **只影响这台主机**，
//!    不破坏其它确实需要代理的目标。

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::error::MainReason;
use crate::prelude::MainResult;

pub const USER_AGENT: &str = "gops";

const PROXY_ENV_KEYS: &[&str] = &[
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
];

/// 使用系统代理检测（reqwest 默认行为）的客户端。
pub fn client(timeout: Duration) -> MainResult<reqwest::Client> {
    build(timeout, false)
}

/// 忽略环境代理的客户端。
pub fn direct_client(timeout: Duration) -> MainResult<reqwest::Client> {
    build(timeout, true)
}

fn build(timeout: Duration, direct: bool) -> MainResult<reqwest::Client> {
    // 这里的请求都是「探测 / 元数据」级的小请求，刻意向 wget 的行为对齐：
    // - 锁 HTTP/1.1：部分镜像/中间设备对 HTTP/2 会卡住（握手完但不回响应）；
    // - 直连回退只走 IPv4：坏掉的 IPv6 路由会把连接拖到整体超时。
    // 这样 HEAD 被挂起、H2 不兼容、IPv6 黑洞这三类网络问题都不会再误伤 gops。
    let mut builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(timeout)
        .http1_only();
    if direct {
        builder = builder
            .no_proxy()
            .local_address(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
    }
    builder
        .build()
        .map_err(|e| MainReason::resource_detail(format!("创建 HTTP 客户端失败：{e}")))
}

/// 发送请求：先用 `client`（会走环境代理），**传输层**失败时用 `direct` 再试一次。
///
/// 状态码失败（4xx/5xx）不算传输失败，不会触发重试 —— 那是目标的问题，不是链路的问题。
pub async fn send_with_direct_retry<F>(
    url: &str,
    client: &reqwest::Client,
    direct: &reqwest::Client,
    build_request: F,
) -> Result<(reqwest::Response, Option<reqwest::Error>), (reqwest::Error, reqwest::Error)>
where
    F: Fn(&reqwest::Client) -> reqwest::RequestBuilder,
{
    match build_request(client).send().await {
        Ok(resp) => Ok((resp, None)),
        Err(first) => match build_request(direct).send().await {
            Ok(resp) => {
                bypass_proxy_for_host(url, &first);
                Ok((resp, Some(first)))
            }
            Err(second) => Err((first, second)),
        },
    }
}

/// 把错误的 `source()` 链拼成一整句。
pub fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(err) = source {
        out.push_str(" → ");
        out.push_str(&err.to_string());
        source = err.source();
    }
    out
}

/// 两次尝试都失败时的补充说明（第一次的原因 + 直连重试的原因）。
pub fn retry_failed_note(first: &reqwest::Error, second: &reqwest::Error) -> String {
    let mut note = format!(
        "{}\n  已用直连重试（HTTP/1.1 + IPv4），仍失败：{}{}",
        error_chain(first),
        error_chain(second),
        proxy_env_note()
    );
    if (first.is_timeout() || second.is_timeout()) && !has_env_proxy() {
        note.push_str(
            "\n  提示：请求超时且未检测到代理环境变量。若 `wget` 能下载，请确认它是否用了\
             ~/.wgetrc / /etc/wgetrc 里的代理（gops 只读 HTTP(S)_PROXY / ALL_PROXY 环境变量）；\
             可以 `export HTTPS_PROXY=<代理>` 后重试。\n  \
             临时可用：先 `wget <url>` 把包下到本地，再 `gops prj upgrade --to <本地包>`。",
        );
    }
    note
}

/// 当前设置了的代理环境变量（只列名字与 scheme，避免泄漏凭据）。
pub fn proxy_env_note() -> String {
    let mut found = Vec::new();
    for key in PROXY_ENV_KEYS {
        if let Some(value) = std::env::var_os(key) {
            let value = value.to_string_lossy();
            let scheme = value.split("://").next().unwrap_or("?").trim();
            found.push(format!("{key}={scheme}://…"));
        }
    }
    if found.is_empty() {
        return String::new();
    }
    format!(
        "\n  检测到代理环境变量 {}（reqwest 会使用，wget 不使用 ALL_PROXY）；\
         如代理不可用可 unset 后重试。",
        found.join("、")
    )
}

/// 当前是否设置了代理环境变量（reqwest 会自动使用它们）。
pub fn has_env_proxy() -> bool {
    PROXY_ENV_KEYS
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()))
}

/// 下载前的轻量探测：只在配置了代理环境变量时做一次。
///
/// 走代理失败而直连成功 → 把主机加进 `NO_PROXY`；后续 `orion-accessor` 的下载
/// 也会因此跳过代理。两边都失败则什么都不做，把真正的错误留给下载本身报。
pub async fn probe_proxy_health(url: &str, timeout: Duration) {
    if !has_env_proxy() {
        return;
    }
    let (Ok(client), Ok(direct)) = (client(timeout), direct_client(timeout)) else {
        return;
    };
    let _ = send_with_direct_retry(url, &client, &direct, |client| {
        client.get(url).header(reqwest::header::RANGE, "bytes=0-0")
    })
    .await;
}

/// 直连可用、走环境代理失败 → 只把这台主机加进 `NO_PROXY`。
///
/// 不能换成“清掉所有代理变量”：其它主机（模块仓库、`api.github.com` 等）可能确实需要代理。
/// 依赖库（`orion-accessor`）构造 `reqwest::Client` 时没有 `no_proxy` 开关，
/// 但它会在**建客户端时**读取 `NO_PROXY`，所以这里改环境变量能让后续下载也生效。
fn bypass_proxy_for_host(url: &str, first: &reqwest::Error) {
    if !has_env_proxy() {
        return;
    }
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return;
    };
    let Some(host) = parsed.host_str().filter(|host| !host.is_empty()) else {
        return;
    };

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // reqwest 先读 `NO_PROXY`、再读 `no_proxy`；写大写即可覆盖。
    let existing = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .unwrap_or_default();
    let covered = existing.split(',').map(str::trim).any(|entry| {
        entry == "*"
            || entry.eq_ignore_ascii_case(host)
            || entry.trim_start_matches('.').eq_ignore_ascii_case(host)
    });
    if covered {
        return;
    }
    let updated = if existing.trim().is_empty() {
        host.to_string()
    } else {
        format!("{existing},{host}")
    };
    // SAFETY: 只改代理相关的环境变量，且只在本进程内生效；gops 自身的逻辑不读它。
    // libc 的 setenv 实现是带锁的，这里再用互斥量串行化并发探测任务。
    unsafe { std::env::set_var("NO_PROXY", &updated) };
    eprintln!(
        "⚠ 走环境代理请求 {host} 失败，已对该主机改用直连：{}",
        error_chain(first)
    );
}

/// 系统 `curl`/`wget` 探测结果。
#[derive(Debug)]
pub enum SystemProbe {
    /// 机器上没有 `curl` 也没有 `wget`。
    Missing,
    /// 拿到了最终 HTTP 状态码。
    Status(u16),
    /// 工具存在，但请求失败（超时/连接错误/退出码异常）。
    Failed(String),
}

/// 系统 `curl`/`wget` 下载结果。
#[derive(Debug)]
pub enum SystemDownload {
    Missing,
    Done,
    Failed(String),
}

/// 用系统 `curl`（其次 `wget`）探测 URL 的最终 HTTP 状态码。
///
/// 只作为 gops 自带 HTTP 客户端的**兜底**：部分受限网络里，Rust TLS 栈的
/// 握手/请求会被中间设备或 WAF 卡住（表现为整体超时），而系统 `wget`/`curl` 正常。
pub async fn system_probe(url: &str, timeout: Duration) -> SystemProbe {
    let max_time = timeout.as_secs().max(1).to_string();
    let curl = tokio::process::Command::new("curl")
        .args([
            "-sS",
            "-L",
            "-o",
            "/dev/null",
            "-r",
            "0-0",
            "-w",
            "%{http_code}",
            "--max-time",
            &max_time,
            url,
        ])
        .kill_on_drop(true)
        .output()
        .await;
    match curl {
        Ok(out) if out.status.success() => {
            let code = String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse::<u16>()
                .unwrap_or(0);
            if code == 0 {
                SystemProbe::Failed("curl 没有返回状态码".to_string())
            } else {
                SystemProbe::Status(code)
            }
        }
        Ok(out) => SystemProbe::Failed(stderr_tail(&out.stderr)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            system_probe_wget(url, &max_time).await
        }
        Err(err) => SystemProbe::Failed(format!("执行 curl 失败：{err}")),
    }
}

async fn system_probe_wget(url: &str, max_time: &str) -> SystemProbe {
    let timeout_arg = format!("--timeout={max_time}");
    let out = tokio::process::Command::new("wget")
        .args([
            "-q",
            "-O",
            "/dev/null",
            "--server-response",
            "--tries=1",
            "--max-redirect=10",
            &timeout_arg,
            "--header=Range: bytes=0-0",
            url,
        ])
        .kill_on_drop(true)
        .output()
        .await;
    match out {
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if let Some(status) = last_http_status(&stderr) {
                return SystemProbe::Status(status);
            }
            if out.status.success() {
                SystemProbe::Status(200)
            } else {
                SystemProbe::Failed(stderr_tail(&out.stderr))
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => SystemProbe::Missing,
        Err(err) => SystemProbe::Failed(format!("执行 wget 失败：{err}")),
    }
}

/// 从 wget `--server-response` 的输出里取最后一个 `HTTP/x.y <code>`。
fn last_http_status(stderr: &str) -> Option<u16> {
    stderr.lines().rev().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix("HTTP/")?;
        let (_version, code) = rest.split_once(' ')?;
        code.split_whitespace().next()?.parse::<u16>().ok()
    })
}

/// 用系统 `curl`（其次 `wget`）把 `url` 下到 `dest`（先写 `.part` 再 rename）。
///
/// 只在 `orion-accessor` 下载失败后调用；成功/失败都会打印一条警告，
/// 便于现场知道这次走了系统工具。
pub async fn system_download(
    url: &str,
    dest: &std::path::Path,
    timeout: Duration,
    auth: Option<(&str, &str)>,
) -> SystemDownload {
    let part = dest.with_extension("part");
    let _ = std::fs::remove_file(&part);
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = std::fs::create_dir_all(parent);
    }
    let connect = timeout.as_secs().max(1).to_string();

    let mut curl = tokio::process::Command::new("curl");
    curl.args([
        "-fSL",
        "--retry",
        "2",
        "--connect-timeout",
        &connect,
        // 大包可能下很久，但不能无限挂起；900s 作为整体上限。
        "--max-time",
        "900",
        "-o",
    ])
    .arg(&part)
    .arg(url)
    .kill_on_drop(true);
    if let Some((user, password)) = auth {
        curl.arg("-u").arg(format!("{user}:{password}"));
    }

    match curl.output().await {
        Ok(out) if out.status.success() => finish_system_download(&part, dest),
        Ok(out) => SystemDownload::Failed(stderr_tail(&out.stderr)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let timeout_arg = format!("--timeout={connect}");
            let mut wget = tokio::process::Command::new("wget");
            wget.args(["-q", "--tries=2", &timeout_arg, "-O"])
                .arg(&part)
                .arg(url)
                .kill_on_drop(true);
            if let Some((user, password)) = auth {
                wget.arg(format!("--user={user}"))
                    .arg(format!("--password={password}"));
            }
            match wget.output().await {
                Ok(out) if out.status.success() => finish_system_download(&part, dest),
                Ok(out) => SystemDownload::Failed(stderr_tail(&out.stderr)),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => SystemDownload::Missing,
                Err(err) => SystemDownload::Failed(format!("执行 wget 失败：{err}")),
            }
        }
        Err(err) => SystemDownload::Failed(format!("执行 curl 失败：{err}")),
    }
}

fn finish_system_download(part: &std::path::Path, dest: &std::path::Path) -> SystemDownload {
    match std::fs::rename(part, dest) {
        Ok(()) => SystemDownload::Done,
        Err(err) => {
            let _ = std::fs::remove_file(part);
            SystemDownload::Failed(format!(
                "重命名 {} -> {} 失败：{err}",
                part.display(),
                dest.display()
            ))
        }
    }
}

/// 取 URL 的最后一段作为文件名（与 accessor 的缓存命名一致）。
pub fn url_file_name(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .path_segments()
                .and_then(|mut segments| segments.next_back().map(str::to_string))
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "file.bin".to_string())
}

fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let tail = lines[lines.len().saturating_sub(3)..].join(" | ");
    if tail.is_empty() {
        "（无输出）".to_string()
    } else {
        tail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_chain_walks_sources() {
        #[derive(Debug)]
        struct Inner;
        impl std::fmt::Display for Inner {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "connection refused")
            }
        }
        impl std::error::Error for Inner {}

        #[derive(Debug)]
        struct Outer(Inner);
        impl std::fmt::Display for Outer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "error sending request")
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        assert_eq!(
            error_chain(&Outer(Inner)),
            "error sending request → connection refused"
        );
    }

    #[tokio::test]
    async fn send_retries_direct_when_proxy_client_fails() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/pkg.tar.gz")
            .with_status(200)
            .create_async()
            .await;
        let url = format!("{}/pkg.tar.gz", server.url());

        // 模拟“代理不可用”：显式指向一个没有监听的本地端口。
        let proxied = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .proxy(reqwest::Proxy::all("http://127.0.0.1:9").unwrap())
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let direct = direct_client(Duration::from_secs(5)).unwrap();

        let (resp, first) =
            send_with_direct_retry(&url, &proxied, &direct, |client| client.get(&url))
                .await
                .unwrap();
        assert_eq!(resp.status(), 200);
        assert!(first.is_some(), "should report the proxy error");
        mock.assert_async().await;
    }

    #[test]
    fn last_http_status_parses_wget_server_response() {
        let stderr = "  HTTP/1.1 302 Found\n  Location: /x\n  HTTP/1.1 206 Partial Content\n";
        assert_eq!(last_http_status(stderr), Some(206));
        assert_eq!(last_http_status("no headers"), None);
    }

    #[tokio::test]
    async fn system_probe_uses_system_tools() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/pkg.tar.gz")
            .with_status(200)
            .create_async()
            .await;
        match system_probe(
            &format!("{}/pkg.tar.gz", server.url()),
            Duration::from_secs(10),
        )
        .await
        {
            SystemProbe::Status(code) => assert!(code == 200 || code == 206, "status={code}"),
            SystemProbe::Missing => {} // 环境里没有 curl/wget 时跳过
            SystemProbe::Failed(err) => panic!("unexpected system probe failure: {err}"),
        }
    }

    #[tokio::test]
    async fn system_download_writes_file() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/pkg.tar.gz")
            .with_status(200)
            .with_body("hello")
            .create_async()
            .await;
        let tmp = tempfile::TempDir::new().unwrap();
        let dest = tmp.path().join("pkg.tar.gz");
        match system_download(
            &format!("{}/pkg.tar.gz", server.url()),
            &dest,
            Duration::from_secs(10),
            None,
        )
        .await
        {
            SystemDownload::Done => {
                assert_eq!(std::fs::read_to_string(&dest).unwrap(), "hello")
            }
            SystemDownload::Missing => {}
            SystemDownload::Failed(err) => panic!("unexpected system download failure: {err}"),
        }
    }
}
