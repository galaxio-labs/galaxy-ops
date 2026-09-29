//! 端到端：`gops sys localize` 应让 `sys/setting/vars.yml` 的变更立即生效（issue #24）。
//!
//! 跑真实 `gops` 二进制 + 隔离 `$HOME`，覆盖：
//! 默认 `localize` **无条件**先重解析（改完 `vars.yml` 一条命令即生效）；
//! `--only` 始终跳过解析（用现有 `merged_vars.yml`）；
//! `gops sys check` 能报出「定义比已解析结果更新」的陈旧（仅比对 `.env` 看不到）。

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use tempfile::TempDir;

fn gops_bin() -> &'static str {
    env!("CARGO_BIN_EXE_gops")
}

fn run_gops(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(gops_bin())
        .current_dir(cwd)
        .env("HOME", home)
        .args(args)
        .output()
        .expect("gops should run")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// 显式设置 mtime，避免依赖挂钟撮合。
fn set_mtime(path: &Path, t: SystemTime) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(t).unwrap();
}

/// 把变量定义写进 `sys/setting/vars.yml`（解析的唯一输入）。
fn write_vars(sys: &Path, domain: &str) {
    std::fs::write(
        sys.join("sys/setting/vars.yml"),
        format!("system:\n  - name: DOMAIN\n    value: {domain}\n"),
    )
    .unwrap();
}

fn write_merged(sys: &Path, domain: &str) {
    std::fs::write(
        sys.join("sys/merged_vars.yml"),
        format!("system:\n  - name: DOMAIN\n    value: {domain}\n"),
    )
    .unwrap();
}

fn setup() -> (TempDir, TempDir, PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let out = run_gops(
        home.path(),
        root.path(),
        &["sys", "new", "--name", "demo", "--kind", "docker-compose"],
    );
    assert!(out.status.success(), "sys new failed: {}", stderr(&out));
    let sys = root.path().join("demo");
    (home, root, sys)
}

fn env_text(sys: &Path) -> String {
    std::fs::read_to_string(sys.join(".env")).unwrap()
}

/// 改 `sys/setting/vars.yml` → 一条 `gops sys localize` 即让 `.env` 生效。
#[test]
fn localize_picks_up_vars_yml_change() {
    let (home, _root, sys) = setup();
    write_vars(&sys, "a.example");

    let out = run_gops(home.path(), &sys, &["sys", "localize"]);
    assert!(out.status.success(), "localize failed: {}", stderr(&out));
    assert!(
        env_text(&sys).contains("DOMAIN=a.example"),
        ".env should hold the resolved value, got: {}",
        env_text(&sys)
    );

    // 再改一次，仍应一条 localize 生效
    write_vars(&sys, "b.example");
    let out = run_gops(home.path(), &sys, &["sys", "localize"]);
    assert!(out.status.success(), "localize failed: {}", stderr(&out));
    assert!(
        env_text(&sys).contains("DOMAIN=b.example"),
        "localize must reflect the new value, got: {}",
        env_text(&sys)
    );
}

/// 默认 `localize` **无条件**重解析：手动塞入的 `merged_vars.yml` 会被覆盖。
#[test]
fn localize_always_reresolves() {
    let (home, _root, sys) = setup();
    write_vars(&sys, "a.example");
    let out = run_gops(home.path(), &sys, &["sys", "localize"]);
    assert!(out.status.success(), "{}", stderr(&out));

    // 塞入“错误”的已解析结果
    write_merged(&sys, "bogus.example");

    let out = run_gops(home.path(), &sys, &["sys", "localize"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let merged = std::fs::read_to_string(sys.join("sys/merged_vars.yml")).unwrap();
    assert!(
        !merged.contains("bogus.example"),
        "default localize must re-resolve from vars.yml, got merged: {merged}"
    );
    assert!(
        env_text(&sys).contains("DOMAIN=a.example"),
        "{}",
        env_text(&sys)
    );
}

/// `--only` 始终跳过解析（用现有 `merged_vars.yml`，不重解析）。
#[test]
fn localize_only_never_resolves() {
    let (home, _root, sys) = setup();
    write_vars(&sys, "a.example");
    let out = run_gops(home.path(), &sys, &["sys", "localize"]);
    assert!(out.status.success(), "{}", stderr(&out));

    // 直接塞一个“错误”的已解析结果；--only 不应重解析
    write_merged(&sys, "bogus.example");

    let out = run_gops(home.path(), &sys, &["sys", "localize", "--only"]);
    assert!(out.status.success(), "--only failed: {}", stderr(&out));
    assert!(
        env_text(&sys).contains("DOMAIN=bogus.example"),
        "--only must localize from the existing resolved vars, got: {}",
        env_text(&sys)
    );
}

/// `gops sys check` 在「定义比已解析结果更新」时报 `[WARN]`（仅比对 `.env` 看不到）。
#[test]
fn check_warns_when_definition_newer_than_merged() {
    let (home, _root, sys) = setup();
    write_vars(&sys, "a.example");
    let out = run_gops(home.path(), &sys, &["sys", "localize"]);
    assert!(out.status.success(), "{}", stderr(&out));

    // 制造陈旧：把所有“非 vars.yml”输入都钉到不新于 merged，再让 vars.yml 更新
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    set_mtime(&sys.join("sys/sys_model.yml"), base);
    set_mtime(&sys.join("sys/setting/vars.yml"), base);
    set_mtime(
        &sys.join("sys/merged_vars.yml"),
        base + Duration::from_secs(10),
    );
    write_vars(&sys, "b.example");
    set_mtime(
        &sys.join("sys/setting/vars.yml"),
        base + Duration::from_secs(60),
    );
    set_mtime(
        &sys.join("sys/setting/vars.yml"),
        base + Duration::from_secs(60),
    );

    let chk = run_gops(home.path(), &sys, &["sys", "check"]);
    assert!(
        stdout(&chk).contains("[WARN]"),
        "check should warn about stale vars, got: {}",
        stdout(&chk)
    );

    // 重新 localize 后不再陈旧
    let out = run_gops(home.path(), &sys, &["sys", "localize"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let chk = run_gops(home.path(), &sys, &["sys", "check"]);
    assert!(
        !stdout(&chk).contains("[WARN]"),
        "no stale warning after localize, got: {}",
        stdout(&chk)
    );
}
