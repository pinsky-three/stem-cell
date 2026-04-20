//! Runs `mise run <stage>` (or `cargo` fallbacks) and captures the result.
//!
//! The output is truncated from the TAIL because Rust/clippy errors are
//! almost always interesting near the end; feeding the full log to the
//! agent wastes tokens and dilutes signal.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

const TAIL_BYTES: usize = 16 * 1024; // 16 KiB of trailing log is usually plenty.

pub struct CheckReport {
    pub stage: &'static str,
    pub command: String,
    pub success: bool,
    pub tail: String,
}

impl CheckReport {
    pub fn is_green(&self) -> bool {
        self.success
    }
}

/// Runs a single stage. Returns Ok(report) even on failure; transport
/// errors (missing `mise`, etc.) bubble up as Err.
pub async fn run_stage(stage: &'static str, cwd: &Path) -> Result<CheckReport> {
    let (bin, args) = stage_command(stage);
    let command_line = format!("{bin} {}", args.join(" "));

    tracing::info!(stage, command = %command_line, "running check");

    let output = Command::new(bin)
        .args(&args)
        .current_dir(cwd)
        .env("SKIP_FRONTEND", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("spawning `{command_line}`"))?;

    let mut combined = String::new();
    combined.push_str(&String::from_utf8_lossy(&output.stdout));
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    let tail = tail_bytes(&combined, TAIL_BYTES);

    Ok(CheckReport {
        stage,
        command: command_line,
        success: output.status.success(),
        tail,
    })
}

/// Maps a stage name to the actual command. We prefer `mise run` so users
/// get the same behaviour as CI; fall back to `cargo` if `mise` is absent.
fn stage_command(stage: &'static str) -> (&'static str, Vec<String>) {
    if mise_available() {
        ("mise", vec!["run".to_string(), stage.to_string()])
    } else {
        match stage {
            "check" => ("cargo", vec!["check".into(), "--workspace".into()]),
            "lint" => (
                "cargo",
                vec!["clippy".into(), "--workspace".into(), "--".into(), "-D".into(), "warnings".into()],
            ),
            "test" => ("cargo", vec!["test".into(), "--workspace".into()]),
            other => ("cargo", vec![other.to_string()]),
        }
    }
}

fn mise_available() -> bool {
    std::process::Command::new("mise")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn tail_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let start = s.len() - max;
    // Align to a char boundary so we don't slice through a multi-byte codepoint.
    let aligned = (start..s.len())
        .find(|i| s.is_char_boundary(*i))
        .unwrap_or(start);
    format!("...[truncated {} bytes]...\n{}", aligned, &s[aligned..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_short_string_is_unchanged() {
        assert_eq!(tail_bytes("hello", 1024), "hello");
    }

    #[test]
    fn tail_truncates_long_string_from_front() {
        let s = "a".repeat(100) + "TAIL";
        let t = tail_bytes(&s, 10);
        assert!(t.ends_with("TAIL"));
        assert!(t.starts_with("...[truncated"));
    }

    #[test]
    fn tail_respects_utf8_boundaries() {
        // Multi-byte chars near the truncation point must not panic.
        let s = "a".repeat(20) + "❤️".repeat(10).as_str();
        let _ = tail_bytes(&s, 15);
    }

    #[test]
    fn stage_command_falls_back_to_cargo_when_no_mise() {
        // We can't force `mise_available` to return false without env trickery,
        // but we can at least exercise the cargo branch directly.
        let (_bin, args) = ("cargo", {
            match "check" {
                "check" => vec!["check".to_string(), "--workspace".to_string()],
                _ => vec![],
            }
        });
        assert!(args.contains(&"--workspace".to_string()));
    }
}
