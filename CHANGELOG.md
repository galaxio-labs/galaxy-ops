# 变更日志

所有重要的项目变更都将记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.0.0/),
并且本项目遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [1.3.0] - 2026-09-17

### 新增功能
- **docker-compose 系统类型**：新增 `SysKind` 与 `sys/sys_model.yml` 的 `kind` 字段，统一 `gops sys` 入口管理 GXL 与纯 compose 系统，部署命令自动分派到 `docker compose`
- **`gops prj reimport`**：按 `ops-prj.yml` 的 `sys_models` 重新导入并保留 `values/`
- **`${SEC_xxx}` 密钥注入**：`sys localize` 导出 `.env`（仅非密钥），密钥运行时从 `~/.galaxy/sec_value.yml` 注入，不落盘

### 重大变更
- **`ops-prj.yml` 与 `ops-systems.yml` 合并**（向后兼容旧文件）
- **`sys_vars.yml` 重命名为 `merged_vars.yml`**（聚合变量：模块 ⊕ 系统合并，向后兼容旧名）

### 改进优化
- 系统文件可选化：`mod_list.yml` / `workflows/` / `setting/list.yml` 缺失时降级为空
- `sys localize` 以系统默认值（`sys/merged_vars.yml` 的 `system:` 段）为基线，值文件只需写需要覆盖的项
- `sys update` 生成的 `values/sys_value.yml` 改为**注释模板**（可用变量全部注释，默认不生效）：只需取消注释要覆盖的项，其余取系统默认值，避免全量快照钉死后续默认值变更
- `sys new` 支持已存在目录（幂等）；移除冗余的 `setup_prj_root_env_vars`（`GXL_PRJ_ROOT` 已由 CLI 启动时的 `setup_start_env_vars` 统一设置）
- 引入 `orion-sec`；许可证统一为 MIT

### Bug 修复
- 修复 `values` 符号链接冲突、`convert_addr`/`build_pkg` 裸目录 panic、`mod new` 硬编码构件地址
- 修复 `load_sys_opr_value`/`load_mod_opr_value` 值文件序列化不一致
- 修复运维项目内 `cd <sys>; gops sys localize` 未使用项目值的问题：按 `ops-prj.yml` 直接解析 `values/<sys_name>/`，不再依赖 `<sys>/values` 符号链接是否完整
- 修复本地化时创建字面量目录（如 `galaxy-ops/${GXL_PRJ_ROOT}`）的问题：路径中残留未展开的 `${VAR}` 或源文件不存在时直接跳过，不再提前创建目标目录

## [1.2.0] - 2026-05-04

- 升级 `orion-error 0.8` 并统一 `orion-infra`/`orion_conf`/`orion-accessor`/`orion-variate` 生态依赖，消除 0.7/0.8 并存
- 恢复 `owe_res`/`err_conv`/`with` 等兼容入口，迁移 `get_reason`/`context` 到 0.8 原生接口

## [1.1.1] - 2026-04-06

- 版本元信息同步到 1.1.1，仓库地址统一为 `galaxio-labs`，清理 workspace 依赖声明

## [1.1.0] - 2026-03-27

- 切换到新一轮 `orion_*` 生态版本，新增 `UPGRADE.md` 与 `compat` 兼容层
- 统一变量大小写不敏感读取、模板渲染收敛；修复 `SysSetting` 作用域初始化、shell heredoc/算术移位注释剥离等边界

## 历史版本（0.x）

- **0.13.0-alpha**：整合 `gmod` 到 `gops`；新增 `SysSetting`/`SysValuePaths`；重构本地化与路径管理
- **0.11.0**：`ds-*` 重命名为 `g*`；新增本地化系统与 accessor；升级 `orion-variate 0.6.2`
- **0.10.6**：新增包管理、自动化模型生成、`UpdateValue` 工作流
- **0.10.5**：添加 `gops` 二进制工具
- **0.10.4**：版本号与构建优化
- **0.10.3**：工作流项目管理
- **0.10.2**：自动化测试支持
- **0.10.1**：包管理支持
- **0.10.0**：项目从 `orion-syspec` 重命名为 `galaxy-ops`，架构重构
- **0.9.0**：工作流项目管理，迁移到 GitHub 仓库
- **0.8.0**：项目初始化，基础架构搭建
