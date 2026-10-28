use super::*;

#[test]
fn reads_the_complete_utf8_document() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.json");
    let document = "{\"categories\":{}}\nñ";
    std::fs::write(&path, document).unwrap();

    assert_eq!(read_capture_catalog(&path).unwrap(), document);
}

#[test]
fn preserves_a_missing_file_error() {
    let directory = tempfile::tempdir().unwrap();
    let error = read_capture_catalog(&directory.path().join("missing.json")).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn preserves_an_invalid_utf8_error() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.json");
    std::fs::write(&path, [0xff]).unwrap();
    let expected = std::fs::read_to_string(&path).unwrap_err();
    let actual = read_capture_catalog(&path).unwrap_err();

    assert_eq!(actual.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(actual.to_string(), expected.to_string());
    assert_eq!(actual.raw_os_error(), expected.raw_os_error());
}

#[test]
fn preserves_a_directory_read_error() {
    let directory = tempfile::tempdir().unwrap();
    let expected = std::fs::read_to_string(directory.path()).unwrap_err();
    let actual = read_capture_catalog(directory.path()).unwrap_err();

    assert_ne!(actual.kind(), std::io::ErrorKind::NotFound);
    assert_eq!(actual.kind(), expected.kind());
    assert_eq!(actual.raw_os_error(), expected.raw_os_error());
    assert_eq!(actual.to_string(), expected.to_string());
}

#[cfg(unix)]
#[test]
fn follows_a_symlink_to_the_document() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("catalog.json");
    let link = directory.path().join("catalog-link.json");
    std::fs::write(&target, "catalog contents").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    assert_eq!(read_capture_catalog(&link).unwrap(), "catalog contents");
}

#[test]
fn transcript_path_preserves_canonical_and_removed_non_ascii_spellings() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(".").join("transcripción-ñ.jsonl");
    std::fs::write(&path, "transcript").unwrap();

    assert_eq!(
        directories::canonical_transcript_path(&path),
        std::fs::canonicalize(&path).unwrap()
    );

    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        directories::canonical_transcript_path(&path).as_os_str(),
        path.as_os_str()
    );
}

#[test]
fn project_children_visit_immediately_and_stop_before_another_callback() {
    use std::ops::ControlFlow;

    let directory = tempfile::tempdir().unwrap();
    for name in ["first", "second", "third"] {
        std::fs::write(directory.path().join(name), "before").unwrap();
    }
    let mut visited = Vec::new();
    let outcome = projects::visit_project_kind_children(directory.path(), |entry| {
        let path = entry.unwrap();
        assert!(visited.is_empty());
        std::fs::write(&path, "during callback").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "during callback");
        visited.push(path);
        ControlFlow::Break(())
    })
    .unwrap();

    assert_eq!(outcome, ControlFlow::Break(()));
    assert_eq!(visited.len(), 1);
    let changed = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .filter(|contents| contents == "during callback")
        .count();
    assert_eq!(changed, 1);
}

#[test]
fn project_children_continue_after_a_child_metadata_error() {
    use std::ops::ControlFlow;

    let directory = tempfile::tempdir().unwrap();
    for name in ["one", "two", "three"] {
        std::fs::write(directory.path().join(name), "child").unwrap();
    }
    let mut failures = Vec::new();
    let outcome = projects::visit_project_kind_children(directory.path(), |entry| {
        let path = entry.unwrap();
        std::fs::remove_file(&path).unwrap();
        failures.push(projects::project_path_metadata(&path).unwrap_err().kind());
        ControlFlow::Continue(())
    })
    .unwrap();

    assert_eq!(outcome, ControlFlow::Continue(()));
    assert_eq!(failures, vec![std::io::ErrorKind::NotFound; 3]);
}

#[test]
fn project_children_preserve_open_errors_without_calling_the_visitor() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("missing");
    let expected = std::fs::read_dir(&path).unwrap_err();
    let actual = projects::visit_project_kind_children(&path, |_| {
        panic!("an open error must not reach the callback")
    })
    .unwrap_err();

    assert_eq!(actual.kind(), expected.kind());
    assert_eq!(actual.raw_os_error(), expected.raw_os_error());
    assert_eq!(actual.to_string(), expected.to_string());
}

#[test]
fn strict_project_children_count_filtered_entries_and_stop_on_the_first_error() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join(".hidden")).unwrap();
    std::fs::write(directory.path().join("file"), "").unwrap();
    let mut scanned = 0;
    projects::visit_team_project_children_strict(directory.path(), &mut scanned, |_, _| {
        panic!("hidden directories and files must not reach the callback")
    })
    .unwrap();
    assert_eq!(scanned, 2);

    scanned = 1_024;
    let error =
        projects::visit_team_project_children_strict(directory.path(), &mut scanned, |_, _| {
            panic!("the entry limit must precede filtering")
        })
        .unwrap_err();
    assert_eq!(error, "invalid_target");
    assert_eq!(scanned, 1_025);

    let missing = directory.path().join("missing");
    let error = projects::visit_team_project_children_strict(&missing, &mut scanned, |_, _| {
        panic!("opening must precede any callback")
    })
    .unwrap_err();
    assert_eq!(error, "unsafe_path");
    assert_eq!(scanned, 1_025);
}

#[test]
fn strict_project_children_propagate_callback_errors_before_the_next_entry() {
    let directory = tempfile::tempdir().unwrap();
    for name in ["one", "two", "three"] {
        std::fs::create_dir(directory.path().join(name)).unwrap();
    }
    let mut scanned = 0;
    let mut callbacks = 0;
    let error = projects::visit_team_project_children_strict(
        directory.path(),
        &mut scanned,
        |name, path| {
            callbacks += 1;
            assert_eq!(path.file_name().unwrap(), name.as_str());
            Err("callback error".to_string())
        },
    )
    .unwrap_err();
    assert_eq!(error, "callback error");
    assert_eq!(scanned, 1);
    assert_eq!(callbacks, 1);
}

#[cfg(unix)]
#[test]
fn strict_project_children_reject_non_utf8_names_before_hidden_filtering() {
    use std::os::unix::ffi::OsStringExt;

    let directory = tempfile::tempdir().unwrap();
    let name = std::ffi::OsString::from_vec(vec![b'.', 0xff]);
    std::fs::create_dir(directory.path().join(name)).unwrap();
    let mut scanned = 0;
    let error =
        projects::visit_team_project_children_strict(directory.path(), &mut scanned, |_, _| {
            panic!("invalid names must not reach the callback")
        })
        .unwrap_err();
    assert_eq!(error, "unsafe_path");
    assert_eq!(scanned, 1);
}

#[test]
fn profile_project_children_skip_open_errors_and_only_visit_directories() {
    let directory = tempfile::tempdir().unwrap();
    let child = directory.path().join("child");
    std::fs::create_dir(&child).unwrap();
    std::fs::write(directory.path().join("file"), "").unwrap();
    let mut visited = Vec::new();
    projects::visit_profile_project_children(&directory.path().join("missing"), |_| {
        panic!("an open error must be skipped")
    });
    projects::visit_profile_project_children(directory.path(), |path| visited.push(path));
    assert_eq!(visited, vec![child]);
}

#[test]
fn source_inventory_open_error_follows_the_read_dir_cut() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    let mut stages = Vec::new();
    let expected = std::fs::read_dir(&missing).unwrap_err();
    let actual = projects::visit_source_inventory_children(
        &missing,
        |stage| {
            stages.push(stage);
            Ok(())
        },
        |_, _| panic!("opening failure must not visit"),
    )
    .unwrap_err();
    assert_eq!(stages, vec!["readDir"]);
    assert_eq!(actual.kind(), expected.kind());
    assert_eq!(actual.raw_os_error(), expected.raw_os_error());
    assert_eq!(actual.to_string(), expected.to_string());
}

#[test]
fn source_inventory_cuts_precede_file_types_and_partial_callbacks() {
    use std::cell::RefCell;

    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("child")).unwrap();
    std::fs::write(directory.path().join("file"), "").unwrap();
    for (failed_stage, ordinal, expected) in [
        ("readDir", 1, vec!["readDir"]),
        ("entry", 1, vec!["readDir", "entry"]),
        ("fileType", 1, vec!["readDir", "entry", "fileType"]),
        (
            "entry",
            2,
            vec!["readDir", "entry", "fileType", "visit", "entry"],
        ),
        (
            "fileType",
            2,
            vec!["readDir", "entry", "fileType", "visit", "entry", "fileType"],
        ),
    ] {
        let events = RefCell::new(Vec::new());
        let mut count = 0;
        let error = projects::visit_source_inventory_children(
            directory.path(),
            |stage| {
                events.borrow_mut().push(stage);
                if stage == failed_stage {
                    count += 1;
                    if count == ordinal {
                        return Err(std::io::Error::from_raw_os_error(123));
                    }
                }
                Ok(())
            },
            |_, _| events.borrow_mut().push("visit"),
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(123));
        assert_eq!(*events.borrow(), expected);
    }

    let events = RefCell::new(Vec::new());
    let mut kinds = Vec::new();
    projects::visit_source_inventory_children(
        directory.path(),
        |stage| {
            events.borrow_mut().push(stage);
            Ok(())
        },
        |path, kind| {
            assert_eq!(events.borrow().last(), Some(&"fileType"));
            events.borrow_mut().push("visit");
            assert!(!kind.is_symlink());
            kinds.push((
                path.file_name().unwrap().to_string_lossy().into_owned(),
                kind.is_dir(),
            ));
        },
    )
    .unwrap();
    kinds.sort();
    assert_eq!(
        kinds,
        vec![("child".to_string(), true), ("file".to_string(), false)]
    );
    assert_eq!(
        *events.borrow(),
        vec!["readDir", "entry", "fileType", "visit", "entry", "fileType", "visit"]
    );
}

#[test]
fn local_template_visitor_stops_on_success_and_each_selected_error() {
    use std::ops::ControlFlow;

    for outcome in ["success", "missing", "non-file", "read-error", "empty"] {
        let root = tempfile::tempdir().unwrap();
        for name in ["one", "two", "three"] {
            std::fs::create_dir(root.path().join(name)).unwrap();
        }
        let mut visits = 0;
        let result = documents::visit_local_template_entries(root.path(), |entry| {
            visits += 1;
            assert_eq!(visits, 1, "advanced after selected {outcome}");
            let (path, _, metadata) = entry.unwrap();
            assert!(metadata.unwrap().is_dir());
            let role = path.join("Role.md");
            match outcome {
                "success" => std::fs::write(&role, "role body").unwrap(),
                "non-file" => std::fs::create_dir(&role).unwrap(),
                "read-error" => std::fs::write(&role, [0xff]).unwrap(),
                "empty" => std::fs::write(&role, "").unwrap(),
                "missing" => {}
                _ => unreachable!(),
            }
            let selected = (|| {
                let meta = documents::local_template_metadata(&role).map_err(|_| "missing")?;
                if !meta.is_file() {
                    return Err("non-file");
                }
                let body = documents::read_local_role_template(&role).map_err(|_| "read-error")?;
                if body.is_empty() {
                    return Err("empty");
                }
                Ok(body)
            })();
            ControlFlow::Break(selected)
        })
        .unwrap();
        assert_eq!(visits, 1);
        let ControlFlow::Break(selected) = result else {
            panic!("selected outcome lost");
        };
        if outcome == "success" {
            assert_eq!(selected.unwrap(), "role body");
        } else {
            assert_eq!(selected.unwrap_err(), outcome);
        }
    }
}

#[test]
fn local_template_visitor_preserves_open_error_and_full_traversal() {
    use std::ops::ControlFlow;

    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");
    let error = documents::visit_local_template_entries::<()>(&missing, |_| {
        panic!("callback after failed open")
    })
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    for name in ["one", "two"] {
        std::fs::write(root.path().join(name), name).unwrap();
    }
    let mut names = Vec::new();
    let result = documents::visit_local_template_entries::<()>(root.path(), |entry| {
        let (path, name, metadata) = entry.unwrap();
        assert_eq!(path.file_name(), Some(name.as_os_str()));
        assert!(metadata.unwrap().is_file());
        names.push(name);
        ControlFlow::Continue(())
    })
    .unwrap();
    assert_eq!(result, ControlFlow::Continue(()));
    assert_eq!(names.len(), 2);
}

#[test]
fn transcript_truncation_preserves_warning_and_state() {
    const TEST: &str = "filesystem::tests::transcript_truncation_preserves_warning_and_state";
    const MARKER: &str = "AC_TRANSCRIPT_TRUNCATION_LOG_CHILD";
    if std::env::var_os(MARKER).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(MARKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("truncation warning captured"));
        return;
    }

    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<(log::Level, String, String)>>>);
    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, record: &log::Record<'_>) {
            self.0.lock().unwrap().push((
                record.level(),
                record.target().to_owned(),
                record.args().to_string(),
            ));
        }
        fn flush(&self) {}
    }
    let records = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    log::set_boxed_logger(Box::new(Capture(records.clone()))).unwrap();
    log::set_max_level(log::LevelFilter::Warn);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("transcript.jsonl");
    std::fs::write(&path, "x\n").unwrap();
    let mut offset = 100;
    let mut remainder = "partial".to_owned();
    let lines =
        transcripts::read_new_lines_with_starts(&path, &mut offset, &mut remainder).unwrap();
    assert!(lines.is_empty());
    assert_eq!(offset, 2);
    assert!(remainder.is_empty());
    assert_eq!(
        *records.lock().unwrap(),
        vec![(
            log::Level::Warn,
            "agentscommander_lib::telegram::jsonl_kernel".to_owned(),
            "[JSONL_TRUNCATE] File shrunk from 100 to 2, skipping past replay".to_owned(),
        )]
    );
    println!("truncation warning captured");
}

#[test]
fn transcript_header_keeps_single_read_bound_and_lossy_first_line() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("rollout-header.jsonl");
    assert_eq!(transcripts::read_transcript_header(&path), None);
    std::fs::write(&path, b"first\xff\r\nsecond\n").unwrap();
    assert_eq!(
        transcripts::read_transcript_header(&path).as_deref(),
        Some("first\u{fffd}\r")
    );
    let mut bytes = vec![b'x'; 70 * 1024];
    bytes.push(b'\n');
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(
        transcripts::read_transcript_header(&path).unwrap().len(),
        64 * 1024
    );
}

#[test]
fn live_transcript_window_returns_raw_bytes_and_caller_bound() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("live.jsonl");
    let missing = transcripts::read_live_preamble_window(&path, 5).unwrap_err();
    assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
    std::fs::write(&path, b"a\n\xc3\xa9\r\nb").unwrap();
    assert_eq!(
        transcripts::read_live_preamble_window(&path, 4).unwrap(),
        (b"\xa9\r\nb".to_vec(), 3)
    );
    assert_eq!(
        transcripts::read_live_preamble_window(&path, 100).unwrap(),
        (b"a\n\xc3\xa9\r\nb".to_vec(), 0)
    );
    assert_eq!(
        transcripts::read_live_preamble_window(&path, 0).unwrap(),
        (Vec::new(), 7)
    );
}

#[test]
fn persisted_session_cwd_probes_preserve_native_results() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file");
    std::fs::write(&file, b"cwd probe").unwrap();
    let missing = root.path().join("missing");
    for path in [root.path(), file.as_path(), missing.as_path()] {
        assert_eq!(
            directories::persisted_session_cwd_exists(path),
            path.exists()
        );
        assert_eq!(directories::restore_cwd_exists(path), path.exists());
        assert_eq!(
            directories::canonical_session_project_path(path),
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
        );
        let expected = std::fs::canonicalize(path);
        match (directories::canonical_carrier_cwd(path), expected) {
            (Ok(actual), Ok(expected)) => assert_eq!(actual, expected),
            (Err(actual), Err(expected)) => {
                assert_eq!(actual.kind(), expected.kind());
                assert_eq!(actual.raw_os_error(), expected.raw_os_error());
                assert_eq!(actual.to_string(), expected.to_string());
            }
            _ => panic!("carrier canonicalization changed the native result"),
        }
    }
    assert_eq!(
        directories::retained_prompt_cwd(root.path()),
        Some(std::fs::canonicalize(root.path()).unwrap())
    );
    assert_eq!(directories::retained_prompt_cwd(&file), None);
    assert_eq!(directories::retained_prompt_cwd(&missing), None);
}

#[tokio::test]
async fn p17_cursor_and_lstat_failures() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing");
    let file = temp.path().join("file");
    std::fs::write(&file, "").unwrap();
    assert!(repositories::open_spec_repo_children(&missing)
        .await
        .is_err());
    assert!(repositories::open_spec_repo_children(&file).await.is_err());
    let empty = temp.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let mut children = repositories::open_spec_repo_children(&empty).await.unwrap();
    assert!(children.next_path().await.is_none());
    let expected = format!(
        "Failed to lstat {}: {}",
        missing.display(),
        std::fs::symlink_metadata(&missing).unwrap_err()
    );
    assert_eq!(
        repositories::spec_repo_link_state(&missing)
            .await
            .unwrap_err(),
        expected
    );
}

#[test]
fn issue_3278_sessions_publish_complete_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snapshot.json");
    let tmp = dir.path().join("snapshot.tmp");
    for document in [r#"{"sessions":[]}"#, r#"{"sessions":[{"name":"ñ"}]}"#] {
        write_session_container_stage(&tmp, document.to_owned()).unwrap();
        assert_eq!(std::fs::read(&tmp).unwrap(), document.as_bytes());
        replace_session_container_file(&tmp, &path).unwrap();
        let actual = std::fs::read(&path).unwrap();
        assert_eq!(actual, document.as_bytes());
        serde_json::from_slice::<serde_json::Value>(&actual).unwrap();
        assert!(!tmp.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
fn issue_3278_sessions_rename_failure_preserves_stage() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().join("snapshot.tmp");
    let dst = dir.path().join("destination");
    std::fs::create_dir(&dst).unwrap();
    let sentinel = dst.join("previous");
    std::fs::write(&sentinel, b"previous").unwrap();
    write_session_container_stage(&tmp, r#"{"sessions":[]}"#.to_owned()).unwrap();
    assert!(replace_session_container_file(&tmp, &dst).is_err());
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"previous");
    assert!(tmp.is_file());
}

#[cfg(unix)]
#[test]
fn issue_3278_parent_directory_sync_success() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().join("snapshot.tmp");
    let dst = dir.path().join("snapshot.json");
    write_session_container_stage(&tmp, r#"{"sessions":[]}"#.to_owned()).unwrap();
    replace_session_container_file(&tmp, &dst).unwrap();
    sync_session_container_parent(&dst).unwrap();
}

#[cfg(unix)]
#[test]
fn issue_3278_parent_directory_sync_missing() {
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join("missing").join("snapshot.json");
    assert!(sync_session_container_parent(&dst).is_err());
}

#[cfg(windows)]
struct Issue3278SyncProbe {
    directory: std::path::PathBuf,
    expected: Vec<u8>,
    observed: Option<std::path::PathBuf>,
}

#[cfg(windows)]
thread_local! {
    static ISSUE_3278_SYNC_PROBE: std::cell::RefCell<Option<Issue3278SyncProbe>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(windows)]
struct Issue3278SyncGuard;

#[cfg(windows)]
impl Issue3278SyncGuard {
    fn arm(directory: &std::path::Path, expected: Vec<u8>) -> Self {
        ISSUE_3278_SYNC_PROBE.with_borrow_mut(|slot| {
            assert!(slot.is_none());
            *slot = Some(Issue3278SyncProbe {
                directory: directory.to_owned(),
                expected,
                observed: None,
            });
        });
        Self
    }

    fn assert_observed(&self) {
        ISSUE_3278_SYNC_PROBE.with_borrow(|slot| {
            assert!(slot.as_ref().unwrap().observed.is_some());
        });
    }
}

#[cfg(windows)]
impl Drop for Issue3278SyncGuard {
    fn drop(&mut self) {
        ISSUE_3278_SYNC_PROBE.with_borrow_mut(|slot| *slot = None);
    }
}

#[cfg(windows)]
pub(super) fn issue_3278_sync_handle(
    path: &std::path::Path,
    file: std::fs::File,
) -> std::io::Result<std::fs::File> {
    ISSUE_3278_SYNC_PROBE.with_borrow_mut(|slot| {
        let Some(probe) = slot.as_mut() else {
            return Ok(file);
        };
        if path.parent() != Some(probe.directory.as_path()) {
            return Ok(file);
        }
        assert!(probe.observed.is_none());
        assert_eq!(path, probe.directory.join("snapshot.tmp"));
        assert_eq!(std::fs::read(path)?, probe.expected);
        drop(file);
        let readonly = std::fs::File::open(path)?;
        probe.observed = Some(path.to_owned());
        Ok(readonly)
    })
}

#[cfg(windows)]
#[test]
fn issue_3278_sync_sessions_failure_keeps_previous() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snapshot.json");
    let previous = br#"{"previous":true}"#;
    std::fs::write(&path, previous).unwrap();
    // Positive control: a successful readonly open must fail at sync, not open/write.
    let readonly = std::fs::File::open(&path).unwrap();
    let control_error = readonly.sync_all().unwrap_err();
    assert_eq!(control_error.raw_os_error(), Some(5));
    drop(readonly);
    let document = r#"{"sessions":[{"name":"ñ"}]}"#.to_owned();
    let expected = document.as_bytes().to_vec();
    let tmp = dir.path().join("snapshot.tmp");
    let guard = Issue3278SyncGuard::arm(dir.path(), expected);
    let result = write_session_container_stage(&tmp, document)
        .and_then(|()| replace_session_container_file(&tmp, &path));
    let error = result.unwrap_err();
    assert_eq!(error.raw_os_error(), control_error.raw_os_error());
    guard.assert_observed();
    // Match the existing session caller: cleanup only its stage on prepare error.
    remove_session_container_stage(&tmp).unwrap();
    assert!(!tmp.exists());
    assert_eq!(std::fs::read(&path).unwrap(), previous);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

struct Issue3298Fault {
    path: std::path::PathBuf,
    calls: usize,
    failures: Vec<(usize, std::io::ErrorKind)>,
}

thread_local! {
    static ISSUE_3298_FAULT: std::cell::RefCell<Option<Issue3298Fault>> =
        const { std::cell::RefCell::new(None) };
}

struct Issue3298Guard(Option<Issue3298Fault>);

impl Issue3298Guard {
    fn arm(path: &std::path::Path, failures: Vec<(usize, std::io::ErrorKind)>) -> Self {
        Self(ISSUE_3298_FAULT.with_borrow_mut(|slot| {
            slot.replace(Issue3298Fault {
                path: path.to_owned(),
                calls: 0,
                failures,
            })
        }))
    }
}

impl Drop for Issue3298Guard {
    fn drop(&mut self) {
        ISSUE_3298_FAULT.with_borrow_mut(|slot| *slot = self.0.take());
    }
}

pub(crate) fn issue_3298_replace_fault(path: &std::path::Path) -> std::io::Result<()> {
    ISSUE_3298_FAULT.with_borrow_mut(|slot| {
        let Some(probe) = slot.as_mut().filter(|probe| probe.path == path) else {
            return Ok(());
        };
        probe.calls += 1;
        match probe.failures.iter().find(|(call, _)| *call == probe.calls) {
            Some((_, kind)) => Err(std::io::Error::new(*kind, "issue3298 injected")),
            None => Ok(()),
        }
    })
}

#[test]
fn issue_3298_reparse_bit_without_symlink() {
    use crate::path_identity::link_flags_indicate_link_or_reparse as flags;
    assert!(flags(false, 0x400));
    assert!(!flags(false, 0));
    assert!(flags(true, 0));
}

#[test]
fn issue_3298_replace_branches_preserve_cleanup_and_retention() {
    use documents::{replace_experiment_artifact, ExperimentArtifactFailure as Failure};
    use std::io::ErrorKind::{AlreadyExists, PermissionDenied};
    // Calls: write=1, flush=2, rename=3, remove=4, second rename=5.
    for stage in 0..=5 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("attempts.jsonl");
        std::fs::write(&path, b"previous").unwrap();
        let failures = match stage {
            0 => vec![(3, AlreadyExists)],
            1..=3 => vec![(stage, PermissionDenied)],
            _ => vec![(3, AlreadyExists), (stage, PermissionDenied)],
        };
        let _guard = Issue3298Guard::arm(&path, failures);
        let result = replace_experiment_artifact(&path, b"next\n");
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        let temps: Vec<_> = entries
            .iter()
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".tmp-")
            })
            .collect();
        if stage == 0 {
            result.unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), b"next\n");
        } else {
            let error = result.unwrap_err();
            match (stage, error) {
                (
                    1,
                    Failure::Write {
                        path: failed,
                        error,
                    },
                )
                | (
                    2,
                    Failure::Flush {
                        path: failed,
                        error,
                    },
                ) => {
                    assert_eq!(&failed, temps[0]);
                    assert_eq!(error.kind(), PermissionDenied);
                }
                (
                    3..=5,
                    Failure::Replace {
                        path: failed,
                        error,
                    },
                ) => {
                    assert_eq!(failed, path);
                    assert_eq!(error.kind(), PermissionDenied);
                }
                (_, other) => panic!("wrong stage: {other:?}"),
            }
            if stage == 5 {
                assert!(!path.exists());
            } else {
                assert_eq!(std::fs::read(&path).unwrap(), b"previous");
            }
        }
        assert_eq!(temps.len(), usize::from(matches!(stage, 1 | 2 | 4)));
        if stage == 1 {
            assert_eq!(std::fs::read(temps[0]).unwrap(), b"");
        } else if matches!(stage, 2 | 4) {
            assert_eq!(std::fs::read(temps[0]).unwrap(), b"next\n");
        }
    }
}

#[test]
fn issue_3298_fault_scope_and_guard_restore() {
    use std::io::ErrorKind::PermissionDenied;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("target");
    let outer = Issue3298Guard::arm(&path, vec![(1, PermissionDenied)]);
    issue_3298_replace_fault(&dir.path().join("other")).unwrap();
    let other_thread = path.clone();
    std::thread::spawn(move || issue_3298_replace_fault(&other_thread).unwrap())
        .join()
        .unwrap();
    let _ = std::panic::catch_unwind(|| {
        let _inner = Issue3298Guard::arm(&path, vec![]);
        panic!("unwind guard");
    });
    assert_eq!(
        issue_3298_replace_fault(&path).unwrap_err().kind(),
        PermissionDenied
    );
    drop(outer);
    issue_3298_replace_fault(&path).unwrap();
}

#[test]
fn issue_3298_visit_is_immediate_and_stops_at_break() {
    use std::ops::ControlFlow;
    let dir = tempfile::tempdir().unwrap();
    for name in ["a", "b", "c"] {
        std::fs::write(dir.path().join(name), b"x").unwrap();
    }
    let mut seen = 0;
    let result = directories::visit_experiment_entries(dir.path(), |entry| {
        let (path, name) = entry.unwrap();
        assert_eq!(path.file_name().unwrap(), name);
        std::fs::remove_file(&path).unwrap();
        seen += 1;
        ControlFlow::Break(path)
    })
    .unwrap();
    assert!(matches!(result, ControlFlow::Break(_)));
    assert_eq!(seen, 1);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    let missing = dir.path().join("missing");
    let expected = std::fs::read_dir(&missing).unwrap_err();
    let actual = directories::visit_experiment_entries(&missing, |_| -> ControlFlow<()> {
        panic!("callback on failed start");
    })
    .unwrap_err();
    assert_eq!(actual.to_string(), expected.to_string());
    assert_eq!(actual.raw_os_error(), expected.raw_os_error());
}

#[test]
fn issue_3329_cli_reads_preserve_text_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("master-token.txt");
    for text in [
        "\u{2003}fixture-token\r\n".to_owned(),
        "\u{feff}fixture-token".to_owned(),
        String::new(),
        "x".repeat(16 * 1024),
    ] {
        std::fs::write(&path, text.as_bytes()).unwrap();
        let native = std::fs::read_to_string(&path).unwrap();
        let actual = documents::read_cli_outbox_text(&path).unwrap();
        assert_eq!(actual, native);
        assert_eq!(
            actual.trim() == "fixture-token",
            native.trim() == "fixture-token"
        );
    }
    std::fs::write(&path, [0xff]).unwrap();
    let classify = |e: std::io::Error| (e.kind(), e.raw_os_error(), e.to_string());
    for path in [path, dir.path().join("missing"), dir.path().to_path_buf()] {
        let native = std::fs::read_to_string(&path).unwrap_err();
        let actual = documents::read_cli_outbox_text(&path).unwrap_err();
        assert_eq!(classify(actual), classify(native));
    }
}

#[test]
fn issue_3329_plain_clear_ports_preserve_native_results() {
    let dir = tempfile::tempdir().unwrap();
    let outbox = dir.path().join("nested/outbox");
    directories::ensure_cli_outbox(&outbox).unwrap();
    assert!(outbox.is_dir());
    directories::ensure_cli_outbox(&outbox).unwrap();
    let request = outbox.join("request.json");
    documents::write_cli_outbox_message(&request, b"longer payload").unwrap();
    documents::write_cli_outbox_message(&request, b"ok").unwrap();
    assert_eq!(std::fs::read(&request).unwrap(), b"ok");
    let classify = |e: std::io::Error| (e.kind(), e.raw_os_error(), e.to_string());
    let native = std::fs::create_dir_all(&request).unwrap_err();
    let actual = directories::ensure_cli_outbox(&request).unwrap_err();
    assert_eq!(classify(actual), classify(native));
    for path in [outbox.clone(), request.join("child")] {
        let native = std::fs::write(&path, b"data").unwrap_err();
        let actual = documents::write_cli_outbox_message(&path, b"data").unwrap_err();
        assert_eq!(classify(actual), classify(native));
    }
    for path in [
        request.clone(),
        outbox,
        dir.path().join("missing"),
        request.join("child"),
    ] {
        let native = std::fs::metadata(&path)
            .map(|m| m.is_file())
            .map_err(classify);
        let actual = documents::cli_outbox_delivery_is_file(&path).map_err(classify);
        assert_eq!(actual, native);
        let native = std::fs::canonicalize(&path).map_err(classify);
        let actual = directories::canonical_cli_discovery_path(&path).map_err(classify);
        assert_eq!(actual, native);
        assert_eq!(directories::cli_discovery_is_dir(&path), path.is_dir());
    }
}
