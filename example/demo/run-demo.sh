#!/usr/bin/env bash
#
# galaxy-ops 端到端演示脚本
#
# 演示 Module -> System -> Ops Project 三层对象的完整交付流程，
# 以及"一个系统、多客户"的组织方式。
#
# 用法:
#   ./run-demo.sh              # 使用 PATH 中的 gops
#   GOPS=/path/to/gops ./run-demo.sh
#
# 说明:
#   - 本脚本覆盖: mod example -> sys new -> sys package -> prj new -> prj import。
#   - 对全新系统（模块默认 enable:false、无 test_envs 依赖），以上步骤无网络依赖，
#     可离线运行。
#   - `gops sys new` 默认交互选择系统型号，脚本用 TEST_MODE=1 自动选择。
#   - 涉及真实模块下载、执行（download/install/start 等）仍需网络与外部 gflow。

set -euo pipefail

GOPS="${GOPS:-gops}"
WORK_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/gops-demo.XXXXXX")"

log()  { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[1;33m[!] %s\033[0m\n' "$*"; }

cleanup() {
  log "演示工作目录保留在（如需清理请手动删除）: ${WORK_ROOT}"
}
trap cleanup EXIT

command -v "$GOPS" >/dev/null 2>&1 || {
  warn "未找到 gops 命令，请先构建或安装：cargo build --bin gops"
  warn "或通过 GOPS=/path/to/gops ./run-demo.sh 指定路径"
  exit 1
}

log "使用 gops: $GOPS"
"$GOPS" --version

cd "$WORK_ROOT"

# ---- 1. Module ----
log "1/5 创建示例模块 (postgresql)"
"$GOPS" mod example

# ---- 2. System ----
log "2/5 创建系统 (web-stack)，TEST_MODE=1 自动选择系统型号"
TEST_MODE=1 "$GOPS" sys new --name web-stack

# ---- 3. Package ----
log "3/5 解析变量并打包系统（先 update 再打包）"
cd web-stack
"$GOPS" sys package
cd ..
ls -la web-stack-*.tar.gz

# ---- 4. Ops Projects（两个客户，导入同一个系统） ----
log "4/5 创建客户项目 customer-a 并导入系统"
"$GOPS" prj new --name customer-a
cd customer-a
"$GOPS" prj import --path ../web-stack-0.1.0.tar.gz
cd ..

log "5/5 创建客户项目 customer-b 并导入同一个系统"
"$GOPS" prj new --name customer-b
cd customer-b
"$GOPS" prj import --path ../web-stack-0.1.0.tar.gz
cd ..

log "演示完成。生成对象如下："
find "$WORK_ROOT" -maxdepth 2 -type d | sort
