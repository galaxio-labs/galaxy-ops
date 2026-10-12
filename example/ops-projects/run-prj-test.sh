#!/usr/bin/env bash
#
# obs_prj_x：`gops prj` 端到端冒烟测试
#
# 对 example/ops-projects/obs_prj_x 这个运维项目，逐个跑 `gops prj` 的现场态命令并做基本断言：
#   diagnose / sync（含“补齐缺失系统”）/ backup / restore / contract / upgrade（guard 负例）
#
# 用法:
#   ./run-prj-test.sh                    # 先 cargo test 重新生成 obs_prj_x，再测
#   GOPS=/path/to/gops ./run-prj-test.sh # 指定 gops（默认 target/{debug,release}/gops，再退 PATH；都没有则自动 cargo build）
#   REGEN=0 ./run-prj-test.sh            # 不重新生成，直接测当前 obs_prj_x
#
# 说明:
#   - obs_prj_x 是**测试生成的**示例（见 tests/sys_test.rs::test_full_flow），默认会被重新生成，
#     保证从已知状态出发；生成只清 ./obs_prj_x，不会动本脚本。
#   - 全程离线：示例系统的 `addr` 指向本仓内的本地包，不下载、不联网。
#   - `prj upgrade` 只支持 kind: docker-compose，而 example_sys_x 是 gxl，故本脚本把它当作
#     **负例**：断言它被明确拒绝（而不是静默通过）。
#   - 为让 diagnose 的“values/ 是否纳入版本控制”检查有意义（示例目录本身被上层 .gitignore 忽略），
#     会在 obs_prj_x 内 `git init`。

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PRJ_DIR="$SCRIPT_DIR/obs_prj_x"

# ---- gops 定位（缺失时自动构建，便于在 CI / 干净检出里一条命令跑通）----
if [ -z "${GOPS:-}" ]; then
  if [ -x "$REPO_ROOT/target/debug/gops" ]; then
    GOPS="$REPO_ROOT/target/debug/gops"
  elif [ -x "$REPO_ROOT/target/release/gops" ]; then
    GOPS="$REPO_ROOT/target/release/gops"
  elif command -v gops >/dev/null 2>&1; then
    GOPS="gops"
  else
    echo "未找到 gops，自动构建：cargo build --bin gops" >&2
    (cd "$REPO_ROOT" && cargo build --bin gops) || { echo "构建 gops 失败" >&2; exit 1; }
    GOPS="$REPO_ROOT/target/debug/gops"
  fi
fi
if ! command -v "$GOPS" >/dev/null 2>&1 && [ ! -x "$GOPS" ]; then
  echo "未找到可执行的 gops：$GOPS（用 GOPS=... 指定）" >&2
  exit 1
fi

FAIL=0
log() { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
ok()  { printf '   \033[32m✓\033[0m %s\n' "$*"; }
bad() { printf '   \033[31m✗\033[0m %s\n' "$*"; FAIL=1; }

log "使用 gops: $GOPS"
"$GOPS" --version || true

# ---- 0. 重新生成示例（可选）----
if [ "${REGEN:-1}" = "1" ]; then
  log "0. 重新生成 obs_prj_x（cargo test --test sys_test test_full_flow）"
  if (cd "$REPO_ROOT" && cargo test --quiet --test sys_test test_full_flow); then
    ok "示例已重新生成"
  else
    bad "生成示例失败"
    exit 1
  fi
fi

[ -d "$PRJ_DIR" ] || { echo "示例目录不存在：$PRJ_DIR" >&2; exit 1; }
[ -f "$PRJ_DIR/ops-prj.yml" ] || { echo "缺 ops-prj.yml：$PRJ_DIR" >&2; exit 1; }

cd "$PRJ_DIR"

# 让 diagnose 的“values/ 是否入库”检查有意义（示例目录被上层 .gitignore 忽略）。
git init -q . 2>/dev/null || true

# ---- 1. diagnose ----
log "1. prj diagnose"
if out=$("$GOPS" prj diagnose 2>&1); then
  echo "$out"
  if printf '%s\n' "$out" | grep -q '\[FAIL\]'; then
    bad "diagnose 报出 FAIL"
  else
    ok "diagnose 通过（无 FAIL）"
  fi
else
  echo "$out"
  bad "diagnose 退出码非 0"
fi

# ---- 2. sync：非破坏覆盖 ----
log "2. prj sync（非破坏覆盖包内内容）"
if "$GOPS" prj sync; then
  ok "sync 成功"
else
  bad "sync 失败"
fi

# ---- 3. sync：补齐缺失系统 ----
log "3. prj sync 补齐缺失系统（删掉系统目录后应被按包重新导入）"
rm -rf example_sys_x
if "$GOPS" prj sync && [ -f example_sys_x/sys/sys_model.yml ] && [ -L example_sys_x/values ]; then
  ok "缺失系统已被重新导入（含 values 符号链接）"
else
  bad "缺失系统未被正确补齐"
fi

# ---- 4. backup ----
# 造一个现场态文件：`.env` 在 `sys-prj.yml` 的 `backup:` 里（模拟 `sys localize` 产出）。
# 注意 `contract: configs`（对外契约）**不进** `backup`，故 `prj backup` 只收这里声明的现场态。
printf 'APP_ENV=demo\n' > example_sys_x/.env
log "4. prj backup（收集现场态：backup 声明的 .env；不含 contract）"
if "$GOPS" prj backup; then
  BKP="$(ls -t backup/*.tar.gz 2>/dev/null | head -1)"
  if [ -n "$BKP" ]; then ok "生成备份：$BKP"; else bad "未生成备份归档"; fi
else
  BKP=""
  bad "backup 失败"
fi

# ---- 5. restore：--list / --dry-run ----
log "5. prj restore --list / --dry-run"
if [ -n "$BKP" ]; then
  if "$GOPS" prj restore --list "$BKP" | grep -q 'example_sys_x/.env'; then
    ok "--list 列出备份的现场态（example_sys_x/.env）"
  else
    bad "--list 未列出预期的现场态条目"
  fi
  if "$GOPS" prj restore --dry-run "$BKP"; then
    ok "--dry-run 通过（未改动现场）"
  else
    bad "restore --dry-run 失败"
  fi
else
  bad "跳过 restore：无备份归档"
fi

# ---- 6. contract：export / import ----
log "6. prj contract export / import"
if "$GOPS" prj contract export; then
  CEXP="$(ls -t contract/*.tar.gz 2>/dev/null | head -1)"
  if [ -n "$CEXP" ]; then
    ok "导出契约归档：$CEXP"
    if "$GOPS" prj contract import "$CEXP"; then
      ok "导入契约归档成功"
    else
      bad "contract import 失败"
    fi
  else
    bad "未生成契约归档"
  fi
else
  bad "contract export 失败"
fi

# ---- 7. upgrade：负例（非 compose 应被拒）----
log "7. prj upgrade 负例（example_sys_x 是 gxl，应被明确拒绝）"
if "$GOPS" prj upgrade --to 1.0.0 --on-failure halt >/dev/null 2>&1; then
  bad "非 compose 系统不应通过 upgrade"
else
  ok "非 compose 被正确拒绝（符合预期）"
fi

# ---- 结果 ----
log "结果"
if [ "$FAIL" = 0 ]; then
  printf '\033[32m全部通过\033[0m\n'
  exit 0
else
  printf '\033[31m存在失败项\033[0m\n'
  exit 1
fi
