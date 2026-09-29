# system 模块说明

`src/system/` 对应 `gops sys` 这一组能力。

## 目标

系统层负责把多个模块组织成一个可操作、可本地化、可交付的系统对象。

它解决的是：

- 系统骨架如何初始化
- 系统模型如何维护
- 模块列表如何维护
- 系统设置如何初始化
- 系统级下载、安装、启动、停止、状态、诊断如何组织

## 当前目录

```text
src/system/
├── conf.rs
├── init.rs
├── mod_list.rs
├── operator.rs
├── path.rs
├── refs.rs
├── spec.rs
├── setting/
│   ├── export.rs
│   ├── localize.rs
│   ├── mod.rs
│   ├── sys.rs
│   └── templatize.rs
└── README.md
```

## 主要文件

### `operator.rs`

系统对象入口。`gops sys new`、`update`、`localize` 以及各类系统操作命令都会落到这里。

### `conf.rs`

系统配置对象 `SysConf`（序列化到 `sys-prj.yml`），目前只含 `test_envs` 依赖集。部署类型 `SysKind`（`gxl` / `docker-compose`）也定义在这里。

### `spec.rs`

系统定义（`SysDefine`，序列化到 `sys/sys_model.yml`）与初始化模板。`SysDefine` 含 `name` / `model`（可选，纯 compose 无型号）/ `kind` / `vender`；`kind` 决定 `sys` 命令分派到 gx 还是 docker compose。

### `mod_list.rs`

系统模块列表相关逻辑。`sys/mod_list.yml` 为**可选**：缺失时按空模块列表处理（例如纯 docker-compose 系统，不组合 galaxy-ops 模块）。

### `path.rs`

系统路径组织，负责定位：

- `sys-prj.yml`
- `sys/sys_model.yml`
- `sys/mod_list.yml`（可选）
- `sys/setting/...`

### `setting/`

系统设置、本地化和模板化相关逻辑。

## 与 CLI 的对应

```text
# gops sys —— 定义 / 交付 / 工件
gops sys new [--kind gxl|docker-compose]
gops sys update
gops sys package
gops sys localize
gops sys setting
gops sys check
gops sys diff

# gops run —— 运行时运维
gops run download/install/uninstall/start/stop/status/diagnose
```

`gops run download/install/uninstall/start/stop/status/diagnose` 按 `sys/sys_model.yml` 的 `kind` 分派：

- `gxl`（默认）：委托给外部 `gx`（`$HOME/bin/gx`，即 `gx run <cmd>`）。
- `docker-compose`：映射到 `docker compose`（`download`→`pull`、`install`→`create`、`start`→`up -d`、`stop`→`stop`、`uninstall`→`down`、`status`→`ps`、`diagnose`→`config`）。

## 值变更表（`sys diff` / `localize` 末尾）

与 `check` 关注的“`.env` 漂移”不同，`diff` 回答的是“**哪些值被覆盖、被哪一层覆盖**”：

```text
gops sys diff [--json]
```

比对「初始层」（`merged_vars.yml` 的系统默认值，`origin=sys-defaults`）与「生效层」（⊕ `values/sys_value.yml`(`sys-setting`) ⊕ `values/value.yml`(`customer`)，均为**未展开**值，避免 `${VAR}` 带来伪变更），逐键列出：

| 列 | 含义 |
|---|---|
| `KEY` | 变量名 |
| `INITIAL` | 初始层取值（`-` 表示初始层无此键） |
| `EFFECTIVE` | 生效值 |
| `ORIGIN` | 生效值来自哪一层（`sys-defaults` / `sys-setting` / `customer`） |
| `MUTABILITY` | 生效值的可变性（`merged_vars.yml` 不序列化可变性，故目前多为 `module`） |
| `STATE` | `same` / `changed` / `added` / `removed`（表格只列非 `same` 行） |

`localize` 结束时也会打印同一张表（无覆盖时打 `[OK] 值无覆盖`）。`gops mod diff` 同理，但按模型分组，初始层为 `mod/<model>/vars.yml`（`mod-default`），来源另有 `mod-setting`（`mod_value.yml`）与 `global`。

## 文件变更表（`localize` 末尾）

`localize` 除写值与 `.env` 外，还会**渲染文件**（sys setting 模板 `src` → `dst`；mod 为 `spec/` → `local/`）。落地后打印本次**新增 / 替换**的文件表：

```text
文件变更 (2 项) @ arm-mac14-host/local:
FILE          STATE
----------------------
artifact.yml  created
depends.yml   replaced
```

- 用 localize **前后内容指纹（sha256）比对**：清空输出树再重建不会把内容未变的文件误报为变更。
- 只报 `created` / `replaced`；**删除不报**（重建输出树时属常态）。
- 需前后基线，故只在 `localize` 呈现；`diff` 仍只呈现值。

## 可选阶段流程（`localize` 扩展点）

`kind: docker-compose` 的系统在 `gops sys localize` 写完 `.env` 之后，**若项目定义了同名 gx 流程 `localize` 则执行它**（`gx run localize`）；否则跳过。这样 compose 系统也有了「本地化后自定义动作」的扩展点（渲染配置模板、生成证书/密钥等），与 `gxl` 系统的扩展点统一。

- **声明只有一处**：流程写在 `_gal/work.gxl`，不在 `sys_model.yml` 里再声明。
- **判定确定**：用 `gx run --exists localize`（galaxy-flow ≥ 0.14）判定存在性，不靠试跑猜退出码。
- **执行命令**：`gx run -e default -d <debug> localize`——因此流程需能用 `env default` 运行（conf 里声明 `env default`；脚手架生成的 `_gal/work.gxl` 已包含）。
- **可选依赖**：gx 未安装 / 版本过旧 / 项目没有该流程 → **静默跳过**（compose 系统照常可用）；`-d 1` 会打印**跳过原因**（如 conf 解析错误、gx 过旧），便于排障。
- **顺序**：gops 先写完 `.env`，再跑流程。
- **值传递**：合并后的配置（与 `.env` **完全一致**——同一批键、同样做过 `${}` 展开）会作为**环境变量注入** gx 子进程，流程里可直接读 `${DOMAIN}` 等；注入时会**保留** `PATH`/`HOME`/`SHELL`/`LD_*`/`DYLD_*`/`GX*`/`GXL_*` 等关键变量不被覆盖（避免破坏 gx 自身或其 shell）。
- **失败即失败**：流程非零退出 → `sys localize` 整体失败。
- **跳过开关**：`gops sys localize --no-flow`。

> 机制与阶段名无关（当前只接 `localize`、且只对 `docker-compose` 生效）；后续要对 `install`/`start`/… 或 `gxl` 放开，只需在 `SysCommandHandler::run_stage_flow` 的调用点接入。

## 密钥处理

纯 docker-compose 系统的密钥**不落盘、不进 `.env`**，用 `${SEC_xxx}` 占位 + 运行时注入：

```text
sys/docker-compose.yaml   →  ${SEC_DB_PASSWORD}（原生占位；位于 sys/ 或旧布局的系统根）
sys localize             →  .env = 系统默认值（merged_vars.yml）+ values/sys_value.yml + values/value.yml 覆盖（仅非密钥，后两者可只写要覆盖的项）
sys start (compose)      →  orion_sec::load_sec_dict()
                            读 ~/.galaxy/sec_value.yml（或 ./.galaxy/sec_value.yml）
                            → SEC_DB_PASSWORD=<明文> 注入 docker compose 子进程环境
```

- 密钥文件：`~/.galaxy/sec_value.yml`（YAML），key 会归一化为大写并加 `SEC_` 前缀（如 `db_password` → `SEC_DB_PASSWORD`）。
- 加载：`orion_sec::load_sec_dict()`（`orion-sec` 依赖），文件缺失时返回空、不报错。
- 密钥只存在于 `docker compose` 那个瞬时子进程的环境里，进程结束即消失。
- `sys diagnose`（`docker compose config`）会打印解析后的配置，因此**注入掩码值 `********` 而非明文**，避免泄露；输出里 `${SEC_xxx}` 显示为 `********`。

## 输出对象

当前系统层最终管理的是这种对象结构：

```text
<system-root>/
├── sys-prj.yml             # 系统配置（test_envs 依赖集）
├── version.txt
├── _gal/
├── values/                 # 值文件目录（sys_value.yml 为注释模板：取消注释要覆盖的项即可；localize 后生成 .env，仅非密钥配置）
└── sys/
    ├── sys_model.yml       # 系统定义（name / model 可选 / kind / vender；kind 决定命令分派）
    ├── docker-compose.yaml # 纯 docker-compose 系统的 compose 定义（sys new 默认生成，用 ${VAR} 占位）
    ├── mod_list.yml        # 可选，缺失时视为空模块列表
    ├── merged_vars.yml    # 聚合变量（sys update 生成：模块变量 ⊕ 系统变量，需入库）
    ├── setting/
    │   ├── vars.yml        # system 段变量定义（源，版本化）
    │   └── list.yml        # 可选，按模块的本地化列表；纯 docker-compose 系统可省略
    └── workflows/          # 可选，GXL 运维流；纯 docker-compose 系统可省略
```

compose 属于「系统定义」，默认随 `sys/` 一起内收。`gops sys` 按
`sys/{compose,docker-compose}.{yaml,yml}` → `<root>/{compose,docker-compose}.{yaml,yml}`
查找（`sys/` 优先，`compose.*` 与 docker 自身的发现优先级一致）。**无论文件放在哪，
项目目录都锚定在系统根**：项目名 = 系统根目录名，相对挂载与 `.env` 都相对系统根，
所以 compose 里的相对挂载写法不需要改。旧布局（compose 放在系统根）不改动也能继续跑。

## 关系

系统是模块之上的组合层，也是运维项目导入的来源对象：

```text
module -> system -> ops_prj
```
