//! `stem init` — scaffold a new stem-cell project from a template.
//!
//! Thin wrapper around `stem_projects::init_from_template` that keeps
//! CLI-level concerns (argument parsing, pretty summary, dry-run) out of
//! the shared library.

use crate::cli::InitArgs;
use anyhow::{Context, Result};
use stem_projects::{InstallOpts, init_from_template, install_toolchain};

pub async fn run(args: InitArgs) -> Result<()> {
    let dest = args
        .dir
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from(&args.name));

    if args.dry_run {
        println!("[dry-run] would scaffold stem-cell project:");
        println!("  name:     {}", args.name);
        println!("  dest:     {}", dest.display());
        println!(
            "  template: {}",
            args.template
                .as_deref()
                .unwrap_or(stem_projects::DEFAULT_TEMPLATE_URL)
        );
        println!("  port:     {}", args.port);
        println!(
            "  install:  {}",
            if args.skip_install {
                "skipped"
            } else {
                "mise install --yes"
            }
        );
        return Ok(());
    }

    tracing::info!(
        name = %args.name,
        dest = %dest.display(),
        template = %args.template.as_deref().unwrap_or(stem_projects::DEFAULT_TEMPLATE_URL),
        "init: scaffolding project"
    );

    let outcome = init_from_template(&args.name, &dest, args.template.as_deref())
        .await
        .context("init_from_template")?;

    if !args.skip_install {
        install_toolchain(
            &outcome.path,
            InstallOpts {
                port: args.port,
                skip_mise_install: false,
            },
        )
        .await
        .context("install_toolchain")?;
    }

    println!();
    println!("──────────── init summary ────────────");
    println!("project:   {}", outcome.manifest.name);
    println!("location:  {}", outcome.path.as_path().display());
    println!(
        "template:  {}",
        outcome.manifest.template.as_deref().unwrap_or("<none>")
    );
    println!("port:      {}", args.port);
    println!("manifest:  stem.yaml");
    if args.skip_install {
        println!("install:   skipped (--skip-install)");
    } else {
        println!("install:   complete");
    }
    println!();
    println!("next steps:");
    println!("  cd {}", outcome.path.as_path().display());
    println!("  mise run dev         # start the preview server");
    println!("  stem modify \"...\"    # ask OpenCode to change the project");

    Ok(())
}
