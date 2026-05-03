//! `stem heal` — run CI checks and let OpenCode patch failures in a loop.
//!
//! Strategy:
//! 1. Run the requested stages in order (check → lint → test by default).
//! 2. If every stage is green, exit 0 immediately.
//! 3. Otherwise, feed the failing stage's tail-log into an OpenCode prompt
//!    and let the agent patch files.
//! 4. Re-run. Repeat up to `max_attempts` times.
//!
//! This is deliberately sequential and conservative: fix one stage at a
//! time so the model's context stays tight and regressions are obvious.

use crate::agent::Agent;
use crate::checks::{CheckReport, run_stage};
use crate::cli::HealArgs;
use crate::modify::print_outcome_summary;
use crate::repo;
use anyhow::Result;
use std::time::Duration;

pub async fn run(args: HealArgs) -> Result<()> {
    let repo = repo::discover()?;
    let stages = args.stage.stages();
    tracing::info!(
        root = %repo.root.display(),
        project = %repo.project_id,
        stages = ?stages,
        max_attempts = args.max_attempts,
        dry_run = args.dry_run,
        "starting heal"
    );

    // Fast path: green on entry.
    if let Some(first_failure) = first_failing_stage(stages, &repo.root).await? {
        if args.dry_run {
            println!(
                "[dry-run] stage `{}` failed; would invoke OpenCode to repair.",
                first_failure.stage
            );
            println!(
                "────── failing output (tail) ──────\n{}",
                first_failure.tail
            );
            return Ok(());
        }

        let agent = Agent::boot(repo.project_id, &repo.root, args.model.clone()).await?;
        let result = heal_loop(&agent, &args, stages, &repo.root, first_failure).await;
        agent.shutdown().await;
        return result;
    }

    println!("all stages green — nothing to heal.");
    Ok(())
}

async fn heal_loop(
    agent: &Agent,
    args: &HealArgs,
    stages: &[&'static str],
    cwd: &std::path::Path,
    mut failure: CheckReport,
) -> Result<()> {
    for attempt in 1..=args.max_attempts {
        tracing::warn!(
            attempt,
            max = args.max_attempts,
            stage = failure.stage,
            "attempting repair"
        );

        let goal = build_repair_prompt(&failure);
        let timeout = Duration::from_secs(args.timeout_secs);
        let outcome = agent
            .run_turn(&goal, Some(REPAIR_SYSTEM_SUFFIX), timeout)
            .await?;
        print_outcome_summary(&format!("heal attempt {attempt}"), &outcome);

        if outcome.diffs.is_empty() {
            tracing::warn!(attempt, "agent made no file changes; giving up");
            anyhow::bail!(
                "heal failed: agent produced no diffs on attempt {attempt} (stage `{}`)",
                failure.stage
            );
        }

        match first_failing_stage(stages, cwd).await? {
            None => {
                println!("\nall stages green after {attempt} attempt(s) — heal successful.");
                return Ok(());
            }
            Some(next) => {
                tracing::warn!(stage = next.stage, "still failing; will retry");
                failure = next;
            }
        }
    }

    anyhow::bail!(
        "heal exhausted {} attempts; last failing stage was `{}`",
        args.max_attempts,
        failure.stage
    )
}

const REPAIR_SYSTEM_SUFFIX: &str = "You are in REPAIR MODE. Read the failing command output carefully, identify the \
     ROOT CAUSE (not just the symptom), and make the minimum edits required to make the \
     stage pass. Never disable tests, never #[allow] lints to hide errors, never mark \
     tests #[ignore]. If the fix requires editing generated or framework code, explain \
     why in chat and make no edits.";

fn build_repair_prompt(failure: &CheckReport) -> String {
    format!(
        "The `{stage}` stage of our CI pipeline is failing.\n\n\
Command: `{command}`\n\n\
Tail of the output:\n\
```\n{tail}\n```\n\n\
Please diagnose the failure and apply the minimum edits required to get this stage back to green.\n\
After editing, do NOT re-run the checks yourself — the `stem` CLI will re-run them.",
        stage = failure.stage,
        command = failure.command,
        tail = failure.tail,
    )
}

async fn first_failing_stage(
    stages: &[&'static str],
    cwd: &std::path::Path,
) -> Result<Option<CheckReport>> {
    for stage in stages {
        let report = run_stage(stage, cwd).await?;
        if !report.is_green() {
            return Ok(Some(report));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_prompt_includes_stage_and_tail() {
        let failure = CheckReport {
            stage: "lint",
            command: "mise run lint".into(),
            success: false,
            tail: "error[E0308]: mismatched types".into(),
        };
        let prompt = build_repair_prompt(&failure);
        assert!(prompt.contains("`lint`"));
        assert!(prompt.contains("mismatched types"));
        assert!(prompt.contains("mise run lint"));
    }
}
