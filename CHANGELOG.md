# 变更日志

所有重要的项目变更都将记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.0.0/),
并且本项目遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [1.2.0] - 2026-05-04

### 重大变更
- **Orion 错误体系升级**: 升级到 `orion-error 0.8`，同步切换到新版模块化 API 路径与 `StructError` 访问方式
- **Orion 生态依赖统一**: 升级 `orion-infra`、`orion_conf`、`orion-accessor`、`orion-variate` 等依赖，消除 `orion-error 0.7` 与 `0.8` 同时存在导致的类型不一致问题

### 改进优化
- **错误兼容层更新**: 在 `compat` / `prelude` 中恢复 `owe_res`、`owe_conf`、`err_conv`、`with`、`want` 等旧调用入口，降低下游迁移成本
- **错误上下文适配**: 将旧版 `get_reason`、`target`、`context`、`error_code` 调用迁移到 0.8 原生 `reason()`、`target_path()`、`contexts()` 等接口
- **路径上下文记录修正**: 对 `Path` / `PathBuf` 上下文记录显式使用 `.display()`，适配新版 `OperationContext::record` 的 `Display` 约束
- **测试工具入口迁移**: 将测试断言 helper 调整到 `orion_error::dev::testing` 新路径

### 测试
- `cargo check` 通过
- `cargo test` 通过：库测试、二进制测试、集成测试与 doc tests 均已验证

## [1.1.1] - 2026-04-06

### 改进优化
- **版本与元信息同步**: 版本号更新到 `1.1.1`，并同步整理 `Cargo.toml`、`version.txt`、许可证声明等发布元信息
- **仓库地址统一**: 项目内引用的 GitHub 组织从旧地址统一切换到 `galaxio-labs`
- **工作区依赖清理**: 收敛 `Cargo.toml` 中的 workspace 依赖声明，移除历史遗留注释与无效配置，降低维护噪音

### 文档更新
- **README 重写**: 重新整理项目定位、核心对象、CLI 用法、仓库结构以及“单系统、多客户交付”设计说明
- **模块文档收敛**: 更新 `src/README.md`、`src/module/README.md`、`src/system/README.md`、`src/ops_prj/README.md` 等文档，使表达与当前实现和产品定位对齐
- **说明文档精简**: 清理并压缩部分历史说明文稿，减少过时或重复内容

### 测试
- **测试资源地址更新**: 将测试中的示例 Git 资源地址同步切换到新的 GitHub 组织路径

## [1.1.0] - 2026-03-27

### 重大变更
- **依赖升级定版**: 正式切换到新一轮 `orion_*` 生态版本，不再回退旧版兼容实现
  - `orion-error` 升级到 `0.6`
  - `orion_conf` 升级到 `0.5`
  - `orion-infra` 升级到 `0.5`
  - `orion-accessor` 通过别名 `orion_variate` 升级到 `0.6`
  - `orion-variate` 通过别名 `orion_vars` 升级到 `0.11`

### 新增功能
- **升级迁移文档**: 新增 `UPGRADE.md`，集中说明配置读写 API、错误处理、变量访问和兼容迁移路径
- **兼容层恢复**: 重新提供 `galaxy_ops::compat::*` 公开过渡入口，保留旧 trait 名用于下游渐进迁移

### 改进优化
- **兼容入口分层**: 将公开兼容 `prelude` 与内部升级用导入层拆开，避免新旧配置读写 trait 同时进入作用域时出现方法解析歧义
- **变量访问统一**: 大小写不敏感读取统一切换为 `get_case_insensitive()`
- **错误语义保留**: 升级后不再把细粒度新错误统一压平回旧的大类错误，尽量保留上游 detail / context / position
- **模板渲染收敛**: 模板渲染链路统一到新语义，不再依赖旧的隐式兼容行为

### Bug 修复
- **系统设置加载初始化**: 修复 `SysSetting::load_from()` 在新版本下未触发加载后初始化，导致变量作用域未正确标记的问题
- **Shell 注释剥离**: 修复 `$((1 << 2))` 算术移位被误判为 heredoc 的问题
- **多 heredoc 处理**: 修复同一条 shell 命令包含多个 heredoc 时仅记录最后一个，导致前续 body 中注释被误删的问题
- **兼容导出入口**: 修复升级后 `prelude::*` / `compat::*` 对下游老调用方的源码级 break

### 测试
- 补充 legacy `prelude` 与 `compat` 读写路径回归测试
- 补充 `SysSetting` 加载后变量作用域初始化测试
- 补充 shell heredoc、算术移位和 YAML 注释剥离边界测试

## [0.13.0-alpha] - 2025-09-01

### 重大变更
- **应用程序整合**: 移除独立的 `gmod` 应用，将其功能完全整合到 `gops` 中
- **常量重命名**: `SYS_MODEL_PRJ_ROOT` 重命名为 `SYS_OPERATORS_ROOT`
- **项目结构重构**: 重命名核心组件以提供更清晰的语义
  - `ModProject` → `ModOperator`
  - `SysProject` → `SysOperator`
  - `ops_prj::proj` → `ops_prj::project`
  - `system::proj` → `system::operator`

### 新增功能
- **系统设置管理**: 新增 `SysSetting` 结构体和完整的设置管理系统
  - 支持模块级别的本地化配置
  - 提供 YAML 格式的配置文件存储
  - 集成变量作用域管理和环境评估
- **系统值路径**: 新增 `SysValuePaths` 结构体统一管理系统值文件路径
- **独立包安装模块**: 新增 `SystemPackageInstaller` 类封装包安装逻辑
  - 提供专门的包安装功能
  - 支持解压缩和文件复制操作
  - 改进安装日志和错误处理
- **模块列表管理**: 将模块列表逻辑分离到独立的 `mod_list.rs` 模块

### 改进优化
- **本地化系统重构**: 完全重新设计本地化系统架构
  - 引入 `SysValuePaths` 结构管理值路径
  - 改进变量评估和环境处理
  - 增强错误处理和上下文记录
- **路径管理集中化**: 将路径管理逻辑集中到 `SysOperatorPath` 模块
  - 统一管理系统操作路径
  - 支持配置文件自动迁移 (v1 → v2)
  - 提供路径存在性检查和创建功能
- **依赖更新**: 升级到 `orion_variate 0.8.2`，新增 `pathdiff` 依赖

### Bug 修复
- 修复测试路径初始化问题
- 改进系统操作器初始化的错误上下文
- 修复操作上下文名称和本地化调用问题
- 修复包安装逻辑中的路径处理问题

## [0.11.0] - 2025-08-10

### 重大变更
- **应用程序重命名**: 将所有 `ds-*` 前缀的应用程序重命名为 `g*` 系列
  - `ds-mod` → `gmod` (模块管理工具)
  - `ds-ops` → `gops` (运维工具)
  - `ds-sys` → `gsys` (系统工具)
  - `ds-mcp` → `gmcp` (MCP 服务)
- **依赖更新**: 升级到 `orion-variate 0.6.2`

### 新增功能
- **本地化系统**: 新增完整的本地化功能
  - 添加 `LocalizeVarPath` 和 `LocalizeSet` 类型
  - 支持模块级别的本地化配置  [mod_list](https://galaxio-labs.github.io/operator-docs/operator/sys/structure/file-organization.html)
  - 添加模板化和设置导出功能
- **通用访问器**: 新增 accessor 模块
  - 统一管理下载操作和资源访问
  - 支持网络访问控制和重定向配置

### 改进优化
- **测试覆盖率**: 大幅提升测试覆盖率
- **资源管理**: 重构资源更新接口，引入 Accessor 统一管理下载操作

### Bug 修复
- 修复模块合并相关的 bug
- 修复版本号相关问题
- 修复 YAML 注释解析问题
- 修复测试框架编译错误
- 修复日志记录相关问题

## [0.10.6] - 2025-07-31

### 新增功能
- 新增包管理功能，支持项目包的创建和管理
- 新增自动化模型生成功能
- 新增测试用例，增强代码覆盖率
- 新增 `UpdateValue` 工作流支持
- 新增 `addr` 的上传方法

### 改进优化
- 重构代码结构，提升代码可读性和维护性
- 优化项目配置和构建流程
- 更新 GXL 初始化流程
- 清理无用代码，减少项目体积
- 改进错误处理和日志记录

### Bug 修复
- 修复版本发布相关的 bug
- 修复混合使用值的问题
- 修复模块配置错误 (`project.toml`)
- 修复代码格式和 clippy 警告

### 重构
- 重命名 `orion_spec` 为 `orion_ops`
- 重命名 `workins` 为 `ops_prj`
- 重命名 `orion_x` 为 `orion_variate`
- 拆分 artifact 整合逻辑
- 将测试目录从 `test` 重命名为 `test_data`

### 文档更新
- 更新 README.md 文档
- 更新项目概述和配置文档
- 更新 GXL 模板和配置


## [0.10.5] - 2025-07-26

### 新增功能
- 添加 `gops` 二进制工具
- 支持系统规范地址使用
- 增强项目工作流管理

### 改进优化
- 优化项目结构和模块组织
- 改进依赖管理和版本控制

## [0.10.4] - 2025-07-31

### 改进优化
- 更新版本号到 0.10.4
- 优化构建配置
- 改进代码质量和格式

### Bug 修复
- 修复模块配置问题
- 修复构建和测试相关问题

## [0.10.3] - 2025-07-29

### 新增功能
- 添加工作流项目管理功能
- 支持项目初始化和配置

### 改进优化
- 重构项目初始化流程
- 改进系统路径和配置管理

## [0.10.2] - 2025-07-28

### 新增功能
- 添加自动化测试支持
- 增强项目配置管理

### 改进优化
- 优化项目构建流程
- 改进错误处理机制

## [0.10.1] - 2025-07-25

### 新增功能
- 添加包管理支持
- 增强项目部署功能

### 改进优化
- 重构包管理逻辑
- 改进项目部署流程

## [0.10.0] - 2025-07-24

### 重大变更
- 项目从 `orion-syspec` 重命名为 `galaxy-ops`
- 重构整个项目架构
- 更新所有依赖和配置

### 新增功能
- 全新的项目管理系统
- 支持多环境部署
- 增强的配置管理
- 改进的构建系统

### 改进优化
- 优化代码结构和组织
- 改进错误处理和日志
- 增强测试覆盖率

## [0.9.0] - 2025-07-17

### 新增功能
- 添加工作流项目管理
- 支持项目初始化和配置管理
- 增强系统规范支持

### 改进优化
- 重构项目结构
- 改进依赖管理
- 优化构建流程

### 重构
- 将 coding 仓库迁移到 github 仓库
- 重命名多个模块和组件

## [0.8.0] - 2025-07-16

### 初始版本
- 项目初始化和基础架构搭建
- 核心功能模块实现
- 基础测试框架建立
- 文档和配置初始化

### 包含模块
- 核心类型定义和错误处理
- 配置管理系统
- 项目构建和部署工具
- 基础测试用例

---

## 版本历史

- **0.13.0-alpha**: 重大架构更新，应用程序整合，新增系统设置管理，重构本地化系统，改进路径管理
- **0.11.0-beta**: 重大更新，重命名应用程序和项目，新增包管理、本地化、访问器等核心功能
- **0.10.6**: 稳定版本，包含完整的包管理和测试支持
- **0.10.5**: 增强项目管理和工作流支持
- **0.10.4**: 修复bug和优化构建流程
- **0.10.3**: 添加工作流项目管理
- **0.10.2**: 增强测试和配置管理
- **0.10.1**: 添加包管理支持
- **0.10.0**: 重大版本更新，项目重命名和架构重构
- **0.9.0**: 工作流项目管理系统
- **0.8.0**: 初始版本发布

[0.13.0-alpha]: https://github.com/galaxio-labs/galaxy-ops/compare/v0.12.5-beta...v0.13.0-alpha
[0.11.0-beta]: https://github.com/galaxio-labs/galaxy-ops/compare/v0.10.6...v0.11.0-beta
[0.10.6]: https://github.com/galaxio-labs/galaxy-ops/compare/v0.10.5...v0.10.6
[0.10.5]: https://github.com/galaxio-labs/galaxy-ops/compare/v0.10.4...v0.10.5
[0.10.4]: https://github.com/galaxio-labs/galaxy-ops/compare/v0.10.3...v0.10.4
[0.10.3]: https://github.com/galaxio-labs/galaxy-ops/compare/v0.10.2...v0.10.3
[0.10.2]: https://github.com/galaxio-labs/galaxy-ops/compare/v0.10.1...v0.10.2
[0.10.1]: https://github.com/galaxio-labs/galaxy-ops/compare/v0.10.0...v0.10.1
[0.10.0]: https://github.com/galaxio-labs/galaxy-ops/compare/v0.9.0...v0.10.0
[0.9.0]: https://github.com/galaxio-labs/galaxy-ops/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/galaxio-labs/galaxy-ops/releases/tag/v0.8.0
