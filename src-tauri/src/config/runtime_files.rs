use crate::errors::StartupError;
use std::path::{Path, PathBuf};

const TOKEN_AND_PID_LIMIT: usize = 128;
const OUTBOX_PATH_LIMIT: usize = 64 * 1024;

pub fn read_master_token() -> Result<Option<String>, StartupError> {
    read_fixed_text("master-token.txt", TOKEN_AND_PID_LIMIT, "read master token")
}

pub fn read_web_token() -> Result<Option<String>, StartupError> {
    read_fixed_text("web-token.txt", TOKEN_AND_PID_LIMIT, "read web token")
}

pub fn read_daemon_pid() -> Result<Option<String>, StartupError> {
    read_fixed_text("daemon.pid", TOKEN_AND_PID_LIMIT, "read daemon PID")
}

pub fn read_app_outbox_path() -> Result<Option<PathBuf>, StartupError> {
    let Some(text) = read_fixed_text(
        "app-outbox-path.txt",
        OUTBOX_PATH_LIMIT,
        "read app outbox pointer",
    )?
    else {
        return Ok(None);
    };
    let path = PathBuf::from(&text);
    validate_outbox_path(&path)?;
    Ok(Some(path))
}

fn read_fixed_text(
    basename: &'static str,
    limit: usize,
    operation: &'static str,
) -> Result<Option<String>, StartupError> {
    let config_dir = match super::config_dir() {
        Some(config_dir) => config_dir,
        None => {
            #[cfg(target_os = "linux")]
            {
                return Err(StartupError::MissingConfigDir {
                    executable: super::current_executable(),
                });
            }
            #[cfg(not(target_os = "linux"))]
            {
                return Ok(None);
            }
        }
    };
    let path = config_dir.join(basename);

    #[cfg(target_os = "linux")]
    let bytes = {
        let root = super::linux_state::prepared_secure_config_root(operation)?;
        root.read_private_file(std::ffi::OsStr::new(basename), limit, operation)?
    };

    #[cfg(not(target_os = "linux"))]
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => {
            if bytes.len() > limit {
                return Err(StartupError::io(
                    operation,
                    &path,
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("runtime marker exceeds {limit}-byte limit"),
                    ),
                ));
            }
            Some(bytes)
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => return Err(StartupError::io(operation, &path, source)),
    };

    bytes
        .map(|bytes| {
            String::from_utf8(bytes).map_err(|source| {
                StartupError::io(
                    operation,
                    &path,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, source),
                )
            })
        })
        .transpose()
}

fn validate_outbox_path(path: &Path) -> Result<(), StartupError> {
    let config_dir = super::config_dir().ok_or_else(|| StartupError::MissingConfigDir {
        executable: super::current_executable(),
    })?;
    let expected_instances = config_dir.join("instances");
    let Some(instance_dir) = path.parent() else {
        return Err(invalid_outbox_pointer(path));
    };
    if path.file_name() != Some(std::ffi::OsStr::new("outbox")) {
        return Err(invalid_outbox_pointer(path));
    }
    let Some(instance_name) = instance_dir.file_name().and_then(|name| name.to_str()) else {
        return Err(invalid_outbox_pointer(path));
    };
    if instance_dir.parent() != Some(expected_instances.as_path()) {
        return Err(invalid_outbox_pointer(path));
    }
    let Ok(instance_id) = uuid::Uuid::parse_str(instance_name) else {
        return Err(invalid_outbox_pointer(path));
    };
    if instance_id.hyphenated().to_string() != instance_name {
        return Err(invalid_outbox_pointer(path));
    }
    Ok(())
}

fn invalid_outbox_pointer(path: &Path) -> StartupError {
    StartupError::io(
        "validate app outbox pointer",
        path,
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pointer must name canonical config-dir/instances/<uuid>/outbox",
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::validate_outbox_path;

    #[cfg(target_os = "linux")]
    fn read_marker_for_test(basename: &str) -> Result<bool, crate::errors::StartupError> {
        match basename {
            "master-token.txt" => super::read_master_token().map(|value| value.is_some()),
            "web-token.txt" => super::read_web_token().map(|value| value.is_some()),
            "daemon.pid" => super::read_daemon_pid().map(|value| value.is_some()),
            "app-outbox-path.txt" => super::read_app_outbox_path().map(|value| value.is_some()),
            other => panic!("unexpected runtime marker {other}"),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn production_marker_readers_preserve_missing_and_reject_unsafe_or_oversized_leaves() {
        const TEST_NAME: &str =
            "config::runtime_files::tests::production_marker_readers_preserve_missing_and_reject_unsafe_or_oversized_leaves";
        if crate::config::linux_state::rerun_exact_test_with_prepared_root(TEST_NAME) {
            return;
        }

        use crate::errors::{StartupError, UnsafePathReason};
        use std::os::unix::ffi::OsStrExt;

        let config_dir = crate::config::config_dir().expect("child config directory");
        let fixture_dir = config_dir.parent().expect("config directory parent");
        for (index, basename, limit) in [
            (0usize, "master-token.txt", super::TOKEN_AND_PID_LIMIT),
            (1usize, "web-token.txt", super::TOKEN_AND_PID_LIMIT),
            (2usize, "daemon.pid", super::TOKEN_AND_PID_LIMIT),
            (3usize, "app-outbox-path.txt", super::OUTBOX_PATH_LIMIT),
        ] {
            let marker_path = config_dir.join(basename);
            assert!(
                !read_marker_for_test(basename).expect("read missing marker"),
                "{basename} must preserve true-missing behavior"
            );

            let symlink_target = fixture_dir.join(format!("marker-symlink-target-{index}"));
            std::fs::write(&symlink_target, b"unchanged").expect("write symlink target");
            std::os::unix::fs::symlink(&symlink_target, &marker_path)
                .expect("create marker symlink");
            match read_marker_for_test(basename).expect_err("symlink marker must fail") {
                StartupError::UnsafePath {
                    path,
                    reason: UnsafePathReason::Symlink,
                    ..
                } => assert_eq!(path, marker_path),
                other => panic!("expected symlink error for {basename}, got {other:?}"),
            }
            assert_eq!(
                std::fs::read(&symlink_target).expect("read symlink target"),
                b"unchanged"
            );
            std::fs::remove_file(&marker_path).expect("remove marker symlink");

            let hardlink_target = fixture_dir.join(format!("marker-hardlink-target-{index}"));
            std::fs::write(&hardlink_target, b"unchanged").expect("write hard-link target");
            std::fs::hard_link(&hardlink_target, &marker_path).expect("create marker hard link");
            match read_marker_for_test(basename).expect_err("hard-linked marker must fail") {
                StartupError::UnsafePath {
                    path,
                    reason: UnsafePathReason::HardLinked { observed: 2 },
                    ..
                } => assert_eq!(path, marker_path),
                other => panic!("expected hard-link error for {basename}, got {other:?}"),
            }
            assert_eq!(
                std::fs::read(&hardlink_target).expect("read hard-link target"),
                b"unchanged"
            );
            std::fs::remove_file(&marker_path).expect("remove marker hard link");

            let fifo_name =
                std::ffi::CString::new(marker_path.as_os_str().as_bytes()).expect("FIFO path");
            // SAFETY: fifo_name is a live, NUL-terminated pathname and the mode is valid.
            let fifo_result = unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) };
            assert_eq!(
                fifo_result,
                0,
                "create marker FIFO: {}",
                std::io::Error::last_os_error()
            );
            match read_marker_for_test(basename).expect_err("FIFO marker must fail") {
                StartupError::UnsafePath {
                    path,
                    reason: UnsafePathReason::WrongObjectType { .. },
                    ..
                } => assert_eq!(path, marker_path),
                other => panic!("expected FIFO error for {basename}, got {other:?}"),
            }
            std::fs::remove_file(&marker_path).expect("remove marker FIFO");

            std::fs::write(&marker_path, vec![b'x'; limit + 1]).expect("write oversized marker");
            match read_marker_for_test(basename).expect_err("oversized marker must fail") {
                StartupError::Io {
                    path,
                    source,
                    operation: _,
                } => {
                    assert_eq!(path, marker_path);
                    assert_eq!(source.kind(), std::io::ErrorKind::InvalidData);
                }
                other => panic!("expected oversized I/O error for {basename}, got {other:?}"),
            }
            std::fs::remove_file(&marker_path).expect("remove oversized marker");
        }
    }

    #[test]
    fn outbox_pointer_rejects_noncanonical_uuid_shape() {
        let config = match crate::config::config_dir() {
            Some(config) => config,
            None => return,
        };
        let bad = config
            .join("instances")
            .join("550e8400e29b41d4a716446655440000")
            .join("outbox");
        assert!(validate_outbox_path(&bad).is_err());
    }
}
