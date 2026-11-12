use super::*;
use crate::context_history::{
    MAX_IDENTITY_BYTES, MAX_METADATA_BYTES, MAX_RECORD_JSON_BYTES, MAX_TEAM_IDS,
};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const NOW: i64 = 1_800_000_000_000;
const HOT_JOURNAL_FIXTURE_ENV: &str = "AC_CONTEXT_HISTORY_HOT_JOURNAL_FIXTURE";

fn enabled() -> HistoryConfig {
    HistoryConfig {
        enabled: true,
        retention_days: 30,
    }
}

fn ac_root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");
    (tmp, ac_root)
}

fn record(sequence: u64, observed_at_utc_ms: i64) -> HistoryRecord {
    HistoryRecord {
        schema_version: 2,
        record_id: Uuid::new_v4(),
        app_run_id: Uuid::from_u128(1),
        runtime_session_id: Uuid::from_u128(2),
        logical_session_id: Uuid::from_u128(3),
        sequence,
        ordinal: 1,
        observed_at_utc_ms,
        logical_started_at_utc_ms: Some(NOW - 1000),
        partial_start: false,
        partial_end: false,
        initial_record_id: Some(Uuid::from_u128(4)),
        initial_kind: InitialKind::FirstObserved,
        continuity_known: true,
        project_path: "C:/projects/demo".to_string(),
        project_name: "demo".to_string(),
        agent_key: "dev-rust".to_string(),
        instance_fqn: "demo:room-1/dev-rust".to_string(),
        room: Some("room-1".to_string()),
        matrix_type: MatrixType::Replica,
        team_attribution: TeamAttribution::Known,
        team_ids: vec!["dev-team".to_string()],
        coding_agent_id: Some("claude".to_string()),
        model: Some("opus".to_string()),
        effort: None,
        metadata_source: MetadataSource::LaunchCommand,
        context_window: None,
        kind: RecordKind::Change,
        percent: Some(42),
        boundary_reason: None,
    }
}

/// A record as close to the 16 KiB JSON limit as whole team ids allow.
fn max_record(sequence: u64, observed_at_utc_ms: i64) -> HistoryRecord {
    let mut record = record(sequence, observed_at_utc_ms);
    record.team_attribution = TeamAttribution::Ambiguous;
    record.team_ids.clear();
    for index in 0..MAX_TEAM_IDS {
        record
            .team_ids
            .push(format!("team-{index:04}-{}", "x".repeat(990)));
        if record.validate() == Err("record_too_large".to_string()) {
            record.team_ids.pop();
            break;
        }
    }
    let size = serde_json::to_vec(&record).expect("serialize").len();
    assert!(
        size > MAX_RECORD_JSON_BYTES - 1100,
        "max record is only {size} bytes"
    );
    assert!(record.validate().is_ok());
    record
}

fn row_count(ac_root: &Path) -> i64 {
    let conn = Connection::open(history_database_path(ac_root)).expect("open");
    conn.query_row("SELECT count(*) FROM records", [], |row| row.get(0))
        .expect("count")
}

fn page_count(ac_root: &Path) -> i64 {
    let conn = Connection::open(history_database_path(ac_root)).expect("open");
    conn.pragma_query_value(None, "page_count", |row| row.get(0))
        .expect("page_count")
}

fn journal_path(ac_root: &Path) -> PathBuf {
    history_directory(ac_root).join("history.sqlite3-journal")
}

fn ids(records: &[HistoryRecord]) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = records.iter().map(|r| r.record_id).collect();
    ids.sort();
    ids
}

// ---- DTO --------------------------------------------------------------------

#[test]
fn dto_accepts_a_valid_change_and_boundaries() {
    assert_eq!(record(1, NOW).validate(), Ok(()));
    let mut zero = record(1, NOW);
    zero.kind = RecordKind::Boundary;
    zero.boundary_reason = Some(BoundaryReason::ObservedZero);
    zero.percent = Some(0);
    assert_eq!(zero.validate(), Ok(()));
    for reason in [BoundaryReason::Lifecycle, BoundaryReason::ObservationGap] {
        let mut boundary = zero.clone();
        boundary.boundary_reason = Some(reason);
        boundary.percent = None;
        assert_eq!(boundary.validate(), Ok(()));
    }
}

#[test]
fn dto_rejects_out_of_range_or_mismatched_values() {
    for percent in [None, Some(0), Some(101)] {
        let mut change = record(1, NOW);
        change.percent = percent;
        assert!(change.validate().is_err(), "change percent {percent:?}");
    }
    let mut gap = record(1, NOW);
    gap.kind = RecordKind::Boundary;
    gap.boundary_reason = Some(BoundaryReason::ObservationGap);
    gap.percent = Some(0);
    assert!(gap.validate().is_err(), "a gap boundary carries no value");
    let mut zero = gap.clone();
    zero.boundary_reason = Some(BoundaryReason::ObservedZero);
    zero.percent = None;
    assert!(zero.validate().is_err(), "an observed zero carries 0");
    let mut window = record(1, NOW);
    window.context_window = Some(200_000);
    assert!(window.validate().is_err());
    let mut version = record(1, NOW);
    version.schema_version = 1;
    assert!(version.validate().is_err());
    let mut sequence = record(1, NOW);
    sequence.sequence = u64::MAX;
    assert!(sequence.validate().is_err());
    let mut observed = record(1, NOW);
    observed.observed_at_utc_ms = -1;
    assert!(observed.validate().is_err());
}

#[test]
fn dto_never_invents_a_lost_start() {
    let mut lost = record(1, NOW);
    lost.logical_started_at_utc_ms = None;
    assert!(
        lost.validate().is_err(),
        "a lost start must be marked partial and unknown"
    );
    lost.partial_start = true;
    assert!(lost.validate().is_err());
    lost.initial_kind = InitialKind::Unknown;
    lost.initial_record_id = None;
    assert_eq!(lost.validate(), Ok(()));
}

#[test]
fn dto_identity_is_exact_bounded_and_never_truncated() {
    for key in ["__agent_dev", "_agent_dev", " dev", "dev ", "room/dev", ""] {
        let mut bad = record(1, NOW);
        bad.agent_key = key.to_string();
        assert!(bad.validate().is_err(), "agent key {key:?}");
    }
    let mut long = record(1, NOW);
    long.project_path = "p".repeat(MAX_IDENTITY_BYTES + 1);
    assert!(long.validate().is_err());
    long.project_path = "p".repeat(MAX_IDENTITY_BYTES);
    assert_eq!(long.validate(), Ok(()));
    let mut model = record(1, NOW);
    model.model = Some("m".repeat(MAX_METADATA_BYTES + 1));
    assert!(model.validate().is_err());
}

#[test]
fn dto_team_ids_are_sorted_unique_and_match_attribution() {
    let mut teams = record(1, NOW);
    teams.team_attribution = TeamAttribution::Ambiguous;
    teams.team_ids = vec!["b".to_string(), "a".to_string()];
    assert!(teams.validate().is_err());
    teams.team_ids = vec!["a".to_string(), "a".to_string()];
    assert!(teams.validate().is_err());
    teams.team_ids = vec!["a".to_string(), "b".to_string()];
    assert_eq!(teams.validate(), Ok(()));
    teams.team_attribution = TeamAttribution::Known;
    assert!(teams.validate().is_err());
    teams.team_attribution = TeamAttribution::Unknown;
    teams.team_ids.clear();
    assert_eq!(teams.validate(), Ok(()));
    teams.team_attribution = TeamAttribution::Known;
    teams.team_ids = vec!["_team_dev".to_string()];
    assert!(teams.validate().is_err());
}

#[test]
fn dto_json_is_camel_case_and_bounded() {
    let json = serde_json::to_value(record(1, NOW)).expect("serialize");
    for field in [
        "recordId",
        "observedAtUtcMs",
        "logicalStartedAtUtcMs",
        "teamIds",
    ] {
        assert!(json.get(field).is_some(), "missing {field}");
    }
    assert_eq!(json["initialKind"], "first_observed");
    assert!(json.get("intervalSeconds").is_none());
    let mut huge = max_record(1, NOW);
    huge.team_ids.push(format!("zz-{}", "y".repeat(1000)));
    assert_eq!(huge.validate(), Err("record_too_large".to_string()));
}

// ---- open, read and append -------------------------------------------------

#[test]
fn reading_a_missing_history_is_empty_and_creates_nothing() {
    let (_tmp, ac_root) = ac_root();
    for attempt in [ReadAttempt::ReadOnly, ReadAttempt::AllowRecovery] {
        assert_eq!(read_history(&ac_root, attempt), Ok(Vec::new()));
    }
    assert!(std::fs::symlink_metadata(history_directory(&ac_root)).is_err());
}

#[test]
fn a_disabled_writer_is_refused_without_creating_anything() {
    let (_tmp, ac_root) = ac_root();
    let result = HistoryWriter::open(&ac_root, &HistoryConfig::DISABLED);
    assert!(matches!(result, Err(StoreError::Disabled)));
    assert!(std::fs::symlink_metadata(history_directory(&ac_root)).is_err());
}

#[test]
fn a_fresh_database_has_the_fixed_layout() {
    let (_tmp, ac_root) = ac_root();
    drop(HistoryWriter::open(&ac_root, &enabled()).expect("open"));
    let conn = Connection::open(history_database_path(&ac_root)).expect("open");
    let pragma = |name: &str| -> i64 {
        conn.pragma_query_value(None, name, |row| row.get(0))
            .expect(name)
    };
    assert_eq!(pragma("page_size"), PAGE_SIZE);
    assert_eq!(pragma("user_version"), DATABASE_USER_VERSION);
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .expect("journal_mode");
    assert_eq!(mode, "delete");
    let indexes: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'records_%'")
        .expect("prepare")
        .query_map([], |row| row.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("collect");
    for index in [
        "records_observed",
        "records_agent",
        "records_logical",
        "records_started",
    ] {
        assert!(indexes.iter().any(|name| name == index), "missing {index}");
    }
}

#[test]
fn records_roundtrip_reopen_and_repeat_idempotently() {
    let (_tmp, ac_root) = ac_root();
    let mut lost = record(3, NOW + 2);
    lost.logical_started_at_utc_ms = None;
    lost.partial_start = true;
    lost.initial_kind = InitialKind::Unknown;
    lost.initial_record_id = None;
    lost.room = None;
    lost.matrix_type = MatrixType::Origin;
    let batch = vec![record(1, NOW), record(2, NOW + 1), lost];
    let mut writer = HistoryWriter::open(&ac_root, &enabled()).expect("open");
    assert_eq!(writer.append(&batch, NOW).expect("append").inserted, 3);
    drop(writer);
    let mut writer = HistoryWriter::open(&ac_root, &enabled()).expect("reopen");
    let again = writer.append(&batch, NOW).expect("append again");
    assert_eq!((again.inserted, again.evicted), (0, 0));
    assert_eq!(read_history(&ac_root, ReadAttempt::ReadOnly), Ok(batch));
}

#[test]
fn invalid_batches_write_nothing() {
    let (_tmp, ac_root) = ac_root();
    let mut writer = HistoryWriter::open(&ac_root, &enabled()).expect("open");
    let mut bad = record(2, NOW);
    bad.percent = Some(0);
    let result = writer.append(&[record(1, NOW), bad], NOW);
    assert!(matches!(result, Err(StoreError::InvalidRecord(_))));
    let oversized: Vec<_> = (0..=MAX_APPEND_BATCH as u64)
        .map(|s| record(s, NOW))
        .collect();
    let result = writer.append(&oversized, NOW);
    assert!(matches!(result, Err(StoreError::InvalidRecord(_))));
    assert_eq!(row_count(&ac_root), 0);
}

#[test]
fn two_writers_share_one_database() {
    let (_tmp, ac_root) = ac_root();
    let mut first = HistoryWriter::open(&ac_root, &enabled()).expect("first");
    let mut second = HistoryWriter::open(&ac_root, &enabled()).expect("second");
    let mut expected = Vec::new();
    for sequence in 0..6 {
        let record = record(sequence, NOW + sequence as i64);
        let writer = if sequence % 2 == 0 {
            &mut first
        } else {
            &mut second
        };
        writer
            .append(std::slice::from_ref(&record), NOW)
            .expect("append");
        expected.push(record);
    }
    assert_eq!(read_history(&ac_root, ReadAttempt::ReadOnly), Ok(expected));
}

#[test]
fn retention_expires_old_records_in_the_append_transaction() {
    let (_tmp, ac_root) = ac_root();
    let config = HistoryConfig {
        enabled: true,
        retention_days: 1,
    };
    let mut writer = HistoryWriter::open(&ac_root, &config).expect("open");
    let old = record(1, NOW - 2 * DAY_MS);
    let kept = record(2, NOW - DAY_MS / 2);
    writer
        .append(&[old, kept.clone()], NOW - 2 * DAY_MS)
        .expect("seed");
    let fresh = record(3, NOW);
    let outcome = writer
        .append(std::slice::from_ref(&fresh), NOW)
        .expect("append");
    assert_eq!(outcome.evicted, 1);
    assert_eq!(
        read_history(&ac_root, ReadAttempt::ReadOnly),
        Ok(vec![kept, fresh])
    );
}

#[test]
fn the_row_cap_evicts_the_oldest_first() {
    assert_eq!(MAX_ROWS, 100_000);
    let (_tmp, ac_root) = ac_root();
    let limits = StoreLimits {
        max_rows: 5,
        ..StoreLimits::PRODUCTION
    };
    let mut writer = HistoryWriter::open_with_limits(&ac_root, &enabled(), limits).expect("open");
    let all: Vec<_> = (0..8).map(|s| record(s, NOW + s as i64)).collect();
    for chunk in all.chunks(3) {
        writer.append(chunk, NOW).expect("append");
    }
    // Repeating a retained id neither evicts nor duplicates.
    let outcome = writer.append(&all[7..], NOW).expect("repeat");
    assert_eq!((outcome.inserted, outcome.evicted), (0, 0));
    assert_eq!(
        read_history(&ac_root, ReadAttempt::ReadOnly),
        Ok(all[3..].to_vec())
    );
}

// ---- capacity ---------------------------------------------------------------

fn assert_bounded(ac_root: &Path) {
    assert!(row_count(ac_root) <= MAX_ROWS);
    assert!(page_count(ac_root) <= MAX_PAGE_COUNT);
    let length = std::fs::metadata(history_database_path(ac_root))
        .expect("metadata")
        .len();
    assert!(length <= (PAGE_SIZE * MAX_PAGE_COUNT) as u64);
    // DELETE journal mode: a committed append leaves no journal behind.
    assert!(std::fs::symlink_metadata(journal_path(ac_root)).is_err());
}

#[test]
fn real_saturation_keeps_recent_records_within_the_page_cap() {
    let (_tmp, ac_root) = ac_root();
    let mut writer = HistoryWriter::open(&ac_root, &enabled()).expect("open");
    let mut sequence = 0_u64;
    let mut next_batch = |size: usize| -> Vec<HistoryRecord> {
        (0..size)
            .map(|_| {
                sequence += 1;
                max_record(sequence, NOW + sequence as i64)
            })
            .collect()
    };
    let mut full_cycles = 0;
    let mut last;
    let mut first_ids = Vec::new();
    for _ in 0..40 {
        last = next_batch(MAX_APPEND_BATCH);
        let outcome = writer.append(&last, NOW).expect("append");
        assert_eq!(outcome.inserted, MAX_APPEND_BATCH);
        if first_ids.is_empty() {
            first_ids = ids(&last);
        }
        if outcome.full_retries > 0 {
            full_cycles += 1;
            if full_cycles == 3 {
                break;
            }
        }
    }
    assert_eq!(
        full_cycles, 3,
        "the cap must be reached and passed several times"
    );
    assert_bounded(&ac_root);
    // Short batches keep working at saturation.
    for _ in 0..3 {
        last = next_batch(1);
        writer.append(&last, NOW).expect("short append");
    }
    drop(writer);
    let mut first = HistoryWriter::open(&ac_root, &enabled()).expect("reopen saturated");
    let mut second = HistoryWriter::open(&ac_root, &enabled()).expect("second writer");
    let mut recent = Vec::new();
    for use_second in [false, true, false] {
        let writer = if use_second { &mut second } else { &mut first };
        last = next_batch(MAX_APPEND_BATCH);
        writer.append(&last, NOW).expect("saturated append");
        recent.extend(last.iter().map(|r| r.record_id));
    }
    assert_bounded(&ac_root);
    let stored = ids(&read_history(&ac_root, ReadAttempt::ReadOnly).expect("read"));
    for id in &recent {
        assert!(stored.binary_search(id).is_ok(), "recent record evicted");
    }
    for id in &first_ids {
        assert!(stored.binary_search(id).is_err(), "oldest record retained");
    }
    assert!(
        stored.len() < usize::try_from(sequence).expect("fits"),
        "old rows evicted"
    );
}

#[test]
fn storage_full_after_eight_retries_keeps_the_prior_state() {
    let (_tmp, ac_root) = ac_root();
    let limits = StoreLimits {
        max_page_count: 64,
        ..StoreLimits::PRODUCTION
    };
    let mut writer = HistoryWriter::open_with_limits(&ac_root, &enabled(), limits).expect("open");
    let prior = vec![record(1, NOW), record(2, NOW + 1)];
    writer.append(&prior, NOW).expect("seed");
    let bytes_before = std::fs::read(history_database_path(&ac_root)).expect("read");
    let huge: Vec<_> = (10..10 + MAX_APPEND_BATCH as u64)
        .map(|s| max_record(s, NOW + s as i64))
        .collect();
    assert_eq!(writer.append(&huge, NOW), Err(StoreError::StorageFull));
    assert_eq!(read_history(&ac_root, ReadAttempt::ReadOnly), Ok(prior));
    let bytes_after = std::fs::read(history_database_path(&ac_root)).expect("read");
    assert_eq!(bytes_before, bytes_after, "no eviction-only commit");
}

// ---- unusable databases -----------------------------------------------------

fn assert_untouched<T: std::fmt::Debug>(
    ac_root: &Path,
    bytes: &[u8],
    result: Result<T, StoreError>,
    expected: &StoreError,
) {
    match result {
        Err(error) => assert_eq!(&error, expected),
        Ok(value) => panic!("expected {expected:?}, got {value:?}"),
    }
    let after = std::fs::read(history_database_path(ac_root)).expect("read");
    assert_eq!(after, bytes, "bytes must be preserved");
}

fn check_rejected(ac_root: &Path, expected: StoreError) {
    let bytes = std::fs::read(history_database_path(ac_root)).expect("read");
    let writer = HistoryWriter::open(ac_root, &enabled()).map(|_| ());
    assert_untouched(ac_root, &bytes, writer, &expected);
    for attempt in [ReadAttempt::ReadOnly, ReadAttempt::AllowRecovery] {
        assert_untouched(ac_root, &bytes, read_history(ac_root, attempt), &expected);
    }
}

fn set_user_version(ac_root: &Path, version: i64) {
    drop(HistoryWriter::open(ac_root, &enabled()).expect("open"));
    let conn = Connection::open(history_database_path(ac_root)).expect("open");
    conn.pragma_update(None, "user_version", version)
        .expect("user_version");
}

#[test]
fn corrupt_databases_are_preserved() {
    let (_tmp, ac_root) = ac_root();
    std::fs::create_dir(history_directory(&ac_root)).expect("mkdir");
    std::fs::write(history_database_path(&ac_root), vec![0x5a; 4096]).expect("write");
    check_rejected(
        &ac_root,
        StoreError::Corrupt("not_a_sqlite_database".to_string()),
    );
}

#[test]
fn newer_and_v1_databases_are_never_migrated() {
    for version in [1, 3] {
        let (_tmp, ac_root) = ac_root();
        set_user_version(&ac_root, version);
        check_rejected(&ac_root, StoreError::UnsupportedVersion(version));
    }
}

#[test]
fn a_foreign_page_size_is_a_limit_mismatch() {
    let (_tmp, ac_root) = ac_root();
    std::fs::create_dir(history_directory(&ac_root)).expect("mkdir");
    let conn = Connection::open(history_database_path(&ac_root)).expect("open");
    conn.pragma_update(None, "page_size", 1024)
        .expect("page_size");
    conn.execute_batch(SCHEMA_SQL).expect("schema");
    drop(conn);
    check_rejected(
        &ac_root,
        StoreError::StorageLimitMismatch("page_size: 1024".to_string()),
    );
}

#[test]
fn a_database_beyond_the_page_cap_is_a_limit_mismatch() {
    let (_tmp, ac_root) = ac_root();
    let mut writer = HistoryWriter::open(&ac_root, &enabled()).expect("open");
    let batch: Vec<_> = (0..8).map(|s| max_record(s, NOW + s as i64)).collect();
    writer.append(&batch, NOW).expect("append");
    drop(writer);
    let bytes = std::fs::read(history_database_path(&ac_root)).expect("read");
    let limits = StoreLimits {
        max_page_count: 8,
        ..StoreLimits::PRODUCTION
    };
    let result = HistoryWriter::open_with_limits(&ac_root, &enabled(), limits).map(|_| ());
    assert!(matches!(result, Err(StoreError::StorageLimitMismatch(_))));
    let after = std::fs::read(history_database_path(&ac_root)).expect("read");
    assert_eq!(after, bytes);
}

#[test]
fn a_snapshot_past_its_deadline_fails_instead_of_returning_part() {
    let (_tmp, ac_root) = ac_root();
    let mut writer = HistoryWriter::open(&ac_root, &enabled()).expect("open");
    writer.append(&[record(1, NOW)], NOW).expect("append");
    let limits = StoreLimits {
        snapshot_deadline: Duration::ZERO,
        ..StoreLimits::PRODUCTION
    };
    let result = read_history_with_limits(&ac_root, ReadAttempt::ReadOnly, limits);
    assert_eq!(result, Err(StoreError::DeadlineExceeded));
}

#[test]
fn a_plain_read_leaves_the_database_byte_identical() {
    let (_tmp, ac_root) = ac_root();
    let mut writer = HistoryWriter::open(&ac_root, &enabled()).expect("open");
    writer.append(&[record(1, NOW)], NOW).expect("append");
    drop(writer);
    let bytes = std::fs::read(history_database_path(&ac_root)).expect("read");
    for attempt in [ReadAttempt::ReadOnly, ReadAttempt::AllowRecovery] {
        assert_eq!(read_history(&ac_root, attempt).expect("read").len(), 1);
    }
    let after = std::fs::read(history_database_path(&ac_root)).expect("read");
    assert_eq!(after, bytes);
}

// ---- concurrency and recovery -----------------------------------------------

#[test]
fn a_reader_makes_commit_busy_and_the_same_id_lands_once_later() {
    let (_tmp, ac_root) = ac_root();
    let mut writer = HistoryWriter::open(&ac_root, &enabled()).expect("open");
    writer.append(&[record(1, NOW)], NOW).expect("seed");
    let reader = Connection::open_with_flags(
        history_database_path(&ac_root),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("reader");
    reader.execute_batch("BEGIN").expect("begin");
    let _: i64 = reader
        .query_row("SELECT count(*) FROM records", [], |row| row.get(0))
        .expect("hold a shared lock");
    let pending = record(2, NOW + 1);
    let batch = std::slice::from_ref(&pending);
    assert_eq!(writer.append(batch, NOW), Err(StoreError::RetryableBusy));
    reader.execute_batch("ROLLBACK").expect("release");
    drop(reader);
    assert_eq!(writer.append(batch, NOW).expect("retry").inserted, 1);
    assert_eq!(writer.append(batch, NOW).expect("repeat").inserted, 0);
    assert_eq!(row_count(&ac_root), 2);
}

/// Child half of the hot-journal fixture: start a large transaction that
/// spills into the database file, then exit without committing or closing.
#[test]
fn hot_journal_fixture_child() {
    let Some(path) = std::env::var_os(HOT_JOURNAL_FIXTURE_ENV) else {
        return;
    };
    let conn = Connection::open(PathBuf::from(path)).expect("open");
    conn.pragma_update(None, "cache_size", 10)
        .expect("cache_size");
    conn.execute_batch("BEGIN IMMEDIATE").expect("begin");
    for sequence in 1000..1300 {
        insert(&conn, &max_record(sequence, NOW + sequence as i64)).expect("insert");
    }
    #[expect(
        clippy::exit,
        reason = "the fixture must die mid-transaction without closing the connection"
    )]
    std::process::exit(0);
}

fn interrupt_a_transaction(ac_root: &Path) {
    let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "context_history::store::tests::hot_journal_fixture_child",
            "--test-threads=1",
        ])
        .env(HOT_JOURNAL_FIXTURE_ENV, history_database_path(ac_root))
        .status()
        .expect("run fixture child");
    assert!(status.success());
    let journal = std::fs::metadata(journal_path(ac_root)).expect("hot journal left behind");
    assert!(journal.len() > 0);
}

fn recover_hot_journal(config_enabled: bool) {
    let (_tmp, ac_root) = ac_root();
    let config = format!(r#"{{"version":1,"enabled":{config_enabled}}}"#);
    std::fs::write(ac_root.join("heatmap.json"), &config).expect("config");
    let committed = vec![record(1, NOW), record(2, NOW + 1)];
    let mut writer = HistoryWriter::open(&ac_root, &enabled()).expect("open");
    writer.append(&committed, NOW).expect("seed");
    drop(writer);
    interrupt_a_transaction(&ac_root);

    let required = read_history(&ac_root, ReadAttempt::ReadOnly);
    assert!(
        matches!(required, Err(StoreError::RecoveryRequired { ref action })
        if action.contains("retry"))
    );
    assert_eq!(
        read_history(&ac_root, ReadAttempt::AllowRecovery),
        Ok(committed.clone()),
        "rollback restores the committed rows and adds none"
    );
    assert!(std::fs::symlink_metadata(journal_path(&ac_root)).is_err());
    assert_eq!(read_history(&ac_root, ReadAttempt::ReadOnly), Ok(committed));
    let config_after = std::fs::read_to_string(ac_root.join("heatmap.json")).expect("config");
    assert_eq!(
        config_after, config,
        "recovery never touches the configuration"
    );
}

#[test]
fn an_interrupted_transaction_is_recovered_on_retry_when_enabled() {
    recover_hot_journal(true);
}

#[test]
fn an_interrupted_transaction_is_recovered_on_retry_when_disabled() {
    recover_hot_journal(false);
}
