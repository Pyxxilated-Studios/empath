use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

type Result<T> = anyhow::Result<T>;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("build-all") => build_all(),
        Some("build-c") => build_c_examples(),
        Some("build-wasm") => build_wasm_examples(),
        Some("build-python") => copy_python_examples(),
        Some("clean") => clean_examples(),
        _ => {
            print_help();
            Ok(())
        }
    }
}

fn print_help() {
    println!("xtask - Empath MTA example module build orchestrator");
    println!();
    println!("USAGE:");
    println!("    cargo xtask <SUBCOMMAND>");
    println!();
    println!("SUBCOMMANDS:");
    println!("    build-all     Build all example modules (C + WASM + Python)");
    println!("    build-c       Build only C shared library examples");
    println!("    build-wasm    Build only WASM example module");
    println!("    build-python  Copy Python examples to target/modules");
    println!("    clean         Clean example module artifacts");
}

fn build_all() -> Result<()> {
    println!("Building all example modules...");
    build_c_examples()?;
    build_wasm_examples()?;
    copy_python_examples()?;
    println!("✅ All example modules built successfully");
    Ok(())
}

// =============================================================================
// C Example Compilation
// =============================================================================

fn build_c_examples() -> Result<()> {
    println!("Building C example modules...");

    build_empath_header()?;

    let profile = std::env::var("PROFILE").unwrap_or_else(|_| String::from("debug"));
    let lib_dir = format!("target/{profile}");
    let output_dir = PathBuf::from("target/modules");

    std::fs::create_dir_all(&output_dir)?;

    // Build log.c first (utility library used by others)
    build_c_module(
        "empath-examples/shared/log.c",
        "log",
        &lib_dir,
        &output_dir,
        &[],
    )?;

    // Build example.c (depends on log)
    build_c_module(
        "empath-examples/shared/example.c",
        "example",
        &lib_dir,
        &output_dir,
        &["log"],
    )?;

    // Build message mutation example (depends on log)
    build_c_module(
        "empath-examples/shared/message_mutation.c",
        "message_mutation",
        &lib_dir,
        &output_dir,
        &["log"],
    )?;

    // Build event.c
    build_c_module(
        "empath-examples/shared/event.c",
        "event",
        &lib_dir,
        &output_dir,
        &[],
    )?;

    println!("✅ C examples built successfully");
    Ok(())
}

fn build_c_module(
    source: &str,
    name: &str,
    lib_dir: &str,
    output_dir: &Path,
    link_libs: &[&str], // Additional libraries to link against
) -> Result<()> {
    let source_path = Path::new(source);
    if !source_path.exists() {
        anyhow::bail!("Source file not found: {source}");
    }

    let output_lib = output_dir.join(format!("lib{name}.{}", lib_extension()));

    // Check if rebuild is needed
    if !needs_rebuild(source_path, &output_lib) {
        println!("  {name}: up to date");
        return Ok(());
    }

    println!("  Compiling {source}...");

    // Compile C code directly using gcc/clang for shared library
    let compiler = if cfg!(target_os = "windows") {
        "cl"
    } else {
        "gcc"
    };

    let mut cmd = Command::new(compiler);
    cmd.arg(source);

    if cfg!(target_os = "windows") {
        // MSVC
        cmd.arg("/LD") // Create DLL
            .arg(format!("/Fe:{}", output_lib.display()))
            .arg("/I")
            .arg("target"); // Include empath.h from target/
    } else {
        // GCC/Clang
        cmd.arg("-fpic")
            .arg("-shared")
            .arg("-o")
            .arg(&output_lib)
            .arg("-Itarget") // Include empath.h from target/
            .arg("-lempath")
            .arg(format!("-L{lib_dir}"))
            .arg("-Ltarget/modules"); // Link against other modules in output_dir

        // Link against additional libraries
        for lib in link_libs {
            cmd.arg(format!("-l{lib}"));
        }
    }

    let status = cmd.status()?;

    if !status.success() {
        anyhow::bail!("Failed to compile {source}");
    }

    println!("  ✅ Built {}", output_lib.display());
    Ok(())
}

fn build_empath_header() -> Result<()> {
    let profile = env::var("PROFILE").unwrap_or_else(|_| String::from("debug"));
    let is_release = profile == "release";

    println!("Ensuring `empath` C header is up to date (profile = {profile})...");
    let mut cmd = Command::new("cargo");
    cmd.arg("build").arg("-p").arg("empath");
    if is_release {
        cmd.arg("--release");
    }

    let status = cmd.status()?;
    if !status.success() {
        anyhow::bail!("cargo build -p empath failed");
    }

    Ok(())
}

fn lib_extension() -> &'static str {
    if cfg!(target_os = "windows") {
        "dll"
    } else {
        "so"
    }
}

fn needs_rebuild(source: &Path, output: &Path) -> bool {
    // If output doesn't exist, rebuild
    let Ok(output_meta) = std::fs::metadata(output) else {
        return true;
    };

    // If source is newer than output, rebuild
    let Ok(source_meta) = std::fs::metadata(source) else {
        return true;
    };

    let Ok(source_time) = source_meta.modified() else {
        return true;
    };

    let Ok(output_time) = output_meta.modified() else {
        return true;
    };

    source_time > output_time
}

// =============================================================================
// WASM Example Compilation
// =============================================================================

fn build_wasm_examples() -> Result<()> {
    println!("Building WASM example module...");

    let profile = std::env::var("PROFILE").unwrap_or_else(|_| String::from("debug"));
    let is_release = profile == "release";

    let wasm_dir = Path::new("empath-examples/wasm");
    if !wasm_dir.exists() {
        println!("  ⚠️  WASM example directory not found, skipping");
        return Ok(());
    }

    // Sync WIT file from empath-wasm to example (single source of truth)
    sync_wit_files()?;

    let output_dir = PathBuf::from("target/modules");
    std::fs::create_dir_all(&output_dir)?;

    let profile_dir = if is_release { "release" } else { "debug" };
    let wasm_artifact = wasm_dir.join(format!(
        "target/wasm32-wasip2/{profile_dir}/simple_validator.wasm"
    ));
    let workspace_artifact = PathBuf::from(format!(
        "target/wasm32-wasip2/{profile_dir}/simple_validator.wasm"
    ));
    let dest = output_dir.join("simple_validator.wasm");

    let sources = [
        wasm_dir.join("src/lib.rs"),
        wasm_dir.join("Cargo.toml"),
        PathBuf::from("empath-wasm/wit/empath.wit"),
        PathBuf::from("empath-wasm-guest/wit/empath.wit"),
        wasm_dir.join("wit/empath.wit"),
    ];

    if artifact_is_fresh(&wasm_artifact, &sources) {
        println!("  simple_validator.wasm up to date");
        if needs_rebuild(&wasm_artifact, &dest) {
            std::fs::copy(&wasm_artifact, &dest)?;
            println!("  ✅ Copied {}", dest.display());
        }

        println!("✅ WASM example built successfully");
        return Ok(());
    }

    if artifact_is_fresh(&dest, &sources) {
        if let Some(parent) = wasm_artifact.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&dest, &wasm_artifact)?;
        println!("  Reused cached WASM artifact");
        println!("✅ WASM example built successfully");
        return Ok(());
    }

    // Check if wasm32-wasip2 target is installed
    if !is_target_installed("wasm32-wasip2")? {
        println!("  wasm32-wasip2 target not installed, installing...");
        install_wasm_target()?;
    }

    // Build the WASM module
    let mut cmd = Command::new("cargo");
    cmd.current_dir(wasm_dir)
        .arg("build")
        .arg("--target")
        .arg("wasm32-wasip2");

    if is_release {
        cmd.arg("--release");
    }

    println!("  Compiling WASM module ({profile} mode)...");
    let status = cmd.status()?;

    if !status.success() {
        anyhow::bail!("WASM build failed");
    }

    // Copy output to target/modules/
    let built_artifact = if wasm_artifact.exists() {
        wasm_artifact.clone()
    } else if workspace_artifact.exists() {
        workspace_artifact.clone()
    } else {
        anyhow::bail!(
            "WASM artifact not found at {} or {}",
            wasm_artifact.display(),
            workspace_artifact.display()
        );
    };

    if !dest.exists() || needs_rebuild(&built_artifact, &dest) {
        std::fs::copy(&built_artifact, &dest)?;
    }
    if built_artifact != wasm_artifact {
        if let Some(parent) = wasm_artifact.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&built_artifact, &wasm_artifact)?;
    }
    println!("  ✅ Built {}", dest.display());

    println!("✅ WASM example built successfully");
    Ok(())
}

fn is_target_installed(target: &str) -> Result<bool> {
    let output = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()?;

    Ok(String::from_utf8_lossy(&output.stdout).contains(target))
}

fn install_wasm_target() -> Result<()> {
    println!("  Installing wasm32-wasip2 target...");
    let status = Command::new("rustup")
        .args(["target", "add", "wasm32-wasip2"])
        .status()?;

    if !status.success() {
        anyhow::bail!("Failed to install wasm32-wasip2 target");
    }

    println!("  ✅ wasm32-wasip2 target installed");
    Ok(())
}

fn sync_wit_files() -> Result<()> {
    let source_wit = Path::new("empath-wasm/wit/empath.wit");
    let dest_wit_dir = Path::new("empath-examples/wasm/wit");
    let dest_wit = dest_wit_dir.join("empath.wit");

    if !source_wit.exists() {
        anyhow::bail!("Source WIT file not found: {}", source_wit.display());
    }

    // Create destination directory if it doesn't exist
    std::fs::create_dir_all(dest_wit_dir)?;

    // Check if we need to copy (source is newer or dest doesn't exist)
    if needs_rebuild(source_wit, &dest_wit) {
        println!("  Syncing WIT file from empath-wasm...");
        std::fs::copy(source_wit, &dest_wit)?;
        println!("  ✅ WIT file synced");
    }

    Ok(())
}

fn artifact_is_fresh(output: &Path, sources: &[PathBuf]) -> bool {
    let Ok(output_meta) = std::fs::metadata(output) else {
        return false;
    };

    let Ok(output_time) = output_meta.modified() else {
        return false;
    };

    for source in sources {
        let Ok(source_meta) = std::fs::metadata(source) else {
            return false;
        };

        let Ok(source_time) = source_meta.modified() else {
            return false;
        };

        if source_time > output_time {
            return false;
        }
    }

    true
}

// =============================================================================
// Python Example Copying
// =============================================================================

fn copy_python_examples() -> Result<()> {
    println!("Copying Python example modules...");

    let output_dir = PathBuf::from("target/modules");
    std::fs::create_dir_all(&output_dir)?;

    let python_dir = Path::new("empath-examples/python");
    if !python_dir.exists() {
        println!("  ⚠️  Python examples directory not found, skipping");
        return Ok(());
    }

    // Copy all .py files from python directory to target/modules/
    for entry in std::fs::read_dir(python_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("py") {
            let filename = path
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("Invalid filename"))?;
            let dest = output_dir.join(filename);

            std::fs::copy(&path, &dest)?;
            println!("  ✅ Copied {}", dest.display());
        }
    }

    println!("✅ Python examples copied successfully");
    Ok(())
}

// =============================================================================
// Clean
// =============================================================================

fn clean_examples() -> Result<()> {
    println!("Cleaning example module artifacts...");

    let output_dir = PathBuf::from("target/modules");
    if output_dir.exists() {
        std::fs::remove_dir_all(&output_dir)?;
        println!("  Removed {}", output_dir.display());
    }

    let wasm_target = Path::new("empath-examples/wasm/target");
    if wasm_target.exists() {
        std::fs::remove_dir_all(wasm_target)?;
        println!("  Removed {}", wasm_target.display());
    }

    println!("✅ Example artifacts cleaned");
    Ok(())
}
