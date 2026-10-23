# 变更日志

所有重要的项目变更都将记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.0.0/),
并且本项目遵循[语义化版本](https://semver.org/lang/zh-CN/)。

## [2.10.5-alpha] - 2026-10-23

### 修复

- **`sys perms --apply` 的 `[applied]` 行不再印成「0660 → 0660」**：修复后的复核会把
  `current` 刷成新值，before 被覆盖，行看上去像什么都没做。现在 `Applied` 项**保留修复前快照**
  （文本读作 `0640 1000:999 → 0660 1000:999 [applied] 模式 0640 不是 0660`），
  只有失败项才刷新成「修复后仍不一致」的现状。

### 工程

- **分组统计不再打印为 0 的段**：`gops run diagnose` 输出 `gops: 2 ok   diagnose: 1 warn`，
  不再是 `1 ok / 0 warn / 0 fail` —— `0 fail` 在日志查看器 / CI 的关键词高亮里会被涂红，
  读起来像“有失败”。（`wist-stackcheck` 的汇总行同口径，见 `wist-stackcheck-core` 0.1.4。）

## [2.10.4-alpha] - 2026-10-23

### 修复

- **`sys perms` 的第二类宿主预置也不再逐项刷屏**：挂载源里的文件若属主既不是部署账号、
  也不是容器身份（Docker 为嵌套挂载点建的 `root` 空目录/空文件、root 跑过的恢复/脚本），
  非 root 既不能 `chown` 也不能 `chmod`。现在按**挂载源**汇总成一条 `[FAIL]`，给出可直接执行的
  `sudo chown -R <部署账号>:<容器 gid> <source>`，文本输出里该挂载标 `[BLOCKED]`。
  与上一版的缺组成员共用同一套「一次说清」机制（`MountReport.blocked`）；`--apply` **照常修复
  其它未被拦的挂载**，已对齐的现场不受影响。
  （现场形态：`d data-plane-run/work/.run 2755 0:999 → 2770 1000:999 [failed]` ×107。）

### 兼容性

- `--json` 的 `mounts[].blocked_by_group`（2.10.3 刚引入）改为统一的 `mounts[].blocked`：
  `null` 或 `{"kind":"group","gid":N}` / `{"kind":"owner","uid":U,"gid":N}`。
  其余字段（`items[]` / `summary` / `errors`）不变。

## [2.10.3-alpha] - 2026-10-23

### 修复

- **`sys perms` 不再把「宿主预置缺失」打成 N 行同一个错**：部署账号不在容器 gid 的组里时，
  `chgrp` 必然 `EPERM`（内核按 gid 数字判定，不看 `/etc/group` 里的名字）。现在动手前先取一次
  真实生效的组，按 gid 汇总成**一条** `[FAIL]`（含 `groupadd` / `usermod -aG` 命令与重新登录提醒），
  受影响挂载在文本里标 `[BLOCKED]`、不展开逐项明细；`--json` 新增 `mounts[].blocked_by_group`。
  root（含 sudo）不受影响；**本来就已对齐**的现场不会因为缺组成员而报错（只在确实有项要改时才拦）。
- `gops run start` 的权限 preflight 与 `prj diagnose` 的措辞随之对齐：从「配置错误 / 配置冲突」改为
  「无法靠改权限解决（配置或宿主预置）」——两者修法不同（改清单 vs 管理员加组），提示不再把人引向
  无效的 `--apply`。

## [2.10.2-alpha] - 2026-10-23

### 修复

- **非交互环境触发的升级不再因“找不到 gops”失败**：`gx run <flow>` 的子进程 `PATH`
  会前置 gops / `gx` 所在目录（原 `PATH` 保留），流程里按裸名字调 `gops ...`
  （如 `gops sys perms --apply`）在 systemd / gwlinkd / cron 下也能找到正确版本。
- **`sys_model.yml` 缺 `vender` 时不再误判系统类型**：该字段改为可缺省，
  docker-compose 系统不会被静默当成 gxl（避免 `start` / `stop` 分派错、
  `diagnose` 报“无 diagnose 流程”）。

## [2.10.1-alpha] - 2026-10-22

### 修复

- **`sys perms` 的密钥路径校验**：`secrets.file` 现在与 `gops sec` 使用同一套规则
  （相对系统根、无 `.`/`..`、无符号链接）。绝对路径或越界路径会在派生之前直接报错，
  避免误把 chmod 作用到系统根之外。
- **`sys-prj.yml: perms` 拼写错误不再被静默忽略**：未知字段（如把 `services` 写成
  `service`）会直接报错，避免对齐被悄悄关掉。
- **`gops sec fix` 的说明与实际行为对齐**：`0640` 策略会把只读的 `0400` 补成 `0440`
  （只补属组读位，始终不加写位）。

## [2.10.0-alpha] - 2026-10-21

### 新增

- **`gops run diagnose`：运行时体检**（原来是 `docker compose config`，现在拆开）：
  采集容器状态/健康/重启/OOM、挂载源、端口冲突、私钥权限、日志错误线索，再调用系统自己的
  `diagnose` 流程（`sys/workflows/operators.gxl`）拿业务结论；人读 `[OK]/[WARN]/[FAIL]`（TTY 着色）
  或 `--json`；退出码 `0`（无 FAIL）/ `1`（有 FAIL）/ `2`（诊断自身跑不起来）。
- **`gops run config`**：打印渲染后的 compose 模型（原 `run diagnose` 的行为，密钥掩码）。
- **体检契约**（`GOPS_DIAGNOSE_CONTEXT` / `GOPS_DIAGNOSE_RESULT`）：上下文与结果两份 JSON，
  系统流程按 `_gal` 约定实现即可接入（见 `docs/design/run-diagnose.md`）。

### 改进

- 体检日志检查**脱敏**（`password/token/bearer` 等值换成 `***`）、**去噪**（否定式/统计行不计，
  `--log-ignore <子串>` 可重复屏蔽）、**折叠**（同一条错误 `×N`）。
- `gx run --exists` 的三态探测（存在 / 没这个流程 / gx 不可用）收敛到一处，
  `sys localize` 与 `run diagnose` 共用——避免「gx 不可用」被静默当成「没有流程」。
- `gops run diagnose` 对一次性任务（`restart: no` + exit 0）不再误报故障。

### 注意

- `gops run diagnose` 现在**不再**打印 `docker compose config`；脚本如需旧行为请改用 `gops run config`。

## [2.9.6-alpha] - 2026-10-21

### 新增

- **`gops sys diff --all`（`-a`）**：默认仍只列变更项（`changed` / `added` / `removed`），
  `--all` 列出**全部键**（含 `same`）。
  - 标题随模式变化：`[sys] 值变更 (14 项)` / `[sys] 值总览 (39 项，其中 14 项变更)`，
    不让人把未变更行误读成“都改了”；
  - **各模块小节同样生效**（否则“全部”是假的）；`--json` 也受 `--all` 影响
    （`state` 字段本就完整，只差是否过滤）；
  - `localize` 结尾的变更汇总**不变**（仍只列变更），两处共享的 `value_rows()` 加了一个
    `include_same` 参数而非另开一条筛选路径。
- **`gops mod diff --all`（`-a`）同步支持**：与 `sys diff` 行为一致（默认只列变更）。
  两边共用 `report::value_section_title()` / `report::groups_all_empty()`，文案与
  “是否打 `[OK]`”的判定不会分叉。

### 测试

- 既有 sys/mod diff 用例补 `--all` 断言（`same` 行参与、总数/变更数、标题文案）；
  新增标题文案与空集判定单测（2 例）、`sys`/`mod` 的 `-a` 解析各 1 例；
  全量 `588+102+6+1+4` passed。

## [2.9.5-alpha] - 2026-10-21

### 新增

- **`gops sec fix`**：把密钥文件权限收敛到 `sys-prj.yml: secrets.mode`（缺省 `0600`，
  与容器共享时 `0640`）—— `gops run start` / `sec list` 报权限不符时不再只能手敲 `chmod`。
  - 路径解析与 `gops sec list` **完全一致**（`secrets.file` → 回退 `.galaxy/sec_value.yml`），
    报错指的文件和修的文件保证是同一个；
  - **只收缩、不放宽**：`0644 → 0600`；本来就只读的（`0400` / `0440`）保持只读；
    `0640` 策略下也不会给 other 位；`0000`（无任何读位）**拒绝修改**，不替人作主加权限；
  - **只改 mode，不改属主**；`0640` 策略下额外打印当前属组 gid 供人工核对（对应容器 gid）；
    符号链接 / 硬链接 / 属主与 euid 不符直接报错并给出处置建议；
  - 修完顺带做一次内容校验（非字符串值 / 非法键名等不属于 fix 范围），失败只提示不报错；
  - 幂等：已符合策略不写盘；`--dry-run` 只打印 `0644 → 0600` 计划；
  - 非 Unix 平台跳过（与加载期校验的 `cfg` 桩一致）。
- 加载期权限错误多一行 `修复：gops sec fix（先 \`--dry-run\` 看计划）`，让报错自带可执行出口。
- `gops --help` 里 `sec` 的描述改为「密钥查看 / 校验修复 / 带密钥执行」。

### 测试

- `sec` 模块新增 9 例（含非 Unix 分支）：声明/回退两种策略、只读保持只读、`0640` 策略、
  dry-run 不写盘、幂等、符号链接与硬链接拒绝、`0000` 拒绝；CLI 参数解析 1 例。

## [2.9.4-alpha] - 2026-10-20

### 新增

- **升级事务的执行记录（日志 P0）**：每次 `prj upgrade` 在
  `~/.galaxy/upgrade/<prj>/runs/<时间戳>/` 下落一份可定位的现场：
  - `run.json`：目标 / 失败策略 / 起始版本 / 开始时间；
  - `events.jsonl`：每步 `start` / `end`（含 `duration_ms`、错误摘要），append-only，
    进程被杀也留得下；
  - `NN-<step>-<sys>.log`：该步子进程（docker compose / gx）的完整输出，单文件 4 MiB 截断；
  - `upgrade.json` 新增 `log_dir`，`--json` 同步输出；CLI 文本输出多一行「日志 <dir>」。
  写日志失败只告警，不影响升级；`runs/` 只保留最近 10 次。
- **失败错误带“最后一屏”**：命令非零退出时，错误里附最后 40 行输出（已脱敏），不再只有
  `命令执行失败，退出状态: 1`。
- **写入时脱敏**：环境里形如 `*TOKEN* / *SECRET* / *PASSWORD*` 的值，以及 `Bearer xxx`、
  `token=xxx` 模式，在写日志和拼错误前统一替换成 `***`。

### 测试

- run log：步骤事件 / 脱敏 / 保留 10 次 / 尾屏有界；升级事务的 `log_dir` 与 `events.jsonl` 断言。
  全量 `578 + 99 + 6 + 1 + 4` passed。

## [2.9.3-alpha] - 2026-10-20

### 修复

- **`gx` 路径不再依赖 `HOME`**：`gx_bin_path()` 原先固定拼 `$HOME/bin/gx`。systemd 服务
  （如 `wist-gwlinkd.service`）默认**不设 `HOME`** → 拼出 `/bin/gx` → 阶段流程探测失败，
  导致远程升级“代码换了、配置没重渲”。现改为：`$HOME/bin/gx`（存在才用）→ `PATH` 查找 →
  字面量 `gx`。
- **`ops-prj.yml` 里的 `${HOME}` 会真正展开了**：`backup.target` / `contract.target` 用
  `EnvDict::default()` 求值，而它**不读进程环境** —— `${HOME}/.backups` 被落成字面量目录
  `<prj>/${HOME}/.backups`。新增 `project::expand_process_env`，按进程环境展开。
- **`prj upgrade` 的阶段流程不再静默跳过**：`localize` 的 gx 流程因 **gx 不可用**而跑不了时，
  升级事务会当场失败（`step=regenerate`），不会拖到 `up` 阶段才炸、留下“新代码 + 旧配置”。
  交互式 `sys localize` 保持宽容，但打印 `WARN`；「项目本来没有这个流程」仍视为正常、静默。

### 测试

- 新增：gx 路径解析 3 例、进程环境展开 1 例、strict / 非 strict 阶段流程 3 例；
  端到端 fake gx 补上 `-V` 探测。全量 `574 + 99 + 6 + 1 + 4` passed，clippy `-D warnings` 干净。

## [2.9.2-alpha] - 2026-10-20

### 修复

- **代码快照的排除集改为「升级前 ∪ 升级后」并集**：2.9.1 在还原时只读**当前**（新版）
  `sys-prj.yml` 的 `protect` / `contract` / `backup`。如果新版把某项从 `protect` 里删掉，
  旧版保护过的目录在回滚时会被当成“代码”删掉，且快照里又没有它（快照按旧声明拍）——
  结果是**删了但没还原**。现在两侧声明取并集，按「任一方保护过就不碰」处理。

## [2.9.1-alpha] - 2026-10-20

### 修复

- **`prj upgrade --on-failure rollback-all` 回滚不回代码（假报成功）**：
  `backup()` 走 `prj backup`，只归档现场态（`configs/` / `knowledge/` / `.wist/`），
  **不含 `sys/` / `scripts/` / `version.txt`**。当失败发生在 `apply` 之后（典型：`pull` 阶段
  镜像拉取失败），回滚只还原了配置，代码仍是新版本 —— 事务却记成 `rolled_back`。
  现在在 `apply` **之前**给每个系统拍**代码快照**（`~/.galaxy/upgrade/<prj>/pre-upgrade/`）：
  只收不在 `protect` / `contract` / `backup` 里的条目（`values` / `.git` 恒跳过），
  `.env` 作为 `localize` 衍生物随代码一起快照。回滚顺序：现场数据 → 代码 → 拉起 → 探活；
  任一失败记 `failed`（不再冒充 `rolled_back`）。升级成功后删除快照；`--on-failure halt`
  保留快照并在 `detail` 里给出路径，供人工恢复。
  - 快照失败视为**未动现场**：直接停在 `step=snapshot`，不进 `apply`、不触发回滚。
  - 还原时会删掉新版新增的代码条目（顶层目录 / 文件），不只覆盖旧文件。

## [2.9.0-alpha] - 2026-10-20

### 新增

- **`sys-prj.yml: secrets.mode`：显式选择密钥文件模式**（`0600` 默认 / `0640`）。
  `0640` 用于与固定 gid 的容器共享：属主 = 部署账号、属组 = 容器 gid，不需要 root。
  ⚠ 这是新字段：2.8.0 的 `secrets` 节使用 `deny_unknown_fields`，会直接拒绝 `mode`；
  **先升级 gops，再部署带该字段的 sys-prj.yml**。
- **`sys-prj.yml: perms.services.<name>`：按服务声明“有效容器身份”**（`uid` / `gid`）。
  用于镜像 `Config.User` 为空、但入口脚本会降权到非 root 的场景（典型：官方 `postgres` 以 root
  启动、`gosu` 到 999 之后才读 `/docker-entrypoint-initdb.d`）。**只影响 `sys perms` 的宿主
  属主/权限对齐，不改变容器运行时身份**；要改运行时请改 compose 的 `user:`。
  全局 `perms.container_uid/gid` 存在时本项被忽略；服务名拼错会告警。

### 修复

- **`Config.User` 为空不再被当成“无法确定”**：Docker 语义下空值就是 root（0:0），不是读取失败。
  `sys perms` 现在让以 root 运行的服务打印 `[INFO]` 并**跳过对齐** —— root 不受宿主文件模式限制，
  强行对齐到 `0640` 反而会弄坏由非 root 进程读取的挂载（`htpasswd` → nginx worker、
  `initdb.d` → 降权后的 postgres）。这也消除了 `db-init` / `db-schema` / `git-proxy` / `web` /
  `postgres` 这类服务上长期存在的误导性 `[WARN] ...（镜像未拉取？）`。
- 真正读不到 `Config.User`（`docker image inspect` 失败，通常是镜像未拉取）才告警，
  提示里直接给出 `perms.services.<name>` 的写法。
- **`secrets.file` 与只读 bind 对齐的冲突**：宿主加载器要求 `0600/0400`，
  `perms` 却按只读挂载要求 `0640`，`gops run start` 会在两条规则之间来回报错（死锁）。
  现在用 `secrets.mode` 显式选择共享策略：默认 `0600`（只属主可读）；与容器共享用 `0640`
  （属组 = 容器 gid），部署账号按 owner 读、容器按 group 读。`perms` 对密钥的期望值是
  `部署账号:容器gid` + 该模式，**不会 chown 到容器 uid**（不引入提权）；root 消费者仍跳过。
  `0600` + 非 root 容器判为**配置错误**（容器读不到，必须改 `secrets.mode`）；
  多个非 root 消费者 uid 不同时同样按配置冲突报错。

## [2.8.0-alpha] - 2026-10-20

### 新增

- **`gops sec`：密钥只读视图 + 带密钥执行**。`gops sec list`（只出键名与长度）、
  `gops sec get <key>`（默认掩码 `********`，`--show` 才明文）、
  `gops sec run -- <cmd>`（注入子进程环境后 `exec`，值不进 argv）。
  刻意**不提供** `env` 子命令：生成 `export ...` 供 `eval` 是命令注入面，也会让密钥长期留在 shell 环境。
- **`sys-prj.yml: secrets` 声明密钥文件**：`secrets.file` 指定系统根内的相对路径，
  `gops run` 与 `gops sec` 共用同一加载器；变量名按 `SEC_` + 大写自动派生（`center_admin_token` →
  `${SEC_CENTER_ADMIN_TOKEN}`），清单里不写变量名和值。
- **`secrets.expose` 按消费方限定注入范围**：`run` / `localize` / `sec`，键名模式只支持 `*`；
  `run`、`sec` 缺省全量，`localize` 缺省**不注入**。`gops sec list --for <消费方>` 可预览。

### 变更

- `gops run` 只在会落地的子命令（`start` / `install` / `diagnose`）读取密钥文件；
  `status` / `stop` / `download` / `uninstall` 不再因密钥文件异常而无法执行。

### 安全

- 密钥文件加载 fail-closed：路径必须相对系统根（禁 `..` / 绝对路径 / 符号链接，含悬空链接），
  只允许属主可读（非 0600/0400 拒绝），拒绝硬链接，拒绝数字 / 浮点 / 嵌套值与非法环境变量名；
  解析错误不输出明文，`SecFile` 的 `Debug` 不含值。
- `secrets:` 与 `expose:` 拒绝未知字段，消费方名拼错直接报错。

## [2.7.3-alpha] - 2026-10-19

### 修复

- **`gops prj upgrade` 升级后挂载配置真正生效**：切换阶段默认先 `docker compose stop` 再 `up -d`。
  以前只跑 `up -d`，compose 只重建镜像/环境变了的容器；bind mount 的配置文件
  （WPL/OML、`*.toml`、证书）内容变了也不会重读，升级后可能仍在跑旧配置。
  不想要这次停机可用 `--no-stop`（回滚同样生效）。

## [2.7.2-alpha] - 2026-10-18

### 修复

- 修正 `sys perms` 在 macOS 临时目录（`/var` → `/private/var`）下的路径断言，CI 恢复全绿。

## [2.7.1-alpha] - 2026-10-18

### 修复

- `sys localize` 的宿主权限自检告警改走 **stderr**，不再污染 stdout 的值/文件变更表
  （脚本与 CI 可继续按 stdout 断言）。

## [2.7.0-alpha] - 2026-10-18

### 新增

- **`gops sys perms`**：从 compose 派生 bind 挂载的宿主属主/权限。默认只检查，`--apply`
  幂等修复，`--json` 机器可读；写挂载 `660/2770`、只读挂载 `640/2750`，容器自建文件不动，
  运行态新文件自动纳入。
- 生命周期自动接线：`sys localize` 后自检告警、`run start` 启动前拦截、
  `prj restore` / `prj upgrade` 后自动对齐、`prj diagnose` 增加 perms 一节。
- `sys-prj.yml` 支持 `perms.container_uid/gid`（rootless / userns-remap）与
  `perms.allow_external`（系统根外的 bind source）。
- 安全保护：`.:/app` 这类“挂载源等于/包含系统根”的写法会跳过并告警；
  同一路径被不同容器身份挂载会判失败，不静默通过。

## [2.6.5] - 2026-10-18

### 修复

- **现场版本缺失时给出明确提示**：`prj upgrade patch/minor/major` 在 `version.txt` 与
  `deliver.lock` 都没有版本时，不再报“不是 semver”，而是提示 `现场版本未知`，
  并给出 `-v` / `--to` 两条出路。
- **版本清单返回 401/403 时不再阻断探测**：部分对象存储对不存在的 `versions.json`
  返回 403，现在视为“没有清单”，继续按 URL 探测。
- **探测失败提示补全**：说明已扫描的窗口候选数，并给出 `-v <精确版本>` / `--to` 逃生口。

### 测试

- 新增 8 个用例：现场版本缺失、无命中提示、stable/beta 通道后缀、patch/minor 基于
  当前 major.minor、清单 403 回退，以及 `resolve_target` 的 minor/stable/版本缺失集成路径。

## [2.6.4] - 2026-10-18

### 变更

- **探测窗口收敛**：`patch` 候选从 50 降到 20，`minor` 从 10 降到 5 个 minor，
  `major` 从 5 降到 3，每个 major 扫 minor 0..=2；仍保持 8 路并发与命中即停。
  对远端 COS 实测，全窗口未命中约 2.4s（改前 patch ≈6.5s / major ≈16s）。

## [2.6.3] - 2026-10-18

### 优化

- **URL 探测改为有界并发**：`patch` / `minor` / `major` 的候选版本按升序每 8 个一批并发探测，
  命中即取该批最小版本并结束；探测成功后也不再重复 HEAD。
  实测远端 COS（单次 404 约 0.4s，连接复用后约 0.13s）：命中场景约 0.5s，
  全窗口未命中由约 6.5s（patch）/ 16s（major）降到约 3.6s。

## [2.6.2] - 2026-10-18

### 修复

- **`gops prj upgrade --help` 续行缩进**：去掉多行说明里的多余前导空格。

## [2.6.1] - 2026-10-18

### 修复

- **`gops prj upgrade --help` 文案同步**：明确普通 HTTP 源（COS/S3/OSS/nginx）上
  `patch`/`minor`/`major` 会按版本规则探测包地址，不再提示“普通 HTTP 无法枚举版本”；
  `latest` 仍需要 `versions.json` / `versions.txt`。

## [2.6.0] - 2026-10-18

### 新增功能

- **普通 HTTP 版本源**：`prj upgrade` 不再要求包地址是 GitHub Releases 或 git 仓库，
  COS / S3 / OSS / nginx 等任意带 `{version}` 的地址即可按关键字升级。
  - `patch` / `minor` / `major` 会按语义化版本规则生成候选 URL 并 HEAD 探测，
    找到第一个真实存在的包（patch 最多向前 50 个，minor/major 也有有界窗口）；
  - 按 addr 里的 `{version}` 模板直接替换出候选地址，不要求额外的清单文件；
  - `latest` 没有天然停止条件，仍需可枚举清单：在 `addr` 版本段的父目录发布
    `versions.json` 或 `versions.txt`（每项一个版本），如
    `https://.../wist-center-stack/versions.json`；没有清单时用 `patch`/`minor`/`major` 或精确版本。

## [2.5.0] - 2026-10-17

### 新增功能

- **`gops self check` / `gops self update` 的 `--channel` 支持短选项 `-c`**：
  `gops self check -c alpha` / `gops self update -c alpha --yes` 与长选项等价。
- **`gops prj upgrade` 支持版本 spec 与通道**：`upgrade [<sys>] <spec>` / `-v/--version <spec>`
  接受 `latest`、`patch`、`minor`（`feat` 别名）、`major` 或精确版本；`-c/--channel stable|alpha|beta`
  选择通道（缺省按当前部署版本后缀推断）。关键字 spec 会在**任何副作用之前**从版本目录
  （GitHub Releases / git tag / 本地包目录）解析成具体版本；成功后把具体版本回写到
  `ops-prj.yml` 的 `version:`（`--no-save` 可关闭）。`--to <url|path>` 仍是完整地址逃生口。
- **升级前预检目标包存在性**：在 diagnose/backup 之前检查本地文件、HTTP（`HEAD`，
  不支持时退回 `GET Range`）或 git 仓库；包不存在/不可访问时直接失败，不产生备份或部分覆盖。
- **同版本 no-op**：目标版本与现场版本全部一致时，不执行 diagnose/backup/apply/pull/up/health，
  直接写一条 `succeeded` 状态（`no_op: true`）；`--force` 可强制重跑。`--dry-run` 会显示 no-op。
- **`prj import` 自动模板化版本地址**：导入 `.../pkg-1.0.0.tar.gz` 时自动把 `ops-prj.yml` 里的
  addr 改写成 `.../pkg-{version}.tar.gz` 并写入 `version: 1.0.0`；支持文件名与相邻 release tag 目录
  （如 `.../v1.0.0/pkg.tar.gz`），git 地址与无法识别版本的地址保持原样。

### 变更

- **`gops prj upgrade --on-failure` 默认值定为 `rollback-all`**：不再强制必填；
  需要停在原地排查时显式传 `--on-failure halt`。

## [2.4.0] - 2026-10-14

### 新增功能

- **`gops self check` / `gops self update` 支持下载镜像前缀 `--mirror <PRESET|URL>`**：可用内置预设
  `--mirror githubproxy`（`https://githubproxy.cc/`），或传自定义前缀 URL。前缀只作用于自升级的
  manifest 与制品下载地址，sha256 校验照常；仅对绝对的 http(s) URL 生效。未知预设名会报明确错误，
  不会被当成 URL 直接使用。

### 变更

- 升级 `wp-self-update` 到 `0.5.0`（`SourceConfig` 新增必填字段 `url_prefix`，属破坏性变更），
  `gops` 依赖由本地 path 切回 crates.io。

## [2.3.2] - 2026-10-13

### 变更

- **`gops sys update` 更名为 `gops sys resolve`**（旧名 `update` 保留为**隐藏别名**）：该命令做的是
  「解析系统定义」—— 按 `sys/mod_list.yml` 各模块的 `addr` 把它们取到新布局 `sys/<model>/mods/<mod>/`、
  解析依赖、把模块变量 ⊕ 系统 `sys/setting/vars.yml` 变量聚合写入 `sys/merged_vars.yml`（解析结果，需入库），
  并刷新 `values/sys_value.yml` 注释模板。原名 `update` 易被误解为「升级 / 更新运行中的系统」，故改名。
  **行为不变**：不生成 `.env`（用 `sys localize`）、不碰运行中的服务（用 `gops run`）。
  相关文案（缺失变量报错、`sys package` 前置解析、`sys localize` 默认先跑的那一步、`prj upgrade` 事务的 regenerate）同步更新。

### 移除

- **移除 `gops sys setting`**：该命令只在 `--init` 时把 `sys/setting/` 覆盖成**示例占位**（`SYS_KEY1=sys_value1` 等），
  且与 `gops sys new`（本就生成 `sys/setting/`、幂等、只补缺失）重叠，还会破坏已有配置。
  需要 setting 骨架时用 `gops sys new --name <系统名>`（只补缺失，不覆盖）。
- **移除 `gops sys check`**：它是只读的「重算值 vs 磁盘 `.env`」漂移卡口，但 `.env` 是 gitignored，
  做不成仓库侧 CI 卡口；而 `sys localize` 默认已先重解析，“改了值没重新 localize”的空间很小。
  保留其唯一有效的部分：**`gops sys localize` 结尾**在「变量定义比已解析结果新」时给一行 `[WARN]`
  （仅比对 `.env` 看不到的那一层）。

## [2.3.1] - 2026-10-12

### 变更

- **现场态声明彻底解耦**：`sys-prj.yml` 的 `pkg_include` / `pkg_exclude` / `protect` / `contract` / `backup`
  五个声明**互相独立**，各由各自的命令消费，**不做任何跨声明的推导 / 覆盖 / 交叠判断**：
  - `pkg_include` / `pkg_exclude`：只服务 `gops sys package`（额外进包 / 不进包；相交时 exclude 优先）；
  - `protect`：只服务 `gops sys update` / `prj sync` / `prj upgrade`（升级不碰）；
  - `backup`：只服务 `gops prj backup` / `restore`；
  - `contract`：只服务 `gops prj contract export` / `import`。
  随之移除 `SysConf::protection_paths()`（`protect ∪ contract`）、`prj sync` 里对 `contract` 的并入，
  以及 `diagnose` 的全部跨声明检查（`contract.overlap`、`backup.missing`、`backup.protect.uncovered`、
  `protect.covers_ignore` / `protect.covers_pkg_exclude`）。`prj diagnose` 只保留各声明**自身**的检查。
- **`sys-prj.yml` 的 `ignore:` 更名为 `pkg_exclude:`**（旧名 `ignore` 仍可读，`#[serde(alias)]`）：
  该节只表达“不进交付包”，改名为名实相符；`gops sys package` 的排除即取此节。
- **`sys-prj.yml` 新增 `pkg_include:`**：在默认（git 入库文件）或 `--full`（整目录）的**基准集之外**
  额外把命中它的路径纳入交付包 —— 用于让**未被 git 跟踪**的生成物 / 制品随包走（可覆盖 `.gitignore`）。
  `pkg_exclude`（排除）**优先于** `pkg_include`（相交时以排除为准）。
- **`contract` 独立于 `protect` / `backup` / 打包**：契约（`sys-prj.yml: contract`）**不**自动纳入 `protect`，
  也**不**自动进 `prj backup`、**不**自动排除出交付包 —— 契约只走**独立归档**
  （`gops prj contract export` / `import`），安全要求与备份频率都和常规现场态不同。
  文档同步更正：此前声称的「声明即自动保护 / 不进包」均已删除（不引入任何隐式逻辑）。

## [2.3.0] - 2026-10-12

### 新增功能

- **对外契约（`contract`）**：`sys-prj.yml` 新增 `contract:` —— 声明**不可重建的公开现场事实**
  （例如现场选的域名落在某个 `.toml` 里）。声明即自动纳入保护（升级不碰）与备份（`restore` 必收），
  且不进交付包；`ops-prj.yml` 新增 `contract:`（归档落点 / 份数），配套 **`gops prj contract export` / `import`**
  独立归档与回填。见 `docs/design/contract.md`。

### 变更

- **`gops prj update` 更名为 `gops prj sync`**（旧名 `update` 保留为**隐藏别名**）：它与事务式发布升级
  `gops prj upgrade` 名字太像，故拆开。
- **`gops prj sync` 现在补齐缺失的系统**：`ops-prj.yml` 记录、但目录不在场的系统会被按包**导入**；
  已在场的系统仍是非破坏覆盖（`protect` / `contract` 不碰、**不删**任何东西）。
- **现场态声明整理**：`sys-prj.yml` 的 `preserve:` 更名为 **`protect:`**（旧名仍可读）；
  `backup:` 由分级 `{restore, rebuild}` 简化为**扁平列表**（只表达“丢失的代价”，**不触发任何删除**）。
- **本地包地址以项目根为基准解析**：`ops-prj.yml` 记录的本地 `addr.path` 相对**项目根**，可移植（如 `../../sys-operators/…`）。
- **依赖解析不再依赖进程 CWD**：模块级 `test_envs` 以**模块根**、模型级 `depends` 以**模型目录**为基准；
  拒绝绝对路径与 `..`，避免“隐式顶替”与 CWD 漂移。
- **启动更友好**：进程 CWD 已不存在（如所在目录被删/重建）时给出可操作提示，不再只留一句 `RES ERROR / get current dir`。

### 移除

- **移除 `gops prj rebuild`**（breaking）：它是**唯一**会清掉「包外未声明内容」的操作（彻底重建 / 换身份）；
  现场态声明（`protect` / `contract` / `backup`）与 `gops prj sync`（**不删**包外内容）已覆盖其诉求。
- **移除 `gops prj reimport`**（breaking）：其「补齐缺失系统」的职责已并入 `gops prj sync`。

详见 [UPGRADE.md](./UPGRADE.md)。

## [2.2.3] - 2026-10-10

### 变更

- **升级 `orion-error` 至 0.9**：`orion-error 0.8→0.9`、`orion-infra 0.7→0.8`、`orion_conf 0.7→0.8`、`orion-accessor 0.8→0.9`、`orion-variate 0.13→0.14`、`orion-sec 0.6→0.7`、`wp-self-update 0.3→0.4`。
- **统一生命周期日志**：把模块 / 本地化 / 系统 / 运维项目各处的 `auto_exit_log!` + 成对 `info!`/`error!` 改为 orion-error 0.9 的 `OperationContext` + `AutoLogGuard`（`doing(...).with_auto_log().with_mod_path(...)` + `mark_suc()`），成功 / 失败 / 取消统一走 `suc!`/`fail!`/`cancel!`。
- **顶层错误输出改用 0.9 诊断渲染**：`report_error` 改走 `StructError::render()`（reason / detail / position / context），不再手写 reason match。

## [2.2.2] - 2026-10-08

### 变更

- **发布制品矩阵收敛为 musl**：Linux 统一使用 musl 静态制品（`x86_64-unknown-linux-musl`、新增 `aarch64-unknown-linux-musl`），不再发布 glibc 制品；制品包内顶层目录名与资产名一致（含平台 triple）。
- **`gops self` 在 Linux 上改用 musl 制品**（升级 `wp-self-update` 至 0.3.3）：静态链接、跨发行版/容器（含 Alpine）通用；过渡期内旧客户端仍可经别名获取同一 musl 制品。

## [2.2.1] - 2026-10-03

### Bug 修复

- **`gops prj upgrade --dry-run` 现在会校验目标可解析**：原来只出计划、不解析目标，
  于是「给了版本但该系统的 `addr` 没有 `{version}` 占位」这种必然失败的输入，计划却是一片绿，
  真跑才在 `apply` 炖。现在 dry-run 逐系统解析目标（只读、不写状态），解析不了就当场报错，
  并把每个系统解析出的地址一并显示在计划里（`--json` 也有 `resolved`）。
- **升级记录的版本读对了**：原来优先读 `deliver.lock`，而它是**交付时生成的 git-ignored 产物、
  不在交付包里**，升级**不会更新它** —— 于是升到新版本后记录仍写旧版本（现场实测：栈包已到
  0.1.23，记录却写 `0.1.17 → 0.1.17`）。现在**优先 `version.txt`**（随包走、升级会更新），
  `deliver.lock` 只作兜底。

## [2.2.0] - 2026-10-03

### 新功能

- **`gops prj upgrade`：发布态的自动化升级事务**（`kind: docker-compose`）。一条命令把
  「备份 → 覆盖包内内容 → 重渲值与 `.env` → 拉镜像 → 切换 → 探活」串成一次**可回滚**的升级，
  替代手敲 `prj update` + `sys localize` + `run download` + `run start` 并靠人记得先备份。
  - **按版本升级**：`ops-prj.yml` 的 system ref 支持 `version:`，`addr` 可用 `{version}` 占位
    （如 `.../x-v{version}.tar.gz`）；`gops prj upgrade --to <版本>` 自动解析，也仍接受完整地址。
  - **先拉后停**：新镜像没到齐绝不动运行中的服务；失败按 `--on-failure` 处置
    （`rollback-all` 整工程回滚 / `halt` 停在中间态），回滚是**状态级**（含库）。
  - 另有 `--dry-run`（只出计划）、`--json`（给编排器判成败）、`--health-cmd`（栈外探活）。
- 含 `{version}` 但没写 `version`、或有版本但 `addr` 没占位，都会在**解析处明确报错**，
  绝不拿字面量 `{version}` 去下载。

### 文档

- 新增 `docs/design/prj-upgrade.md`：发布态升级事务的设计与取舍（阶段序、失败处置、边界）。

### 内部

- 测试稳定性：不再在用例里 `remove_var("TEST_MODE")`（进程级全局，撤掉会让并发跑的其它用例
  掉进交互式选择而 panic），并让 `WorkDirWithLock` 不再被一次 panic **中毒**连带拖垮其它用例。

## [2.1.4] - 2026-10-03

### Bug 修复

- **`prj rebuild` 把保留项搬到「目标已存在」时按类型清理**：保留项现在可能是**目录**（整棵搬），
  若包内恰好有同名**目录**，原来只 `remove_file` 会在目录上失败 —— 改为目录 `remove_dir_all`、
  其余 `remove_file`（符号链接仍按链接删，不跟进去）。
- **保留项收集跳过**任意深度的 `.git`**（与 `walk_files` 同一口径；原来只跳顶层）。

### 文档

- `docs/design/prj-backup-restore.md` §5.1 补「保留项粒度 = 最浅匹配项」：命中目录就整棵 `rename`、
  不进入内部（连带的 `EACCES` 缘由）。

## [2.1.3] - 2026-10-03

### Bug 修复

- **`gops prj rebuild` 不再卡在「容器以别的属主写的运行期目录」上**：保留项（`sys-prj.yml: preserve`）
  的搬走/搬回，从**逐文件 `rename`** 改成**按最浅匹配项整体 `rename`** —— 命中一个目录就整棵搬，
  **不再进入**它内部。于是 `preserve: configs` 会把 `configs` 一次性改名搬走再搬回，全程不碰
  容器私有的 `configs/gateway/state/knowledge/`（uid 999、部署账号读写不了）—— 逐文件搬会在那里
  `rename` 时因**父目录不可写**而 `Permission denied (os error 13)`。
  语义不变（`path_matches` 是祖先匹配：目录命中 ⇒ 子树全部命中）；细到文件的模式（如
  `configs/gateway/state/*.pem`）仍按文件收集。

## [2.1.2] - 2026-10-03

### Bug 修复

- **`gops prj restore` 不再被「目标文件归别的属主」挡住**（备份**收得进**、还原**写不回**）。
  现场真实一例：`configs/gateway/state/wist-gateway-store.db*` 由**容器身份**（`999:999`）创建
  （`align-host-perms.sh` 刻意不碰容器自建的库），而 `prj restore` 以**部署账号**跑 ——
  原来用 `fs::copy` **原地覆盖**，打开目标写入即 `EACCES`，还原中断在半途（只留一句 Permission denied）。
  现在落盘改为**同目录暂存 + `rename(2)`**：`rename` 只要求**目录**可写、**不要求目标文件可写**，
  所以归别人的文件也换得掉，且**不需要提权**。附带两点：单个文件落盘变**原子**（不留半截）；
  还原后库文件的属主是**部署账号**，正是「恢复搬过来的库」那种，`align-host-perms.sh` 会把属组
  放到容器 gid（`660`）。
- 只有当**目录**也不可写时才失败，且报错直接给出处置（停容器后重试 / 先 `sudo rm -f` 该文件）；
  目标同名条目是**目录**时也改为一句人话，不再抛 `ENOTDIR`。

## [2.1.1] - 2026-10-01

### Bug 修复

- **修复「全新的机器首次 `gops prj import / update / reimport` 必失败」**：取包前未保证
  `$HOME/ds-package` 存在，而下载器在「目标不是已存在的目录」时会把**整条路径当成文件名**写——
  于是 `~/ds-package` 被写成一个文件，随后建 `~/ds-package/<包名>` 报 `create_dir_all … ENOTDIR`。
  现在取包前先保证工作目录就绪；若该路径已被同名文件 / 悬空符号链接占用，会**明确报错并给处置命令**，
  不再只留一句看不懂的 `system error`。
- 解包目录准备失败时改为透出**底层 io 原文**（只读 / 磁盘满 / 权限 / 被同名文件占用）；
  同名文件或符号链接会被删掉重建（原来直接失败）。

### 变更

- 取包/解包工作目录不再硬编 `$HOME/ds-package`，优先级：`GOPS_PACKAGE_DIR` 环境变量 >
  旧目录 `$HOME/ds-package`（**存在且是目录**时沿用，不打断在跑的机器）> 平台缓存目录的 `gops`
  （Linux `$XDG_CACHE_HOME/gops`，缺省 `~/.cache/gops`；macOS `~/Library/Caches/gops`）。
  `--debug 1` 会打印实际使用的工作目录。

### 说明

- `ds-package` 这个前缀来自早期的 `ds-sys` / `ds-mod` / `ds-ops` 二进制（0.11.0 已改名为 `g*`），
  当时漏改了这个目录名。仍是目录的机器会继续沿用；想搬到新位置，删掉它即可。

## [2.1.0] - 2026-10-01

### 新增功能

- **`gops prj` 有了现场态能力：非破坏升级 / 备份 / 还原 / 重建。** 声明分两级——
  `sys-prj.yml` 的 `preserve:`（升级不覆盖、也不删）与 `backup.{restore,rebuild}`（备份分级），
  `ops-prj.yml` 的 `backup.{target,keep,systems}`（落点 / 份数 / 收哪些系统）。
  - `gops prj update`：把包内内容覆盖到已交付系统上，`preserve` 一律不碰 —— 升级不再只能靠人搬文件。
  - `gops prj backup` / `prj restore`：按分级收现场态，含清单 + sha256，并**点名含私钥的条目**提示离机保管；
    还原先解到临时目录再合并，失败不留半截。
  - `gops prj rebuild [<系统名>]`：重建系统目录 —— 现场态搬走搬回、旧目录先留后删、失败回滚。
    这是**唯一**会丢“包外未声明内容”的操作，所以不做成默认。
- **`gops prj diagnose`**（原名 `doctor`，旧名仍可用）：输出改成与 agentd 同口径（`[OK]/[WARN]/[FAIL]` +
  缩进 detail + `→` 处置提示 + `结论:`，非 TTY / `NO_COLOR` 自动纯文本，有 FAIL 时退 1 供 CI 卡口）；
  新增检查 `ignore ⊆ preserve`、`sys_models` 重名、`backup.restore` 是否声明、preserve 是否被备份档覆盖。

### 变更

- `gops prj reimport` 收紧为**只补缺失**：目录已存在就拒绝（旧行为是 `rm -rf` 重装），
  并指向 `prj update`（非破坏升级）与 `prj rebuild`（重建）。
- `gops prj update` 不再只更新项目 conf：现在还会逐个系统下载包做非破坏覆盖；包不可达时会失败。

### Bug 修复

- `prj rebuild` 回滚没走干净时不再顺手删掉暂存区（那可能是原目录与现场态的**最后一份拷贝**）；
  上次遗留的暂存区会让下一次 `rebuild` 直接拒绝，而不是把它覆盖掉。

## [2.0.13] - 2026-09-30

### Bug 修复
- `gops self skill install`（整包）的目标目录名改为**按来源推导**（远程 URL 末段 / 本地目录名），不再固定为 `gops-skills` —— 修复 `--source galaxio-labs/gx-skills` 被装成 `gops-skills` 的问题。

## [2.0.12] - 2026-09-30

### 新增功能
- **`gops self skill`**：安装 / 列出 agent skills（默认源 [gops-skills](https://github.com/galaxio-labs/gops-skills)）到各 agent 目录（codex / claude / zed / 自定义）。用内置 `git2` 浅 clone + `serde_yaml` 校验 `SKILL.md` frontmatter，不再依赖 `install.sh` / `python3` / `ruby`；`install` 支持 `--source` / `--ref` / `--target` / `--dir` / `--symlink` / `--yes`，`list` 列出来源仓库中的 skills。

### 文档
- README：安装说明补齐「稳定版 / 测试版（beta）/ 开发版（alpha）」三通道，并新增「安装 agent skills」小节；CLI 概览的「常用子命令」按命令分节。

## [2.0.11] - 2026-09-30

### 依赖更新
- `orion-accessor` 0.8.3（下载原子性）：`download` 先写 `<file>.part`，成功后原子替换；校验 `Content-Length`；对传输 / 5xx / 截断失败重试最多 3 次。`gops run download` 的缓存复用语义不变，但不再在目标位置留下半包。
- 说明：2.0.10 的 `Cargo.lock` 仍锁 `orion-accessor 0.8.2`（已发布的二进制未含此修复），本版升至 `0.8.3`。

### 文档
- `UPGRADE.md` 补齐 2.0 破坏性变更与依赖版本对齐。

## [2.0.10] - 2026-09-30

> 汇总 2.0.1 – 2.0.9（alpha 快速迭代）。

### 新增功能
- **变更呈现**：`gops sys diff` / `gops mod diff` 逐键列出值的**初始值 → 生效值**、来源与可变性（含系统层/模块分组；`sys diff` 另列 `sys/setting/<mod>` 相对模块 `spec/` 的**文件覆盖**）；`sys` / `mod localize` 末尾附带值 + 文件变更表。`--json` 可脚本消费。
- **`gops self` 自升级**：`status | check | update | rollback`。
- **`gops sys package`** 支持 `sys-prj.yml` 的 `ignore:` 节（默认与 `--full` 都生效）。

### 重大变更（破坏性）
- **拆出 `gops run`**：`download` / `install` / `uninstall` / `start` / `stop` / `status` / `diagnose` 从 `gops sys` 移到 **`gops run <cmd>`**；`gops sys` 只保留定义 / 交付 / 工件。
- **`gops sys diff --json`** 由平铺数组改为分组对象 `{ system, modules, files }`。
- `gops sys package` 的 `--no-git` 更名 **`--full`**（旧名保留为隐藏别名）。

### 改进优化
- 版本横幅与错误输出改走 **stderr**，保证 `--json` 时 stdout 干净。
- 自升级 `rollback` 用临时文件 + rename 原子替换，并保留最近 5 个备份。
- `sys check` 对「定义比已解析结果更新」给出 `[WARN]`（不改变退出码）。

### Bug 修复
- `sys diff` / `localize` 补上**模块层覆盖**（此前只比对系统层，漏掉 `values/<mod>/mod_value.yml`；现按 `mod_list.yml` 逐模块分组）。
- 值文件的**小写键**不再被静默忽略（加载时统一归一化为大写）。
- `sys localize` 默认**先重解析变量**（改 `sys/setting/vars.yml` 后一条命令即生效；`--only` 仍跳过）。

## [2.0.0] - 2026-09-29

> **破坏性**：系统模块布局重构，与 1.x 不完全兼容；请配合 `galaxio-hub/ops-gxl` 的 `2.0` 通道。

### 新增功能
- **`gops sys package` 默认只含入库文件**：按 `git ls-files` 打包（需在 git 仓库内），自动排除 `sys/*/mods/`、`**/local`、`.env` 等产物；`--no-git` 才打当前目录全部。`deliver.lock` 始终随包分发。
- **`localize` 阶段扩展点（docker-compose）**：`sys localize` 写完 `.env` 后，若 `_gal/work.gxl` 有同名 `localize` 流程则执行（`gx run localize`）；gx 缺失 / 版本过旧 / 无该流程则静默跳过（保持「compose 无需 gx」）。`--no-flow` 可跳过。

### 改进优化
- host 模块制品缓存提升到 host 级（`sys/<model>/local/cache`，同模型模块共享；不被 `sys localize` 清掉）；旧模块需重新生成 `workflows/operators.gxl`。
- 阶段流程注入的配置与 `.env` 完全一致，并保留 `PATH` / `HOME` / `GX*` / `GXL_*` 等关键变量。

### 重大变更（破坏性）
- **模块布局按模型分组**：`sys update` 把模块落到 `sys/<model>/mods/<mod>/`（原 `sys/mods/<mod>/<model>/`）；算子模板 `extern` 统一指向 `galaxio-hub/ops-gxl` 的 `2.0` 通道（`main` 留给旧布局）。
- **兼容与迁移**：读取优先新布局、缺失回退旧布局（存量项目不重跑 `update` 也能 `localize`）；`update` 成功后清理遗留 `sys/mods`、给 `.gitignore` 补 `sys/*/mods`、并自动迁移 `sys/setting/list.yml` 里旧布局的 `dst`。

## [1.3.3] - 2026-09-28

### 新增功能
- **docker-compose 定义内收**：`gops sys new --kind docker-compose` 默认生成 `sys/docker-compose.yaml`；`gops sys` 按 `sys/{compose,docker-compose}.{yaml,yml}` → `<root>/…` 查找（`sys/` 优先）

### 改进优化
- compose 项目目录始终锚定系统根（项目名 = 根目录名，相对挂载与 `.env` 相对根）：`sys/` 布局显式 `-f` + `--project-directory` 并合并同目录 `<stem>.override.{yaml,yml}`（显式 `-f` 会关闭 docker 的 override 自动合并）；旧布局（compose 在根）不传全局参数，保留 override 自动合并与 `COMPOSE_FILE` 语义
- 示例 `knowlege/docker-compose` 精简为纯 compose 最小形态：移除对纯 compose 系统无用的 `_gal/`，新增可版本化的 `values/value.yml` 客户覆盖样例

### Bug 修复
- 修复示例 `knowlege/docker-compose` 无法 `up`：去掉「固定端口 + 多副本」的冲突（改为单副本），db 密码改用 `${SEC_xxx:-demo}` 回退值，开箱即可 `gops sys start`
- `sys new --kind docker-compose` 模板补充「固定端口 + 多副本会端口冲突」的注释

### 重大变更
- `kind: gxl` 系统脚手架示例 compose 迁到 `sys/docker-compose.yaml`（原根目录），旧布局仍兼容

## [1.3.2] - 2026-09-27

### 新增功能
- **交付锁 `deliver.lock`**：`gops sys package` 生成（置于系统根目录并随交付包分发），记录 `name`/`version`/`kind`/`model`、模块引用（名称/模型/来源地址/是否启用）与值指纹（`sha256:` 覆盖 `sys/merged_vars.yml` 与整个 `values/` 目录树），回答"部署的是哪一版、用了什么值"，为复现与回滚留依据
- **`gops sys check`（轻量漂移报告）**：只读比对"当前合并值 vs 已生成的 `.env`"，报告"值已变更但未重新 localize"（不写盘、不做完整 reconcile；存在漂移时非零退出，可用于 CI 卡口）
- **`gops prj doctor`**：只读体检客户值 `values/` 是否被版本控制纳管——目录是否存在、`ops-prj.yml` 导入的每个系统是否有值目录、是否被 `.gitignore` 忽略、是否有未提交改动；`--strict` 将警告升级为错误
- 新增直接依赖 `sha2`（交付锁的内容指纹）

### 改进优化
- 抽出 `project::render_env`（与 `export_env_file` 同源），供漂移比对复用，避免"生成"与"比对"两套逻辑漂移
- 为 k8s Helm 脚手架补齐回归测试：chart 生成/不覆盖用户修改/幂等、模板内容不变量、`ModelSTD::is_k8s`、`k8s_var_init` 作用域、仅 k8s 模型生成 chart、example 模型可 localize

### Bug 修复
- 修复 `gops mod example` / `mod 4test` 的 k8s 模型构件形态：应为容器镜像（`local: docker_image`），与 `helm_ops` 对齐，而非二进制归档
- 修复 `workflow/act.rs` 模板测试中 `matches!(...)` 被当作语句丢弃、断言实际无效的问题
- 修复 `gops prj doctor` 汇总行把条目总数当作"提示"数打印（有警告时重复计数）
- 修正 `render_env` / `export_env_file` 的文档注释被错误拼接的问题

## [1.3.1] - 2026-09-27

### 新增功能
- **k8s 模型 Helm 脚手架**：`gops mod new` 为 `x86-ubt22-k8s` 自动生成 Helm chart（`spec/confs/{Chart.yaml,values.yaml,templates/{deployment,service}.yaml}`），并写入 k8s 约定变量（`APP_NAME`/`NAMESPACE`/`IMAGE_*`/`REPLICA_COUNT`/`SERVICE_*` 及系统 `KUBECONFIG`/`AIR_GAPPED`/`RUNTIME`）与 `setting.yml`（用 `[[ ]]` 渲染标签并排除 `spec/confs/templates`，避免与 Helm 的 `{{ }}` 冲突）
- k8s 模型默认算子改用 `helm_ops`（`mod operators : helm_ops { }`），开箱支持 `install`/`uninstall`/`update`/`status`/`download`

### 改进优化
- host 模型 `download` 脚手架同时支持 git 仓库（`origin_addr.repo[/tag]`）与 http(s) 归档（`origin_addr.url`）；下载前清理缓存目录，重复下载幂等
- chart 脚手架仅在文件缺失时写入，重复 `save`/`mod update` 不会覆盖用户对 `spec/confs` 的修改
- 统一 host / k8s 模板的 `extern` 指向 `galaxio-hub/ops-gxl` 与 `${GXL_CHANNEL:main}`
- 抽取 k8s 约定变量为 `k8s_var_init`，供 `gops mod new` / `gops mod example` 复用；`IMAGE_TAG` 允许模块级修改
- chart `values.yaml` 暴露 `imagePullSecret`，Deployment 仅在其非空时注入 `imagePullSecrets`

### Bug 修复
- 修复 `gops mod example` 生成的 k8s 模型缺少 Helm chart 与 k8s 变量、导致 `mod localize` 报 `Failed to access variable ... IMAGE_REGISTRY` 的问题
- 移除 host 算子模板中未使用的 `__into` / `_used.json` 读取

## [1.3.0] - 2026-09-17

### 新增功能
- **docker-compose 系统类型**：新增 `SysKind` 与 `sys/sys_model.yml` 的 `kind` 字段，统一 `gops sys` 入口管理 GXL 与纯 compose 系统，部署命令自动分派到 `docker compose`
- **`gops prj reimport`**：按 `ops-prj.yml` 的 `sys_models` 重新导入并保留 `values/`
- **`${SEC_xxx}` 密钥注入**：`sys localize` 导出 `.env`（仅非密钥），密钥运行时从 `~/.galaxy/sec_value.yml` 注入，不落盘

### 重大变更
- **`ops-prj.yml` 与 `ops-systems.yml` 合并**（向后兼容旧文件）
- **`sys_vars.yml` 重命名为 `merged_vars.yml`**（聚合变量：模块 ⊕ 系统合并，向后兼容旧名）

### 改进优化
- 系统文件可选化：`mod_list.yml` / `workflows/` / `setting/list.yml` 缺失时降级为空
- `sys localize` 以系统默认值（`sys/merged_vars.yml` 的 `system:` 段）为基线，值文件只需写需要覆盖的项
- `sys update` 生成的 `values/sys_value.yml` 改为**注释模板**（可用变量全部注释，默认不生效）：只需取消注释要覆盖的项，其余取系统默认值，避免全量快照钉死后续默认值变更
- `sys new` 支持已存在目录（幂等）；移除冗余的 `setup_prj_root_env_vars`（`GXL_PRJ_ROOT` 已由 CLI 启动时的 `setup_start_env_vars` 统一设置）
- 引入 `orion-sec`；许可证统一为 MIT

### Bug 修复
- 修复 `values` 符号链接冲突、`convert_addr`/`build_pkg` 裸目录 panic、`mod new` 硬编码构件地址
- 修复 `kind: gxl` 部署命令仍在调用旧执行器 `gflow` 的问题：执行器已更名为 `gx`，改为 `gx run -e <env> -d <n> [--cmd-arg <mod>] <cmd>`，版本要求 `gx >= 0.13.0`
- 修复 GXL 系统模板 `_gal/work.gxl` 中 `SYS_BIN` / `MOD_BIN` 仍指向已废弃的 `gsys` / `gmod` 的问题：改为 `gops sys` / `gops mod`（`${ENV_SYS_BIN} update` 即 `gops sys update`）
- `setting/list.yml` 示例中的占位模块名由 `gflow` 改为 `example`（去掉已废弃的工具名）
- 修复 `load_sys_opr_value`/`load_mod_opr_value` 值文件序列化不一致
- 修复运维项目内 `cd <sys>; gops sys localize` 未使用项目值的问题：按 `ops-prj.yml` 直接解析 `values/<sys_name>/`，不再依赖 `<sys>/values` 符号链接是否完整
- 修复本地化时创建字面量目录（如 `galaxy-ops/${GXL_PRJ_ROOT}`）的问题：路径中残留未展开的 `${VAR}` 或源文件不存在时直接跳过，不再提前创建目标目录

## [1.2.0] - 2026-05-04

- 升级 `orion-error 0.8` 并统一 `orion-infra`/`orion_conf`/`orion-accessor`/`orion-variate` 生态依赖，消除 0.7/0.8 并存
- 恢复 `owe_res`/`err_conv`/`with` 等兼容入口，迁移 `get_reason`/`context` 到 0.8 原生接口

## [1.1.1] - 2026-04-06

- 版本元信息同步到 1.1.1，仓库地址统一为 `galaxio-labs`，清理 workspace 依赖声明

## [1.1.0] - 2026-03-27

- 切换到新一轮 `orion_*` 生态版本，新增 `UPGRADE.md` 与 `compat` 兼容层
- 统一变量大小写不敏感读取、模板渲染收敛；修复 `SysSetting` 作用域初始化、shell heredoc/算术移位注释剥离等边界

## 历史版本（0.x）

- **0.13.0-alpha**：整合 `gmod` 到 `gops`；新增 `SysSetting`/`SysValuePaths`；重构本地化与路径管理
- **0.11.0**：`ds-*` 重命名为 `g*`；新增本地化系统与 accessor；升级 `orion-variate 0.6.2`
- **0.10.6**：新增包管理、自动化模型生成、`UpdateValue` 工作流
- **0.10.5**：添加 `gops` 二进制工具
- **0.10.4**：版本号与构建优化
- **0.10.3**：工作流项目管理
- **0.10.2**：自动化测试支持
- **0.10.1**：包管理支持
- **0.10.0**：项目从 `orion-syspec` 重命名为 `galaxy-ops`，架构重构
- **0.9.0**：工作流项目管理，迁移到 GitHub 仓库
- **0.8.0**：项目初始化，基础架构搭建
