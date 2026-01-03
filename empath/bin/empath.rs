#[cfg(not(any(target_os = "macos", unix)))]
compile_error!("Only macos and unix are currently supported");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config_path = find_config_file()?;
    let empath = empath::config::parse_config_file(&config_path)?;

    empath.run().await
}

/// Find the configuration file using the following precedence:
/// 1. `EMPATH_CONFIG` environment variable
/// 2. ./empath.config.ron (current working directory)
/// 3. /etc/empath/empath.config.ron (system-wide config)
fn find_config_file() -> anyhow::Result<std::path::PathBuf> {
    fn normalize_path(path: std::path::PathBuf) -> anyhow::Result<std::path::PathBuf> {
        let absolute = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()?.join(path)
        };
        Ok(std::fs::canonicalize(&absolute)?)
    }

    if let Ok(env_path) = std::env::var("EMPATH_CONFIG") {
        let path = std::path::PathBuf::from(env_path);
        if path.exists() {
            return normalize_path(path);
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
            return normalize_path(path.clone());
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

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};
    use std::{env, fs};

    use anyhow::{Result, anyhow};
    use tempfile::tempdir;

    use super::find_config_file;

    struct EnvGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = self.previous.take() {
                unsafe {
                    env::set_var(self.key, value);
                }
            } else {
                unsafe {
                    env::remove_var(self.key);
                }
            }
        }
    }

    struct DirGuard {
        previous: PathBuf,
    }

    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = env::set_current_dir(&self.previous);
        }
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = LOCK.get_or_init(|| Mutex::new(()));
        match lock.lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        }
    }

    fn set_env_var(key: &'static str, value: &OsStr) -> EnvGuard {
        let previous = env::var_os(key);
        unsafe {
            env::set_var(key, value);
        }
        EnvGuard { key, previous }
    }

    fn clear_env_var(key: &'static str) -> EnvGuard {
        let previous = env::var_os(key);
        unsafe {
            env::remove_var(key);
        }
        EnvGuard { key, previous }
    }

    fn set_current_dir(path: &std::path::Path) -> Result<DirGuard> {
        let previous = env::current_dir()?;
        env::set_current_dir(path)?;
        Ok(DirGuard { previous })
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri does not support filesystem temp dirs")]
    fn find_config_file_prefers_env_var() -> Result<()> {
        let _lock = env_lock();
        let temp = tempdir()?;
        let config_path = temp.path().join("custom.config.ron");
        fs::write(&config_path, b"")?;
        let expected = fs::canonicalize(&config_path)?;

        let _env_guard = set_env_var("EMPATH_CONFIG", config_path.as_os_str());
        let found = find_config_file()?;

        assert_eq!(found, expected);
        Ok(())
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri does not support filesystem temp dirs")]
    fn find_config_file_errors_when_env_missing() -> Result<()> {
        let _lock = env_lock();
        let temp = tempdir()?;
        let config_path = temp.path().join("missing.config.ron");

        let _env_guard = set_env_var("EMPATH_CONFIG", config_path.as_os_str());
        let Err(err) = find_config_file() else {
            return Err(anyhow!("expected an error for missing EMPATH_CONFIG path"));
        };
        assert!(
            err.to_string()
                .contains("EMPATH_CONFIG points to non-existent file")
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri does not support filesystem temp dirs")]
    fn find_config_file_falls_back_to_cwd() -> Result<()> {
        let _lock = env_lock();
        let temp = tempdir()?;
        let config_path = temp.path().join("empath.config.ron");
        fs::write(&config_path, b"")?;
        let expected = fs::canonicalize(&config_path)?;

        let _env_guard = clear_env_var("EMPATH_CONFIG");
        let _dir_guard = set_current_dir(temp.path())?;
        let found = find_config_file()?;

        assert_eq!(found, expected);
        Ok(())
    }
}
