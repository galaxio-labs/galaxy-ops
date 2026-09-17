# ops_prj 模块说明

`src/ops_prj/` 对应 `gops prj` 这一组能力。

## 目标

项目层负责把一个 `System` 导入到具体客户或环境项目中，形成可持续维护的交付对象。

它解决的是：

- 运维项目如何初始化
- 系统如何导入到项目
- 项目如何更新本地引用

## 当前目录

```text
src/ops_prj/
├── conf.rs
├── import.rs
├── init.rs
├── install.rs
├── path.rs
├── project.rs
├── system.rs
└── README.md
```

## 主要文件

### `project.rs`

运维项目对象入口，围绕 `ops-prj.yml`（项目 manifest：name + work_envs + sys_models）工作。

> 历史：原 `ops-systems.yml` 已合并进 `ops-prj.yml`；加载时若存在旧的 `ops-systems.yml` 会自动合并并迁移到单文件。

`owner_project_value_dir(sys_dir)`：给定项目内的系统目录，返回项目为它维护的值目录 `values/<sys_name>`（依据上层 `ops-prj.yml` 与系统名匹配）。`gops sys localize` / `sys update` 用它定位客户值，因此不依赖 `<sys>/values` 符号链接是否完整。

### `import.rs`

系统导入与重新导入逻辑，对应 `gops prj import` / `gops prj reimport`。

- `import_sys`：导入一个新系统
- `reimport`：按 `ops-prj.yml` 记录的 `sys_models` 重新导入系统，保留 `values/` 客户值（适用于“删除了已导入系统目录、但保留了 values/ + ops-prj.yml”的场景）

### `init.rs`

项目初始化模板，对应 `gops prj new`。

### `path.rs`

项目路径组织，负责定位 `ops-prj.yml`（`target_file` 保留用于向后兼容旧 `ops-systems.yml`）。

### `system.rs`

项目内系统引用相关逻辑（`OpsSystem` / `OpsTarget`）。

## 与 CLI 的对应

```text
gops prj new
gops prj import
gops prj update
gops prj reimport
```

## 输出对象

当前项目层默认生成的对象结构类似：

```text
<ops-project-root>/
├── ops-prj.yml       # name + work_envs + sys_models（已导入系统列表）
├── values/           # 客户值目录（values/<sys>/value.yml 客户覆盖；prj reimport 保留）
├── version.txt
└── _gal/
```

## 关系

项目层是交付对象层：

```text
module -> system -> ops_prj
```

其中：

- `module` 提供可复用能力单元
- `system` 提供共享系统定义
- `ops_prj` 提供客户或环境级落地对象
