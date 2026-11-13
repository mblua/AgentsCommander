use super::super::json_object::{
    ensure_config_parent, read_config_root, update_config_json_object, ConfigJsonError,
};
use serde_json::Value;
use std::error::Error as _;
use std::io;
use std::path::Path;

#[test]
fn every_variant_keeps_the_old_text() {
    let path = Path::new("cfg/settings.json");
    let cases = [
        (
            ConfigJsonError::NoParent { path: path.into() },
            format!("Local config {} has no parent directory", path.display()),
        ),
        (
            ConfigJsonError::CreateParent {
                parent: path.into(),
                source: io::Error::other("boom"),
            },
            format!("Failed to create {}: {}", path.display(), "boom"),
        ),
        (
            ConfigJsonError::Read {
                path: path.into(),
                source: io::Error::other("boom"),
            },
            format!("Failed to read {}: {}", path.display(), "boom"),
        ),
        (
            ConfigJsonError::Parse {
                path: path.into(),
                source: serde_json::from_str::<Value>("{bad").unwrap_err(),
            },
            format!(
                "Failed to parse {}: {}",
                path.display(),
                serde_json::from_str::<Value>("{bad").unwrap_err()
            ),
        ),
        (
            ConfigJsonError::NotObject { path: path.into() },
            format!("Local config {} must be a JSON object", path.display()),
        ),
        (
            ConfigJsonError::Missing { path: path.into() },
            format!("Local config {} does not exist", path.display()),
        ),
        (
            ConfigJsonError::Serialize {
                path: path.into(),
                source: serde_json::from_str::<Value>("{bad").unwrap_err(),
            },
            format!(
                "Failed to serialize {}: {}",
                path.display(),
                serde_json::from_str::<Value>("{bad").unwrap_err()
            ),
        ),
        (
            ConfigJsonError::WriteStage {
                tmp: path.into(),
                source: io::Error::other("boom"),
            },
            format!("Failed to write temp config {}: {}", path.display(), "boom"),
        ),
        (
            ConfigJsonError::FlushStage {
                tmp: path.into(),
                source: io::Error::other("boom"),
            },
            format!("Failed to flush temp config {}: {}", path.display(), "boom"),
        ),
        (
            ConfigJsonError::SyncStage {
                tmp: path.into(),
                source: io::Error::other("boom"),
            },
            format!("Failed to sync temp config {}: {}", path.display(), "boom"),
        ),
    ];
    for (error, old_text) in cases {
        assert_eq!(error.to_string(), old_text);
    }
}

#[test]
fn io_variants_keep_the_cause() {
    let path = Path::new("cfg/settings.json");
    let cases = [
        ConfigJsonError::CreateParent {
            parent: path.into(),
            source: io::Error::other("boom"),
        },
        ConfigJsonError::Read {
            path: path.into(),
            source: io::Error::other("boom"),
        },
        ConfigJsonError::WriteStage {
            tmp: path.into(),
            source: io::Error::other("boom"),
        },
        ConfigJsonError::FlushStage {
            tmp: path.into(),
            source: io::Error::other("boom"),
        },
        ConfigJsonError::SyncStage {
            tmp: path.into(),
            source: io::Error::other("boom"),
        },
    ];
    for error in cases {
        let cause = error.source().unwrap().downcast_ref::<io::Error>().unwrap();
        assert_eq!(cause.kind(), io::ErrorKind::Other);
        assert_eq!(cause.to_string(), "boom");
    }
}

#[test]
fn serde_variants_keep_the_cause() {
    let path = Path::new("cfg/settings.json");
    let original = serde_json::from_str::<Value>("{bad")
        .unwrap_err()
        .to_string();
    let cases = [
        ConfigJsonError::Parse {
            path: path.into(),
            source: serde_json::from_str::<Value>("{bad").unwrap_err(),
        },
        ConfigJsonError::Serialize {
            path: path.into(),
            source: serde_json::from_str::<Value>("{bad").unwrap_err(),
        },
    ];
    for error in cases {
        let cause = error
            .source()
            .unwrap()
            .downcast_ref::<serde_json::Error>()
            .unwrap();
        assert_eq!(cause.to_string(), original);
    }
}

#[test]
fn plain_variants_have_no_source() {
    let path = Path::new("cfg/settings.json");
    for error in [
        ConfigJsonError::NoParent { path: path.into() },
        ConfigJsonError::NotObject { path: path.into() },
        ConfigJsonError::Missing { path: path.into() },
    ] {
        assert!(error.source().is_none());
    }
}

#[test]
fn string_conversion_is_display() {
    let error = ConfigJsonError::Missing {
        path: Path::new("cfg/settings.json").into(),
    };
    let text = error.to_string();
    assert_eq!(String::from(error), text);
}

#[test]
fn parse_failure_through_public_fn_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, "{bad").unwrap();
    let error = update_config_json_object(&path, false, |_| Ok(())).unwrap_err();
    let cause = serde_json::from_str::<Value>("{bad").unwrap_err();
    assert_eq!(
        error,
        format!("Failed to parse {}: {}", path.display(), cause)
    );
}

#[test]
fn read_config_root_missing_and_not_object() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let error = read_config_root(&path, false).unwrap_err();
    assert!(matches!(error, ConfigJsonError::Missing { .. }));
    assert_eq!(
        error.to_string(),
        format!("Local config {} does not exist", path.display())
    );
    assert_eq!(
        read_config_root(&path, true).unwrap(),
        serde_json::json!({})
    );
    std::fs::write(&path, "[]").unwrap();
    let error = read_config_root(&path, false).unwrap_err();
    assert!(matches!(error, ConfigJsonError::NotObject { .. }));
    assert_eq!(
        error.to_string(),
        format!("Local config {} must be a JSON object", path.display())
    );
}

#[test]
fn ensure_config_parent_wires_the_right_variant() {
    let empty = Path::new("");
    let error = ensure_config_parent(empty).unwrap_err();
    assert!(matches!(error, ConfigJsonError::NoParent { .. }));
    assert_eq!(
        error.to_string(),
        format!("Local config {} has no parent directory", empty.display())
    );
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("blocker");
    std::fs::write(&parent, "blocker").unwrap();
    let path = parent.join("settings.json");
    let os = std::fs::create_dir_all(&parent).unwrap_err();
    let error = ensure_config_parent(&path).unwrap_err();
    assert!(matches!(error, ConfigJsonError::CreateParent { .. }));
    assert_eq!(
        error.to_string(),
        format!("Failed to create {}: {}", parent.display(), os)
    );
    let cause = error.source().unwrap().downcast_ref::<io::Error>().unwrap();
    assert_eq!(cause.kind(), os.kind());
}
