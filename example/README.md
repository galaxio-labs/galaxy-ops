# 示例与演示

本目录用于展示 `galaxy-ops` 的模块 / 系统 / 运维项目三层对象如何组织，以及"一个系统、多客户交付"的落地方式。

## 目录说明

```text
example/
├── dev-mac-env/         # 一个真实的运维项目示例（mac-devkit 系统，本地 tar.gz 导入）
├── knowlege/mysql/      # 示例知识/配置文件（my.cnf 等）
├── knowlege/docker-compose/  # docker-compose 完整示例（可直接运行的 nginx+postgres 系统）
├── mod-operators/       # 模块输出根目录（gops 运行时/测试写入，非示例内容）
├── sys-operators/       # 系统输出根目录（同上）
├── ops-projects/        # 运维项目输出根目录（同上）
└── demo/                # 端到端演示脚本与说明
    └── run-demo.sh
```

> 注意：`mod-operators/`、`sys-operators/`、`ops-projects/` 三个目录是 `src/const_vars.rs` 中约定的默认输出/测试写入目录，正常情况下应为空，请勿把它们当作示例。

## 核心概念回顾

```text
Module -> System -> Ops Project
```

- `Module`：最小可复用运维单元（`gops mod`）
- `System`：组合多个模块的交付单元（`gops sys`）
- `Ops Project`：面向具体客户/环境的落地项目（`gops prj`）

## 快速演示

```bash
# 1. 构建 gops（或直接使用已安装的 gops）
cargo build --bin gops

# 2. 进入一个空的工作目录
mkdir demo-work && cd demo-work

# 3. 创建一个示例模块（生成 postgresql 模块）
gops mod example

# 4. 创建系统（交互选择系统型号；脚本中可用 TEST_MODE=1 自动选择）
gops sys new --name web-stack

# 5. 解析变量并打包为交付产物（生成 ../web-stack-0.1.0.tar.gz）
cd web-stack
gops sys package
cd ..

# 6. 创建运维项目并导入同一个系统（客户 A / B）
gops prj new --name customer-a
cd customer-a
gops prj import --path ../web-stack-0.1.0.tar.gz
cd ..
gops prj new --name customer-b
cd customer-b
gops prj import --path ../web-stack-0.1.0.tar.gz
cd ..
```

详细步骤与生成结构见下文；演示脚本见 [demo/run-demo.sh](./demo/run-demo.sh)。

## 生成结构速览

### Module（`gops mod example` / `gops mod new --name <name>`）

```text
<module>/
├── mod-prj.yml              # 模块项目配置（test_envs）
├── version.txt
├── _gal/                    # GXL 项目文件
│   ├── work.gxl
│   ├── adm.gxl
│   └── project.toml
└── mod/
    ├── arm-mac14-host/      # 按 ModelSTD 拆分的目标模型
    │   ├── vars.yml         # 变量定义（immutable / system / module）
    │   ├── spec/
    │   │   ├── artifact.yml # 构件地址
    │   │   └── depends.yml  # 依赖
    │   ├── workflows/operators.gxl
    │   └── _gal/work.gxl
    └── x86-ubt22-k8s/
        └── ...
```

### System（`gops sys new --name <name>`）

```text
<system>/
├── sys-prj.yml              # 系统项目配置（test_envs）
├── version.txt
├── _gal/                    # GXL 项目文件
│   ├── work.gxl
│   ├── adm.gxl
│   └── project.toml
├── values/                  # 值文件目录（本地化输入）
└── sys/
    ├── sys_model.yml        # 系统定义（name / model 可选 / kind / vender）
    ├── mod_list.yml         # 模块列表（引用外部模块）
    ├── setting/             # 系统设置（vars.yml / list.yml）
    └── workflows/operators.gxl
```

### Ops Project（`gops prj new --name <name>`）

```text
<project>/
├── ops-prj.yml              # 运维项目 manifest（name + work_envs + sys_models 已导入系统列表）
├── version.txt
└── _gal/                    # GXL 项目文件
    ├── work.gxl
    └── adm.gxl
```

## "一个系统、多客户"的差异化方式

差异不写死在系统定义里，而是放在各项目的值文件里：

```text
System/Module Spec（共享定义）
  + Customer Values（客户差异：域名、IP、端口、资源规格、开关等）
  -> Localized Output（各客户独立的最终配置）
```

- `gops prj import` 把同一个系统导入多个项目
- 每个项目在 `values/<system>/` 下维护客户值：**只需写要覆盖的项**，其余取系统默认值（`sys/merged_vars.yml` 的 `system:` 段）；`value.yml` 作为额外覆盖层
- `gops sys localize` / `gops mod localize` 渲染出本地化产物
- 在项目内的系统目录执行 `gops sys localize` 时，会按上层 `ops-prj.yml` 直接使用 `values/<system>/`，不依赖 `<sys>/values` 符号链接是否完整

## docker-compose 的处理方式

推荐**不把 compose 文件当 galaxy-ops 模板**，而是用 Docker Compose 原生 `${VAR}`，让 `gops sys localize` 把系统值导出成系统根目录的 `.env`：

```text
System（共享定义）              Ops Project（客户差异）
  sys/docker-compose.yaml 用 ${VAR}（含 ${SEC_xxx} 密钥占位）
  sys/setting/vars.yml（默认）    values/value.yml（客户覆盖，版本化）
                                    ↓  gops sys localize
                                    .env（仅非密钥配置，供 compose 消费，位于系统根）

密钥（密码/token）→ 运行时由 gops run start 从 ~/.galaxy/sec_value.yml 注入子进程环境，不落盘
```

**规则**：`.env = 系统默认值（merged_vars.yml）+ values/sys_value.yml + values/value.yml`（后两者只需写要覆盖的项，仅非密钥配置）；密钥用 `${SEC_xxx}` 占位，不写进 `.env`。

理由：compose 文件保持“合法”，可随时 `docker compose config` 校验；**版本化的是值文件（配置），`.env` 只是非密钥配置的生成产物**；密钥走 `~/.galaxy/sec_value.yml`（`orion-sec` 运行时注入），不进版本库、不落盘。

`gops sys new` 现在**默认生成**一个最小 `sys/docker-compose.yaml`（单 `app` 服务 + `${SERVICE_IMAGE}`/`${SERVICE_PORT}`/`${REPLICAS}`）和对应的 `sys/setting/vars.yml` 变量段，新系统开箱即带 compose 能力。

**文件位置可声明**：compose 属于「系统定义」，默认放在 `sys/docker-compose.yaml`；`gops sys` 按 `sys/{compose,docker-compose}.{yaml,yml}` → `<root>/{compose,docker-compose}.{yaml,yml}` 查找（`sys/` 优先）。无论放在哪，compose 的**项目目录都锚定在系统根**——项目名 = 系统根目录名、相对挂载与 `.env` 都相对系统根，`${VAR}` 的相对挂载写法不需要改。旧仓（compose 在根）不改动也能继续跑。

完整示例见 [knowlege/docker-compose](./knowlege/docker-compose/)——它是一个**可直接运行的完整系统**（nginx + postgres + 卷 + 密钥占位），也是「纯 docker-compose 系统」的最小形态：

```text
knowlege/docker-compose/
├── sys-prj.yml              # 系统根配置（test_envs）
├── version.txt
├── _gal/
│   └── work.gxl             # 可选：定义 localize 阶段流程（扩展点），见下
├── sys/
│   ├── sys_model.yml        # name: web-stack / kind: docker-compose
│   ├── docker-compose.yaml  # 共享定义，用 ${NGINX_TAG}/${HTTP_PORT}/${SEC_DB_PASSWORD} 占位
│   ├── setting/vars.yml     # 非密钥系统变量（system 段）
│   └── merged_vars.yml      # 聚合变量（sys update 生成，需入库）
└── values/
    └── value.yml            # 客户覆盖（版本化）：只写要覆盖的项
```

- `~/.galaxy/sec_value.yml`：全局密钥文件（`db_password` / `postgres_password` 等，运行时注入为 `${SEC_*}`），不随项目提交
- `_gal/work.gxl`：**可选**。这里定义了一个 `localize` 流程做**阶段扩展点**验证（见下）；不定义也行，缺了它纯 compose 系统照样跑
- **没有 `sys/mod_list.yml`、`sys/workflows/`、`sys/setting/list.yml`**：纯 docker-compose 系统不依赖 GXL / gx（`_gal/work.gxl` 只用于可选的阶段流程），这些缺失时分别按空处理
- `sys_model.yml` 的 `name`（`web-stack`）是 gops 的**系统名**（决定交付包名 `web-stack-0.1.0.tar.gz`）；docker 的项目名则取自**系统根目录名**（`docker-compose`）——两者用途不同，不必一致

直接体验（含客户化；`values/value.yml` 已内置一份只写差异的覆盖样例）：

```bash
cd example/knowlege/docker-compose
# 可选：把全局密钥写到 ~/.galaxy/sec_value.yml（不在项目里）
gops sys localize                # 合并系统默认值 + values/value.yml（客户覆盖）→ 系统根的 .env
                                 # 随后若定义了同名 gx 流程 localize，则顺带执行它（本示例有）
cat .env                         # NGINX_TAG=1.27-alpine / HTTP_PORT=8081 / ...（无密钥明文）
cat web.conf                     # server { listen 8081; }：阶段流程读到刚合并的 HTTP_PORT（幂等，重复 localize 不重写）
```

## 可选阶段流程（`localize` 扩展点）

`gops sys localize` 写完 `.env` 后，**若项目在 `_gal/work.gxl` 定义了同名 gx 流程 `localize` 就执行它**，否则跳过——compose 系统由此获得「本地化后自定义动作」（渲染配置模板、生成证书/密钥…）的扩展点，与 gxl 系统统一。要点：

- **声明只有一处**：流程写在 `_gal/work.gxl`，不在 `sys_model.yml` 里再声明；
- **执行命令**：`gx run -e default -d <debug> localize`（故流程需能用 `env default` 运行；本示例的 `_gal/work.gxl` 有 `env default`）；
- **值传递**：合并后的配置（与 `.env` **完全一致**，含 `${}` 展开）作为**环境变量注入** gx 子进程，流程里可直接读 `${DOMAIN}` 等；
- **可选依赖**：需要 gx（galaxy-flow）≥ 0.14；gx 缺失 / 版本过旧 / 没有该流程 → **静默跳过**（保持「compose 无需 gx」；`-d 1` 会打印跳过原因）；
- **失败即失败**：流程非零退出 → `gops sys localize` 整体失败；`gops sys localize --no-flow` 可跳过该阶段；
- 机制与阶段名无关（当前只接 `localize`、只对 docker-compose 生效）。

本示例的 `_gal/work.gxl` 就用它**幂等地**写了一份 `web.conf`（`test -f web.conf || …`，存在即跳过）——这正是「证书/密钥必须生成一次即稳定」的写法。详见 [`../src/system/README.md`](../src/system/README.md)。

该系统的 `sys/sys_model.yml` 已标记 `kind: docker-compose`，因此 `gops run` 的部署命令会自动映射到 `docker compose`，无需安装 gx：

```bash
gops run diagnose   # = docker compose config（校验并展示解析后的 compose）
gops run start      # = docker compose up -d
gops run status     # = docker compose ps
gops run stop       # = docker compose stop
gops run uninstall  # = docker compose down
```

`.env` 由 `export_env_file` 生成（`src/project.rs`）：键大写、简单标量原样输出、含空格/特殊字符的值用双引号包裹，嵌套对象/列表序列化为 JSON。密钥不落盘：`gops run start` 通过 `orion_sec::load_sec_dict()` 读 `~/.galaxy/sec_value.yml`，以 `SEC_*` 环境变量注入 `docker compose` 子进程（`app/gops/commands/run_cmd.rs`）。

## 关键流程与前置条件（实测）

以下结论基于对当前 `gops` 的实际运行验证：

1. **`gops mod example` / `mod new` / `sys new` / `prj new`**：可正常生成骨架，无网络依赖。

2. **`gops sys new` 需要交互选择系统型号**（`dialoguer::Select`）。脚本化时可通过环境变量 `TEST_MODE=1` 自动选择第一个支持型号。

3. **`gops mod update` / `sys update`**：负责下载依赖、解析变量（生成 `sys/merged_vars.yml`）并初始化值文件。生成的 `values/sys_value.yml` 是**注释模板**（可用变量已注释，默认不生效）——取消注释需要覆盖的项即可，其余取系统默认值。该步骤**需要网络**（下载 `mod_list.yml` 指向的仓库、`artifact.yml` 指向的构件）。

4. **`gops mod localize` / `sys localize`**：`sys localize` 在系统变量未解析时会**自动先 `update`**，再渲染 `.env`（= `sys/merged_vars.yml` 默认值 ⊕ `values/sys_value.yml` ⊕ `values/value.yml`，后两者可选、可只写要覆盖的项）；`--only` 跳过 update。`mod localize` 仍依赖先 `mod update`。

5. **`gops prj import --path <path>`**：`path` 必须是一个**打包产物**（本地 `.tar.gz` 或 git/http 地址），**不是裸目录**。推荐直接用 `gops sys package` 生成（见上），它会先执行 `update` 解析变量再打包：

   ```bash
   cd web-stack && gops sys package   # 生成 ../web-stack-0.1.0.tar.gz
   gops prj import --path ../web-stack-0.1.0.tar.gz
   ```

6. **`gops run download/install/start/stop/status/diagnose`**：按 `sys/sys_model.yml` 的 `kind` 字段分派：
   - `kind: gxl`（默认，兼容旧系统）：委托给外部 `gx` 执行（`$HOME/bin/gx`，即 `gx run -e <env> -d <n> <cmd>`），要求 `gx >= 0.13.0`。
   - `kind: docker-compose`：直接映射到 `docker compose` 子命令（`download`→`pull`、`install`→`create`、`start`→`up -d`、`stop`→`stop`、`uninstall`→`down`、`status`→`ps`、`diagnose`→`config`），无需安装 `gx`。

## 当前已知问题（已知限制）

- **`prj import` 要求系统已解析变量**：导入流程依赖 `sys/merged_vars.yml`（由 `gops sys update` 解析模块并合并变量后生成）。该文件需要入库（不 gitignore），保证源码、交付包与导入期望一致；若导入一个从未 `update` 过、且未提交 `merged_vars.yml` 的系统，会明确报错：`系统变量未解析：缺少 .../sys/merged_vars.yml。请先在该系统上执行 gops sys update 解析变量`。推荐直接使用 `gops sys package`（内部先 update 再打包）。
- **执行链依赖外部环境**：工作流模板引用了 `galaxy-operators/*.git` 等外部仓库，`update` / 执行需网络，离线不可用。
- 已修复的问题：`prj import` 的 `values` 符号链接冲突、`convert_addr` 对裸目录的 panic、`mod new` 硬编码 postgresql 构件地址、`merged_vars.yml` 缺失时的晦涩报错（现改为清晰可操作的提示），并新增 `gops sys package` 固化“先 update 再打包”的交付约束。

## 参考

- 项目总览：[../README.md](../README.md)、[../PROJECT_OVERVIEW.md](../PROJECT_OVERVIEW.md)
- 源码结构：[../src/README.md](../src/README.md)
- Module / System / Ops Project 文档：[../src/module/README.md](../src/module/README.md)、[../src/system/README.md](../src/system/README.md)、[../src/ops_prj/README.md](../src/ops_prj/README.md)
