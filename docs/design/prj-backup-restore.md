# `gops prj`：现场态、备份与还原（方案，待评审）

> 状态：**已实现**（`galaxy-ops` 2.0.x-alpha）。记录于 2026-10-01。
> 触发场景：在真实部署上，`gops prj reimport` 会 `rm -rf` 掉系统目录里**不属于交付包**的一切
> （身份私钥、store 库、运行态），而 `gops prj update` 又只管项目 conf —— 升级/备份/还原这三件
> 事在 gops 里没有对应能力，只能每个系统自己写脚本（如 `wist-gateway-stack/scripts/backup-gateway.sh`）。
>
> 代码落点：`src/system/conf.rs`（`preserve` / `backup`）、`src/ops_prj/conf.rs`（项目侧 `backup`）、
> `src/ops_prj/update.rs`（非破坏性覆盖）、`src/ops_prj/backup.rs`（收/放）、
> `src/ops_prj/diagnose.rs`（检查）、`src/ops_prj/import.rs`（`reimport` 只补缺失 + `rebuild` 原子重建）。

## 1. 现状（代码核对）

| 命令 | 今天做什么 | 依据 |
| --- | --- | --- |
| `prj import` | 下载/解包 → 铺进 `<项目根>/<系统名>/`；重建 `values/` 符号链接 | `src/ops_prj/import.rs:15`、`src/ops_prj/install.rs:99-129` |
| `prj reimport` | 对 `ops-prj.yml` 里每个系统：`remove_dir_all(<项目根>/<系统名>)` → 重新导入 | `src/ops_prj/import.rs:62-81` |
| `prj update` | 只更新项目 conf（`OpsProjectConf::update_local` + save），**不进系统目录** | `src/ops_prj/project.rs:89-98` |
| `prj doctor` | 只检查 `values/` 是否纳入版本控制（建议改名 `prj diagnose`，见 §5） | `app/gops/commands/prj_cmd.rs` |

三个缺口：

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
  → `reimport` 会对同一目录 **wipe+install 两遍**（第一遍会把部署降到旧版本）。

## 3. 概念：现场态（site state）

**定义**：系统目录里**不属于交付包**、且**升级不得覆盖**的东西。它是备份的选取依据，也是
`reimport`/非破坏更新的排除依据。

**不变式**：`sys-prj.yml: ignore ⊆ preserve`。
凡是"不进交付包"的，按定义就不是包内容，因此必须升级不动 —— 不一致由 `prj diagnose`（现名 `doctor`）报出。
（注意两者**不相等**：`.env`、`values/`、`deliver.lock` 都不在 `ignore` 里，但都属于现场态。）

`values/` 是其中一个特例（在系统目录外层、以符号链接暴露），保留现有机制不变。

## 4. 声明

### 4.1 系统侧 `sys-prj.yml`（布局与分级，所有部署一致）

```yaml
ignore: [...]        # 既有：不进交付包（`gops sys package` 用）
preserve: [...]      # 新增：升级不覆盖（ignore ⊆ preserve）
backup:              # 新增：两档 —— 只表达“丢失的代价”，**不触发任何删除**
  restore: [...]     # 丢了要重装 / 换身份：默认**收**，还原时放
  rebuild: [...]     # 可重建：默认**不收**，`--include-rebuild` 才收（价值只是省一次重新投放/生成）
# 不在上面两档的 = 不收、不碰 —— 不需要单独声明一档
# （“故意不收”与“忘了写”的区分交给 `prj diagnose`，不是靠多一个档名）
```

例（`wist-gateway-stack`）：

```yaml
preserve:
  - .env
  - configs
  - packages
  - data-plane-run
backup:
  restore:
    - configs/gateway/state/*.pem
    - configs/gateway/state/wist-gateway-store.db*
    - configs/gateway/wist-gateway.toml
    - configs/gateway/wist-gateway.value.json
    - configs/web/tls
  rebuild:
    - packages
# configs/gateway/state/logs/（约 11 GB/天） 与 data-plane-run/ 不在任何一档：不收、不碰。
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
      level: restore                        # rebuild | restore
      include: [...]                        # 现场增量（本项目额外要收的）
      exclude: [...]                        # 现场减量（本项目不收系统声明的某项）
```

**分工理由**：路径与分级放**系统侧**（系统作者才知道自己的布局，且所有部署一致）；落点 / 收哪些系统 /
哪一级 / 现场增删放**项目侧**（运维决策）。这样每个项目不必把系统的路径抄一遍 —— 否则又是"同一件事写两处"。

## 5. 命令族（对现有命令做增量，不推翻）

| 命令 | 现状 | 目标 |
| --- | --- | --- |
| `prj update` | 只更新 conf | **非破坏性更新系统内容**：只覆盖**包内含**的路径，`preserve` 一律不碰 ← 缺的就是这条 |
| `prj backup` | — | 按 `backup` 分级收；输出清单 + sha256；**点名含私钥的条目**并提示离机保管 |
| `prj restore` | — | 把备份里的**现场态**合并回现场（目录在就覆盖；目录没了就按归档重建这部分）；先解到临时再合并 |
| `prj reimport` | `rm -rf` + 重装 | **只补缺失**：目录已存在就拒绝（指向 `prj update` / `prj rebuild`）|
| `prj rebuild [<系统名>]` | — | **原子重建**：preserve 先搬到临时 → 旧目录改名 → 重建 → 搬回；失败可回滚（消除“删了却没装上”的中间态） |
| `prj diagnose` | 现名 `prj doctor`，只查 `values/` | 加：`ignore ⊆ preserve`、备份是否覆盖了系统声明的 `restore` 级、“不在任何备份档里”的路径（提示而非报错）、`sys_models` 重名 |

**改名 `doctor` → `diagnose`**：与 agentd 侧的 `diagnose` 同一口径（用户侧只记一个词）。
这是对外命令改名，建议按本仓既有做法把 `doctor` 留为**隐藏别名**（同 `sys package --no-git` → `--full`）；
若决定不留，按 breaking change 记入 `UPGRADE.md` / CHANGELOG 的变更表。

**可复用的既有实现**：`gops sys package` 已经实现了"git 入库文件 + `ignore:`"的选取逻辑
（`src/system/pack.rs:18-47`，含 glob 归一化等细节）。**非破坏性更新应当直接以它为白名单** ——
"交付包里有什么，就覆盖什么"，不要在新代码或 shell 脚本里重写一遍 git/glob 逻辑。

### 5.1 `prj rebuild` 的确切行为

一句话：**重建系统目录，但把现场态整体搬过去、搬回来；失败可回滚。** 它不是裸 `rm -rf`。

```
gops prj rebuild [<系统名>]

0. 先取包：下载/解包到临时区。**取不到就什么都不动**（今天的行为是先删再发现取不到）。
1. 预演：列出三类东西
     a) 将被删除：<项目根>/<系统名>/（整目录）
     b) 将被原样搬回：preserve: 里的条目
     c) 包外且**未声明**的条目 —— 单列并标注“这些也会随重建消失”
   （这一步也是它不做成默认的理由：危险动作必须有个自己的动词。）
2. 原子重建（关键是用 rename，不是 rm）：
     a) preserve: 的路径 → <项目根>/.rebuild-tmp/<系统名>/
     b) 旧系统目录 rename 成 .rebuild-tmp/<系统名>.old      ← 不是删
     c) 新包内容铺到 <项目根>/<系统名>/
     d) preserve 搬回原位
     e) 重建 `values -> ../values/<系统名>` 符号链接
3. 成功 → 才删 .old 与 tmp；失败 → 把 .old rename 回原位（现场回到动手前的样子）
4. 提示下一步：`gops sys localize`
```

> **保留项的粒度 = 最浅匹配项**：`path_matches` 是**按祖先**匹配的（目录命中 ⇒ 子树全部命中），
> 所以命中一个目录就**整棵 `rename`** 搬走、**不进入它内部**；细到文件的模式（如 `configs/gateway/state/*.pem`）
> 仍按文件收集。这样 `preserve: configs` 不会钻进容器以**别的 uid** 写的运行期子目录
> （如 `configs/gateway/state/knowledge/`）—— 逐文件 `rename` 会在那里因**父目录不可写**而 `EACCES`。

三条路径的边界：

| | 包内文件 | 包外·已声明 `preserve` | 包外·**未声明存量** | 失败时 |
| --- | --- | --- | --- | --- |
| `prj update`（默认路） | 覆盖 | **不动** | **不动** | 取包/解包失败 ⇒ 目录完全未动；覆盖阶段逐文件，中途失败可能已写入部分新文件（不删任何东西） |
| `prj rebuild` | 换新 | 搬走 → 搬回 | **会消失**（预演里单列） | 回滚到动手前（回滚失败会告警，并指出备份留在哪） |
| `prj reimport`（默认） | — | — | — | **目录已存在就不执行**（拒绝并指向 `prj update` / `prj rebuild`）；只建**缺失的**系统目录 |

> `prj update` 不追求“全或全无”：真正的失败源是取包/解包（已在动手前完成），
> 之后的逐文件覆盖只在磁盘满/权限这类情况下会中断。要做到严格原子得先整份拷到临时再换，
> 成本远高于收益（升级本身不丢数据，重跑一次即可对齐）。

两处取舍：

1. `prj rebuild` 是**唯一**会删“未声明存量”的操作 —— 所以它是个**独立的动词**（而不是挂在 `reimport` 上的开关），
   且有预演清单；默认路径（`update` / `reimport`）仍遵守 §6「没声明就保守」。预演里未声明项要**单独列**，不要混在“将删除”里。
2. 没声明 `preserve:` 的系统，预演必须给强警告：*“未声明现场态 → 重建会丢掉全部包外内容
   （很可能包含身份材料）”*。否则 `prj rebuild` 就等于换身份。

## 6. 设计约束（会坚持的三条）

1. **没声明就保守**：未声明的路径**绝不删、升级不覆盖**（宁可留垃圾）；备份时列为"未纳入"让人看见。
2. **秘密显性**：备份中含私钥/凭据的条目必须点名 + 提示离机保管；且**不随交付包走**（与 `ignore` 配合）。
3. **原子**：`restore` 先解到临时再替换；`reimport` 不再出现中间态。

## 7. 落地顺序（均已完成）

1. ✅ `sys-prj.yml: preserve:` + **`prj update` 非破坏化**（`update.rs::apply_overlay`）
   —— 立刻解决"升级只能靠人搬文件"。
2. ✅ `prj backup` / `prj restore`（分级 + sha256 清单 + 私钥点名 + `keep` 清理 + 先解临时再合并）。
3. ✅ `prj reimport` 收紧：**只补缺失**，目录已存在就拒绝（指向 `prj update` / `prj rebuild`）；
   `prj rebuild` 才原子重建（现场态搬走搬回、旧目录改名保留、失败回滚）。
4. ✅ `prj doctor` → `prj diagnose`（`doctor` 留为隐藏别名），新增 `ignore ⊆ preserve`、
   `sys_models` 重名、`backup.restore` 缺失、preserve 未被备份覆盖等检查。

## 8. 未定问题

- 备份落点是否支持远端（s3 / ssh）与加密？现阶段建议只做本地目录 + 明确的“含私钥”提示。
- `keep` 的清理策略与命名（时间戳？内容摘要？）如何定。
- **运行期保留（retention/GC）不属于本方案**：`discard` 已取消，那 11 GB/天 的采集日志由系统侧自己轮转，
  或另立一条带阈值的保留声明（见 §9.1）。
- `prj doctor` → `prj diagnose` 的别名策略：保留隐藏别名，还是直接断（进 breaking 表）。
- **`deliver.lock` 要不要记“包内文件清单”**：`prj update` 想顺带删掉“上一版包内、这一版包外”的文件
  （不删会留旧版本残渣），但需要知道上一版的文件清单；现在 `deliver.lock` 只记 `merged_vars.yml` / `values/` 的摘要。
- `values/` 是否显式写进 `preserve`（现为隐式特例）。

## 9. 附：由本方案暴露、但独立处理的两个现场问题

1. **采集日志落在配置目录且无轮转**：`configs/gateway/state/logs/agent-logs.ndjson` 约 11 GB/天。
   它既撑爆备份，也让"configs = 身份 + 少量配置"的直觉失真（运行态放进了配置目录）。
   建议：挪到运行态目录，或先给一条保留/轮转策略。
2. **`ops-prj.yml` 的 `sys_models` 重名**：会导致 `reimport` 对同一目录重复 wipe+install，
   `backup` 重复收。建议清理，并让 `prj diagnose` 报重名。
