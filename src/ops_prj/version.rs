//! 版本 spec 与版本目录（catalog）。
//!
//! `gops prj upgrade` 的 `-v/--version` / 位置参数支持：
//! - `latest`：当前 channel 内最高版本；
//! - `patch`：同 `major.minor` 的下一个可用 patch；
//! - `minor` / `feat`：同 `major` 的下一个可用 minor；
//! - `major`：下一个可用 major；
//! - 精确版本：直接按 `addr` 的 `{version}` 模板渲染。
//!
//! 关键字 spec 不能靠“版本号算术 +1”：必须从**可用版本列表**里选，否则会拿一个不存在的
//! 版本去下载。本模块负责从常见来源取列表：
//! - 本地包目录（`addr` 是本地路径模板）；
//! - GitHub Releases（`https://github.com/<owner>/<repo>/releases/...`）；
//! - git 仓库 tag（`git@...` / `*.git`）；
//! - 通用 HTTP 版本清单（版本段父目录的 `versions.json` / `versions.txt`）。
//!
//! 普通 HTTP 包地址（COS/S3/OSS/nginx）没有清单时，`patch`/`minor`/`major` 会按语义化
//! 版本规则生成候选地址并探测是否存在（有界窗口）；`latest` 没有天然停止条件，仍需要清单。

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::error::{MainError, MainReason};
use crate::ops_prj::prelude::*;
use crate::ops_prj::system::{OpsSystem, VERSION_PLACEHOLDER};
use crate::self_update::ReleaseChannel;

/// `-v/--version` 的取值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VersionSpec {
    /// 使用 `ops-prj.yml` 里声明的 `version:`。
    Declared,
    Latest,
    Patch,
    Minor,
    Major,
    /// 精确版本（保留用户写法，只做首尾空白与前缀 `v` 的归一化）。
    Exact(String),
}

impl VersionSpec {
    /// 解析用户输入。关键字大小写不敏感；`feat` 是 `minor` 的别名。
    pub fn parse(input: &str) -> MainResult<Self> {
        let raw = input.trim();
        if raw.is_empty() {
            return Err(MainReason::logic_detail("版本 spec 不能为空"));
        }
        match raw.to_ascii_lowercase().as_str() {
            "latest" => Ok(Self::Latest),
            "patch" => Ok(Self::Patch),
            "minor" | "feat" => Ok(Self::Minor),
            "major" => Ok(Self::Major),
            _ => {
                validate_version_string(raw)?;
                Ok(Self::Exact(normalize_exact(raw)))
            }
        }
    }

    /// 是否是需要版本目录的关键字 spec。
    pub fn is_keyword(&self) -> bool {
        matches!(self, Self::Latest | Self::Patch | Self::Minor | Self::Major)
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Declared => "declared",
            Self::Latest => "latest",
            Self::Patch => "patch",
            Self::Minor => "minor",
            Self::Major => "major",
            Self::Exact(v) => v,
        }
    }

    /// 位置参数是不是“像版本 spec”。用于区分 `upgrade <sys>` 与 `upgrade <spec>`。
    pub fn looks_like(input: &str) -> bool {
        let raw = input.trim();
        if matches!(
            raw.to_ascii_lowercase().as_str(),
            "latest" | "patch" | "minor" | "feat" | "major"
        ) {
            return true;
        }
        let no_v = raw
            .strip_prefix('v')
            .or_else(|| raw.strip_prefix('V'))
            .unwrap_or(raw);
        no_v.chars().next().is_some_and(|c| c.is_ascii_digit())
    }
}

/// 比较两个版本字符串是否相同（忽略首尾空白与可选的 `v` 前缀）。
pub fn same_version(a: &str, b: &str) -> bool {
    let a = normalize_exact(a);
    let b = normalize_exact(b);
    !a.is_empty() && a == b
}

/// 去掉常见的前缀 `v`（`v0.10.0` → `0.10.0`），其余原样保留。
pub fn normalize_exact(input: &str) -> String {
    let raw = input.trim();
    let stripped = raw
        .strip_prefix('v')
        .or_else(|| raw.strip_prefix('V'))
        .filter(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or(raw);
    stripped.to_string()
}

fn validate_version_string(input: &str) -> MainResult<()> {
    let ok = !input.is_empty()
        && !input.contains("..")
        && input != "."
        && input
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_'));
    if ok {
        return Ok(());
    }
    Err(MainReason::logic_detail(format!(
        "无效版本 `{input}`：只允许字母、数字、`.`、`-`、`+`、`_`，且不能包含 `..`；\
         路径/URL 请用 `--to`。"
    )))
}

/// 版本目录里的一条可用版本。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionEntry {
    pub version: semver::Version,
}

impl VersionEntry {
    pub fn new(version: semver::Version) -> Self {
        Self { version }
    }

    /// 通道由 semver 预发布后缀决定：无后缀 = stable，`-alpha*` = alpha，`-beta*` = beta。
    pub fn channel(&self) -> Option<ReleaseChannel> {
        channel_of(&self.version)
    }
}

pub fn channel_of(version: &semver::Version) -> Option<ReleaseChannel> {
    if version.pre.is_empty() {
        return Some(ReleaseChannel::Stable);
    }
    let pre = version.pre.as_str().to_ascii_lowercase();
    if pre.starts_with("alpha") {
        Some(ReleaseChannel::Alpha)
    } else if pre.starts_with("beta") {
        Some(ReleaseChannel::Beta)
    } else {
        None
    }
}

/// 从当前部署版本推断通道（用于 `--channel` 缺省值）。
pub fn infer_channel(version: &str) -> ReleaseChannel {
    let raw = version.trim();
    if raw.is_empty() {
        return ReleaseChannel::Stable;
    }
    let no_v = raw
        .strip_prefix('v')
        .or_else(|| raw.strip_prefix('V'))
        .unwrap_or(raw);
    semver::Version::parse(no_v)
        .ok()
        .and_then(|v| channel_of(&v))
        .unwrap_or(ReleaseChannel::Stable)
}

/// 列出某系统可用的版本。
pub async fn list_versions(project_root: &Path, sys: &OpsSystem) -> MainResult<Vec<VersionEntry>> {
    try_list_versions(project_root, sys)
        .await?
        .ok_or_else(|| no_catalog_error(sys))
}

/// 尝试从可枚举来源取版本；普通 HTTP 且没有版本清单时返回 `None`，
/// 由调用方决定是否按 `patch`/`minor`/`major` 规则做 URL 探测。
pub async fn try_list_versions(
    project_root: &Path,
    sys: &OpsSystem,
) -> MainResult<Option<Vec<VersionEntry>>> {
    let template = sys.addr_template();
    if !template.contains(VERSION_PLACEHOLDER) {
        return Err(MainReason::logic_detail(format!(
            "系统 {} 的 addr 没有 {VERSION_PLACEHOLDER} 占位，无法按版本解析 latest/patch/minor/major。\n  \
             在 addr 里写 {VERSION_PLACEHOLDER}，或用 `--to <完整地址>`。",
            sys.sys().name()
        )));
    }

    if template.starts_with("git@") || template.ends_with(".git") {
        return Ok(Some(list_git_tags(&template).await?));
    }
    if template.starts_with("http://") || template.starts_with("https://") {
        // 通用 HTTP 版本目录：在版本段父目录放 versions.json / versions.txt。
        // 这样不绑定 GitHub，普通对象存储（COS/S3/OSS/nginx）也能枚举版本。
        if let Some(base) = http_version_base(&template)
            && let Some(entries) = fetch_version_manifest(&base).await?
        {
            return Ok(Some(entries));
        }
        if let Some((owner, repo)) = github_repo(&template) {
            return Ok(Some(list_github_releases(&owner, &repo).await?));
        }
        return Ok(None);
    }

    Ok(Some(list_local_versions(project_root, &template)?))
}

const MAX_PATCH_PROBES: u64 = 20;
const MAX_MINOR_PROBES: u64 = 5;
const MAX_MAJOR_PROBES: u64 = 3;
const MAX_MINOR_PER_MAJOR: u64 = 2;
const MAX_PATCH_PER_MINOR: u64 = 5;
/// 并发探测数。对象存储上 404 的响应通常 100~500ms，串行扫窗口会明显拖慢升级；
/// 8 路并发既能把最坏情况压到 1~2s，也不至于触发对端限流。
const PROBE_CONCURRENCY: usize = 8;
/// 探测用的单请求超时。比正常下载的 20s 短，避免个别挂起的候选拖住整批。
const PROBE_TIMEOUT_SECS: u64 = 10;

/// 没有版本清单时，按版本规则生成候选 URL 并 HEAD 探测。
///
/// - `patch`：同 major.minor，patch+1..+N，取存在的最小 patch；
/// - `minor`：major 不变，minor+1..+N，每个 minor 从 patch 0 开始探测；
/// - `major`：major+1..+N，minor/patch 从 0 开始探测。
///
/// 候选按版本升序分批（每批 [`PROBE_CONCURRENCY`] 个）并发探测；一批里只要有命中，
/// 就取该批最小版本并立即结束，因此正常“下一个版本”场景只发 1 批请求。
///
/// `latest` 没有天然停止条件，仍要求版本清单 / GitHub / git / 本地目录。
pub async fn probe_versions(
    sys: &OpsSystem,
    spec: &VersionSpec,
    current: &str,
    channel: ReleaseChannel,
) -> MainResult<Vec<VersionEntry>> {
    match spec {
        VersionSpec::Latest => {
            return Err(MainReason::logic_detail(format!(
                "系统 {} 的 latest 需要可枚举的版本清单（versions.json / versions.txt、\
                 GitHub Releases、git tag 或本地目录）；\n  \
                 `patch` / `minor` / `major` 可以按规则做 URL 探测。",
                sys.sys().name()
            )));
        }
        VersionSpec::Patch | VersionSpec::Minor | VersionSpec::Major => {}
        _ => {
            return Err(MainReason::logic_detail(
                "内部错误：只有 patch/minor/major 支持 URL 探测",
            ));
        }
    }

    if current.trim().is_empty() {
        return Err(MainReason::logic_detail(format!(
            "系统 {} 的现场版本未知（version.txt / deliver.lock 都没有版本），\
             无法按 {} 探测下一个版本；\n  \
             用 `-v <精确版本>` 或 `--to <完整地址>`。",
            sys.sys().name(),
            spec.as_str()
        )));
    }
    let current = parse_current(current)?;
    let suffix = channel_suffix(channel);
    let candidates = candidate_versions(&current, spec, suffix)?;
    let client = reqwest::Client::builder()
        .user_agent("gops")
        .timeout(Duration::from_secs(PROBE_TIMEOUT_SECS))
        .build()
        .map_err(|e| MainReason::resource_detail(format!("创建 HTTP 客户端失败：{e}")))?;

    for chunk in candidates.chunks(PROBE_CONCURRENCY) {
        let mut tasks = tokio::task::JoinSet::new();
        for version in chunk {
            let client = client.clone();
            let url = sys.resolved_addr_with(&version.to_string())?;
            let version = version.clone();
            tasks.spawn(async move {
                let exists = http_object_exists(&client, &url).await?;
                Ok::<_, MainError>((version, exists))
            });
        }

        let mut hits = Vec::new();
        let mut first_error = None;
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(Ok((version, true))) => hits.push(version),
                Ok(Ok((_, false))) => {}
                Ok(Err(err)) => {
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                }
                Err(err) => {
                    if first_error.is_none() {
                        first_error =
                            Some(MainReason::logic_detail(format!("探测任务异常：{err}")));
                    }
                }
            }
        }

        // 同一批里已有命中时，个别候选的网络/权限错误不影响结果。
        if let Some(version) = hits.into_iter().min() {
            return Ok(vec![VersionEntry::new(version)]);
        }
        if let Some(err) = first_error {
            return Err(err);
        }
    }

    Err(MainReason::logic_detail(format!(
        "按 URL 探测没有找到 {} 版本（当前 {}，通道 {}，窗口 {} 个候选）；\n  \
         可发布 versions.json / versions.txt，或用 `-v <精确版本>` / `--to <完整地址>`。",
        spec.as_str(),
        current,
        channel.as_str(),
        candidates.len()
    )))
}

/// 按 spec 生成**升序**候选版本列表（不重复、不含当前版本）。
fn candidate_versions(
    current: &semver::Version,
    spec: &VersionSpec,
    suffix: &str,
) -> MainResult<Vec<semver::Version>> {
    let build = |major: u64, minor: u64, patch: u64| -> MainResult<semver::Version> {
        let raw = format!("{major}.{minor}.{patch}{suffix}");
        semver::Version::parse(&raw)
            .map_err(|e| MainReason::logic_detail(format!("生成探测版本失败 `{raw}`：{e}")))
    };

    let mut out = Vec::new();
    match spec {
        VersionSpec::Patch => {
            for offset in 1..=MAX_PATCH_PROBES {
                out.push(build(
                    current.major,
                    current.minor,
                    current.patch.saturating_add(offset),
                )?);
            }
        }
        VersionSpec::Minor => {
            for minor_offset in 1..=MAX_MINOR_PROBES {
                let minor = current.minor.saturating_add(minor_offset);
                for patch in 0..=MAX_PATCH_PER_MINOR {
                    out.push(build(current.major, minor, patch)?);
                }
            }
        }
        VersionSpec::Major => {
            for major_offset in 1..=MAX_MAJOR_PROBES {
                let major = current.major.saturating_add(major_offset);
                // 兼容“major 版本从 1.1.0 开始”的情况：minor/patch 都做有界扫描。
                for minor in 0..=MAX_MINOR_PER_MAJOR {
                    for patch in 0..=MAX_PATCH_PER_MINOR {
                        out.push(build(major, minor, patch)?);
                    }
                }
            }
        }
        _ => {
            return Err(MainReason::logic_detail(
                "内部错误：只有 patch/minor/major 支持 URL 探测",
            ));
        }
    }
    Ok(out)
}

fn channel_suffix(channel: ReleaseChannel) -> &'static str {
    match channel {
        ReleaseChannel::Stable => "",
        ReleaseChannel::Alpha => "-alpha",
        ReleaseChannel::Beta => "-beta",
    }
}

async fn http_object_exists(client: &reqwest::Client, url: &str) -> MainResult<bool> {
    let mut head = client.head(url);
    if let Some(token) = github_token_for(url) {
        head = head.bearer_auth(token);
    }
    let resp = head
        .send()
        .await
        .map_err(|e| MainReason::resource_detail(format!("探测目标失败（HEAD）{url}: {e}")))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(true);
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }

    // 部分对象存储/静态服务器不接受 HEAD；用最小 Range GET 再确认。
    if matches!(status.as_u16(), 403 | 405 | 501) {
        let mut get = client.get(url).header(reqwest::header::RANGE, "bytes=0-0");
        if let Some(token) = github_token_for(url) {
            get = get.bearer_auth(token);
        }
        let resp = get.send().await.map_err(|e| {
            MainReason::resource_detail(format!("探测目标失败（GET Range）{url}: {e}"))
        })?;
        let status = resp.status();
        if status.is_success() {
            return Ok(true);
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        return Err(MainReason::resource_detail(format!(
            "探测目标失败：{url}\n  GET Range → HTTP {status}"
        )));
    }

    Err(MainReason::resource_detail(format!(
        "探测目标失败：{url}\n  HEAD → HTTP {status}"
    )))
}

fn github_token_for(url: &str) -> Option<String> {
    let is_github = url.starts_with("https://github.com/")
        || url.starts_with("http://github.com/")
        || url.starts_with("https://api.github.com/");
    if is_github { github_token() } else { None }
}

fn no_catalog_error(sys: &OpsSystem) -> MainError {
    let template = sys.addr_template();
    let hint = http_version_base(&template)
        .map(|base| format!("{base}versions.json（或 versions.txt）"))
        .unwrap_or_else(|| "<addr 版本段父目录>/versions.json（或 versions.txt）".to_string());
    MainReason::logic_detail(format!(
        "系统 {} 的 addr 是普通 HTTP 地址，gops 无法自动枚举可用版本。\n  \
         可选：\n  \
         1) 在 {hint} 发布版本清单；\n  \
         2) 用 `patch`/`minor`/`major` 按规则探测（需要当前版本已知）；\n  \
         3) 用 `-v <精确版本>` 或 `--to <完整地址>`；\n  \
         4) 把 addr 指向 GitHub Releases / git tag。",
        sys.sys().name()
    ))
}

/// 从版本目录里按 spec 选择目标版本。
///
/// - `Latest` 取通道内最高版本；
/// - `Patch` / `Minor` / `Major` 取**大于当前版本的最小可用版本**；
/// - 精确版本与 `Declared` 不经过这里（由运行时直接渲染）。
pub fn select_version(
    entries: &[VersionEntry],
    spec: &VersionSpec,
    current: Option<&str>,
    channel: ReleaseChannel,
) -> MainResult<VersionEntry> {
    let candidates: Vec<&VersionEntry> = entries
        .iter()
        .filter(|e| e.channel() == Some(channel))
        .collect();

    match spec {
        VersionSpec::Latest => {
            let selected = candidates
                .into_iter()
                .max_by(|a, b| a.version.cmp(&b.version))
                .cloned()
                .ok_or_else(|| {
                    MainReason::logic_detail(format!(
                        "通道 {} 没有可用版本（版本目录为空或全部是其它通道）",
                        channel.as_str()
                    ))
                })?;
            // 默认不降级：`latest` 解析到比现场更低的版本时，多半是通道/仓库选错了。
            if let Some(current) = current
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .and_then(|v| parse_current(v).ok())
                && selected.version < current
            {
                return Err(MainReason::logic_detail(format!(
                    "通道 {} 的最新版本 {} 低于当前部署版本 {}，拒绝隐式降级；\n  \
                     如确需降级，请用精确版本（-v {}）或检查 addr/通道。",
                    channel.as_str(),
                    selected.version,
                    current,
                    selected.version
                )));
            }
            Ok(selected)
        }
        VersionSpec::Patch | VersionSpec::Minor | VersionSpec::Major => {
            let current = current.ok_or_else(|| {
                MainReason::logic_detail(format!(
                    "无法解析 {}：当前部署版本未知（{} 与 deliver.lock 都没有版本）",
                    spec.as_str(),
                    "version.txt"
                ))
            })?;
            let current = parse_current(current)?;
            let next = candidates
                .into_iter()
                .filter(|e| matches_bump(&e.version, &current, spec))
                .min_by(|a, b| a.version.cmp(&b.version))
                .cloned();
            next.ok_or_else(|| {
                MainReason::logic_detail(format!(
                    "通道 {} 没有比当前版本 {} 更合适的 {} 版本",
                    channel.as_str(),
                    current,
                    spec.as_str()
                ))
            })
        }
        VersionSpec::Declared | VersionSpec::Exact(_) => Err(MainReason::logic_detail(
            "内部错误：精确版本/声明版本不应走版本目录选择",
        )),
    }
}

fn parse_current(input: &str) -> MainResult<semver::Version> {
    let raw = input.trim();
    let no_v = raw
        .strip_prefix('v')
        .or_else(|| raw.strip_prefix('V'))
        .unwrap_or(raw);
    semver::Version::parse(no_v).map_err(|e| {
        MainReason::logic_detail(format!(
            "当前版本 `{input}` 不是 semver，无法解析 patch/minor/major：{e}"
        ))
    })
}

fn matches_bump(current: &semver::Version, base: &semver::Version, spec: &VersionSpec) -> bool {
    match spec {
        VersionSpec::Patch => {
            current.major == base.major && current.minor == base.minor && current.patch > base.patch
        }
        VersionSpec::Minor => current.major == base.major && current.minor > base.minor,
        VersionSpec::Major => current.major > base.major,
        _ => false,
    }
}

fn list_local_versions(project_root: &Path, template: &str) -> MainResult<Vec<VersionEntry>> {
    let path = Path::new(template);
    let file_name = path
        .file_name()
        .ok_or_else(|| MainReason::logic_detail(format!("本地 addr 缺少文件名：{template}")))?;
    let file_name = file_name.to_string_lossy();
    if file_name.matches(VERSION_PLACEHOLDER).count() != 1 {
        return Err(MainReason::logic_detail(format!(
            "本地 addr 的文件名必须包含且只包含一个 {VERSION_PLACEHOLDER}：{template}"
        )));
    }
    let (prefix, suffix) = file_name
        .split_once(VERSION_PLACEHOLDER)
        .ok_or_else(|| MainReason::logic_detail(format!("无法解析本地 addr 模板：{template}")))?;

    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            if parent.is_absolute() {
                parent.to_path_buf()
            } else {
                project_root.join(parent)
            }
        }
        _ => project_root.to_path_buf(),
    };

    let mut entries = Vec::new();
    let read = std::fs::read_dir(&dir).map_err(|e| {
        MainReason::resource_detail(format!(
            "读取版本目录失败 {}: {e}；本地 addr={template}",
            dir.display()
        ))
    })?;
    for entry in read {
        let entry = entry.map_err(|e| {
            MainReason::resource_detail(format!("读取版本目录项失败 {}: {e}", dir.display()))
        })?;
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(version) = name
            .strip_prefix(prefix)
            .and_then(|s| s.strip_suffix(suffix))
            .filter(|v| !v.is_empty())
        else {
            continue;
        };
        if let Ok(version) = semver::Version::parse(version) {
            entries.push(VersionEntry::new(version));
        }
    }

    entries.sort_by(|a, b| a.version.cmp(&b.version));
    entries.dedup();
    if entries.is_empty() {
        return Err(MainReason::resource_detail(format!(
            "本地版本目录里没有匹配 `{prefix}{VERSION_PLACEHOLDER}{suffix}` 的包：{}",
            dir.display()
        )));
    }
    Ok(entries)
}

/// GitHub API/Release 鉴权 token（`GITHUB_TOKEN` 优先，其次 `GH_TOKEN`）。
pub fn github_token() -> Option<String> {
    std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("GH_TOKEN"))
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

/// 从 HTTP 模板里取“版本段父目录”，作为版本清单的落点。
///
/// 例：`https://host/wist-center-stack/v{version}/x.tar.gz`
///     → `https://host/wist-center-stack/`
///     `https://host/pkgs/s-v{version}.tar.gz`
///     → `https://host/pkgs/`
fn http_version_base(template: &str) -> Option<String> {
    let scheme_end = template.find("://")?;
    let scheme = &template[..scheme_end];
    let rest = &template[scheme_end + 3..];
    let slash = rest.find('/')?;
    let authority = &rest[..slash];
    let path = &rest[slash..];
    let segments: Vec<&str> = path.split('/').collect();
    let version_idx = segments
        .iter()
        .position(|segment| segment.contains(VERSION_PLACEHOLDER))?;

    let mut base = format!("{scheme}://{authority}");
    for segment in &segments[..version_idx] {
        if !segment.is_empty() {
            base.push('/');
            base.push_str(segment);
        }
    }
    base.push('/');
    Some(base)
}

/// 读取通用 HTTP 版本清单：优先 `versions.json`，其次 `versions.txt`。
async fn fetch_version_manifest(base: &str) -> MainResult<Option<Vec<VersionEntry>>> {
    let client = reqwest::Client::builder()
        .user_agent("gops")
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| MainReason::resource_detail(format!("创建 HTTP 客户端失败：{e}")))?;

    for (file, is_json) in [("versions.json", true), ("versions.txt", false)] {
        let url = format!("{base}{file}");
        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| MainReason::resource_detail(format!("读取版本清单失败 {url}: {e}")))?;
        let status = resp.status();
        // 清单是可选增强：部分对象存储对不存在的对象返回 401/403，
        // 不能因此拦住后面的 URL 探测；真不存在与无权限在这里等价。
        if status == reqwest::StatusCode::NOT_FOUND || matches!(status.as_u16(), 401 | 403) {
            continue;
        }
        if !status.is_success() {
            return Err(MainReason::resource_detail(format!(
                "读取版本清单失败：{url}\n  GET → HTTP {status}（清单需要公开可读）"
            )));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| MainReason::resource_detail(format!("读取版本清单内容失败 {url}: {e}")))?;
        let entries = if is_json {
            parse_manifest_json(&body)?
        } else {
            parse_manifest_text(&body)?
        };
        return Ok(Some(entries));
    }
    Ok(None)
}

fn parse_manifest_json(body: &str) -> MainResult<Vec<VersionEntry>> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| MainReason::resource_detail(format!("解析 versions.json 失败：{e}")))?;
    let items = match &value {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Object(obj) => obj
            .get("versions")
            .and_then(|value| value.as_array())
            .ok_or_else(|| {
                MainReason::resource_detail(
                    "versions.json 需要是数组，或包含 `versions` 数组的对象",
                )
            })?,
        _ => {
            return Err(MainReason::resource_detail(
                "versions.json 需要是数组，或包含 `versions` 数组的对象",
            ));
        }
    };

    let mut entries = Vec::new();
    for item in items {
        let raw = match item {
            serde_json::Value::String(value) => value.as_str(),
            serde_json::Value::Object(obj) => obj
                .get("version")
                .and_then(|value| value.as_str())
                .unwrap_or_default(),
            _ => continue,
        };
        if let Ok(version) = semver::Version::parse(&normalize_exact(raw)) {
            entries.push(VersionEntry::new(version));
        }
    }
    normalize_entries(entries)
}

fn parse_manifest_text(body: &str) -> MainResult<Vec<VersionEntry>> {
    let mut entries = Vec::new();
    for line in body.lines() {
        let raw = line.split('#').next().unwrap_or("").trim();
        if raw.is_empty() {
            continue;
        }
        if let Ok(version) = semver::Version::parse(&normalize_exact(raw)) {
            entries.push(VersionEntry::new(version));
        }
    }
    normalize_entries(entries)
}

fn normalize_entries(mut entries: Vec<VersionEntry>) -> MainResult<Vec<VersionEntry>> {
    entries.sort_by(|a, b| a.version.cmp(&b.version));
    entries.dedup();
    if entries.is_empty() {
        return Err(MainReason::resource_detail(
            "版本清单没有合法的 semver 版本（每行/每项一个版本）",
        ));
    }
    Ok(entries)
}

/// 从 GitHub 地址里取 `owner/repo`。
pub fn github_repo(template: &str) -> Option<(String, String)> {
    let rest = template
        .strip_prefix("https://github.com/")
        .or_else(|| template.strip_prefix("http://github.com/"))
        .or_else(|| template.strip_prefix("https://www.github.com/"))
        .or_else(|| template.strip_prefix("http://www.github.com/"))?;
    let mut parts = rest.split('/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim().trim_end_matches(".git");
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
}

async fn list_github_releases(owner: &str, repo: &str) -> MainResult<Vec<VersionEntry>> {
    let api = format!("https://api.github.com/repos/{owner}/{repo}/releases?per_page=100");
    let client = reqwest::Client::builder()
        .user_agent("gops")
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| MainReason::resource_detail(format!("创建 HTTP 客户端失败：{e}")))?;
    let mut req = client
        .get(&api)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json");
    if let Some(token) = github_token() {
        req = req.bearer_auth(token);
    }

    let resp = req.send().await.map_err(|e| {
        MainReason::resource_detail(format!("请求 GitHub Releases 失败 {api}: {e}"))
    })?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| MainReason::resource_detail(format!("读取 GitHub 响应失败：{e}")))?;
    if !status.is_success() {
        let hint = match status.as_u16() {
            401 | 403 => "（可能是限流或私有仓；可设置 GITHUB_TOKEN）",
            404 => "（仓库不存在或无权访问）",
            _ => "",
        };
        return Err(MainReason::resource_detail(format!(
            "GitHub Releases 请求失败 {status}{hint}: {api}\n  {}",
            body.chars().take(300).collect::<String>()
        )));
    }

    parse_github_releases(&body).map_err(|e| {
        MainReason::resource_detail(format!(
            "GitHub 仓库 {owner}/{repo} 的 release 列表不可用：{e}"
        ))
    })
}

fn parse_github_releases(body: &str) -> MainResult<Vec<VersionEntry>> {
    let releases: Vec<GhRelease> = serde_json::from_str(body)
        .map_err(|e| MainReason::resource_detail(format!("解析 GitHub Releases 响应失败：{e}")))?;
    let mut entries = Vec::new();
    for release in releases {
        if release.draft {
            continue;
        }
        let tag = release
            .tag_name
            .strip_prefix('v')
            .or_else(|| release.tag_name.strip_prefix('V'))
            .unwrap_or(&release.tag_name);
        if let Ok(version) = semver::Version::parse(tag) {
            entries.push(VersionEntry::new(version));
        }
    }
    entries.sort_by(|a, b| a.version.cmp(&b.version));
    entries.dedup();
    if entries.is_empty() {
        return Err(MainReason::resource_detail(
            "没有可用的 release tag（需要形如 v1.2.3 / v1.2.3-alpha）",
        ));
    }
    Ok(entries)
}

async fn list_git_tags(addr: &str) -> MainResult<Vec<VersionEntry>> {
    let mut command = tokio::process::Command::new("git");
    command
        .args(["ls-remote", "--tags", addr])
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .map_err(|_| {
            MainReason::resource_detail(format!("git ls-remote --tags 超时（30s）：{addr}"))
        })?
        .map_err(|e| MainReason::resource_detail(format!("执行 git ls-remote 失败：{e}")))?;
    if !out.status.success() {
        return Err(MainReason::resource_detail(format!(
            "git ls-remote --tags 失败（{addr}）：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }

    let mut entries = parse_git_tags(&String::from_utf8_lossy(&out.stdout));
    entries.sort_by(|a, b| a.version.cmp(&b.version));
    entries.dedup();
    if entries.is_empty() {
        return Err(MainReason::resource_detail(format!(
            "git 仓库 {addr} 没有形如 v1.2.3 / v1.2.3-alpha 的 tag"
        )));
    }
    Ok(entries)
}

fn parse_git_tags(output: &str) -> Vec<VersionEntry> {
    let mut entries = Vec::new();
    for line in output.lines() {
        let Some((_, refname)) = line.split_once('\t') else {
            continue;
        };
        let Some(tag) = refname.strip_prefix("refs/tags/") else {
            continue;
        };
        let tag = tag.strip_suffix("^{}").unwrap_or(tag);
        let tag = tag
            .strip_prefix('v')
            .or_else(|| tag.strip_prefix('V'))
            .unwrap_or(tag);
        if let Ok(version) = semver::Version::parse(tag) {
            entries.push(VersionEntry::new(version));
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entries(specs: &[&str]) -> Vec<VersionEntry> {
        specs
            .iter()
            .map(|s| VersionEntry::new(semver::Version::parse(s).unwrap()))
            .collect()
    }

    #[test]
    fn version_spec_parses_keywords_aliases_and_exact() {
        assert_eq!(VersionSpec::parse("latest").unwrap(), VersionSpec::Latest);
        assert_eq!(VersionSpec::parse(" PATCH ").unwrap(), VersionSpec::Patch);
        assert_eq!(VersionSpec::parse("minor").unwrap(), VersionSpec::Minor);
        assert_eq!(VersionSpec::parse("feat").unwrap(), VersionSpec::Minor);
        assert_eq!(VersionSpec::parse("major").unwrap(), VersionSpec::Major);
        assert_eq!(
            VersionSpec::parse("v0.10.0-alpha").unwrap(),
            VersionSpec::Exact("0.10.0-alpha".to_string())
        );
        assert!(VersionSpec::parse("").is_err());
        assert!(VersionSpec::parse("../../etc").is_err());
    }

    #[test]
    fn version_spec_rejects_path_traversal_and_bad_chars() {
        for bad in [
            "", " ", ".", "..", "1..2", "../x", "a/b", "a b", "a?b", "a#b", "a:b",
        ] {
            assert!(VersionSpec::parse(bad).is_err(), "{bad:?}");
        }
        // 合法的 semver 预发布 / 构建元数据仍可用。
        assert!(VersionSpec::parse("1.2.3-alpha.1+build.5").is_ok());
    }

    #[test]
    fn same_version_ignores_whitespace_and_v_prefix() {
        assert!(same_version("0.10.0", "0.10.0"));
        assert!(same_version(" v0.10.0 ", "0.10.0"));
        assert!(same_version("0.10.0", "V0.10.0"));
        assert!(!same_version("0.10.0", "0.10.1"));
        assert!(!same_version("", "0.10.0"));
        assert!(!same_version("  ", ""));
    }

    #[test]
    fn normalize_exact_strips_only_version_v_prefix() {
        assert_eq!(normalize_exact("v1.2.3"), "1.2.3");
        assert_eq!(normalize_exact("V1.2.3"), "1.2.3");
        assert_eq!(normalize_exact("  v1.2.3  "), "1.2.3");
        assert_eq!(normalize_exact("1.2.3"), "1.2.3");
        // `v` 后面不是数字时不当版本前缀处理。
        assert_eq!(normalize_exact("vNext"), "vNext");
    }

    #[test]
    fn looks_like_spec_distinguishes_system_names() {
        for spec in ["latest", "patch", "feat", "0.10.0", "v0.10.0", "1.2.3-rc.1"] {
            assert!(VersionSpec::looks_like(spec), "{spec}");
        }
        for name in ["web-stack", "example_sys_x", "latest-x"] {
            assert!(!VersionSpec::looks_like(name), "{name}");
        }
    }

    #[test]
    fn infer_channel_from_current_version() {
        assert_eq!(infer_channel("0.10.0"), ReleaseChannel::Stable);
        assert_eq!(infer_channel("0.10.0-alpha"), ReleaseChannel::Alpha);
        assert_eq!(infer_channel("v0.10.0-alpha.1"), ReleaseChannel::Alpha);
        assert_eq!(infer_channel("0.10.0-beta.2"), ReleaseChannel::Beta);
        assert_eq!(infer_channel(""), ReleaseChannel::Stable);
    }

    #[test]
    fn select_latest_filters_by_channel() {
        let all = entries(&["0.9.0", "0.10.0-alpha", "0.10.0", "0.11.0-beta"]);
        assert_eq!(
            select_version(&all, &VersionSpec::Latest, None, ReleaseChannel::Stable)
                .unwrap()
                .version
                .to_string(),
            "0.10.0"
        );
        assert_eq!(
            select_version(&all, &VersionSpec::Latest, None, ReleaseChannel::Alpha)
                .unwrap()
                .version
                .to_string(),
            "0.10.0-alpha"
        );
        assert_eq!(
            select_version(&all, &VersionSpec::Latest, None, ReleaseChannel::Beta)
                .unwrap()
                .version
                .to_string(),
            "0.11.0-beta"
        );
    }

    #[test]
    fn select_patch_minor_major_prefers_next_available() {
        let all = entries(&["0.9.0", "0.9.2", "0.10.0", "0.11.0", "1.0.0"]);
        assert_eq!(
            select_version(
                &all,
                &VersionSpec::Patch,
                Some("0.9.0"),
                ReleaseChannel::Stable
            )
            .unwrap()
            .version
            .to_string(),
            "0.9.2"
        );
        assert_eq!(
            select_version(
                &all,
                &VersionSpec::Minor,
                Some("0.9.0"),
                ReleaseChannel::Stable
            )
            .unwrap()
            .version
            .to_string(),
            "0.10.0"
        );
        assert_eq!(
            select_version(
                &all,
                &VersionSpec::Major,
                Some("0.9.0"),
                ReleaseChannel::Stable
            )
            .unwrap()
            .version
            .to_string(),
            "1.0.0"
        );
    }

    #[test]
    fn select_handles_v_prefix_and_rejects_nonsemver_current() {
        let all = entries(&["0.9.0", "0.9.1"]);
        assert_eq!(
            select_version(
                &all,
                &VersionSpec::Patch,
                Some("v0.9.0"),
                ReleaseChannel::Stable
            )
            .unwrap()
            .version
            .to_string(),
            "0.9.1"
        );
        assert!(
            select_version(
                &all,
                &VersionSpec::Patch,
                Some("build-1"),
                ReleaseChannel::Stable
            )
            .is_err()
        );
        assert!(select_version(&all, &VersionSpec::Minor, None, ReleaseChannel::Stable).is_err());
        assert!(
            select_version(
                &all,
                &VersionSpec::Patch,
                Some("   "),
                ReleaseChannel::Stable
            )
            .is_err()
        );
    }

    #[test]
    fn select_ignores_unknown_prerelease_channels() {
        let all = entries(&["0.10.0-rc.1"]);
        assert!(select_version(&all, &VersionSpec::Latest, None, ReleaseChannel::Stable).is_err());
        assert!(select_version(&all, &VersionSpec::Latest, None, ReleaseChannel::Alpha).is_err());
    }

    #[test]
    fn select_latest_refuses_implicit_downgrade() {
        let all = entries(&["0.9.0", "0.10.0"]);
        let err = select_version(
            &all,
            &VersionSpec::Latest,
            Some("0.11.0"),
            ReleaseChannel::Stable,
        )
        .unwrap_err();
        assert!(err.to_string().contains("拒绝隐式降级"), "{err}");
    }

    #[test]
    fn select_errors_when_no_candidate() {
        let all = entries(&["0.9.0"]);
        assert!(
            select_version(
                &all,
                &VersionSpec::Patch,
                Some("0.9.0"),
                ReleaseChannel::Stable
            )
            .is_err()
        );
        assert!(select_version(&all, &VersionSpec::Latest, None, ReleaseChannel::Alpha).is_err());
    }

    #[tokio::test]
    async fn list_local_versions_scans_matching_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        for name in [
            "s-v0.9.0.tar.gz",
            "s-v0.10.0-alpha.tar.gz",
            "s-v0.10.0.tar.gz",
            "other-v9.9.9.tar.gz",
            "s-vnot-a-version.tar.gz",
        ] {
            std::fs::write(root.join(name), "x").unwrap();
        }
        let sys = test_sys("s-v{version}.tar.gz");
        let versions = list_versions(root, &sys).await.unwrap();
        let names: Vec<String> = versions.iter().map(|v| v.version.to_string()).collect();
        assert_eq!(names, vec!["0.9.0", "0.10.0-alpha", "0.10.0"]);
    }

    #[tokio::test]
    async fn list_local_versions_supports_absolute_and_bare_templates() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("s-v1.0.0.tar.gz"), "x").unwrap();

        let bare = test_sys("s-v{version}.tar.gz");
        assert_eq!(
            list_versions(tmp.path(), &bare).await.unwrap()[0]
                .version
                .to_string(),
            "1.0.0"
        );

        let absolute = tmp.path().join("s-v{version}.tar.gz");
        let absolute = test_sys(&absolute.to_string_lossy());
        assert_eq!(
            list_versions(tmp.path(), &absolute).await.unwrap()[0]
                .version
                .to_string(),
            "1.0.0"
        );
    }

    #[tokio::test]
    async fn list_local_versions_rejects_bad_templates() {
        let tmp = TempDir::new().unwrap();

        // `{version}` 只在目录里，文件名里没有 → 无法从文件名解析版本。
        let no_file_placeholder = test_sys("v{version}/s.tar.gz");
        let err = list_versions(tmp.path(), &no_file_placeholder)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("文件名必须包含"), "{err}");

        // 文件名里多个占位符 → 拒绝，避免歧义。
        let two = test_sys("s-{version}-{version}.tar.gz");
        let err = list_versions(tmp.path(), &two).await.unwrap_err();
        assert!(err.to_string().contains("文件名必须包含"), "{err}");
    }

    #[tokio::test]
    async fn list_local_versions_reports_missing_directory() {
        let tmp = TempDir::new().unwrap();
        let sys = test_sys("nope/s-v{version}.tar.gz");
        let err = list_versions(tmp.path(), &sys).await.unwrap_err();
        assert!(err.to_string().contains("读取版本目录失败"), "{err}");
    }

    #[test]
    fn http_version_base_extracts_parent_of_version_segment() {
        assert_eq!(
            http_version_base("https://host/wist-center-stack/v{version}/x.tar.gz"),
            Some("https://host/wist-center-stack/".to_string())
        );
        assert_eq!(
            http_version_base("https://host/pkgs/s-v{version}.tar.gz"),
            Some("https://host/pkgs/".to_string())
        );
        assert_eq!(
            http_version_base("https://host/v{version}/x.tar.gz"),
            Some("https://host/".to_string())
        );
        assert_eq!(http_version_base("https://host/pkgs/s.tar.gz"), None);
    }

    #[test]
    fn parse_manifest_json_supports_array_and_object() {
        let body = r#"{"versions":["v0.1.9-alpha",{"version":"0.1.10-alpha"},"bad"]}"#;
        let versions: Vec<String> = parse_manifest_json(body)
            .unwrap()
            .iter()
            .map(|entry| entry.version.to_string())
            .collect();
        assert_eq!(versions, vec!["0.1.9-alpha", "0.1.10-alpha"]);

        let body = r#"["0.1.9-alpha","0.1.10-alpha","0.1.9-alpha"]"#;
        let versions: Vec<String> = parse_manifest_json(body)
            .unwrap()
            .iter()
            .map(|entry| entry.version.to_string())
            .collect();
        assert_eq!(versions, vec!["0.1.9-alpha", "0.1.10-alpha"]);
        assert!(parse_manifest_json("[]").is_err());
        assert!(parse_manifest_json("not-json").is_err());
    }

    #[test]
    fn parse_manifest_text_skips_comments_and_invalid() {
        let body = "# versions\n0.1.9-alpha\n\n bad \n0.1.10-alpha # latest\nv0.1.9-alpha\n";
        let versions: Vec<String> = parse_manifest_text(body)
            .unwrap()
            .iter()
            .map(|entry| entry.version.to_string())
            .collect();
        assert_eq!(versions, vec!["0.1.9-alpha", "0.1.10-alpha"]);
        assert!(parse_manifest_text("# only comments\n").is_err());
    }

    #[tokio::test]
    async fn list_versions_reads_generic_http_manifest() {
        let mut server = mockito::Server::new_async().await;
        let manifest = server
            .mock("GET", "/wist-center-stack/versions.json")
            .with_status(200)
            .with_body(r#"{"versions":["0.1.9-alpha","0.1.10-alpha"]}"#)
            .create_async()
            .await;
        let template = format!(
            "{}/wist-center-stack/v{{version}}/wist-center-stack-v{{version}}.tar.gz",
            server.url()
        );
        let sys = test_sys(&template);

        let versions: Vec<String> = list_versions(Path::new("."), &sys)
            .await
            .unwrap()
            .iter()
            .map(|entry| entry.version.to_string())
            .collect();
        assert_eq!(versions, vec!["0.1.9-alpha", "0.1.10-alpha"]);
        manifest.assert_async().await;
    }

    #[tokio::test]
    async fn list_versions_falls_back_to_versions_txt() {
        let mut server = mockito::Server::new_async().await;
        let json = server
            .mock("GET", "/pkgs/versions.json")
            .with_status(404)
            .create_async()
            .await;
        let txt = server
            .mock("GET", "/pkgs/versions.txt")
            .with_status(200)
            .with_body("0.1.9-alpha\n0.1.10-alpha\n")
            .create_async()
            .await;
        let template = format!("{}/pkgs/s-v{{version}}.tar.gz", server.url());
        let sys = test_sys(&template);

        let versions: Vec<String> = list_versions(Path::new("."), &sys)
            .await
            .unwrap()
            .iter()
            .map(|entry| entry.version.to_string())
            .collect();
        assert_eq!(versions, vec!["0.1.9-alpha", "0.1.10-alpha"]);
        json.assert_async().await;
        txt.assert_async().await;
    }

    #[tokio::test]
    async fn probe_versions_patch_finds_next_existing() {
        let mut server = mockito::Server::new_async().await;
        let missing = server
            .mock(
                "HEAD",
                "/wist-center-stack/v0.1.10-alpha/wist-center-stack-v0.1.10-alpha.tar.gz",
            )
            .with_status(404)
            .create_async()
            .await;
        let found = server
            .mock(
                "HEAD",
                "/wist-center-stack/v0.1.11-alpha/wist-center-stack-v0.1.11-alpha.tar.gz",
            )
            .with_status(200)
            .create_async()
            .await;
        let sys = test_sys(&format!(
            "{}/wist-center-stack/v{{version}}/wist-center-stack-v{{version}}.tar.gz",
            server.url()
        ));

        let entries = probe_versions(
            &sys,
            &VersionSpec::Patch,
            "0.1.9-alpha",
            ReleaseChannel::Alpha,
        )
        .await
        .unwrap();
        assert_eq!(entries[0].version.to_string(), "0.1.11-alpha");
        missing.assert_async().await;
        found.assert_async().await;
    }

    #[tokio::test]
    async fn probe_versions_minor_and_major() {
        let mut server = mockito::Server::new_async().await;
        let minor = server
            .mock("HEAD", "/pkg/v0.2.0-alpha/pkg-v0.2.0-alpha.tar.gz")
            .with_status(200)
            .create_async()
            .await;
        let major = server
            .mock("HEAD", "/pkg/v1.0.0-alpha/pkg-v1.0.0-alpha.tar.gz")
            .with_status(200)
            .create_async()
            .await;
        let sys = test_sys(&format!(
            "{}/pkg/v{{version}}/pkg-v{{version}}.tar.gz",
            server.url()
        ));

        let entries = probe_versions(
            &sys,
            &VersionSpec::Minor,
            "0.1.9-alpha",
            ReleaseChannel::Alpha,
        )
        .await
        .unwrap();
        assert_eq!(entries[0].version.to_string(), "0.2.0-alpha");

        let entries = probe_versions(
            &sys,
            &VersionSpec::Major,
            "0.1.9-alpha",
            ReleaseChannel::Alpha,
        )
        .await
        .unwrap();
        assert_eq!(entries[0].version.to_string(), "1.0.0-alpha");
        minor.assert_async().await;
        major.assert_async().await;
    }

    #[tokio::test]
    async fn probe_versions_major_can_find_first_minor_of_next_major() {
        let mut server = mockito::Server::new_async().await;
        for patch in 0..=5 {
            let path = format!("/pkg/v1.0.{patch}-alpha/pkg-v1.0.{patch}-alpha.tar.gz");
            server
                .mock("HEAD", path.as_str())
                .with_status(404)
                .create_async()
                .await;
        }
        let found = server
            .mock("HEAD", "/pkg/v1.1.0-alpha/pkg-v1.1.0-alpha.tar.gz")
            .with_status(200)
            .create_async()
            .await;
        let sys = test_sys(&format!(
            "{}/pkg/v{{version}}/pkg-v{{version}}.tar.gz",
            server.url()
        ));

        let entries = probe_versions(
            &sys,
            &VersionSpec::Major,
            "0.1.9-alpha",
            ReleaseChannel::Alpha,
        )
        .await
        .unwrap();
        assert_eq!(entries[0].version.to_string(), "1.1.0-alpha");
        found.assert_async().await;
    }

    #[tokio::test]
    async fn probe_versions_prefers_smallest_version_in_batch() {
        let mut server = mockito::Server::new_async().await;
        let first = server
            .mock("HEAD", "/pkg/v0.1.10-alpha/pkg-v0.1.10-alpha.tar.gz")
            .with_status(200)
            .expect(1)
            .create_async()
            .await;
        let second = server
            .mock("HEAD", "/pkg/v0.1.11-alpha/pkg-v0.1.11-alpha.tar.gz")
            .with_status(200)
            .expect(1)
            .create_async()
            .await;
        let sys = test_sys(&format!(
            "{}/pkg/v{{version}}/pkg-v{{version}}.tar.gz",
            server.url()
        ));

        let entries = probe_versions(
            &sys,
            &VersionSpec::Patch,
            "0.1.9-alpha",
            ReleaseChannel::Alpha,
        )
        .await
        .unwrap();
        assert_eq!(entries[0].version.to_string(), "0.1.10-alpha");
        first.assert_async().await;
        second.assert_async().await;
    }

    #[tokio::test]
    async fn probe_versions_stops_after_first_batch_with_hit() {
        let mut server = mockito::Server::new_async().await;
        let found = server
            .mock("HEAD", "/pkg/v0.1.10-alpha/pkg-v0.1.10-alpha.tar.gz")
            .with_status(200)
            .expect(1)
            .create_async()
            .await;
        // 第二批的第一个候选：第一批已命中时不应再被请求。
        let second_batch = server
            .mock("HEAD", "/pkg/v0.1.18-alpha/pkg-v0.1.18-alpha.tar.gz")
            .with_status(200)
            .expect(0)
            .create_async()
            .await;
        let sys = test_sys(&format!(
            "{}/pkg/v{{version}}/pkg-v{{version}}.tar.gz",
            server.url()
        ));

        let entries = probe_versions(
            &sys,
            &VersionSpec::Patch,
            "0.1.9-alpha",
            ReleaseChannel::Alpha,
        )
        .await
        .unwrap();
        assert_eq!(entries[0].version.to_string(), "0.1.10-alpha");
        found.assert_async().await;
        second_batch.assert_async().await;
    }

    #[tokio::test]
    async fn probe_versions_ignores_failed_candidate_when_hit_exists() {
        let mut server = mockito::Server::new_async().await;
        // 0.1.10 对 HEAD / GET Range 都返回 403，0.1.11 正常存在；
        // 同一批里有命中时，个别候选的权限错误不应拖垮升级。
        let denied_head = server
            .mock("HEAD", "/pkg/v0.1.10-alpha/pkg-v0.1.10-alpha.tar.gz")
            .with_status(403)
            .expect(1)
            .create_async()
            .await;
        let denied_get = server
            .mock("GET", "/pkg/v0.1.10-alpha/pkg-v0.1.10-alpha.tar.gz")
            .match_header("range", "bytes=0-0")
            .with_status(403)
            .expect(1)
            .create_async()
            .await;
        let found = server
            .mock("HEAD", "/pkg/v0.1.11-alpha/pkg-v0.1.11-alpha.tar.gz")
            .with_status(200)
            .expect(1)
            .create_async()
            .await;
        let sys = test_sys(&format!(
            "{}/pkg/v{{version}}/pkg-v{{version}}.tar.gz",
            server.url()
        ));

        let entries = probe_versions(
            &sys,
            &VersionSpec::Patch,
            "0.1.9-alpha",
            ReleaseChannel::Alpha,
        )
        .await
        .unwrap();
        assert_eq!(entries[0].version.to_string(), "0.1.11-alpha");
        denied_head.assert_async().await;
        denied_get.assert_async().await;
        found.assert_async().await;
    }

    #[tokio::test]
    async fn probe_versions_unknown_current_is_clear_error() {
        let sys = test_sys("https://host/pkg/v{version}/pkg-v{version}.tar.gz");

        for current in ["", "   "] {
            let err = probe_versions(&sys, &VersionSpec::Patch, current, ReleaseChannel::Alpha)
                .await
                .unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("现场版本未知"), "{msg}");
            assert!(msg.contains("--to"), "{msg}");
        }
    }

    #[tokio::test]
    async fn probe_versions_no_hit_reports_window_and_escape_hatch() {
        let mut server = mockito::Server::new_async().await;
        for patch in 10..=29 {
            let path = format!("/pkg/v0.1.{patch}-alpha/pkg-v0.1.{patch}-alpha.tar.gz");
            server
                .mock("HEAD", path.as_str())
                .with_status(404)
                .create_async()
                .await;
        }
        let sys = test_sys(&format!(
            "{}/pkg/v{{version}}/pkg-v{{version}}.tar.gz",
            server.url()
        ));

        let err = probe_versions(
            &sys,
            &VersionSpec::Patch,
            "0.1.9-alpha",
            ReleaseChannel::Alpha,
        )
        .await
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("没有找到 patch"), "{msg}");
        assert!(msg.contains("0.1.9-alpha"), "{msg}");
        assert!(msg.contains("窗口 20 个候选"), "{msg}");
        assert!(msg.contains("-v <精确版本>"), "{msg}");
        assert!(msg.contains("--to"), "{msg}");
    }

    #[tokio::test]
    async fn probe_versions_renders_channel_suffix() {
        let mut server = mockito::Server::new_async().await;
        let stable = server
            .mock("HEAD", "/pkg/v0.1.10/pkg-v0.1.10.tar.gz")
            .with_status(200)
            .expect(1)
            .create_async()
            .await;
        let beta = server
            .mock("HEAD", "/pkg/v1.0.0-beta/pkg-v1.0.0-beta.tar.gz")
            .with_status(200)
            .expect(1)
            .create_async()
            .await;
        let sys = test_sys(&format!(
            "{}/pkg/v{{version}}/pkg-v{{version}}.tar.gz",
            server.url()
        ));

        let entries = probe_versions(&sys, &VersionSpec::Patch, "0.1.9", ReleaseChannel::Stable)
            .await
            .unwrap();
        assert_eq!(entries[0].version.to_string(), "0.1.10");

        let entries = probe_versions(
            &sys,
            &VersionSpec::Major,
            "0.1.9-beta",
            ReleaseChannel::Beta,
        )
        .await
        .unwrap();
        assert_eq!(entries[0].version.to_string(), "1.0.0-beta");

        stable.assert_async().await;
        beta.assert_async().await;
    }

    #[tokio::test]
    async fn probe_versions_uses_current_major_minor() {
        let mut server = mockito::Server::new_async().await;
        let patch = server
            .mock("HEAD", "/pkg/v2.7.5-alpha/pkg-v2.7.5-alpha.tar.gz")
            .with_status(200)
            .expect(1)
            .create_async()
            .await;
        let minor = server
            .mock("HEAD", "/pkg/v2.8.0-alpha/pkg-v2.8.0-alpha.tar.gz")
            .with_status(200)
            .expect(1)
            .create_async()
            .await;
        let sys = test_sys(&format!(
            "{}/pkg/v{{version}}/pkg-v{{version}}.tar.gz",
            server.url()
        ));

        let entries = probe_versions(
            &sys,
            &VersionSpec::Patch,
            "2.7.4-alpha",
            ReleaseChannel::Alpha,
        )
        .await
        .unwrap();
        assert_eq!(entries[0].version.to_string(), "2.7.5-alpha");

        let entries = probe_versions(
            &sys,
            &VersionSpec::Minor,
            "2.7.4-alpha",
            ReleaseChannel::Alpha,
        )
        .await
        .unwrap();
        assert_eq!(entries[0].version.to_string(), "2.8.0-alpha");

        patch.assert_async().await;
        minor.assert_async().await;
    }

    #[tokio::test]
    async fn try_list_versions_skips_forbidden_manifest() {
        let mut server = mockito::Server::new_async().await;
        let json = server
            .mock("GET", "/pkgs/versions.json")
            .with_status(403)
            .create_async()
            .await;
        let txt = server
            .mock("GET", "/pkgs/versions.txt")
            .with_status(403)
            .create_async()
            .await;
        let sys = test_sys(&format!("{}/pkgs/s-v{{version}}.tar.gz", server.url()));

        let entries = try_list_versions(Path::new("."), &sys).await.unwrap();
        assert!(entries.is_none(), "403 清单应视为不存在，交给 URL 探测");
        json.assert_async().await;
        txt.assert_async().await;
    }

    #[test]
    fn candidate_versions_windows_are_ascending_and_bounded() {
        let current = semver::Version::parse("0.1.9-alpha").unwrap();

        let patch = candidate_versions(&current, &VersionSpec::Patch, "-alpha").unwrap();
        assert_eq!(patch.len(), MAX_PATCH_PROBES as usize);
        assert_eq!(patch[0].to_string(), "0.1.10-alpha");
        assert_eq!(patch[patch.len() - 1].to_string(), "0.1.29-alpha");
        assert!(patch.windows(2).all(|w| w[0] < w[1]));

        let minor = candidate_versions(&current, &VersionSpec::Minor, "-alpha").unwrap();
        assert_eq!(
            minor.len(),
            (MAX_MINOR_PROBES * (MAX_PATCH_PER_MINOR + 1)) as usize
        );
        assert_eq!(minor[0].to_string(), "0.2.0-alpha");
        assert_eq!(minor[1].to_string(), "0.2.1-alpha");
        assert_eq!(minor[6].to_string(), "0.3.0-alpha");
        assert!(minor.windows(2).all(|w| w[0] < w[1]));

        let major = candidate_versions(&current, &VersionSpec::Major, "-alpha").unwrap();
        assert_eq!(
            major.len(),
            (MAX_MAJOR_PROBES * (MAX_MINOR_PER_MAJOR + 1) * (MAX_PATCH_PER_MINOR + 1)) as usize
        );
        assert_eq!(major[0].to_string(), "1.0.0-alpha");
        assert!(major.windows(2).all(|w| w[0] < w[1]));

        let stable = candidate_versions(&current, &VersionSpec::Major, "").unwrap();
        assert_eq!(stable[0].to_string(), "1.0.0");
    }

    #[tokio::test]
    async fn probe_versions_latest_and_bad_current_are_clear_errors() {
        let sys = test_sys("https://host/pkg/v{version}/pkg-v{version}.tar.gz");

        let err = probe_versions(
            &sys,
            &VersionSpec::Latest,
            "0.1.9-alpha",
            ReleaseChannel::Alpha,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("latest"), "{err}");

        let err = probe_versions(&sys, &VersionSpec::Patch, "build-1", ReleaseChannel::Alpha)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("不是 semver"), "{err}");
    }

    #[tokio::test]
    async fn list_versions_reports_missing_generic_manifest() {
        let mut server = mockito::Server::new_async().await;
        let json = server
            .mock("GET", "/pkgs/versions.json")
            .with_status(404)
            .create_async()
            .await;
        let txt = server
            .mock("GET", "/pkgs/versions.txt")
            .with_status(404)
            .create_async()
            .await;
        let template = format!("{}/pkgs/s-v{{version}}.tar.gz", server.url());
        let sys = test_sys(&template);

        let err = list_versions(Path::new("."), &sys).await.unwrap_err();
        assert!(err.to_string().contains("versions.json"), "{err}");
        assert!(
            err.to_string().contains(&format!("{}/pkgs/", server.url())),
            "{err}"
        );
        json.assert_async().await;
        txt.assert_async().await;
    }

    #[test]
    fn github_repo_extracts_owner_and_repo() {
        assert_eq!(
            github_repo(
                "https://github.com/dayu-sec/wist-gateway-stack/releases/download/v{version}/x.tar.gz"
            ),
            Some(("dayu-sec".to_string(), "wist-gateway-stack".to_string()))
        );
        assert_eq!(
            github_repo("https://github.com/o/r.git"),
            Some(("o".to_string(), "r".to_string()))
        );
        assert_eq!(
            github_repo("http://github.com/o/r/releases/download/v1/x.tar.gz"),
            Some(("o".to_string(), "r".to_string()))
        );
        assert_eq!(
            github_repo("https://www.github.com/o/r/releases/download/v1/x.tar.gz"),
            Some(("o".to_string(), "r".to_string()))
        );
        assert_eq!(github_repo("https://example.com/o/r.tar.gz"), None);
        assert_eq!(github_repo("https://github.com/o"), None);
    }

    #[test]
    fn parse_github_releases_skips_drafts_and_invalid_tags() {
        let body = r#"[
          {"tag_name":"v0.9.0","draft":false},
          {"tag_name":"v0.10.0-alpha","draft":false},
          {"tag_name":"v0.11.0","draft":true},
          {"tag_name":"release-0.12.0","draft":false},
          {"tag_name":"v0.10.0","draft":false},
          {"tag_name":"v0.10.0","draft":false}
        ]"#;
        let versions: Vec<String> = parse_github_releases(body)
            .unwrap()
            .iter()
            .map(|e| e.version.to_string())
            .collect();
        assert_eq!(versions, vec!["0.9.0", "0.10.0-alpha", "0.10.0"]);
    }

    #[test]
    fn parse_github_releases_rejects_empty_or_invalid_payload() {
        assert!(parse_github_releases("[]").is_err());
        assert!(parse_github_releases("not json").is_err());
    }

    #[test]
    fn parse_git_tags_handles_lightweight_and_annotated_tags() {
        let output = "a\trefs/tags/v0.9.0\nb\trefs/tags/v0.10.0\nc\trefs/tags/v0.10.0^{}\nd\trefs/tags/not-a-version\ne\trefs/tags/0.10.1-beta\n";
        let versions: Vec<String> = parse_git_tags(output)
            .iter()
            .map(|e| e.version.to_string())
            .collect();
        assert_eq!(versions, vec!["0.9.0", "0.10.0", "0.10.0", "0.10.1-beta"]);
    }

    fn test_sys(addr: &str) -> OpsSystem {
        use crate::system::SysKind;
        use crate::system::spec::SysDefine;
        let define = SysDefine::new_without_model("s").with_kind(SysKind::DockerCompose);
        let addr = crate::artifact::types::convert_addr(addr).unwrap();
        OpsSystem::new(define, addr)
    }
}
