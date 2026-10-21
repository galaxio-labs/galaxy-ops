# `gops run diagnose`：运行时体检（gops 与 `diagnose` 流程的分工）

> 状态：**第 1 步已实现**（`gops run diagnose` / `gops run config`，2026-10-21）。
> 已定：`--logs N`（默认 200，`0` = 不抓）、人读默认按 TTY 着色（`--no-color` / `NO_COLOR` 关）、
> `--fail-on fail|warn`（默认 `fail`）、`--timeout`（默认 60s，钳在 5–3600）。
> 流程侧落点：`sys/workflows/operators.gxl` 的 `mod operators { flow diagnose { … } }`（见 §3.2）。
> 待办：`wist-gateway-stack` 的 `flow diagnose` 首个检查（`wparse.work_drift`）。
> 触发场景：`wist-gateway-stack`（`kind: docker-compose`）现场排查「`/data/topology` 里少了 3 个
> sink 文件」，耗时长——因为 `gops run diagnose` 现在只打印 `docker compose config`，
> 而真正能发现差异的比对逻辑写死在栈的私有脚本 `scripts/show-wparse-state.sh` 里。

## 1. 现状（代码核对）

| 事实 | 证据 |
| --- | --- |
| `gops run diagnose`（docker-compose）**就是** `docker compose config` | `app/gops/commands/run_cmd.rs:202-206`（`diagnose -> config`）、`src/system/README.md:85` |
| 它只输出**渲染后的编排模型**：服务、镜像、env、mount 的 source/target、卷名；密钥掩码 `********` | 同上 + `src/system/README.md:229` |
| `gops sys` **没有** `diagnose` 子命令（只有 `new / resolve / package / localize / diff / perms`） | `app/gops/commands/sys_cmd.rs:159-216` |
| `src/system/README.md:229` 里「`sys diagnose`（`docker compose config`）」是**过期/错误描述** | 同上，无对应实现 |
| 真正的诊断能力已有：`gops prj diagnose`（定义/交付面：`values/` 纳管、`contract` 齐备、perms） | `src/ops_prj/diagnose.rs:250` |
| 系统本地运维流写在 `sys/workflows/operators.gxl` 的 `mod operators { flow <name> {…} }`，由 `_gal/work.gxl` 的 `mod main : operators {}` 纳入，`gops sys localize` 用 `gx run localize` 触发 | `local-work/gateway-local/wist-gateway-stack/sys/workflows/operators.gxl`、`src/system/init/_gal/work.gxl:23-27` |
| `gops` 已有「阶段流程」机制：`gx run --exists <flow>` 探测 + 合并配置注入环境变量 | `app/gops/commands/sys_cmd.rs:797`（`run_stage_flow`）、`app/gops/commands/gx_dispatch.rs:131` |

结论：**名字对不上**。`diagnose` 被用来指「打印渲染结果」，而「诊断」这个词本该指
「环境事实 + 期望 vs 实际」。且「期望」里有一半是**业务知识**（如 wparse 的
`project_remote` 克隆 vs `/data` 工作根），gops 不可能内置。

## 2. 目标与分工原则

一句话：**gops 管「是什么」，流程管「应该是什么」。**

| | gops | `diagnose` 流程（GXL） |
| --- | --- | --- |
| 懂什么 | 编排语义、容器/挂载/端口/证书这类**通用事实** | 这个系统的**业务语义**（谁同步谁、哪个版本该生效、退出码 75 是什么意思） |
| 干什么 | 采集 → 内置检查 → 调流程 → 归并 → 呈现 → 退出码 | 读 gops 采好的事实，跑本系统专属比对，交回结构化 checks |
| 不干什么 | 不判断业务内容对不对、不修现场 | 不重跑通用采集、不打印人读表格、不决定退出码、不修现场 |

判据：**换一个栈仍然成立的检查归 gops；只有这个栈知道的检查归流程。**

## 3. 命名与落点（本次要定的两件事）

### 3.1 命令

| 命令 | 语义 | 备注 |
| --- | --- | --- |
| `gops run diagnose` | **运行时体检**（本方案） | 默认人读；`--json` 机器读；退出码有含义 |
| `gops run config` | 现在的行为：打印 `docker compose config` | 已拆出（`compose_subcommand("config") → config`） |
| `gops sys diagnose` | **不加** | `sys` 面是定义/交付；README:229 的错误描述随本方案删除 |

兼容：`diagnose` 保留**旧语义的兜底**一个版本——当系统既没有 compose 模型、也没有 `diagnose`
流程可用时，打印 config 并提示「这不是诊断」。脚本若仍依赖旧输出，改用 `run config`。

### 3.2 流程文件：`operators.gxl`，不是 `diagnose.gxl`

**结论：写在 `sys/workflows/operators.gxl` 的 `mod operators` 里，与现有 `flow localize` 同构。**

理由：

1. 本栈约定的加载链是 `_gal/work.gxl` → `extern mod operators { path = "./sys/workflows" }`
   → `mod main : operators {}`；GXL 的 `path` 指向**目录**，mod 名取**文件名** `operators`。
   单独放一个 `diagnose.gxl` 不会被加载，除非改继承链。
2. GXL 示例全是**单继承**（`mod X : Y {}`）；把 `diagnose.gxl` 挂进 `main` 需要多继承，
   属未验证语法，不该为「多一个文件」引入结构风险。
3. `operators.gxl` 已经被栈用作**本系统本地运维流**的落点（`flow localize` 就在里面），
   `flow diagnose` 放进去是同一层语义，零结构改动。

**例外（什么时候才拆成独立 `.gxl`）**：诊断项要跨栈复用、要发成包（进 `ops-gxl` 之类的上游仓库）、
或文件大到影响可读性。届时拆出的 mod 由 `operators` 侧引入，`gops` 侧契约不变（只认流程名）。

## 4. gops 侧：五段流水线

| 段 | 内容 | 失败语义 |
| --- | --- | --- |
| ① 采集 | `docker compose config`（解析结果）+ `ps` + `inspect`（状态/health/restart/exit/OOM/ports/mounts）+ 宿主 stat（bind source 存在性、属主、权限）+ 日志尾巴 | 采集失败 = 诊断无法进行 → 退出码 `2` |
| ② 内置检查 | 见 §7 左列 | 记 `[FAIL]`，继续 |
| ③ 扩展 | `gx run --exists diagnose` 探测；存在则注入上下文执行；`--no-ext` 关闭 | 缺失 → 跳过（不记项）；崩溃 → `[FAIL] gops.ext.diagnose`（带 stderr 末 20 行） |
| ④ 归并 | 内置 + 扩展 checks 合并；排序 `fail → warn → ok`，同级按 `source`、`code` | — |
| ⑤ 呈现 | 人读 / `--json`；退出码 | — |

全程**只读**：不 `cp` / 不 `chmod` / 不重启。修复动作只给 `hint`。

## 5. 契约 A：gops → 流程（Context Bundle v1）

gops 写临时 JSON（0600，运行结束删除；`--keep-context` 保留），路径经环境变量传入：

```
GOPS_DIAGNOSE_CONTEXT=/tmp/gops-diagnose-XXXX/context.json
GOPS_DIAGNOSE_RESULT=/tmp/gops-diagnose-XXXX/result.json
GOPS_SYSTEM_ROOT=<系统根绝对路径>
GOPS_SYSTEM_NAME=wist-gateway-stack
GOPS_SYSTEM_KIND=docker-compose|gxl
GOPS_DIAGNOSE_ENV=default
```

```json
{
  "schema_version": "1",
  "generated_at": "2026-10-21T11:55:00+08:00",
  "system": { "name": "wist-gateway-stack", "kind": "docker-compose", "root": "/…/wist-gateway-stack" },
  "compose": {
    "project": "wist-gateway-stack",
    "services": [
      {
        "name": "wparse",
        "container": "wist-gateway-stack-wparse",
        "image": "ghcr.io/wp-labs/warp-parse:0.31.1-alpha",
        "state": "running", "health": null, "restarts": 0, "exit_code": null,
        "ports": [{ "target": 9000, "published": "9000", "host_ip": null }],
        "mounts": [
          { "type": "bind", "source": "/…/data-plane-run/work", "target": "/data",
            "read_only": false, "source_exists": true, "owner": "999:999", "mode": "0755" },
          { "type": "volume", "source": "wist-gateway-stack-wparse-run", "target": "/data/.run",
            "driver": "local", "empty": false }
        ]
      }
    ]
  },
  "host": { "os": "macos", "docker": "27.x", "user": "zuowenjian" },
  "config": { "resolved": { "…": "docker compose config 结果，密钥已掩码" } }
}
```

约束：

- 路径一律**绝对路径**；只有 `config.resolved` 可能因体积被折叠（超 2 MiB → `{"omitted": true}`）。
- 字段**只增不改**；破坏性变更必须抬 `schema_version`。
- 密钥沿用现有掩码 `********`（`src/system/README.md:229`）。

## 6. 契约 B：流程 → gops（Flow Result v1）

流程写 `$GOPS_DIAGNOSE_RESULT`，**不写 stdout**（stdout 归流程自己调试，gops 收进 `--verbose`）：

```json
{
  "schema_version": "1",
  "source": "diagnose",
  "checks": [
    {
      "code": "wparse.work_drift",
      "status": "fail",
      "title": "工作目录与远端克隆不一致",
      "detail": "clone 有 3 个文件未落到 /data：topology/sinks/business.d/{ignore,raw_log,sink}.toml；其余 12 个 hash 相同",
      "hint": "确认发布是否只更新了 clone；比对命令见 scripts/show-wparse-state.sh",
      "evidence": {
        "only_source": ["topology/sinks/business.d/ignore.toml"],
        "only_work": [],
        "changed": []
      }
    }
  ]
}
```

约束：

- `code` **稳定 + 带命名空间**（`wparse.` / `gateway.` / `center.`），同一次运行内不得重复。
- `status ∈ ok|warn|fail`，与 gops 内置同一套词（不引 `INFO/WARN/ERROR`）。
- `evidence` 自由对象，单条 ≤ 16 KiB，必须可 JSON 化。
- 文件缺失 / `checks` 为空 = 流程没给结论 → 记 `[WARN] gops.ext.diagnose`（「流程未输出结果」），**不判 FAIL**。

## 7. 检查项归属

| 检查 | code | 归属 |
| --- | --- | --- |
| compose 可解析、服务数、镜像标签 | `gops.compose.render` | gops |
| 声明了但容器不存在 / 非 running（**一次性任务 `restart: no` 且 exit 0 豁免**） | `gops.container.<svc>` | gops |
| unhealthy / 反复重启 / exit code / OOMKilled | `gops.container.<svc>.health` / `.restarts` / `.oom` | gops |
| bind source 存在性、属主、权限（按服务聚合成一条） | `gops.mount.<svc>` | gops |
| named volume 存在且非空 | `gops.volume.<name>` | gops |
| published 端口重复发布（同 host:port/proto） | `gops.port.<host:port/proto>` | gops |
| 证书/私钥存在且权限不过宽（私钥类路径） | `gops.cert.<svc>` | gops |
| 容器最近 error 日志摘要（只采集，不解读） | `gops.logs.<svc>` | gops |
| **clone vs 工作根逐文件漂移** | `wparse.work_drift` | **flow** |
| state 记录的已应用版本 vs 实际生效 | `wparse.applied_version` | **flow** |
| admin token 属主 == 容器 uid | `wparse.admin_token` | **flow** |
| 规则仓库可达 / TLS 信任锚 | `wparse.remote_reachable` | **flow** |
| 退出码 75 的业务含义（flock 抢锁） | `wparse.flock_contention` | **flow** |

## 8. 输出

### 8.1 人读（默认，stdout）

```
wist-gateway-stack · docker-compose · env=default

[FAIL] wparse.work_drift
       工作目录与远端克隆不一致
       clone 有 3 个文件未落到 /data：topology/sinks/business.d/{ignore,raw_log,sink}.toml；其余 12 个 hash 相同
       → 确认发布是否只更新了 clone；比对命令见 scripts/show-wparse-state.sh

[OK] gops.container.wparse
       `wparse` running
       container=wist-gateway-stack-wparse image=ghcr.io/wp-labs/warp-parse:0.31.1-alpha
[WARN] gops.logs.wparse
       `wparse` 最近 200 行含 12 条错误线索
       2026-10-21 … unauthorized request_id=…

gops: 7 ok / 1 warn / 0 fail   diagnose: 0 ok / 0 warn / 1 fail
结论: 7 项正常 / 1 项警告 / 1 项失败；首要问题：工作目录与远端克隆不一致
```

颜色沿用 `prj diagnose` 既有码（`OK/WARN/FAIL` + ANSI），不引新词汇。

### 8.2 `--json`（stdout 只有 JSON；人读与调试走 stderr）

```json
{
  "schema_version": "1",
  "system": { "name": "wist-gateway-stack", "kind": "docker-compose", "env": "default" },
  "generated_at": "2026-10-21T11:55:00+08:00",
  "summary": { "ok": 7, "warn": 1, "fail": 1, "exit_code": 1 },
  "sources": [
    { "name": "gops", "checks": 8, "ok": 7, "warn": 1, "fail": 0, "ran": true },
    { "name": "diagnose", "checks": 1, "ok": 0, "warn": 0, "fail": 1, "ran": true }
  ],
  "checks": [
    { "code": "wparse.work_drift", "source": "diagnose", "status": "fail",
      "title": "工作目录与远端克隆不一致", "detail": "…", "hint": "…", "evidence": { } }
  ]
}
```

- `checks[]` 顺序：`fail → warn → ok`，同级按 `source`、`code`。
- `--include-context` 才带 §5 的 `context`（默认不带，避免 JSON 退化成配置转储）。

### 8.3 退出码

| 码 | 含义 |
| --- | --- |
| `0` | 无 `fail`（`warn` 允许；`--fail-on warn` 收紧） |
| `1` | 至少一条 `fail`（含 `gops.ext.diagnose` 崩溃） |
| `2` | 诊断自身无法完成（compose 不可解析、采集失败、参数错） |

## 9. `kind` 分派

| kind | 行为 |
| --- | --- |
| `docker-compose` | gops 采集 + 内置检查 + （有则）`gx run diagnose` 扩展 + 归并呈现 |
| `gxl` | gops 只做：`gx run --exists diagnose` → `gx run -e <env> diagnose`（注入 §5 环境变量）→ 读结果 → 归并呈现/退出码。**无** compose 内置检查；流程不存在或 `--no-ext` 时记 WARN（`flow_absent` / `flow_skipped`），**不静默** |

## 10. 实现落点（已落地）

gops 侧：

- `app/gops/commands/run_cmd.rs`：`RunCmd::Config`（新）+ `RunCmd::Diagnose(RunDiagnoseArgs)`（体检入口，
  `handle_diagnose` 串起采集 → 内置 → 流程 → 渲染）；`compose_subcommand` 里 `diagnose -> config` 已移除。
- `src/system/diagnose/{mod,context,checks,report}.rs`：交接目录（`Bundle`，0700/0600 的临时目录）、
  事实采集、内置检查、报告/排序/渲染/退出码。
- `app/gops/commands/gx_dispatch.rs`：`probe_flow`（三态）+ `run_gx_flow_capture`（超时 + 输出走 stderr）。
- `src/system/README.md`：映射与过期描述已改正。

栈侧（`wist-gateway-stack`）：

- `sys/workflows/operators.gxl` 增 `flow diagnose`。
- `scripts/diagnose-wparse-drift.sh`（**已落地**）：clone vs 工作根的逐文件 sha256 比对
  （models / conf / topology / connectors），产出 `wparse.work_drift`；并比 `project_remote_state.json`
  的 revision 与克隆 HEAD，产出 `wparse.applied_version.{models,infra}`。容器靠 `docker compose ps -q` 自己找，
  脚本经 **stdin** 交给容器 `sh -s`（不写容器磁盘、不依赖容器里有 git）；`scripts/show-wparse-state.sh`
  保留为人工排查入口。
- 待补：`wparse.admin_token` / `wparse.remote_reachable`（前者已在 `gops cert/mount` 里覆盖一部分）。

## 11. 已定的取舍

1. **多继承**：本轮不碰；流程写在 `operators.gxl` 的 `mod operators` 里（§3.2）。
2. **超时**：流程默认 60s（`--timeout`，钳在 5–3600），**超时记 FAIL**（诊断没跑完不能当没事）。
3. **warn 门限**：默认 `--fail-on fail`（CI 友好）；要卡 warn 时显式 `--fail-on warn`。
4. **日志量**：`--logs N`，默认 200 行，`0` = 不抓；只认 `error|fatal|panic|unauthorized|denied` 这几个词，
   命中才出 `[WARN] gops.logs.<svc>`（附最近 3 条样例）。
5. **一次性任务**：`restart: no` + `exit 0` 视为**正常完成**（不能把 compose 的 `wpars-*run-init` 判成故障）。
6. **颜色**：默认 `report::use_color()`（TTY 且无 `NO_COLOR`）；`--no-color` 强制关。
7. **交接目录**：`Bundle` 实现 `Drop`，任何早退（`?` / `exit`）都不会把临时目录留在 `/tmp`；`--keep-context` 才保留。
8. **`.env` 注入**：现场值不许覆盖 `GOPS_*` 契约变量（否则一个 `.env` 就能劫持交接路径）。
9. **流程结果校验**：`code` 重复 → 整份结果不合契约（避免脚本按 code 取项拿到两条）。
10. **gx 报错入报告**：只留前 3 行 / 300 字（完整 trace 用 `-d 1` 或单独跑流程）。

7. **日志脱敏**：`gops.logs.*` 的样本先掩 `password/passwd/secret/token/api_key/authorization/bearer` 的值，再进报告。
8. **日志噪声**：`no error` / `errors=0` / `error rate` 等否定式不计；重复行按“去时间戳/数字”归一化折叠成 `×N`；
   站点可用 `--log-ignore <子串>`（可重复）屏蔽已知噪声。
9. **漂移豁免**：`WPARSE_DRIFT_IGNORE`（空格分隔 glob）或 `sys/diagnose/drift-ignore.txt` 声明不参与全等比对的文件
   （典型：模板渲染出来的配置文件）。
10. **新增 vs 既有**：漂移清单落 `data-plane-run/diagnose/work-drift.prev`，下次比对出 `total/new/fixed`，
   标题带“新增 N”，`evidence` 带三个计数 —— 报告从“全量刷屏”变成“这次变了什么”。

## 12. 不做

- 不做自动修复（`--fix` 另立，需显式确认）。
- 不做跨机巡检（中心下发命令跑 `gops run diagnose --json` 即可，是另一层）。
- 不让流程决定退出码（退出码是 gops 的唯一职责）。

## 13. 中心收集（已实现：center 0.19.5 / gwlinkd 0.16.1 工作区）

体检结果通过**同一条命令队列**回到中心，不新增监听端口、不需要网关入站：

| 侧 | 改动 |
| --- | --- |
| center | `gateway_commands.kind`（`proxy` 缺省 / `diagnose` / `restart-gateway` / `restart-gwlinkd`，启动时幂等补列）；`POST /api/v1/admin/gateways/{id}/diagnose`（202，幂等复用 pending/leased）；`GET …/diagnose`（最近一条的 `report` 原文 + `state`）；`POST …/restart`（body `{"target":"gateway"\|"gwlinkd"}`）；`GET …/commands/{command_id}` |
| gwlinkd | 轮询响应带 `kind`；控制命令不走 wparse 白名单：`diagnose` → `gops run diagnose --json --no-color`；`restart-gateway` → `gops run stop` + `run start`；`restart-gwlinkd` → 回执后延迟触发服务管理器重启 |

**租约**：控制类命令统一 360s（`CONTROL_LEASE_SECONDS`），`proxy` 仍 30s。这是因为
`diagnose`/`restart-gateway` 最长可跑 300s —— 租约短于执行时间会让中心判过期、重派一条，
同一台机器上跑两份，而且迟到回执会因为 `state='leased'` 守卫而丢失。编译期断言锁住这个关系。

`restart-gwlinkd` 的时序：先回执（202 `{scheduled:true}`），`sleep 2s` 后执行
`systemctl restart wist-gwlinkd` / `launchctl kickstart -k system/com.dayu-sec.wist-gwlinkd`。
没装服务定义（手动 `run`）时回 409，**不杀进程**——否则重启后没人拉起。

回执语义（与 gops 退出码对齐）：

| gops 退出码 | 回执 | 中心状态 |
| --- | --- | --- |
| 0（全 OK） | body = 报告，`error=None` | `done` |
| 1（有 FAIL） | body = 报告，`error=None` | `done`（**有问题是结果，不是命令失败**） |
| 2（诊断跑不起来） / 超时 / 非 JSON | `error=…` | `failed` |

页面轮询 `GET …/diagnose`：`state` 区分“没跑”（pending/leased）、“跑了”（done，看 `report.summary`）与“执行失败”（failed/expired）。

两个时间参数必须满足：**gwlinkd 体检超时（≤300s）< center 体检租约（360s）**。
否则体检还在跑时中心就判租约过期 → 重派一条 → 同一台机器跑两份。
体检在 gwlinkd 的 relay 循环里**串行**执行（期间不拉新命令），所以默认超时不宜调大。
