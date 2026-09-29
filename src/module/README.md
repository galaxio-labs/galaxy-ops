# module 模块说明

`src/module/` 对应 `gops mod` 这一组能力。

## 目标

模块层负责定义最小可复用运维单元。

它解决的是：

- 模块骨架如何初始化
- 模块依赖和引用如何维护
- 模块如何按 `ModelSTD` 组织
- 模块如何做本地化

## 当前目录

```text
src/module/
├── depend.rs
├── init.rs
├── model.rs
├── operator.rs
├── refs.rs
├── spec.rs
├── prelude.rs
└── README.md
```

## 主要文件

### `operator.rs`

模块对象入口。`gops mod new`、`update`、`localize` 的核心流程都会落到这里。

### `spec.rs`

模块规范与模板初始化相关逻辑。

### `model.rs`

模块模型相关结构，围绕 `ModelSTD` 组织模块内容。

### `refs.rs`

模块引用与更新相关逻辑。

### `depend.rs`

模块依赖相关逻辑。

### `init.rs`

模块初始化模板和骨架内容。

## 与 CLI 的对应

```text
gops mod example
gops mod new
gops mod update
gops mod localize
gops mod diff [--json]
```

`gops mod diff` 按模型比对「模块默认值」（`mod/<model>/vars.yml`，`origin=mod-default`）与生效值，逐键列出 `INITIAL` / `EFFECTIVE` / `ORIGIN` / `MUTABILITY` / `STATE`；`localize` 末尾也会打印同一张表。

## 输出对象

当前模块层最终管理的是这种对象结构：

```text
<module-root>/
├── mod-prj.yml
├── _gal/
└── mod/<model>/...
```

## 关系

模块不是交付终点。

它的上游关系是：

```text
module -> system -> ops_prj
```
