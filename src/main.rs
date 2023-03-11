#[macro_use]
extern crate lazy_static;

use anyhow::Result;

use clap::{Command, CommandFactory, Parser, Subcommand};

use clap_complete::{generate, Generator, Shell};
mod utils;
mod logs;

#[derive(Debug, Parser)]
#[command(name = "container", author, version, about, long_about = None)] // Read from `Cargo.toml`
struct Cli {
    // If provided, outputs the completion file for given shell
    #[arg(long = "generate", value_enum)]
    generator: Option<Shell>,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command()]
    Logs {
        #[arg(short, long)]
        container: Option<String>,
        #[arg(short, long)]
        namespace: Option<String>,
        #[arg(long)]
        since: Option<u8>,
        #[arg(short, long)]
        follow: bool,
    },
}

fn print_completions<G: Generator>(gen: G, cmd: &mut Command) {
    generate(gen, cmd, cmd.get_name().to_string(), &mut std::io::stdout());
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    if let Some(generator) = cli.generator {
        let mut cmd = Cli::command();
        eprintln!("Generating completion file for {generator:?}...");
        print_completions(generator, &mut cmd);

        return Ok(());
    } else if let Some(command) = cli.command {
        match command {
            Commands::Logs {
                container,
                namespace,
                since,
                follow,
            } => {
                crate::logs::process_logs(namespace.as_deref(), container.as_deref(), since, follow).await?;
            }
        }
    }

    Ok(())
}
