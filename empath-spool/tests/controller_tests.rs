#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::PathBuf;

use empath_spool::FileBackingStore;

#[test]
fn test_path_validation_rejects_parent_dir() {
    let result = FileBackingStore::builder()
        .path(PathBuf::from("/var/spool/../etc/passwd"))
        .build();

    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("cannot contain '..'")
    );
}

#[test]
fn test_path_validation_rejects_relative_paths() {
    let result = FileBackingStore::builder()
        .path(PathBuf::from("relative/path"))
        .build();

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("must be absolute"));
}

#[test]
#[cfg(unix)]
fn test_path_validation_rejects_unix_system_directories() {
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

        assert!(result.is_err(), "Path {path} should be rejected but wasn't");
        assert!(
            result.unwrap_err().to_string().contains("system directory"),
            "Wrong error for path {path}"
        );
    }
}

#[test]
#[cfg(windows)]
fn test_path_validation_rejects_windows_system_directories() {
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

        assert!(result.is_err(), "Path {path} should be rejected but wasn't");
        assert!(
            result.unwrap_err().to_string().contains("system directory"),
            "Wrong error for path {path}"
        );
    }
}

#[test]
#[cfg(unix)]
fn test_path_validation_accepts_valid_unix_paths() {
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

        assert!(
            result.is_ok(),
            "Valid path {} was rejected: {:?}",
            path,
            result.unwrap_err()
        );
    }
}

#[test]
#[cfg(windows)]
fn test_path_validation_accepts_valid_windows_paths() {
    let valid_paths = vec![
        "D:\\Mail\\Spool",
        "C:\\Users\\Admin\\Mail",
        "E:\\Empath\\Spool",
    ];

    for path in valid_paths {
        let result = FileBackingStore::builder()
            .path(PathBuf::from(path))
            .build();

        assert!(
            result.is_ok(),
            "Valid path {} was rejected: {:?}",
            path,
            result.unwrap_err()
        );
    }
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
fn test_deserialization_accepts_valid_unix_path() {
    let valid_config = r#"(
        path: "/var/spool/empath"
    )"#;

    let result: Result<FileBackingStore, _> = ron::from_str(valid_config);
    assert!(
        result.is_ok(),
        "Valid path rejected during deserialization: {:?}",
        result.unwrap_err()
    );
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
fn test_deserialization_accepts_valid_windows_path() {
    let valid_config = r#"(
        path: "D:\\Mail\\Spool"
    )"#;

    let result: Result<FileBackingStore, _> = ron::from_str(valid_config);
    assert!(
        result.is_ok(),
        "Valid path rejected during deserialization: {:?}",
        result.unwrap_err()
    );
}
