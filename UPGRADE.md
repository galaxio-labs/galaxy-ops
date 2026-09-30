# 2.0 升级指南

面向**使用 `gops` 做交付 / 运维的人**：说明 2.0 的主要价值（按重要性排序），以及升级时要做的操作。
（库 / 开发者视角的依赖与 API 变更见 [API-MIGRATION.md](./API-MIGRATION.md)。）

> 一句话：2.0 把 `gops` 从「一包系统脚本」升级为 **定义 / 交付 / 运行三段清晰**、**按部署目标组织**、**自带配置可见性与自升级**的交付工具。

## 2.0 主要价值（按重要性排序）

### 1. `gops run` 与 `gops sys` 分离：命令模型清晰化

`gops` 的两半职责被明确切开：

- `gops sys`：系统的**定义 / 交付 / 工件** —— `new` / `update` / `localize` / `package` / `setting` / `check` / `diff`。
- `gops run`：在目标环境里的**运行时运维** —— `download` / `install` / `uninstall` / `start` / `stop` / `status` / `diagnose`。

**价值**：`sys` 是「把系统做出来、发出去」（研发/交付侧），`run` 是「把系统装起来、跑起来」（现场侧）。两者生命周期不同，分开后脚本与心智都不再含糊。

```bash
gops sys start     # 旧

gops run start     # 新
```

### 2. 模块布局按模型分组：一套定义，多部署目标并存

`gops sys update` 现在把模块落到 **`sys/<model>/mods/<mod>/`**（旧：`sys/mods/<mod>/<model>/`）——**同一部署目标的模块集中一处**，host / k8s 等多目标可并存。

**价值**：目录结构直接映射「按部署目标（模型）分组」；找模块、共享 host 级缓存（`sys/<model>/local/cache/`）、按目标打包都更自然。

**不用手动搬**：读取优先新布局、缺失回退旧布局，存量系统不重跑 `update` 也能继续 `localize`。

### 3. 支持 docker-compose：纯 compose 系统也纳入统一管理

`gops sys new --kind docker-compose` 可建**纯 compose 系统**；此时 `gops run start/stop/status/...` 自动映射到 `docker compose`，`localize` 生成 `.env`，密钥用 `${SEC_xxx}` 占位、运行时从 `~/.galaxy/sec_value.yml` 注入。

**价值**：**不写一行 GXL**，也能用同一套 `sys` / `run` 界面管理「下载 / 安装 / 启停 / 状态 / 诊断」。

**位置变化**：compose 文件默认改放 **`sys/docker-compose.yaml`**（compose 属于「系统定义」）。旧系统把 compose 放在系统根目录照常可用；如需内收：`git mv docker-compose.yml sys/docker-compose.yaml`。`.env` 位置不变，仍在系统根。

### 4. `gops sys diff`：配置可见性

只读比对「初始默认值」与「客户覆盖后的生效值」，逐键列出 `KEY / INITIAL / EFFECTIVE / ORIGIN / MUTABILITY / STATE`，并按**系统层 / 模块层**分组；`localize` 结束时也会打印同一张表。

**价值**：直接回答「**哪些值被覆盖了、被哪一层覆盖**」。这正是「一份系统定义、多客户交付」能放心**快速重部署而不出错**的前提。

```bash
gops sys diff
gops sys diff --json    # 机器可读
```

### 5. `gops sys package` 增强：交付物干净可控

- **默认只打 git 入库文件**：自动排除 `sys/*/mods/`、`**/local`、`.env` 等产物——适合入库 / 交付源码。
- **`--full`**（旧名 `--no-git` 保留为隐藏别名）：打当前目录全部，含制品与本地化产物——适合**隔离网络**的一体化交付。
- **`sys-prj.yml` 支持 `ignore:` 节**：两种模式都生效，由系统自己声明要排除什么。

### 6. `gops self update`：自升级闭环

```bash
gops self status
gops self check --channel <stable|alpha|beta>
gops self update --channel <channel> --yes
gops self rollback
```

**价值**：升级 / 回滚不再依赖重跑安装脚本，也不必另装工具。

### 7. `gops self skill`：配套 skills 也由 gops 管理

```bash
gops self skill install           # 默认装 galaxio-labs/gops-skills
gops self skill list
gops self skill install --source galaxio-labs/gx-skills   # 也可装 gx 的 skills
```

**价值**：把「工具 + 其配套知识资产」的获取收敛到 `gops self` 一处。

## 升级要做的事（checklist）

1. **改名**：脚本 / CI 里 `gops sys <run-cmd>` → `gops run <cmd>`；`sys package --no-git` → `--full`（不改也能用）。
2. **迁移存量系统**：在每个系统目录跑一次 `gops sys update`，它会（幂等）：

   - 清理遗留 `sys/mods/`；
   - 给 `.gitignore` 补 `sys/*/mods`；
   - 把 `sys/setting/list.yml` 里旧布局的 `dst` 迁到 `sys/<model>/mods/<mod>/local/`。

3. **核对配置**：`gops sys diff` / `gops sys localize` 复核生效值；`gops sys check` 应干净（退出 0）。
4. **（docker-compose 系统）** 视需要把 compose 内收到 `sys/docker-compose.yaml`。
5. **算子模板**：系统 / 模块脚手架的 `extern` 需指向 `galaxio-hub/ops-gxl` 的 **`2.0`** 通道（`main` 只服务旧布局 / 旧 gops）。

## 升级后要注意的行为变化

- **值文件键改为大小写不敏感**：`values/*.yml` / `values/<mod>/mod_value.yml` 加载时统一按大写处理。此前写成小写、以为「没生效」的覆盖，现在会**真的生效**——升级后请用 `gops sys diff` 复核一遍。
- **`sys check` 会提示定义陈旧**：`sys/setting/vars.yml` 等定义比已解析结果新时给 `[WARN]`（不改变退出码）。
- **下载不再留半包**：中断的 `gops run download` 不再在目标位置留下不完整文件（由旧版遗留的截断文件仍会被复用，可删除或强制重下）。
