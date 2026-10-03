# 变更日志

所有重要的项目变更都将记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.0.0/),
并且本项目遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

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
