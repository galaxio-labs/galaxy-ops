# 库 / 开发者迁移（API）

面向把 `galaxy-ops` 作为**库依赖**的调用方（不是使用 `gops` CLI 的交付/运维人员——那部分见 [UPGRADE.md](./UPGRADE.md)）。
本文件说明 2.0 的依赖升级与库 API / 行为迁移。

## 迁移原则

- 不回退到旧版 `orion_*` 依赖。
- 内部实现统一使用上游新 trait 和新方法名。
- 对外仍保留 `galaxy_ops::compat::*` 作为过渡桥接，但这些旧名字已标记为 `deprecated`。
- 新代码不要继续扩散旧命名，优先直接使用 `orion_conf` 当前版本 API。

## 依赖升级

- `orion-error` → `0.8`
- `orion_conf` → `0.7`
- `orion-infra` → `0.7`
- `orion-accessor`（别名 `orion_variate`）→ `0.8`
- `orion-variate`（别名 `orion_vars`）→ `0.13`
- 新增 `orion-sec`（`0.6`，读取 `~/.galaxy/sec_value.yml` 注入 `${SEC_xxx}`）与 `wp-self-update`（`0.3`，支撑 `gops self`）

## 配置读写 API 迁移

旧接口仍可通过 `galaxy_ops::compat::*` 使用，但推荐直接迁到 `orion_conf` 新 trait。

| 旧写法 | 新写法 |
| --- | --- |
| `Configable` | `orion_conf::ConfigIO` |
| `JsonAble` | `orion_conf::JsonIO` |
| `Yamlable` | `orion_conf::YamlIO` |
| `ValueConfable` | `orion_conf::TextConfigIO` |
| `Persistable` | `orion_conf::FilePersist` |
| `StorageLoadEvent` | `orion_conf::LoadHook` |
| `from_conf()` | `load_conf()` |
| `from_json()` | `load_json()` |
| `from_yml()` | `load_yaml()` |
| `save_yml()` | `save_yaml()` |

旧写法：

```rust
use galaxy_ops::compat::{Configable, Yamlable};

let setting = Setting::from_conf(path)?;
setting.save_yml(out)?;
```

新写法：

```rust
use orion_conf::{ConfigIO, YamlIO};

let setting = Setting::load_conf(path)?;
setting.save_yaml(out)?;
```

## `compat` 模块的定位

当前版本重新保留了 `galaxy_ops::compat`，并恢复了 `galaxy_ops::prelude::*` 中的旧名称导出，用于降低外部升级时的源码级 break 风险。

- 短期：外部项目可继续 `use galaxy_ops::compat::*`，或继续用旧的 `galaxy_ops::prelude::*` 入口先完成版本升级。
- 中期：逐步替换到 `orion_conf::*` 原生 trait。
- 新代码不要再依赖旧名称；若同时需要旧名和新名，优先显式导入 `orion_conf::*`，避免方法解析冲突。

## 错误处理语义

这次升级不再用「压平错误」的方式适配新依赖：

- accessor 下载失败会保留原始 `AddrReason`；
- `MainReason` 新增 `Accessor(AddrReason)` 路径；
- `detail` / `position` / `context` 尽量原样保留；
- `module` / `system` / `ops` 更新链路不再统一压成 `*Reason::Update`。

如果之前依赖笼统的 `Update` 分类，需要改成同时处理更细的 `MainReason` 分支。

## 变量访问

大小写不敏感读取统一改为：

```rust
dict.get_case_insensitive("key")
```

不要再使用旧的 `ucase_get()`。

## 模板与本地化

- 不再在渲染失败时静默 fallback 到默认 renderer；
- `value_file` 缺失会返回显式资源错误；
- shell / YAML 注释解析的兼容逻辑已补齐测试。

如果之前依赖「失败后自动降级渲染」，需要在业务层显式决定 fallback 策略。

## 加载后初始化

`LoadHook` 只是上游的 trait 名，不代表上游 `load_*` 会自动调用 hook。

`galaxy-ops` 内部像 `SysSetting::load_from()` 这种确实依赖加载后初始化的路径，已改为显式调用 `loaded_event_do()`。若你的下游类型也依赖加载后修正状态，不要假设上游 `load_conf` / `load_yaml` 会自动触发 hook，应在自己的封装入口里显式处理。

## 建议迁移顺序

1. 升级依赖版本，确保项目可编译。
2. 把 `from_*` / `save_yml` / `Persistable` 等旧命名替换为新接口。
3. 把 `ucase_get()` 统一替换为 `get_case_insensitive()`。
4. 复查错误处理，确认没有把新的细粒度错误又包回旧的大类错误。
5. 最后再移除对 `galaxy_ops::compat::*` 的依赖。
