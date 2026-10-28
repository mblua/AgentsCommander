//! #2374 - the neutral, versioned on-disk snapshot of the remote CI view.
//!
//! The PTY sweeper produces it and the `room activity` CLI reads it, and the
//! two must agree on the bytes without either depending on the other: the
//! producer lives in `pty::remote_watcher` and the reader in `cli::workgroup`,
//! so this sibling module owns the schema, the codec and the freshness window
//! both sides call. It points only DOWN, at the artifact registry that names
//! the file and the local config writer that publishes it.
//!
//! The file is machine-local, regenerated on every sweep and never committed
//! (see the registry row in `config::instance_artifacts`). Older binaries
//! ignore it; newer binaries tolerate its absence, so no migration exists.

use std::path::Path;

use crate::filesystem::remote_activity;

use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

/// Artifact name shared by owner directory operations and compatible path-based tests.
pub(crate) use super::instance_artifacts::REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME;
use super::local_config_io::write_file_atomic;

/// How long a published snapshot stays authoritative, in seconds.
///
/// Pinned to three `ROUND_INTERVAL` ticks (3 x 10 s). A live daemon republishes
/// every round, so a file older than three rounds means the producer stopped
/// publishing even though the PID is still alive; the reader must stop lending
/// the stale bytes authority rather than let a running daemon make old CI
/// states current.
pub(crate) const REMOTE_ACTIVITY_SNAPSHOT_MAX_AGE_SECS: u64 = 30;

/// The only schema version this module writes or accepts.
const REMOTE_ACTIVITY_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// One repo's CI state as published on disk. Deliberately a neutral copy of
/// `pty::remote_watcher::CiState`: neither side of the producer/reader seam
/// imports the other's type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PersistedCiState {
    Running,
    Idle,
    Unknown,
}

/// One repo's published CI view: the state plus the #2473 detail. `run_ids`
/// and `pull_requests` are meaningful only while `Running`; `unknown_reason`
/// only while `Unknown` (one of the `ci-query-*` producer codes).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PersistedRepoCi {
    pub(crate) state: PersistedCiState,
    pub(crate) run_ids: Vec<u64>,
    pub(crate) pull_requests: Vec<u64>,
    pub(crate) unknown_reason: Option<String>,
}

/// The validated in-memory form of the snapshot: a parsed publication instant
/// and path/CI pairs, both awaited by the CLI's pure aggregation.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RemoteActivitySnapshot {
    pub(crate) generated_at: DateTime<Utc>,
    pub(crate) repos: Vec<(String, PersistedRepoCi)>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotDto {
    schema_version: u32,
    generated_at: String,
    repos: Vec<RepoDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoDto {
    path: String,
    ci_state: PersistedCiState,
    /// #2473 additive fields: absent in older bytes, omitted when empty so the
    /// pre-#2473 shape is byte-identical. Schema stays 1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    run_ids: Vec<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pull_requests: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unknown_reason: Option<String>,
}

fn sorted_dedup(values: &[u64]) -> Vec<u64> {
    let mut values = values.to_vec();
    values.sort_unstable();
    values.dedup();
    values
}

/// Atomically publish to an explicit path for compatibility with existing fixtures.
/// Production callers use `write_snapshot_to_dir`. Entries are sorted by their RAW path
/// string before serialization, so the bytes are deterministic and no CLI path normalizer is
/// needed here. A failure returns the writer's message and leaves no partial
/// file behind (`write_file_atomic` publishes by rename).
pub(crate) fn write_snapshot(
    path: &Path,
    generated_at: DateTime<Utc>,
    repos: &[(String, PersistedRepoCi)],
) -> Result<(), String> {
    let mut entries: Vec<RepoDto> = repos
        .iter()
        .map(|(path, ci)| RepoDto {
            path: path.clone(),
            ci_state: ci.state,
            run_ids: sorted_dedup(&ci.run_ids),
            pull_requests: sorted_dedup(&ci.pull_requests),
            unknown_reason: ci.unknown_reason.clone(),
        })
        .collect();
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let dto = SnapshotDto {
        schema_version: REMOTE_ACTIVITY_SNAPSHOT_SCHEMA_VERSION,
        generated_at: generated_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        repos: entries,
    };
    let bytes = serde_json::to_vec_pretty(&dto)
        .map_err(|e| format!("failed to serialize remote activity snapshot: {e}"))?;
    write_file_atomic(path, &bytes)
}

/// Publish the instance snapshot in its config directory using the existing atomic codec.
pub(crate) fn write_snapshot_to_dir(
    dir: &Path,
    generated_at: DateTime<Utc>,
    repos: &[(String, PersistedRepoCi)],
) -> Result<(), String> {
    write_snapshot(
        &dir.join(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME),
        generated_at,
        repos,
    )
}

/// Read and validate `path`, the reader the CLI and the producer round-trip
/// test share. Rejects unreadable bytes, malformed JSON, a schema other than
/// version 1, an unparseable `generatedAt`, and an empty path entry: each is a
/// reason to report `unknown`, never a reason to guess.
pub(crate) fn read_snapshot(path: &Path) -> Result<RemoteActivitySnapshot, String> {
    let bytes = remote_activity::read_activity_snapshot(path)
        .map_err(|e| format!("remote activity snapshot is unreadable: {e}"))?;
    let dto: SnapshotDto = serde_json::from_slice(&bytes)
        .map_err(|e| format!("remote activity snapshot is malformed: {e}"))?;
    if dto.schema_version != REMOTE_ACTIVITY_SNAPSHOT_SCHEMA_VERSION {
        return Err(format!(
            "remote activity snapshot schemaVersion {} is not supported",
            dto.schema_version
        ));
    }
    let generated_at = DateTime::parse_from_rfc3339(&dto.generated_at)
        .map_err(|e| format!("remote activity snapshot generatedAt is invalid: {e}"))?
        .with_timezone(&Utc);
    let mut repos = Vec::with_capacity(dto.repos.len());
    for repo in dto.repos {
        if repo.path.is_empty() {
            return Err("remote activity snapshot has an empty repo path".to_string());
        }
        repos.push((
            repo.path,
            PersistedRepoCi {
                state: repo.ci_state,
                run_ids: repo.run_ids,
                pull_requests: repo.pull_requests,
                unknown_reason: repo.unknown_reason,
            },
        ));
    }
    Ok(RemoteActivitySnapshot {
        generated_at,
        repos,
    })
}

/// Read the instance snapshot from its config directory using the existing codec.
pub(crate) fn read_snapshot_from_dir(dir: &Path) -> Result<RemoteActivitySnapshot, String> {
    read_snapshot(&dir.join(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME))
}

/// Whether `snapshot` may be read as current at `now`: at most
/// `REMOTE_ACTIVITY_SNAPSHOT_MAX_AGE_SECS` old and no more than the same window
/// in the future (a small clock skew is tolerated; a large one is not
/// authority). The window is closed at both ends. Pure, so every boundary is
/// tested with injected instants and no sleeping.
pub(crate) fn snapshot_is_fresh(snapshot: &RemoteActivitySnapshot, now: DateTime<Utc>) -> bool {
    let window = ChronoDuration::seconds(REMOTE_ACTIVITY_SNAPSHOT_MAX_AGE_SECS as i64);
    let age = now.signed_duration_since(snapshot.generated_at);
    age >= -window && age <= window
}

#[cfg(test)]
mod tests;
