use std::process::Command;

fn main() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root not found");

    let frontend = workspace_root.join("frontend");
    let index = workspace_root.join("public/index.html");

    println!("cargo:rerun-if-changed={}", index.display());
    println!("cargo:rerun-if-changed={}", frontend.join("src").display());
    println!(
        "cargo:rerun-if-changed={}",
        frontend.join("astro.config.mjs").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        frontend.join("package.json").display()
    );

    if std::env::var("SKIP_FRONTEND").is_ok() {
        println!("cargo:warning=SKIP_FRONTEND set — skipping frontend build");
        return;
    }

    if !frontend.join("node_modules").exists() {
        run("npm", &["install"], &frontend);
    }

    run("npm", &["run", "build"], &frontend);
}

fn run(cmd: &str, args: &[&str], dir: &std::path::Path) {
    let status = Command::new(cmd)
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|e| panic!("{cmd} failed to start: {e}"));

    assert!(status.success(), "{cmd} {args:?} exited with {status}");
}
