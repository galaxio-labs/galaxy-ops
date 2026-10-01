# 项目背景

galaxy-ops 是一个现代化的运维管理平台，提供模块化管理、系统配置、包管理、工作流自动化等核心功能。
galaxy-ops,它通过为组件、系统提供 operator, 来实现对系统进行一键化的维护，如：
- 系统所有组件的安装、配置、启动、停止、重启等
- 系统的配置、监控、日志等
主要功能：
-  通过 gmod 来管理 mod operator, 包括创建、更新、本地化; 再由 gflow 来完成 安装、启动、停止、重启等
-  通过 gsys 来管理 sys operator, 包括创建、更新、本地化; 再由 gflow 来完成 安装、启动、停止、重启等
  sys operator 组合多个 mod operator, 来定义一个系统

## 工作规则
- 任务完成后需要把结果写到当前文档

## 工作任务

[x]  把src/module 构建的生成的文件结构，写到到docs/operator/mod, 生成的文件信息可以参考 example/modules/mysql_mock ,注意不是 rust 的源代码文件结构

完成结果：
- 已将src/module/init目录下的构建文件结构复制到docs/operator/mod
- 包含_gal/、host/、k8s/目录结构
- 创建了mod-prj.yml和version.txt文件
- 目录结构与example/modules/mysql_mock格式一致

[x] 为app下的每个二进制文件添加对应的man page, 补充输入参数说明
  - 已创建 man page 目录: docs/cmd/flow/man1/
  - 已创建 gsys.1: 包含 new, update, localize 命令文档
  - 已创建 gmod.1: 包含 example, new, update, localize 命令文档
  - 已创建 gops.1: 包含 new, import, update, localize, setting 命令文档
  - 已创建 README.md: 提供使用指南和安装说明
  - 所有参数已详细说明: --debug, --log, --force, --path, --value, --default
[x] 为app下的每个二进制文件添加clap文档注释，支持--help查看详细帮助
  - 完成结果：
    - 为gsys、gmod、gops三个二进制文件的args.rs添加详细clap文档注释
    - 每个命令枚举都有long_about详细描述
    - 每个参数都有help文本和使用说明
    - 支持--help查看完整文档，格式统一规范
    - 代码编译验证通过，文档注释格式正确

[x]   编写 mod-operaor 的文档
  - 目的： 为开发者、运维者编写 mod-operaor 编写指南
  - 输入： 代码 src/module 下的代码；  example/modules 下生成的示例
  - 输出： docs/operator/mod/

  完成结果：
  - 已完成完整的 Mod-Operator 开发指南文档，写入 docs/operator/mod/README.md
  - 文档包含完整的文件结构说明，详细描述了 Mod-Operator 的标准目录结构和各文件作用
  - 支持的目标平台详解，包括 ModelSTD 标准模型和所有支持的平台组合
  - 核心配置文件详解，包括 artifact.yml、depends.yml、vars.yml、setting.yml、workflows/operators.gxl 等所有配置文件的完整说明和示例
  - 开发工作流程，从创建模块到发布的完整步骤
  - 最佳实践指南，包括模块设计原则、工作流设计、变量管理、依赖管理等方面
  - 调试和故障排除指南，提供常见问题解决方案和调试技巧
  - 完整的示例模块，包括 PostgreSQL、Redis、Nginx 等实际案例
  - 文档基于对 src/module 代码和 example/modules 示例的深入研究，确保准确性和实用性


[x]   编写 sys-operaor 的文档
  - 目的： 为开发者、运维者编写 sys-operaor 编写指南
  - 输入： 代码 src/system 下的代码；  example/sys-model-prj 下生成的示例
  - 输出： docs/operator/sys/

  完成结果：
  - 已完成完整的 Sys-Operator 开发指南文档，写入 docs/operator/sys/README.md
  - 文档包含完整的文件结构说明，详细描述了 Sys-Operator 的标准目录结构和各文件作用
  - 支持的目标平台详解，包括 ModelSTD 标准模型和所有支持的平台组合
  - 核心配置文件详解，包括 sys_model.yml、mod_list.yml、vars.yml、adm.gxl、work.gxl、operators.gxl、sys-prj.yml 等所有配置文件的完整说明和示例
  - 开发工作流程，从创建系统到发布的完整步骤
  - 最佳实践指南，包括系统设计原则、配置管理、工作流设计、安全管理等方面
  - 调试和故障排除指南，提供常见问题解决方案和调试技巧
  - 完整的示例系统，包括微服务系统、数据处理平台、企业应用系统等实际案例
  - 文档基于对 src/system 代码和 example/sys-model-prj 示例的深入研究，确保准确性和实用性

[ ] 把 gmod ,gsys 两个APP 命令，合并到 gops， 减少使用者的理解成本
  -  gmod 转换为  gops mod
  -  gsys 转换为  gops sys

[x] 扩展 `gops prj`：现场态声明 + 备份 / 还原 / 非破坏更新
  - 目的：`prj reimport` 会 `rm -rf` 掉包外的现场态（身份私钥 / store / 运行态），`prj update` 又只管项目 conf ——
    升级、备份、还原三件事在 gops 里没有对应能力，只能每个系统自己写脚本。
  - 输入：`src/ops_prj/`（import / project / install / conf）、`src/system/pack.rs`，以及真实部署现场（见文档 §2）
  - 输出：docs/design/prj-backup-restore.md
  - 要点：`sys-prj.yml` 新增 `preserve` / `backup`（restore / rebuild 两档），`ops-prj.yml` 新增 `backup`（落点 / 份数 / 系统与级别）；
    不变式 `ignore ⊆ preserve`；非破坏更新直接复用 `sys package` 的文件选取逻辑
  - 已实现：`prj update` 非破坏覆盖（`update.rs::apply_overlay`）、`prj backup` / `prj restore`（`backup.rs`，含 sha256 清单 / 私钥点名 / `keep` 清理）、
    `prj reimport` 只补缺失（默认不删）+ `prj rebuild` 原子重建、`prj doctor` → `prj diagnose`（含 `ignore ⊆ preserve`、`sys_models` 重名等检查）；均已补测试
  - 遗留（独立现场问题，见文档 §9）：采集日志 11GB/天 无轮转且落在 `configs/` 下；`gateway-alone/ops-prj.yml` 的 `sys_models` 重名
