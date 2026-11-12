//! Sole owner of `.ac/context-history/history.sqlite3` and its journal.
//!
//! The store keeps paths, schema, limits and policy; physical directory and
//! inspection operations belong to `filesystem::context_history`. Writers open
//! only for an enabled configuration; explicit reads and hot-journal recovery
//! work regardless of it and never create anything.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, ErrorCode, OpenFlags, OptionalExtension, Row};
use uuid::Uuid;

use super::config::HistoryConfig;
use super::{
    BoundaryReason, HistoryRecord, InitialKind, MatrixType, MetadataSource, RecordKind,
    TeamAttribution,
};
use crate::filesystem::context_history::{self as history_fs, HistoryDatabaseInspection};

/// Directory, relative to the Project AC Root, holding the database.
pub(crate) const HISTORY_DIRECTORY_NAME: &str = "context-history";
/// Database file name inside [`HISTORY_DIRECTORY_NAME`].
pub(crate) const HISTORY_DATABASE_NAME: &str = "history.sqlite3";
/// `PRAGMA user_version` of the schema-2 database.
pub(crate) const DATABASE_USER_VERSION: i64 = 2;
/// Fixed database page size.
pub(crate) const PAGE_SIZE: i64 = 4096;
/// Main database page cap: 16384 pages of 4096 bytes = 64 MiB.
pub(crate) const MAX_PAGE_COUNT: i64 = 16384;
/// Row cap; the oldest rows are evicted first.
pub(crate) const MAX_ROWS: i64 = 100_000;
/// Largest batch accepted by one append.
pub(crate) const MAX_APPEND_BATCH: usize = 256;
/// Further transactions attempted after a storage-full rollback.
pub(crate) const MAX_FULL_RETRIES: u32 = 8;
/// SQLite busy timeout for every connection.
pub(crate) const BUSY_TIMEOUT: Duration = Duration::from_millis(100);
/// Total time budget of one snapshot read, recovery included.
pub(crate) const SNAPSHOT_DEADLINE: Duration = Duration::from_secs(5);
/// Rows fetched per snapshot batch.
pub(crate) const SNAPSHOT_BATCH_ROWS: i64 = 512;

const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
const MAX_DATABASE_BYTES: u64 = (PAGE_SIZE * MAX_PAGE_COUNT) as u64;
const RECOVERY_ACTION: &str = "close other AgentsCommander instances of this project, allow \
    writing history.sqlite3 and its directory, and retry the query";

/// Storage failures; none of them is reported as an empty history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoreError {
    /// A writer was requested while history capture is disabled.
    Disabled,
    /// A record or batch failed validation; nothing was written.
    InvalidRecord(String),
    /// Physical path or I/O failure.
    Io(String),
    /// The database bytes are not a usable SQLite database.
    Corrupt(String),
    /// The database carries another schema version; never migrated here.
    UnsupportedVersion(i64),
    /// Page size or page cap differ from the fixed limits; never repaired.
    StorageLimitMismatch(String),
    /// SQLite reported busy or locked; the transaction was rolled back.
    RetryableBusy,
    /// Storage stayed full after every eviction attempt; prior state kept.
    StorageFull,
    /// A hot journal blocks a read-only snapshot; `action` tells the user how
    /// to let the next attempt recover it.
    RecoveryRequired { action: String },
    /// The snapshot did not finish within [`SNAPSHOT_DEADLINE`].
    DeadlineExceeded,
    /// The database holds more rows than a snapshot may return.
    TooManyRows(i64),
    /// Any other SQLite failure.
    Sqlite(String),
}

fn sqlite_error(error: rusqlite::Error) -> StoreError {
    let Some(code) = error.sqlite_error() else {
        return StoreError::Sqlite(error.to_string());
    };
    if code.extended_code == rusqlite::ffi::SQLITE_READONLY_ROLLBACK {
        return StoreError::RecoveryRequired {
            action: RECOVERY_ACTION.to_string(),
        };
    }
    match code.code {
        ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => StoreError::RetryableBusy,
        ErrorCode::DiskFull => StoreError::StorageFull,
        ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => {
            StoreError::Corrupt(error.to_string())
        }
        _ => StoreError::Sqlite(error.to_string()),
    }
}

/// Fixed limits; tests shrink them to reach real SQLite capacity quickly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoreLimits {
    pub(crate) max_page_count: i64,
    pub(crate) max_rows: i64,
    pub(crate) snapshot_deadline: Duration,
}

impl StoreLimits {
    pub(crate) const PRODUCTION: Self = Self {
        max_page_count: MAX_PAGE_COUNT,
        max_rows: MAX_ROWS,
        snapshot_deadline: SNAPSHOT_DEADLINE,
    };
}

/// `<ac_root>/context-history`.
pub(crate) fn history_directory(ac_root: &Path) -> PathBuf {
    ac_root.join(HISTORY_DIRECTORY_NAME)
}

/// `<ac_root>/context-history/history.sqlite3`.
pub(crate) fn history_database_path(ac_root: &Path) -> PathBuf {
    history_directory(ac_root).join(HISTORY_DATABASE_NAME)
}

fn inspect(path: &Path) -> Result<Option<HistoryDatabaseInspection>, StoreError> {
    history_fs::inspect_history_database(path).map_err(|error| StoreError::Io(error.to_string()))
}

/// Check an existing, non-empty file before any SQLite connection touches it.
fn verify_header(
    inspection: &HistoryDatabaseInspection,
    limits: StoreLimits,
) -> Result<(), StoreError> {
    let max_bytes = u64::try_from(PAGE_SIZE * limits.max_page_count).unwrap_or(MAX_DATABASE_BYTES);
    if inspection.length > max_bytes {
        return Err(StoreError::StorageLimitMismatch(format!(
            "database_too_large: {}",
            inspection.length
        )));
    }
    let header = &inspection.header;
    if header.len() < 64 || &header[..16] != SQLITE_MAGIC {
        return Err(StoreError::Corrupt("not_a_sqlite_database".to_string()));
    }
    let page_size = i64::from(u16::from_be_bytes([header[16], header[17]]));
    let page_size = if page_size == 1 { 65536 } else { page_size };
    if page_size != PAGE_SIZE {
        return Err(StoreError::StorageLimitMismatch(format!(
            "page_size: {page_size}"
        )));
    }
    let user_version = i64::from(i32::from_be_bytes([
        header[60], header[61], header[62], header[63],
    ]));
    if user_version != DATABASE_USER_VERSION {
        return Err(StoreError::UnsupportedVersion(user_version));
    }
    Ok(())
}

fn query_i64(conn: &Connection, pragma: &str) -> Result<i64, StoreError> {
    conn.pragma_query_value(None, pragma, |row| row.get(0))
        .map_err(sqlite_error)
}

/// Verify page size, page count and schema version of an open database.
fn verify_open_database(conn: &Connection, limits: StoreLimits) -> Result<(), StoreError> {
    let page_size = query_i64(conn, "page_size")?;
    if page_size != PAGE_SIZE {
        return Err(StoreError::StorageLimitMismatch(format!(
            "page_size: {page_size}"
        )));
    }
    let page_count = query_i64(conn, "page_count")?;
    if page_count > limits.max_page_count {
        return Err(StoreError::StorageLimitMismatch(format!(
            "page_count: {page_count}"
        )));
    }
    let user_version = query_i64(conn, "user_version")?;
    if user_version != DATABASE_USER_VERSION {
        return Err(StoreError::UnsupportedVersion(user_version));
    }
    Ok(())
}

/// Set and re-read the page cap on every writable connection.
fn apply_page_cap(conn: &Connection, limits: StoreLimits) -> Result<(), StoreError> {
    let applied: i64 = conn
        .pragma_update_and_check(
            Some(rusqlite::DatabaseName::Main),
            "max_page_count",
            limits.max_page_count,
            |row| row.get(0),
        )
        .map_err(sqlite_error)?;
    let reread = query_i64(conn, "max_page_count")?;
    if applied != limits.max_page_count || reread != limits.max_page_count {
        return Err(StoreError::StorageLimitMismatch(format!(
            "max_page_count: {applied}/{reread}"
        )));
    }
    Ok(())
}

const SCHEMA_SQL: &str = "
CREATE TABLE records (
    record_id TEXT PRIMARY KEY NOT NULL,
    schema_version INTEGER NOT NULL CHECK (schema_version = 2),
    app_run_id TEXT NOT NULL,
    runtime_session_id TEXT NOT NULL,
    logical_session_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    observed_at_utc_ms INTEGER NOT NULL CHECK (observed_at_utc_ms >= 0),
    logical_started_at_utc_ms INTEGER
        CHECK (logical_started_at_utc_ms IS NULL OR logical_started_at_utc_ms >= 0),
    partial_start INTEGER NOT NULL CHECK (partial_start IN (0, 1)),
    partial_end INTEGER NOT NULL CHECK (partial_end IN (0, 1)),
    initial_record_id TEXT,
    initial_kind TEXT NOT NULL
        CHECK (initial_kind IN ('confirmed_zero_transition', 'first_observed', 'unknown')),
    continuity_known INTEGER NOT NULL CHECK (continuity_known IN (0, 1)),
    project_path TEXT NOT NULL,
    project_name TEXT NOT NULL,
    agent_key TEXT NOT NULL,
    instance_fqn TEXT NOT NULL,
    room TEXT,
    matrix_type TEXT NOT NULL CHECK (matrix_type IN ('replica', 'origin')),
    team_attribution TEXT NOT NULL
        CHECK (team_attribution IN ('known', 'unknown', 'ambiguous')),
    team_ids TEXT NOT NULL CHECK (json_valid(team_ids) AND json_type(team_ids) = 'array'),
    coding_agent_id TEXT,
    model TEXT,
    effort TEXT,
    metadata_source TEXT NOT NULL CHECK (metadata_source IN ('launch_command', 'unavailable')),
    context_window INTEGER CHECK (context_window IS NULL),
    kind TEXT NOT NULL CHECK (kind IN ('change', 'boundary')),
    percent INTEGER CHECK (percent IS NULL OR percent BETWEEN 0 AND 100),
    boundary_reason TEXT,
    CHECK (
        (kind = 'change' AND percent BETWEEN 1 AND 100 AND boundary_reason IS NULL)
        OR (kind = 'boundary' AND boundary_reason = 'observed_zero' AND percent = 0)
        OR (kind = 'boundary' AND boundary_reason IN ('lifecycle', 'observation_gap')
            AND percent IS NULL)
    ),
    UNIQUE (app_run_id, runtime_session_id, sequence)
);
CREATE INDEX records_observed ON records (observed_at_utc_ms, record_id);
CREATE INDEX records_agent ON records (agent_key, observed_at_utc_ms, record_id);
CREATE INDEX records_logical ON records (logical_session_id, observed_at_utc_ms, record_id);
CREATE INDEX records_started ON records (logical_started_at_utc_ms, logical_session_id);
PRAGMA user_version = 2;
";

fn rollback(conn: &Connection) {
    if !conn.is_autocommit() {
        // A failed rollback leaves SQLite to roll back when the connection closes.
        let _ = conn.execute_batch("ROLLBACK");
    }
}

/// Run `body` inside `BEGIN IMMEDIATE`; any failure rolls back completely.
fn immediate<T>(
    conn: &Connection,
    body: impl FnOnce(&Connection) -> Result<T, StoreError>,
) -> Result<T, StoreError> {
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(sqlite_error)?;
    let result = body(conn).and_then(|value| {
        conn.execute_batch("COMMIT").map_err(sqlite_error)?;
        Ok(value)
    });
    if result.is_err() {
        rollback(conn);
    }
    result
}

fn ensure_schema(conn: &Connection, fresh: bool) -> Result<(), StoreError> {
    if query_i64(conn, "user_version")? == DATABASE_USER_VERSION {
        return Ok(());
    }
    immediate(conn, |conn| {
        // Another writer may have initialized the database while we waited.
        let user_version = query_i64(conn, "user_version")?;
        if user_version == DATABASE_USER_VERSION {
            return Ok(());
        }
        let objects: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |row| row.get(0))
            .map_err(sqlite_error)?;
        if !fresh || user_version != 0 || objects != 0 {
            return Err(StoreError::UnsupportedVersion(user_version));
        }
        conn.execute_batch(SCHEMA_SQL).map_err(sqlite_error)
    })
}

/// A writable connection to an enabled project's history database.
pub(crate) struct HistoryWriter {
    conn: Connection,
    retention_ms: i64,
    limits: StoreLimits,
}

/// Outcome of a committed append.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AppendOutcome {
    /// Records newly inserted (repeated record ids are not counted).
    pub(crate) inserted: usize,
    /// Rows removed by retention, cap or storage-full eviction.
    pub(crate) evicted: usize,
    /// Storage-full retries before the commit.
    pub(crate) full_retries: u32,
}

impl HistoryWriter {
    /// Open (creating when missing) the history database of `ac_root`.
    pub(crate) fn open(ac_root: &Path, config: &HistoryConfig) -> Result<Self, StoreError> {
        Self::open_with_limits(ac_root, config, StoreLimits::PRODUCTION)
    }

    pub(crate) fn open_with_limits(
        ac_root: &Path,
        config: &HistoryConfig,
        limits: StoreLimits,
    ) -> Result<Self, StoreError> {
        if !config.enabled {
            return Err(StoreError::Disabled);
        }
        history_fs::ensure_history_directory(&history_directory(ac_root))
            .map_err(|error| StoreError::Io(error.to_string()))?;
        let path = history_database_path(ac_root);
        let fresh = match inspect(&path)? {
            Some(inspection) if inspection.length > 0 => {
                verify_header(&inspection, limits)?;
                false
            }
            _ => true,
        };
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let conn = Connection::open_with_flags(&path, flags).map_err(sqlite_error)?;
        conn.busy_timeout(BUSY_TIMEOUT).map_err(sqlite_error)?;
        if fresh {
            conn.pragma_update(None, "page_size", PAGE_SIZE)
                .map_err(sqlite_error)?;
        }
        let mode: String = conn
            .pragma_update_and_check(None, "journal_mode", "DELETE", |row| row.get(0))
            .map_err(sqlite_error)?;
        if !mode.eq_ignore_ascii_case("delete") {
            return Err(StoreError::Sqlite(format!("journal_mode: {mode}")));
        }
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(sqlite_error)?;
        apply_page_cap(&conn, limits)?;
        let page_size = query_i64(&conn, "page_size")?;
        if page_size != PAGE_SIZE {
            return Err(StoreError::StorageLimitMismatch(format!(
                "page_size: {page_size}"
            )));
        }
        ensure_schema(&conn, fresh)?;
        verify_open_database(&conn, limits)?;
        Ok(Self {
            conn,
            retention_ms: config.retention_ms(),
            limits,
        })
    }

    /// Append up to [`MAX_APPEND_BATCH`] records in one transaction that also
    /// applies retention and the row cap. Repeated record ids are idempotent.
    ///
    /// On storage-full the transaction rolls back completely and the same
    /// batch is retried in a new transaction that evicts a growing share of
    /// the existing rows; after [`MAX_FULL_RETRIES`] retries the prior state is
    /// kept and [`StoreError::StorageFull`] is returned. Busy or locked rolls
    /// back and returns [`StoreError::RetryableBusy`] without evicting.
    pub(crate) fn append(
        &mut self,
        records: &[HistoryRecord],
        now_utc_ms: i64,
    ) -> Result<AppendOutcome, StoreError> {
        if records.len() > MAX_APPEND_BATCH {
            return Err(StoreError::InvalidRecord("batch_too_large".to_string()));
        }
        for record in records {
            record.validate().map_err(StoreError::InvalidRecord)?;
        }
        let cutoff = now_utc_ms.saturating_sub(self.retention_ms);
        for full_retries in 0..=MAX_FULL_RETRIES {
            let attempt = immediate(&self.conn, |conn| {
                append_once(conn, records, cutoff, self.limits, full_retries)
            });
            match attempt {
                Ok((inserted, evicted)) => {
                    return Ok(AppendOutcome {
                        inserted,
                        evicted,
                        full_retries,
                    })
                }
                Err(StoreError::StorageFull) => {}
                Err(error) => return Err(error),
            }
        }
        Err(StoreError::StorageFull)
    }
}

fn exists(conn: &Connection, record_id: &str) -> Result<bool, StoreError> {
    conn.query_row(
        "SELECT 1 FROM records WHERE record_id = ?1",
        [record_id],
        |_| Ok(()),
    )
    .optional()
    .map(|found| found.is_some())
    .map_err(sqlite_error)
}

/// Evict up to `count` oldest rows by `(observed_at_utc_ms, record_id)`,
/// never a record id of the batch being appended.
fn evict_oldest(conn: &Connection, count: i64, batch_ids: &str) -> Result<usize, StoreError> {
    if count <= 0 {
        return Ok(0);
    }
    conn.execute(
        "DELETE FROM records WHERE record_id IN (
            SELECT record_id FROM records
            WHERE record_id NOT IN (SELECT value FROM json_each(?2))
            ORDER BY observed_at_utc_ms, record_id LIMIT ?1)",
        params![count, batch_ids],
    )
    .map_err(sqlite_error)
}

/// One transaction body: expiry, cap, storage-full eviction, then inserts.
fn append_once(
    conn: &Connection,
    records: &[HistoryRecord],
    cutoff: i64,
    limits: StoreLimits,
    full_retries: u32,
) -> Result<(usize, usize), StoreError> {
    let mut evicted = conn
        .execute(
            "DELETE FROM records WHERE observed_at_utc_ms < ?1",
            [cutoff],
        )
        .map_err(sqlite_error)?;
    let mut ids: Vec<String> = records.iter().map(|r| r.record_id.to_string()).collect();
    ids.sort();
    ids.dedup();
    let mut new_rows = 0_i64;
    for id in &ids {
        if !exists(conn, id)? {
            new_rows += 1;
        }
    }
    let batch_ids = serde_json::to_string(&ids).map_err(|e| StoreError::Sqlite(e.to_string()))?;
    let existing: i64 = conn
        .query_row("SELECT count(*) FROM records", [], |row| row.get(0))
        .map_err(sqlite_error)?;
    let present = i64::try_from(ids.len()).unwrap_or(i64::MAX) - new_rows;
    let evictable = existing - present;
    let over_cap = existing + new_rows - limits.max_rows;
    let full_share = match full_retries {
        0 => 0,
        k if k >= MAX_FULL_RETRIES => evictable,
        k => evictable - (evictable >> k),
    };
    evicted += evict_oldest(conn, over_cap.max(full_share), &batch_ids)?;
    let mut inserted = 0;
    for record in records {
        inserted += insert(conn, record)?;
    }
    Ok((inserted, evicted))
}

fn insert(conn: &Connection, record: &HistoryRecord) -> Result<usize, StoreError> {
    let team_ids =
        serde_json::to_string(&record.team_ids).map_err(|e| StoreError::Sqlite(e.to_string()))?;
    let as_i64 = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
    conn.execute(
        "INSERT INTO records (
            record_id, schema_version, app_run_id, runtime_session_id, logical_session_id,
            sequence, ordinal, observed_at_utc_ms, logical_started_at_utc_ms, partial_start,
            partial_end, initial_record_id, initial_kind, continuity_known, project_path,
            project_name, agent_key, instance_fqn, room, matrix_type, team_attribution,
            team_ids, coding_agent_id, model, effort, metadata_source, context_window, kind,
            percent, boundary_reason
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
            ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)
        ON CONFLICT (record_id) DO NOTHING",
        params![
            record.record_id.to_string(),
            record.schema_version,
            record.app_run_id.to_string(),
            record.runtime_session_id.to_string(),
            record.logical_session_id.to_string(),
            as_i64(record.sequence),
            as_i64(record.ordinal),
            record.observed_at_utc_ms,
            record.logical_started_at_utc_ms,
            record.partial_start,
            record.partial_end,
            record.initial_record_id.map(|id| id.to_string()),
            record.initial_kind.as_str(),
            record.continuity_known,
            record.project_path,
            record.project_name,
            record.agent_key,
            record.instance_fqn,
            record.room,
            record.matrix_type.as_str(),
            record.team_attribution.as_str(),
            team_ids,
            record.coding_agent_id,
            record.model,
            record.effort,
            record.metadata_source.as_str(),
            record.context_window.map(as_i64),
            record.kind.as_str(),
            record.percent,
            record.boundary_reason.map(BoundaryReason::as_str),
        ],
    )
    .map_err(sqlite_error)
}

/// How a snapshot read treats a hot journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadAttempt {
    /// Read-only; a hot journal returns [`StoreError::RecoveryRequired`].
    ReadOnly,
    /// The user's retry: at most one bounded native rollback, then read-only.
    AllowRecovery,
}

/// Read every record, ordered by `(observed_at_utc_ms, record_id)`.
///
/// A missing database is an empty history and nothing is created. Reads
/// never prune, and any failure is an error rather than a partial result.
pub(crate) fn read_history(
    ac_root: &Path,
    attempt: ReadAttempt,
) -> Result<Vec<HistoryRecord>, StoreError> {
    read_history_with_limits(ac_root, attempt, StoreLimits::PRODUCTION)
}

pub(crate) fn read_history_with_limits(
    ac_root: &Path,
    attempt: ReadAttempt,
    limits: StoreLimits,
) -> Result<Vec<HistoryRecord>, StoreError> {
    let deadline = Instant::now() + limits.snapshot_deadline;
    let path = history_database_path(ac_root);
    if !verified_present(&path, limits)? {
        return Ok(Vec::new());
    }
    match snapshot(&path, limits, deadline) {
        Err(StoreError::RecoveryRequired { .. }) if attempt == ReadAttempt::AllowRecovery => {
            if !recover(&path, limits, deadline)? {
                return Ok(Vec::new());
            }
            snapshot(&path, limits, deadline)
        }
        other => other,
    }
}

/// `false` for a missing or empty database; an error for unverifiable bytes.
fn verified_present(path: &Path, limits: StoreLimits) -> Result<bool, StoreError> {
    match inspect(path)? {
        Some(inspection) if inspection.length > 0 => {
            verify_header(&inspection, limits)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Let SQLite roll back its hot journal through one writable connection that
/// never creates, inserts, deletes, prunes or vacuums. Returns `false` when
/// the database disappeared.
fn recover(path: &Path, limits: StoreLimits, deadline: Instant) -> Result<bool, StoreError> {
    if Instant::now() >= deadline {
        return Err(StoreError::DeadlineExceeded);
    }
    if !verified_present(path, limits)? {
        return Ok(false);
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let conn = Connection::open_with_flags(path, flags).map_err(sqlite_error)?;
    conn.busy_timeout(BUSY_TIMEOUT).map_err(sqlite_error)?;
    // The first read takes a shared lock, which rolls the hot journal back.
    let _: i64 = conn
        .query_row("SELECT count(*) FROM sqlite_master", [], |row| row.get(0))
        .map_err(sqlite_error)?;
    verify_open_database(&conn, limits)?;
    drop(conn);
    Ok(true)
}

fn snapshot(
    path: &Path,
    limits: StoreLimits,
    deadline: Instant,
) -> Result<Vec<HistoryRecord>, StoreError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let conn = Connection::open_with_flags(path, flags).map_err(sqlite_error)?;
    conn.busy_timeout(BUSY_TIMEOUT).map_err(sqlite_error)?;
    conn.execute_batch("BEGIN").map_err(sqlite_error)?;
    let result = snapshot_rows(&conn, limits, deadline);
    rollback(&conn);
    result
}

fn snapshot_rows(
    conn: &Connection,
    limits: StoreLimits,
    deadline: Instant,
) -> Result<Vec<HistoryRecord>, StoreError> {
    verify_open_database(conn, limits)?;
    let total: i64 = conn
        .query_row("SELECT count(*) FROM records", [], |row| row.get(0))
        .map_err(sqlite_error)?;
    if total > limits.max_rows {
        return Err(StoreError::TooManyRows(total));
    }
    let mut statement = conn
        .prepare(
            "SELECT * FROM records
            WHERE (observed_at_utc_ms, record_id) > (?1, ?2)
            ORDER BY observed_at_utc_ms, record_id LIMIT ?3",
        )
        .map_err(sqlite_error)?;
    let mut records = Vec::with_capacity(usize::try_from(total).unwrap_or(0));
    let mut cursor = (i64::MIN, String::new());
    loop {
        if Instant::now() >= deadline {
            return Err(StoreError::DeadlineExceeded);
        }
        let batch = statement
            .query_map(params![cursor.0, cursor.1, SNAPSHOT_BATCH_ROWS], read_row)
            .map_err(sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_error)?;
        let Some(last) = batch.last() else {
            return Ok(records);
        };
        cursor = (last.observed_at_utc_ms, last.record_id.to_string());
        for record in batch {
            record.validate().map_err(StoreError::Corrupt)?;
            records.push(record);
        }
    }
}

fn conversion(column: &str, error: &dyn std::fmt::Display) -> rusqlite::Error {
    let detail = format!("{column}: {error}");
    rusqlite::Error::InvalidColumnType(0, detail, rusqlite::types::Type::Text)
}

fn uuid_column(row: &Row<'_>, column: &str) -> rusqlite::Result<Uuid> {
    let text: String = row.get(column)?;
    Uuid::parse_str(&text).map_err(|error| conversion(column, &error))
}

fn enum_column<T>(
    row: &Row<'_>,
    column: &str,
    parse: fn(&str) -> Option<T>,
) -> rusqlite::Result<T> {
    let text: String = row.get(column)?;
    parse(&text).ok_or_else(|| conversion(column, &"unknown value"))
}

fn u64_column(row: &Row<'_>, column: &str) -> rusqlite::Result<u64> {
    let value: i64 = row.get(column)?;
    u64::try_from(value).map_err(|error| conversion(column, &error))
}

fn read_row(row: &Row<'_>) -> rusqlite::Result<HistoryRecord> {
    let team_ids: String = row.get("team_ids")?;
    let initial_record_id: Option<String> = row.get("initial_record_id")?;
    let context_window: Option<i64> = row.get("context_window")?;
    let boundary_reason: Option<String> = row.get("boundary_reason")?;
    Ok(HistoryRecord {
        schema_version: row.get("schema_version")?,
        record_id: uuid_column(row, "record_id")?,
        app_run_id: uuid_column(row, "app_run_id")?,
        runtime_session_id: uuid_column(row, "runtime_session_id")?,
        logical_session_id: uuid_column(row, "logical_session_id")?,
        sequence: u64_column(row, "sequence")?,
        ordinal: u64_column(row, "ordinal")?,
        observed_at_utc_ms: row.get("observed_at_utc_ms")?,
        logical_started_at_utc_ms: row.get("logical_started_at_utc_ms")?,
        partial_start: row.get("partial_start")?,
        partial_end: row.get("partial_end")?,
        initial_record_id: initial_record_id
            .map(|text| {
                Uuid::parse_str(&text).map_err(|error| conversion("initial_record_id", &error))
            })
            .transpose()?,
        initial_kind: enum_column(row, "initial_kind", InitialKind::parse)?,
        continuity_known: row.get("continuity_known")?,
        project_path: row.get("project_path")?,
        project_name: row.get("project_name")?,
        agent_key: row.get("agent_key")?,
        instance_fqn: row.get("instance_fqn")?,
        room: row.get("room")?,
        matrix_type: enum_column(row, "matrix_type", MatrixType::parse)?,
        team_attribution: enum_column(row, "team_attribution", TeamAttribution::parse)?,
        team_ids: serde_json::from_str(&team_ids)
            .map_err(|error| conversion("team_ids", &error))?,
        coding_agent_id: row.get("coding_agent_id")?,
        model: row.get("model")?,
        effort: row.get("effort")?,
        metadata_source: enum_column(row, "metadata_source", MetadataSource::parse)?,
        context_window: context_window
            .map(|value| u64::try_from(value).map_err(|error| conversion("context_window", &error)))
            .transpose()?,
        kind: enum_column(row, "kind", RecordKind::parse)?,
        percent: row.get("percent")?,
        boundary_reason: boundary_reason
            .map(|text| {
                BoundaryReason::parse(&text)
                    .ok_or_else(|| conversion("boundary_reason", &"unknown value"))
            })
            .transpose()?,
    })
}

#[cfg(test)]
mod tests;
