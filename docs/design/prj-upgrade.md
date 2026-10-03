# `gops prj`：发布态的自动化升级事务（方案，待评审）

> 状态：**第 1、2 步已实现**（2026-10-03）。评审结论见 §3.1 / §3.2 / §3.3；§3.3 的失败处置**默认值**仍未定。
> 触发场景：`wist-gateway-stack`（`kind: docker-compose`）要做**自动化升级**。
> 「升级」的零件其实已经齐了（2.1.0 起：`prj update` 非破坏覆盖、`prj backup/restore`、`prj diagnose`；
> 见 [prj-backup-restore.md](./prj-backup-restore.md)），缺的是**一个动词把它们串成一次带回滚的事务**：
> `gops run` 只有 `download/install/uninstall/start/stop/status/diagnose`，**没有 `update` / `restart`**；
> `prj update` 也**不管运行时**——不会「取到新包前先备份、拉镜像失败别停旧栈、起不来要回滚」。
>
> 代码落点（已落地）：`src/ops_prj/upgrade.rs`（事务状态机 + `ProjectRuntime` + 测试）、
> `src/ops_prj/system.rs`（`version` 字段 + `{version}` 模板解析）、
> `app/gops/commands/prj_cmd.rs`（`PrjCmd::Upgrade` + `ComposeDispatch`）、
> `app/gops/commands/run_cmd.rs`（`compose_cmd_in`：按系统目录跑 compose）、
> `app/gops/commands/sys_cmd.rs`（`localize_in`：按系统目录跑 localize）。

## 1. 现状（代码核对）

| 命令 | 今天做什么 | 依据 |
| --- | --- | --- |
| `prj update` | 按 `addr` 重新取包 → overlay 到已导入系统；命中 `preserve` 跳过、包外未声明不删 | `src/ops_prj/update.rs:71-109`（`update_sys_content` / `overlay_one_system`）、`apply_overlay:115` |
| `prj backup` / `prj restore` | 现场态归档（restore/rebuild 两档 + 清单 + sha256 + `keep`）/ 合并回现场 | `src/ops_prj/backup.rs` |
| `prj rebuild` | 原子重建：现场态搬走→旧目录改名→铺新→搬回；失败回滚 | `src/ops_prj/import.rs:144` |
| `prj diagnose` | `ignore ⊆ preserve`、`values/` 纳管、`sys_models` 重名 | `src/ops_prj/diagnose.rs:250` |
| `sys update` / `sys localize` | 解析 vars → `merged_vars.yml`；渲染 `.env` + 模板，并跑系统自己的 `gx run localize` 流程 | `README.md`「发布态」 |
| `gops run` | `download/install/uninstall/start/stop/status/diagnose` → `docker compose pull/create/down/up -d/stop/ps/config` | `app/gops/commands/run_cmd.rs:48-119`、`compose_subcommand:221-232` |
| `deliver.lock` / `sys check` | 版本与值指纹 / 值改了没 `localize` 的漂移卡口 | `gops sys package`、`gops sys check` |

三个缺口：

1. **没有「升级」这个动词**：`run` 无 `update`/`restart`；`prj update` 只覆盖磁盘内容，**不管运行时**
   （拉新镜像 / 重启 / 健康检查 / 回滚一个都没有）。
2. **system ref 没有版本**：`OpsSystem { sys, addr }`（`src/ops_prj/system.rs:7-10`），
   `Address` 只有 Local/Git/Http 三种（`src/ops_prj/import.rs:399-405`），**版本写死在 URL/path 里**。
3. **失败不可回滚**：`prj update` 覆盖内容后，新镜像拉不下 / 起不来时**没有「回到动手前」的动作**——
   `backup`/`restore` 存在，但要人手动串，且顺序错了（先停后拉）就没救。

根因不是缺备份能力，而是 **gops 缺一个「发布态升级」的事务边界**：把「内容更新」与「运行时切换」
当成两件各自为政的事，中间没有回滚点。

## 2. 目标：一条事务，四条不变量

```
preflight → backup → apply → regenerate(localize) → pull → up → health → commit
                                                              ↘ 任一步失败 → 按 `--on-failure` 处置（§3.3）
```

四条**不变量**，任何实现都必须满足：

1. **先拉后停**：新镜像没到本机，**绝不**换容器（`apply` 改内容 ≠ 停服务；切换只发生在 `pull` 全绿之后）。
2. **回滚是状态级**：不是「只换回镜像」。有状态服务（网关开机跑 SQLite 迁移）要 `prj restore`
   把**库**一起还原——否则「换回旧镜像 + 已被前滚的库」是坏组合。
3. **健康检查在栈外**：探活脚本由调用方给（gops 不该知道 HTTP/域名/端口）。回滚判定才因此通用。
4. **幂等可续**：同版本 no-op；落**状态文件**，崩了能诊断、能重试（不每次重启从头来）。

## 3. 设计

### 3.1 `--to` 的取值：版本或地址（已实现）

`--to` 两用：

- **看起来像地址**（含 `://`，或以 `/`、`./`、`../`、`git@` 开头）→ 原样用于**所有**系统（第 1 步行为）；
- **否则当版本** → 用**该系统** ref 的 `addr` 模板（`{version}`）渲染后再下载（`--to` 覆盖声明版本）。

system ref 新增可选 `version`；`addr` 可用 `{version}` 占位：

```yaml
sys_models:
- sys:
    name: wist-gateway-stack
    kind: docker-compose
    vender: dayu-sec
  version: 0.1.23
  addr:
    url: https://github.com/dayu-sec/wist-gateway-stack/releases/download/v{version}/wist-gateway-stack-v{version}.tar.gz
```

解析集中在一处（`OpsSystem::resolved_addr` / `resolved_addr_with`）：**所有**用 addr 的地方
（`prj import` / `reimport` / `rebuild` / `update` / `upgrade`）都走它。含 `{version}` 但没写 `version`、
或给了版本但 addr 没有占位 —— 都在解析处**明确报错**，绝不拿字面量 `{version}` 去下载。

> 通道 `channel: stable|alpha|beta`（对齐 `gops self` / `galaxio-labs/get`，见 `src/self_update/`）
> 仍未做，属后续超集。
>
> 注：升级前后的版本仍从 `deliver.lock`（§3.4）读，状态文件里的 `from_version` / `to_version` 不缺。

### 3.2 新命令 `gops prj upgrade` 与阶段序

```
gops prj upgrade [<sys>] --to <version|url|path> --on-failure <rollback-all|halt> [--dry-run] [--health-cmd ...] [--json]
```

| flag | 作用 |
| --- | --- |
| `--to <version\|url\|path>` | 目标版本（按 ref 的 addr 模板 `{version}` 渲染）或完整地址；**必填**（见 §3.1） |
| `--dry-run` | 只出计划：会覆盖哪些文件、跳过哪些（preserve）、拉哪些镜像；**不动现场** |
| `--health-cmd <shell>` / `--health-timeout <sec>` | 栈外探活；**给了才做回滚判定**（没给则只切换、不判定） |
| `--on-failure <rollback-all\|halt>` | 失败处置（§3.3）。**默认未定 → 现阶段必填**，不给就拒绝 |
| `--json` | 机读结果（契约见 §3.4） |

**不提供 `--no-backup`**：备份是事务的前提，不开逃生门（以后确需再加）。
旧镜像**默认保留**（便于秒级回滚），不设开关——等有清理需求再加。

**阶段序**（每个阶段对**全部**目标系统做完再进下一阶段；步骤都复用既有实现，不新造）：

```
0. 解析目标地址；与当前 deliver.lock 比对 → 相同则 no-op 退出
1. prj diagnose（全系统）            ← 有 fail 即停；--dry-run 打印计划后退出
2. prj backup（全系统）              ← 回滚点，含 state/*.db*
3. apply：prj update（全系统）        ← 只动磁盘内容，不停服务
4. regenerate：sys update?(按需) + sys localize（全系统）
5. pull：run download（全系统）       ← 新镜像没全到齐，绝不进入下一步
6. up：run start（全系统）            ← 第一次真正切换
7. health：--health-cmd（重试至 timeout）+ run status（全系统）
8. 全绿 → 写状态（from→to / 时间 / backup_id）
   任一失败 → 按 `--on-failure` 处置（§3.3）
```

> **先拉后停**在这里是结构性的：第 5 步 `pull` 对全部系统做完，才允许第 6 步 `up` ——
> 「镜像拉不到」这个最常见的失败源，在**没停任何服务**时就被挡下。

### 3.3 失败处置：`--on-failure`（参数已定，**默认未定**）

失败时**做什么**由参数指定，不写死在实现里：

| 取值 | 语义 |
| --- | --- |
| `--on-failure rollback-all` | **整工程原子**：被升级的**所有**系统一起提交，任一失败 → 全部回滚（**含已成功的**）。 |
| `--on-failure halt` | **停在那里**：不继续，已切换的**不回滚**；如实报告「哪些成功、哪个失败、卡在哪一步」，交人处理。 |

回滚动作（仅 `rollback-all`，逆序）：

```
prj restore(全部系统的 backup_id) → run start(全部) → health(全部) → 报「已回滚（整工程）」
```

**默认值未定** → 现阶段 `--on-failure` **必填**：不给就拒绝，**不替人选默认**。等定了默认值再放开。

两种取值的代价（写清，别让人意外）：

- `rollback-all`：**备份必须全部先做**（第 2 步对每个系统成功才进第 3 步）；**两次停机**——已成功的
  系统也回滚一次，这是原子性的代价。
- `halt`：**不会两次停机**，但现场会**停在中间态**（部分新、部分旧）；需要人接手，且下次 `upgrade`
  要从状态文件看出「上次停在哪个系统/哪一步」。
- 两者共同的前提：**回滚是状态级**（`restore` 把库一起还原，§2 不变量 2）；`restore` 自身失败要
  **大声**报出备份归档路径与当前中间态，别静默。

> 单系统工程（如 `wist-gateway-stack` 当前只有一条 `sys_models`）下：`rollback-all` 退化为「失败即回滚自己」，
> `halt` 退化为「失败就停在坏版本上」——差别只在**要不要自动回退**。

### 3.4 状态文件与机读契约

沿用 `self_update` 的形状（`~/.galaxy/self_update/gops/state.json`，`SelfUpdateState`，
`src/self_update/model.rs:43-54`）：

```
~/.galaxy/upgrade/<prj>/upgrade.json
{ from_version, to_version, step, status, backup_id, updated_at, detail }
```

（记录是**工程级**的一条：事务本来就是整工程一次提交，`from_version`/`to_version`
按 `系统=版本` 逗号拼接，如 `web-stack=0.1.22,other=0.3.1`。）

与 agentd 的 `state/upgrade.json` **同形**——平台侧一套升级 UX，别为发布态再造一套字段。
`from_version` / `to_version` 从**升级前/后的 `deliver.lock`** 读（§3.1 注），不需要调用方输入。

**机读契约（已定）**：`--json` 在 stdout 输出单行同形 JSON；
`status ∈ succeeded | failed | rolled_back`（`halt` 停在中间态记 `failed`；`rollback-all` 回退成功记 `rolled_back`）；
进程退出码：成功 `0`，**失败与已回滚都非 0**（回滚是「失败的一种结局」，不能算成功）。

### 3.5 `kind` 分派

事务骨架共用，只有「拉 / 起 / 停」三处按 `sys/sys_model.yml: kind` 分派：

| 步骤 | `kind: docker-compose` | `kind: gxl` |
| --- | --- | --- |
| 拉 | `docker compose pull` | `gx run download` |
| 起 | `docker compose up -d` | `gx run start` |
| 停 | `docker compose stop` | `gx run stop` |

（`gxl` 的 module op-flow 本来就有 `update`/`restart`，见 `README.md` 的 ops-gxl 契约。）

## 4. 边界（哪些进 gops，哪些留系统侧）

| 进 gops（通用，可复用于任何 gops prj） | 留系统侧 / 编排方（gops 不该知道） |
| --- | --- |
| system ref 的 version（§3.1） | 探活脚本内容（如 `curl -f https://<域名>/…`） |
| `prj upgrade` 事务 / `--on-failure` 处置 / 幂等 / 状态文件 / `--dry-run` / `--json` | 镜像 tag 语义（`GATEWAY_TAG`/`WEB_TAG`/`WPARSE_TAG`） |
| backup→update→localize→pull→up 的编排 + 健康钩子调用 | 「先栈后 agent」的顺序与授权 |
| 复用 `prj backup/restore`、`sys check` 漂移卡口 | 触发与调度（timer 轮询 / 上级 push）、执行器进程 |

## 5. 现场证据（`wist-gateway-stack`，2026-10-03）

- **手工升级序列**（`README.md`「发布态」）：`prj update` → `sys update`/`sys localize` →
  `run download` → `run start`。今天全靠人肉执行 + 记得先备份；顺序错（先停后拉）就没救。
- **网关在栈内**：换 `gateway` 容器时**控制面会断** → 执行器必须在 **host 侧**、回滚**不能依赖网关**；
  上级要容忍这段空窗。
- **有状态、且会前滚迁移**：`configs/gateway/state/wist-gateway-store.db` 会在开机跑 SQLite 迁移
  （例：`0023_agent_machine_profile.sql`）→ **回滚必须含 DB**。`sys-prj.yml: backup.restore` 已声明它
  （见 [prj-backup-restore.md](./prj-backup-restore.md) §4.1），事务里直接用。
- **版本依赖链**：`GATEWAY_TAG`/`WEB_TAG` 来自网关/前端制品；`WPARSE_TAG` 与 `data-plane/` **强耦合**
  （引擎版本一变，`conf/connectors/models/topology` 可能跟着变）→ 必须同版本一起换；stack 包本身带
  `data-plane/`，`prj update` 会覆盖它。
- **部署顺序**：**先网关、后 agent**（`AgentStatusReport` 是 `deny_unknown_fields`，新 agent 打旧网关 400）。
  编排器要把 stack 升级串在 agent 升级**之前**，并等网关健康。

## 6. 落地顺序

1. ✅ **`prj upgrade`（先只走 docker-compose 路）**：阶段序（§3.2）+ `--on-failure`（§3.3，**暂必填**）+
   `--dry-run` + `--json`；**无 `--no-backup`**。复用 `diagnose`/`backup`/`update`/`restore` +
   `run download/start/status`。
2. ✅ **system ref `version` + `{version}` 模板**（§3.1），`--to <ver>` 生效 → 再补通道解析。
3. **`kind: gxl` 分派**（§3.5）。
4. **host 侧执行器**（timer 轮询 / 上级派 work）——**不在 gops 内**，另行设计（见 §8）。

## 7. 未定问题

- **`--on-failure` 的默认值**（未决）：`rollback-all` 还是 `halt`？取决于「中间态可接受度」与「两次停机」
  哪个更痛；**现阶段必填、不猜**（§3.3）。
- **`deliver.lock` 是否记「上一版包内文件清单」**（未决）：决定 `prj upgrade` 能否清掉「上一版包内、
  这一版包外」的旧版本残渣。**本方案不依赖它**：先只覆盖、不删残渣；与
  [prj-backup-restore.md](./prj-backup-restore.md) §8 是同一个未定问题。
- **`version` 字段的形态**（排期项）：`{version}` url 模板，还是通道清单（对齐 `galaxio-labs/get`）。
- **回滚失败的兜底**：`prj restore` 自身失败时的退出码与话术（要指出备份归档路径 + 当前中间态）。

## 8. 附：明确不在本方案内

- **触发与调度**（timer 轮询通道 / 上级 push）、授权与审计 → 属**系统侧的 host 执行器**。
- **镜像仓库可达性/鉴权**（内网镜像、`IMAGE_SPC` 覆盖）→ 属部署配置（`values/value.yml`）。
- **探活脚本内容** → 属系统侧。
- **反代/证书重签等 localize 副作用**（如 `align-host-perms.sh` 可能要 sudo）→ 仍由 `sys localize` 的
  localize 流程负责；`prj upgrade` 只调它，不重写它。
