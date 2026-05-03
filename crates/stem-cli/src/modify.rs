//! `stem modify` — send a free-form instruction to OpenCode against this repo.

use crate::agent::Agent;
use crate::cli::ModifyArgs;
use crate::repo;
use anyhow::Result;
use std::time::Duration;

pub async fn run(args: ModifyArgs) -> Result<()> {
    let repo = repo::discover()?;
    tracing::info!(root = %repo.root.display(), project = %repo.project_id, "repo discovered");

    if args.dry_run {
        println!("[dry-run] would send the following prompt to OpenCode:");
        println!("  repo:  {}", repo.root.display());
        println!("  model: {}", args.model.as_deref().unwrap_or("<default>"));
        println!("  goal:  {}", args.goal);
        return Ok(());
    }

    let agent = Agent::boot(repo.project_id, &repo.root, args.model.clone()).await?;
    let timeout = Duration::from_secs(args.timeout_secs);

    let outcome = agent.run_turn(&args.goal, None, timeout).await?;
    agent.shutdown().await;

    print_outcome_summary("modify", &outcome);
    Ok(())
}

pub fn print_outcome_summary(label: &str, outcome: &crate::agent::SessionOutcome) {
    let changed = outcome.diffs.len();
    let additions: i64 = outcome.diffs.iter().map(|d| d.additions).sum();
    let deletions: i64 = outcome.diffs.iter().map(|d| d.deletions).sum();

    println!();
    println!("──────────── {label} summary ────────────");
    println!("session:  {}", outcome.session_id);
    println!(
        "state:    {}",
        if outcome.reached_idle {
            "idle"
        } else {
            "timed out / aborted"
        }
    );
    println!("files:    {changed} changed (+{additions}, -{deletions})");

    for diff in &outcome.diffs {
        println!(
            "  {:<9} {:<60} +{} -{}",
            diff.status, diff.path, diff.additions, diff.deletions
        );
    }
}
