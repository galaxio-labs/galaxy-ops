# galaxy-ops 项目总览

## 项目定位

galaxy-ops 是面向数字业务保障场景的开源运维交付工具，用 Rust 编写，CLI 入口为 `gops`。

它负责**运维能力的组织、配置、组合与交付**，将项目实施过程沉淀为可复用、可演进的交付资产。它不负责工作流的实际执行——执行能力由同生态的 `galaxy-flow`（GXL 工作流引擎）提供。

一句话概括：

> `galaxy-ops` 组织、配置、组合、交付运维能力；`galaxy-flow` 定义并执行这些工作流。

## 核心对象

`galaxy-ops` 围绕三类核心对象工作：

```text
Module -> System -> Ops Project
```

- `Module`：最小可复用运维单元，包含规范、依赖、变量、模板和工作流。
- `System`：由多个模块组合形成的交付单元，用于表达完整系统的结构与操作方式。
- `Ops Project`：面向具体客户环境或部署现场的运维项目，用于导入系统、管理本地值和持续更新。

三层分离后，同一个系统可以被多个客户项目重复使用，客户差异（域名、IP、端口、证书、资源规格、开关等）放在各自项目的值文件里，不写死在系统定义中。

## 项目结构

```text
galaxy-ops/
├── app/gops/                  # gops CLI（clap 命令定义与分发）
│   └── commands/
│       ├── mod_cmd.rs         # gops mod
│       ├── sys_cmd.rs         # gops sys
│       ├── prj_cmd.rs         # gops prj
│       └── common/            # 公共参数（debug/log/force/localize）
├── src/
│   ├── lib.rs                 # 库入口
│   ├── artifact/              # 构件与资源下载
│   ├── module/                # 模块对象层（对应 gops mod）
│   ├── system/                # 系统对象层（对应 gops sys）
│   ├── ops_prj/               # 运维项目对象层（对应 gops prj）
│   ├── workflow/              # 与 GXL 工作流相关的适配层
│   ├── localize/              # 本地化执行与模板渲染
│   ├── accessor.rs            # 访问器与资源获取入口
│   ├── infra/                 # 日志、环境与基础设施辅助
│   ├── error.rs               # 统一错误类型
│   ├── types.rs               # 公共 trait / 选项 / 路径类型
│   ├── const_vars.rs          # 稳定文件名与目录名常量
│   ├── conf.rs                # 配置辅助
│   ├── project.rs             # 项目级公共结构
│   ├── tools.rs               # 工具函数与宏
│   ├── compat.rs              # 旧接口兼容层（已标记 deprecated）
│   └── prelude.rs             # 对外预导出
├── example/                   # 示例项目（当前较稀疏）
├── tests/                     # 集成测试
└── test_data/                 # 测试数据（helm/yaml 等）
```

## 核心模块说明

### 1. `module/`（对应 `gops mod`）

模块对象层，负责最小可复用运维单元的骨架初始化、引用/依赖维护、模型组织和本地化。

- `operator.rs`：模块对象入口（`new` / `update` / `localize` 核心流程）
- `spec.rs`：模块规范与模板初始化
- `model.rs`：模块模型结构（围绕 `ModelSTD` 组织）
- `refs.rs` / `depend.rs`：模块引用与依赖
- `init/`：模块初始化模板（`_gal` / `host` / `k8s`）

### 2. `system/`（对应 `gops sys`）

系统对象层，把多个模块组织成可操作、可本地化、可交付的系统对象，并提供系统级操作入口。

- `operator.rs`：系统对象入口
- `conf.rs`：`SysConf`（`test_envs`）与 `SysKind`（`gxl` / `docker-compose`）；`kind` 在 `SysDefine`（`sys_model.yml`）里，决定 `sys` 命令分派到 gx 还是 docker compose
- `spec.rs` / `mod_list.rs`：系统定义与模块列表
- `path.rs`：系统路径组织（`sys-prj.yml` / `sys_model.yml` / `mod_list.yml` / `setting/`）
- `setting/`：系统设置、本地化与模板化（`export` / `localize` / `sys` / `templatize`）
- `init/`：系统初始化模板（`_gal` / `workflows`）

纯 docker-compose 系统的密钥用 `${SEC_xxx}` 占位，运行时由 `orion-sec` 从 `~/.galaxy/sec_value.yml` 注入，不落盘。

### 3. `ops_prj/`（对应 `gops prj`）

项目对象层，把 `System` 导入到具体客户或环境项目，形成可持续维护的交付对象。

- `project.rs`：项目对象入口（`ops-prj.yml`，含 name / work_envs / sys_models）
- `import.rs` / `install.rs`：系统导入、重新部署与安装
- `system.rs` / `path.rs`：项目内系统引用与路径组织
- `init/`：项目初始化模板

### 4. `workflow/`

与 GXL 工作流相关的适配层，不是独立的执行引擎。它把 GXL 文件当作工作流资源进行保存、加载与分发：

- `gxl.rs`：`GxlAction`（单个 GXL 文件的内容 + 文件名）
- `act.rs`：`Workflow` / `Workflows`（一组工作流的保存与加载）
- `prj.rs`：`GxlProject`（`_gal/work.gxl`、`adm.gxl`、`project.toml` 的项目级组织）

### 5. `artifact/`

构件与资源下载相关结构：

- `core.rs`：`Artifact`（名称、版本、来源地址、缓存地址与本地路径，提供下载到本地能力）
- `package.rs`：`ArtifactPackage`（`Vec<Artifact>` 的透明包装）
- `types.rs`：`PackageType` / `BinPackage` / `GitPackage`，以及把 URL/路径解析为地址的 `convert_addr` / `build_pkg`

### 6. `localize/`

本地化执行与模板渲染：

- 基于 Handlebars 的模板渲染（`tpl_impl.rs`）
- 针对 C / Shell / YAML 三种注释格式的剥离处理（含 heredoc、算术移位、YAML 块标量等边界）
- `conf` / `exec` / `path` / `set` / `tpl_path` 共同支撑本地化流程

### 7. 公共支撑

- `accessor.rs`：访问器入口，供模块/系统/项目在 update / import / download 场景下获取资源
- `infra/`：日志（`log.rs`）、环境与路径辅助（`path.rs`）
- `error.rs`：统一错误类型，CLI 最终通过它整理成一致的 `MainResult` 报错格式
- `const_vars.rs`：`mod-prj.yml`、`sys-prj.yml`、`ops-prj.yml`、`sys_model.yml` 等稳定文件名/目录名约定

## CLI 命令

```text
gops mod
  ├── example     创建示例模块
  ├── new         定义新模块
  ├── update      更新模块引用/依赖
  └── localize    本地化模块配置

gops sys
  ├── new         创建系统（--kind gxl|docker-compose 指定部署类型）
  ├── update      更新系统引用
  ├── package     解析变量并打包为 .tar.gz 交付产物
  ├── localize    生成系统本地化结果（并导出 .env 供 docker-compose 使用）
  ├── setting     初始化系统设置
  └── download / install / uninstall / start / stop / status / diagnose
                  （按 sys/sys_model.yml 的 kind 分派：gxl → 外部 gx（`gx run <cmd>`）；docker-compose → docker compose）

gops prj
  ├── new         创建运维工程
  ├── import      导入系统到工程
  ├── update      更新工程本地引用
  └── reimport    按 ops-prj.yml 重新导入系统（保留 values/ 客户值）
```

## 技术栈

核心依赖（见 `Cargo.toml`）：

- **CLI**：`clap`（derive）
- **异步**：`tokio`
- **模板渲染**：`handlebars`
- **序列化**：`serde` / `serde_json` / `serde_yaml` / `serde_ini` / `toml`
- **网络**：`reqwest`
- **Git**：`git2`
- **压缩**：`flate2` / `tar`
- **错误/配置/基础设施/密钥**：`orion-error` / `orion-conf` / `orion-infra` / `orion-accessor` / `orion-variate` / `orion-sec`（内部生态）

## 构建与测试

```bash
# 构建
cargo build --release

# 调试运行
cargo run --bin gops -- --help

# 测试
cargo test
```

## 相关文档

- [README](./README.md)：项目定位、核心对象与 Quick Start
- [源码结构](./src/README.md)
- [核心文件说明](./src/core_files.md)
- [升级迁移指南](./UPGRADE.md)
- [Module 模块文档](./src/module/README.md)
- [System 系统文档](./src/system/README.md)
- [Ops Project 文档](./src/ops_prj/README.md)

## 许可证

MIT License，详见 `LICENSE` 文件。
