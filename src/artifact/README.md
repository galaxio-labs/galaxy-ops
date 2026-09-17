# artifact 模块文档

`src/artifact/` 负责构件（artifact）与资源下载相关的结构。它描述"从哪里获取一个构件、如何把它下载到本地"，供模块、系统、项目在 `update` / `import` / `download` 等场景下使用。

> 注意：本模块不是包管理系统。不包含仓库、依赖解析、校验和、签名验证等能力。包/构件地址的解析见 `types.rs`，下载逻辑见 `core.rs`。

## 当前目录

```text
src/artifact/
├── core.rs
├── package.rs
├── types.rs
└── README.md
```

## 主要结构

### `Artifact`（`core.rs`）

描述一个可下载的构件：

```rust
pub struct Artifact {
    name: String,             // 构件名称
    version: String,          // 版本
    origin_addr: Address,     // 来源地址（git / http / local）
    cache_addr: Option<Address>, // 可选缓存地址
    cache_enable: bool,       // 是否启用缓存
    local: String,            // 本地路径
}
```

关键方法：

```rust
impl Artifact {
    pub fn new(name, version, addr, local) -> Self;
    // 从来源地址下载到本地目标路径
    pub async fn deploy_repo_to_local(&self, accessor, dest_path, options) -> AddrResult<UpdateUnit>;
}
```

`deploy_repo_to_local` 通过 `accessor` 把 `origin_addr` 指向的资源下载并重命名到 `dest_path`。

### `ArtifactPackage`（`package.rs`）

`Vec<Artifact>` 的透明包装，用 `#[serde(transparent)]` 直接序列化为数组：

```rust
pub struct ArtifactPackage {
    items: Vec<Artifact>,
}
```

实现了 `Deref` / `DerefMut` 指向 `Vec<Artifact>`，因此可直接使用 `len()` / `push()` / 索引等 Vec 方法。

### `PackageType` / 地址解析（`types.rs`）

把一段 URL 或本地路径解析为统一的 `Address` 类型：

```rust
pub enum PackageType {
    Bin(BinPackage),   // 二进制/归档包（.tar.gz）
    Git(GitPackage),   // Git 仓库
}
```

核心函数：

```rust
// 把输入字符串解析为 Address
pub fn convert_addr(input: &str) -> Address;
// 把输入字符串解析为 PackageType（Bin / Git）
pub fn build_pkg(input: &str) -> PackageType;
```

支持的输入形式（参见 `types.rs` 中的示例）：

- 本地归档：`/Users/dayu/ds-build/mac-devkit-0.1.5.tar.gz` → `Address::Local`
- HTTPS Git：`https://github.com/galaxio-labs/galaxy-flow.git` → `Address::Git`
- SSH Git：`git@github.com:galaxio-labs/galaxy-flow.git` → `Address::Git`
- HTTPS 归档：`https://.../xxx.tar.gz` → `Address::Http`

## 与对象层的关系

`artifact` 不直接对应某个一级 CLI 命令，它服务于 `module`、`system`、`ops_prj` 的更新/导入/下载流程，是"构件地址表达 + 下载"的公共底座。
