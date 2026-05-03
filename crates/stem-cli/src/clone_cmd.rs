//! `stem clone` — clone an existing stem-cell project from a git URL.
//!
//! Delegates the actual work to `stem_projects::clone_repo` so the
//! runtime's `SpawnEnvironment` and this command share the exact same
//! implementation.

use crate::cli::CloneArgs;
use anyhow::{Context, Result};
use std::path::PathBuf;
use stem_projects::{CloneOpts, InstallOpts, clone_repo, install_toolchain};

pub async fn run(args: CloneArgs) -> Result<()> {
    let dest = args
        .dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(derive_dir_from_url(&args.git_url)));

    if args.dry_run {
        println!("[dry-run] would clone:");
        println!("  from:   {}", args.git_url);
        println!("  into:   {}", dest.display());
        if let Some(ref b) = args.branch {
            println!("  branch: {b}");
        }
        println!(
            "  install: {}",
            if args.install {
                format!("yes (port {})", args.port)
            } else {
                "no".to_string()
            }
        );
        return Ok(());
    }

    tracing::info!(
        git_url = %args.git_url,
        dest = %dest.display(),
        "clone: pulling project"
    );

    let project = clone_repo(
        &args.git_url,
        &dest,
        CloneOpts {
            progress: true,
            branch: args.branch.clone(),
            auth: None,
        },
    )
    .await
    .context("clone_repo")?;

    if args.install {
        install_toolchain(
            &project,
            InstallOpts {
                port: args.port,
                skip_mise_install: false,
            },
        )
        .await
        .context("install_toolchain")?;
    }

    println!();
    println!("──────────── clone summary ────────────");
    println!("source:    {}", args.git_url);
    println!("location:  {}", project.as_path().display());
    if let Some(ref b) = args.branch {
        println!("branch:    {b}");
    }
    if args.install {
        println!("install:   complete (port {})", args.port);
    } else {
        println!("install:   skipped (pass --install to bootstrap)");
    }

    Ok(())
}

/// Extracts a reasonable default directory name from a git URL. Handles
/// both HTTPS (`https://host/a/b.git`) and SSH (`git@host:a/b.git`).
fn derive_dir_from_url(url: &str) -> String {
    let last = url.rsplit(['/', ':']).next().unwrap_or(url);
    last.strip_suffix(".git").unwrap_or(last).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_dir_from_https_url() {
        assert_eq!(derive_dir_from_url("https://github.com/a/b.git"), "b");
    }

    #[test]
    fn derives_dir_from_ssh_url() {
        assert_eq!(derive_dir_from_url("git@github.com:a/b.git"), "b");
    }

    #[test]
    fn derives_dir_without_suffix() {
        assert_eq!(derive_dir_from_url("https://example.com/foo"), "foo");
    }
}
