use super::*;

const ISSUE_2733_HINT: &str = "Disable the Loop or choose an existing room";

fn issue_2733_config_bytes(ac_root: &Path, id: &str) -> Vec<u8> {
    std::fs::read(loop_dir(ac_root, id).join(LOOP_CONFIG_FILE)).expect("config bytes")
}

#[test]
fn issue_2733_disable_with_missing_room_succeeds() {
    let tmp = fixture_project();
    let ac_root = tmp.path().join(".ac");
    let config = sample_config();
    create_loop_files(tmp.path(), &ac_root, &config).expect("create with room present");
    // Positive control: the same kind of update succeeds while the room resolves.
    update_loop_files(
        tmp.path(),
        &ac_root,
        &config.loop_def.id,
        LoopUpdatePatch {
            name: Some("Still resolvable".to_string()),
            ..LoopUpdatePatch::default()
        },
    )
    .expect("update while room resolves");

    std::fs::remove_dir_all(ac_root.join("wg-1-dev-team")).expect("remove room");
    update_loop_files(
        tmp.path(),
        &ac_root,
        &config.loop_def.id,
        LoopUpdatePatch {
            enabled: Some(false),
            ..LoopUpdatePatch::default()
        },
    )
    .expect("disable with missing room");

    let reread = read_loop_config(&loop_dir(&ac_root, &config.loop_def.id)).expect("reread");
    assert!(!reread.loop_def.enabled);
    let text =
        String::from_utf8(issue_2733_config_bytes(&ac_root, &config.loop_def.id)).expect("utf8");
    assert!(text.contains("enabled = false"), "{text}");
}

#[test]
fn issue_2733_edit_disabled_loop_with_missing_room_succeeds() {
    let tmp = fixture_project();
    let ac_root = tmp.path().join(".ac");
    let mut config = sample_config();
    config.loop_def.enabled = false;
    create_loop_files(tmp.path(), &ac_root, &config).expect("create disabled");

    update_loop_files(
        tmp.path(),
        &ac_root,
        &config.loop_def.id,
        LoopUpdatePatch {
            name: Some("Retargeted".to_string()),
            workgroup: Some("room-99-missing".to_string()),
            ..LoopUpdatePatch::default()
        },
    )
    .expect("edit disabled loop to a missing room");

    let reread = read_loop_config(&loop_dir(&ac_root, &config.loop_def.id)).expect("reread");
    assert_eq!(reread.loop_def.name, "Retargeted");
    assert_eq!(reread.target.workgroup, "room-99-missing");
    assert!(!reread.loop_def.enabled);
}

#[test]
fn issue_2733_enable_with_missing_room_is_rejected() {
    let tmp = fixture_project();
    let ac_root = tmp.path().join(".ac");
    let mut config = sample_config();
    config.loop_def.enabled = false;
    config.target.workgroup = "room-99-missing".to_string();
    create_loop_files(tmp.path(), &ac_root, &config).expect("create disabled");
    let before = issue_2733_config_bytes(&ac_root, &config.loop_def.id);

    let err = update_loop_files(
        tmp.path(),
        &ac_root,
        &config.loop_def.id,
        LoopUpdatePatch {
            enabled: Some(true),
            ..LoopUpdatePatch::default()
        },
    )
    .expect_err("enable must be rejected");

    assert!(err.contains("not found in project"), "{err}");
    assert!(err.contains(ISSUE_2733_HINT), "{err}");
    assert_eq!(
        issue_2733_config_bytes(&ac_root, &config.loop_def.id),
        before
    );
}

#[test]
fn issue_2733_update_while_enabled_with_missing_room_is_rejected() {
    let tmp = fixture_project();
    let ac_root = tmp.path().join(".ac");
    let config = sample_config();
    create_loop_files(tmp.path(), &ac_root, &config).expect("create enabled");
    std::fs::remove_dir_all(ac_root.join("wg-1-dev-team")).expect("remove room");
    let before = issue_2733_config_bytes(&ac_root, &config.loop_def.id);

    let err = update_loop_files(
        tmp.path(),
        &ac_root,
        &config.loop_def.id,
        LoopUpdatePatch {
            name: Some("Renamed".to_string()),
            ..LoopUpdatePatch::default()
        },
    )
    .expect_err("save while enabled must be rejected");

    assert!(err.contains("not found in project"), "{err}");
    assert!(err.contains(ISSUE_2733_HINT), "{err}");
    assert_eq!(
        issue_2733_config_bytes(&ac_root, &config.loop_def.id),
        before
    );
}

#[test]
fn issue_2733_create_disabled_missing_room_ok_enabled_rejected() {
    let tmp = fixture_project();
    let ac_root = tmp.path().join(".ac");
    let mut disabled = sample_config();
    disabled.loop_def.enabled = false;
    disabled.target.workgroup = "room-99-missing".to_string();
    create_loop_files(tmp.path(), &ac_root, &disabled).expect("create disabled");

    let mut enabled = disabled.clone();
    enabled.loop_def.id = "enabled-missing".to_string();
    enabled.loop_def.enabled = true;
    let err = create_loop_files(tmp.path(), &ac_root, &enabled)
        .expect_err("create enabled must be rejected");
    assert!(err.contains(ISSUE_2733_HINT), "{err}");
    assert!(!loop_dir(&ac_root, "enabled-missing").exists());
}

#[test]
fn issue_2733_structural_errors_rejected_when_disabled() {
    let tmp = fixture_project();
    let mut base = sample_config();
    base.loop_def.enabled = false;
    validate_loop_config(tmp.path(), &base).expect("disabled baseline is valid");

    let mut cases = Vec::new();
    let mut c = base.clone();
    c.trigger.expr = "bad".to_string();
    cases.push(("cron", c));
    let mut c = base.clone();
    c.target.workgroup = "team-1".to_string();
    cases.push(("prefix", c));
    let mut c = base.clone();
    c.prompt.body = "  ".to_string();
    cases.push(("prompt", c));
    let mut c = base.clone();
    c.loop_def.name = String::new();
    cases.push(("name", c));
    let mut c = base.clone();
    c.trigger.timezone = "utc".to_string();
    cases.push(("timezone", c));

    for (label, config) in cases {
        assert!(
            validate_loop_config(tmp.path(), &config).is_err(),
            "{label} must be rejected while disabled"
        );
    }
}

#[test]
fn issue_2733_audit_kind_round_trips() {
    let json = serde_json::to_string(&LoopAuditKind::TargetUnresolved).expect("serialize");
    assert_eq!(json, "\"targetUnresolved\"");
    let back: LoopAuditKind = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, LoopAuditKind::TargetUnresolved);

    let legacy = concat!(
        r#"{"runId":"6a7cfa8e-0e0a-4a0f-9d1e-2f3d9a1b4c55","loopId":"daily-sync","#,
        r#""projectPath":"/tmp/project","kind":"pendingBusy","dueAt":"2025-01-01T09:00:00Z","#,
        r#""startedAt":"2025-01-01T09:00:01Z","completedAt":null,"#,
        r#""target":"proj:wg-1-dev-team/tech-lead","sessionId":null,"#,
        r#""busyCoordinatorPolicy":"waitUntilIdle","error":null,"promptSnapshot":null}"#,
    );
    let parsed = serde_json::from_str::<LoopAuditEntry>(legacy).expect("legacy row parses");
    assert_eq!(parsed.kind, LoopAuditKind::PendingBusy);
}

#[test]
fn loop_id_sanitizes_and_rejects_unsafe_existing_ids() {
    assert_eq!(
        sanitize_loop_id("Weekday Standup!").unwrap(),
        "weekday-standup"
    );
    for id in ["", "-daily", "daily-", "../daily", "daily!", "daily_loop"] {
        assert!(validate_loop_id(id).is_err(), "{id} should fail");
    }
}

#[test]
fn loop_policy_serializes_camel_case_keys() {
    let mut config = sample_config();
    config.policy.busy_coordinator = BusyCoordinatorPolicy::ForceInject;
    let toml = toml::to_string(&config).expect("toml");
    assert!(toml.contains("missedWhileClosed = \"notify\""), "{toml}");
    assert!(toml.contains("busyCoordinator = \"forceInject\""), "{toml}");
}

#[test]
fn cron_validation_rejects_six_fields_before_parser() {
    let err = validate_cron_expr("0 0 9 * * 1").unwrap_err();
    assert!(err.contains("exactly 5 fields"), "{err}");
}

#[test]
fn cron_preview_returns_future_due_time() {
    let after = "2026-06-13T21:00:00Z"
        .parse::<DateTime<Utc>>()
        .expect("time");
    let next = next_due_after("*/5 * * * *", after)
        .expect("preview")
        .expect("next");
    assert!(next > after);
}

#[test]
fn latest_due_between_uses_previous_match_for_long_gaps() {
    let now = "2026-06-13T21:17:00Z"
        .parse::<DateTime<Utc>>()
        .expect("time");
    let after = now - chrono::Duration::days(30);
    let due = latest_due_between("* * * * *", after, now)
        .expect("due")
        .expect("due present");

    assert!(due > after);
    assert!(due <= now);
    assert!(now - due < chrono::Duration::minutes(2));
}

#[test]
fn loop_update_patch_only_resets_on_actual_schedule_or_delivery_changes() {
    let mut config = sample_config();
    let reset = apply_loop_update_patch(
        &mut config,
        LoopUpdatePatch {
            name: Some("Renamed standup".to_string()),
            ..LoopUpdatePatch::default()
        },
    )
    .expect("name update");
    assert!(!reset);
    assert_eq!(config.loop_def.name, "Renamed standup");

    let reset = apply_loop_update_patch(
        &mut config,
        LoopUpdatePatch {
            expr: Some("0 9 * * 1-5".to_string()),
            workgroup: Some("wg-1-dev-team".to_string()),
            prompt_body: Some("Summarize status".to_string()),
            busy_coordinator: Some(BusyCoordinatorPolicy::WaitUntilIdle),
            enabled: Some(true),
            ..LoopUpdatePatch::default()
        },
    )
    .expect("noop update");
    assert!(!reset);

    let reset = apply_loop_update_patch(
        &mut config,
        LoopUpdatePatch {
            expr: Some("30 9 * * 1-5".to_string()),
            ..LoopUpdatePatch::default()
        },
    )
    .expect("cron update");
    assert!(reset);
    assert_eq!(config.trigger.expr, "30 9 * * 1-5");
}

/// AC-1 - a legacy persisted config (no `sessionStart` key) loads as
/// `Fresh`; the fixture is asserted not to carry the key so the test cannot
/// pass for the wrong reason.
#[test]
fn legacy_config_without_session_start_loads_fresh() {
    let fixture = r#"
[loop]
id = "legacy-loop"
name = "Legacy loop"
enabled = true

[trigger]
kind = "cron"
expr = "0 9 * * *"
timezone = "local"

[target]
kind = "workgroupCoordinator"
workgroup = "wg-1-dev-team"

[prompt]
body = "Send status"

[policy]
busyCoordinator = "waitUntilIdle"
"#;
    assert!(!fixture.contains("sessionStart"));

    let parsed: LoopConfigToml = toml::from_str(fixture).expect("legacy config parses");
    assert_eq!(parsed.policy.session_start, LoopSessionStart::Fresh);
    assert_eq!(
        parsed.policy.busy_coordinator,
        BusyCoordinatorPolicy::WaitUntilIdle
    );
}

/// AC-2 - the struct default is `Fresh` and both variants keep their wire
/// spellings across a serialize/deserialize round trip.
#[test]
fn loop_session_start_defaults_to_fresh_and_round_trips() {
    assert_eq!(LoopPolicy::default().session_start, LoopSessionStart::Fresh);
    for (variant, wire) in [
        (LoopSessionStart::Fresh, "\"fresh\""),
        (LoopSessionStart::Accumulate, "\"accumulate\""),
    ] {
        let encoded = serde_json::to_string(&variant).expect("serialize session start");
        assert_eq!(encoded, wire);
        let decoded: LoopSessionStart =
            serde_json::from_str(&encoded).expect("deserialize session start");
        assert_eq!(decoded, variant);
    }

    let toml = toml::to_string(&sample_config()).expect("toml");
    assert!(toml.contains("sessionStart = \"fresh\""), "{toml}");
}

/// AC-11 - `None` leaves the stored value untouched; `Some(Accumulate)`
/// applies exactly like `busy_coordinator` and the summary reflects it.
#[test]
fn loop_update_patch_applies_session_start_like_busy_coordinator() {
    let mut config = sample_config();
    assert_eq!(config.policy.session_start, LoopSessionStart::Fresh);

    let reset = apply_loop_update_patch(
        &mut config,
        LoopUpdatePatch {
            session_start: None,
            ..LoopUpdatePatch::default()
        },
    )
    .expect("no-op patch");
    assert!(!reset);
    assert_eq!(config.policy.session_start, LoopSessionStart::Fresh);

    let reset = apply_loop_update_patch(
        &mut config,
        LoopUpdatePatch {
            session_start: Some(LoopSessionStart::Accumulate),
            ..LoopUpdatePatch::default()
        },
    )
    .expect("session start patch");
    assert!(reset);
    assert_eq!(config.policy.session_start, LoopSessionStart::Accumulate);

    let tmp = fixture_project();
    let summary = summary_from_parts(&tmp.path().join(".ac"), &config, &LoopState::default());
    assert_eq!(summary.session_start, LoopSessionStart::Accumulate);
}

/// AC-14 - a `sessionStart`-only edit is visible to the delivery
/// revalidation, and equal configs still match.
#[test]
fn loop_delivery_config_matches_pins_session_start() {
    let current = sample_config();
    assert!(loop_delivery_config_matches(&current, &current.clone()));

    let mut expected = current.clone();
    expected.policy.session_start = LoopSessionStart::Accumulate;
    assert!(!loop_delivery_config_matches(&current, &expected));
    assert!(!loop_delivery_config_matches(&expected, &current));
}

#[test]
fn storage_writes_config_state_and_dedupes_audit() {
    let tmp = fixture_project();
    let ac_root = tmp.path().join(".ac");
    let config = sample_config();
    validate_loop_config(tmp.path(), &config).expect("valid config");

    let dir = write_loop_config(&ac_root, &config).expect("write config");
    assert!(dir.join(LOOP_CONFIG_FILE).is_file());

    let state = LoopState {
        last_checked_at: Some(Utc::now()),
        ..LoopState::default()
    };
    write_loop_state_atomic(&dir, &state).expect("state write");
    let read = read_loop_state(&dir).expect("state read");
    assert!(read.last_checked_at.is_some());

    let entry = LoopAuditEntry {
        run_id: Uuid::new_v4(),
        loop_id: config.loop_def.id.clone(),
        project_path: tmp.path().to_string_lossy().to_string(),
        kind: LoopAuditKind::PendingBusy,
        due_at: Utc::now(),
        started_at: Utc::now(),
        completed_at: None,
        target: Some("proj:wg-1-dev-team/tech-lead".to_string()),
        session_id: None,
        busy_coordinator_policy: BusyCoordinatorPolicy::WaitUntilIdle,
        session_start: Some(LoopSessionStart::Fresh),
        error: None,
        prompt_snapshot: None,
    };
    append_loop_audit_once(&dir, &entry).expect("audit one");
    append_loop_audit_once(&dir, &entry).expect("audit duplicate");
    let content = std::fs::read_to_string(dir.join(LOOP_AUDIT_FILE)).expect("audit read");
    assert_eq!(content.lines().count(), 1);
}

#[test]
fn storage_writes_do_not_recreate_loop_dirs_without_config() {
    let tmp = fixture_project();
    let ac_root = tmp.path().join(".ac");
    let config = sample_config();
    let dir = write_loop_config(&ac_root, &config).expect("write config");
    let state = LoopState {
        last_checked_at: Some(Utc::now()),
        ..LoopState::default()
    };
    let entry = LoopAuditEntry {
        run_id: Uuid::new_v4(),
        loop_id: config.loop_def.id.clone(),
        project_path: tmp.path().to_string_lossy().to_string(),
        kind: LoopAuditKind::PendingBusy,
        due_at: Utc::now(),
        started_at: Utc::now(),
        completed_at: None,
        target: Some("proj:wg-1-dev-team/tech-lead".to_string()),
        session_id: None,
        busy_coordinator_policy: BusyCoordinatorPolicy::WaitUntilIdle,
        session_start: Some(LoopSessionStart::Fresh),
        error: None,
        prompt_snapshot: None,
    };

    std::fs::remove_dir_all(&dir).expect("remove loop dir");
    assert!(write_loop_state_atomic(&dir, &state).is_err());
    assert!(!dir.exists());
    assert!(append_loop_audit_once(&dir, &entry).is_err());
    assert!(!dir.exists());

    std::fs::create_dir_all(&dir).expect("recreate dir without config");
    assert!(write_loop_state_atomic(&dir, &state).is_err());
    assert!(append_loop_audit_once(&dir, &entry).is_err());
    assert!(!dir.join(LOOP_STATE_FILE).exists());
    assert!(!dir.join(LOOP_AUDIT_FILE).exists());
}

#[test]
fn discovery_omits_prompt_body_and_tolerates_bad_state() {
    let tmp = fixture_project();
    let ac_root = tmp.path().join(".ac");
    let config = sample_config();
    let dir = write_loop_config(&ac_root, &config).expect("write config");
    std::fs::write(dir.join(LOOP_STATE_FILE), "{bad json").expect("bad state");

    let loops = discover_loops_in_project(tmp.path());
    assert_eq!(loops.len(), 1);
    assert_eq!(loops[0].id, "weekday-standup");
    assert_eq!(loops[0].prompt_preview, "Summarize status");
    assert!(loops[0].last_checked_at.is_none());
}

/// AC-15 - a legacy audit row (written before `sessionStart` existed)
/// parses, and re-serializes the field as JSON `null` rather than as any
/// concrete value the run may never have used. The test names no Rust
/// field, so it compiles against both shapes and fails at runtime.
#[test]
fn legacy_audit_row_without_session_start_parses_as_not_recorded() {
    let line = concat!(
        r#"{"runId":"6a7cfa8e-0e0a-4a0f-9d1e-2f3d9a1b4c55","loopId":"daily-sync","#,
        r#""projectPath":"/tmp/project","kind":"pendingBusy","dueAt":"2025-01-01T09:00:00Z","#,
        r#""startedAt":"2025-01-01T09:00:01Z","completedAt":null,"#,
        r#""target":"proj:wg-1-dev-team/tech-lead","sessionId":null,"#,
        r#""busyCoordinatorPolicy":"waitUntilIdle","error":null,"promptSnapshot":null}"#,
    );

    assert!(
        !line.contains("sessionStart"),
        "the legacy fixture must not carry the key, or this test passes for the wrong reason"
    );

    let parsed = serde_json::from_str::<LoopAuditEntry>(line).expect("legacy audit row parses");

    assert_eq!(
        serde_json::to_value(&parsed)
            .expect("re-serialize")
            .get("sessionStart"),
        Some(&serde_json::Value::Null),
        "an absent key must read back as not recorded, never as a concrete default"
    );
}

/// AC-16 - a legacy audit row still takes part in the append-once dedupe,
/// so no duplicate row is appended and the file is never rewritten.
#[test]
fn legacy_audit_row_still_deduplicates_an_append() {
    let tmp = fixture_project();
    let ac_root = tmp.path().join(".ac");
    let config = sample_config();
    let dir = write_loop_config(&ac_root, &config).expect("write config");

    let legacy = format!(
        concat!(
            r#"{{"runId":"6a7cfa8e-0e0a-4a0f-9d1e-2f3d9a1b4c55","loopId":"{}","#,
            r#""projectPath":"/tmp/project","kind":"pendingBusy","dueAt":"2025-01-01T09:00:00Z","#,
            r#""startedAt":"2025-01-01T09:00:01Z","completedAt":null,"target":null,"#,
            r#""sessionId":null,"busyCoordinatorPolicy":"waitUntilIdle","error":null,"#,
            r#""promptSnapshot":null}}"#,
        ),
        config.loop_def.id
    );
    assert!(!legacy.contains("sessionStart"));
    let audit_path = dir.join(LOOP_AUDIT_FILE);
    std::fs::write(&audit_path, format!("{}\n", legacy)).expect("write legacy audit line");

    let mut value: serde_json::Value = serde_json::from_str(&legacy).expect("legacy line as value");
    value
        .as_object_mut()
        .expect("object")
        .insert("sessionStart".to_string(), serde_json::json!("fresh"));
    let entry: LoopAuditEntry = serde_json::from_value(value).expect("entry to append");

    append_loop_audit_once(&dir, &entry).expect("append against a legacy audit file");

    let content = std::fs::read_to_string(&audit_path).expect("audit read");
    assert_eq!(
        content.lines().count(),
        1,
        "the legacy row must be seen by the dedupe, so nothing is appended"
    );
    assert_eq!(content, format!("{}\n", legacy), "no row may be rewritten");
}
