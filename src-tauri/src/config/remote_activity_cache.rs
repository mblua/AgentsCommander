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

use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

/// The artifact name re-exported so both call sites join the registry's
/// constant through this module instead of importing
/// `config::instance_artifacts` directly (which would add a second
/// CLI-to-registry arc for a name).
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

/// The validated in-memory form of the snapshot: a parsed publication instant
/// and path/state pairs, both awaited by the CLI's pure aggregation.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RemoteActivitySnapshot {
    pub(crate) generated_at: DateTime<Utc>,
    pub(crate) repos: Vec<(String, PersistedCiState)>,
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
}

/// Atomically publish the whole-file snapshot to `path` (the caller joins
/// `REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME` onto its directory, the only permitted
/// destination). Entries are sorted by their RAW path string before
/// serialization, so the bytes are deterministic and no CLI path normalizer is
/// needed here. A failure returns the writer's message and leaves no partial
/// file behind (`write_file_atomic` publishes by rename).
pub(crate) fn write_snapshot(
    path: &Path,
    generated_at: DateTime<Utc>,
    repos: &[(String, PersistedCiState)],
) -> Result<(), String> {
    let mut entries: Vec<RepoDto> = repos
        .iter()
        .map(|(path, ci_state)| RepoDto {
            path: path.clone(),
            ci_state: *ci_state,
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

/// Read and validate `path`, the reader the CLI and the producer round-trip
/// test share. Rejects unreadable bytes, malformed JSON, a schema other than
/// version 1, an unparseable `generatedAt`, and an empty path entry: each is a
/// reason to report `unknown`, never a reason to guess.
pub(crate) fn read_snapshot(path: &Path) -> Result<RemoteActivitySnapshot, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("remote activity snapshot is unreadable: {e}"))?;
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
        repos.push((repo.path, repo.ci_state));
    }
    Ok(RemoteActivitySnapshot {
        generated_at,
        repos,
    })
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
mod tests {
    use super::*;

    fn temp_path() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME);
        (dir, path)
    }

    fn instant(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("instant")
            .with_timezone(&Utc)
    }

    #[test]
    fn write_then_read_round_trip_sorts_raw_paths_and_pins_schema_one() {
        let (_dir, path) = temp_path();
        let generated_at = instant("2026-09-22T11:41:41Z");
        let entries = vec![
            ("z:/repo-b".to_string(), PersistedCiState::Idle),
            ("a:/repo-a".to_string(), PersistedCiState::Running),
        ];
        write_snapshot(&path, generated_at, &entries).expect("write");

        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
        assert_eq!(raw["schemaVersion"], 1);
        assert_eq!(raw["generatedAt"], "2026-09-22T11:41:41Z");
        assert_eq!(raw["repos"][0]["path"], "a:/repo-a");
        assert_eq!(raw["repos"][0]["ciState"], "running");
        assert_eq!(raw["repos"][1]["path"], "z:/repo-b");
        assert_eq!(raw["repos"][1]["ciState"], "idle");

        let snapshot = read_snapshot(&path).expect("read back");
        assert_eq!(snapshot.generated_at, generated_at);
        assert_eq!(
            snapshot.repos,
            vec![
                ("a:/repo-a".to_string(), PersistedCiState::Running),
                ("z:/repo-b".to_string(), PersistedCiState::Idle),
            ]
        );
    }

    #[test]
    fn write_snapshot_leaves_only_the_published_file() {
        let (dir, path) = temp_path();
        write_snapshot(&path, instant("2026-09-22T11:41:41Z"), &[]).expect("write");
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert_eq!(names, vec![REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME.to_string()]);
        let snapshot = read_snapshot(&path).expect("empty snapshot reads");
        assert!(snapshot.repos.is_empty());
    }

    #[test]
    fn read_rejects_schema_other_than_one() {
        let (_dir, path) = temp_path();
        std::fs::write(
            &path,
            br#"{"schemaVersion":2,"generatedAt":"2026-09-22T11:41:41Z","repos":[]}"#,
        )
        .expect("write");
        let error = read_snapshot(&path).expect_err("schema 2 rejected");
        assert!(error.contains("schemaVersion 2"), "{error}");
    }

    #[test]
    fn read_rejects_invalid_generated_at() {
        let (_dir, path) = temp_path();
        std::fs::write(
            &path,
            br#"{"schemaVersion":1,"generatedAt":"not-a-timestamp","repos":[]}"#,
        )
        .expect("write");
        let error = read_snapshot(&path).expect_err("invalid timestamp rejected");
        assert!(error.contains("generatedAt is invalid"), "{error}");
    }

    #[test]
    fn read_rejects_malformed_json_and_empty_paths() {
        let (_dir, path) = temp_path();
        std::fs::write(&path, b"{not json").expect("write");
        assert!(read_snapshot(&path).is_err());

        std::fs::write(
            &path,
            br#"{"schemaVersion":1,"generatedAt":"2026-09-22T11:41:41Z","repos":[{"path":"","ciState":"idle"}]}"#,
        )
        .expect("write");
        let error = read_snapshot(&path).expect_err("empty path rejected");
        assert!(error.contains("empty repo path"), "{error}");
    }

    #[test]
    fn freshness_window_is_closed_at_the_boundary_on_both_sides() {
        let generated_at = instant("2026-09-22T11:41:41Z");
        let snapshot = RemoteActivitySnapshot {
            generated_at,
            repos: Vec::new(),
        };
        let window = ChronoDuration::seconds(REMOTE_ACTIVITY_SNAPSHOT_MAX_AGE_SECS as i64);

        assert!(snapshot_is_fresh(&snapshot, generated_at), "age zero");
        assert!(
            snapshot_is_fresh(&snapshot, generated_at + window),
            "exactly 30 s old"
        );
        assert!(
            !snapshot_is_fresh(
                &snapshot,
                generated_at + window + ChronoDuration::seconds(1)
            ),
            "31 s old is stale"
        );
        assert!(
            snapshot_is_fresh(&snapshot, generated_at - window),
            "exactly 30 s in the future is tolerated skew"
        );
        assert!(
            !snapshot_is_fresh(
                &snapshot,
                generated_at - window - ChronoDuration::seconds(1)
            ),
            "31 s in the future is not authority"
        );
    }
}
