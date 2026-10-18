//! docker-compose 调用的公共部分：文件发现、全局参数、override 合并。
//!
//! `gops run`（运行时分派）与 `gops sys perms`（从 compose 派生 bind 挂载）都要以
//! **系统根**为项目目录调用 docker compose。把这段规则放在库里，避免两处各写一份、
//! 慢慢漂移。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::system::path::SysOperatorPath;

/// 生效的 compose 主文件 + 调用 `docker compose` 时需放在子命令之前的全局参数。
///
/// 文件查找走 [`SysOperatorPath::compose_candidates`]：内收布局（`sys/` 下）优先，
/// 其次旧布局（系统根）。
pub fn compose_invocation(dir: &Path) -> (Option<PathBuf>, Vec<OsString>) {
    let candidates = SysOperatorPath::new(dir).compose_candidates();
    let compose_file = candidates.first().cloned();
    if candidates.len() > 1 {
        eprintln!(
            "warn: 检测到 {} 个 compose 文件，使用 {}（其余忽略）",
            candidates.len(),
            compose_file.as_deref().unwrap_or(Path::new("")).display()
        );
    }
    if compose_file.is_none() {
        eprintln!(
            "warn: 未找到 compose 文件（已按 sys/ 与系统根的 {{compose,docker-compose}}.{{yaml,yml}} 查找），交由 docker compose 自行发现"
        );
    }
    let args = compose_global_args(compose_file.as_deref(), dir);
    (compose_file, args)
}

/// 构造 `docker compose` 的全局参数（必须位于子命令之前）。
///
/// 分两种布局：
/// - **内收布局**（compose 在 `sys/` 下）：显式 `-f <file> --project-directory <系统根>`，
///   把项目目录锚回系统根，避免项目名与相对挂载基准漂到 `sys/`。显式 `-f` 会关闭 docker
///   对 override 的自动合并，因此这里同时显式合并同目录的 `*.override.{yaml,yml}`。
/// - **旧布局**（compose 就在系统根）：**不传任何全局参数**，退回 docker 自动发现。
///   这样能完整保留 docker 的 override 自动合并与 `COMPOSE_FILE` 环境变量语义，
///   保证旧仓不加改动照样跑。此时项目目录本就等于系统根，语义与内收布局一致。
///
/// 未找到 compose 文件时同样返回空参数，交由 docker 自行发现。
pub fn compose_global_args(compose_file: Option<&Path>, project_dir: &Path) -> Vec<OsString> {
    let Some(file) = compose_file else {
        return Vec::new();
    };
    // 旧布局：文件就在系统根，交给 docker 自动发现（保留 override 合并 / COMPOSE_FILE）。
    if file.parent() == Some(project_dir) {
        return Vec::new();
    }

    let mut args = vec![OsString::from("-f"), file.as_os_str().to_os_string()];
    for override_file in compose_override_files(file) {
        args.push(OsString::from("-f"));
        args.push(override_file.into_os_string());
    }
    args.push(OsString::from("--project-directory"));
    args.push(project_dir.as_os_str().to_os_string());
    args
}

/// 与 compose 主文件同目录、按 docker 约定命名的 override 文件（存在者）：
/// `<stem>.override.yaml` → `<stem>.override.yml`。
pub fn compose_override_files(base: &Path) -> Vec<PathBuf> {
    let (Some(stem), Some(dir)) = (base.file_stem().and_then(|s| s.to_str()), base.parent()) else {
        return Vec::new();
    };
    ["yaml", "yml"]
        .into_iter()
        .map(|ext| dir.join(format!("{stem}.override.{ext}")))
        .filter(|path| path.is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::compose_global_args;
    use std::ffi::OsString;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn test_compose_global_args_anchor_to_project_dir() {
        let root = Path::new("/srv/gateway");
        let file = root.join("sys/docker-compose.yaml");

        assert_eq!(
            compose_global_args(Some(&file), root),
            vec![
                OsString::from("-f"),
                OsString::from("/srv/gateway/sys/docker-compose.yaml"),
                OsString::from("--project-directory"),
                OsString::from("/srv/gateway"),
            ]
        );
    }

    #[test]
    fn test_compose_global_args_empty_for_root_layout() {
        // 旧布局（compose 就在系统根）：不传 global args，退回 docker 自动发现，
        // 以保留 docker-compose.override.yml 的自动合并与 COMPOSE_FILE 环境变量语义。
        let root = Path::new("/srv/gateway");
        assert!(compose_global_args(Some(&root.join("docker-compose.yml")), root).is_empty());
        assert!(compose_global_args(Some(&root.join("compose.yaml")), root).is_empty());
    }

    #[test]
    fn test_compose_global_args_merges_override_in_sys_layout() {
        // 显式 -f 会关闭 override 自动合并，因此内收布局要把同目录的 override 显式带上。
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sys")).unwrap();
        std::fs::write(root.join("sys/docker-compose.yaml"), "base").unwrap();
        std::fs::write(root.join("sys/docker-compose.override.yaml"), "override").unwrap();
        let file = root.join("sys/docker-compose.yaml");

        assert_eq!(
            compose_global_args(Some(&file), root),
            vec![
                OsString::from("-f"),
                root.join("sys/docker-compose.yaml").into_os_string(),
                OsString::from("-f"),
                root.join("sys/docker-compose.override.yaml")
                    .into_os_string(),
                OsString::from("--project-directory"),
                root.as_os_str().to_os_string(),
            ]
        );
    }

    #[test]
    fn test_compose_global_args_empty_without_compose_file() {
        // 未找到 compose 文件时不加全局参数，仍交由 docker 自行发现（兼容旧布局）
        assert!(compose_global_args(None, Path::new("/srv/gateway")).is_empty());
    }
}
