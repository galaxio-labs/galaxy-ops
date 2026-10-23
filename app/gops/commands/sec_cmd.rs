//! `gops sec` —— 密钥文件的只读视图、“带密钥执行”与权限修复。
//!
//! 设计约束（安全基线，见 `galaxy_ops::system::sec` 模块文档）：
//! - **不提供 `env` 子命令**：把 `export ...` 打到 stdout 供 `eval` 会成为命令注入面
//!   （值里带 `$(...)` 就会执行），而且会让密钥长期留在交互 shell 的环境里；
//! - `get` **默认掩码**，`--show` 才输出明文；
//! - `run` 用 `exec` + 进程环境注入，不经过 argv（`ps` 不可见），是唯一推荐入口；
//! - `fix` 是唯一的写入面：只 `chmod` 到 `secrets.mode`（只收缩；不改属主），不碰内容。

use clap::{Args, Parser};
use galaxy_ops::error::{MainReason, MainResult};
use galaxy_ops::infra::DfxArgsGetter;
use galaxy_ops::system::Subscriber;
use galaxy_ops::system::sec::{self, SECRET_MASK, SecFile};

use crate::commands::common::DebugLogArgs;

#[derive(Debug, Args)]
pub struct SecListArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,

    #[arg(long, help = "以 JSON 输出（只含键名与长度，不含值）")]
    pub json: bool,

    #[arg(
        long = "for",
        value_name = "run|localize|sec",
        help = "按消费方视角预览：只列 `secrets.expose.<消费方>` 实际会注入的键"
    )]
    pub for_consumer: Option<String>,
}

#[derive(Debug, Args)]
pub struct SecGetArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,

    /// 键名：`center_admin_token` 与 `SEC_CENTER_ADMIN_TOKEN` 等价。
    pub key: String,

    #[arg(long, help = "输出明文（默认输出掩码 ********）")]
    pub show: bool,
}

#[derive(Debug, Args)]
pub struct SecRunArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,

    #[arg(
        long,
        help = "使用原键名（去掉 SEC_ 前缀并转小写）：SEC_WP_GIT_PUSH_TOKEN → wp_git_push_token"
    )]
    pub raw: bool,

    #[arg(
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true,
        help = "要执行的命令；用 `--` 与 gops 自己的参数分开：gops sec run -- sh -c '...'"
    )]
    pub cmd: Vec<String>,
}

#[derive(Debug, Args)]
pub struct SecFixArgs {
    #[clap(flatten)]
    pub debug_log: DebugLogArgs,

    #[arg(long, help = "只报告将要做什么，不改权限")]
    pub dry_run: bool,
}

#[derive(Debug, Parser)]
pub enum SecCmd {
    /// 列出密钥键名与长度（List Secret Keys）
    #[command(
        about = "列出密钥键名与长度（List Secret Keys）",
        long_about = "只列键名与值长度，**不输出值**。用于排障时确认“文件里到底有哪些键、是否为空”。\n\
                     路径解析规则见 `sys-prj.yml: secrets.file`（未声明时回退 `.galaxy/sec_value.yml`）。\n\
                     Usage: gops sec list [--json]"
    )]
    List(SecListArgs),

    /// 读取单个密钥（Get Secret Value）
    #[command(
        about = "读取单个密钥（Get Secret Value）",
        long_about = "读取单个密钥。**默认输出掩码** `********`，需显式 `--show` 才输出明文，\n\
                     避免演示/录屏/误重定向时直接暴露 token。\n\
                     Usage: gops sec get <key> [--show]"
    )]
    Get(SecGetArgs),

    /// 带密钥环境变量执行命令（Run With Secrets）
    #[command(
        about = "带密钥环境变量执行命令（Run With Secrets）",
        long_about = "把密钥文件里的 `SEC_*` 注入子进程环境后 `exec` 目标命令：\n\
                     · 值只进进程环境，不进 argv（`ps` 不可见）；\n\
                     · 只影响该子进程，不会留在当前 shell。\n\
                     Usage: gops sec run [--raw] -- <cmd...>"
    )]
    Run(SecRunArgs),

    /// 把密钥文件权限修正到 secrets.mode（Fix Secret File Permissions）
    #[command(
        about = "把密钥文件权限修正到 secrets.mode（Fix Secret File Permissions）",
        long_about = "按 `sys-prj.yml: secrets.mode`（缺省 `0600`；与容器共享时 `0640`）把密钥文件的\n\
                     权限收敛到策略值：**只收缩写位与 other 位**（原本只读的保持只读；\n\
                     `0640` 会为只读文件补属组读位），只改 mode、不改属主，\n\
                     也不解链接 / 硬链接（遇到即报错）。\n\
                     路径解析与 `gops sec list` 完全一致（`secrets.file` → 回退 `.galaxy/sec_value.yml`）。\n\
                     幂等：已符合策略时不写盘。\n\
                     Usage: gops sec fix [--dry-run]"
    )]
    Fix(SecFixArgs),
}

pub struct SecCommandHandler;

impl SecCommandHandler {
    pub async fn execute(cmd: SecCmd) -> MainResult<()> {
        match cmd {
            SecCmd::List(args) => Self::handle_list(args),
            SecCmd::Get(args) => Self::handle_get(args),
            SecCmd::Run(args) => Self::handle_run(args),
            SecCmd::Fix(args) => Self::handle_fix(args),
        }
    }

    fn handle_list(args: SecListArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);
        let mut sec = Self::load()?;
        let mut scope: Option<Subscriber> = None;
        if let Some(name) = args.for_consumer.as_deref() {
            let subscriber = Subscriber::parse(name).ok_or_else(|| {
                MainReason::logic_detail(format!(
                    "未知消费方 `{name}`（可用：run / localize / sec）"
                ))
            })?;
            sec = sec.scoped(subscriber)?;
            scope = Some(subscriber);
        }
        let entries = sec.entries();

        if args.json {
            let items: Vec<serde_json::Value> = entries
                .iter()
                .map(|(key, len)| serde_json::json!({ "key": key, "len": len }))
                .collect();
            let mut payload = serde_json::json!({
                "file": sec.path().display().to_string(),
                "items": items,
            });
            if let Some(subscriber) = scope {
                payload["scope"] = serde_json::json!(subscriber.as_str());
            }
            println!("{payload}");
            return Ok(());
        }

        if let Some(subscriber) = scope {
            println!(
                "密钥文件 {}（消费方 {}）",
                sec.path().display(),
                subscriber.as_str()
            );
        }
        if entries.is_empty() {
            println!("（未加载到任何密钥：{}）", sec.path().display());
            return Ok(());
        }
        let width = entries.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        for (key, len) in entries {
            println!("{key:<width$}  len={len}");
        }
        Ok(())
    }

    fn handle_get(args: SecGetArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);
        let sec = Self::load()?;
        let value = sec.get(&args.key).ok_or_else(|| {
            MainReason::logic_detail(format!(
                "密钥文件 {} 里没有键 `{}`（用 `gops sec list` 看有哪些）",
                sec.path().display(),
                args.key
            ))
        })?;
        if args.show {
            println!("{value}");
        } else {
            println!("{SECRET_MASK}");
        }
        Ok(())
    }

    fn handle_fix(args: SecFixArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);
        let cwd = std::env::current_dir().map_err(|err| {
            MainReason::logic_detail(format!("无法获取当前目录（回到系统根再试）：{err}"))
        })?;
        let outcome = sec::fix_for_system(&cwd, args.dry_run)?;

        if !outcome.found {
            println!(
                "未找到密钥文件（{}）：未声明 secrets.file，.galaxy 下也没有 —— 无需修复",
                outcome.path.display()
            );
            return Ok(());
        }

        let policy_note = if outcome.policy == 0o640 {
            "属组可读（容器 gid 共享）"
        } else {
            "只属主可读"
        };
        println!(
            "密钥文件 {}（{}）",
            outcome.path.display(),
            if outcome.declared {
                "sys-prj.yml 声明"
            } else {
                ".galaxy 回退"
            }
        );
        println!("  策略   {:04o}（{policy_note}）", outcome.policy);

        let (Some(before), Some(after)) = (outcome.before, outcome.after) else {
            println!("  当前   非 Unix 平台：权限修正跳过");
            return Ok(());
        };

        // `0640` 的“能读”靠属组：本工具不改属组，就把现值摆出来让人自己核对。
        if let (0o640, Some(gid)) = (outcome.policy, outcome.gid) {
            println!("  属组   {gid}（0640 需等于容器 gid；本工具不改属组）");
        }

        if !outcome.changed {
            println!("  当前   {before:04o} —— 已符合策略，无需改动");
        } else if args.dry_run {
            println!("  当前   {before:04o} → 将改为 {after:04o}（dry-run，未改动）");
        } else {
            println!("  当前   {before:04o} → {after:04o}（已修复）");
            // 权限修好不等同于“能用”：内容问题（非字符串值、非法键名）不在 fix 范围内，
            // 但不应让人拿着一个“已修复”去撞下一个报错。
            if let Err(err) = sec::load_for_system(&cwd) {
                println!("  注意   权限已修复，但内容校验仍失败：{err}");
            }
        }
        Ok(())
    }

    fn handle_run(args: SecRunArgs) -> MainResult<()> {
        galaxy_ops::infra::configure_dfx_logging(&args);
        // `gops sec run` 是显式操作，缺省注入全量（`expose.sec` 可收窄）。
        let sec = Self::load()?.scoped(Subscriber::Sec)?;

        let program = args
            .cmd
            .first()
            .ok_or_else(|| MainReason::logic_detail("用法：gops sec run [--raw] -- <cmd...>"))?;
        let mut command = std::process::Command::new(program);
        command.args(&args.cmd[1..]);
        for (name, value) in sec.env_pairs() {
            let name = if args.raw {
                SecFile::raw_name(&name)
            } else {
                name
            };
            command.env(name, value);
        }

        // Unix：直接 exec 替换当前进程 —— 不留父进程、不留 argv、退出码就是子进程的。
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let err = command.exec();
            Err(MainReason::logic_detail(format!(
                "无法执行 `{program}`：{err}"
            )))
        }
        #[cfg(not(unix))]
        {
            match command.status() {
                Ok(status) => std::process::exit(status.code().unwrap_or(1)),
                Err(err) => Err(MainReason::logic_detail(format!(
                    "无法执行 `{program}`：{err}"
                ))),
            }
        }
    }

    fn load() -> MainResult<SecFile> {
        let cwd = std::env::current_dir().map_err(|err| {
            MainReason::logic_detail(format!("无法获取当前目录（回到系统根再试）：{err}"))
        })?;
        sec::load_for_system(&cwd)
    }
}

impl DfxArgsGetter for SecListArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

impl DfxArgsGetter for SecGetArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

impl DfxArgsGetter for SecRunArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

impl DfxArgsGetter for SecFixArgs {
    fn debug_level(&self) -> usize {
        self.debug_log.debug_level()
    }
    fn log_setting(&self) -> Option<String> {
        self.debug_log.log_setting()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_list_and_get() {
        assert!(matches!(
            SecCmd::try_parse_from(["sec", "list", "--json"]).unwrap(),
            SecCmd::List(a) if a.json
        ));
        assert!(matches!(
            SecCmd::try_parse_from(["sec", "get", "center_admin_token"]).unwrap(),
            SecCmd::Get(a) if !a.show && a.key == "center_admin_token"
        ));
        assert!(matches!(
            SecCmd::try_parse_from(["sec", "get", "k", "--show"]).unwrap(),
            SecCmd::Get(a) if a.show
        ));
    }

    #[test]
    fn parses_fix_with_dry_run() {
        assert!(matches!(
            SecCmd::try_parse_from(["sec", "fix"]).unwrap(),
            SecCmd::Fix(a) if !a.dry_run
        ));
        assert!(matches!(
            SecCmd::try_parse_from(["sec", "fix", "--dry-run"]).unwrap(),
            SecCmd::Fix(a) if a.dry_run
        ));
    }

    #[test]
    fn parses_run_with_leading_dash_args() {
        let cmd = SecCmd::try_parse_from(["sec", "run", "--", "sh", "-c", "echo $X"]).unwrap();
        match cmd {
            SecCmd::Run(a) => {
                assert_eq!(a.cmd, vec!["sh", "-c", "echo $X"]);
                assert!(!a.raw);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parses_run_raw() {
        let cmd = SecCmd::try_parse_from(["sec", "run", "--raw", "--", "env"]).unwrap();
        assert!(matches!(cmd, SecCmd::Run(a) if a.raw && a.cmd == vec!["env"]));
    }

    #[test]
    fn run_requires_command() {
        assert!(SecCmd::try_parse_from(["sec", "run"]).is_err());
    }

    /// `--` 可以省略：`trailing_var_arg` 从第一个位置参数开始原样收集。
    #[test]
    fn parses_run_without_double_dash() {
        let cmd = SecCmd::try_parse_from(["sec", "run", "ls", "-la"]).unwrap();
        assert!(matches!(cmd, SecCmd::Run(a) if a.cmd == vec!["ls", "-la"]));
    }

    #[test]
    fn get_requires_key() {
        assert!(SecCmd::try_parse_from(["sec", "get"]).is_err());
    }

    #[test]
    fn list_defaults_to_human_output() {
        assert!(matches!(
            SecCmd::try_parse_from(["sec", "list"]).unwrap(),
            SecCmd::List(a) if !a.json
        ));
    }

    #[test]
    fn parses_list_for_consumer() {
        let cmd = SecCmd::try_parse_from(["sec", "list", "--for", "localize"]).unwrap();
        assert!(matches!(cmd, SecCmd::List(a) if a.for_consumer.as_deref() == Some("localize")));
        let bad = SecCmd::try_parse_from(["sec", "list", "--for", "everything"]);
        // 取值域由运行时校验（报错信息里给出可用值）
        assert!(bad.is_ok());
    }

    #[test]
    fn parses_list_for_with_json() {
        let cmd = SecCmd::try_parse_from(["sec", "list", "--for", "run", "--json"]).unwrap();
        assert!(
            matches!(cmd, SecCmd::List(a) if a.json && a.for_consumer.as_deref() == Some("run"))
        );
    }
}
