pub mod common;
pub mod mod_cmd;
pub mod prj_cmd;
pub mod self_cmd;
pub mod sys_cmd;

use clap::Parser;
use galaxy_ops::error::MainResult;

pub use mod_cmd::{ModCmd, ModCommandHandler};
pub use prj_cmd::{PrjCmd, PrjCommandHandler};
pub use self_cmd::{SelfCmd, SelfCommandHandler};
pub use sys_cmd::{SysCmd, SysCommandHandler};

#[derive(Debug, Parser)]
#[command(name = "gops")]
#[command(
    version,
    about = "Galaxy Operations System - 系统操作管理工具",
    long_about = "Galaxy Operations System - 系统操作管理工具

用于管理系统配置、导入模块、更新引用等操作的核心工具。"
)]
pub enum GInsCmd {
    /// 工程管理命令 (Project Management Commands)
    #[command(subcommand, about = "工程管理命令 (Project Management Commands)")]
    Prj(PrjCmd),

    /// 模块管理命令 (Module Management Commands)
    #[command(subcommand, about = "模块管理命令 (Module Management Commands)")]
    Mod(ModCmd),

    /// 系统管理命令 (System Management Commands)
    #[command(subcommand, about = "系统管理命令 (System Management Commands)")]
    Sys(SysCmd),

    /// 自升级命令 (Self-Update Commands)
    #[command(name = "self", subcommand, about = "自升级命令 (Self-Update Commands)")]
    SelfUpdate(SelfCmd),
}

/// 输出模式：Machine 用于机器可读（如 `self check --json`），期间不打印版本横幅。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Human,
    Machine,
}

pub struct CommandDispatcher;

impl CommandDispatcher {
    pub fn output_mode(cmd: &GInsCmd) -> OutputMode {
        match cmd {
            GInsCmd::SelfUpdate(SelfCmd::Check(args)) if args.json => OutputMode::Machine,
            _ => OutputMode::Human,
        }
    }

    pub async fn dispatch(cmd: GInsCmd) -> MainResult<()> {
        match cmd {
            GInsCmd::Mod(mod_cmd) => ModCommandHandler::execute(mod_cmd).await,
            GInsCmd::Prj(prj_cmd) => PrjCommandHandler::execute(prj_cmd).await,
            GInsCmd::Sys(sys_cmd) => SysCommandHandler::execute(sys_cmd).await,
            GInsCmd::SelfUpdate(self_cmd) => SelfCommandHandler::execute(self_cmd).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandDispatcher, GInsCmd, OutputMode, SelfCmd};
    use clap::Parser;

    #[test]
    fn output_mode_is_machine_for_self_check_json() {
        let cmd = GInsCmd::try_parse_from(["gops", "self", "check", "--json"])
            .expect("self check --json should parse");
        assert_eq!(CommandDispatcher::output_mode(&cmd), OutputMode::Machine);
    }

    #[test]
    fn output_mode_is_human_for_plain_self_check() {
        let cmd =
            GInsCmd::try_parse_from(["gops", "self", "check"]).expect("self check should parse");
        assert_eq!(CommandDispatcher::output_mode(&cmd), OutputMode::Human);
    }

    #[test]
    fn self_subcommand_parses_status() {
        let cmd = GInsCmd::try_parse_from(["gops", "self", "status"]).expect("parse self status");
        assert!(matches!(cmd, GInsCmd::SelfUpdate(SelfCmd::Status)));
    }
}
