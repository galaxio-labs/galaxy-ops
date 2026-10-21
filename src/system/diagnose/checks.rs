//! 内置检查（`gops` 侧，**与业务无关**）。
//!
//! 判据见 `docs/design/run-diagnose.md` §2：换一个栈仍然成立的检查写在这里；
//! 「这个系统的期望」属于 `diagnose` 流程，不在这里。
//!
//! 所有检查只看 [`Context`]（已采集的事实）—— 不再调 docker，便于单测与复用。

use std::collections::BTreeMap;
use std::process::Command;

use super::context::{Context, MountFacts, ServiceFacts};
use super::report::{Check, CheckStatus};

/// 内置检查的 `source`（流程交回的结果用流程名）。
pub const SOURCE: &str = "gops";

/// 一次体检要看的日志行数（`--logs`，`0` = 不抓日志）。
pub const DEFAULT_LOG_LINES: usize = 200;

/// 从已采集的事实推出全部内置检查。
pub fn builtin(context: &Context, log_lines: usize, log_ignore: &[String]) -> Vec<Check> {
    let mut checks = Vec::new();
    let Some(compose) = context.compose.as_ref() else {
        // `kind: gxl`：没有 compose 可查，交给流程。
        return checks;
    };

    checks.push(render_check(context, compose));
    if let Some(error) = &compose.ps_error {
        checks.push(
            Check::warn(SOURCE, "gops.compose.ps", "无法读取容器列表")
                .with_detail(error)
                .with_hint("确认 docker daemon 可用：`docker info`"),
        );
    }

    checks.extend(container_checks(&compose.services));
    checks.extend(mount_checks(&compose.services));
    checks.extend(port_conflicts(&compose.services));
    checks.extend(cert_checks(&compose.services));

    if log_lines > 0 {
        checks.extend(log_checks(&compose.services, log_lines, log_ignore));
    }
    checks
}

/// 测试便捷入口：不忽略任何日志行。
#[cfg(test)]
fn builtin_all(context: &Context, log_lines: usize) -> Vec<Check> {
    builtin(context, log_lines, &[])
}

fn render_check(context: &Context, compose: &super::context::ComposeFacts) -> Check {
    let services = compose.services.len();
    let project = compose.project.as_deref().unwrap_or("-");
    let title = format!("compose 渲染成功（{services} 个服务）");
    Check::ok(SOURCE, "gops.compose.render", title)
        .with_detail(format!("project={project} root={}", context.system.root))
}

fn container_checks(services: &[ServiceFacts]) -> Vec<Check> {
    let mut checks = Vec::new();
    for service in services {
        let code = format!("gops.container.{}", service.name);
        let Some(_container) = service.container.as_deref() else {
            checks.push(
                Check::fail(SOURCE, code, format!("服务 `{}` 没有容器", service.name))
                    .with_detail(
                        service
                            .image
                            .as_deref()
                            .map(|image| format!("image={image}"))
                            .unwrap_or_default(),
                    )
                    .with_hint("`gops run status` 看声明；未创建就 `gops run start`"),
            );
            continue;
        };

        match service.state.as_deref() {
            Some("running") => checks.push(
                Check::ok(SOURCE, code.clone(), format!("`{}` running", service.name))
                    .with_detail(container_detail(service)),
            ),
            // 一次性任务（`restart: no`）跑完并退出 0 是**正常完成** —— 不是故障。
            Some(state) if is_completed_one_shot(service) => checks.push(
                Check::ok(
                    SOURCE,
                    code.clone(),
                    format!("`{}` 一次性任务已完成（{state} 0）", service.name),
                )
                .with_detail(container_detail(service)),
            ),
            Some(state) => checks.push(
                Check::fail(
                    SOURCE,
                    code.clone(),
                    format!("`{}` 未运行（{state}）", service.name),
                )
                .with_detail(container_detail(service))
                .with_hint("看容器日志尾部；`gops run start` 重新拉起"),
            ),
            None => checks.push(
                Check::warn(
                    SOURCE,
                    code.clone(),
                    format!("`{}` 运行态未知", service.name),
                )
                .with_detail("docker inspect 没有返回该容器（daemon 不可用？）"),
            ),
        }

        if service.health.as_deref() == Some("unhealthy") {
            checks.push(
                Check::fail(
                    SOURCE,
                    format!("{code}.health"),
                    format!("`{}` 健康检查失败", service.name),
                )
                .with_hint("看 healthcheck 的定义与容器内探针输出"),
            );
        }
        if service.oom_killed {
            checks.push(
                Check::fail(
                    SOURCE,
                    format!("{code}.oom"),
                    format!("`{}` 曾因 OOM 被杀", service.name),
                )
                .with_hint("提高内存上限或查泄漏"),
            );
        }
        if service.restarts > 0 {
            checks.push(
                Check::warn(
                    SOURCE,
                    format!("{code}.restarts"),
                    format!("`{}` 重启过 {} 次", service.name, service.restarts),
                )
                .with_hint("重启次数持续增长时看 `docker inspect .State.Error`"),
            );
        }
    }
    checks
}

fn is_completed_one_shot(service: &ServiceFacts) -> bool {
    service.restart.as_deref() == Some("no") && service.exit_code == Some(0)
}

fn container_detail(service: &ServiceFacts) -> String {
    let mut parts = Vec::new();
    if let Some(container) = &service.container {
        parts.push(format!("container={container}"));
    }
    if let Some(image) = &service.image {
        parts.push(format!("image={image}"));
    }
    if let Some(code) = service.exit_code
        && service.state.as_deref() != Some("running")
    {
        parts.push(format!("exit_code={code}"));
    }
    if let Some(error) = &service.error {
        parts.push(format!("state.error={error}"));
    }
    parts.join(" ")
}

/// 每个服务的 bind 挂载：source 必须存在；顺带把属主/权限摊开（权限是否**对**由 `sys perms` 判）。
fn mount_checks(services: &[ServiceFacts]) -> Vec<Check> {
    let mut checks = Vec::new();
    for service in services {
        if service.mounts.is_empty() {
            continue;
        }
        let code = format!("gops.mount.{}", service.name);
        let missing: Vec<&MountFacts> = service
            .mounts
            .iter()
            .filter(|mount| mount.kind == "bind" && mount.source_exists == Some(false))
            .collect();
        let detail = service
            .mounts
            .iter()
            .map(describe_mount)
            .collect::<Vec<_>>()
            .join("\n");
        if missing.is_empty() {
            checks.push(
                Check::ok(
                    SOURCE,
                    code,
                    format!(
                        "`{}` 的 {} 个挂载源都在",
                        service.name,
                        service.mounts.len()
                    ),
                )
                .with_detail(detail),
            );
        } else {
            let list = missing
                .iter()
                .map(|mount| format!("{} → {}", mount.source, mount.target))
                .collect::<Vec<_>>()
                .join("；");
            checks.push(
                Check::fail(
                    SOURCE,
                    code,
                    format!("`{}` 有 {} 个挂载源不存在", service.name, missing.len()),
                )
                .with_detail(format!("{list}\n{detail}"))
                .with_hint("先在宿主建目录（属主按容器 uid），再 `gops run start`"),
            );
        }
    }
    checks
}

fn describe_mount(mount: &MountFacts) -> String {
    let mut text = format!(
        "{} {} → {}{}",
        mount.kind,
        mount.source,
        mount.target,
        if mount.read_only { " (ro)" } else { "" }
    );
    if let Some(exists) = mount.source_exists
        && !exists
    {
        text.push_str(" [源不存在]");
    }
    if let (Some(owner), Some(mode)) = (&mount.owner, &mount.mode) {
        text.push_str(&format!(" {owner} {mode}"));
    }
    text
}

/// 同一宿主 `ip:port` 被两个服务发布 = 启动时必然冲突（提前报，别等 compose 报）。
fn port_conflicts(services: &[ServiceFacts]) -> Vec<Check> {
    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for service in services {
        for port in &service.ports {
            let Some(published) = &port.published else {
                continue;
            };
            let host = port.host_ip.as_deref().unwrap_or("0.0.0.0");
            let key = format!(
                "{host}:{published}/{}",
                port.protocol.as_deref().unwrap_or("tcp")
            );
            seen.entry(key).or_default().push(service.name.clone());
        }
    }
    let mut checks = Vec::new();
    for (address, mut owners) in seen {
        owners.sort();
        owners.dedup();
        if owners.len() > 1 {
            checks.push(
                Check::fail(SOURCE, format!("gops.port.{address}"), "宿主端口重复发布")
                    .with_detail(format!(
                        "{address} 被这些服务同时发布：{}",
                        owners.join("、")
                    ))
                    .with_hint("改 published 端口；或让其中一个不发布"),
            );
        }
    }
    checks
}

/// 私钥类文件不应被同组/其他用户读到（`0600`/`0400` 才对）。
fn cert_checks(services: &[ServiceFacts]) -> Vec<Check> {
    let mut checks = Vec::new();
    for service in services {
        for mount in &service.mounts {
            if mount.kind != "bind" || mount.source_exists != Some(true) {
                continue;
            }
            let looks_private = mount.source.ends_with(".key")
                || mount.source.ends_with(".key.pem")
                || mount.source.contains("/private/");
            if !looks_private {
                continue;
            }
            let Some(mode) = &mount.mode else {
                continue;
            };
            let Ok(mode) = u32::from_str_radix(mode, 8) else {
                continue;
            };
            if mode & 0o077 != 0 {
                checks.push(
                    Check::warn(
                        SOURCE,
                        format!("gops.cert.{}", service.name),
                        "私钥文件的权限过宽",
                    )
                    .with_detail(format!("{} mode={mode:04o}（应为 600/400）", mount.source))
                    .with_hint("chmod 600 <私钥>；属主按容器 uid"),
                );
            }
        }
    }
    checks
}

/// 每个运行中容器最近 N 行里有没有 error 级线索（**只采集 + 脱敏，不解读**）。
///
/// 三类噪声在这里被收掉（P1-6）：
/// 1. 否定式：`no error` / `0 errors` / `error=none` / `error_count=0`；
/// 2. `--log-ignore` 里的子串（站点自行屏蔽已知噪声）；
/// 3. 同一条错误刷屏：归一化（去时间戳/数字）后折叠成 `×N`。
///
/// 输出前先脱敏（P0-2）：`password=…` / `Bearer …` 这类值不进报告。
fn log_checks(services: &[ServiceFacts], lines: usize, ignore: &[String]) -> Vec<Check> {
    let ignore: Vec<String> = ignore
        .iter()
        .map(|item| item.to_ascii_lowercase())
        .collect();
    let mut checks = Vec::new();
    for service in services {
        let Some(container) = service.container.as_deref() else {
            continue;
        };
        if service.state.as_deref() != Some("running") {
            continue;
        }
        let Ok(output) = Command::new("docker")
            .args(["logs", "--tail", &lines.to_string(), container])
            .output()
        else {
            continue;
        };
        // `docker logs` 把容器 stdout 与 stderr 分开给；两股都要看。
        let mut text = String::from_utf8_lossy(&output.stdout).to_string();
        text.push('\n');
        text.push_str(&String::from_utf8_lossy(&output.stderr));

        let hits: Vec<&str> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter(|line| !is_ignored(line, &ignore))
            .filter(|line| looks_like_error(line))
            .collect();
        if hits.is_empty() {
            continue;
        }

        // 归一化后计数：同一个错误只占一行（带 ×N）。顺序按出现次数降序。
        let mut folded = fold_hits(hits.iter().copied());
        folded.sort_by_key(|item| std::cmp::Reverse(item.0));
        let kinds = folded.len();
        let detail = folded
            .iter()
            .take(3)
            .map(|(count, sample)| format!("×{count} {sample}"))
            .collect::<Vec<_>>()
            .join("\n");

        checks.push(
            Check::warn(
                SOURCE,
                format!("gops.logs.{}", service.name),
                format!(
                    "`{}` 最近 {lines} 行含 {} 条错误线索（{kinds} 类）",
                    service.name,
                    hits.len()
                ),
            )
            .with_detail(detail)
            .with_hint(
                "`docker logs --tail 500 <容器>` 看上下文；误报可用 `--log-ignore <子串>` 屏蔽",
            ),
        );
    }
    checks
}

/// 归一化后折叠（出现次数, 脱敏样本）；同一条错误刷屏只占一行。
fn fold_hits<'a>(hits: impl Iterator<Item = &'a str>) -> Vec<(usize, String)> {
    let mut folded: Vec<(String, usize, String)> = Vec::new();
    for line in hits {
        let sample = truncate_line(&redact(line.trim()), 200);
        let key = normalize_line(&sample);
        match folded.iter_mut().find(|(seen, _, _)| *seen == key) {
            Some((_, count, _)) => *count += 1,
            None => folded.push((key, 1, sample)),
        }
    }
    folded
        .into_iter()
        .map(|(_, count, sample)| (count, sample))
        .collect()
}

fn is_ignored(line: &str, ignore: &[String]) -> bool {
    let lower = line.to_ascii_lowercase();
    ignore.iter().any(|needle| lower.contains(needle))
}

/// 关键词启发式：命中 error 级词，但先排除明显否定/统计行。
fn looks_like_error(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    const NEGATIVE: &[&str] = &[
        "no error",
        "0 error",
        "0 errors",
        "error=none",
        "error=0",
        "error_count=0",
        "errors=0",
        "error rate",
    ];
    if NEGATIVE.iter().any(|needle| lower.contains(needle)) {
        return false;
    }
    [
        "error",
        "fatal",
        "panic",
        "unauthorized",
        "denied",
        "exception",
        "traceback",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// 归一化：去时间戳、把数字/十六进制 ID 统一成 `#`，让“同一错误的重复”能被折叠。
fn normalize_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut skipping_number = false;
    for ch in line.chars() {
        if ch.is_ascii_digit() {
            if !skipping_number {
                out.push('#');
                skipping_number = true;
            }
        } else {
            skipping_number = false;
            out.push(ch);
        }
    }
    out
}

/// 脱敏：`password=…` / `token: …` / `Bearer …` 这类值换成 `***`。
///
/// 不追求覆盖所有格式（没有正则依赖），只收住报告里最常见的泄露面。
fn redact(line: &str) -> String {
    // `Bearer <token>` 是“空格分隔”的，不走 key=value 那条路。
    let mut out = mask_bearer(line);
    for key in [
        "password",
        "passwd",
        "secret",
        "token",
        "api_key",
        "apikey",
        "authorization",
    ] {
        out = mask_after_key(&out, key);
    }
    out
}

/// `Bearer <token>`：把紧随其后的那个词换成 `***`。
fn mask_bearer(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let mut result = line.to_string();
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find("bearer") {
        let start = search_from + rel + "bearer".len();
        let rest = &result[start..];
        let ws = rest.len() - rest.trim_start().len();
        let value_at = start + ws;
        let value = &result[value_at..];
        let value_len = value
            .find(|ch: char| ch.is_whitespace() || ch == '"' || ch == ',' || ch == ';')
            .unwrap_or(value.len());
        if value_len > 0 {
            let mut masked = String::with_capacity(result.len());
            masked.push_str(&result[..value_at]);
            masked.push_str("***");
            masked.push_str(&result[value_at + value_len..]);
            result = masked;
        }
        search_from = value_at + 3;
        if search_from >= lower.len() {
            break;
        }
    }
    result
}

/// 找到 `key` 后的 `=` / `:`（允许空格），把后面的一个“词”换成 `***`。
fn mask_after_key(line: &str, key: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let mut result = line.to_string();
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find(key) {
        let start = search_from + rel + key.len();
        let rest = &lower[start..];
        let trimmed = rest.trim_start();
        let sep_len = rest.len() - trimmed.len();
        let sep = trimmed.chars().next();
        if matches!(sep, Some('=') | Some(':')) {
            // `=` / `:` 之后的空白 + 值（直到空白 / 引号 / 逗号）
            let value_start = start + sep_len + 1;
            let after = &line[value_start..];
            let skip_ws = after.len() - after.trim_start().len();
            let value_at = value_start + skip_ws;
            let value = &line[value_at..];
            let value_len = if value.starts_with('"') || value.starts_with('\'') {
                // 引号值：一直占到闭合引号（含引号），否则到空白/逗号
                let quote = value.chars().next().unwrap_or('"');
                value[1..]
                    .find(quote)
                    .map(|end| end + 2)
                    .unwrap_or(value.len())
            } else {
                value
                    .find(|ch: char| ch.is_whitespace() || ch == '"' || ch == ',' || ch == ';')
                    .unwrap_or(value.len())
            };
            if value_len > 0 {
                let mut masked = String::with_capacity(result.len() + 3);
                masked.push_str(&result[..value_at]);
                masked.push_str("***");
                masked.push_str(&result[value_at + value_len..]);
                result = masked;
            }
        }
        search_from = start;
        if search_from >= lower.len() {
            break;
        }
    }
    result
}

fn truncate_line(line: &str, limit: usize) -> String {
    if line.chars().count() <= limit {
        return line.to_string();
    }
    let mut text: String = line.chars().take(limit).collect();
    text.push('…');
    text
}

/// 供 `--json` 之外的统计用：内置检查是否存在 fail。
pub fn has_fail(checks: &[Check]) -> bool {
    checks.iter().any(|check| check.status == CheckStatus::Fail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::diagnose::context::{ComposeFacts, MountFacts, PortFacts, ServiceFacts};
    use serde_json::json;

    fn service(name: &str) -> ServiceFacts {
        ServiceFacts {
            name: name.to_string(),
            container: Some(format!("stack-{name}-1")),
            image: Some("img:1".to_string()),
            state: Some("running".to_string()),
            restart: Some("unless-stopped".to_string()),
            health: None,
            restarts: 0,
            exit_code: None,
            oom_killed: false,
            error: None,
            ports: Vec::new(),
            mounts: Vec::new(),
        }
    }

    fn context(services: Vec<ServiceFacts>) -> Context {
        let mut context = Context::minimal(super::super::context::SystemInfo {
            name: "stack".into(),
            kind: "docker-compose".into(),
            root: "/srv/stack".into(),
            env: "default".into(),
        });
        context.compose = Some(ComposeFacts {
            project: Some("stack".into()),
            services,
            ps_error: None,
        });
        context
    }

    fn codes(checks: &[Check]) -> Vec<&str> {
        checks.iter().map(|check| check.code.as_str()).collect()
    }

    #[test]
    fn a_healthy_stack_has_no_failures() {
        let checks = builtin_all(&context(vec![service("a"), service("b")]), 0);
        assert!(!has_fail(&checks), "{:?}", codes(&checks));
        assert!(codes(&checks).contains(&"gops.compose.render"));
    }

    #[test]
    fn a_completed_one_shot_is_not_a_failure() {
        let mut init = service("init");
        init.state = Some("exited".into());
        init.exit_code = Some(0);
        init.restart = Some("no".into());
        let checks = builtin_all(&context(vec![init]), 0);
        let check = checks
            .iter()
            .find(|check| check.code == "gops.container.init")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Ok, "{}", check.title);
        assert!(!has_fail(&checks));
    }

    #[test]
    fn a_failed_one_shot_is_still_a_failure() {
        let mut init = service("init");
        init.state = Some("exited".into());
        init.exit_code = Some(1);
        init.restart = Some("no".into());
        let checks = builtin_all(&context(vec![init]), 0);
        assert!(codes(&checks).contains(&"gops.container.init"));
        assert!(has_fail(&checks));
    }

    #[test]
    fn a_missing_container_is_a_failure() {
        let mut broken = service("a");
        broken.container = None;
        broken.state = None;
        let checks = builtin_all(&context(vec![broken]), 0);
        assert!(
            checks
                .iter()
                .any(|check| check.code == "gops.container.a" && check.status == CheckStatus::Fail)
        );
    }

    #[test]
    fn an_exited_container_reports_the_exit_code() {
        let mut broken = service("a");
        broken.state = Some("exited".into());
        broken.exit_code = Some(75);
        broken.error = Some("lock held".into());
        let checks = builtin_all(&context(vec![broken]), 0);
        let check = checks
            .iter()
            .find(|check| check.code == "gops.container.a")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Fail);
        let detail = check.detail.as_deref().unwrap();
        assert!(detail.contains("exit_code=75"), "{detail}");
        assert!(detail.contains("state.error=lock held"), "{detail}");
    }

    #[test]
    fn restarts_and_oom_are_reported() {
        let mut flaky = service("a");
        flaky.restarts = 3;
        flaky.oom_killed = true;
        let checks = builtin_all(&context(vec![flaky]), 0);
        assert!(codes(&checks).contains(&"gops.container.a.oom"));
        assert!(codes(&checks).contains(&"gops.container.a.restarts"));
    }

    #[test]
    fn a_missing_bind_source_is_a_failure_with_the_path_listed() {
        let mut broken = service("a");
        broken.mounts = vec![MountFacts {
            kind: "bind".into(),
            source: "/missing/dir".into(),
            target: "/data".into(),
            read_only: false,
            source_exists: Some(false),
            owner: None,
            mode: None,
        }];
        let checks = builtin_all(&context(vec![broken]), 0);
        let check = checks
            .iter()
            .find(|check| check.code == "gops.mount.a")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.detail.as_deref().unwrap().contains("/missing/dir"));
    }

    #[test]
    fn duplicate_published_ports_are_a_failure() {
        let mut a = service("a");
        a.ports = vec![PortFacts {
            published: Some("443".into()),
            target: 3000,
            host_ip: None,
            protocol: Some("tcp".into()),
        }];
        let mut b = service("b");
        b.ports = a.ports.clone();
        let checks = builtin_all(&context(vec![a, b]), 0);
        assert!(codes(&checks).contains(&"gops.port.0.0.0.0:443/tcp"));
    }

    #[test]
    fn a_world_readable_private_key_is_a_warning() {
        let mut svc = service("a");
        svc.mounts = vec![MountFacts {
            kind: "bind".into(),
            source: "/srv/identity/private/admin.key.pem".into(),
            target: "/cfg/key.pem".into(),
            read_only: true,
            source_exists: Some(true),
            owner: Some("999:999".into()),
            mode: Some("0644".into()),
        }];
        let checks = builtin_all(&context(vec![svc]), 0);
        let check = checks
            .iter()
            .find(|check| check.code == "gops.cert.a")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.detail.as_deref().unwrap().contains("0644"));
    }

    #[test]
    fn an_unhealthy_container_is_a_failure() {
        let mut svc = service("a");
        svc.health = Some("unhealthy".into());
        let checks = builtin_all(&context(vec![svc]), 0);
        let check = checks
            .iter()
            .find(|check| check.code == "gops.container.a.health")
            .expect("要有健康检查失败项");
        assert_eq!(check.status, CheckStatus::Fail);
    }

    #[test]
    fn unknown_runtime_is_a_warning_not_a_failure() {
        // 容器名拿到了，但 inspect 没返回（daemon 抖动）：不能当“正常”，也不该当“挂了”。
        let mut svc = service("a");
        svc.state = None;
        let checks = builtin_all(&context(vec![svc]), 0);
        let check = checks
            .iter()
            .find(|check| check.code == "gops.container.a")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Warn);
    }

    #[test]
    fn named_volumes_are_not_checked_for_host_existence() {
        let mut svc = service("a");
        svc.mounts = vec![MountFacts {
            kind: "volume".into(),
            source: "stack_data".into(),
            target: "/storage".into(),
            read_only: false,
            source_exists: None,
            owner: None,
            mode: None,
        }];
        let checks = builtin_all(&context(vec![svc]), 0);
        let check = checks
            .iter()
            .find(|check| check.code == "gops.mount.a")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Ok, "named volume 没有宿主路径");
    }

    #[test]
    fn a_strict_key_permission_is_not_reported() {
        let mut svc = service("a");
        svc.mounts = vec![MountFacts {
            kind: "bind".into(),
            source: "/srv/identity/private/admin.key.pem".into(),
            target: "/cfg/key.pem".into(),
            read_only: true,
            source_exists: Some(true),
            owner: Some("999:999".into()),
            mode: Some("0600".into()),
        }];
        let checks = builtin_all(&context(vec![svc]), 0);
        assert!(!codes(&checks).contains(&"gops.cert.a"));
    }

    #[test]
    fn ports_on_different_host_ips_are_not_conflicts() {
        let with = |ip: &str, published: Option<&str>| {
            let mut svc = service("a");
            svc.ports = vec![PortFacts {
                published: published.map(str::to_string),
                target: 3000,
                host_ip: Some(ip.to_string()),
                protocol: Some("tcp".into()),
            }];
            svc
        };
        let mut loopback = with("127.0.0.1", Some("443"));
        loopback.name = "b".into();
        let checks = builtin_all(&context(vec![with("0.0.0.0", Some("443")), loopback]), 0);
        assert!(
            !codes(&checks)
                .iter()
                .any(|code| code.starts_with("gops.port."))
        );

        // 没有 published（只声明容器端口）不参与冲突判定
        let mut a = with("0.0.0.0", None);
        a.name = "a".into();
        let mut b = with("0.0.0.0", None);
        b.name = "b".into();
        let checks = builtin_all(&context(vec![a, b]), 0);
        assert!(
            !codes(&checks)
                .iter()
                .any(|code| code.starts_with("gops.port."))
        );
    }

    #[test]
    fn log_keyword_heuristic_skips_negatives() {
        assert!(looks_like_error("ERROR: column does not exist"));
        assert!(looks_like_error("FATAL: role missing"));
        assert!(!looks_like_error("completed with no error"));
        assert!(!looks_like_error("errors=0"));
        assert!(!looks_like_error("error rate: 0.1%"));
    }

    #[test]
    fn log_ignore_is_case_insensitive_substring() {
        let ignore = vec!["permission denied".to_string()];
        assert!(is_ignored("FATAL: Permission denied for database", &ignore));
        assert!(!is_ignored("ERROR: column does not exist", &ignore));
    }

    #[test]
    fn redaction_masks_common_secret_shapes() {
        assert_eq!(redact("password=hunter2 user=x"), "password=*** user=x");
        assert_eq!(redact("token: abc123,"), "token: ***,");
        // 两套规则会叠掩（`Bearer` 先按空格掩，`authorization:` 再当值掩）；
        // 只要值不在报告里即可，不绑定具体掩码形状。
        let bearer = redact("Authorization: Bearer abc.def");
        assert!(!bearer.contains("abc.def"), "{bearer}");
        assert!(bearer.contains("***"), "{bearer}");
    }

    #[test]
    fn redaction_keeps_quoted_values_out_of_the_report() {
        assert_eq!(redact("secret=\"s3cr3t\" rest"), "secret=*** rest");
    }

    #[test]
    fn repeated_errors_fold_into_one_line_with_count() {
        let hits = vec![
            "2026-10-21 19:50:42 ERROR: column 12 missing",
            "2026-10-21 19:51:42 ERROR: column 99 missing",
            "2026-10-21 19:52:42 FATAL: something else",
        ];
        let mut folded = fold_hits(hits.into_iter());
        folded.sort_by_key(|item| std::cmp::Reverse(item.0));
        assert_eq!(folded.len(), 2, "去数字/时间后应归并成两类");
        assert_eq!(folded[0].0, 2);
        assert!(folded[0].1.contains("ERROR: column"));
        assert_eq!(folded[1].0, 1);
    }

    #[test]
    fn normalize_line_drops_volatile_numbers() {
        assert_eq!(
            normalize_line("2026-10-21 19:50:42 ERROR: id=42"),
            "#-#-# #:#:# ERROR: id=#"
        );
    }

    #[test]
    fn gxl_context_has_no_builtin_checks() {
        let context = Context::minimal(super::super::context::SystemInfo {
            name: "s".into(),
            kind: "gxl".into(),
            root: "/srv/s".into(),
            env: "default".into(),
        });
        assert!(builtin_all(&context, 200).is_empty());
    }

    #[test]
    fn compose_render_check_mentions_service_count() {
        let checks = builtin_all(&context(vec![service("a"), service("b")]), 0);
        let render = checks
            .iter()
            .find(|check| check.code == "gops.compose.render")
            .unwrap();
        assert!(render.title.contains("2 个服务"), "{}", render.title);
        assert_eq!(json!(render.status.as_str()), json!("ok"));
    }
}
