//! 运行时事实采集（`gops run diagnose` 的第 ① 段，**只读**）。
//!
//! 采集对象与契约见 `docs/design/run-diagnose.md` §5（Context Bundle v1）：
//! - `docker compose config --format json`（声明 + 渲染后的挂载/端口/镜像；密钥已掩码）
//! - `docker compose ps --all --format json`（服务 → 容器名）
//! - `docker inspect`（状态 / 健康 / 重启次数 / OOM / 退出码 / 错误）
//! - 宿主 `stat`（bind source 是否存在、属主、权限）
//!
//! **不做判断**：这里只产出事实，结论在 [`super::checks`]。

use std::path::Path;
use std::process::Command;

use serde_derive::Serialize;
use serde_json::{Value, json};

use crate::error::{MainReason, MainResult};
use crate::system::compose::compose_invocation;

/// 契约版本（破坏性变更才抬）。
pub const SCHEMA_VERSION: &str = "1";
/// `config.resolved` 的体积上限：超了只留标记（契约 §5）。
const MAX_CONFIG_BYTES: usize = 2 * 1024 * 1024;

/// gops → 流程 的上下文。
#[derive(Clone, Debug, Serialize)]
pub struct Context {
    pub schema_version: &'static str,
    pub generated_at: String,
    pub system: SystemInfo,
    pub host: HostInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compose: Option<ComposeFacts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<ConfigFacts>,
}

/// 系统标识。
#[derive(Clone, Debug, Serialize)]
pub struct SystemInfo {
    pub name: String,
    pub kind: String,
    pub root: String,
    pub env: String,
}

/// 宿主信息（排障时用于区分“栈的问题”还是“机器的问题”）。
#[derive(Clone, Debug, Serialize)]
pub struct HostInfo {
    pub os: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

/// `docker compose config` 的渲染结果（密钥已掩码）。
#[derive(Clone, Debug, Serialize)]
pub struct ConfigFacts {
    pub resolved: Value,
}

/// compose 服务与容器的对应关系。
#[derive(Clone, Debug, Default, Serialize)]
pub struct ComposeFacts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub services: Vec<ServiceFacts>,
    /// `docker compose ps` 失败原因（配置渲染成功时才会走到这里）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ps_error: Option<String>,
}

/// 单个服务的声明 + 运行态。
#[derive(Clone, Debug, Serialize)]
pub struct ServiceFacts {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// compose 里的 `user:`（`900:900` / `wist` / 缺省）。
    ///
    /// 用于判断「私钥 0640 的属组是不是容器 gid」——只看数字形式。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// `running` / `exited` / `created` / …；拿不到就是 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// 重启策略（`no` / `unless-stopped` / `always` …）。一次性任务（`no`）退出 0 不算故障。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart: Option<String>,
    /// `healthy` / `unhealthy` / `starting`；无健康检查为 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
    pub restarts: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub oom_killed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub ports: Vec<PortFacts>,
    pub mounts: Vec<MountFacts>,
}

/// 一个发布端口。
#[derive(Clone, Debug, Serialize)]
pub struct PortFacts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    pub target: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
}

/// 一个挂载（bind / volume）。
#[derive(Clone, Debug, Serialize)]
pub struct MountFacts {
    #[serde(rename = "type")]
    pub kind: String,
    pub source: String,
    pub target: String,
    pub read_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_exists: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

impl Context {
    /// 采集全部事实。
    ///
    /// `secret_env`：渲染 compose 时要注入的环境（`SEC_*` → **掩码值**），
    /// 与 `gops run diagnose` 的旧行为一致，避免明文进上下文。
    ///
    /// `compose config` 失败 = 诊断无法进行 → `Err`（CLI 退 `2`）。
    pub fn collect(
        dir: &Path,
        system: SystemInfo,
        secret_env: &[(String, String)],
    ) -> MainResult<Self> {
        let (_, global_args) = compose_invocation(dir);
        let resolved = compose_config_json(dir, &global_args, secret_env)?;
        let compose = collect_compose_facts(dir, &global_args, secret_env, &resolved);
        let resolved = fit_config(resolved);

        Ok(Self {
            schema_version: SCHEMA_VERSION,
            generated_at: chrono::Local::now().to_rfc3339(),
            system,
            host: collect_host(),
            compose: Some(compose),
            config: Some(ConfigFacts { resolved }),
        })
    }

    /// 最小上下文（`kind: gxl`：没有 compose 可采，只给身份与环境）。
    pub fn minimal(system: SystemInfo) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            generated_at: chrono::Local::now().to_rfc3339(),
            system,
            host: collect_host(),
            compose: None,
            config: None,
        }
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

fn collect_host() -> HostInfo {
    HostInfo {
        os: std::env::consts::OS.to_string(),
        docker: command_stdout(Command::new("docker").args([
            "version",
            "--format",
            "{{.Server.Version}}",
        ])),
        user: std::env::var("USER").ok(),
    }
}

fn command_stdout(command: &mut Command) -> Option<String> {
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn compose_command(
    dir: &Path,
    global_args: &[std::ffi::OsString],
    args: &[&str],
    secret_env: &[(String, String)],
) -> Command {
    let mut command = Command::new("docker");
    command
        .arg("compose")
        .args(global_args)
        .args(args)
        .current_dir(dir);
    for (key, value) in secret_env {
        command.env(key, value);
    }
    command
}

fn compose_config_json(
    dir: &Path,
    global_args: &[std::ffi::OsString],
    secret_env: &[(String, String)],
) -> MainResult<Value> {
    let output = compose_command(
        dir,
        global_args,
        &["config", "--format", "json"],
        secret_env,
    )
    .output()
    .map_err(|err| MainReason::logic_detail(format!("无法执行 docker compose：{err}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(MainReason::logic_detail(format!(
            "docker compose config 失败（诊断无法进行）：{stderr}"
        )));
    }
    serde_json::from_slice(&output.stdout).map_err(|err| {
        MainReason::logic_detail(format!(
            "docker compose config --format json 输出无法解析：{err}（需要 compose v2.21+）"
        ))
    })
}

/// 从渲染结果 + 容器快照装配服务事实。
///
/// **纯函数**（不跑 docker）：解析规则（`container_name`、端口/挂载/重启策略、
/// inspect 字段映射）全部在这里，单测不依赖 docker。
fn compose_facts(
    resolved: &Value,
    containers: &std::collections::BTreeMap<String, String>,
    inspected: &std::collections::BTreeMap<String, Value>,
    ps_error: Option<String>,
) -> ComposeFacts {
    let services = resolved
        .get("services")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let project = resolved
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut facts: Vec<ServiceFacts> = Vec::new();
    for (name, spec) in &services {
        let container = containers.get(name).cloned();
        let runtime = container
            .as_ref()
            .and_then(|name| inspected.get(name.trim_start_matches('/')));
        facts.push(ServiceFacts {
            name: name.clone(),
            container: container.clone(),
            image: spec
                .get("image")
                .and_then(Value::as_str)
                .map(str::to_string),
            user: spec.get("user").and_then(Value::as_str).map(str::to_string),
            state: runtime.and_then(|item| field_str(item, "state")),
            restart: declared_restart(spec),
            health: runtime.and_then(|item| field_str(item, "health")),
            restarts: runtime
                .and_then(|item| item.get("RestartCount").and_then(Value::as_u64))
                .unwrap_or(0),
            exit_code: runtime
                .and_then(|item| item.pointer("/State/ExitCode").and_then(Value::as_i64)),
            oom_killed: runtime
                .and_then(|item| item.pointer("/State/OOMKilled").and_then(Value::as_bool))
                .unwrap_or(false),
            error: runtime
                .and_then(|item| item.pointer("/State/Error").and_then(Value::as_str))
                .filter(|text| !text.trim().is_empty())
                .map(str::to_string),
            ports: declared_ports(spec),
            mounts: declared_mounts(spec),
        });
    }
    facts.sort_by(|a, b| a.name.cmp(&b.name));

    ComposeFacts {
        project,
        services: facts,
        ps_error,
    }
}

/// 从渲染结果装配服务事实（声明态 + 容器运行态）。
fn collect_compose_facts(
    dir: &Path,
    global_args: &[std::ffi::OsString],
    secret_env: &[(String, String)],
    resolved: &Value,
) -> ComposeFacts {
    let (containers, ps_error) = compose_ps(dir, global_args, secret_env);
    let inspected = inspect_containers(containers.values());
    compose_facts(resolved, &containers, &inspected, ps_error)
}

/// 服务名 → 容器名（`docker compose ps`）。失败不影响其他检查，只记一条 `ps_error`。
fn compose_ps(
    dir: &Path,
    global_args: &[std::ffi::OsString],
    secret_env: &[(String, String)],
) -> (std::collections::BTreeMap<String, String>, Option<String>) {
    let out = std::collections::BTreeMap::new();
    let mut command = compose_command(
        dir,
        global_args,
        &["ps", "--all", "--format", "json"],
        secret_env,
    );
    let output = match command.output() {
        Ok(output) => output,
        Err(err) => return (out, Some(format!("无法执行 docker compose ps：{err}"))),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return (out, Some(format!("docker compose ps 失败：{stderr}")));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    (parse_ps(&json_items(&text)), None)
}

/// `docker compose ps --format json` 的条目 → 服务名 → 容器名。
fn parse_ps(items: &[Value]) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for item in items {
        let service = item
            .get("Service")
            .and_then(Value::as_str)
            .or_else(|| item.get("service").and_then(Value::as_str));
        let name = item
            .get("Name")
            .and_then(Value::as_str)
            .or_else(|| item.get("name").and_then(Value::as_str));
        if let (Some(service), Some(name)) = (service, name) {
            out.insert(service.to_string(), name.to_string());
        }
    }
    out
}

/// `docker inspect` 一次拿全部容器；失败返回空表（各服务的运行态即为未知）。
fn inspect_containers<'a>(
    names: impl Iterator<Item = &'a String>,
) -> std::collections::BTreeMap<String, Value> {
    let names: Vec<&String> = names.collect();
    if names.is_empty() {
        return std::collections::BTreeMap::new();
    }
    let mut command = Command::new("docker");
    command.arg("inspect");
    for name in names {
        command.arg(name);
    }
    let Ok(output) = command.output() else {
        return std::collections::BTreeMap::new();
    };
    if !output.status.success() {
        return std::collections::BTreeMap::new();
    }
    let Ok(array) = serde_json::from_slice::<Vec<Value>>(&output.stdout) else {
        return std::collections::BTreeMap::new();
    };
    let mut out = std::collections::BTreeMap::new();
    for item in array {
        if let Some(name) = item.get("Name").and_then(Value::as_str) {
            out.insert(name.trim_start_matches('/').to_string(), item);
        }
    }
    out
}

/// 解析 compose 的重启策略（短格式字符串 / 长格式对象）。
fn declared_restart(spec: &Value) -> Option<String> {
    match spec.get("restart") {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Object(map)) => map
            .get("Name")
            .or_else(|| map.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

fn field_str(item: &Value, what: &str) -> Option<String> {
    match what {
        "state" => item
            .pointer("/State/Status")
            .and_then(Value::as_str)
            .map(str::to_string),
        "health" => item
            .pointer("/State/Health/Status")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

/// 解析 compose 的端口声明（`"443:3000"` / `"127.0.0.1:19090:19090"` / 长格式对象）。
fn declared_ports(spec: &Value) -> Vec<PortFacts> {
    let mut out = Vec::new();
    let Some(ports) = spec.get("ports").and_then(Value::as_array) else {
        return out;
    };
    for item in ports {
        match item {
            Value::String(text) => {
                // `[host:]published:target[/proto]`；rsplit 后依次是 target / published / host。
                // （IPv6 里的冒号不在本函数范围内：compose 会把它渲染成长格式对象。）
                let body = text.split('/').next().unwrap_or(text);
                let parts: Vec<&str> = body.rsplit(':').collect();
                if let Some(target) = parts.first().and_then(|item| item.parse().ok()) {
                    out.push(PortFacts {
                        published: parts.get(1).map(|item| item.to_string()),
                        target,
                        host_ip: parts.get(2).map(|item| item.to_string()),
                        protocol: text.split('/').nth(1).map(str::to_string),
                    });
                }
            }
            Value::Object(map) => {
                let target = map.get("target").and_then(Value::as_u64).unwrap_or(0) as u16;
                out.push(PortFacts {
                    published: map.get("published").and_then(|value| match value {
                        Value::String(text) => Some(text.clone()),
                        Value::Number(number) => Some(number.to_string()),
                        _ => None,
                    }),
                    target,
                    host_ip: map
                        .get("host_ip")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    protocol: map
                        .get("protocol")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                });
            }
            _ => {}
        }
    }
    out
}

/// 解析 compose 的卷声明，并对 bind source 做宿主 `stat`。
fn declared_mounts(spec: &Value) -> Vec<MountFacts> {
    let mut out = Vec::new();
    let Some(volumes) = spec.get("volumes").and_then(Value::as_array) else {
        return out;
    };
    for item in volumes {
        let (kind, source, target, read_only) = match item {
            Value::String(text) => {
                let mut parts = text.split(':');
                let source = parts.next().unwrap_or_default().to_string();
                let target = parts.next().unwrap_or_default().to_string();
                let flags = parts.next().unwrap_or_default();
                let kind = if source.starts_with('/') || source.starts_with('.') {
                    "bind"
                } else {
                    "volume"
                };
                (kind.to_string(), source, target, flags.contains("ro"))
            }
            Value::Object(map) => {
                let kind = map
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("volume")
                    .to_string();
                (
                    kind,
                    map.get("source")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    map.get("target")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    map.get("read_only")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                        || map
                            .get("readOnly")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                )
            }
            _ => continue,
        };
        let stat = (kind == "bind").then(|| host_stat(Path::new(&source)));
        let (source_exists, owner, mode) = match stat.flatten() {
            Some(facts) => (Some(true), facts.0, facts.1),
            None if kind == "bind" => (Some(false), None, None),
            None => (None, None, None),
        };
        out.push(MountFacts {
            kind,
            source,
            target,
            read_only,
            source_exists,
            owner,
            mode,
        });
    }
    out
}

/// `(owner, mode)`；文件不存在返回 `None`。
#[cfg(unix)]
fn host_stat(path: &Path) -> Option<(Option<String>, Option<String>)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some((
        Some(format!("{}:{}", meta.uid(), meta.gid())),
        Some(format!("{:04o}", meta.mode() & 0o7777)),
    ))
}

#[cfg(not(unix))]
fn host_stat(path: &Path) -> Option<(Option<String>, Option<String>)> {
    std::fs::metadata(path).ok().map(|_| (None, None))
}

/// 解析「JSON 数组」或「一行一个 JSON 对象」（不同 compose 版本两种都有）。
fn json_items(text: &str) -> Vec<Value> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Ok(array) = serde_json::from_str::<Vec<Value>>(trimmed) {
        return array;
    }
    trimmed
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

/// 超过上限就折叠（契约 §5）。
fn fit_config(resolved: Value) -> Value {
    match serde_json::to_string(&resolved) {
        Ok(text) if text.len() > MAX_CONFIG_BYTES => json!({ "omitted": true }),
        _ => resolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_facts_merge_declared_and_runtime() {
        use std::collections::BTreeMap;
        let resolved = json!({
            "name": "stack",
            "services": {
                "b": {},
                "a": {
                    "image": "img:1",
                    "restart": "unless-stopped",
                    "ports": ["443:3000"],
                    "volumes": [
                        {"type": "bind", "source": "/srv/data", "target": "/data", "read_only": true}
                    ]
                }
            }
        });
        let mut containers = BTreeMap::new();
        containers.insert("a".to_string(), "stack-a-1".to_string());
        let mut inspected = BTreeMap::new();
        inspected.insert(
            "stack-a-1".to_string(),
            json!({
                "Name": "/stack-a-1",
                "RestartCount": 2,
                "State": {
                    "Status": "running", "ExitCode": 0, "OOMKilled": false, "Error": "",
                    "Health": {"Status": "healthy"}
                }
            }),
        );

        let facts = compose_facts(&resolved, &containers, &inspected, None);
        assert_eq!(facts.project.as_deref(), Some("stack"));
        assert_eq!(facts.services.len(), 2);
        // 按服务名排序（输出稳定）
        assert_eq!(facts.services[0].name, "a");
        assert_eq!(facts.services[0].container.as_deref(), Some("stack-a-1"));
        assert_eq!(facts.services[0].state.as_deref(), Some("running"));
        assert_eq!(facts.services[0].health.as_deref(), Some("healthy"));
        assert_eq!(facts.services[0].restarts, 2);
        assert_eq!(facts.services[0].restart.as_deref(), Some("unless-stopped"));
        assert_eq!(facts.services[0].ports[0].published.as_deref(), Some("443"));
        assert_eq!(facts.services[0].mounts.len(), 1);
        // 没有容器的服务：容器与运行态都是 None（不是“正常”）
        assert_eq!(facts.services[1].name, "b");
        assert!(facts.services[1].container.is_none());
        assert!(facts.services[1].state.is_none());
    }

    #[test]
    fn compose_facts_keep_ps_error_for_the_report() {
        let facts = compose_facts(
            &json!({"services": {}}),
            &Default::default(),
            &Default::default(),
            Some("docker compose ps failed".to_string()),
        );
        assert!(facts.services.is_empty());
        assert_eq!(facts.ps_error.as_deref(), Some("docker compose ps failed"));
    }

    #[test]
    fn parse_ps_accepts_capitalized_and_lowercase_keys() {
        let items = vec![
            json!({"Service": "a", "Name": "stack-a-1"}),
            json!({"service": "b", "name": "stack-b-1"}),
            json!({"Name": "no-service"}),
        ];
        let map = parse_ps(&items);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("a").map(String::as_str), Some("stack-a-1"));
        assert_eq!(map.get("b").map(String::as_str), Some("stack-b-1"));
    }

    #[test]
    fn restart_accepts_long_form_objects() {
        assert_eq!(
            declared_restart(&json!({"restart": {"Name": "no", "MaximumRetryCount": 0}}))
                .as_deref(),
            Some("no")
        );
        assert_eq!(declared_restart(&json!({})), None);
    }

    #[test]
    fn ports_parse_short_syntax() {
        let spec = json!({"ports": ["443:3000", "127.0.0.1:19090:19090"]});
        let ports = declared_ports(&spec);
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].published.as_deref(), Some("443"));
        assert_eq!(ports[0].target, 3000);
        assert_eq!(ports[1].target, 19090);
    }

    #[test]
    fn ports_parse_long_syntax() {
        let spec = json!({"ports": [{"target": 9000, "published": "9000", "protocol": "tcp"}]});
        let ports = declared_ports(&spec);
        assert_eq!(ports[0].target, 9000);
        assert_eq!(ports[0].published.as_deref(), Some("9000"));
    }

    #[test]
    fn mounts_distinguish_bind_and_volume_and_stat_the_host() {
        let dir = std::env::temp_dir().join(format!("gops-diag-mount-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let spec = json!({
            "volumes": [
                { "type": "bind", "source": dir.display().to_string(), "target": "/data",
                  "read_only": false },
                { "type": "bind", "source": "/definitely/missing", "target": "/x" },
                { "type": "volume", "source": "vol", "target": "/v" }
            ]
        });
        let mounts = declared_mounts(&spec);
        assert_eq!(mounts[0].source_exists, Some(true));
        assert_eq!(mounts[1].source_exists, Some(false));
        assert_eq!(mounts[2].kind, "volume");
        assert_eq!(mounts[2].source_exists, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_items_accepts_array_and_jsonl() {
        assert_eq!(json_items("[{\"a\":1},{\"a\":2}]").len(), 2);
        assert_eq!(json_items("{\"a\":1}\n{\"a\":2}\n").len(), 2);
        assert!(json_items("  ").is_empty());
    }

    #[test]
    fn oversized_config_is_collapsed() {
        let big = json!({ "blob": "x".repeat(MAX_CONFIG_BYTES + 8) });
        assert_eq!(fit_config(big)["omitted"], json!(true));
    }

    #[test]
    fn minimal_context_has_no_compose() {
        let context = Context::minimal(SystemInfo {
            name: "s".into(),
            kind: "gxl".into(),
            root: "/tmp/s".into(),
            env: "default".into(),
        });
        let value = context.to_json();
        assert_eq!(value["schema_version"], json!("1"));
        assert!(value.get("compose").is_none());
        assert_eq!(value["system"]["kind"], json!("gxl"));
    }
}
