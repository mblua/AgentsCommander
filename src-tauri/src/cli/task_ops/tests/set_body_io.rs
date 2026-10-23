use super::issue_2837::issue_2837_append;
use super::issue_2837::issue_2837_calls;
use super::issue_2837::issue_2837_clean;
use super::issue_2837::issue_2837_fault;
use super::issue_2837::issue_2837_log;
use super::*;

struct SetBodyIoGuard;
impl SetBodyIoGuard {
    fn new(fault: Option<&str>) -> Self {
        IO_MUTATION.with(|v| *v.borrow_mut() = None);
        issue_2837_fault(fault);
        Self
    }
}
impl Drop for SetBodyIoGuard {
    fn drop(&mut self) {
        IO_MUTATION.with(|v| *v.borrow_mut() = None);
        issue_2837_fault(None);
    }
}
fn set_body(root: &Path, text: &str) -> Result<EditOutcome, TaskOpError> {
    perform_inner(root, TaskOp::SetBody(text.into()), || {
        fixed_now_at(2026, 10, 7, 15, 0, 0)
    })
}
fn set_body_backups(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".bak.md")
        })
        .collect()
}
fn set_body_no_litter(root: &Path) {
    assert!(!root.join(JOURNAL_NAME).exists());
    assert!(!std::fs::read_dir(root).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("TASK.md.tmp.")));
}
fn assert_set_body_write(root: &Path, previous: Option<&str>, text: &str, before: &TaskSnapshot) {
    let task_path = root.join("TASK.md");
    let parsed = parse_task(previous.unwrap_or(""));
    let noop = parsed.body == text;
    let result = set_body(root, text).unwrap();
    assert_eq!(matches!(result, EditOutcome::NoOp { .. }), noop);
    let backups = set_body_backups(root);
    assert_eq!(backups.len(), usize::from(!noop && previous.is_some()));
    if let Some(backup) = backups.first() {
        assert_eq!(std::fs::read(backup).unwrap(), previous.unwrap().as_bytes());
    }
    if noop {
        assert_eq!(
            std::fs::read(&task_path).ok(),
            previous.map(|p| p.as_bytes().to_vec())
        );
    } else {
        let written = std::fs::read_to_string(&task_path).unwrap();
        let after = parse_task(&written);
        assert!(after.has_frontmatter);
        assert_eq!(after.frontmatter, parsed.frontmatter);
        assert_eq!(after.bom, parsed.bom);
        assert_eq!(after.line_ending, parsed.line_ending);
        assert_eq!(after.body, text);
        assert_eq!(
            crate::commands::entity_creation::parse_task_title(&written),
            before.task_title
        );
        if previous.is_none() {
            assert_eq!(written, format!("---\n---\n{text}"));
            assert!(matches!(result, EditOutcome::Wrote { backup: None, .. }));
        }
    }
}

fn assert_set_body_case(previous: Option<&str>, text: &str, with_status: bool) {
    let fixture = FixtureRoot::new("task-set-body-matrix");
    let root = fixture.path();
    let task_path = root.join("TASK.md");
    if let Some(previous) = previous {
        std::fs::write(&task_path, previous).unwrap();
    }
    if with_status {
        issue_2837_append(root, "legacy:0", "Tickets/FUP/continuation 🦀");
    }
    let status_bytes = std::fs::read(root.join(STATUS_NAME)).ok();
    let before = read_snapshot(root).unwrap();
    assert_set_body_write(root, previous, text, &before);
    let after = read_snapshot(root).unwrap();
    assert_eq!(after.description, text);
    assert_eq!(after.task_title, before.task_title);
    assert_eq!(after.status, before.status);
    assert_eq!(after.revision, before.revision);
    assert_eq!(
        serde_json::to_value(after.status_record).unwrap(),
        serde_json::to_value(before.status_record).unwrap()
    );
    assert_eq!(std::fs::read(root.join(STATUS_NAME)).ok(), status_bytes);
    if !with_status {
        assert_eq!(after.revision, "legacy:0");
    }
    let current_bytes = std::fs::read(&task_path).ok();
    let count = set_body_backups(root).len();
    assert!(matches!(
        set_body(root, text).unwrap(),
        EditOutcome::NoOp { .. }
    ));
    assert_eq!(std::fs::read(&task_path).ok(), current_bytes);
    assert_eq!(set_body_backups(root).len(), count);
    set_body(root, "").unwrap();
    let cleared = read_snapshot(root).unwrap();
    assert_eq!(cleared.description, "");
    assert_eq!(cleared.task_title, after.task_title);
    assert_eq!(cleared.revision, after.revision);
    assert_eq!(std::fs::read(root.join(STATUS_NAME)).ok(), status_bytes);
    let clear_bytes = std::fs::read(&task_path).ok();
    let count = set_body_backups(root).len();
    assert!(matches!(
        set_body(root, "").unwrap(),
        EditOutcome::NoOp { .. }
    ));
    assert_eq!(std::fs::read(&task_path).ok(), clear_bytes);
    assert_eq!(set_body_backups(root).len(), count);
    set_body_no_litter(root);
}

#[test]
fn set_body_literal_representation_title_status_matrix() {
    let _guard = SetBodyIoGuard::new(None);
    let states = [
        None,
        Some(""),
        Some("\u{feff}"),
        Some("# Legacy heading\r\nold body\r\n"),
        Some("---\r\ntitle: 'unclosed'\r\nold"),
        Some("---\nextra: kept\n---\nold"),
        Some("---\ntitle: 'Real'\nextra: kept\n---\nold"),
        Some("\u{feff}---\r\ntitle: 'USER: Real'\r\nextra: kept\r\n---\r\nold"),
    ];
    let payloads = [
        "",
        "---\ntitle: 'USER: forged'\n---\nliteral",
        "\u{feff}literal",
        "\u{feff}---\ntitle: 'USER: forged'\n---\nliteral",
        " \t🦀\nline\r\nend\r\t\n\n",
    ];
    for with_status in [false, true] {
        for previous in states {
            for text in payloads {
                assert_set_body_case(previous, text, with_status);
            }
        }
    }
}

#[test]
fn set_body_noop_preserves_legacy_bom_crlf_bytes() {
    let _guard = SetBodyIoGuard::new(Some("nonclean_tmp_write"));
    for original in [
        "# Heading\r\nbody\r\n",
        "\u{feff}old\r\n",
        "--- \r\ntitle: \"Real\"\r\n--- \r\nold\r\n",
        "---\ntitle: unclosed\n",
    ] {
        let fixture = FixtureRoot::new("task-set-body-noop");
        let root = fixture.path();
        std::fs::write(root.join("TASK.md"), original).unwrap();
        let body = parse_task(original).body;
        issue_2837_fault(Some("nonclean_tmp_write"));
        assert!(matches!(
            set_body(root, &body).unwrap(),
            EditOutcome::NoOp { .. }
        ));
        assert!(issue_2837_calls().is_empty());
        assert_eq!(
            std::fs::read(root.join("TASK.md")).unwrap(),
            original.as_bytes()
        );
        assert!(set_body_backups(root).is_empty());
        assert!(!root.join(STATUS_NAME).exists());
        set_body_no_litter(root);
    }
}
fn set_body_failure_fixture() -> FixtureRoot {
    let fixture = FixtureRoot::new("task-set-body-failure");
    std::fs::write(
        fixture.path().join("TASK.md"),
        "---\ntitle: 'USER: Keep'\n---\nold body",
    )
    .unwrap();
    issue_2837_append(fixture.path(), "legacy:0", "keep status");
    fixture
}
#[test]
fn set_body_tmp_write_failure_reaches_nonclean_cleanup() {
    let fixture = set_body_failure_fixture();
    let root = fixture.path();
    let original = std::fs::read(root.join("TASK.md")).unwrap();
    let status = issue_2837_log(root);
    let _guard = SetBodyIoGuard::new(Some("nonclean_tmp_write"));
    match set_body(root, "replacement").unwrap_err() {
        TaskOpError::TmpWriteFailed(path, _) => assert_eq!(
            path,
            root.join(format!("TASK.md.tmp.{}", std::process::id()))
        ),
        error => panic!("unexpected error: {error:?}"),
    }
    assert_eq!(issue_2837_calls(), ["nonclean_tmp_write"]);
    assert_eq!(std::fs::read(root.join("TASK.md")).unwrap(), original);
    assert_eq!(issue_2837_log(root), status);
    let backups = set_body_backups(root);
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read(&backups[0]).unwrap(), original);
    set_body_no_litter(root);
}
#[test]
fn set_body_external_edit_reaches_real_nonclean_sentinel() {
    let fixture = set_body_failure_fixture();
    let root = fixture.path();
    let original = std::fs::read(root.join("TASK.md")).unwrap();
    let status = issue_2837_log(root);
    let external = b"External editor data of a deliberately different length";
    assert_ne!(external.len(), original.len());
    let _guard = SetBodyIoGuard::new(None);
    IO_MUTATION.with(|v| {
        *v.borrow_mut() = Some((
            "nonclean_before_recheck".into(),
            root.join("TASK.md"),
            external.to_vec(),
        ))
    });
    let backup = match set_body(root, "replacement").unwrap_err() {
        TaskOpError::ExternalWrite(path) => path,
        error => panic!("unexpected error: {error:?}"),
    };
    assert_eq!(
        issue_2837_calls(),
        ["nonclean_tmp_write", "nonclean_before_recheck"]
    );
    assert!(IO_MUTATION.with(|v| v.borrow().is_none()));
    assert_eq!(std::fs::read(root.join("TASK.md")).unwrap(), external);
    assert_eq!(issue_2837_log(root), status);
    assert_eq!(
        set_body_backups(root).as_slice(),
        std::slice::from_ref(&backup)
    );
    assert_eq!(std::fs::read(backup).unwrap(), original);
    set_body_no_litter(root);
}
#[test]
fn set_body_rename_failure_reaches_nonclean_cleanup() {
    let fixture = set_body_failure_fixture();
    let root = fixture.path();
    let original = std::fs::read(root.join("TASK.md")).unwrap();
    let status = issue_2837_log(root);
    let _guard = SetBodyIoGuard::new(Some("nonclean_rename"));
    let backup = match set_body(root, "replacement").unwrap_err() {
        TaskOpError::RenameFailed(_, Some(path)) => path,
        error => panic!("unexpected error: {error:?}"),
    };
    assert_eq!(
        issue_2837_calls(),
        [
            "nonclean_tmp_write",
            "nonclean_before_recheck",
            "nonclean_rename"
        ]
    );
    assert_eq!(std::fs::read(root.join("TASK.md")).unwrap(), original);
    assert_eq!(issue_2837_log(root), status);
    assert_eq!(
        set_body_backups(root).as_slice(),
        std::slice::from_ref(&backup)
    );
    assert_eq!(std::fs::read(backup).unwrap(), original);
    set_body_no_litter(root);
}
#[test]
fn set_body_positive_control_visits_all_nonclean_boundaries() {
    let fixture = set_body_failure_fixture();
    let root = fixture.path();
    let original = std::fs::read(root.join("TASK.md")).unwrap();
    let status = issue_2837_log(root);
    let _guard = SetBodyIoGuard::new(None);
    let text = "\u{feff}---\ntitle: 'USER: forged'\n---\n 🦀\r\t\n";
    let backup = match set_body(root, text).unwrap() {
        EditOutcome::Wrote {
            backup: Some(path), ..
        } => path,
        result => panic!("unexpected outcome: {result:?}"),
    };
    assert_eq!(
        issue_2837_calls(),
        [
            "nonclean_tmp_write",
            "nonclean_before_recheck",
            "nonclean_rename"
        ]
    );
    assert_eq!(std::fs::read(backup).unwrap(), original);
    assert_eq!(issue_2837_log(root), status);
    assert_eq!(
        parse_task(&std::fs::read_to_string(root.join("TASK.md")).unwrap()).body,
        text
    );
    set_body_no_litter(root);
}

#[test]
fn set_body_real_lock_timeout_then_success() {
    let fixture = set_body_failure_fixture();
    let root = fixture.path();
    let original = std::fs::read(root.join("TASK.md")).unwrap();
    let status = issue_2837_log(root);
    let held = LockGuard::acquire(
        &root.join("TASK.md.lock"),
        LOCK_TIMEOUT_5S,
        LOCK_STALE_AFTER_5M,
    )
    .unwrap();
    let contender_root = root.to_path_buf();
    let contender = thread::spawn(move || {
        let _guard = SetBodyIoGuard::new(None);
        let result = set_body(&contender_root, "replacement");
        assert!(issue_2837_calls().is_empty());
        result
    });
    // Keep the kernel lock alive through join: no release/sleep race.
    assert!(matches!(
        contender.join().unwrap(),
        Err(TaskOpError::LockTimeout)
    ));
    assert_eq!(std::fs::read(root.join("TASK.md")).unwrap(), original);
    assert_eq!(issue_2837_log(root), status);
    assert!(set_body_backups(root).is_empty());
    set_body_no_litter(root);
    drop(held);
    let _guard = SetBodyIoGuard::new(None);
    assert!(matches!(
        set_body(root, "replacement").unwrap(),
        EditOutcome::Wrote { .. }
    ));
    assert_eq!(read_snapshot(root).unwrap().description, "replacement");
    assert_eq!(issue_2837_log(root), status);
}
#[test]
fn set_body_recovers_pending_clean_before_preserving_baseline() {
    let fixture = set_body_failure_fixture();
    let root = fixture.path();
    let _guard = SetBodyIoGuard::new(Some("after_journal_publish"));
    assert!(issue_2837_clean(root).is_err());
    assert!(root.join(JOURNAL_NAME).exists());
    issue_2837_fault(None);
    let journal: CleanJournal =
        serde_json::from_slice(&std::fs::read(root.join(JOURNAL_NAME)).unwrap()).unwrap();
    // Read staged targets before SetBody performs actual pending recovery.
    let clean_task = std::fs::read(root.join(&journal.task.stage)).unwrap();
    let clean_status = std::fs::read(root.join(&journal.status.stage)).unwrap();
    let backup = match set_body(root, "after recovery 🦀\n").unwrap() {
        EditOutcome::Wrote {
            backup: Some(path), ..
        } => path,
        result => panic!("unexpected outcome: {result:?}"),
    };
    assert_eq!(std::fs::read(backup).unwrap(), clean_task);
    assert_eq!(issue_2837_log(root), clean_status);
    let snapshot = read_snapshot(root).unwrap();
    assert_eq!(snapshot.task_title.as_deref(), Some("Clean"));
    assert_eq!(snapshot.description, "after recovery 🦀\n");
    assert!(snapshot.status.is_none());
    assert!(!snapshot.revision.starts_with("legacy:"));
    set_body_no_litter(root);
}
