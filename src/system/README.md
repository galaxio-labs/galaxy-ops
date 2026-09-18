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
gops sys new [--kind gxl|docker-compose]
gops sys update
gops sys package
gops sys localize
gops sys setting
gops sys download/install/uninstall/start/stop/status/diagnose
```

`download/install/uninstall/start/stop/status/diagnose` 按 `sys/sys_model.yml` 的 `kind` 分派：

- `gxl`（默认）：委托给外部 `gx`（`$HOME/bin/gx`，即 `gx run <cmd>`）。
- `docker-compose`：映射到 `docker compose`（`download`→`pull`、`install`→`create`、`start`→`up -d`、`stop`→`stop`、`uninstall`→`down`、`status`→`ps`、`diagnose`→`config`）。

## 密钥处理

纯 docker-compose 系统的密钥**不落盘、不进 `.env`**，用 `${SEC_xxx}` 占位 + 运行时注入：

```text
docker-compose.yml        →  ${SEC_DB_PASSWORD}（原生占位）
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
├── docker-compose.yml      # 系统级 docker-compose 定义（sys new 默认生成，用 ${VAR} 占位）
├── version.txt
├── _gal/
├── values/                 # 值文件目录（sys_value.yml 为注释模板：取消注释要覆盖的项即可；localize 后生成 .env，仅非密钥配置）
└── sys/
    ├── sys_model.yml       # 系统定义（name / model 可选 / kind / vender；kind 决定命令分派）
    ├── mod_list.yml        # 可选，缺失时视为空模块列表
    ├── merged_vars.yml    # 聚合变量（sys update 生成：模块变量 ⊕ 系统变量，需入库）
    ├── setting/
    │   ├── vars.yml        # system 段变量定义（源，版本化）
    │   └── list.yml        # 可选，按模块的本地化列表；纯 docker-compose 系统可省略
    └── workflows/          # 可选，GXL 运维流；纯 docker-compose 系统可省略
```

## 关系

系统是模块之上的组合层，也是运维项目导入的来源对象：

```text
module -> system -> ops_prj
```
