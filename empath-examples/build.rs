use std::path::Path;
use std::process::Command;

fn main() {
    // Trigger rebuild if any example source changes
    println!("cargo:rerun-if-changed=shared/example.c");
    println!("cargo:rerun-if-changed=shared/event.c");
    println!("cargo:rerun-if-changed=shared/log.c");
    println!("cargo:rerun-if-changed=shared/log.h");
    println!("cargo:rerun-if-changed=wasm/src/lib.rs");
    println!("cargo:rerun-if-changed=wasm/Cargo.toml");
    println!("cargo:rerun-if-changed=python/simple.py");
    println!("cargo:rerun-if-changed=python/spam_filter.py");
    // Track canonical WIT file in empath-wasm (will be synced during build)
    println!("cargo:rerun-if-changed=../empath-ffi/wit/empath.wit");

    // Pass the build profile to xtask
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| String::from("debug"));

    // Get absolute path to workspace root
    let workspace_root = Path::new("..")
        .canonicalize()
        .expect("Failed to get workspace root");

    // Try to use pre-built xtask binary first to avoid circular dependency
    let xtask_bin = workspace_root.join("target").join(&profile).join("xtask");
    let xtask_bin = if cfg!(target_os = "windows") {
        xtask_bin.with_extension("exe")
    } else {
        xtask_bin
    };

    let status = if xtask_bin.exists() {
        // Use pre-built binary
        // xtask expects to run from workspace root
        Command::new(&xtask_bin)
            .current_dir(&workspace_root)
            .arg("build-all")
            .env("PROFILE", profile)
            .status()
            .expect("Failed to run xtask binary")
    } else {
        // Build and run xtask (first-time build)
        // Use --release=false or --release based on profile to avoid rebuilding unnecessarily
        let mut cmd = Command::new("cargo");
        cmd.args(["run", "--package", "xtask"]);

        cmd.current_dir(&workspace_root) // Run from workspace root
            .arg("--")
            .arg("build-all")
            .env("PROFILE", profile)
            .status()
            .expect("Failed to run xtask")
    };

    if !status.success() {
        panic!("xtask build-all failed");
    }
}
