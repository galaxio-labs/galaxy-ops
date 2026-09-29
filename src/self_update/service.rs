use super::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;

use super::model::{CheckResult, ReleaseChannel, SelfUpdateState, StatusResult, UpdateResult};
use super::rollback;
use super::storage::SelfUpdateStorage;

const MANIFEST_BASE_URL: &str =
    "https://raw.githubusercontent.com/galaxio-labs/get/main/updates/gops";
const PRODUCT_NAME: &str = "gops";
/// 本地保留的备份数量上限。
const MAX_BACKUPS: usize = 5;

#[derive(Clone, Debug, Default)]
pub struct CheckRequest {
    pub channel: ReleaseChannel,
}

#[derive(Clone, Debug, Default)]
pub struct UpdateRequest {
    pub channel: ReleaseChannel,
    pub to_version: Option<String>,
    pub yes: bool,
    pub dry_run: bool,
    pub force: bool,
}

#[derive(Clone, Debug)]
pub struct SelfUpdateService {
    storage: SelfUpdateStorage,
}

impl SelfUpdateService {
    pub fn new() -> MainResult<Self> {
        Ok(Self {
            storage: SelfUpdateStorage::new()?,
        })
    }

    pub fn status(&self) -> MainResult<StatusResult> {
        let install_dir = resolve_install_dir()?;
        let mut state = self.storage.load_state()?;
        state.current_version = Some(env!("CARGO_PKG_VERSION").to_string());
        Ok(StatusResult {
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            install_dir,
            state,
        })
    }

    pub async fn check(&self, req: CheckRequest) -> MainResult<CheckResult> {
        let _lock = self.storage.acquire_lock()?;
        let mut state = self.storage.load_state()?;
        let channel = req.channel;

        let wp_source = wp_source(channel);
        let wp_req = wp_self_update::CheckRequest {
            product: PRODUCT_NAME.to_string(),
            source: wp_source,
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            branch: channel.as_str().to_string(),
        };

        let report = wp_self_update::check(wp_req).await.map_err(|e| {
            let _ = record_failure_state(
                &self.storage,
                &mut state,
                channel,
                None,
                "check_failed",
                &e.to_string(),
            );
            convert_wp_error(e)
        })?;

        state.last_checked_at = Some(now_text());
        state.last_channel = Some(channel);
        state.last_remote_version = Some(report.latest_version.clone());
        state.last_result = Some(if report.update_available {
            "update_available".to_string()
        } else {
            "up_to_date".to_string()
        });
        state.last_error = None;
        self.storage.save_state(&state)?;

        Ok(CheckResult {
            channel,
            current_version: report.current_version,
            remote_version: report.latest_version,
            has_update: report.update_available,
        })
    }

    pub async fn update(&self, req: UpdateRequest) -> MainResult<UpdateResult> {
        let _lock = self.storage.acquire_lock()?;
        let mut state = self.storage.load_state()?;
        let channel = req.channel;
        let current = env!("CARGO_PKG_VERSION").to_string();

        let wp_source = wp_source(channel);

        // 先检查以获取远端版本
        let check_req = wp_self_update::CheckRequest {
            product: PRODUCT_NAME.to_string(),
            source: wp_source.clone(),
            current_version: current.clone(),
            branch: channel.as_str().to_string(),
        };

        let check_report = wp_self_update::check(check_req).await.map_err(|e| {
            let _ = record_failure_state(
                &self.storage,
                &mut state,
                channel,
                None,
                "update_failed",
                &e.to_string(),
            );
            convert_wp_error(e)
        })?;

        let remote = check_report.latest_version.clone();

        // 校验 --to 指定的目标版本
        if let Some(expect) = &req.to_version
            && expect.trim() != remote
        {
            let err = MainReason::Uvs(UvsReason::validation_error())
                .to_err()
                .doing("validate target update version")
                .with_context(("expect", expect.as_str()))
                .with_context(("manifest", remote.as_str()))
                .with_detail(format!(
                    "target version mismatch: expect={expect}, manifest={remote}"
                ));
            let _ = record_failure_state(
                &self.storage,
                &mut state,
                channel,
                Some(remote.clone()),
                "update_failed",
                &err.to_string(),
            );
            return Err(err);
        }

        // 无需更新
        if !req.force && !check_report.update_available {
            state.last_checked_at = Some(now_text());
            state.last_channel = Some(channel);
            state.last_remote_version = Some(remote.clone());
            state.last_result = Some("up_to_date".to_string());
            state.last_error = None;
            self.storage.save_state(&state)?;
            return Ok(UpdateResult {
                channel,
                from_version: current,
                to_version: remote,
                backup_id: None,
                updated: false,
            });
        }

        if req.dry_run {
            state.last_checked_at = Some(now_text());
            state.last_channel = Some(channel);
            state.last_remote_version = Some(remote.clone());
            state.last_result = Some("dry_run".to_string());
            state.last_error = None;
            self.storage.save_state(&state)?;
            return Ok(UpdateResult {
                channel,
                from_version: current,
                to_version: remote,
                backup_id: None,
                updated: false,
            });
        }

        // 执行升级（下载 / 校验 / 安装由 wp-self-update 完成）
        let install_dir = resolve_install_dir()?;
        // 升级前先把当前二进制备份到 `~/.galaxy/self_update/gops/backups/<id>`，
        // 否则 `self rollback` 找不到可恢复的文件（wp-self-update 自身的备份不对外暴露）。
        let backup_id = next_backup_id(&self.storage.backups_dir());
        let backup_dir = self.storage.backups_dir().join(&backup_id);
        if let Err(e) = backup_current_binary(&install_dir, &self.storage.backups_dir(), &backup_id)
        {
            // 备份写一半失败：清掉，避免半成品被 rollback 误选
            let _ = std::fs::remove_dir_all(&backup_dir);
            return Err(e);
        }

        let wp_update_req = wp_self_update::UpdateRequest {
            product: PRODUCT_NAME.to_string(),
            // 与 `bin_name` 一致（Windows 下为 `gops.exe`）；wp-self-update 按名字逐字匹配。
            target: wp_self_update::UpdateTarget::Bins(target_bins()),
            source: wp_source,
            current_version: current.clone(),
            install_dir: Some(install_dir.clone()),
            yes: req.yes,
            dry_run: false,
            force: req.force,
        };

        let update_report = wp_self_update::update(wp_update_req).await.map_err(|e| {
            // 升级未发生，清理刚建的备份，避免 rollback 误选到“同版本备份”
            let _ = std::fs::remove_dir_all(&backup_dir);
            let _ = record_failure_state(
                &self.storage,
                &mut state,
                channel,
                Some(remote.clone()),
                "update_failed",
                &e.to_string(),
            );
            convert_wp_error(e)
        })?;

        // 未实际安装（典型为未加 --yes 时交互拒绝，status="aborted"）：
        // 不能记为“已更新”，也不保留备份。
        if !update_report.updated {
            let _ = std::fs::remove_dir_all(&backup_dir);
            state.last_checked_at = Some(now_text());
            state.last_channel = Some(channel);
            state.last_remote_version = Some(remote.clone());
            state.last_result = Some(update_report.status.clone());
            state.last_error = None;
            self.storage.save_state(&state)?;
            return Ok(UpdateResult {
                channel,
                from_version: current,
                to_version: remote,
                backup_id: None,
                updated: false,
            });
        }

        state.last_checked_at = Some(now_text());
        state.last_channel = Some(channel);
        state.last_remote_version = Some(remote.clone());
        state.last_result = Some("updated".to_string());
        state.last_error = None;
        state.current_version = Some(remote.clone());
        state.installed_at = Some(now_text());
        state.last_backup_id = Some(backup_id.clone());
        self.storage.save_state(&state)?;
        // 保留最近若干备份，避免无界增长（尽力而为，不影响升级结果）。
        let _ = self.storage.prune_backups(MAX_BACKUPS);

        Ok(UpdateResult {
            channel,
            from_version: current,
            to_version: remote,
            backup_id: Some(backup_id),
            updated: update_report.updated,
        })
    }

    pub fn rollback(&self, id: Option<&str>) -> MainResult<UpdateResult> {
        let _lock = self.storage.acquire_lock()?;
        let backups = self.storage.list_backups_desc()?;
        let backup_id = select_backup_id(&backups, id)?;
        let mut state = self.storage.load_state()?;
        let channel = state.last_channel.unwrap_or_default();
        let from_version = state
            .current_version
            .clone()
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
        let backup_dir = self.storage.backups_dir().join(&backup_id);
        let install_dir = resolve_install_dir().inspect_err(|e| {
            let _ = record_rollback_failure_state(&self.storage, &mut state, &e.to_string());
        })?;
        rollback::rollback(&install_dir, &backup_dir).inspect_err(|e| {
            let _ = record_rollback_failure_state(&self.storage, &mut state, &e.to_string());
        })?;
        rollback::health_check(&install_dir).inspect_err(|e| {
            let _ = record_rollback_failure_state(&self.storage, &mut state, &e.to_string());
        })?;
        let restored_version = read_installed_version(&install_dir);
        state.installed_at = Some(now_text());
        match restored_version {
            Ok(v) => {
                state.last_result = Some("rollback".to_string());
                state.last_error = None;
                state.current_version = Some(v.clone());
                self.storage.save_state(&state)?;
                Ok(UpdateResult {
                    channel,
                    from_version,
                    to_version: v,
                    backup_id: Some(backup_id),
                    updated: true,
                })
            }
            Err(e) => {
                state.last_result = Some("rollback_version_unknown".to_string());
                state.last_error = Some(e.to_string());
                self.storage.save_state(&state)?;
                Ok(UpdateResult {
                    channel,
                    from_version,
                    to_version: "unknown".to_string(),
                    backup_id: Some(backup_id),
                    updated: true,
                })
            }
        }
    }
}

fn wp_source(channel: ReleaseChannel) -> wp_self_update::SourceConfig {
    wp_self_update::SourceConfig {
        channel: wp_self_update::UpdateChannel::from(channel),
        kind: wp_self_update::SourceKind::Manifest {
            updates_base_url: MANIFEST_BASE_URL.to_string(),
            updates_root: None,
        },
    }
}

/// 升级目标二进制名（与备份/健康检查一致，随平台带 `.exe`）。
fn target_bins() -> Vec<String> {
    rollback::SELF_UPDATE_BINARIES
        .iter()
        .map(|b| rollback::bin_name(b))
        .collect()
}

fn resolve_install_dir() -> MainResult<PathBuf> {
    let exe = std::env::current_exe()
        .source_resource()
        .doing("resolve current executable path")?;
    let dir = exe.parent().ok_or_else(|| {
        MainReason::resource_detail("cannot resolve install dir from executable path")
    })?;
    // 与 wp-self-update 安装时一致地去符号链接，避免经 PATH 软链调用时两者目录不一致。
    let dir = fs::canonicalize(dir)
        .source_resource()
        .doing("canonicalize install dir")
        .with_context(("path", dir))?;
    Ok(dir)
}

/// 把当前安装目录下的可执行文件复制到备份目录（保留权限位），供回滚使用。
fn backup_current_binary(
    install_dir: &Path,
    backups_dir: &Path,
    backup_id: &str,
) -> MainResult<()> {
    let dst_dir = backups_dir.join(backup_id);
    fs::create_dir_all(&dst_dir)
        .source_resource()
        .doing("create self update backup dir")
        .with_context(("path", dst_dir.as_path()))?;
    for bin in rollback::SELF_UPDATE_BINARIES {
        let src = install_dir.join(rollback::bin_name(bin));
        let dst = dst_dir.join(rollback::bin_name(bin));
        fs::copy(&src, &dst)
            .source_resource()
            .doing("backup current binary")
            .with_context(("src", src.as_path()))
            .with_context(("dst", dst.as_path()))?;
    }
    Ok(())
}

fn record_failure_state(
    storage: &SelfUpdateStorage,
    state: &mut SelfUpdateState,
    channel: ReleaseChannel,
    remote_version: Option<String>,
    result: &str,
    err: &str,
) -> MainResult<()> {
    state.last_checked_at = Some(now_text());
    state.last_channel = Some(channel);
    state.last_remote_version = remote_version;
    state.last_result = Some(result.to_string());
    state.last_error = Some(err.to_string());
    storage.save_state(state)
}

fn record_rollback_failure_state(
    storage: &SelfUpdateStorage,
    state: &mut SelfUpdateState,
    err: &str,
) -> MainResult<()> {
    state.last_result = Some("rollback_failed".to_string());
    state.last_error = Some(err.to_string());
    storage.save_state(state)
}

fn select_backup_id(backups: &[String], id: Option<&str>) -> MainResult<String> {
    match id {
        Some(raw) => {
            if !is_valid_backup_id(raw) {
                return Err(MainReason::Uvs(UvsReason::validation_error())
                    .to_err()
                    .doing("validate backup id")
                    .with_context(("backup_id", raw))
                    .with_detail(format!(
                        "invalid backup id: backup_id={raw}, expected=14 digits"
                    )));
            }
            if backups.iter().any(|v| v == raw) {
                Ok(raw.to_string())
            } else {
                Err(MainReason::Uvs(UvsReason::validation_error())
                    .to_err()
                    .doing("select rollback backup id")
                    .with_context(("backup_id", raw))
                    .with_detail(format!("backup id not found: backup_id={raw}")))
            }
        }
        None => backups
            .first()
            .cloned()
            .ok_or_else(|| {
                MainReason::Uvs(UvsReason::validation_error())
                    .to_err()
                    .with_detail("no backup found")
            })
            .doing("select latest rollback backup"),
    }
}

fn is_valid_backup_id(input: &str) -> bool {
    input.len() == 14 && input.bytes().all(|b| b.is_ascii_digit())
}

fn next_backup_id(backups_dir: &Path) -> String {
    // 14 位时间戳（与 `is_valid_backup_id` 一致）；同秒重复时顺延到未占用的秒。
    let mut t = Utc::now();
    loop {
        let id = t.format("%Y%m%d%H%M%S").to_string();
        if !backups_dir.join(&id).exists() {
            return id;
        }
        t += chrono::Duration::seconds(1);
    }
}

fn now_text() -> String {
    Utc::now().to_rfc3339()
}

fn read_installed_version(install_dir: &std::path::Path) -> MainResult<String> {
    let bin = install_dir.join(rollback::bin_name("gops"));
    let out = std::process::Command::new(&bin)
        .arg("--version")
        .output()
        .source_resource()
        .doing("run installed binary version command")
        .with_context(("bin", &bin))?;
    if !out.status.success() {
        return Err(MainReason::resource_detail(format!(
            "version command failed: {} --version exit={}",
            bin.display(),
            out.status
        )));
    }
    let text = format!(
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    parse_version_from_text(&text).ok_or_else(|| {
        MainReason::resource_detail(format!(
            "cannot parse version output: bin={}",
            bin.display()
        ))
    })
}

/// 从 `--version` 输出里提取第一个合法语义化版本。
fn parse_version_from_text(text: &str) -> Option<String> {
    for tok in text.split_whitespace() {
        let normalized = tok
            .trim_matches(|c: char| ['"', ',', ':', ';', '(', ')'].contains(&c))
            .trim();
        if normalized.is_empty() {
            continue;
        }
        if let Ok(v) = semver::Version::parse(normalized.trim_start_matches('v')) {
            return Some(v.to_string());
        }
    }
    None
}

fn convert_wp_error(e: impl std::fmt::Display) -> MainError {
    MainReason::resource_detail(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        backup_current_binary, is_valid_backup_id, next_backup_id, parse_version_from_text,
        select_backup_id, target_bins,
    };
    use crate::self_update::rollback;

    #[test]
    fn backup_id_requires_membership() {
        let backups = vec!["20260305010101".to_string()];
        assert!(select_backup_id(&backups, Some("20260305010101")).is_ok());
        assert!(select_backup_id(&backups, Some("20260305010102")).is_err());
    }

    #[test]
    fn backup_id_rejects_path_like_input() {
        let backups = vec!["20260305010101".to_string()];
        assert!(select_backup_id(&backups, Some("../20260305010101")).is_err());
        assert!(select_backup_id(&backups, Some("/tmp/x")).is_err());
    }

    #[test]
    fn select_backup_id_picks_latest_when_unspecified() {
        let backups = vec!["20260305010102".to_string(), "20260305010101".to_string()];
        assert_eq!(select_backup_id(&backups, None).unwrap(), "20260305010102");
    }

    #[test]
    fn select_backup_id_errors_when_empty() {
        assert!(select_backup_id(&[], None).is_err());
    }

    #[test]
    fn validate_backup_id() {
        assert!(is_valid_backup_id("20260321123456"));
        assert!(!is_valid_backup_id("2026032112345")); // too short
        assert!(!is_valid_backup_id("20260321123456a")); // has letter
    }

    #[test]
    fn parse_version_from_noisy_output() {
        assert_eq!(
            parse_version_from_text("gops 2.0.1").as_deref(),
            Some("2.0.1")
        );
        assert_eq!(
            parse_version_from_text("gops: v2.0.1-alpha").as_deref(),
            Some("2.0.1-alpha")
        );
    }

    #[test]
    fn parse_version_returns_none_without_semver() {
        assert_eq!(parse_version_from_text("no version here"), None);
    }

    #[test]
    fn backup_then_rollback_round_trip() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let install = tmp.path().join("install");
        let backups = tmp.path().join("backups");
        std::fs::create_dir_all(&install).expect("install dir");
        std::fs::create_dir_all(&backups).expect("backups dir");
        std::fs::write(install.join("gops"), b"v1").expect("write current");

        backup_current_binary(&install, &backups, "20260101000000").expect("backup");

        // 模拟升级覆盖了二进制
        std::fs::write(install.join("gops"), b"v2").expect("write upgraded");

        rollback::rollback(&install, &backups.join("20260101000000")).expect("rollback");
        assert_eq!(std::fs::read(install.join("gops")).unwrap(), b"v1");
    }

    #[test]
    fn next_backup_id_skips_occupied_seconds() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let backups = tmp.path().join("backups");
        std::fs::create_dir_all(&backups).expect("backups dir");

        let first = next_backup_id(&backups);
        assert!(is_valid_backup_id(&first));
        // 占用该 id 后，下一个 id 必须不同且仍合法（同秒顺延）
        std::fs::create_dir_all(backups.join(&first)).expect("occupy");
        let second = next_backup_id(&backups);
        assert_ne!(first, second);
        assert!(is_valid_backup_id(&second));
    }

    #[test]
    fn target_bins_uses_platform_binary_name() {
        let bins = target_bins();
        assert_eq!(bins, vec![rollback::bin_name("gops")]);
        if cfg!(windows) {
            assert_eq!(bins[0], "gops.exe");
        } else {
            assert_eq!(bins[0], "gops");
        }
    }
}
