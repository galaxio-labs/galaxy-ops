# `gops prj`：现场态、备份与还原（方案，待评审）

> 状态：**已实现**（`galaxy-ops` 2.0.x-alpha）。记录于 2026-10-01。
> 触发场景：在真实部署上，当时的 `gops prj reimport` 会 `rm -rf` 掉系统目录里**不属于交付包**的一切
> （身份私钥、store 库、运行态），而当时的 `gops prj sync` 又只管项目 conf —— 升级/备份/还原这三件
> 事在 gops 里没有对应能力，只能每个系统自己写脚本（如 `wist-gateway-stack/scripts/backup-gateway.sh`）。
>
> 代码落点：`src/system/conf.rs`（`protect` / `contract` / `backup`）、`src/ops_prj/conf.rs`（项目侧 `backup`）、
> `src/ops_prj/update.rs`（非破坏性覆盖 + 补齐缺失系统）、`src/ops_prj/backup.rs`（收/放）、
> `src/ops_prj/diagnose.rs`（检查）、`src/ops_prj/import.rs`（导入 / 补齐缺失系统）。

## 1. 现状

| 命令 | 今天做什么 | 依据 |
| --- | --- | --- |
| `prj import` | 下载/解包 → 铺进 `<项目根>/<系统名>/`；重建 `values/` 符号链接 | `src/ops_prj/import.rs`、`src/ops_prj/install.rs` |
| `prj sync` | 非破坏覆盖包内内容 + **补齐缺失系统**（评审时仅更新项目 conf） | `src/ops_prj/update.rs`、`src/ops_prj/project.rs` |

> 注：本方案评审时还存在 `prj reimport`（`rm -rf` + 重装，后收紧为“只补缺失”），现已**移除**、
> 职责并入 `prj sync`（见 §5 / CHANGELOG）。

方案针对的三个缺口（评审时基线）：

1. **没有"升级已交付系统"的非破坏性命令**：`update` 不管内容，`reimport` 是全量重建。
2. **没有备份/还原能力**：`ops-prj.yml` 只有 `name / work_envs / sys_models`（`src/ops_prj/conf.rs:13-18`），
   没有任何"哪些是现场生成物、该收到哪"的声明。
3. **破坏性不可见、且非原子**：`reimport` 的 `remove_dir_all` 无确认、无 dry-run；先删后装，
   中途失败（网络断、包坏）就**目录没了且不可回滚**。

根因不是缺命令，而是 **gops 只认得 `values/` 一个"客户态"概念**，系统目录里其余一切都被当作包产物；
而真实系统会把**身份材料与运行态**放在树内。

## 2. 现场证据（真实系统实测，2026-10-01）

以 `wist-gateway-stack` 的一个实际部署为例（`gateway-alone/`）：

```
configs/                                     915 M
  └ gateway/state/logs/agent-logs.ndjson     908 M   ← 数据面转发的采集日志落盘
  └ gateway/state/wist-gateway-store.db{,-wal} 6.6 M ← agent 凭据 / 工作 / 知识库指针
  └ gateway/state/*.pem                        ~24 K  ← 5 份身份/签名材料
  └ gateway/knowledge/                         48 K   ← 初始知识包副本
data-plane-run/                              1.0 G  ← 引擎运行态（输出/pid/锁/sqlite）
```

- `agent-logs.ndjson`：236 万行、覆盖约 1 小时 52 分 → **≈ 480 MB/小时 ≈ 11 GB/天，且没有轮转**
  （代码注释自述"等保留策略定了再考虑入库"）。
- 真正**丢了要重装 / 换身份**的只有 **≈ 6.7 M**；明确可丢弃的是 **≈ 2 G**。
  → **差三个数量级**：所以"收什么"必须**比目录粒度更细**，否则会得到一份 900 M+、99% 是垃圾的备份。
- 另一处现场问题：同一份 `ops-prj.yml` 出现**两条同名 `sys_models`**（一条指向旧发布包、一条指向本地包）
  → 当时 `reimport` 会对同一目录 **wipe+install 两遍**（第一遍会把部署降到旧版本；该命令现已移除）。

## 3. 概念：现场态（site state）

**定义**：系统目录里**不属于交付包**的文件。`protect` / `backup` / `contract` 各自声明其中一部分 ——
三个声明**互相独立**，各由各自的命令消费（`sys resolve`/`prj sync`、`prj backup`、`prj contract export`）。

`values/` 是其中一个特例（在系统目录外层、以符号链接暴露），保留现有机制不变。

## 4. 声明

### 4.1 系统侧 `sys-prj.yml`（布局与分级，所有部署一致）

```yaml
pkg_include: [...]   # 打包纳入（基准集之外补入；默认模式；pkg_exclude 优先）—— 只服务 `sys package`
pkg_exclude: [...]   # 打包排除（旧名 `ignore`）—— 只服务 `sys package`
protect: [...]       # 升级不覆盖、也不删 —— 只服务 `sys resolve` / `prj sync` / `prj upgrade`
contract: [...]      # 对外契约（不可重建的公开现场事实）—— 只服务 `prj contract export` / `import`
backup: [...]        # 备份选取（扁平列表）—— 只服务 `prj backup` / `restore`
# 五个声明**互相独立**，各由各自的命令消费；不做跨声明的推导 / 覆盖 / 交叠判断。
# 不在 backup 里的 = 不收、不碰（不触发任何删除）。
```

例（`wist-gateway-stack`）：

```yaml
protect:
  - .env
  - configs
  - packages
  - data-plane-run
backup:
  - configs/gateway/state/*.pem
  - configs/gateway/state/wist-gateway-store.db*
  - configs/gateway/wist-gateway.toml
  - configs/gateway/wist-gateway.value.json
  - configs/web/tls
# configs/gateway/state/logs/（约 11 GB/天） 与 data-plane-run/ 不在 backup 里：不收、不碰。
```

> 原先草案里的 `discard` 一档**已取消**。原因：它的字面像是“可以/应该删”，而本流程**不删任何东西**；
> 真正要“删”的是**运行期保留（retention/GC）**那件事（有阈值、有周期），和“备份选取表”不是一回事。
> 把两者混成一档会让人以为“写进 `discard` 就会自动清理”。运行期清理见 §9.1。

### 4.2 项目侧 `ops-prj.yml`（这套部署的运维决策）

```yaml
backup:
  target: /Volumes/backup/gateway-alone     # 落点（目录；可写 ${VAR}）
  keep: 10                                  # 保留最近 N 份
  systems:
    - name: wist-gateway-stack
      include: [...]                        # 现场增量（本项目额外要收的）
      exclude: [...]                        # 现场减量（本项目不收系统声明的某项）
```

**分工理由**：路径放**系统侧**（系统作者才知道自己的布局，且所有部署一致）；落点 / 收哪些系统 /
现场增删放**项目侧**（运维决策）。这样每个项目不必把系统的路径抄一遍 —— 否则又是"同一件事写两处"。

## 5. 命令族（对现有命令做增量，不推翻）

| 命令 | 现状（评审时） | 目标 |
| --- | --- | --- |
| `prj sync` | 只更新 conf | **非破坏更新系统内容 + 补齐缺失系统**：已在的只覆盖**包内含**路径、`protect` 一律不碰；缺失的按包导入 ← 缺的就是这条 |
| `prj backup` | — | 按 `backup`（**不含** `contract`）收；输出清单 + sha256；**点名含私钥的条目**并提示离机保管 |
| `prj restore` | — | 把备份里的**现场态**合并回现场（目录在就覆盖；目录没了就按归档重建这部分）；先解到临时再合并 |
| `prj diagnose` | 现名 `prj doctor`，只查 `values/` | 各声明**自身**的检查（`values/` 纳管、`contract` 齐备、`sys_models` 重名）；**不做**跨声明关系推理 |

**改名 `doctor` → `diagnose`**：与 agentd 侧的 `diagnose` 同一口径（用户侧只记一个词）。
这是对外命令改名，建议按本仓既有做法把 `doctor` 留为**隐藏别名**（同 `sys package --no-git` → `--full`）；
若决定不留，按 breaking change 记入 `UPGRADE.md` / CHANGELOG 的变更表。

**可复用的既有实现**：`gops sys package` 已经实现了"git 入库文件 + `pkg_exclude:`"的选取逻辑
（`src/system/pack.rs:18-47`，含 glob 归一化等细节）。**非破坏性更新应当直接以它为白名单** ——
"交付包里有什么，就覆盖什么"，不要在新代码或 shell 脚本里重写一遍 git/glob 逻辑。

> 注：方案里还有 `prj rebuild`（原子重建，能清掉**包外未声明内容**）与 `prj reimport`（只补缺失）——
> 二者均已在后续版本**移除**（见 CHANGELOG）。现在 `prj sync` **补齐缺失系统 + 非破坏覆盖**，
> 且**不删**包外未声明内容。

三条路径的边界：

| | 包内文件 | 包外·已声明 `protect` | 包外·**未声明存量** | 失败时 |
| --- | --- | --- | --- | --- |
| `prj sync`（默认路） | 覆盖 | **不动** | **不动** | 取包/解包失败 ⇒ 目录完全未动；覆盖阶段逐文件，中途失败可能已写入部分新文件（不删任何东西） |
| `prj sync`（系统目录缺失） | 整包**导入** | — | — | 取包/解包失败 ⇒ 目录完全未建；不留可回滚的中间态 |

> `prj sync` 不追求“全或全无”：真正的失败源是取包/解包（已在动手前完成），
> 之后的逐文件覆盖只在磁盘满/权限这类情况下会中断。要做到严格原子得先整份拷到临时再换，
> 成本远高于收益（升级本身不丢数据，重跑一次即可对齐）。

## 6. 设计约束（会坚持的三条）

1. **没声明就保守**：未声明的路径**绝不删、升级不覆盖**（宁可留垃圾）。
2. **秘密显性**：备份中含私钥/凭据的条目必须点名 + 提示离机保管；且**不随交付包走**（与 `pkg_exclude` 配合）。
3. **原子**：`restore` 先解到临时再替换；导入/覆盖都不留可回滚的中间态。

## 7. 落地顺序（均已完成）

1. ✅ `sys-prj.yml: protect:` + **`prj sync` 非破坏化**（`update.rs::apply_overlay`）
   —— 立刻解决"升级只能靠人搬文件"。
2. ✅ `prj backup` / `prj restore`（sha256 清单 + 私钥点名 + `keep` 清理 + 先解临时再合并）。
3. ✅ “补齐缺失系统”归入 `prj sync`（`reimport` 命令后续**移除**）。
4. ✅ `prj doctor` → `prj diagnose`（`doctor` 留为隐藏别名）：各声明**自身**的检查（`values/` 纳管、
   `contract` 齐备、`sys_models` 重名）。

## 8. 未定问题

- 备份落点是否支持远端（s3 / ssh）与加密？现阶段建议只做本地目录 + 明确的“含私钥”提示。
- `keep` 的清理策略与命名（时间戳？内容摘要？）如何定。
- **运行期保留（retention/GC）不属于本方案**：`discard` 已取消，那 11 GB/天 的采集日志由系统侧自己轮转，
  或另立一条带阈值的保留声明（见 §9.1）。
- `prj doctor` → `prj diagnose` 的别名策略：保留隐藏别名，还是直接断（进 breaking 表）。
- **`deliver.lock` 要不要记“包内文件清单”**：`prj sync` 想顺带删掉“上一版包内、这一版包外”的文件
  （不删会留旧版本残渣），但需要知道上一版的文件清单；现在 `deliver.lock` 只记 `merged_vars.yml` / `values/` 的摘要。
- `values/` 是否显式写进 `protect`（现为隐式特例）。

## 9. 附：由本方案暴露、但独立处理的两个现场问题

1. **采集日志落在配置目录且无轮转**：`configs/gateway/state/logs/agent-logs.ndjson` 约 11 GB/天。
   它既撑爆备份，也让"configs = 身份 + 少量配置"的直觉失真（运行态放进了配置目录）。
   建议：挪到运行态目录，或先给一条保留/轮转策略。
2. **`ops-prj.yml` 的 `sys_models` 重名**：会导致同一系统目录被重复导入 / 收两遍。
   建议清理，并让 `prj diagnose` 报重名。
