# 变更日志

所有重要的项目变更都将记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.0.0/),
并且本项目遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [2.0.6] - 2026-09-30

### 新增功能
- **`localize` 打印文件变更表（新增 / 替换）**：`gops mod localize`（渲染 `spec/` → `local/`）与 `gops sys localize`（sys setting 模板 `src` → `dst`）在落地文件后，打印本次**新增 / 替换**的文件表 `FILE | STATE`（`created` / `replaced`）。

### 说明
- 用 localize **前后内容指纹（sha256）比对**，所以「先清空 `local/` 再重建」不会把内容未变的文件误报为变更；**删除不报**（重建输出树时属常态，非信号）。
- 文件变更表只在 **`localize`**（有前后基线时）呈现；`gops sys diff` / `gops mod diff` 仍只呈现**值**（新增/替换文件需基线，无法从只读状态推断）。
- `mod/<model>/local/` 内运行时产物（`bin/`、`cache/` 等）被 localize 清掉属预期，不计入。

## [2.0.5] - 2026-09-30

### 新增功能
- **值变更表（`gops sys diff` / `gops mod diff`）**：逐键呈现「初始值 → 生效值」，回答“哪些值被覆盖、被哪一层覆盖”。列为 `KEY` / `INITIAL` / `EFFECTIVE` / `ORIGIN` / `MUTABILITY` / `STATE`（`same|changed|added|removed`，只列非 `same` 行）；`--json` 输出机器可读结果。
- **`localize` 末尾附带变更表**：`gops sys localize` 与 `gops mod localize` 完成后打印同一张表（无覆盖时打 `[OK] 值无覆盖`），不必再跑一次 `diff`。

### 说明
- **比对用「未展开」值**：两侧均为 env 展开前的原始值，避免 `${VAR}` 展开名造成伪变更。
- `mod` 的 `MUTABILITY` 取自 `VarCollection`（`immutable`/`system`/`module`）；`sys` 的 `merged_vars.yml` 不序列化可变性，故目前多为 `module`。
- `gops prj diff` 暂不提供（prj 视角即逐系统的 `sys` 表）。

## [2.0.4] - 2026-09-29

### 重大变更
- **`gops sys` 拆分出运行时运维命令 `gops run`**：`download`/`install`/`uninstall`/`start`/`stop`/`status`/`diagnose` 从 `gops sys` 移到 **`gops run <cmd>`**（语义不变：按 `sys/sys_model.yml` 的 `kind` 分派到 gx 算子流或 docker compose）。`gops sys` 只保留**定义/交付/工件**：`new`/`update`/`localize`/`package`/`setting`/`check`。**破坏性**：`gops sys start` 之类需改为 `gops run start`。

## [2.0.3] - 2026-09-29

### 新增功能
- **`gops sys package` 支持 `sys-prj.yml` 的 `ignore:` 节**：列出打包时排除的路径（glob，相对系统根），**两种模式（默认 / `--full`）都生效**；匹配文件自身或其任一祖先目录（目录级模式如 `sys/*/mods` 排除整棵子树，`*` 不跨 `/`）。`deliver.lock` 不受影响、仍随包分发；空节不写入 `sys-prj.yml`。

### 变更
- **`gops sys package` 打包模式命名**：`--no-git` 更名 **`--full`**（含义不变：打当前目录全部，**含制品**与本地化产物，用于隔离网络交付）；默认仍只打 git 入库文件（**不含制品**）。旧名 `--no-git` 保留为隐藏别名（兼容）。

### Bug 修复
- **`gops sys localize` 默认重解析变量（#24）**：此前仅在 `sys/merged_vars.yml` **缺失**时才解析，改完 `sys/setting/vars.yml` 后 `localize` 不生效、`sys check` 还报 `[OK]`（它只比对 `.env` ↔ “已解析值 ⊕ 值文件”）。现在 `localize`（默认）**无条件先解析**（即 `gops sys update` 的解析阶段），改完 `sys/setting/vars.yml` 一条命令即生效；`--only` 语义不变（始终跳过解析，用现有 `merged_vars.yml`）。
- **`gops sys check` 暴露「定义比已解析结果更新」的陈旧**：输出 `[WARN] 变量定义... 比 sys/merged_vars.yml 更新`，提示运行 `gops sys localize`（仅提示，不改变退出码，避免 mtime 抖动误伤 CI）。

## [2.0.2] - 2026-09-29

### 新增功能
- **`gops self` 自升级**：与 `gx self` 对齐，新增 `gops self status|check|update|rollback`——`check` 查询通道最新版本（支持 `--json` 机器可读输出），`update` 下载/校验/安装并备份旧版本，`rollback` 回到备份版本。制品清单来自 `galaxio-labs/get` 的 `updates/gops` 通道（与 `inst-x.sh gops <channel>` 同一来源）；实现复用 `wp-self-update`。状态与备份存放于 `~/.galaxy/self_update/gops`，与 `gx` 的同名目录隔离。

### 改进优化
- **输出流对齐 `gx`**：版本横幅由 stdout 改到 **stderr**（并仅在人类可读模式打印，`self check --json` 时不打印）；错误报告 `report_error` 由 `println!` 改为 **`eprintln!`**——保证 `--json` 场景下 stdout 始终干净（失败时 stdout 为空、错误在 stderr）。
- **自升级健壮性**：`self rollback` 改为**临时文件 + rename 原子替换**安装目录下的可执行文件（直接就地覆盖运行中的二进制在 Linux 会 `ETXTBSY` 失败）；升级前先备份当前二进制（否则回滚无文件可恢复，这是相对 `gx self` 的修正）；同秒重复升级时备份 id 顺延避免覆盖；升级成功后只保留最近 5 个备份。

## [2.0.1] - 2026-09-29

### 变更
- 补发：发布时生成更新清单（`updates/gops/<channel>`），`inst-x.sh gops <alpha|beta|stable>` / `wp-inst install --source …/updates/gops` 可直接下载与更新

## [2.0.0] - 2026-09-29

> **破坏性变更**：系统模块布局重构，与 1.x 不完全兼容。新 gops 请配合 `galaxio-hub/ops-gxl` 的 `2.0` 通道使用。

### 新增功能
- **`gops sys package` 默认只含入库文件**：默认按 `git ls-files` 打包（等价 `git archive` 的“只含入库文件”，需在 git 仓库内运行），自然排除 `.gitignore` 忽略的产物（`sys/*/mods/`、`**/local`、`.env` 等）；`--no-git` 才打包当前目录全部（仅跳过 `.git/`）。`deliver.lock` 作为交付清单始终随包分发；符号链接按 `git archive` 语义**保留为链接**（不解引用），打包过程带进度显示。
- **`localize` 阶段扩展点（docker-compose）**：`gops sys localize` 写完 `.env` 后，若项目在 `_gal/work.gxl` 定义了同名 gx 流程 `localize` 则执行它（`gx run localize`），否则跳过——给 compose 系统补上「本地化后自定义动作」（渲染配置模板、生成证书/密钥等）的扩展点，与 gxl 系统统一。存在性判定用 `gx run --exists`（依赖 galaxy-flow ≥ 0.14），gx 缺失 / 版本过旧 / 无该流程时**静默跳过**，保持 compose「无需 gx」的默认；流程非零退出则 localize 整体失败。新增 `--no-flow` 跳过该阶段。

### 改进优化
- 合并后的配置（与 `.env` **完全一致**——同一份 evaled 字典、同样做过 `${}` 展开）作为**环境变量注入** gx 子进程，流程里可直接读 `${DOMAIN}` 等；注入时**保留** `PATH`/`HOME`/`LD_*`/`DYLD_*`/`GX*`/`GXL_*` 等关键变量不被覆盖；`-d 1` 会打印跳过原因（默认静默）。抽出 `project::env_pairs` 供注入复用；`gxl` 分派与阶段流程统一走同一 `gx` 调用入口（`run_gx_flow`）
- 示例 `knowlege/docker-compose` 增加 `_gal/work.gxl` 的 `localize` 流程做**验证**（幂等写 `web.conf`），并补 `tests/sys_localize_stage_test.rs` 端到端（真实 `gops` 二进制 + 假 `$HOME/bin/gx`）
- **host 模块制品缓存提升到 host 级**：`gops mod new` 的 host `download` 流程把制品下到 `${GXL_SHARED_LOCAL:../../local}/cache`（系统内 = `sys/<model>/local/cache`，同模型模块共享；独立运行 = `<repo>/local/cache`，可用 `GXL_SHARED_LOCAL` 覆盖）；缓存位于模块 `local/` 之外，不会被 `sys localize` 清掉。旧模块需重新生成 `workflows/operators.gxl` 才生效

### 重大变更
- **系统模块布局改为按模型分组**：`gops sys update` 现在把模块落到 `sys/<model>/mods/<mod>/`（原 `sys/mods/<mod>/<model>/`），同一部署目标的模块集中一处，便于按目标整体检视与打包。本地化产物路径（`sys/setting/list.yml` 的 `dst`）与脚手架 `.gitignore`（`sys/*/mods`）同步调整；系统与模块算子模板的 `extern` 统一指向权威仓库 `galaxio-hub/ops-gxl` 并改用 `2.0` 通道（`main` 保留给旧布局，供旧 gops 使用）。**兼容与迁移**：读取时优先新布局，缺失时回退旧布局 `sys/mods/<mod>/<model>`（存量项目不重跑 `update` 也能继续 `localize`）；`gops sys update` 成功后会自动清理遗留的 `sys/mods`，并给已有系统的 `.gitignore` 补上 `sys/*/mods`（旧规则只有 `sys/mods`，匹配不到新布局；幂等、只追加不改动其他行）；`sys/setting/list.yml` 里旧布局的 `dst` 会在 `update` 时**自动迁移**为 `sys/<model>/mods/<mod>/local/`（否则 localize 会把产物写回旧路径、重建 `sys/mods`）

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
