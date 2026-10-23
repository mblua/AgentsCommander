use super::*;

// T16 (#1172 D5), rescoped by #1175. READ THIS BEFORE TRUSTING IT.
//
// This is a SOURCE-LEVEL INVENTORY over `session.rs` alone. It does NOT prove
// that a resume never rotates, and it does not close the class of changes that
// retarget the rotation. Three measured facts bound it:
//
//   - A cross-file alias (`pub(crate) use ... as <alias>;` in `config/mod.rs`
//     plus an ungated call to the alias here) keeps assertions 1 to 3 intact
//     while a resume rotates. Recorded in the issue.
//   - A cross-file CALL added in `lib.rs` before the restore at `:2350` is
//     invisible to every assertion here and to both behavioral tests below.
//   - `skip_auto_resume |= is_coordinator;` inserted immediately above the
//     binding leaves ALL FIVE assertions and BOTH behavioral tests green while
//     a coordinator resume rotates. Measured; plan 2.8.
//
// What it DOES enforce, and what nothing else in this suite can:
//   - assertions 1 to 3: `rotate_origin_memory_at_spawn` is REFERENCED exactly
//     twice in this file, once in the comment and once in the gated call. A
//     third occurrence is an UNBUDGETED REFERENCE; it need not be a call. This
//     is what catches an ungated second call added on a launch branch the
//     behavioral tests do not drive (`execute_restart_transaction` at `:3897`,
//     the Root Agent launch at `:4703`).
//   - assertions 4 and 5: the name `start_fresh` is bound exactly once, and its
//     right-hand side is exactly the resume flag. That is a complete statement
//     about ONE LINE. It says nothing about the value flowing into that line,
//     which is the residual named above and in plan D5.
//
// The normative property is guarded behaviorally by
// `a_production_shaped_startup_restore_never_rotates_while_a_fresh_launch_does`
// and `create_session_inner_rotates_the_fresh_target_and_not_the_resumed_one`,
// for the entry points and argument shapes those tests drive and no further.
// Plan section 4.5 enumerates what neither side covers.
//
// One invisible dependency, already satisfied: the split needle below uses
// `\n`. This repository sets `core.autocrlf=true` on Windows checkouts, and
// `.gitattributes` `*.rs text eol=lf` is what keeps `session.rs` LF on disk
// (measured: 0 CRLF, 9344 LF). If that attribute were ever dropped, this test
// fails LOUD rather than silent: assertion 1 would count 2, because the needle
// also appears as this test's own `let gated_call = "..."` literal.
#[test]
fn rotate_origin_memory_at_spawn_has_one_gated_call_in_session_rs() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/commands/session.rs"
    ))
    .expect("read session.rs");
    let production = source
        .split("#[cfg(test)]\nmod tests")
        .next()
        .expect("production session source");
    let normalized = production.split_whitespace().collect::<String>();
    let gated_call = concat!(
        "ifstart_fresh&&context_result.is_ok(){",
        "crate::config::agent_memory::rotate_origin_memory_at_spawn(&cwd);}"
    );

    assert_eq!(
        normalized.matches(gated_call).count(),
        1,
        "#1172 D5: the memory rotation must stay behind `start_fresh && context_result.is_ok()`. \
             This pins the gate's exact spelling at the single call site in this file; it does not \
             prove the resume property, and the behavioral tests named above prove it only for the \
             entry points they drive."
    );
    // What assertions 2 and 3 add to assertion 1, stated as the bounded
    // property they actually enforce.
    //
    // Assertion 1 pins one exact spelling of the gated call, so a second
    // call spelled differently would not match its needle: a different
    // argument, the path wrapped in parens so it reads `..._at_spawn)(`, or
    // a block comment between the identifier and the paren. Assertions 2
    // and 3 reach those by counting the BARE identifier rather than an
    // invocation shape, and by pinning the one legitimate occurrence that
    // is not a call, the comment 5.4 requires above the gate.
    //
    // The property they enforce, and nothing broader: within `session.rs`
    // the exact identifier `rotate_origin_memory_at_spawn` occurs exactly
    // twice, once in that comment and once in the call assertion 1 already
    // pinned. A third occurrence is unbudgeted.
    //
    // That is a count of one identifier in one file. It does NOT establish
    // that every route to the rotation is accounted for. A call reached
    // through a cross-file alias, and a call added in another file, both
    // leave this count at two; both are recorded in the header above, and
    // the second is plan 9.2's M9, measured green against all five
    // assertions here and both behavioral tests below.
    let comment_mention = "`rotate_origin_memory_at_spawn`returns`()`";
    assert_eq!(
        normalized.matches(comment_mention).count(),
        1,
        "#1172 D5: the comment above the gate must keep naming the rotation exactly once; \
             it is the one non-call mention the reference budget below accounts for."
    );
    assert_eq!(
        normalized.matches("rotate_origin_memory_at_spawn").count(),
        2,
        "#1172 D5: within THIS FILE the rotation entry point must be REFERENCED exactly twice - \
             once in the comment above the gate, once in the gated call itself. A third occurrence \
             is unbudgeted: it may be a second call site, an import, or any other reference, and \
             each of those is a change this inventory deliberately refuses to absorb silently. A \
             cross-file alias evades this count entirely; see the header comment."
    );
    // #1175 D2 part 4. The gate's INPUT, not just its shape. Bounded claim: this
    // pins ONE LINE. It cannot see how `skip_auto_resume` got its value; see the
    // header and plan D5 residual 3.
    assert_eq!(
        normalized
            .matches("letstart_fresh=skip_auto_resume;")
            .count(),
        1,
        "#1175: `start_fresh` must be bound to the resume flag ALONE. Appending `|| <extra>` \
             here makes a RESUME rotate on the production launches that supply the extra input. \
             Measured (plan 2.7): `|| pending_start_fresh.is_some()` reds this assertion AND the \
             production-shaped restore test; the `|| is_coordinator` variant reds only this one, \
             because no cwd in this test binary can be a coordinator."
    );
    // #1175 round 2. Assertion 4 alone is defeated by ordinary shadowing:
    // `let start_fresh = skip_auto_resume; let start_fresh = start_fresh || X;`
    // preserves its needle exactly. MEASURED (plan 2.8): this assertion is the
    // only thing in the suite that reds on that form.
    assert_eq!(
        normalized.matches("letstart_fresh=").count(),
        1,
        "#1175: `start_fresh` must be bound EXACTLY ONCE. A second binding shadows the first \
             and can compose any additional input into the rotation decision while assertion 4's \
             needle stays intact."
    );
}

/// #1175 B1. The behavioral guard on #1172 D5's normative property, within the
/// scope D5 of this plan declares. This EXECUTES the launch path and looks at
/// the filesystem, so unlike the reference inventory above it reds under the
/// cross-file alias the issue built.
///
/// The resume half is PRODUCTION-SHAPED, not merely nominal. Round 0 passed
/// `pending_start_fresh = None`, `resolved_spawn = None`, `agent_id = None` and
/// `skip_tooling_save = true`, none of which is what `lib.rs:2313-2372` passes
/// on an ordinary startup restore. Grinch showed the cost: a one-line
/// `let start_fresh = skip_auto_resume || pending_start_fresh.is_some();`
/// left every round-0 guard green while a real restore rotated, because
/// production passes `Some(false)` there and round 0 passed `None`. Each
/// argument below is annotated with the production line it mirrors.
///
/// The fresh half is the POSITIVE CONTROL and is not optional: without it, a
/// green resume half would also be produced by a launch that never reached the
/// rotation chokepoint, or by the feature having been deleted outright.
///
/// Observation happens at every boundary, never only at the end. See D3.
#[tokio::test]
async fn a_production_shaped_startup_restore_never_rotates_while_a_fresh_launch_does() {
    let (_fixture, resumed, fresh) = rotation_fixture();
    // `lib.rs:2314` rebuilds the spawn recipe from settings before restoring.
    let settings = test_settings();
    let spawn = super::super::build_configured_agent_spawn_for_cwd(
        &settings,
        "codex",
        &resumed.replica_cwd,
        None,
    )
    .expect("resolve the configured codex spawn")
    .expect("codex is configured in test_settings");
    // `lib.rs:2334-2340` takes shell, args and label from the rebuilt spawn.
    let shell = spawn.shell.clone();
    let shell_args = spawn.shell_args.clone();
    let agent_label = Some(spawn.trusted_agent_label.clone());

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(settings, Arc::clone(&session_mgr), Arc::clone(&pty_mgr));

    let restore = crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
    let restored = super::super::create_session_inner_for_restore(
        &restore,
        &session_mgr,
        &pty_mgr,
        shell,
        shell_args,
        resumed.replica_cwd.clone(),
        Some("startup restore".to_string()),
        Some("codex".to_string()), // lib.rs:2358, ps.agent_id
        agent_label,               // lib.rs:2339, spawn.trusted_agent_label
        false,                     // lib.rs:2360, "Persist tooling on restore"
        Vec::new(),
        false,       // lib.rs:2362, skip_auto_resume_for_restore(false): RESUME
        Some(spawn), // lib.rs:2363, the rebuilt recipe
        None,
        None,        // lib.rs:2365, headless caller keeps 120x30
        Some(false), // lib.rs:2366, Some(ps.start_fresh_on_restore). LOAD-BEARING.
        None,
        None,
        None,
    )
    .await;
    let restored = restored.expect("the production-shaped restore must launch");

    // The launch must be an ACTUAL provider resume, not just a `false` argument.
    // The Codex injection at `:1999` gates on `agent_kind`, and its body gates
    // again on `if let Some(ref aid) = agent_id` at `:2000`, so this only holds
    // because the rebuilt spawn supplies the agent id at `:1446-1450`.
    let effective = restored
        .effective_shell_args
        .clone()
        .unwrap_or_else(|| restored.shell_args.clone());
    assert!(
        effective.iter().any(|arg| arg == "resume"),
        "#1175: the restore must be an ACTUAL Codex provider resume, got {effective:?}"
    );

    // Boundary 1: observe BOTH sides before the second cause runs.
    assert_resume_left_memory_alone(&resumed, "production-shaped startup restore");
    assert_memory_pristine(&fresh, "fresh side, before its own launch");

    let created = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "codex".to_string(),
        Vec::new(),
        fresh.replica_cwd.clone(),
        Some("fresh launch".to_string()),
        None,
        None,
        true,
        Vec::new(),
        true, // a fresh create
        None,
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await;
    assert!(created.is_ok(), "the fresh launch must launch: {created:?}");
    assert_eq!(
        backend.spawn_count.load(Ordering::SeqCst),
        2,
        "both launches must reach the PTY (this proves nothing about the context block)"
    );

    // Boundary 2: the fresh side rotated, and the resume side is STILL untouched.
    assert_memory_rotated_once(&fresh, "fresh create");
    assert_resume_left_memory_alone(&resumed, "restore, rechecked after the fresh launch");
    close_test_coordinator(&app).await;
}

/// #1175 B2. The same function twice with matched arguments and the same
/// session label, one launch fresh and one launch resuming, in the OPPOSITE
/// order from B1.
///
/// It is deliberately NOT called a one-boolean experiment, and it never was
/// one: the cwd differs, and the second launch runs against a `SessionManager`,
/// selection coordinator and backend the first already mutated. Naming that
/// "the flag alone" would grant exactly the false confidence #1175 exists to
/// remove. What it does establish, together with B1, is that no discriminator
/// based solely on first-versus-second launch explains the result: B1 runs
/// resume then fresh, this runs fresh then resume.
///
/// The cwd differs for a positive reason, not because it has to. Two
/// same-target `CreateSelectionIntent::User` creates are supported and are
/// already exercised by `sequential_user_same_target_creates_remain_compatible`
/// (`:6958`); the dedup gate at `:1609` applies to Background and Suppress
/// only. Separate replicas are used so each side has its own independently
/// seeded, non-empty `memory/` to observe.
///
/// Its resume half uses the same `false` polarity as the #599 reopen of a
/// closed coordinator, but it is NOT that scenario: `is_coordinator` is
/// deterministically false for every cwd in this binary, and the Tauri
/// `create_session` producer is bypassed. Plan D5 records coordinator polarity
/// and outer composition as untested.
///
/// The rotating role is assigned to the OPPOSITE replica from B1's, so no
/// single fixture directory is the one that always rotates.
#[tokio::test]
async fn create_session_inner_rotates_the_fresh_target_and_not_the_resumed_one() {
    let (_fixture, fresh, resumed) = rotation_fixture();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );

    let created = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "codex".to_string(),
        Vec::new(),
        fresh.replica_cwd.clone(),
        Some("matched differential".to_string()),
        Some("codex".to_string()),
        Some("Codex".to_string()),
        true,
        Vec::new(),
        true, // FRESH first: the opposite order from B1
        None,
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await;
    assert!(created.is_ok(), "the fresh create must launch: {created:?}");

    // Boundary 1.
    assert_memory_rotated_once(&fresh, "fresh create, observed immediately");
    assert_memory_pristine(&resumed, "resume side, before its own launch");

    let reopened = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        "codex".to_string(),
        Vec::new(),
        resumed.replica_cwd.clone(),
        Some("matched differential".to_string()),
        Some("codex".to_string()),
        Some("Codex".to_string()),
        true,
        Vec::new(),
        false, // the #599 reopen value: this launch RESUMES
        None,
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await;
    assert!(reopened.is_ok(), "the reopen must launch: {reopened:?}");
    assert_eq!(
        backend.spawn_count.load(Ordering::SeqCst),
        2,
        "both launches must reach the PTY"
    );

    // Boundary 2.
    assert_resume_left_memory_alone(&resumed, "reopen that resumes");
    assert_memory_rotated_once(&fresh, "fresh side, rechecked after the resume");
    close_test_coordinator(&app).await;
}

#[test]
fn create_session_inner_marks_spawning_until_spawn_returns() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/commands/session.rs"
    ))
    .expect("read session.rs");
    let production = source
        .split("#[cfg(test)]\nmod tests")
        .next()
        .expect("production session source");
    let normalized = production.split_whitespace().collect::<String>();
    let first_gate = normalized
        .find(concat!(
            "enforce_owned_unarchived_for_spawn(app,&cwd,&session_label,&create_owner,",
            "&create_manager,lifecycle_domain,inline_transaction,).await?;"
        ))
        .expect("first archive gate");
    let mark = normalized
        .find("letspawn_mark={")
        .expect("spawn mark immediately before pending-session creation");
    let pending_create = normalized
        .find("letpending_result=mgr.create_pending_session_with_binding_and_carrier_permit(")
        .expect("pending-session creation");
    let spawn = normalized
        .find("letspawn_result=PtyManager::spawn(pty_mgr,session.backend_kind,spawn_spec).await;")
        .expect("spawn call");
    let drop_mark = normalized
        .find("drop(spawn_mark);")
        .expect("spawn mark drop");
    let spawn_error = normalized
        .find("ifletErr(e)=spawn_result{")
        .expect("spawn error handling");

    assert!(
        first_gate < mark,
        "spawn mark must be created only after pre-create validation"
    );
    assert!(
        mark < pending_create,
        "spawn mark must precede pending-session creation"
    );
    assert!(
        pending_create < spawn,
        "pending-session creation must run before PTY spawn"
    );
    assert!(
        spawn < drop_mark,
        "spawn mark must stay live while spawn awaits"
    );
    assert!(
        drop_mark < spawn_error,
        "spawn mark must drop immediately after spawn returns"
    );
}

// Runtime witness for plan section 12: drive create_session_inner with a
// fake backend and assert archive_liveness sees the spawn mark WHILE the PTY
// is spawning, then sees it retired once the PTY exists. Unlike the
// source-scrape guards above, this executes the code, so it reds under a
// string-preserving runtime mutation (e.g. mark_spawning inserting nothing,
// or the mark dropped before PtyManager::spawn).
#[tokio::test]
async fn create_session_inner_holds_a_spawn_mark_until_the_pty_exists() {
    use crate::pty::backend::PtyBackend;

    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let expected_cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let backend = Arc::new(GatedSpawnBackend::new(started_tx, release_rx, false));
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let app_handle = app.handle().clone();

    let task = {
        let app_handle = app_handle.clone();
        let session_mgr = session_mgr.clone();
        let pty_mgr = pty_mgr.clone();
        let cwd = cwd.clone();
        tokio::spawn(async move {
            super::super::create_session_inner(
                &app_handle,
                &session_mgr,
                &pty_mgr,
                "hold-mark-test-command".to_string(),
                Vec::new(),
                cwd,
                Some("hold-mark".to_string()),
                None,
                None,
                true,
                Vec::new(),
                true,
                None,
                None,
                None, // #973 - no view in this test: 120x30
                CreateSelectionIntent::User,
            )
            .await
        })
    };

    // Park inside the backend's spawn. PtyManager::spawn releases the outer
    // manager mutex before awaiting backend.spawn, so the mark is readable
    // here without racing the spawn task.
    started_rx.await.expect("spawn started");
    let (pending, _) = pty_mgr.lock().unwrap().archive_liveness(&[]);
    assert_eq!(
        pending,
        vec![crate::pty::manager::PendingSpawn {
            cwd: expected_cwd.clone(),
            label: "hold-mark".to_string(),
        }],
        "spawn mark must be live while the PTY is still being spawned"
    );

    let _ = release_tx.send(());
    let info = task
        .await
        .expect("join create_session_inner")
        .expect("create_session_inner should succeed");

    assert!(
        backend.has_session(Uuid::parse_str(&info.id).expect("session id is a uuid")),
        "the PTY must exist once create_session_inner returns Ok"
    );
    let (pending, _) = pty_mgr.lock().unwrap().archive_liveness(&[]);
    assert!(
        pending.is_empty(),
        "spawn mark must retire once the PTY exists"
    );
}

// Runtime witness for plan section 12: the mark is held across a FAILING
// spawn exactly as across a succeeding one, and is retired on the rollback
// path. Holding the mark across the in-flight spawn is what makes this red
// under a string-preserving mutation (mark_spawning no-op / drop before
// spawn); the empty-afterwards assertion pins retirement on the failure arm.
#[tokio::test]
async fn create_session_inner_retires_the_spawn_mark_on_spawn_failure() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let expected_cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let backend = Arc::new(GatedSpawnBackend::new(started_tx, release_rx, true));
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        test_settings(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let app_handle = app.handle().clone();

    let task = {
        let app_handle = app_handle.clone();
        let session_mgr = session_mgr.clone();
        let pty_mgr = pty_mgr.clone();
        let cwd = cwd.clone();
        tokio::spawn(async move {
            super::super::create_session_inner(
                &app_handle,
                &session_mgr,
                &pty_mgr,
                "retire-mark-test-command".to_string(),
                Vec::new(),
                cwd,
                Some("retire-mark".to_string()),
                None,
                None,
                true,
                Vec::new(),
                true,
                None,
                None,
                None, // #973 - no view in this test: 120x30
                CreateSelectionIntent::User,
            )
            .await
        })
    };

    started_rx.await.expect("spawn started");
    let (pending, _) = pty_mgr.lock().unwrap().archive_liveness(&[]);
    assert_eq!(
        pending,
        vec![crate::pty::manager::PendingSpawn {
            cwd: expected_cwd.clone(),
            label: "retire-mark".to_string(),
        }],
        "spawn mark must be live while the failing spawn is in flight"
    );

    let _ = release_tx.send(());
    let err = task
        .await
        .expect("join create_session_inner")
        .expect_err("create_session_inner should fail when the spawn fails");
    assert!(err.contains("synthetic spawn failure"), "{err}");

    let (pending, _) = pty_mgr.lock().unwrap().archive_liveness(&[]);
    assert!(
        pending.is_empty(),
        "spawn mark must retire after a failed spawn"
    );
    assert!(
        session_mgr.read().await.list_sessions().await.is_empty(),
        "rollback must remove the pre-created record on spawn failure"
    );
}

#[test]
fn restart_session_inner_probes_archive_before_destroying_session() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/commands/session.rs"
    ))
    .expect("read session.rs");
    let probe = source
        .find("crate::config::archive_gate::probe_spawn_refusal(app, &cwd).await?;")
        .expect("restart probe call");
    let replacement_spawn = source
        .find("// 3. Release the old runtime slot")
        .expect("restart teardown marker");

    assert!(
        probe < replacement_spawn,
        "restart must probe archive refusal before spawning or destroying a session"
    );
}
