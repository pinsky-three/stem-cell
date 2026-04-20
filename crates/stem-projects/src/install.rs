//! Toolchain install + `.env`/`.mise.toml` port-patching bootstrap.
//!
//! Mirrors the shell pipeline that previously lived inside
//! `run_subprocess_setup` in the runtime. We keep it as a single
//! `bash -c` script (instead of breaking every `sed`/`grep` into a
//! tokio step) because:
//!
//! 1. The original pipeline is battle-tested and changes would be
//!    hard to verify without full integration runs.
//! 2. Container mode will want to run the same script verbatim in a
//!    Docker context (Phase 1.5).
//! 3. `bash -c` already short-circuits on `set -e`, which matches
//!    the runtime's previous `Err(...)` mapping.

use crate::ProjectPath;
use crate::error::{Error, Result};
use crate::patch::astro_port_patch_snippet;
use std::time::Instant;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct InstallOpts {
    /// Port to substitute into `.env`, `.mise.toml`, and the Astro
    /// `package.json` so the preview binds somewhere that won't collide
    /// with the parent stem-cell host.
    pub port: u16,

    /// If true, skip the `mise install --yes` step (useful for tests
    /// against template fixtures that don't ship a toolchain).
    pub skip_mise_install: bool,
}

impl Default for InstallOpts {
    fn default() -> Self {
        Self {
            port: 4200,
            skip_mise_install: false,
        }
    }
}

/// Runs the full clone-and-patch bootstrap *assuming the clone already
/// happened*. Callers should `clone_repo` first, then `install_toolchain`.
///
/// The script is emitted by `build_install_script` so it can be
/// snapshot-tested without spawning bash.
pub async fn install_toolchain(project: &ProjectPath, opts: InstallOpts) -> Result<()> {
    let script = build_install_script(project, &opts);
    let started = Instant::now();

    tracing::info!(
        dest = %project.as_path().display(),
        port = opts.port,
        skip_mise_install = opts.skip_mise_install,
        "install_toolchain: running bootstrap"
    );

    let status = Command::new("bash")
        .args(["-c", &script])
        .env("MISE_YES", "1")
        .status()
        .await
        .map_err(|e| {
            // Preserve the underlying IO error text in the log; the typed
            // error variant is enough for callers to react.
            tracing::error!(error = %e, "failed to spawn bash");
            Error::InstallFailed {
                phase: "spawn bash".into(),
                exit_code: -1,
            }
        })?;

    let elapsed_ms = started.elapsed().as_millis() as u64;

    if !status.success() {
        tracing::error!(
            dest = %project.as_path().display(),
            elapsed_ms,
            exit = ?status.code(),
            "install_toolchain: bootstrap failed"
        );
        return Err(Error::InstallFailed {
            phase: "bootstrap".into(),
            exit_code: status.code().unwrap_or(-1),
        });
    }

    tracing::info!(
        dest = %project.as_path().display(),
        elapsed_ms,
        "install_toolchain: complete"
    );

    Ok(())
}

/// Renders the bootstrap script. Exposed for unit tests so we can lock
/// in whitespace / env-patching behaviour without running bash.
pub fn build_install_script(project: &ProjectPath, opts: &InstallOpts) -> String {
    let work_dir = project.as_path().display().to_string();
    let port = opts.port;
    let astro_patch = astro_port_patch_snippet(port);

    let mise_install = if opts.skip_mise_install {
        "echo '[stem-projects] skipping mise install'".to_string()
    } else {
        // Use flock on the mise install lock so concurrent spawns on the
        // same host don't step on each other (mise has its own locking,
        // but belt-and-suspenders matters when the host runs several
        // stem-cell instances).
        "if command -v flock >/dev/null 2>&1; then flock /tmp/mise-install.lock $MISE install --yes; else $MISE install --yes; fi".to_string()
    };

    // NOTE: this script is byte-for-byte equivalent to the body of
    // `run_subprocess_setup` minus the leading `git clone` (which now
    // lives in `clone_repo`). Do not reformat without updating the
    // snapshot tests.
    format!(
        "set -e && \
         cd \"{dir}\" && \
         MISE=$( command -v mise || echo ~/.local/bin/mise ) && \
         if [ ! -x \"$MISE\" ]; then \
           curl -fsSL https://mise.run | bash && MISE=~/.local/bin/mise; \
         fi && \
         $MISE trust && \
         sed 's/^PORT = .*/PORT = \"{port}\"/' .mise.toml > .mise.toml.tmp && mv .mise.toml.tmp .mise.toml && \
         if [ -f .env ]; then \
           _sc_env=$(mktemp) || exit 1; \
           (grep -vE '^[[:space:]]*PORT=' .env || true) > \"$_sc_env\" && \
           printf 'PORT=%s\\n' '{port}' >> \"$_sc_env\" && \
           mv \"$_sc_env\" .env; \
         fi && \
         {astro_patch} && \
         {mise_install}",
        dir = work_dir,
        port = port,
        astro_patch = astro_patch,
        mise_install = mise_install,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn script_substitutes_port_in_mise_and_env_patches() {
        let project = ProjectPath(PathBuf::from("/tmp/example"));
        let script = build_install_script(
            &project,
            &InstallOpts {
                port: 5000,
                skip_mise_install: true,
            },
        );
        assert!(script.contains("cd \"/tmp/example\""));
        assert!(script.contains("PORT = \"5000\""));
        assert!(script.contains("PORT=%s")); // printf format preserved
        assert!(script.contains("--port 5000"));
    }

    #[test]
    fn script_gates_mise_install_behind_flock_by_default() {
        let project = ProjectPath(PathBuf::from("/tmp/example"));
        let script = build_install_script(&project, &InstallOpts::default());
        assert!(script.contains("flock /tmp/mise-install.lock"));
        assert!(script.contains("$MISE install --yes"));
    }

    #[test]
    fn skip_mise_install_replaces_the_install_step() {
        let project = ProjectPath(PathBuf::from("/tmp/example"));
        let script = build_install_script(
            &project,
            &InstallOpts {
                port: 4200,
                skip_mise_install: true,
            },
        );
        assert!(!script.contains("$MISE install"));
        assert!(script.contains("skipping mise install"));
    }

    /// Every segment of the old runtime script must still appear in the
    /// new one — this catches the case where someone accidentally drops
    /// a step during a future refactor.
    #[test]
    fn preserves_every_step_from_the_runtime_script() {
        let project = ProjectPath(PathBuf::from("/work"));
        let script = build_install_script(&project, &InstallOpts::default());
        for needle in [
            "set -e",
            "MISE=$( command -v mise || echo ~/.local/bin/mise )",
            "curl -fsSL https://mise.run | bash",
            "$MISE trust",
            "sed 's/^PORT = .*/PORT = \"4200\"/' .mise.toml",
            "if [ -f .env ]",
            "grep -vE '^[[:space:]]*PORT='",
        ] {
            assert!(
                script.contains(needle),
                "missing runtime step `{needle}` in generated script"
            );
        }
    }
}
