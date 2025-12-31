use std::path::PathBuf;

use empath_spool::FileBackingStore;

#[test]
fn test_path_validation_rejects_parent_dir() -> Result<(), Box<dyn std::error::Error>> {
    let result = FileBackingStore::builder()
        .path(PathBuf::from("/var/spool/../etc/passwd"))
        .build();

    let Err(err) = result else {
        return Err("expected path rejection".into());
    };
    assert!(err.to_string().contains("cannot contain '..'"));
    Ok(())
}

#[test]
fn test_path_validation_rejects_relative_paths() -> Result<(), Box<dyn std::error::Error>> {
    let result = FileBackingStore::builder()
        .path(PathBuf::from("relative/path"))
        .build();

    let Err(err) = result else {
        return Err("expected path rejection".into());
    };
    assert!(err.to_string().contains("must be absolute"));
    Ok(())
}

#[test]
#[cfg(unix)]
fn test_path_validation_rejects_unix_system_directories() -> Result<(), Box<dyn std::error::Error>>
{
    let system_paths = vec![
        "/etc/spool",
        "/bin/messages",
        "/sbin/mail",
        "/usr/bin/data",
        "/boot/spool",
        "/sys/messages",
        "/proc/mail",
        "/dev/spool",
    ];

    for path in system_paths {
        let result = FileBackingStore::builder()
            .path(PathBuf::from(path))
            .build();

        let Err(err) = result else {
            return Err(format!("path {path} should be rejected").into());
        };
        assert!(
            err.to_string().contains("system directory"),
            "Wrong error for path {path}"
        );
    }
    Ok(())
}

#[test]
#[cfg(windows)]
fn test_path_validation_rejects_windows_system_directories()
-> Result<(), Box<dyn std::error::Error>> {
    let system_paths = vec![
        "C:\\Windows\\System32",
        "C:\\Program Files\\Test",
        "C:\\Program Files (x86)\\Test",
        "C:\\ProgramData\\Mail",
        // Test case-insensitive matching
        "c:\\windows\\system32",
        "c:\\program files\\test",
    ];

    for path in system_paths {
        let result = FileBackingStore::builder()
            .path(PathBuf::from(path))
            .build();

        let err = match result {
            Ok(_) => return Err(format!("path {path} should be rejected").into()),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("system directory"),
            "Wrong error for path {path}"
        );
    }
    Ok(())
}

#[test]
#[cfg(unix)]
fn test_path_validation_accepts_valid_unix_paths() -> Result<(), Box<dyn std::error::Error>> {
    let valid_paths = vec![
        "/var/spool/empath",
        "/home/user/mail",
        "/opt/empath/spool",
        "/tmp/test-spool",
    ];

    for path in valid_paths {
        let result = FileBackingStore::builder()
            .path(PathBuf::from(path))
            .build();

        let Err(err) = result else {
            continue;
        };
        return Err(format!("Valid path {path} was rejected: {err:?}").into());
    }
    Ok(())
}

#[test]
#[cfg(windows)]
fn test_path_validation_accepts_valid_windows_paths() -> Result<(), Box<dyn std::error::Error>> {
    let valid_paths = vec![
        "D:\\Mail\\Spool",
        "C:\\Users\\Admin\\Mail",
        "E:\\Empath\\Spool",
    ];

    for path in valid_paths {
        let result = FileBackingStore::builder()
            .path(PathBuf::from(path))
            .build();

        let err = match result {
            Ok(_) => continue,
            Err(err) => err,
        };
        return Err(format!("Valid path {path} was rejected: {err:?}").into());
    }
    Ok(())
}

#[test]
#[cfg(unix)]
fn test_deserialization_validates_unix_path() {
    // Test that deserialization also validates paths
    let invalid_config = r#"(
        path: "/etc/passwd"
    )"#;

    let result: Result<FileBackingStore, _> = ron::from_str(invalid_config);
    assert!(result.is_err());
}

#[test]
#[cfg(unix)]
fn test_deserialization_accepts_valid_unix_path() -> Result<(), Box<dyn std::error::Error>> {
    let valid_config = r#"(
        path: "/var/spool/empath"
    )"#;

    let result: Result<FileBackingStore, _> = ron::from_str(valid_config);
    let Err(err) = result else {
        return Ok(());
    };
    Err(format!("Valid path rejected during deserialization: {err:?}").into())
}

#[test]
#[cfg(windows)]
fn test_deserialization_validates_windows_path() {
    // Test that deserialization also validates paths
    let invalid_config = r#"(
        path: "C:\\Windows\\System32"
    )"#;

    let result: Result<FileBackingStore, _> = ron::from_str(invalid_config);
    assert!(result.is_err());
}

#[test]
#[cfg(windows)]
fn test_deserialization_accepts_valid_windows_path() -> Result<(), Box<dyn std::error::Error>> {
    let valid_config = r#"(
        path: "D:\\Mail\\Spool"
    )"#;

    let result: Result<FileBackingStore, _> = ron::from_str(valid_config);
    let Err(err) = result else {
        return Ok(());
    };
    Err(format!("Valid path rejected during deserialization: {err:?}").into())
}
