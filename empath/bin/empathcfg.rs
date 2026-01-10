use std::path::PathBuf;

use clap::Parser;

/// Validate and emit Empath configuration files.
#[derive(Parser, Debug)]
#[command(
    name = "empathcfg",
    about = "Validate and emit Empath configuration",
    long_about = None,
    version,
    arg_required_else_help = true,
    subcommand_required = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Validate configuration files
    Check {
        /// Config file paths to validate
        #[arg(value_name = "PATH", required = true, num_args = 1..)]
        paths: Vec<PathBuf>,
    },
    /// Emit a canonical default config to stdout
    EmitDefault,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::EmitDefault => {
            let rendered = empath::config::emit_default_config()?;
            print!("{rendered}");
        }
        Command::Check { paths } => {
            for path in paths {
                validate_config(&path)?;
                println!("OK: {}", path.display());
            }
        }
    }

    Ok(())
}

fn validate_config(path: &std::path::Path) -> anyhow::Result<()> {
    empath::config::parse_config_file(path).map(|_| ())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command, validate_config};

    #[test]
    #[cfg_attr(miri, ignore)]
    fn parse_check_command() -> Result<(), String> {
        let cli = Cli::try_parse_from(["empathcfg", "check", "one.ron", "two.ron"])
            .map_err(|err| err.to_string())?;
        match cli.command {
            Command::Check { paths } => {
                assert_eq!(paths.len(), 2);
                assert!(paths.iter().any(|path| path.ends_with("one.ron")));
                assert!(paths.iter().any(|path| path.ends_with("two.ron")));
                Ok(())
            }
            Command::EmitDefault => Err("expected check command".to_string()),
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn parse_emit_default_command() -> Result<(), String> {
        let cli =
            Cli::try_parse_from(["empathcfg", "emit-default"]).map_err(|err| err.to_string())?;
        match cli.command {
            Command::EmitDefault => Ok(()),
            Command::Check { .. } => Err("expected emit-default command".to_string()),
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn validate_config_accepts_default() -> anyhow::Result<()> {
        let content = empath::config::emit_default_config()?;
        let path = std::env::temp_dir().join(format!(
            "empathcfg-test-{}.ron",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::write(&path, content)?;
        let result = validate_config(&path);
        let _ = std::fs::remove_file(&path);
        result
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn validate_config_missing_file_reports_error() {
        let path = std::env::temp_dir().join("empathcfg-missing-file.ron");
        let err = validate_config(&path).err();
        assert!(err.is_some());
    }
}
