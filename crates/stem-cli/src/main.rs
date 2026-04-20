//! Entry point for the `stem` CLI.
//!
//! Composes subcommands from the `cli` module. Keep this file minimal —
//! logic lives in topic-specific modules (heal, modify, doctor).

mod agent;
mod checks;
mod cli;
mod doctor;
mod heal;
mod modify;
mod repo;

use clap::Parser;
use cli::{Cli, Command};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load `.env` best-effort so users can run `stem` from a plain shell.
    let _ = dotenvy::dotenv();

    init_tracing();

    let cli = Cli::parse();
    match cli.command {
        Command::Doctor(args) => doctor::run(args).await,
        Command::Modify(args) => modify::run(args).await,
        Command::Heal(args) => heal::run(args).await,
    }
}

/// Pretty logs by default; JSON when `STEM_LOG_FORMAT=json`.
/// Controlled by `RUST_LOG` (defaults to `stem_cli=info,opencode_client=info`).
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("stem_cli=info,opencode_client=info,warn")
    });

    let json = std::env::var("STEM_LOG_FORMAT")
        .map(|v| v.eq_ignore_ascii_case("json"))
        .unwrap_or(false);

    if json {
        fmt().with_env_filter(filter).json().init();
    } else {
        fmt()
            .with_env_filter(filter)
            .with_target(false)
            .compact()
            .init();
    }
}
