//! `gops self` 自升级：查询 / 检查 / 升级 / 回滚。
//!
//! 与 `gx self` 对齐：状态与备份存放在 `~/.galaxy/self_update/gops`，制品清单来自
//! `galaxio-labs/get` 的 `updates/gops` 通道。实际的下载/校验/安装复用 `wp-self-update`。

mod model;
mod prelude;
mod rollback;
mod service;
mod storage;

pub use model::{CheckResult, ReleaseChannel, SelfUpdateState, StatusResult, UpdateResult};
pub use service::{CheckRequest, SelfUpdateService, UpdateRequest};
