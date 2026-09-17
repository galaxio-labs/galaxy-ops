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
- 每个项目在 `values/<system>/` 下维护自己的 `value.yml`
- `gops sys localize` / `gops mod localize` 渲染出本地化产物

## docker-compose.yml 的处理方式

推荐**不把 compose 文件当 galaxy-ops 模板**，而是用 Docker Compose 原生 `${VAR}`，让 `gops sys localize` 把系统值导出成同目录的 `.env`：

```text
System（共享定义）              Ops Project（客户差异）
  docker-compose.yml 用 ${VAR}（含 ${SEC_xxx} 密钥占位）
  sys/setting/vars.yml（默认）    values/value.yml（客户覆盖，版本化）
                                    ↓  gops sys localize
                                    .env（仅非密钥配置，供 compose 消费）

密钥（密码/token）→ 运行时由 gops sys start 从 ~/.galaxy/sec_value.yml 注入子进程环境，不落盘
```

**规则**：`.env = vars.yml 默认值 + values/value.yml 客户覆盖（仅非密钥配置）`；密钥用 `${SEC_xxx}` 占位，不写进 `.env`。

理由：compose 文件保持“合法”，可随时 `docker compose config` 校验；**版本化的是值文件（配置），`.env` 只是非密钥配置的生成产物**；密钥走 `~/.galaxy/sec_value.yml`（`orion-sec` 运行时注入），不进版本库、不落盘。

`gops sys new` 现在**默认生成**一个最小 `docker-compose.yml`（单 `app` 服务 + `${SERVICE_IMAGE}`/`${SERVICE_PORT}`/`${REPLICAS}`）和对应的 `sys/setting/vars.yml` 变量段，新系统开箱即带 compose 能力。

完整示例见 [knowlege/docker-compose](./knowlege/docker-compose/)——它是一个**可直接运行的完整系统**（nginx + postgres + 卷 + 密钥占位）：

- `docker-compose.yml`：共享定义，用 `${NGINX_TAG}`、`${HTTP_PORT}`、`${SEC_DB_PASSWORD}` 等占位
- `sys/setting/vars.yml`：非密钥配置的 `system:` 变量段
- `~/.galaxy/sec_value.yml`：全局密钥文件（`db_password` / `postgres_password` 等，运行时注入为 `${SEC_*}`），不随项目提交
- **没有 `sys/mod_list.yml`、`sys/workflows/`、`sys/setting/list.yml`**：这是纯 docker-compose 系统（三者缺失时分别按空处理）

直接体验（含客户化）：

```bash
cd example/knowlege/docker-compose
TEST_MODE=1 gops sys update      # 解析变量（生成 effective_vars.yml + values）
# 密钥写到全局密钥文件（不在项目里）：
#   ~/.galaxy/sec_value.yml
#     db_password: "xxx"
#     postgres_password: "yyy"
mkdir -p values && printf 'HTTP_PORT: 8081\nREPLICAS: 5\n' > values/value.yml  # 客户覆盖
gops sys localize                # 生成 .env = 默认 + 客户覆盖（不含密钥）
cat .env                         # HTTP_PORT=8081 / REPLICAS=5 / ...（无密钥明文）
```

该系统的 `sys/sys_model.yml` 已标记 `kind: docker-compose`，因此 `gops sys` 的部署命令会自动映射到 `docker compose`，无需安装 gflow：

```bash
gops sys diagnose   # = docker compose config（校验并展示解析后的 compose）
gops sys start      # = docker compose up -d
gops sys status     # = docker compose ps
gops sys stop       # = docker compose stop
gops sys uninstall  # = docker compose down
```

`.env` 由 `export_env_file` 生成（`src/project.rs`）：键大写、简单标量原样输出、含空格/特殊字符的值用双引号包裹，嵌套对象/列表序列化为 JSON。密钥不落盘：`gops sys start` 通过 `orion_sec::load_sec_dict()` 读 `~/.galaxy/sec_value.yml`，以 `SEC_*` 环境变量注入 `docker compose` 子进程（`app/gops/commands/sys_cmd.rs`）。

## 关键流程与前置条件（实测）

以下结论基于对当前 `gops` 的实际运行验证：

1. **`gops mod example` / `mod new` / `sys new` / `prj new`**：可正常生成骨架，无网络依赖。

2. **`gops sys new` 需要交互选择系统型号**（`dialoguer::Select`）。脚本化时可通过环境变量 `TEST_MODE=1` 自动选择第一个支持型号。

3. **`gops mod update` / `sys update`**：负责下载依赖并调用 `init_setting_value` 初始化值文件（`values/<model>/sys_value.yml`、`mod_value.yml` 等）。该步骤**需要网络**（下载 `mod_list.yml` 指向的仓库、`artifact.yml` 指向的构件）。

4. **`gops mod localize` / `sys localize`**：依赖上一步生成的值文件。如果值文件未初始化（未先执行 `update`），会报 `read file .../values/.../sys_value.yml` 错误。即本地化的正确顺序是 **先 `update`，再 `localize`**。

5. **`gops prj import --path <path>`**：`path` 必须是一个**打包产物**（本地 `.tar.gz` 或 git/http 地址），**不是裸目录**。推荐直接用 `gops sys package` 生成（见上），它会先执行 `update` 解析变量再打包：

   ```bash
   cd web-stack && gops sys package   # 生成 ../web-stack-0.1.0.tar.gz
   gops prj import --path ../web-stack-0.1.0.tar.gz
   ```

6. **`gops sys download/install/start/stop/status/diagnose`**：按 `sys/sys_model.yml` 的 `kind` 字段分派：
   - `kind: gxl`（默认，兼容旧系统）：委托给外部 `gflow` 二进制执行（`$HOME/bin/gflow`），要求 `gflow >= 0.11.2`。
   - `kind: docker-compose`：直接映射到 `docker compose` 子命令（`download`→`pull`、`install`→`create`、`start`→`up -d`、`stop`→`stop`、`uninstall`→`down`、`status`→`ps`、`diagnose`→`config`），无需安装 `gflow`。

## 当前已知问题（已知限制）

- **`prj import` 要求系统已解析变量**：导入流程依赖 `sys/effective_vars.yml`（由 `gops sys update` 解析模块并合并变量后生成）。该文件需要入库（不 gitignore），保证源码、交付包与导入期望一致；若导入一个从未 `update` 过、且未提交 `effective_vars.yml` 的系统，会明确报错：`系统变量未解析：缺少 .../sys/effective_vars.yml。请先在该系统上执行 gops sys update 解析变量，再打包导入`。推荐直接使用 `gops sys package`（内部先 update 再打包）。
- **执行链依赖外部环境**：工作流模板引用了 `galaxy-operators/*.git` 等外部仓库，`update` / 执行需网络，离线不可用。
- 已修复的问题：`prj import` 的 `values` 符号链接冲突、`convert_addr` 对裸目录的 panic、`mod new` 硬编码 postgresql 构件地址、`effective_vars.yml` 缺失时的晦涩报错（现改为清晰可操作的提示），并新增 `gops sys package` 固化“先 update 再打包”的交付约束。

## 参考

- 项目总览：[../README.md](../README.md)、[../PROJECT_OVERVIEW.md](../PROJECT_OVERVIEW.md)
- 源码结构：[../src/README.md](../src/README.md)
- Module / System / Ops Project 文档：[../src/module/README.md](../src/module/README.md)、[../src/system/README.md](../src/system/README.md)、[../src/ops_prj/README.md](../src/ops_prj/README.md)
