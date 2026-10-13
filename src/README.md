# galaxy-ops 源码结构

这份文档只描述当前 `galaxy-ops` 仓库里真实存在的源码模块，不再保留旧版推断型架构说明。

## 顶层模块

当前 `src/lib.rs` 暴露的主要模块：

```text
artifact/
module/
system/
ops_prj/
workflow/
localize/
infra/
accessor/
conf.rs
const_vars.rs
error.rs
project.rs
types.rs
tools.rs
compat.rs
prelude.rs
```

## 核心分层

### 1. `module/`

模块对象层，对应 `gops mod`。

主要职责：

- 定义模块骨架
- 维护模块引用与依赖
- 维护模块模型目录
- 处理模块本地化

### 2. `system/`

系统对象层，对应 `gops sys`。

主要职责：

- 定义系统骨架
- 维护系统模型和模块列表
- 提供系统解析、本地化、打包入口

### 3. `ops_prj/`

运维项目对象层，对应 `gops prj`。

主要职责：

- 创建运维项目
- 导入系统到项目
- 更新项目本地引用

### 4. `workflow/`

工作流相关适配层。这里不是独立 CLI，而是库内与 GXL / 工作流组织相关的部分。

### 5. `artifact/`

构件、下载资源和相关结构。

### 6. 共同支撑模块

- `accessor/`：访问器和资源获取入口
- `infra/`：日志、环境与基础设施辅助能力
- `localize/`：本地化执行与模板渲染
- `conf.rs`：配置辅助
- `const_vars.rs`：稳定文件名和目录名常量
- `error.rs`：统一错误类型
- `types.rs`：公共 trait / 选项 / 路径类型

## 对象关系

```text
Module -> System -> Ops Project
```

代码层对应关系：

- `module/` 负责模块对象
- `system/` 负责系统对象
- `ops_prj/` 负责项目对象

## CLI 对应

```text
gops mod  <-> src/module
gops sys  <-> src/system
gops prj  <-> src/ops_prj
```

## 当前边界

这份文档不再描述以下旧内容：

- 并不存在的 `package/`、`app_sys/` 等顶层模块
- 推断型的资源管理器 / 任务管理器大架构
- 与当前代码不一致的旧 API 示例
