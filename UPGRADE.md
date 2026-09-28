# 升级迁移指南

本文档面向从旧版 `galaxy-ops` 升级到当前版本的调用方，重点说明这次依赖升级带来的 API 迁移点，以及推荐的落地方式。

## docker-compose 文件位置变更（1.3.3）

docker-compose 系统的 compose 文件从系统**根目录**改为默认放在 **`sys/docker-compose.yaml`**（compose 属于「系统定义」，与 `sys/` 下其它定义同源）。

- `gops sys new --kind docker-compose` 现在生成到 `sys/docker-compose.yaml`。
- `gops sys` 的查找链：`sys/{compose,docker-compose}.{yaml,yml}` → `<root>/{compose,docker-compose}.{yaml,yml}`（`sys/` 优先；`compose.*` 与 docker 自身发现优先级一致）。
- **语义不变**：无论文件放在哪，项目目录都锚定在系统根——项目名 = 系统根目录名，相对挂载与 `.env` 都相对系统根。
- **向后兼容**：旧系统的 compose 保持放在根目录即可继续运行，无需改动；如需内收，直接 `git mv docker-compose.yml sys/docker-compose.yaml`。
- **`.env` 位置不变**：仍在系统根。

行为变更提醒：`kind: gxl` 系统的脚手架也会在 `sys/docker-compose.yaml` 生成一份示例 compose（旧版写 `<root>/docker-compose.yml`）。

## 迁移原则

- 不回退到旧版 `orion_*` 依赖。
- 内部实现统一使用上游新 trait 和新方法名。
- 对外仍保留 `galaxy_ops::compat::*` 作为过渡桥接，但这些旧名字已经标记为 `deprecated`。
- 新代码不要继续扩散旧命名，优先直接使用 `orion_conf` 当前版本 API。

## 依赖升级

- `orion-error` 升级到 `0.6`
- `orion_conf` 升级到 `0.5`
- `orion-infra` 升级到 `0.5`
- `orion-accessor` 通过别名 `orion_variate` 升级到 `0.6`
- `orion-variate` 通过别名 `orion_vars` 升级到 `0.11`

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

### 示例

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

建议：

- 外部项目短期内可以继续 `use galaxy_ops::compat::*`，或者继续使用旧的 `galaxy_ops::prelude::*` 入口先完成版本升级。
- 中期应逐步替换到 `orion_conf::*` 原生 trait。
- 新代码不要继续依赖这些旧名称；如果同时需要旧名和新名，请优先直接显式导入 `orion_conf::*`，避免方法解析冲突。

## 错误处理语义

这次升级不再用“压平错误”方式适配新依赖。

重点变化：

- accessor 下载失败会保留原始 `AddrReason`
- `MainReason` 新增 `Accessor(AddrReason)` 路径
- `detail` / `position` / `context` 会尽量原样保留
- `module/system/ops` 更新链路不再统一压成 `*Reason::Update`

如果你之前依赖的是笼统的 `Update` 错误分类，需要改成同时处理更细的 `MainReason` 分支。

## 模板与本地化

模板渲染逻辑也已经收敛到新语义：

- 不再在渲染失败时静默 fallback 到默认 renderer
- `value_file` 缺失会返回显式资源错误
- shell / YAML 注释解析的兼容逻辑已经补齐对应测试

如果你之前依赖“失败后自动降级渲染”的行为，需要在业务层显式决定 fallback 策略。

## 变量访问

大小写不敏感读取统一改为：

```rust
dict.get_case_insensitive("key")
```

不要再继续使用旧的 `ucase_get()`。

## 加载后初始化

`LoadHook` 只是新版本上游的 trait 名，不代表上游 `load_*` 会自动调用 hook。

在 `galaxy-ops` 内部，像 `SysSetting::load_from()` 这种确实依赖加载后初始化的路径，已经改为显式调用 `loaded_event_do()`。

如果你的下游类型也依赖加载后修正状态，不要假设上游 `load_conf/load_yaml` 会自动触发 hook，应该在自己的封装入口里显式处理。

## 建议迁移顺序

1. 先升级依赖版本，确保项目可编译。
2. 把 `from_*` / `save_yml` / `Persistable` 等旧命名替换为新接口。
3. 把 `ucase_get()` 统一替换为 `get_case_insensitive()`。
4. 复查错误处理，确认没有把新的细粒度错误又包回旧的大类错误。
5. 最后再移除对 `galaxy_ops::compat::*` 的依赖。
