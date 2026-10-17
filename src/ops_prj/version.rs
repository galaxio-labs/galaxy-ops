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
//! - git 仓库 tag（`git@...` / `*.git`）。
//!
//! 普通 HTTP 包地址没有可枚举的版本列表，此时只支持精确版本或 `--to <完整地址>`。

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::error::MainReason;
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
    let template = sys.addr_template();
    if !template.contains(VERSION_PLACEHOLDER) {
        return Err(MainReason::logic_detail(format!(
            "系统 {} 的 addr 没有 {VERSION_PLACEHOLDER} 占位，无法按版本解析 latest/patch/minor/major。\n  \
             在 addr 里写 {VERSION_PLACEHOLDER}，或用 `--to <完整地址>`。",
            sys.sys().name()
        )));
    }

    if template.starts_with("git@") || template.ends_with(".git") {
        return list_git_tags(&template).await;
    }
    if template.starts_with("http://") || template.starts_with("https://") {
        if let Some((owner, repo)) = github_repo(&template) {
            return list_github_releases(&owner, &repo).await;
        }
        return Err(MainReason::logic_detail(format!(
            "系统 {} 的 addr 不是 GitHub Releases / git 地址，gops 无法枚举可用版本。\n  \
             用 `-v <精确版本>` 或 `--to <完整地址>`；或把 addr 指向 GitHub Releases。",
            sys.sys().name()
        )));
    }

    list_local_versions(project_root, &template)
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
