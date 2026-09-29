use super::prelude::*;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::model::SelfUpdateState;

const GALAXY_DIR: &str = ".galaxy";
const SELF_UPDATE_DIR: &str = "self_update";
/// 产品子目录：与 gx 共用 `~/.galaxy/self_update` 时避免状态/备份/锁互相覆盖。
const PRODUCT_DIR: &str = "gops";
const STATE_FILE: &str = "state.json";
const LOCK_FILE: &str = "lock";
const BACKUPS_DIR: &str = "backups";
const STALE_LOCK_MAX_AGE_SECS: u64 = 24 * 60 * 60;

/// 进程内自升级互斥锁；`Drop` 时删除锁文件。
pub struct FileLock {
    path: PathBuf,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// 自升级状态/备份/锁的本地存储（`~/.galaxy/self_update/gops`）。
#[derive(Clone, Debug)]
pub struct SelfUpdateStorage {
    root: PathBuf,
}

impl SelfUpdateStorage {
    pub fn new() -> MainResult<Self> {
        let home = home::home_dir().ok_or_else(|| {
            MainReason::resource_detail("cannot resolve home directory: self update needs home dir")
        })?;
        let root = home
            .join(GALAXY_DIR)
            .join(SELF_UPDATE_DIR)
            .join(PRODUCT_DIR);
        let this = Self { root };
        this.ensure_layout()?;
        Ok(this)
    }

    pub fn state_path(&self) -> PathBuf {
        self.root.join(STATE_FILE)
    }

    pub fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK_FILE)
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.root.join(BACKUPS_DIR)
    }

    pub fn ensure_layout(&self) -> MainResult<()> {
        let backups = self.backups_dir();
        fs::create_dir_all(&backups)
            .source_resource()
            .doing("create self update layout")
            .with_context(("path", backups.as_path()))?;
        Ok(())
    }

    pub fn load_state(&self) -> MainResult<SelfUpdateState> {
        let path = self.state_path();
        if !path.exists() {
            return Ok(SelfUpdateState::default());
        }
        let content = fs::read_to_string(&path)
            .source_resource()
            .doing("read self update state")
            .with_context(("path", path.as_path()))?;
        serde_json::from_str::<SelfUpdateState>(&content)
            .source_data()
            .doing("parse self update state")
            .with_context(("path", path.as_path()))
    }

    pub fn save_state(&self, state: &SelfUpdateState) -> MainResult<()> {
        let path = self.state_path();
        let content = serde_json::to_string_pretty(state)
            .source_data()
            .doing("serialize self update state")
            .with_context(("path", path.as_path()))?;
        // 先写临时文件再 rename：避免写入中断留下损坏的 state.json
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, content)
            .source_resource()
            .doing("write self update state")
            .with_context(("path", tmp.as_path()))?;
        fs::rename(&tmp, &path)
            .source_resource()
            .doing("commit self update state")
            .with_context(("path", path.as_path()))?;
        Ok(())
    }

    pub fn acquire_lock(&self) -> MainResult<FileLock> {
        let path = self.lock_path();
        match create_lock_file(&path) {
            Ok(lock) => Ok(lock),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                if lock_is_stale(&path, Duration::from_secs(STALE_LOCK_MAX_AGE_SECS)) {
                    if let Some(pid) = read_lock_pid(&path)
                        && process_is_running(pid)
                    {
                        return Err(MainReason::resource_detail(format!(
                            "self update is busy: lock_file={}, pid={} still running",
                            path.display(),
                            pid
                        )));
                    }
                    let _ = fs::remove_file(&path);
                    create_lock_file(&path)
                        .source_resource()
                        .doing("create self update lock file")
                        .with_context(("path", path.as_path()))
                } else {
                    Err(MainReason::resource_detail(format!(
                        "self update is busy: lock_file={}, remove it manually if previous process crashed",
                        path.display()
                    )))
                }
            }
            Err(err) => Err::<FileLock, _>(err)
                .source_resource()
                .doing("create self update lock file")
                .with_context(("path", path.as_path())),
        }
    }

    pub fn list_backups_desc(&self) -> MainResult<Vec<String>> {
        let mut list = Vec::new();
        let backups = self.backups_dir();
        for item in fs::read_dir(&backups)
            .source_resource()
            .doing("read self update backups dir")
            .with_context(("path", backups.as_path()))?
        {
            let item = item
                .source_resource()
                .doing("read self update backup entry")
                .with_context(("path", backups.as_path()))?;
            if item
                .file_type()
                .source_resource()
                .doing("read backup entry file type")
                .with_context(("path", item.path().as_path()))?
                .is_dir()
            {
                let file_name = item.file_name();
                if let Some(name) = file_name.to_str() {
                    list.push(name.to_string());
                }
            }
        }
        list.sort();
        list.reverse();
        Ok(list)
    }

    /// 只保留最新的 `keep` 个备份，删除更旧的（按目录名倒序）。
    pub fn prune_backups(&self, keep: usize) -> MainResult<()> {
        for id in self.list_backups_desc()?.into_iter().skip(keep) {
            let dir = self.backups_dir().join(&id);
            fs::remove_dir_all(&dir)
                .source_resource()
                .doing("prune old self update backup")
                .with_context(("path", dir.as_path()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
impl SelfUpdateStorage {
    /// 测试用：直接指定存储根目录，避免写入真实的 `$HOME`。
    pub(crate) fn with_root(root: PathBuf) -> Self {
        Self { root }
    }
}

fn create_lock_file(path: &Path) -> io::Result<FileLock> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(format!("pid={}\n", std::process::id()).as_bytes())?;
    Ok(FileLock {
        path: path.to_path_buf(),
    })
}

fn lock_is_stale(path: &Path, max_age: Duration) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    let Ok(elapsed) = modified.elapsed() else {
        return false;
    };
    elapsed > max_age
}

fn read_lock_pid(path: &Path) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("pid=")
            && let Ok(pid) = v.trim().parse::<u32>()
        {
            return Some(pid);
        }
    }
    None
}

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn process_is_running(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::{SelfUpdateStorage, lock_is_stale, process_is_running, read_lock_pid};
    use crate::self_update::model::SelfUpdateState;
    use std::time::Duration;

    #[test]
    fn parse_lock_pid() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let path = dir.path().join("lock");
        std::fs::write(&path, "pid=12345\n").expect("write lock");
        assert_eq!(read_lock_pid(&path), Some(12345));
    }

    #[test]
    fn parse_lock_pid_ignores_malformed() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let path = dir.path().join("lock");
        std::fs::write(&path, "garbage\npid=notanumber\n").expect("write lock");
        assert_eq!(read_lock_pid(&path), None);
    }

    #[test]
    fn fresh_lock_is_not_stale() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let path = dir.path().join("lock");
        std::fs::write(&path, "pid=1\n").expect("write lock");
        assert!(!lock_is_stale(&path, Duration::from_secs(3600)));
    }

    #[test]
    fn missing_lock_is_not_stale() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let path = dir.path().join("missing");
        assert!(!lock_is_stale(&path, Duration::from_secs(0)));
    }

    #[test]
    fn current_process_is_running() {
        assert!(process_is_running(std::process::id()));
    }

    #[test]
    fn save_then_load_round_trip_leaves_no_tmp() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let storage = SelfUpdateStorage::with_root(tmp.path().to_path_buf());
        storage.ensure_layout().expect("layout");

        let state = SelfUpdateState {
            last_channel: Some(crate::self_update::ReleaseChannel::Alpha),
            last_result: Some("updated".to_string()),
            ..Default::default()
        };
        storage.save_state(&state).expect("save");
        assert_eq!(storage.load_state().expect("load"), state);
        assert!(!tmp.path().join("state.json.tmp").exists());
    }

    #[test]
    fn load_state_defaults_when_absent() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let storage = SelfUpdateStorage::with_root(tmp.path().to_path_buf());
        assert_eq!(
            storage.load_state().expect("load"),
            SelfUpdateState::default()
        );
    }

    #[test]
    fn prune_keeps_newest_and_drops_oldest() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let storage = SelfUpdateStorage::with_root(tmp.path().to_path_buf());
        storage.ensure_layout().expect("layout");
        for id in [
            "20260101000001",
            "20260101000002",
            "20260101000003",
            "20260101000004",
            "20260101000005",
            "20260101000006",
            "20260101000007",
        ] {
            std::fs::create_dir_all(storage.backups_dir().join(id)).expect("mk backup");
        }

        storage.prune_backups(5).expect("prune");
        let remaining = storage.list_backups_desc().expect("list");
        assert_eq!(
            remaining,
            vec![
                "20260101000007",
                "20260101000006",
                "20260101000005",
                "20260101000004",
                "20260101000003",
            ]
        );
    }
}
