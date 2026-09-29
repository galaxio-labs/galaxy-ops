//! 端到端：`gops sys localize` 的「阶段扩展点」（issue #23）。
//!
//! 跑**真实的 `gops` 二进制**，并把 `$HOME/bin/gx` 换成一个假脚本，从而在进程级
//! （无并发 env 竞争）验证：探测通过则执行、注入合并值、跳过路径、失败语义。

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

fn gops_bin() -> &'static str {
    env!("CARGO_BIN_EXE_gops")
}

/// 写一个假的 `$HOME/bin/gx`：`--exists` 时按 `GX_EXISTS_RC` 退出（默认 0=存在），
/// 否则把注入的环境变量记到 `$MARKER`，再按 `GX_RUN_RC` 退出。
fn write_fake_gx(home: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gx = bin.join("gx");
    std::fs::write(
        &gx,
        r#"#!/bin/sh
for a in "$@"; do
  if [ "$a" = "--exists" ]; then exit "${GX_EXISTS_RC:-0}"; fi
done
{ printf 'DOMAIN=%s\n' "$DOMAIN"; printf 'BASE_URL=%s\n' "$BASE_URL"; printf 'PATH=%s\n' "$PATH"; } > "$MARKER"
exit "${GX_RUN_RC:-0}"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&gx, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn run_gops(home: &Path, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(gops_bin());
    cmd.current_dir(cwd).env("HOME", home);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.args(args).output().expect("gops should run")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// 建一个 compose 系统（真实 `gops sys new`），并写 merged_vars + `_gal/work.gxl`。
fn setup_system(home: &Path, root: &Path) -> PathBuf {
    let out = run_gops(
        home,
        root,
        &["sys", "new", "--name", "demo", "--kind", "docker-compose"],
        &[],
    );
    assert!(out.status.success(), "sys new failed: {}", stderr(&out));

    let sys = root.join("demo");
    // 变量定义写在 `sys/setting/vars.yml`（`localize` 会从它重解析出 `merged_vars.yml`）。
    // `BASE_URL` 含 `${DOMAIN}`（验证注入值与 `.env` 一样**展开**）；
    // `PATH` 故意设成不存在的路径：它必须**不被注入**（保留变量）。
    std::fs::write(
        sys.join("sys/setting/vars.yml"),
        "system:\n\
         - name: DOMAIN\n  value: example.test\n\
         - name: BASE_URL\n  value: \"http://${DOMAIN}:5432\"\n\
         - name: PATH\n  value: /nonexistent-should-not-apply\n",
    )
    .unwrap();
    std::fs::create_dir_all(sys.join("_gal")).unwrap();
    std::fs::write(
        sys.join("_gal/work.gxl"),
        "mod envs { env default {} }\nmod main { flow localize { gx.echo ( value : \"x\" ) ; } }\n",
    )
    .unwrap();
    sys
}

fn setup() -> (TempDir, TempDir, PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    write_fake_gx(home.path());
    let sys = setup_system(home.path(), root.path());
    (home, root, sys)
}

#[test]
fn stage_flow_runs_and_injects_merged_env() {
    let (home, root, sys) = setup();
    let marker = root.path().join("marker.txt");

    let out = run_gops(
        home.path(),
        &sys,
        &["sys", "localize"],
        &[("MARKER", marker.to_str().unwrap())],
    );
    assert!(out.status.success(), "localize failed: {}", stderr(&out));

    // `.env` 写出
    assert!(sys.join(".env").exists(), ".env should be written");

    // 流程被执行，且合并值以环境变量注入；`BASE_URL` 已展开（与 `.env` 一致）
    let recorded = std::fs::read_to_string(&marker).expect("stage flow should have run");
    assert!(
        recorded.contains("DOMAIN=example.test"),
        "merged value should be injected, got: {recorded}"
    );
    assert!(
        recorded.contains("BASE_URL=http://example.test:5432"),
        "injected value must be expanded like .env, got: {recorded}"
    );
    // 保留变量 PATH 不被合并配置覆盖
    assert!(
        !recorded.contains("PATH=/nonexistent-should-not-apply"),
        "reserved PATH must not be injected, got: {recorded}"
    );
    // 与 `.env` 交叉核对：同一展开值
    let env = std::fs::read_to_string(sys.join(".env")).unwrap();
    assert!(
        env.contains("BASE_URL=http://example.test:5432"),
        ".env should hold the same expanded value, got: {env}"
    );
}

#[test]
fn stage_flow_skipped_when_absent() {
    let (home, root, sys) = setup();
    let marker = root.path().join("marker.txt");

    // 探测返回非 0：视为“无该流程” → 跳过
    let out = run_gops(
        home.path(),
        &sys,
        &["sys", "localize"],
        &[("MARKER", marker.to_str().unwrap()), ("GX_EXISTS_RC", "1")],
    );
    assert!(
        out.status.success(),
        "should still succeed: {}",
        stderr(&out)
    );
    assert!(!marker.exists(), "absent flow must be skipped");
}

#[test]
fn no_flow_flag_skips_the_stage() {
    let (home, root, sys) = setup();
    let marker = root.path().join("marker.txt");

    let out = run_gops(
        home.path(),
        &sys,
        &["sys", "localize", "--no-flow"],
        &[("MARKER", marker.to_str().unwrap())],
    );
    assert!(out.status.success(), "should succeed: {}", stderr(&out));
    assert!(!marker.exists(), "--no-flow must skip the stage flow");
}

#[test]
fn stage_flow_failure_fails_localize() {
    let (home, root, sys) = setup();
    let marker = root.path().join("marker.txt");

    let out = run_gops(
        home.path(),
        &sys,
        &["sys", "localize"],
        &[("MARKER", marker.to_str().unwrap()), ("GX_RUN_RC", "3")],
    );
    assert!(
        !out.status.success(),
        "a non-zero stage flow must fail the whole localize"
    );
    // `.env` 已经写出（顺序：先 .env 后流程）
    assert!(sys.join(".env").exists(), ".env should already be written");
}

#[test]
fn skip_reason_is_visible_at_debug_level() {
    let (home, root, sys) = setup();
    let marker = root.path().join("marker.txt");
    let env = [("MARKER", marker.to_str().unwrap()), ("GX_EXISTS_RC", "1")];

    // 默认：静默跳过（不打扰）
    let quiet = run_gops(home.path(), &sys, &["sys", "localize"], &env);
    assert!(quiet.status.success(), "{}", stderr(&quiet));
    assert!(
        !stderr(&quiet).contains("skip stage flow"),
        "default should stay silent, got: {}",
        stderr(&quiet)
    );

    // `-d 1`：给出跳过原因，便于发现“写了流程却没跑”
    let debug = run_gops(home.path(), &sys, &["sys", "localize", "-d", "1"], &env);
    assert!(debug.status.success(), "{}", stderr(&debug));
    assert!(
        stderr(&debug).contains("skip stage flow"),
        "debug should explain the skip, got: {}",
        stderr(&debug)
    );
}

#[test]
fn stage_flow_skipped_when_gx_missing() {
    let home = tempfile::tempdir().unwrap(); // 无 bin/gx
    let root = tempfile::tempdir().unwrap();
    let sys = setup_system(home.path(), root.path());
    let marker = root.path().join("marker.txt");

    let out = run_gops(
        home.path(),
        &sys,
        &["sys", "localize"],
        &[("MARKER", marker.to_str().unwrap())],
    );
    assert!(
        out.status.success(),
        "compose must work without gx: {}",
        stderr(&out)
    );
    assert!(!marker.exists());
}
