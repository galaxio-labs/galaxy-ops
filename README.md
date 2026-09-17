# Galaxy-OPS
[![CI](https://github.com/galaxio-labs/galaxy-ops/workflows/CI/badge.svg)](https://github.com/galaxio-labs/galaxy-ops/actions)
[![Coverage Status](https://codecov.io/gh/galaxio-labs/galaxy-ops/branch/main/graph/badge.svg)](https://codecov.io/gh/galaxio-labs/galaxy-ops)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-blue.svg)](https://www.rust-lang.org)

galaxy-ops 是面向数字业务保障场景的开源运维交付工具，用于组织、配置、组合和交付运维能力，并将项目实施过程沉淀为可复用、可演进的交付资产。

在整体产品分工中：
- `galaxy-ops` 负责模块、系统和运维项目的组织与交付
- `galaxy-flow` 提供基于 GXL 的工作流定义与执行能力

## Why Galaxy-OPS

很多自动化工具解决的是“命令怎么执行”，`galaxy-ops` 更关注“运维能力如何被组织、组合、配置、本地化和交付”。

它不是泛化的 CI/CD 平台，也不是单纯的部署脚手架，而是把模块、系统和项目这些交付对象沉淀为可复用资产的工具层。

## Core Objects

`galaxy-ops` 围绕三类核心对象工作：

- `Module`：最小可复用运维单元，包含规范、依赖、变量、模板和工作流。
- `System`：由多个模块组合形成的交付单元，用于表达完整系统的结构与操作方式。
- `Ops Project`：面向具体客户环境或部署现场的运维项目，用于导入系统、管理本地值和持续更新。

对应关系可以简化为：

```text
Module -> System -> Ops Project
```

## What It Does

当前仓库中的 `gops` CLI 主要提供三组能力：

- `gops mod`：创建模块、生成示例、更新引用、本地化模块配置。
- `gops sys`：创建系统、更新系统、生成环境本地化结果，并执行下载、安装、启动、停止、状态查询等系统操作。
- `gops prj`：创建运维工程、导入系统、更新本地项目引用。

这意味着 `galaxy-ops` 负责交付组织层，而不是直接替代工作流执行引擎：

- `galaxy-ops` 管理模块、系统、项目与交付结构
- `galaxy-flow` 管理 GXL 工作流的定义与执行

## CLI Overview

主命令：

```bash
gops <COMMAND>
```

一级命令：

- `gops mod`
- `gops sys`
- `gops prj`

常用子命令：

```bash
# 创建模块
gops mod new --name nginx

# 本地化模块配置
gops mod localize

# 创建系统
gops sys new --name web-stack

# 更新系统引用
gops sys update

# 解析变量并打包系统为 .tar.gz
cd web-stack
gops sys package

# 创建运维工程
gops prj new --name customer-a

# 向运维工程导入已打包的系统
gops prj import --path /path/to/web-stack-0.1.0.tar.gz

# 更新运维工程本地引用
gops prj update

# 重新导入系统（保留 values/ 客户值）
gops prj reimport
```

查看完整帮助：

```bash
gops --help
gops mod --help
gops sys --help
gops prj --help
```

## Quick Start

### Build

```bash
cargo build --release
```

调试运行：

```bash
cargo run --bin gops -- --help
```

### 1. Create a Module

```bash
mkdir demo-work && cd demo-work
gops mod new --name nginx
```

模块目录中通常会包含规范、变量、值文件和工作流等内容，用于表达一个可复用运维单元。

### 2. Create a System

```bash
gops sys new --name web-stack
```

系统用于组合多个模块，并形成一个更接近交付视角的系统定义。

默认是模块式（GXL）系统；也支持纯 docker-compose 系统：`gops sys new --name web-stack --kind docker-compose`。此时 `sys start/stop/status/...` 自动映射到 `docker compose`，密钥用 `${SEC_xxx}` 占位、运行时从 `~/.galaxy/sec_value.yml` 注入（详见 [example/README.md](./example/README.md) 与 [src/system/README.md](./src/system/README.md)）。

### 3. Package the System

```bash
cd web-stack
gops sys package
```

`sys package` 会先执行一次 `update`（解析模块变量、生成 `resolved_vars.yml`），再把系统打包为父目录下的 `<name>-<version>.tar.gz`，作为可交付产物。

### 4. Create an Ops Project

```bash
cd ..
gops prj new --name customer-a
cd customer-a
gops prj import --path ../web-stack-0.1.0.tar.gz
gops prj update
```

运维项目用于把系统导入到具体环境，并在本地持续维护配置和值文件。

## Repository Layout

```text
galaxy-ops/
├── app/gops/           # gops CLI
├── src/module/         # 模块模型、引用与本地化
├── src/system/         # 系统模型、系统操作与设置
├── src/ops_prj/        # 运维项目创建、导入与更新
├── src/workflow/       # 与工作流相关的适配层
├── example/            # 示例项目
└── tests/              # 集成测试
```

## Key Design Points

- 模块化：先定义模块，再组合系统，最后落到具体运维项目。
- 本地化：通过变量和值文件生成特定环境下可执行的配置结果。
- 可交付：把项目里的临时脚本和配置沉淀为结构化资产。
- 与 GXL 协同：工作流执行能力由 `galaxy-flow` 提供，`galaxy-ops` 不重复发明执行器。

## One System, Many Customers

`galaxy-ops` 能支持“一个系统，多客户自动化部署”，核心不是复制脚本，而是把“可复用定义”和“客户差异化配置”分层管理。

它的设计可以概括为三层：

- `Module` 层：沉淀可复用的模块规范、依赖、模板、变量和工作流。
- `System` 层：把多个模块组合成一个可交付系统，并维护系统级变量、模块列表和系统工作流。
- `Ops Project` 层：每个客户或每个环境对应一个独立运维项目，负责导入同一个系统，并维护自己的本地值文件和交付结果。

这三层分离后，同一个系统可以被多个客户项目重复使用，而不会把客户差异直接写死在系统定义里。

### 关键设计

#### 1. 系统定义与客户配置分离

系统本身负责表达：

- 系统包含哪些模块
- 模块之间如何组合
- 需要哪些系统变量和模块变量
- 默认工作流和交付结构是什么

客户差异则放在运维项目和值文件里表达，例如：

- 域名
- IP / 端口
- 证书路径
- 资源规格
- 环境开关
- 客户特有依赖地址

这意味着“系统定义”可以保持稳定，“客户配置”可以独立演进。

#### 2. 通过值文件做本地化

`galaxy-ops` 会为系统和模块初始化值文件，并在本地化阶段生成目标环境所需的配置结果。

可以把它理解为：

```text
System / Module Spec
  + Customer Values
  -> Localized Output
```

也就是说：

- 同一个 `System` 可以导入到多个 `Ops Project`
- 每个 `Ops Project` 使用不同的 `value.yml`、模块值和系统值
- 本地化后得到各客户独立的最终配置和部署结果

#### 3. 通过导入和更新保持共享定义

`gops prj import` 用于把系统导入到客户运维项目中。

之后：

- 公共系统定义可以继续演进
- 客户项目可以通过 `gops prj update` 获取共享定义的更新
- 客户自己的值文件和本地配置仍保留在各自项目中

这样可以避免“每交付一个客户，就复制一份完全独立的系统代码”。

#### 4. 模块级与系统级同时支持差异化

差异化不只发生在系统层，也可以发生在模块层：

- 系统级值：控制整个系统的公共行为
- 模块级值：控制具体模块在某个客户环境中的配置

这让同一个系统既能复用统一结构，也能在数据库、网关、采集、规则、路径等模块上做客户级差异。

### 最小流程

```text
1. 定义可复用 Module
2. 组合形成 System
3. 为客户 A 创建 Ops Project，导入同一个 System
4. 为客户 B 创建 Ops Project，导入同一个 System
5. 分别维护客户 A / B 的值文件
6. 分别执行本地化与部署动作
```

对应到命令上，大致是：

```bash
# 创建系统
gops sys new --name web-stack

# 解析变量并打包（生成 web-stack-0.1.0.tar.gz）
cd web-stack
gops sys package
cd ..

# 客户 A
gops prj new --name customer-a
cd customer-a
gops prj import --path ../web-stack-0.1.0.tar.gz
gops prj update

# 客户 B
cd ..
gops prj new --name customer-b
cd customer-b
gops prj import --path ../web-stack-0.1.0.tar.gz
gops prj update
```

在这个模型里：

- `web-stack` 是共享系统定义
- `customer-a` 和 `customer-b` 是两个独立交付项目
- 两个项目共享同一套系统结构，但保留各自的本地值和交付结果

这正是 `galaxy-ops` 支持多客户自动化部署的基础设计。

## Documentation

- [升级迁移指南](./UPGRADE.md)
- [项目总览](./PROJECT_OVERVIEW.md)
- [Module 模块文档](./src/module/README.md)
- [System 系统文档](./src/system/README.md)
- [Ops Project 文档](./src/ops_prj/README.md)

## Positioning

如果用一句话概括：

`galaxy-ops` 是用于组织、配置、组合和交付运维能力的开源工具；`galaxy-flow` 是负责定义和执行这些工作流的开源引擎。
