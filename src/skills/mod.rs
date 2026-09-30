//! `gops self skill`：安装 / 列出 agent skills。
//!
//! 与 `gops-skills/install.sh` 对齐，但用内置实现（`git2` 浅 clone + `serde_yaml`
//! 校验 frontmatter），不依赖外部的 `git` / `python3` / `ruby`。

mod installer;
mod model;
mod prelude;

pub use installer::{DEFAULT_COLLECTION_NAME, InstallRequest, SkillService};
pub use model::{ResolvedTarget, SkillInstallReport, SkillPlatform, SkillSource, SkillTarget};
