//! Clap definitions. Each subcommand has its own argument struct so the
//! topic modules stay independent of the parser.

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "stem",
    version,
    about = "Stem-cell ecosystem CLI — self-modify & self-heal via OpenCode",
    long_about = None,
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Diagnose the local environment (opencode binary, providers, repo root).
    Doctor(DoctorArgs),

    /// Ask OpenCode to modify this repository with a natural-language goal.
    Modify(ModifyArgs),

    /// Run checks (check / lint / test) and ask OpenCode to fix any failures.
    Heal(HealArgs),

    /// Scaffold a new stem-cell project from a template (defaults to `stem-cell-shrank`).
    Init(InitArgs),

    /// Clone an existing stem-cell project from a git URL.
    Clone(CloneArgs),
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Emit machine-readable JSON instead of a pretty table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ModifyArgs {
    /// The goal to send to OpenCode (required positional prompt).
    pub goal: String,

    /// Override `OPENCODE_MODEL` for this invocation.
    #[arg(long, env = "OPENCODE_MODEL")]
    pub model: Option<String>,

    /// Hard cap on how long to wait for the session to reach idle.
    #[arg(long, default_value_t = 1800)]
    pub timeout_secs: u64,

    /// Print what would be sent to OpenCode and exit.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum HealStage {
    Check,
    Lint,
    Test,
    All,
}

impl HealStage {
    pub fn stages(&self) -> &'static [&'static str] {
        match self {
            HealStage::Check => &["check"],
            HealStage::Lint => &["lint"],
            HealStage::Test => &["test"],
            HealStage::All => &["check", "lint", "test"],
        }
    }
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Project name. Becomes the scaffold directory name when `--dir` is omitted.
    pub name: String,

    /// Template URL (or built-in name). Defaults to `$STEM_DEFAULT_TEMPLATE` or
    /// the canonical `stem-cell-shrank` seed.
    #[arg(long, env = "STEM_DEFAULT_TEMPLATE")]
    pub template: Option<String>,

    /// Parent directory for the scaffold. Defaults to `./<name>`.
    #[arg(long)]
    pub dir: Option<PathBuf>,

    /// Skip the `mise install --yes` step (useful when offline or inside CI).
    #[arg(long)]
    pub skip_install: bool,

    /// Port to patch into `.env` / `.mise.toml` / Astro's `package.json`.
    #[arg(long, default_value_t = 4200)]
    pub port: u16,

    /// Print what would happen and exit.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct CloneArgs {
    /// Git URL (HTTPS or SSH) to clone.
    pub git_url: String,

    /// Destination directory. Defaults to a directory named after the last
    /// path segment of `git_url`.
    #[arg(long)]
    pub dir: Option<PathBuf>,

    /// Optional single branch to check out.
    #[arg(long)]
    pub branch: Option<String>,

    /// After cloning, run the toolchain install (`mise install`, etc.).
    #[arg(long)]
    pub install: bool,

    /// Port to use when `--install` patches `.env` / `.mise.toml` / Astro.
    #[arg(long, default_value_t = 4200)]
    pub port: u16,

    /// Print what would happen and exit.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct HealArgs {
    /// Which pipeline stage(s) to heal. Defaults to the full `check → lint → test` chain.
    #[arg(long, value_enum, default_value_t = HealStage::All)]
    pub stage: HealStage,

    /// How many repair cycles to attempt before giving up.
    #[arg(long, default_value_t = 3)]
    pub max_attempts: u32,

    /// Override `OPENCODE_MODEL` for repair prompts.
    #[arg(long, env = "OPENCODE_MODEL")]
    pub model: Option<String>,

    /// Hard cap on how long each repair prompt may run.
    #[arg(long, default_value_t = 1800)]
    pub timeout_secs: u64,

    /// Run checks only; do NOT invoke OpenCode to apply fixes.
    #[arg(long)]
    pub dry_run: bool,
}
