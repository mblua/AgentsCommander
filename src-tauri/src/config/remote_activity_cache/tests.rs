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

fn ci(state: PersistedCiState) -> PersistedRepoCi {
    PersistedRepoCi {
        state,
        run_ids: Vec::new(),
        pull_requests: Vec::new(),
        unknown_reason: None,
    }
}

#[test]
fn r1_round_trip_carries_run_ids_pull_requests_and_reason_sorted_deduped() {
    let (_dir, path) = temp_path();
    let entries = vec![
        (
            "a:/repo-a".to_string(),
            PersistedRepoCi {
                state: PersistedCiState::Running,
                run_ids: vec![30, 10, 30],
                pull_requests: vec![7, 7, 2],
                unknown_reason: None,
            },
        ),
        (
            "b:/repo-b".to_string(),
            PersistedRepoCi {
                state: PersistedCiState::Unknown,
                run_ids: Vec::new(),
                pull_requests: Vec::new(),
                unknown_reason: Some("ci-query-timeout".to_string()),
            },
        ),
    ];
    write_snapshot(&path, instant("2026-09-22T11:41:41Z"), &entries).expect("write");
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    assert_eq!(raw["repos"][0]["runIds"], serde_json::json!([10, 30]));
    assert_eq!(raw["repos"][0]["pullRequests"], serde_json::json!([2, 7]));
    assert_eq!(raw["repos"][1]["unknownReason"], "ci-query-timeout");
    let snapshot = read_snapshot(&path).expect("read back");
    assert_eq!(
        snapshot.repos,
        vec![
            (
                "a:/repo-a".to_string(),
                PersistedRepoCi {
                    state: PersistedCiState::Running,
                    run_ids: vec![10, 30],
                    pull_requests: vec![2, 7],
                    unknown_reason: None,
                }
            ),
            entries[1].clone(),
        ]
    );
}

#[test]
fn r2_old_v1_bytes_without_new_keys_read_as_empty_and_none() {
    let (_dir, path) = temp_path();
    std::fs::write(&path, include_bytes!("tests/fixtures/old-v1.json")).expect("write");
    let snapshot = read_snapshot(&path).expect("old bytes read");
    assert_eq!(
        snapshot.repos,
        vec![("a:/repo-a".to_string(), ci(PersistedCiState::Running))]
    );
}

#[test]
fn r3_empty_vectors_and_none_are_not_serialized() {
    let (_dir, path) = temp_path();
    write_snapshot(
        &path,
        instant("2026-09-22T11:41:41Z"),
        &[("a:/repo-a".to_string(), ci(PersistedCiState::Idle))],
    )
    .expect("write");
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    let keys: Vec<&String> = raw["repos"][0]
        .as_object()
        .expect("object")
        .keys()
        .collect();
    assert_eq!(keys, vec!["ciState", "path"]);
}

#[test]
fn write_then_read_round_trip_sorts_raw_paths_and_pins_schema_one() {
    let (_dir, path) = temp_path();
    let generated_at = instant("2026-09-22T11:41:41Z");
    let entries = vec![
        ("z:/repo-b".to_string(), ci(PersistedCiState::Idle)),
        ("a:/repo-a".to_string(), ci(PersistedCiState::Running)),
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
            ("a:/repo-a".to_string(), ci(PersistedCiState::Running)),
            ("z:/repo-b".to_string(), ci(PersistedCiState::Idle)),
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

    std::fs::write(&path, include_bytes!("tests/fixtures/empty-path.json")).expect("write");
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

#[test]
fn directory_apis_preserve_path_codec_bytes_data_and_published_basename() {
    let (_old_dir, old_path) = temp_path();
    let dir = tempfile::tempdir().expect("tempdir");
    let generated_at = instant("2026-09-22T11:41:41Z");
    let entries = vec![
        (
            "z:/repo".to_string(),
            PersistedRepoCi {
                state: PersistedCiState::Running,
                run_ids: vec![30, 10, 30],
                pull_requests: vec![7, 2, 7],
                unknown_reason: None,
            },
        ),
        (
            "a:/repo".to_string(),
            PersistedRepoCi {
                state: PersistedCiState::Unknown,
                run_ids: Vec::new(),
                pull_requests: Vec::new(),
                unknown_reason: Some("ci-query-timeout".to_string()),
            },
        ),
    ];
    write_snapshot(&old_path, generated_at, &entries).expect("path write");
    write_snapshot_to_dir(dir.path(), generated_at, &entries).expect("directory write");
    let path = dir.path().join(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        std::fs::read(&old_path).unwrap()
    );
    let expected = read_snapshot(&old_path).expect("path read");
    assert_eq!(read_snapshot_from_dir(dir.path()).unwrap(), expected);
    assert_eq!(read_snapshot(&path).unwrap(), expected);
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        names,
        vec![std::ffi::OsString::from(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME)]
    );
}

#[test]
fn directory_reader_preserves_missing_and_malformed_path_errors() {
    let (dir, path) = temp_path();
    assert_eq!(read_snapshot_from_dir(dir.path()), read_snapshot(&path));
    assert!(read_snapshot_from_dir(dir.path()).is_err());
    std::fs::write(&path, b"{not json").unwrap();
    assert_eq!(read_snapshot_from_dir(dir.path()), read_snapshot(&path));
    assert!(read_snapshot_from_dir(dir.path()).is_err());
}
