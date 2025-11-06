#![deny(clippy::pedantic, clippy::all, clippy::nursery)]
#![allow(clippy::must_use_candidate)]

#[cfg(not(any(target_os = "macos", unix)))]
compile_error!("Only macos and unix are currently supported");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config_path = find_config_file()?;
    let config_content = std::fs::read_to_string(&config_path).map_err(|e| {
        anyhow::anyhow!(
            "Failed to read config from {}: {}",
            config_path.display(),
            e
        )
    })?;
    let empath: empath::controller::Empath = ron::from_str(&config_content)?;

    empath.run().await
}

/// Find the configuration file using the following precedence:
/// 1. `EMPATH_CONFIG` environment variable
/// 2. ./empath.config.ron (current working directory)
/// 3. /etc/empath/empath.config.ron (system-wide config)
fn find_config_file() -> anyhow::Result<std::path::PathBuf> {
    if let Ok(env_path) = std::env::var("EMPATH_CONFIG") {
        let path = std::path::PathBuf::from(env_path);
        if path.exists() {
            return Ok(path);
        }
        anyhow::bail!(
            "EMPATH_CONFIG points to non-existent file: {}",
            path.display()
        );
    }

    let default_paths = vec![
        std::path::PathBuf::from("./empath.config.ron"),
        std::path::PathBuf::from("/etc/empath/empath.config.ron"),
    ];

    for path in &default_paths {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    let paths_tried = default_paths
        .iter()
        .map(|p| format!("  - {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");

    anyhow::bail!(
        "No configuration file found. Tried:\n  - EMPATH_CONFIG environment variable\n{paths_tried}"
    )
}
