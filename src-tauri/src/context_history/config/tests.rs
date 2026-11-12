use super::*;

#[test]
fn disabled_is_off_with_the_default_thirty_days() {
    assert_eq!(
        HistoryConfig::DISABLED,
        HistoryConfig {
            enabled: false,
            retention_days: 30,
        }
    );
}

#[test]
fn retention_ms_covers_whole_days_at_both_bounds() {
    let day = 24 * 60 * 60 * 1000;
    for days in [1, 30, 365] {
        let config = HistoryConfig {
            enabled: true,
            retention_days: days,
        };
        assert_eq!(config.retention_ms(), i64::from(days) * day);
    }
}

#[test]
fn loaded_values_compare_by_config_and_status() {
    let loaded = LoadedHistoryConfig {
        config: HistoryConfig::DISABLED,
        status: ConfigStatus::Loaded,
    };
    assert_eq!(loaded.clone(), loaded);
    assert_ne!(
        loaded,
        LoadedHistoryConfig {
            config: HistoryConfig::DISABLED,
            status: ConfigStatus::Missing,
        }
    );
    assert_ne!(
        ConfigStatus::Invalid("a".to_string()),
        ConfigStatus::Invalid("b".to_string())
    );
}
