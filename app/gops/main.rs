extern crate clap;
extern crate log;
mod commands;

use clap::Parser;
use commands::{CommandDispatcher, GInsCmd};
use galaxy_ops::error::{MainResult, report_error};
use galaxy_ops::prelude::ErrorOwe;
use orion_vars::vars::setup_start_env_vars;

#[tokio::main]
async fn main() {
    use std::process;
    match GxOps::run().await {
        Err(e) => report_error(e),
        Ok(_) => {
            return;
        }
    }
    process::exit(-1);
}

pub struct GxOps {}

impl GxOps {
    pub async fn run() -> MainResult<()> {
        setup_start_env_vars().source_resource()?;
        let cmd = GInsCmd::parse();
        println!("gops: {}", env!("CARGO_PKG_VERSION"));
        CommandDispatcher::dispatch(cmd).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[test]
    fn test_command_structure() {
        // Test that all expected commands are available
        let app = GInsCmd::command();
        let subcommands = app.get_subcommands();
        let subcommands_vec: Vec<&clap::Command> = subcommands.collect();

        let mut found_mod = false;
        let mut found_sys = false;
        let mut found_prj = false;

        for subcommand in &subcommands_vec {
            match subcommand.get_name() {
                "mod" => found_mod = true,
                "sys" => found_sys = true,
                "prj" => found_prj = true,
                _ => {}
            }
        }

        assert!(found_mod, "Mod subcommand should be available");
        assert!(found_sys, "Sys subcommand should be available");
        assert!(found_prj, "Prj subcommand should be available");

        // Verify no other subcommands exist
        let expected_commands = vec!["mod", "sys", "prj"];
        let actual_commands: Vec<&str> = subcommands_vec.iter().map(|cmd| cmd.get_name()).collect();

        for expected_cmd in &expected_commands {
            assert!(
                actual_commands.contains(expected_cmd),
                "{expected_cmd} subcommand should be available"
            );
        }

        assert_eq!(
            expected_commands.len(),
            actual_commands.len(),
            "No extra subcommands should exist"
        );
    }

    #[ignore = "reason"]
    #[test]
    fn test_all_commands_parse() {
        // Test that all commands can be parsed without error
        let commands = vec![
            vec!["gops", "prj", "new", "--name", "test-project"],
            vec!["gops", "prj", "import", "--path", "/test/path"],
            vec!["gops", "prj", "update"],
            vec!["gops", "prj", "setting"],
            vec!["gops", "mod", "localize"],
            vec!["gops", "sys", "localize"],
            vec!["gops", "mod", "example"],
            vec!["gops", "mod", "new", "--name", "test-module"],
            vec!["gops", "mod", "update"],
            vec!["gops", "sys", "new", "--name", "test-system"],
            vec!["gops", "sys", "update"],
        ];

        for cmd_args in commands {
            let result = GInsCmd::try_parse_from(cmd_args.clone());
            assert!(result.is_ok(), "Failed to parse command: {cmd_args:?}");
        }
    }

    #[tokio::test]
    async fn test_commands_with_options() {
        // Test commands with various options
        let commands = vec![
            vec!["gops", "prj", "new", "--name", "test"],
            vec![
                "gops", "prj", "import", "--debug", "1", "--force", "2", "--path", "/test",
            ],
            vec![
                "gops",
                "prj",
                "update",
                "--debug",
                "2",
                "--log",
                "cmd=debug",
            ],
            vec![
                "gops",
                "mod",
                "localize",
                "--debug",
                "1",
                "--log",
                "local=debug",
                "--value",
                "test.yml",
                "--default",
            ],
        ];

        for cmd_args in commands {
            let result = GInsCmd::try_parse_from(cmd_args.clone());
            assert!(
                result.is_ok(),
                "Failed to parse command with options: {cmd_args:?}"
            );
        }
    }
}
