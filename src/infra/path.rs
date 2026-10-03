use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use log::info;

use crate::error::{MainReason, MainResult};

/// 取包/解包的工作目录。
///
/// 优先级（高 → 低）：
/// 1. 环境变量 `GOPS_PACKAGE_DIR`（显式指定；也方便把工作区放大盘上）；
/// 2. 旧目录 `$HOME/ds-package`（**已存在**时沿用，不打断在跑的机器）；
/// 3. 平台缓存目录下的 `gops`：Linux `$XDG_CACHE_HOME/gops`（缺省 `~/.cache/gops`）、
///    macOS `~/Library/Caches/gops`、Windows `%LOCALAPPDATA%\gops`。
///
/// 为什么不再裸放在 `$HOME`：`ds-` 是早期二进制（`ds-sys`/`ds-mod`/`ds-ops`）留下的前缀，
/// 0.11.0 把它们改名为 `g*` 时**漏改了这个目录名**；它只是个瞬时缓存，不该占 `$HOME` 一个槽位。
/// 注意「旧目录只在它**是目录**时沿用」——现场出现过它被下载器写成一个**文件**的情形，那种情况直接跳到缓存目录。
pub fn package_work_dir() -> PathBuf {
    resolve_work_dir(
        std::env::var_os("GOPS_PACKAGE_DIR").map(PathBuf::from),
        home::home_dir().map(|h| h.join("ds-package")),
        default_cache_dir(),
    )
}

/// [`package_work_dir`] 的纯函数主体：除了对 `legacy` 做一次 `is_dir()` 探测，不读其他环境态，便于单测。
///
/// **空值视为未设置**：`GOPS_PACKAGE_DIR=` 这种写法若原样用，会得到一个空路径 ——
/// `create_dir_all("")` 是 no-op，下载器随后拿到空目录名，报错变成 `File::create("")` 的 ENOENT，
/// 看起来像“配置没问题却失败”。
fn resolve_work_dir(
    override_dir: Option<PathBuf>,
    legacy: Option<PathBuf>,
    cache: PathBuf,
) -> PathBuf {
    if let Some(dir) = override_dir.filter(|p| !p.as_os_str().is_empty()) {
        return dir;
    }
    if let Some(legacy) = legacy
        && legacy.is_dir()
    {
        return legacy;
    }
    cache
}

/// 平台缓存目录下的 `gops` 工作区。
fn default_cache_dir() -> PathBuf {
    cache_base().join("gops")
}

fn cache_base() -> PathBuf {
    cache_base_from(
        std::env::var_os("XDG_CACHE_HOME"),
        home::home_dir(),
        std::env::var_os("LOCALAPPDATA"),
    )
}

/// 平台缓存根（纯函数，便于单测；不改环境变量）。`xdg` 非空则在任何平台优先。
fn cache_base_from(
    xdg: Option<OsString>,
    home: Option<PathBuf>,
    local_appdata: Option<OsString>,
) -> PathBuf {
    if let Some(dir) = xdg.filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let home = home.unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library/Caches")
    } else if cfg!(target_os = "windows") {
        local_appdata
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Local"))
    } else {
        home.join(".cache")
    }
}

/// 下载/解包的目标目录就绪：不存在就建；**存在但不是目录**时直接报错（附处置命令）。
///
/// 为什么必须显式做这一步：`orion-accessor` 的 http 下载器在「目标不是已存在的目录」时，
/// 会把**整条路径当成文件名**（`http.rs` 的 `dest_dir.is_dir()` else 分支）。于是本该是目录的
/// 路径被写成一个文件，随后 `create_dir_all(dir/<包名>)` 必失败（ENOTDIR）。
///
/// 现场一例（零历史的新机器）：`~/ds-package` 不存在 → 首个包被下载成 `~/ds-package` 这个“文件”
/// → `gops prj reimport` 报 `create_dir_all … [path: ~/ds-package/wist-gateway-stack-…]`。
/// 而在已经有 `~/ds-package` 的机器上（早期跑过 gops）永远看不到 —— 所以这类 bug 只会在
/// **首次部署的目标机**上冒出来。
pub fn ensure_download_dir(dir: &Path) -> MainResult<()> {
    if dir.is_dir() {
        return Ok(());
    }
    // 普通文件 / 悬空符号链接：占位但不是目录，直接删不掉就报清楚
    if std::fs::symlink_metadata(dir).is_ok() {
        return Err(MainReason::logic_detail(format!(
            "下载目录被占用：{} 已存在但不是目录（历史上可能被当成文件写过）。\n  \
             请删除后重试：rm -rf {}",
            dir.display(),
            dir.display()
        )));
    }
    std::fs::create_dir_all(dir).map_err(|e| {
        MainReason::logic_detail(format!("创建下载目录失败：{}（{e}）", dir.display()))
    })
}

static WORKDIR_LOCK: Mutex<()> = Mutex::new(());

/// 切 CWD（**不持锁**）：仅限单线程/生产场景；测试或任何并发场景请用 [`WorkDirWithLock`]。
pub struct WorkDir {
    original_dir: PathBuf,
}

impl WorkDir {
    pub fn change<S: Into<PathBuf>>(target_dir: S) -> std::io::Result<Self> {
        let original_dir = env::current_dir()?;
        let target = target_dir.into();
        info!("set current dir:{}", target.display());
        env::set_current_dir(&target)?;
        Ok(Self { original_dir })
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        info!("set current dir:{}", self.original_dir.display());
        if let Err(e) = env::set_current_dir(&self.original_dir) {
            log::error!("Failed to restore directory: {e}",);
        }
    }
}

/// 切 CWD 并**持锁**：`chdir` 是**进程级全局**，多线程/多测试并发切换会互相踩，
/// 所以凡是要改 CWD 的地方（尤其测试）都用它，不要用裸 `set_current_dir` 或 [`WorkDir`]。
pub struct WorkDirWithLock {
    original_dir: PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl WorkDirWithLock {
    pub fn change<S: Into<PathBuf>>(target_dir: S) -> std::io::Result<Self> {
        // 这把锁保护的是「进程级 CWD」这一全局状态，不是锁里的数据（`()`）。
        // 所以某个持有者 panic 只让锁「中毒」，**不该**把后续持锁者一起拖垮 —— 直接从中毒状态取回。
        let lock = WORKDIR_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let original_dir = env::current_dir()?;
        let target = target_dir.into();
        info!("set current dir:{}", target.display());
        env::set_current_dir(&target)?;
        Ok(Self {
            original_dir,
            _lock: lock,
        })
    }
}

impl Drop for WorkDirWithLock {
    fn drop(&mut self) {
        info!("set current dir:{}", self.original_dir.display());
        if let Err(e) = env::set_current_dir(&self.original_dir) {
            log::error!("Failed to restore directory: {e}",);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn resolve_work_dir_prefers_override_then_legacy_then_cache() {
        let tmp = TempDir::new().unwrap();
        let legacy = tmp.path().join("ds-package");
        let cache = tmp.path().join("cache/gops");

        // 1) env 覆盖最高优先（即使旧目录已存在）
        let over = tmp.path().join("explicit");
        assert_eq!(
            resolve_work_dir(Some(over.clone()), Some(legacy.clone()), cache.clone()),
            over
        );

        // 1b) env 写成空串 → 视为未设置（不能把空路径当工作目录）
        assert_eq!(
            resolve_work_dir(Some(PathBuf::new()), None, cache.clone()),
            cache.clone()
        );

        // 2) 无覆盖 + 旧目录**存在且是目录** → 沿用旧目录（不打断在跑的机器）
        std::fs::create_dir_all(&legacy).unwrap();
        assert_eq!(
            resolve_work_dir(None, Some(legacy.clone()), cache.clone()),
            legacy
        );

        // 3) 旧目录不存在（或不是目录） → 落到平台缓存目录
        assert_eq!(
            resolve_work_dir(None, Some(tmp.path().join("nope")), cache.clone()),
            cache
        );
        assert_eq!(resolve_work_dir(None, None, cache.clone()), cache.clone());

        // 4) 旧目录被同名**文件**占着（现场就这么来的） → 同样跳过它
        let as_file = tmp.path().join("as-file");
        std::fs::write(&as_file, b"tarball").unwrap();
        assert_eq!(resolve_work_dir(None, Some(as_file), cache.clone()), cache);
    }

    #[test]
    fn default_cache_dir_ends_with_gops() {
        // 不假设具体平台，只钉住“落在某个缓存根的 gops 子目录下”
        let dir = default_cache_dir();
        assert_eq!(dir.file_name().and_then(|n| n.to_str()), Some("gops"));
        assert!(dir.is_absolute() || dir.starts_with("."), "{dir:?}");
    }

    #[test]
    fn cache_base_prefers_nonempty_xdg_then_platform() {
        let home = PathBuf::from("/home/u");

        // 非空 XDG → 任何平台都用它
        assert_eq!(
            cache_base_from(Some(OsString::from("/xdg/cache")), Some(home.clone()), None),
            PathBuf::from("/xdg/cache")
        );
        // 空 XDG → 视为未设置，回落到平台根
        let empty = cache_base_from(Some(OsString::new()), Some(home.clone()), None);
        if cfg!(target_os = "macos") {
            assert_eq!(empty, home.join("Library/Caches"));
        } else if cfg!(target_os = "windows") {
            assert_eq!(empty, home.join("AppData/Local"));
        } else {
            assert_eq!(empty, home.join(".cache"));
        }

        // 无 home：不 panic，退化为相对路径
        assert!(cache_base_from(None, None, None).starts_with("."));
    }

    #[test]
    fn package_work_dir_is_never_empty() {
        // 守卫「空覆盖 → 空工作目录」那条回归
        assert!(!package_work_dir().as_os_str().is_empty());
    }

    /// 复刻现场事故：工作区路径被下载器写成了**文件**。
    ///
    /// 两层保护分别验证：① 解析时不再选它（回到缓存目录）；② 即便有人硬指向它，
    /// 也得到一句可操作的错，而不是后续 ENOTDIR 的“看不懂”。
    #[test]
    fn incident_work_dir_written_as_file() {
        let tmp = TempDir::new().unwrap();
        let work = tmp.path().join("ds-package");
        std::fs::write(&work, b"tarball-artifact").unwrap();
        let cache = tmp.path().join("cache/gops");

        let resolved = resolve_work_dir(None, Some(work.clone()), cache.clone());
        assert_eq!(resolved, cache, "不是目录 → 不能选它当工作区");

        let err = ensure_download_dir(&work).unwrap_err();
        let detail = err.detail().as_deref().unwrap_or_default();
        assert!(
            detail.contains(&work.display().to_string()),
            "应指出具体路径：{detail}"
        );
        // 报错不破坏现场
        assert_eq!(std::fs::read(&work).unwrap(), b"tarball-artifact");
    }

    #[test]
    fn ensure_download_dir_accepts_symlink_to_dir() {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        ensure_download_dir(&link).unwrap();
        assert!(link.is_dir());
    }

    #[test]
    fn ensure_download_dir_creates_when_missing_and_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("a/b/ds-package");
        ensure_download_dir(&dir).unwrap();
        assert!(dir.is_dir());
        // 幂等：已经是目录就不动
        ensure_download_dir(&dir).unwrap();
        assert!(dir.is_dir());
    }

    #[test]
    fn ensure_download_dir_rejects_file_and_dangling_symlink() {
        let tmp = TempDir::new().unwrap();
        // 文件占位（现场就是这么来的：下载器把目录路径当文件名写了）
        let f = tmp.path().join("ds-package");
        std::fs::write(&f, b"tarball").unwrap();
        let err = ensure_download_dir(&f).unwrap_err();
        let detail = err.detail().as_deref().unwrap_or_default();
        assert!(detail.contains("不是目录"), "detail={detail}");
        assert!(detail.contains("rm -rf"), "detail={detail}");
        // 报错时**不改动**现场（人自己决定删不删）
        assert!(f.is_file());

        // 悬空符号链接：is_dir()/exists() 都为 false，但 symlink_metadata 命中
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(tmp.path().join("nope"), &link).unwrap();
        assert!(ensure_download_dir(&link).is_err());
    }
}
