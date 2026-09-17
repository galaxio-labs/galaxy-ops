# workflow 模块文档

`src/workflow/` 是与 GXL 工作流相关的**适配层**，不是独立的执行引擎。

它把 GXL 工作流当作一种工作流资源进行组织：保存、加载、分发，供模块、系统、项目在生成骨架时写入 `_gal/` 与 `workflows/` 目录。真正的 GXL 定义与执行由 `galaxy-flow` 提供。

## 当前目录

```text
src/workflow/
├── mod.rs
├── gxl.rs
├── act.rs
├── prj.rs
└── prelude.rs
```

## 主要结构

### `GxlAction`（`gxl.rs`）

单个 GXL 文件的内容载体：

```rust
pub struct GxlAction {
    file: String,   // 文件名
    code: String,   // 文件内容
}
```

关键方法：

```rust
impl GxlAction {
    pub fn new(file: String, code: String) -> Self;
    // 判断文件名是否为约定俗成的动作文件
    pub fn is_action(path: &Path) -> bool;
}
```

`is_action` 识别以下约定文件名：

- `setup.gxl`
- `update.gxl`
- `port.gxl`
- `backup.gxl`
- `uninstall.gxl`

`GxlAction` 实现了 `FilePersist`，保存时把 `code` 写入目标目录下的 `file` 文件，加载时读取文件内容。

### `Workflow` / `Workflows`（`act.rs`）

一组工作流的容器：

```rust
pub enum Workflow {
    Gxl(GxlAction),
}

pub struct Workflows {
    actions: Vec<Workflow>,
}

pub type ModWorkflows = Workflows;
pub type SysWorkflows = Workflows;
```

`Workflows` 实现 `FilePersist`：

- 保存：把每个 `Workflow` 写入 `WORKFLOWS_DIR`（即 `workflows/`）目录
- 加载：遍历 `workflows/` 目录，按扩展名分发加载（当前仅支持 `.gxl`），失败项会被记录并忽略

`Workflow` 本身按文件扩展名分发：`.gxl` → `GxlAction`，其它类型暂不支持。

### `GxlProject`（`prj.rs`）

项目级的 GXL 文件组织，对应 `_gal/` 目录下的三个文件：

```rust
pub struct GxlProject {
    work: String,          // _gal/work.gxl
    adm: Option<String>,   // _gal/adm.gxl
    prj: Option<String>,   // _gal/project.toml
}
```

保存行为：

- 总是写入 `_gal/work.gxl`
- 存在 `adm` 时写入 `_gal/adm.gxl`，并在缺少 `version.txt` 时初始化为 `0.1.0`
- 存在 `prj` 时写入 `_gal/project.toml`

对应常量见 `src/const_vars.rs`：

```text
WORK_GXL = "work.gxl"
ADM_GXL  = "adm.gxl"
PRJ_TOML = "project.toml"
```

## 与对象层的关系

`workflow` 不直接对应某个一级 CLI 命令，它服务于 `module`、`system`、`ops_prj` 的骨架初始化，负责把默认/模板工作流落盘到 `_gal/` 和 `workflows/` 目录。
