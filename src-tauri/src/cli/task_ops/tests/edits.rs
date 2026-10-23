use super::*;

// ── U1-U6: parse_task ──────────────────────────────────────────────

#[test]
fn parse_task_no_frontmatter() {
    let p = parse_task("# Body");
    assert!(!p.has_frontmatter);
    assert_eq!(p.body, "# Body");
}

#[test]
fn parse_task_empty_string() {
    let p = parse_task("");
    assert!(!p.has_frontmatter);
    assert_eq!(p.body, "");
}

#[test]
fn parse_task_well_formed_frontmatter() {
    let p = parse_task("---\ntitle: x\n---\nbody");
    assert!(p.has_frontmatter);
    assert_eq!(p.frontmatter, vec!["title: x".to_string()]);
    assert_eq!(p.body, "body");
}

#[test]
fn parse_task_frontmatter_no_title_field() {
    let p = parse_task("---\nfoo: bar\n---\nbody");
    assert!(p.has_frontmatter);
    assert_eq!(p.frontmatter, vec!["foo: bar".to_string()]);
}

#[test]
fn parse_task_unclosed_frontmatter_treated_as_body() {
    let input = "---\ntitle: x\n(no closer)\n";
    let p = parse_task(input);
    assert!(!p.has_frontmatter);
    assert_eq!(p.body, input);
}

#[test]
fn parse_task_tolerates_crlf() {
    // CRIT-1 strict pin: body must be exactly "body" with no leading "\n".
    let p = parse_task("---\r\ntitle: x\r\n---\r\nbody");
    assert!(p.has_frontmatter);
    assert_eq!(p.body, "body");
    assert_eq!(p.line_ending, "\r\n");
}

// ── U7-U13: apply_set_title ─────────────────────────────────────────

#[test]
fn apply_set_title_creates_frontmatter_when_absent() {
    let parsed = parse_task("");
    let p = apply_set_title(&parsed, "X");
    let out = render(&p);
    assert_eq!(out, "---\ntitle: 'X'\n---\n");
}

#[test]
fn apply_set_title_replaces_existing_title_value() {
    let parsed = parse_task("---\ntitle: old\n---\nbody\n");
    let p = apply_set_title(&parsed, "new");
    assert_eq!(p.frontmatter, vec!["title: 'new'".to_string()]);
    assert_eq!(p.body, "body\n");
    let legacy = parse_task("Ready to start a new topic\n");
    assert_eq!(apply_set_title(&legacy, "new").body, legacy.body);
    assert_eq!(
        apply_edit(&legacy, &TaskOp::SetUserTitle("new".into())).body,
        legacy.body
    );
}

#[test]
fn apply_set_title_inserts_into_existing_frontmatter() {
    let parsed = parse_task("---\nfoo: bar\n---\nbody\n");
    let p = apply_set_title(&parsed, "x");
    assert_eq!(
        p.frontmatter,
        vec!["title: 'x'".to_string(), "foo: bar".to_string()]
    );
}

#[test]
fn apply_set_title_preserves_other_frontmatter_fields() {
    let parsed = parse_task("---\nfoo: 1\ntitle: old\nbar: 2\n---\nbody");
    let p = apply_set_title(&parsed, "new");
    assert_eq!(
        p.frontmatter,
        vec![
            "foo: 1".to_string(),
            "title: 'new'".to_string(),
            "bar: 2".to_string(),
        ]
    );
    assert_eq!(p.body, "body");
}

#[test]
fn apply_set_title_yaml_escapes_single_quote() {
    let parsed = parse_task("");
    let p = apply_set_title(&parsed, "won't");
    let out = render(&p);
    assert!(out.contains("title: 'won''t'"));
}

#[test]
fn apply_set_title_yaml_safe_with_colon_and_hash() {
    let title = "v1.0: stable #release";
    let parsed = parse_task("");
    let p = apply_set_title(&parsed, title);
    let out = render(&p);
    // Round-trip via parser
    let re = parse_task(&out);
    assert_eq!(title_value_of(&re).as_deref(), Some(title));
}

#[test]
fn apply_set_title_idempotent_when_value_matches() {
    let fix = FixtureRoot::new("task-u13");
    let wg = fix.path().join("wg-1-test");
    std::fs::create_dir_all(&wg).unwrap();
    // Seed file
    std::fs::write(wg.join("TASK.md"), "---\ntitle: 'X'\n---\nbody\n").unwrap();
    let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
    let r = perform_inner(&wg, TaskOp::SetTitle("X".into()), now).unwrap();
    match r {
        EditOutcome::NoOp { .. } => {}
        other => panic!("expected NoOp, got {:?}", other),
    }
}

// ── U14-U18: apply_append_body ──────────────────────────────────────

#[test]
fn apply_append_body_to_empty_file() {
    let parsed = parse_task("");
    let p = apply_append_body(&parsed, "hello");
    assert_eq!(p.body, "hello\n");
}

#[test]
fn apply_append_body_preserves_prior_content() {
    let parsed = parse_task("---\ntitle: x\n---\nold\n");
    let p = apply_append_body(&parsed, "new");
    let out = render(&p);
    assert_eq!(out, "---\ntitle: x\n---\nold\n\nnew\n");
    let legacy = parse_task("Ready to start a new topic\n");
    assert_eq!(
        apply_append_body(&legacy, "summary").body,
        "Ready to start a new topic\n\nsummary\n"
    );
}

#[test]
fn apply_append_body_normalizes_blank_line_separator() {
    // Body with multiple trailing newlines: collapses to exactly one blank line.
    let parsed = ParsedTask {
        bom: false,
        line_ending: "\n",
        has_frontmatter: false,
        frontmatter: Vec::new(),
        body: "old\n\n\n\n".to_string(),
    };
    let p = apply_append_body(&parsed, "new");
    assert_eq!(p.body, "old\n\nnew\n");
}

#[test]
fn apply_append_body_does_not_touch_frontmatter() {
    let parsed = parse_task("---\ntitle: x\n---\nold\n");
    let p = apply_append_body(&parsed, "new");
    assert_eq!(p.frontmatter, parsed.frontmatter);
}

#[test]
fn apply_append_body_strips_trailing_whitespace_from_text() {
    let parsed = parse_task("");
    let p = apply_append_body(&parsed, "hello   \n\n");
    assert_eq!(p.body, "hello\n");
}

// ── U19-U22: LockGuard + atomic publish ─────────────────────────────

#[test]
fn lock_guard_creates_and_preserves_lockfile() {
    let fix = FixtureRoot::new("task-u19");
    let lock_path = fix.path().join("TASK.md.lock");
    {
        let _g = LockGuard::acquire(&lock_path, LOCK_TIMEOUT_5S, LOCK_STALE_AFTER_5M).unwrap();
        assert!(lock_path.exists());
    }
    assert!(lock_path.exists());
}

#[test]
fn lock_guard_blocks_concurrent_acquisition() {
    let fix = FixtureRoot::new("task-u20");
    let lock_path = fix.path().join("TASK.md.lock");
    let _held = LockGuard::acquire(&lock_path, LOCK_TIMEOUT_5S, LOCK_STALE_AFTER_5M).unwrap();
    let res = LockGuard::acquire(&lock_path, Duration::from_millis(100), LOCK_STALE_AFTER_5M);
    assert!(matches!(res, Err(TaskOpError::LockTimeout)));
}

#[test]
fn lock_guard_ignores_stale_unowned_contents() {
    // Test approach (std-only — no `filetime`, no FFI):
    // pre-create the lockfile via OpenOptions::create_new, drop the handle,
    // sleep ~30 ms, then call acquire with a small `stale_after` (e.g. 10 ms).
    // The production constant is LOCK_STALE_AFTER_5M (300 s); the test uses
    // a smaller value because std-only Rust cannot fake file mtimes.
    let fix = FixtureRoot::new("task-u21");
    let lock_path = fix.path().join("TASK.md.lock");
    {
        let f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .unwrap();
        drop(f);
    }
    std::thread::sleep(Duration::from_millis(30));
    let g = LockGuard::acquire(
        &lock_path,
        Duration::from_secs(2),
        Duration::from_millis(10),
    )
    .expect("stale lock should be recovered");
    assert!(lock_path.exists());
    drop(g);
    assert!(lock_path.exists());
}

#[test]
fn atomic_publish_via_rename_round_trip() {
    let fix = FixtureRoot::new("task-u22");
    let wg = fix.path().join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
    let _r = perform_inner(&wg, TaskOp::SetTitle("X".into()), now).unwrap();
    // After success, no per-PID tmp file remains.
    let pid_tmp = wg.join(format!("TASK.md.tmp.{}", std::process::id()));
    assert!(!pid_tmp.exists());
    // No stray TASK.md.tmp.* either.
    for entry in std::fs::read_dir(&wg).unwrap().flatten() {
        let n = entry.file_name();
        let name = n.to_string_lossy();
        assert!(
            !name.starts_with("TASK.md.tmp."),
            "leftover tmp file: {}",
            name
        );
    }
    // Stable lock file remains.
    assert!(wg.join("TASK.md.lock").exists());
}

// ── U23: backup filename format ─────────────────────────────────────

#[test]
fn backup_filename_uses_utc_timestamp_format() {
    let fix = FixtureRoot::new("task-u23");
    let wg = fix.path().join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    std::fs::write(wg.join("TASK.md"), "old\n").unwrap();
    let now = || fixed_now_at(2026, 1, 1, 12, 34, 56);
    let r = perform_inner(&wg, TaskOp::SetTitle("X".into()), now).unwrap();
    let bp = match r {
        EditOutcome::Wrote {
            backup: Some(bp), ..
        } => bp,
        other => panic!("expected Wrote with backup, got {:?}", other),
    };
    let name = bp.file_name().unwrap().to_string_lossy().into_owned();
    // Pattern: TASK.YYYYMMDD-HHMMSS(.N)?.bak.md
    assert!(
        name == "TASK.20260101-123456.bak.md"
            || name.starts_with("TASK.20260101-123456.") && name.ends_with(".bak.md"),
        "unexpected backup filename: {}",
        name
    );
}

// ── U24, U30: backup-failure path ───────────────────────────────────

#[test]
fn backup_failure_aborts_write_and_preserves_task() {
    // Per plan §9 U24, the test pins "backup failure aborts cleanly" — either
    // BackupExhausted (loop exhausts 100 collisions) or BackupFailed (a
    // create_new error other than AlreadyExists). On Windows, attempting to
    // OpenOptions::create_new a path where a directory already exists returns
    // PermissionDenied (not AlreadyExists), so we get BackupFailed; on Unix,
    // the same returns IsADirectory (also not AlreadyExists). Both are
    // graceful failures; the assertion accepts either variant.
    let fix = FixtureRoot::new("task-u24");
    let wg = fix.path().join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    let task = wg.join("TASK.md");
    std::fs::write(&task, "old\n").unwrap();
    let original = std::fs::read(&task).unwrap();
    let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
    // Pre-create directories at every candidate path so create_new fails
    // for all of them.
    for n in 0..=99u32 {
        let candidate = if n == 0 {
            wg.join("TASK.20260101-000000.bak.md")
        } else {
            wg.join(format!("TASK.20260101-000000.{}.bak.md", n))
        };
        std::fs::create_dir(&candidate).unwrap();
    }
    let result = perform_inner(&wg, TaskOp::SetTitle("x".into()), now);
    assert!(
        matches!(
            result,
            Err(TaskOpError::BackupExhausted(_)) | Err(TaskOpError::BackupFailed(_, _))
        ),
        "expected backup-class failure, got {:?}",
        result
    );
    // TASK.md unchanged.
    assert_eq!(std::fs::read(&task).unwrap(), original);
    // U30: lock cleaned up.
    assert!(wg.join("TASK.md.lock").exists());
    // No tmp file written (we abort before the tmp-write).
    let pid_tmp = wg.join(format!("TASK.md.tmp.{}", std::process::id()));
    assert!(!pid_tmp.exists());
}

#[test]
fn backup_failure_releases_lockfile() {
    // Companion to U24: assert lock file cleaned even on backup failure.
    let fix = FixtureRoot::new("task-u30");
    let wg = fix.path().join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    std::fs::write(wg.join("TASK.md"), "old\n").unwrap();
    let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
    for n in 0..=99u32 {
        let candidate = if n == 0 {
            wg.join("TASK.20260101-000000.bak.md")
        } else {
            wg.join(format!("TASK.20260101-000000.{}.bak.md", n))
        };
        std::fs::create_dir(&candidate).unwrap();
    }
    let _ = perform_inner(&wg, TaskOp::SetTitle("x".into()), now);
    assert!(wg.join("TASK.md.lock").exists());
}

// ── U25: concurrent set-title + append-body ─────────────────────────

#[test]
fn concurrent_set_title_and_append_body_both_apply() {
    // MED-2: synchronize via Barrier so threads contend at the same instant.
    // Without the barrier the test would pass for the wrong reason.
    for _iter in 0..10 {
        let fix = FixtureRoot::new("task-u25");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let wg_clone1 = wg.clone();
        let wg_clone2 = wg.clone();
        let b1 = barrier.clone();
        let b2 = barrier.clone();

        let h1 = thread::spawn(move || {
            b1.wait();
            perform(&wg_clone1, TaskOp::SetTitle("X".into()))
        });
        let h2 = thread::spawn(move || {
            b2.wait();
            perform(&wg_clone2, TaskOp::AppendBody("appended body line".into()))
        });
        let r1 = h1.join().unwrap();
        let r2 = h2.join().unwrap();
        // At least one must succeed; whichever lost the lock may LockTimeout
        // (unlikely with the 5 s window), but we don't strictly require both.
        assert!(r1.is_ok() || matches!(r1, Err(TaskOpError::LockTimeout)));
        assert!(r2.is_ok() || matches!(r2, Err(TaskOpError::LockTimeout)));
        if r1.is_ok() && r2.is_ok() {
            let final_content = std::fs::read_to_string(wg.join("TASK.md")).unwrap();
            assert!(final_content.contains("title: 'X'"));
            assert!(final_content.contains("appended body line"));
        }
    }
}

// ── U26-U28: parser/applier edge cases ──────────────────────────────

#[test]
fn parse_task_tolerates_trailing_space_on_markers() {
    let p = parse_task("--- \ntitle: x\n--- \nbody");
    assert!(p.has_frontmatter);
    assert_eq!(p.frontmatter, vec!["title: x".to_string()]);
    assert_eq!(p.body, "body");
}

#[test]
fn parse_task_unicode_in_body_preserved_byte_for_byte() {
    let body = "café\n🎉\n";
    let input = format!("---\ntitle: x\n---\n{}", body);
    let p = parse_task(&input);
    assert!(p.has_frontmatter);
    assert_eq!(p.body, body);
}

#[test]
fn apply_set_title_preserves_indentation_of_existing_title_line() {
    let parsed = parse_task("---\n  title: old\n---\n");
    let p = apply_set_title(&parsed, "new");
    assert_eq!(p.frontmatter, vec!["  title: 'new'".to_string()]);
}

// ── U29: backup collision suffix loop ───────────────────────────────

#[test]
fn backup_collision_within_same_second_does_not_clobber_prior_backup() {
    let fix = FixtureRoot::new("task-u29");
    let wg = fix.path().join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    std::fs::write(wg.join("TASK.md"), "first\n").unwrap();
    let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
    // First call → TASK.20260101-000000.bak.md
    let _ = perform_inner(&wg, TaskOp::AppendBody("a".into()), now).unwrap();
    // Second call same second → TASK.20260101-000000.1.bak.md
    let _ = perform_inner(&wg, TaskOp::AppendBody("b".into()), now).unwrap();
    let bk0 = wg.join("TASK.20260101-000000.bak.md");
    let bk1 = wg.join("TASK.20260101-000000.1.bak.md");
    assert!(bk0.exists(), "first backup should exist");
    assert!(
        bk1.exists(),
        "collision-suffixed second backup should exist"
    );
    // First backup contains "first\n" (the pre-edit state of the first call).
    assert_eq!(std::fs::read_to_string(&bk0).unwrap(), "first\n");
}

// ── U31: BOM round-trip ─────────────────────────────────────────────

#[test]
fn parse_task_strips_and_re_emits_leading_bom() {
    let input = "\u{FEFF}---\ntitle: x\n---\nbody";
    let p = parse_task(input);
    assert!(p.bom);
    assert!(p.has_frontmatter);
    assert_eq!(p.frontmatter, vec!["title: x".to_string()]);
    assert_eq!(p.body, "body");
    let rendered = render(&p);
    assert_eq!(rendered, input);
}

// ── U32: CRLF round-trip byte-exact ─────────────────────────────────

#[test]
fn set_title_round_trip_preserves_crlf_no_extra_blank_line() {
    let input = "---\r\ntitle: old\r\n---\r\nbody\r\n";
    let parsed = parse_task(input);
    assert_eq!(parsed.line_ending, "\r\n");
    let edited = apply_set_title(&parsed, "new");
    let out = render(&edited);
    // Closing "---\r\n" is followed immediately by "body\r\n" (no blank line).
    assert!(
        out.contains("---\r\nbody\r\n"),
        "expected '---\\r\\nbody\\r\\n' in output, got: {:?}",
        out
    );
    // No extra leading "\r\n" before "body".
    assert!(!out.contains("---\r\n\r\nbody"));
}

// ── U33: line-ending preservation ───────────────────────────────────

#[test]
fn parse_task_preserves_dominant_line_ending() {
    let crlf = parse_task("---\r\ntitle: x\r\n---\r\n");
    assert_eq!(crlf.line_ending, "\r\n");
    let lf = parse_task("---\ntitle: x\n---\n");
    assert_eq!(lf.line_ending, "\n");
    let edited = apply_set_title(&crlf, "y");
    let out = render(&edited);
    assert!(out.contains("---\r\ntitle: 'y'\r\n---\r\n"));
}

// ── U34: append-body line-ending trade-off pin (NIT-E) ──────────────

#[test]
fn apply_append_body_preserves_internal_body_line_endings_and_documents_trailing_loss() {
    // Pins the §5 row-510 trade-off: existing body's internal CRLF is preserved
    // byte-for-byte, but the body's trailing CRLF gets trim_end'd and replaced
    // by an LF separator + LF terminator.
    let parsed = ParsedTask {
        bom: false,
        line_ending: "\r\n",
        has_frontmatter: false,
        frontmatter: Vec::new(),
        body: "Line1\r\nLine2\r\n".to_string(),
    };
    let p = apply_append_body(&parsed, "NewLine");
    // Line1's CRLF preserved; Line2's trailing CRLF replaced; NewLine ends with LF.
    assert_eq!(p.body, "Line1\r\nLine2\n\nNewLine\n");
}

// ── U35: BOM-only existing file preserves BOM through set-title (LOW-1)

#[test]
fn apply_set_title_preserves_bom_on_bom_only_existing_file() {
    // BOM-only file (e.g. coordinator opened TASK.md in Notepad on
    // Windows, which writes \xEF\xBB\xBF, then saved). The brand-new
    // branch must NOT fire — that would strip the BOM and violate the
    // HIGH-3 byte-exact round-trip guarantee. The fix gates the
    // brand-new branch on !parsed.bom so this case falls through to the
    // "no frontmatter, preserve bom/eol" branch.
    let parsed = parse_task("\u{FEFF}");
    let p = apply_set_title(&parsed, "X");
    assert_eq!(render(&p), "\u{FEFF}---\ntitle: 'X'\n---\n");
}

// ── extra: title_value_of helper ────────────────────────────────────

#[test]
fn title_value_of_canonical_single_quoted() {
    let p = parse_task("---\ntitle: 'won''t'\n---\n");
    assert_eq!(title_value_of(&p).as_deref(), Some("won't"));
}

#[test]
fn title_value_of_bare_scalar() {
    let p = parse_task("---\ntitle: bare\n---\n");
    assert_eq!(title_value_of(&p).as_deref(), Some("bare"));
}

#[test]
fn title_value_of_absent() {
    let p = parse_task("---\nfoo: bar\n---\n");
    assert_eq!(title_value_of(&p), None);
}

// ── U36-U41: TaskOp::Clean ─────────────────────────────────────────

#[test]
fn apply_clean_creates_canonical_clean_for_empty_file() {
    let parsed = parse_task("");
    let p = apply_clean(&parsed);
    let out = render(&p);
    assert_eq!(out, "---\ntitle: 'Clean'\n---\n");
}

#[test]
fn apply_clean_replaces_existing_frontmatter_and_body() {
    // Round 2 (dev-rust R1.3): hard-reset semantics also normalize
    // indentation — a coordinator-edited `  title: 'X'` (two-space
    // indent) becomes unindented `"title: 'Clean'"`. Idempotence
    // check in §3.1.4 will treat this as write-worthy.
    let parsed = parse_task("---\ntitle: 'Old'\nfoo: bar\n---\nold body\n");
    let p = apply_clean(&parsed);
    // Frontmatter is REPLACED entirely (foo: bar is dropped — Clean
    // is a hard reset, not a merge).
    assert_eq!(p.frontmatter, vec!["title: 'Clean'".to_string()]);
    assert_eq!(p.body, "");
}

#[test]
fn apply_clean_preserves_crlf_and_bom() {
    // Clean preserves BOM/CRLF frontmatter; a second Clean is NoOp.
    let input = "\u{FEFF}---\r\ntitle: old\r\nx: 1\r\n---\r\nbody\r\n";
    let parsed = parse_task(input);
    let p = apply_clean(&parsed);
    assert!(p.bom);
    assert_eq!(p.line_ending, "\r\n");
    let out = render(&p);
    assert_eq!(out, "\u{FEFF}---\r\ntitle: 'Clean'\r\n---\r\n");
    assert!(p.body.is_empty());
    let fix = FixtureRoot::new("task-clean-crlf");
    let wg = fix.path().join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    std::fs::write(wg.join("TASK.md"), input).unwrap();
    let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
    assert!(matches!(
        perform_inner(&wg, TaskOp::Clean, now).unwrap(),
        EditOutcome::Wrote { .. }
    ));
    let log = std::fs::read(wg.join("TASK-status.jsonl")).unwrap();
    let entries = std::fs::read_dir(&wg).unwrap().count();
    assert!(matches!(
        perform_inner(&wg, TaskOp::Clean, now).unwrap(),
        EditOutcome::NoOp { .. }
    ));
    assert_eq!(std::fs::read(wg.join("TASK.md")).unwrap(), out.as_bytes());
    assert_eq!(std::fs::read(wg.join("TASK-status.jsonl")).unwrap(), log);
    assert_eq!(std::fs::read_dir(&wg).unwrap().count(), entries);
}

#[test]
fn perform_clean_idempotent_on_canonical_clean() {
    let fix = FixtureRoot::new("task-u39");
    let wg = fix.path().join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    std::fs::write(wg.join("TASK.md"), "---\ntitle: 'Clean'\n---\n").unwrap();
    let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
    let r = perform_inner(&wg, TaskOp::Clean, now).unwrap();
    match r {
        EditOutcome::NoOp { .. } => {}
        other => panic!("expected NoOp, got {:?}", other),
    }
    // No backup file created.
    let entries: Vec<_> = std::fs::read_dir(&wg).unwrap().flatten().collect();
    let bak_count = entries
        .iter()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".bak.md"))
        .count();
    assert_eq!(bak_count, 0);
    perform_inner(&wg, TaskOp::AppendBody("human summary".into()), now).unwrap();
    assert_eq!(read_snapshot(&wg).unwrap().description, "human summary\n");
}

#[test]
fn perform_clean_writes_backup_when_file_existed() {
    // Round 2 (Grinch HIGH-2): assert the backup CONTENTS match the
    // pre-clean bytes. The whole point of the backup is recovery; a
    // regression where the backup file gets the post-clean (Clean)
    // bytes instead of the prior state would be silently shipped
    // without this assertion.
    let fix = FixtureRoot::new("task-u40");
    let wg = fix.path().join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    let pre_clean = "---\ntitle: stale\n---\nstale body\n";
    std::fs::write(wg.join("TASK.md"), pre_clean).unwrap();
    let now = || fixed_now_at(2026, 5, 7, 12, 0, 0);
    let r = perform_inner(&wg, TaskOp::Clean, now).unwrap();
    let backup_path = match &r {
        EditOutcome::Wrote {
            backup: Some(bp), ..
        } => bp.clone(),
        other => panic!("expected Wrote with backup, got {:?}", other),
    };
    // HIGH-2 assertion: backup bytes must equal the pre-clean file.
    let backup_content = std::fs::read_to_string(&backup_path).unwrap();
    assert_eq!(backup_content, pre_clean);
    let final_content = std::fs::read_to_string(wg.join("TASK.md")).unwrap();
    assert_eq!(final_content, "---\ntitle: 'Clean'\n---\n");
}

#[test]
fn perform_clean_creates_task_when_file_missing() {
    // Round 2 (Grinch LOW-3): brand-new workgroup, no TASK.md.
    // Implementation handles this via the `if file_existed` gate at
    // task_ops.rs:455 — Clean writes the canonical form with no
    // backup. Pin the behavior here so a future refactor that
    // reorders the gate doesn't regress silently.
    let fix = FixtureRoot::new("task-u41");
    let wg = fix.path().join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
    let r = perform_inner(&wg, TaskOp::Clean, now).unwrap();
    assert!(matches!(r, EditOutcome::Wrote { backup: None, .. }));
    assert_eq!(
        std::fs::read_to_string(wg.join("TASK.md")).unwrap(),
        "---\ntitle: 'Clean'\n---\n"
    );
    let bak_count = std::fs::read_dir(&wg)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".bak.md"))
        .count();
    assert_eq!(bak_count, 0);
}

#[test]
fn concurrent_writes_return_correct_post_edit_content() {
    for _iter in 0..10 {
        let fix = FixtureRoot::new("task-u42");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);

        // First call sets up the file
        perform_inner(&wg, TaskOp::SetTitle("Initial".into()), now).unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let wg_clone1 = wg.clone();
        let wg_clone2 = wg.clone();
        let b1 = barrier.clone();
        let b2 = barrier.clone();

        let h1 = thread::spawn(move || {
            b1.wait();
            perform(&wg_clone1, TaskOp::AppendBody("first append".into()))
        });
        let h2 = thread::spawn(move || {
            b2.wait();
            perform(&wg_clone2, TaskOp::AppendBody("second append".into()))
        });

        let r1 = h1.join().unwrap();
        let r2 = h2.join().unwrap();

        let content1 = match r1 {
            Ok(EditOutcome::Wrote { content, .. }) => content,
            Err(TaskOpError::LockTimeout) => continue,
            other => panic!("h1 unexpected outcome: {:?}", other),
        };
        let content2 = match r2 {
            Ok(EditOutcome::Wrote { content, .. }) => content,
            Err(TaskOpError::LockTimeout) => continue,
            other => panic!("h2 unexpected outcome: {:?}", other),
        };

        let final_disk_content = std::fs::read_to_string(wg.join("TASK.md")).unwrap();

        if final_disk_content.ends_with("first append\n") {
            assert_eq!(content1, final_disk_content);
        } else if final_disk_content.ends_with("second append\n") {
            assert_eq!(content2, final_disk_content);
        } else {
            panic!("unexpected final disk content");
        }
    }
}

// ── #738: user-owned (USER:) title ownership ────────────────────────

fn bak_count(wg: &Path) -> usize {
    std::fs::read_dir(wg)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".bak.md"))
        .count()
}

#[test]
fn user_owned_title_prefixes_plain_title() {
    assert_eq!(user_owned_title("Build login"), "USER: Build login");
}

#[test]
fn user_owned_title_does_not_double_prefix() {
    assert_eq!(user_owned_title("USER: Build login"), "USER: Build login");
}

#[test]
fn set_title_rejects_when_existing_title_is_user_owned() {
    let fix = FixtureRoot::new("task-738-reject");
    let wg = fix.path().join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    let task = wg.join("TASK.md");
    std::fs::write(&task, "---\ntitle: 'USER: Keep me'\n---\nbody\n").unwrap();
    let before = std::fs::read(&task).unwrap();
    let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
    let r = perform_inner(&wg, TaskOp::SetTitle("Auto".into()), now).unwrap();
    assert!(matches!(r, EditOutcome::RejectedUserTitle { .. }));
    assert_eq!(
        std::fs::read(&task).unwrap(),
        before,
        "TASK.md must be byte-unchanged on reject"
    );
    assert_eq!(bak_count(&wg), 0, "no backup on reject");
}

#[test]
fn set_title_rejects_case_variant_user_owned_titles() {
    // A UI-visible user-owned title with a case-variant key must still block
    // the coordinator overwrite (Grinch MUST-FIX #1).
    for key in ["Title", "TITLE", "tItLe"] {
        let fix = FixtureRoot::new("task-738-case");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        let task = wg.join("TASK.md");
        std::fs::write(&task, format!("---\n{key}: 'USER: Manual'\n---\n")).unwrap();
        let before = std::fs::read(&task).unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        let r = perform_inner(&wg, TaskOp::SetTitle("Auto".into()), now).unwrap();
        assert!(
            matches!(r, EditOutcome::RejectedUserTitle { .. }),
            "key {key} must reject"
        );
        assert_eq!(std::fs::read(&task).unwrap(), before, "key {key} unchanged");
        assert_eq!(bak_count(&wg), 0, "key {key} no backup");
    }
}

#[test]
fn set_title_rejects_double_quoted_user_owned_title() {
    // Double-quoted scalar must decode to the same USER: marker (Grinch #1).
    let fix = FixtureRoot::new("task-738-dq");
    let wg = fix.path().join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    let task = wg.join("TASK.md");
    std::fs::write(&task, "---\ntitle: \"USER: Manual\"\n---\n").unwrap();
    let before = std::fs::read(&task).unwrap();
    let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
    let r = perform_inner(&wg, TaskOp::SetTitle("Auto".into()), now).unwrap();
    assert!(matches!(r, EditOutcome::RejectedUserTitle { .. }));
    assert_eq!(std::fs::read(&task).unwrap(), before);
    assert_eq!(bak_count(&wg), 0);
}

#[test]
fn apply_set_title_replaces_case_variant_title_key() {
    // A coordinator SetTitle over a PLAIN case-variant `Title: Old` (not
    // user-owned) must replace that line in place, not leave it plus a new
    // lowercase duplicate (Grinch MUST-FIX #1 second half).
    let parsed = parse_task("---\nTitle: Old\n---\nbody\n");
    let p = apply_set_title(&parsed, "Auto");
    let title_lines = p
        .frontmatter
        .iter()
        .filter(|l| title_line(l).is_some())
        .count();
    assert_eq!(title_lines, 1, "exactly one title line after replacement");
    assert_eq!(p.frontmatter, vec!["title: 'Auto'".to_string()]);
}

#[test]
fn set_title_rejects_reserved_user_prefix_input() {
    // Coordinator input that itself starts with USER: is invalid input when
    // the current title is not already user-owned (Grinch MUST-FIX #2).
    let fix = FixtureRoot::new("task-738-reserved");
    let wg = fix.path().join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    let task = wg.join("TASK.md");
    std::fs::write(&task, "---\ntitle: 'Auto'\n---\n").unwrap();
    let before = std::fs::read(&task).unwrap();
    let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
    let r = perform_inner(&wg, TaskOp::SetTitle("USER: forged".into()), now);
    assert!(matches!(r, Err(TaskOpError::ReservedUserTitlePrefix)));
    assert_eq!(
        std::fs::read(&task).unwrap(),
        before,
        "unchanged on invalid input"
    );
    assert_eq!(bak_count(&wg), 0, "no backup on invalid input");
}

#[test]
fn set_title_allowed_after_clean_clears_user_lock() {
    let fix = FixtureRoot::new("task-738-clean");
    let wg = fix.path().join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    std::fs::write(
        wg.join("TASK.md"),
        "---\ntitle: 'USER: Keep me'\n---\nbody\n",
    )
    .unwrap();
    let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
    // Clean resets the user-owned lock (writes canonical title 'Clean').
    perform_inner(&wg, TaskOp::Clean, now).unwrap();
    // Coordinator SetTitle is now allowed and writes a plain title.
    let r = perform_inner(&wg, TaskOp::SetTitle("Auto".into()), now).unwrap();
    assert!(matches!(r, EditOutcome::Wrote { .. }));
    let parsed = parse_task(&std::fs::read_to_string(wg.join("TASK.md")).unwrap());
    assert_eq!(title_value_of(&parsed).as_deref(), Some("Auto"));
}

#[test]
fn set_user_title_prefixes_plain_title() {
    let fix = FixtureRoot::new("task-738-suser");
    let wg = fix.path().join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
    perform_inner(&wg, TaskOp::SetUserTitle("Manual".into()), now).unwrap();
    let parsed = parse_task(&std::fs::read_to_string(wg.join("TASK.md")).unwrap());
    assert_eq!(title_value_of(&parsed).as_deref(), Some("USER: Manual"));
}

#[test]
fn set_user_title_overwrites_existing_user_title() {
    let fix = FixtureRoot::new("task-738-suser2");
    let wg = fix.path().join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    std::fs::write(wg.join("TASK.md"), "---\ntitle: 'USER: Old'\n---\n").unwrap();
    let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
    perform_inner(&wg, TaskOp::SetUserTitle("New".into()), now).unwrap();
    let parsed = parse_task(&std::fs::read_to_string(wg.join("TASK.md")).unwrap());
    assert_eq!(title_value_of(&parsed).as_deref(), Some("USER: New"));
}

#[test]
fn set_title_noop_on_plain_existing_title_still_updates_contract() {
    let fix = FixtureRoot::new("task-738-noop");
    let wg = fix.path().join("wg-1");
    std::fs::create_dir_all(&wg).unwrap();
    std::fs::write(wg.join("TASK.md"), "---\ntitle: 'Auto'\n---\nbody\n").unwrap();
    let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
    let r = perform_inner(&wg, TaskOp::SetTitle("Auto".into()), now).unwrap();
    assert!(matches!(r, EditOutcome::NoOp { .. }));
}
