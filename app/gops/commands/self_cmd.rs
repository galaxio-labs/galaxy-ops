use clap::{ArgAction, Args, Parser};
use derive_getters::Getters;
use galaxy_ops::error::{MainReason, MainResult};
use galaxy_ops::prelude::{ToStructError, UvsReason};
use galaxy_ops::self_update::{
    CheckRequest, CheckResult, ReleaseChannel, SelfUpdateService, UpdateRequest,
};
use galaxy_ops::skills::{InstallRequest, SkillPlatform, SkillService, SkillSource, SkillTarget};
use std::io::IsTerminal;
use std::path::PathBuf;
use wp_self_update::{VersionRelation, compare_versions_str, relation_message};

#[derive(Debug, Parser)]
pub enum SelfCmd {
    /// 查看当前版本、安装目录与最近一次自升级状态 (Show self-update status)
    Status,

    /// 检查指定通道是否有新版本 (Check for a newer version)
    Check(SelfCheckArgs),

    /// 升级 gops 到指定通道的最新版本 (Update gops to the latest release)
    Update(SelfUpdateArgs),

    /// 回滚到最近一次升级前的版本 (Rollback to the previous version)
    Rollback(SelfRollbackArgs),

    /// 管理 agent skills（安装 / 列出）(Manage agent skills: install / list)
    #[command(
        subcommand,
        about = "管理 agent skills（安装 / 列出）(Manage agent skills)"
    )]
    Skill(SkillCmd),
}

#[derive(Debug, Parser)]
pub enum SkillCmd {
    /// 安装 skills 到 agent skills 目录 (Install skills into agent skill dirs)
    Install(SkillInstallArgs),

    /// 列出来源仓库中可安装的 skills (List installable skills)
    List(SkillListArgs),
}

#[derive(Debug, Args, Clone, Getters)]
pub struct SkillInstallArgs {
    /// `skills/` 下的 skill 名；缺省安装整包（顶层路由 + `skills/`）
    pub skill: Option<String>,

    /// 来源：`owner/repo`、git URL，或本地目录
    #[arg(long, default_value = "galaxio-labs/gops-skills")]
    pub source: String,

    /// 分支或标签
    #[arg(long = "ref", default_value = "main")]
    pub git_ref: String,

    /// 目标平台：`codex|claude|zed|all`，可重复
    #[arg(long, action = ArgAction::Append)]
    pub target: Vec<String>,

    /// 自定义目标目录，可重复
    #[arg(long, action = ArgAction::Append)]
    pub dir: Vec<PathBuf>,

    /// 用符号链接替代复制（仅本地来源）
    #[arg(long, action = ArgAction::SetTrue, default_value = "false")]
    pub symlink: bool,

    /// 跳过覆盖确认
    #[arg(long, action = ArgAction::SetTrue, default_value = "false")]
    pub yes: bool,
}

#[derive(Debug, Args, Clone, Getters)]
pub struct SkillListArgs {
    /// 来源：`owner/repo`、git URL，或本地目录
    #[arg(long, default_value = "galaxio-labs/gops-skills")]
    pub source: String,

    /// 分支或标签
    #[arg(long = "ref", default_value = "main")]
    pub git_ref: String,
}

#[derive(Debug, Args, Clone, Getters)]
pub struct SelfCheckArgs {
    #[arg(long, default_value = "stable")]
    pub channel: String,

    #[arg(long, action = ArgAction::SetTrue, default_value = "false")]
    pub json: bool,
}

#[derive(Debug, Args, Clone, Getters)]
pub struct SelfUpdateArgs {
    #[arg(long, default_value = "stable")]
    pub channel: String,

    #[arg(long = "to")]
    pub to_version: Option<String>,

    #[arg(long, action = ArgAction::SetTrue, default_value = "false")]
    pub yes: bool,

    #[arg(long = "dry-run", action = ArgAction::SetTrue, default_value = "false")]
    pub dry_run: bool,

    #[arg(long, action = ArgAction::SetTrue, default_value = "false")]
    pub force: bool,
}

#[derive(Debug, Args, Clone, Getters)]
pub struct SelfRollbackArgs {
    #[arg(long = "id")]
    pub backup_id: Option<String>,
}

/// `gops self` 的处理器。
pub struct SelfCommandHandler;

impl SelfCommandHandler {
    pub async fn execute(cmd: SelfCmd) -> MainResult<()> {
        match cmd {
            SelfCmd::Status => {
                let svc = SelfUpdateService::new()?;
                let status = svc.status()?;
                println!("current_version={}", status.current_version);
                println!("install_dir={}", status.install_dir.display());
                if let Some(v) = status.state.last_remote_version {
                    println!("state.last_remote_version={v}");
                }
                if let Some(v) = status.state.last_result {
                    println!("state.last_result={v}");
                }
                if let Some(v) = status.state.last_error {
                    println!("state.last_error={v}");
                }
            }
            SelfCmd::Check(args) => {
                let svc = SelfUpdateService::new()?;
                let channel = parse_channel(args.channel.as_str())?;
                let out = svc.check(CheckRequest { channel }).await?;
                if args.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "channel": out.channel.as_str(),
                            "current_version": out.current_version,
                            "remote_version": out.remote_version,
                            "has_update": out.has_update
                        }))
                        .map_err(|e| MainReason::resource_detail(e.to_string()))?
                    );
                } else {
                    print_self_check_report(&out)?;
                }
            }
            SelfCmd::Update(args) => {
                let svc = SelfUpdateService::new()?;
                let channel = parse_channel(args.channel.as_str())?;
                let req = UpdateRequest {
                    channel,
                    to_version: args.to_version.clone(),
                    yes: args.yes,
                    dry_run: args.dry_run,
                    force: args.force,
                };
                let out = svc.update(req).await?;
                println!("channel={}", out.channel.as_str());
                println!("from={}", out.from_version);
                println!("to={}", out.to_version);
                println!("updated={}", out.updated);
                if let Some(id) = out.backup_id {
                    println!("backup_id={id}");
                }
            }
            SelfCmd::Rollback(args) => {
                let svc = SelfUpdateService::new()?;
                let out = svc.rollback(args.backup_id.as_deref())?;
                println!("rollback=true");
                if let Some(id) = out.backup_id {
                    println!("backup_id={id}");
                }
            }
            SelfCmd::Skill(cmd) => {
                execute_skill(cmd)?;
            }
        }
        Ok(())
    }
}

/// `gops self skill` 的处理器。
fn execute_skill(cmd: SkillCmd) -> MainResult<()> {
    let svc = SkillService::new()?;
    match cmd {
        SkillCmd::Install(args) => {
            let source = SkillSource::parse(&args.source, &args.git_ref)?;
            let source_desc = source.describe();
            let req = InstallRequest {
                source,
                skill: args.skill.clone(),
                targets: parse_skill_targets(&args.target, &args.dir)?,
                symlink: args.symlink,
                yes: args.yes,
            };
            let report = svc.install(&req)?;
            println!("Source:    {source_desc}");
            println!("Name:      {}", report.name);
            println!("Validated: {} SKILL.md", report.skill_files.len());
            for loc in &report.installed {
                println!("Installed: {}", loc.dir.display());
                println!("Platform:  {}", loc.platform);
            }
        }
        SkillCmd::List(args) => {
            let source = SkillSource::parse(&args.source, &args.git_ref)?;
            let names = svc.list(&source)?;
            println!("Source: {}", source.describe());
            if names.is_empty() {
                println!("(no skills found)");
            } else {
                for name in names {
                    println!("  {name}");
                }
            }
        }
    }
    Ok(())
}

/// 把 `--target` / `--dir` 解析成安装目标；`all` 展开为全部平台。
fn parse_skill_targets(targets: &[String], dirs: &[PathBuf]) -> MainResult<Vec<SkillTarget>> {
    let mut out = Vec::new();
    for raw in targets {
        if raw.trim().eq_ignore_ascii_case("all") {
            out.extend(SkillPlatform::ALL.into_iter().map(SkillTarget::Platform));
            continue;
        }
        let platform = SkillPlatform::parse(raw).ok_or_else(|| {
            MainReason::Uvs(UvsReason::validation_error())
                .to_err()
                .with_detail(format!("--target={raw}, expected=codex|claude|zed|all"))
        })?;
        out.push(SkillTarget::Platform(platform));
    }
    for dir in dirs {
        out.push(SkillTarget::Dir(dir.clone()));
    }
    Ok(out)
}

fn parse_channel(input: &str) -> MainResult<ReleaseChannel> {
    ReleaseChannel::parse(input).ok_or_else(|| {
        MainReason::Uvs(UvsReason::validation_error())
            .to_err()
            .with_detail(format!("channel={input}, expected=stable|alpha|beta"))
    })
}

fn print_self_check_report(out: &CheckResult) -> MainResult<()> {
    print!("{}", format_self_check_report(out, use_color())?);
    Ok(())
}

fn format_self_check_report(out: &CheckResult, use_color: bool) -> MainResult<String> {
    let relation =
        compare_versions_str(&out.current_version, &out.remote_version).map_err(|e| {
            MainReason::resource_detail(format!(
                "compare self-update versions failed: current={}, remote={}, error={}",
                out.current_version, out.remote_version, e
            ))
        })?;

    let mut lines = vec![
        "Self-check result".to_string(),
        format!(
            "  Channel  : {}",
            render_channel(out.channel.as_str(), use_color)
        ),
        format!("  Current  : {}", out.current_version),
        format!(
            "  Remote   : {}",
            render_remote_version(&out.remote_version, relation, use_color)
        ),
        format!(
            "  Status   : {}",
            render_relation_message(relation, use_color)
        ),
    ];

    if relation == VersionRelation::UpdateAvailable {
        lines.push(format!(
            "  Action   : gops self update --channel {} --yes",
            out.channel.as_str()
        ));
    }

    Ok(format!("{}\n", lines.join("\n")))
}

fn use_color() -> bool {
    if !std::io::stdout().is_terminal() {
        return false;
    }
    // NO_COLOR 仅在非空值时才禁用颜色（遵循 spec）
    match std::env::var_os("NO_COLOR") {
        Some(v) => v.is_empty(),
        None => true,
    }
}

fn render_channel(channel: &str, use_color: bool) -> String {
    if !use_color {
        return channel.to_string();
    }
    let code = match channel {
        "stable" => "32",
        "beta" => "33",
        "alpha" => "35",
        _ => return channel.to_string(),
    };
    format!("\x1b[{code}m{channel}\x1b[0m")
}

fn render_remote_version(version: &str, relation: VersionRelation, use_color: bool) -> String {
    if !use_color {
        return version.to_string();
    }
    match relation {
        VersionRelation::UpdateAvailable => format!("\x1b[1;92m{version}\x1b[0m"),
        VersionRelation::AheadOfChannel => format!("\x1b[90m{version}\x1b[0m"),
        _ => version.to_string(),
    }
}

fn render_relation_message(relation: VersionRelation, use_color: bool) -> String {
    let message = relation_message(relation);
    if !use_color {
        return message.to_string();
    }
    match relation {
        VersionRelation::UpdateAvailable => format!("\x1b[1;92m{message}\x1b[0m"),
        VersionRelation::AheadOfChannel => format!("\x1b[90m{message}\x1b[0m"),
        VersionRelation::UpToDate => format!("\x1b[32m{message}\x1b[0m"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SelfCheckArgs, SelfCmd, SkillCmd, format_self_check_report, parse_channel,
        parse_skill_targets, render_relation_message,
    };
    use clap::Parser;
    use galaxy_ops::self_update::{CheckResult, ReleaseChannel};
    use std::path::PathBuf;
    use wp_self_update::VersionRelation;

    #[test]
    fn parse_self_check_defaults_channel_to_stable() {
        let cmd = SelfCmd::try_parse_from(["self", "check"]).expect("self check should parse");
        match cmd {
            SelfCmd::Check(args) => {
                assert_eq!(args.channel, "stable");
                assert!(!args.json);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_self_update_defaults_channel_to_stable() {
        let cmd = SelfCmd::try_parse_from(["self", "update"]).expect("self update should parse");
        match cmd {
            SelfCmd::Update(args) => {
                assert_eq!(args.channel, "stable");
                assert_eq!(args.to_version, None);
                assert!(!args.yes);
                assert!(!args.dry_run);
                assert!(!args.force);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_channel_rejects_invalid() {
        assert!(parse_channel("gamma").is_err());
        assert_eq!(parse_channel("alpha").unwrap(), ReleaseChannel::Alpha);
    }

    #[test]
    fn report_shows_update_action_when_available() {
        let out = CheckResult {
            channel: ReleaseChannel::Alpha,
            current_version: "2.0.0".to_string(),
            remote_version: "2.0.1-alpha".to_string(),
            has_update: true,
        };
        let rendered = format_self_check_report(&out, false).expect("render");
        assert!(rendered.contains("Self-check result"));
        assert!(rendered.contains("gops self update --channel alpha --yes"));
    }

    #[test]
    fn report_marks_up_to_date_without_action() {
        let out = CheckResult {
            channel: ReleaseChannel::Stable,
            current_version: "2.0.1".to_string(),
            remote_version: "2.0.1".to_string(),
            has_update: false,
        };
        let rendered = format_self_check_report(&out, false).expect("render");
        assert!(!rendered.contains("Action   :"));
    }

    #[test]
    fn check_args_getters() {
        let args = SelfCheckArgs {
            channel: "beta".to_string(),
            json: true,
        };
        assert_eq!(args.json(), &true);
    }

    #[test]
    fn report_has_ansi_when_color_enabled() {
        let out = CheckResult {
            channel: ReleaseChannel::Alpha,
            current_version: "2.0.0".to_string(),
            remote_version: "2.0.1-alpha".to_string(),
            has_update: true,
        };
        let rendered = format_self_check_report(&out, true).expect("render");
        assert!(rendered.contains('\u{1b}'));
    }

    #[test]
    fn relation_message_is_plain_without_color() {
        for relation in [
            VersionRelation::UpdateAvailable,
            VersionRelation::AheadOfChannel,
            VersionRelation::UpToDate,
        ] {
            assert!(!render_relation_message(relation, false).contains('\u{1b}'));
        }
    }

    #[test]
    fn parse_self_skill_install_defaults() {
        let cmd = SelfCmd::try_parse_from(["self", "skill", "install"]).expect("parse");
        match cmd {
            SelfCmd::Skill(SkillCmd::Install(args)) => {
                assert_eq!(args.source, "galaxio-labs/gops-skills");
                assert_eq!(args.git_ref, "main");
                assert!(args.skill.is_none());
                assert!(args.target.is_empty());
                assert!(args.dir.is_empty());
                assert!(!args.symlink);
                assert!(!args.yes);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_self_skill_install_flags() {
        let cmd = SelfCmd::try_parse_from([
            "self",
            "skill",
            "install",
            "gops-engineering",
            "--target",
            "zed",
            "--dir",
            "/tmp/skills",
            "--symlink",
            "--yes",
        ])
        .expect("parse");
        match cmd {
            SelfCmd::Skill(SkillCmd::Install(args)) => {
                assert_eq!(args.skill.as_deref(), Some("gops-engineering"));
                assert_eq!(args.target, vec!["zed".to_string()]);
                assert_eq!(args.dir, vec![PathBuf::from("/tmp/skills")]);
                assert!(args.symlink);
                assert!(args.yes);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_self_skill_list() {
        let cmd = SelfCmd::try_parse_from(["self", "skill", "list", "--ref", "v1"]).expect("parse");
        match cmd {
            SelfCmd::Skill(SkillCmd::List(args)) => assert_eq!(args.git_ref, "v1"),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_skill_targets_expands_all_plus_explicit_and_dirs() {
        let dir = PathBuf::from("/tmp/x");
        let targets = parse_skill_targets(
            &["all".to_string(), "zed".to_string()],
            std::slice::from_ref(&dir),
        )
        .expect("parse");
        // `all` 展开为 3 个平台，再加显式 `zed` 与 1 个自定义目录（目录去重留给服务层）
        assert_eq!(targets.len(), 5);
    }

    #[test]
    fn parse_skill_targets_rejects_unknown() {
        assert!(parse_skill_targets(&["vscode".to_string()], &[]).is_err());
    }
}
