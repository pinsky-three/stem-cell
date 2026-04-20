//! `stem doctor` — fast environment diagnostics.
//!
//! Validates that the pieces `stem modify` / `stem heal` depend on are
//! reachable: a git repo, the `opencode` binary, and at least one model
//! provider (hosted key or local Ollama).

use crate::cli::DoctorArgs;
use crate::repo;
use anyhow::Result;
use serde::Serialize;
use std::process::Stdio;

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub repo_root: Option<String>,
    pub project_id: Option<String>,
    pub opencode_bin: Option<String>,
    pub opencode_version: Option<String>,
    pub mise_available: bool,
    pub providers: Vec<String>,
    pub warnings: Vec<String>,
    pub ok: bool,
}

pub async fn run(args: DoctorArgs) -> Result<()> {
    let mut report = DoctorReport {
        repo_root: None,
        project_id: None,
        opencode_bin: None,
        opencode_version: None,
        mise_available: false,
        providers: Vec::new(),
        warnings: Vec::new(),
        ok: true,
    };

    match repo::discover() {
        Ok(info) => {
            report.repo_root = Some(info.root.display().to_string());
            report.project_id = Some(info.project_id.to_string());
        }
        Err(e) => {
            report.warnings.push(format!("repo: {e}"));
            report.ok = false;
        }
    }

    report.mise_available = which("mise").is_some();
    if !report.mise_available {
        report
            .warnings
            .push("mise not found on PATH; stem will fall back to raw `cargo` commands".into());
    }

    if let Some(bin) = resolve_opencode() {
        report.opencode_version = opencode_version(&bin);
        report.opencode_bin = Some(bin);
    } else {
        report
            .warnings
            .push("opencode binary not found (tried `mise where opencode` and PATH)".into());
        report.ok = false;
    }

    report.providers = detect_providers();
    if report.providers.is_empty() {
        report.warnings.push(
            "no AI providers configured; set one of OPENROUTER_API_KEY, ANTHROPIC_API_KEY, OPENAI_API_KEY, or OLLAMA_MODELS".into(),
        );
        report.ok = false;
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render_pretty(&report);
    }

    if !report.ok {
        std::process::exit(1);
    }
    Ok(())
}

fn render_pretty(r: &DoctorReport) {
    println!("stem doctor");
    println!("───────────");
    println!(
        "repo root      : {}",
        r.repo_root.as_deref().unwrap_or("<not found>")
    );
    println!(
        "project id     : {}",
        r.project_id.as_deref().unwrap_or("-")
    );
    println!(
        "mise           : {}",
        if r.mise_available { "available" } else { "missing" }
    );
    println!(
        "opencode bin   : {}",
        r.opencode_bin.as_deref().unwrap_or("<not found>")
    );
    println!(
        "opencode ver   : {}",
        r.opencode_version.as_deref().unwrap_or("-")
    );
    println!(
        "providers      : {}",
        if r.providers.is_empty() {
            "<none>".to_string()
        } else {
            r.providers.join(", ")
        }
    );

    if !r.warnings.is_empty() {
        println!();
        println!("warnings:");
        for w in &r.warnings {
            println!("  - {w}");
        }
    }

    println!();
    println!("status: {}", if r.ok { "OK" } else { "DEGRADED" });
}

fn which(bin: &str) -> Option<String> {
    let output = std::process::Command::new("which")
        .arg(bin)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn resolve_opencode() -> Option<String> {
    // Try `mise where opencode` first (matches what the runtime does).
    if let Ok(out) = std::process::Command::new("mise")
        .args(["where", "opencode"])
        .output()
        && out.status.success()
    {
        let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
        for suffix in ["opencode", "bin/opencode"] {
            let candidate = format!("{dir}/{suffix}");
            if std::path::Path::new(&candidate).exists() {
                return Some(candidate);
            }
        }
    }
    which("opencode")
}

fn opencode_version(bin: &str) -> Option<String> {
    let out = std::process::Command::new(bin)
        .arg("--version")
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn detect_providers() -> Vec<String> {
    const KEYS: &[(&str, &str)] = &[
        ("openrouter", "OPENROUTER_API_KEY"),
        ("anthropic", "ANTHROPIC_API_KEY"),
        ("openai", "OPENAI_API_KEY"),
        ("google", "GOOGLE_API_KEY"),
        ("groq", "GROQ_API_KEY"),
        ("mistral", "MISTRAL_API_KEY"),
        ("xai", "XAI_API_KEY"),
        ("deepseek", "DEEPSEEK_API_KEY"),
    ];

    let mut out: Vec<String> = KEYS
        .iter()
        .filter(|(_, env)| std::env::var(env).is_ok())
        .map(|(name, _)| (*name).to_string())
        .collect();

    if let Ok(models) = std::env::var("OLLAMA_MODELS")
        && !models.trim().is_empty()
    {
        out.push(format!("ollama ({models})"));
    }

    out
}
