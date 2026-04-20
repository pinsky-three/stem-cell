//! End-to-end test for `stem init` / `stem clone` against a local
//! bare-repo fixture. Avoids network dependence so the test is reliable
//! in CI.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Build the stem binary once for the test run, then return its path.
fn stem_binary() -> PathBuf {
    // CARGO_BIN_EXE_<name> is set by cargo for integration tests.
    PathBuf::from(env!("CARGO_BIN_EXE_stem"))
}

fn run_ok(argv: &[&str], cwd: Option<&Path>) {
    let mut cmd = Command::new(argv[0]);
    cmd.args(&argv[1..]);
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    let status = cmd.status().expect("command spawn");
    assert!(status.success(), "{argv:?}");
}

/// Creates a tiny local bare repo with one commit so `stem clone` /
/// `stem init --template <path>` have something real to clone.
fn make_bare_fixture(ws: &Path) -> PathBuf {
    let bare = ws.join("seed.git");
    run_ok(&["git", "init", "--bare", bare.to_str().unwrap()], None);

    let seed = ws.join("seed-worktree");
    run_ok(&["git", "init", seed.to_str().unwrap()], None);
    std::fs::write(seed.join("README.md"), "hello\n").unwrap();
    std::fs::write(seed.join(".mise.toml"), "[env]\nPORT = \"4200\"\n").unwrap();
    run_ok(&["git", "add", "."], Some(&seed));
    run_ok(
        &[
            "git",
            "-c",
            "user.email=a@b.c",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            "init",
        ],
        Some(&seed),
    );
    run_ok(
        &["git", "remote", "add", "origin", bare.to_str().unwrap()],
        Some(&seed),
    );
    run_ok(
        &["git", "push", "origin", "HEAD:refs/heads/main"],
        Some(&seed),
    );

    bare
}

#[test]
fn stem_clone_copies_local_bare_repo() {
    let ws = tempfile::tempdir().unwrap();
    let bare = make_bare_fixture(ws.path());

    let dest = ws.path().join("cloned");

    let status = Command::new(stem_binary())
        .args([
            "clone",
            bare.to_str().unwrap(),
            "--dir",
            dest.to_str().unwrap(),
        ])
        .status()
        .expect("spawn stem");
    assert!(status.success(), "stem clone failed");

    assert!(dest.join(".git").exists());
    assert!(dest.join("README.md").exists());
}

#[test]
fn stem_init_writes_manifest_and_clones_template() {
    let ws = tempfile::tempdir().unwrap();
    let bare = make_bare_fixture(ws.path());

    let dest = ws.path().join("my-app");

    // Use --skip-install so the test doesn't try to download mise /
    // run npm install on a CI machine.
    let status = Command::new(stem_binary())
        .args([
            "init",
            "my-app",
            "--template",
            bare.to_str().unwrap(),
            "--dir",
            dest.to_str().unwrap(),
            "--skip-install",
        ])
        .status()
        .expect("spawn stem");
    assert!(status.success(), "stem init failed");

    assert!(dest.join(".git").exists());
    assert!(dest.join("README.md").exists());

    // Manifest should be written at the project root.
    let manifest = std::fs::read_to_string(dest.join("stem.yaml")).expect("manifest");
    assert!(manifest.contains("name: my-app"));
    assert!(manifest.contains(bare.to_str().unwrap()));
}

#[test]
fn stem_init_dry_run_does_nothing_on_disk() {
    let ws = tempfile::tempdir().unwrap();
    let dest = ws.path().join("nope");

    let output = Command::new(stem_binary())
        .args([
            "init",
            "nope",
            "--dir",
            dest.to_str().unwrap(),
            "--template",
            "file:///definitely-does-not-exist",
            "--dry-run",
        ])
        .output()
        .expect("spawn stem");
    assert!(output.status.success(), "dry-run should succeed");
    assert!(!dest.exists(), "dry-run must not create the dest dir");
}
