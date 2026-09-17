# 核心文件说明

本文档只描述当前 `galaxy-ops` 代码里真实存在且稳定的核心文件。

## 核心文件

### `src/types.rs`

公共类型与 trait 定义。

这里主要承载：

- 本地化相关选项
- 更新相关 trait
- 路径和值对象

它被 `module`、`system`、`ops_prj` 等模块共同依赖。

### `src/error.rs`

统一错误类型和错误输出入口。

CLI 最终通过这里把内部错误整理成一致的 `MainResult` / 报错格式。

### `src/const_vars.rs`

全局稳定常量，尤其是文件名和目录名约定，例如：

- `mod-prj.yml`
- `sys-prj.yml`
- `ops-prj.yml`
- `mod_list.yml`
- `sys_model.yml`
- `values/`
- `setting/`
- `workflows/`

这份文件对文档和实现对齐非常关键。

### `src/conf.rs`

配置辅助能力，不等于一个完整“系统配置中心”。它是若干对象加载和保存逻辑的一部分。

### `src/project.rs`

项目级公共结构与辅助逻辑。

### `src/tools.rs`

工具函数和宏辅助。

### `src/accessor.rs`

访问器入口，供模块、系统、项目在 update / import / download 等场景下获取资源。

### `src/compat.rs`

兼容性处理逻辑。

### `src/prelude.rs`

对外预导出。

## 与对象模块的关系

这些核心文件本身不直接对应某个一级 CLI 命令，它们服务于：

- `src/module/`
- `src/system/`
- `src/ops_prj/`

## 文档边界

不再保留以下旧式描述：

- 代码里并不存在的 `resource.rs`、`software.rs`、`task.rs` 等核心文件推断
- 预设的“大而全系统管理框架”
- 与当前模块名不一致的旧文件名
