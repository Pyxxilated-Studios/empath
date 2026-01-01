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
