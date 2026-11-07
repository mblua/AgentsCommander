use super::*;

fn ac_root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");
    (tmp, ac_root)
}

fn load_bytes(bytes: &[u8]) -> (LoadedHistoryConfig, Vec<u8>) {
    let (_tmp, ac_root) = ac_root();
    let path = config_path(&ac_root);
    std::fs::write(&path, bytes).expect("write config");
    let loaded = load_history_config(&ac_root);
    let after = std::fs::read(&path).expect("read config back");
    (loaded, after)
}

fn assert_invalid(bytes: &[u8], expected: &str) {
    let (loaded, after) = load_bytes(bytes);
    assert_eq!(loaded.config, HistoryConfig::DISABLED);
    match loaded.status {
        ConfigStatus::Invalid(diagnostic) => {
            assert!(
                diagnostic.contains(expected),
                "{diagnostic:?} lacks {expected:?}"
            );
            assert!(diagnostic.chars().count() <= MAX_DIAGNOSTIC_CHARS);
        }
        other => panic!("expected invalid, got {other:?}"),
    }
    assert_eq!(
        after, bytes,
        "an invalid config is never repaired or rewritten"
    );
}

#[test]
fn missing_config_is_disabled_and_creates_nothing() {
    let (_tmp, ac_root) = ac_root();
    let loaded = load_history_config(&ac_root);
    assert_eq!(loaded.config, HistoryConfig::DISABLED);
    assert_eq!(loaded.status, ConfigStatus::Missing);
    assert_eq!(std::fs::read_dir(&ac_root).expect("list .ac").count(), 0);
}

#[test]
fn missing_ac_root_is_disabled_and_creates_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    let loaded = load_history_config(&ac_root);
    assert_eq!(loaded.status, ConfigStatus::Missing);
    assert!(std::fs::symlink_metadata(&ac_root).is_err());
}

#[test]
fn minimal_config_defaults_to_disabled_thirty_days() {
    let (loaded, _) = load_bytes(br#"{"version":1}"#);
    assert_eq!(loaded.status, ConfigStatus::Loaded);
    assert_eq!(loaded.config, HistoryConfig::DISABLED);
    assert_eq!(loaded.config.retention_days, 30);
}

#[test]
fn enabled_config_with_retention_loads() {
    let (loaded, _) = load_bytes(br#"{"version":1,"enabled":true,"retentionDays":365}"#);
    assert_eq!(loaded.status, ConfigStatus::Loaded);
    assert_eq!(
        loaded.config,
        HistoryConfig {
            enabled: true,
            retention_days: 365
        }
    );
    assert_eq!(loaded.config.retention_ms(), 365 * 24 * 60 * 60 * 1000);
}

#[test]
fn retention_bounds_are_inclusive() {
    let (loaded, _) = load_bytes(br#"{"version":1,"retentionDays":1}"#);
    assert_eq!(loaded.config.retention_days, 1);
    assert_invalid(
        br#"{"version":1,"retentionDays":0}"#,
        "retention_days_out_of_range",
    );
    assert_invalid(
        br#"{"version":1,"retentionDays":366}"#,
        "retention_days_out_of_range",
    );
}

#[test]
fn obsolete_interval_seconds_disables_with_diagnostic() {
    assert_invalid(
        br#"{"version":1,"enabled":true,"intervalSeconds":60}"#,
        "obsolete_interval_seconds",
    );
}

#[test]
fn other_versions_disable_with_diagnostic() {
    assert_invalid(br#"{"version":2,"enabled":true}"#, "unsupported_version");
    assert_invalid(br#"{"enabled":true}"#, "invalid_config");
}

#[test]
fn strict_parsing_rejects_unknown_duplicate_and_mistyped_fields() {
    assert_invalid(
        br#"{"version":1,"enabled":true,"extra":1}"#,
        "invalid_config",
    );
    assert_invalid(
        br#"{"version":1,"enabled":true,"enabled":false}"#,
        "invalid_json",
    );
    assert_invalid(br#"{"version":1,"enabled":"yes"}"#, "invalid_config");
    assert_invalid(b"not json", "invalid_json");
}

#[test]
fn oversized_config_disables_with_bounded_diagnostic() {
    let mut bytes = br#"{"version":1,"enabled":true,"pad":""#.to_vec();
    bytes.resize(MAX_CONFIG_BYTES + 1, b' ');
    assert_invalid(&bytes, "unreadable");
}

#[test]
fn a_directory_in_place_of_the_config_is_unreadable() {
    let (_tmp, ac_root) = ac_root();
    std::fs::create_dir(config_path(&ac_root)).expect("create directory");
    let loaded = load_history_config(&ac_root);
    assert_eq!(loaded.config, HistoryConfig::DISABLED);
    assert!(matches!(loaded.status, ConfigStatus::Invalid(ref d) if d.contains("unreadable")));
}
