use super::*;

// ---- 8..21: provisioning ---------------------------------------------

#[test]
fn missing_file_seeds_canonical_bytes() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    let report = ensure_injected_messages(dir.path(), &warnings).expect("provision");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::Seeded)
    );

    // Against the transcribed literal, never against the generator.
    assert_eq!(EXPECTED_SEED.len(), 3712, "the pinned seed is 3712 bytes");
    assert!(!EXPECTED_SEED.contains('\r'), "the pinned seed is LF");
    let written = read(&main_path(dir.path()));
    assert_eq!(written, EXPECTED_SEED);
    assert!(!written.contains("\r\n"), "seeded file must be LF");
    assert!(written.ends_with("'''\n"));
    assert!(!written.ends_with("\n\n"), "exactly one trailing newline");
    assert!(written.contains("schema_version = 1"));
    assert!(written.contains("coverage_version = 2"));
    assert!(written.contains("[messages.context-alert]"));

    let state: InjectedMessagesState =
        serde_json::from_str(&read(&state_path(dir.path()))).expect("sidecar");
    assert_eq!(state.schema_version, SCHEMA_VERSION);
    assert_eq!(
        state.messages[CONTEXT_ALERT_MESSAGE_ID].last_seeded_sha256,
        Some(PINNED_SHA256.to_string())
    );
    // camelCase on the wire.
    assert!(read(&state_path(dir.path())).contains("lastSeededSha256"));
}

#[test]
fn untouched_entry_is_refreshed_when_default_changes() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    ensure_injected_messages(dir.path(), &warnings).expect("seed");
    let before = read(&main_path(dir.path()));

    let specs = next_specs();
    let report = provision(dir.path(), &specs, None, &warnings).expect("refresh");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::Refreshed)
    );

    let after = read(&main_path(dir.path()));
    assert_ne!(before, after);
    assert!(after.contains(NEXT_CONTEXT_ALERT_TEMPLATE));
    assert!(!after.contains(DEFAULT_CONTEXT_ALERT_TEMPLATE));
    assert_eq!(
        load_registry_from_dir(dir.path(), &warnings).get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&NEXT_CONTEXT_ALERT_TEMPLATE.to_string())
    );

    let state: InjectedMessagesState =
        serde_json::from_str(&read(&state_path(dir.path()))).expect("sidecar");
    assert_eq!(
        state.messages[CONTEXT_ALERT_MESSAGE_ID].last_seeded_sha256,
        Some(sha256_hex(NEXT_CONTEXT_ALERT_TEMPLATE.as_bytes()))
    );

    // And it settles: a second run with the same specs changes nothing.
    let report = provision(dir.path(), &specs, None, &warnings).expect("settle");
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::AlreadyCurrent)
    );
    assert_eq!(read(&main_path(dir.path())), after);
}

#[test]
fn edited_entry_is_never_clobbered() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    ensure_injected_messages(dir.path(), &warnings).expect("seed");
    let edited = read(&main_path(dir.path()))
        .replace(DEFAULT_CONTEXT_ALERT_TEMPLATE, "MY OWN WORDING %MEMBER%");
    std::fs::write(main_path(dir.path()), &edited).expect("edit");

    let specs = next_specs();
    for _ in 0..2 {
        let report = provision(dir.path(), &specs, None, &warnings).expect("provision");
        assert_eq!(report.writer_disabled, None);
        assert_eq!(
            report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
            Some(&EntryOutcome::PreservedUserEdit)
        );
        assert_eq!(read(&main_path(dir.path())), edited);
    }
    assert_eq!(
        load_registry_from_dir(dir.path(), &warnings).get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&"MY OWN WORDING %MEMBER%".to_string())
    );
    let state: InjectedMessagesState =
        serde_json::from_str(&read(&state_path(dir.path()))).expect("sidecar");
    assert_eq!(
        state.messages[CONTEXT_ALERT_MESSAGE_ID].last_observed_sha256,
        Some(sha256_hex(b"MY OWN WORDING %MEMBER%"))
    );
}

#[test]
fn surgical_write_preserves_comments_and_unknown_keys() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    let source = format!(
        "# lead comment\nunknown_top = 7\nschema_version = 1\n\n[messages.zzz-other]\ntemplate = \
         '''\nzzz\n'''\n\n[messages.context-alert]\n# doc comment\nunknown_inside = \
         true\ntemplate = '''\n{}\n'''\n# trailing comment\n",
        DEFAULT_CONTEXT_ALERT_TEMPLATE
    );
    std::fs::write(main_path(dir.path()), &source).expect("write");

    // Reconstruct the expectation from the span itself, which is exactly
    // what the writer does. No absolute offset is asserted.
    let doc: WriteDoc = toml::from_str(&source).expect("write parse");
    let span = doc.messages[CONTEXT_ALERT_MESSAGE_ID]
        .template
        .as_ref()
        .expect("span")
        .span();
    assert!(source[span.clone()].starts_with("'''"));
    assert!(source[span.clone()].ends_with("'''"));
    let expected = format!(
        "{}{}{}",
        &source[..span.start],
        quote_literal(NEXT_CONTEXT_ALERT_TEMPLATE, false),
        &source[span.end..]
    );

    let specs = next_specs();
    let report = provision(dir.path(), &specs, None, &warnings).expect("refresh");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(read(&main_path(dir.path())), expected);

    let after = read(&main_path(dir.path()));
    assert!(after.contains("# lead comment"));
    assert!(after.contains("unknown_top = 7"));
    assert!(after.contains("# doc comment"));
    assert!(after.contains("unknown_inside = true"));
    assert!(after.contains("# trailing comment"));
    assert!(after.contains("[messages.zzz-other]"));
    assert!(after.contains("zzz"));
}

#[test]
fn append_is_idempotent_and_preserves_existing_bytes() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    let source = format!(
        "# operator comment\nschema_version = 1\n\n[messages.context-alert]\ntemplate = \
         '''\n{}\n'''\n# a trailing comment with no newline after it",
        DEFAULT_CONTEXT_ALERT_TEMPLATE
    );
    std::fs::write(main_path(dir.path()), &source).expect("write");

    let specs = two_specs();
    let report = provision(dir.path(), &specs, None, &warnings).expect("append");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(
        report.outcomes.get(EXTRA_MESSAGE_ID),
        Some(&EntryOutcome::Appended)
    );
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::AlreadyCurrent)
    );

    let after = read(&main_path(dir.path()));
    // The original bytes are a prefix: nothing before the append moved, and
    // the trailing comment did not swallow the appended block.
    assert!(after.starts_with(&source));
    assert!(after.contains(&format!("[messages.{}]", EXTRA_MESSAGE_ID)));
    assert!(after.contains(EXTRA_TEMPLATE));

    let report = provision(dir.path(), &specs, None, &warnings).expect("second run");
    assert_eq!(
        report.outcomes.get(EXTRA_MESSAGE_ID),
        Some(&EntryOutcome::AlreadyCurrent)
    );
    assert_eq!(read(&main_path(dir.path())), after);
}

#[test]
fn malformed_toml_preserves_bytes_and_disables_writer() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    let source = "schema_version = 1\n[messages.context-alert\ntemplate = 'oops\n";
    std::fs::write(main_path(dir.path()), source).expect("write");

    let report = ensure_injected_messages(dir.path(), &warnings).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert_eq!(read(&main_path(dir.path())), source);
    assert!(load_registry_from_dir(dir.path(), &warnings).is_empty());
    assert_eq!(
        render(CONTEXT_ALERT_MESSAGE_ID, &alert_values(), &warnings),
        render_with_template(DEFAULT_CONTEXT_ALERT_TEMPLATE, &alert_values(), &warnings)
    );
}

#[test]
fn newer_schema_version_disables_writer_and_renders_defaults() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    let source =
        "schema_version = 2\ncoverage_version = 1\n\n[messages.context-alert]\ntemplate = \
        '''\nOPERATOR TEXT %MEMBER%\n'''\n";
    std::fs::write(main_path(dir.path()), source).expect("write");

    let report = ensure_injected_messages(dir.path(), &warnings).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert_eq!(read(&main_path(dir.path())), source);
    // Structure unknown: fall back to the built-in defaults.
    assert!(load_registry_from_dir(dir.path(), &warnings).is_empty());
}

#[test]
fn newer_coverage_version_keeps_rendering_from_file() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    let source = format!(
        "schema_version = 1\ncoverage_version = {}\n\n[messages.context-alert]\ntemplate = \
         '''\nOPERATOR TEXT %MEMBER%\n'''\n",
        NEWER_COVERAGE_VERSION
    );
    std::fs::write(main_path(dir.path()), &source).expect("write");

    let report = ensure_injected_messages(dir.path(), &warnings).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert_eq!(read(&main_path(dir.path())), source);
    // A downgrade must not discard the operator's text.
    assert_eq!(
        load_registry_from_dir(dir.path(), &warnings).get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&"OPERATOR TEXT %MEMBER%".to_string())
    );
}

#[test]
fn git_conflict_markers_disable_writer() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    // Half one: markers at top level, which is also a TOML parse error.
    let dir = tempdir();
    let top_level =
        "schema_version = 1\n<<<<<<< HEAD\nmine = 1\n=======\ntheirs = 2\n>>>>>>> other\n";
    std::fs::write(main_path(dir.path()), top_level).expect("write");
    let report = ensure_injected_messages(dir.path(), &warnings).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert_eq!(read(&main_path(dir.path())), top_level);

    // Half two: markers inside a ''' literal, which parses cleanly and is
    // the case the scan exists to catch.
    let dir = tempdir();
    let inside = "schema_version = 1\n\n[messages.context-alert]\ntemplate = '''\n<<<<<<< \
        HEAD\nmine\n=======\ntheirs\n>>>>>>> other\n'''\n";
    std::fs::write(main_path(dir.path()), inside).expect("write");
    assert!(toml::from_str::<toml::Value>(inside).is_ok());
    let report = ensure_injected_messages(dir.path(), &warnings).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert_eq!(read(&main_path(dir.path())), inside);
    assert!(load_registry_from_dir(dir.path(), &warnings).is_empty());
}

#[test]
fn unknown_message_id_and_unknown_key_are_preserved_not_fatal() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    let source =
        "schema_version = 1\nunknown_top = \"x\"\n\n[messages.not-a-known-id]\ntemplate = \
        '''\nsomething\n'''\n\n[messages.context-alert]\nunknown_inside = 1\ntemplate = \
        '''\nOPERATOR %MEMBER%\n'''\n";
    std::fs::write(main_path(dir.path()), source).expect("write");

    let report = ensure_injected_messages(dir.path(), &warnings).expect("provision");
    assert_eq!(report.writer_disabled, None);
    // The source carries no `coverage_version`, so the writer is enabled and
    // the four ids this binary knows and the file does not are appended. The
    // operator's bytes stay a prefix: nothing already written moves.
    let after = read(&main_path(dir.path()));
    assert!(after.starts_with(source));
    let tail = &after[source.len()..];
    assert_eq!(
        appended_headers(tail),
        expected_appended_headers(),
        "the appended tail holds exactly the four new ids, in KNOWN_MESSAGES order"
    );
    assert_eq!(
        load_registry_from_dir(dir.path(), &warnings).get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&"OPERATOR %MEMBER%".to_string())
    );
    for spec in KNOWN_MESSAGES.iter().skip(1) {
        assert_eq!(
            load_registry_from_dir(dir.path(), &warnings).get(spec.id),
            Some(&spec.default_template.to_string()),
            "{}",
            spec.id
        );
    }
    // The unknown id is preserved but never rendered.
    assert!(!load_registry_from_dir(dir.path(), &warnings).contains_key("not-a-known-id"));
}

#[test]
fn missing_or_corrupt_sidecar_uses_recognizer() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    // Recognized shipped default, no sidecar at all: refreshed.
    let dir = tempdir();
    ensure_injected_messages(dir.path(), &warnings).expect("seed");
    std::fs::remove_file(state_path(dir.path())).expect("drop sidecar");
    let specs = next_specs();
    let report = provision(dir.path(), &specs, None, &warnings).expect("provision");
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::Refreshed)
    );
    assert!(read(&main_path(dir.path())).contains(NEXT_CONTEXT_ALERT_TEMPLATE));

    // Unrecognized content, corrupt sidecar: preserved.
    let dir = tempdir();
    ensure_injected_messages(dir.path(), &warnings).expect("seed");
    let edited =
        read(&main_path(dir.path())).replace(DEFAULT_CONTEXT_ALERT_TEMPLATE, "hand written");
    std::fs::write(main_path(dir.path()), &edited).expect("edit");
    std::fs::write(state_path(dir.path()), "{ not json").expect("corrupt");
    let report = provision(dir.path(), &specs, None, &warnings).expect("provision");
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::PreservedUserEdit)
    );
    assert_eq!(read(&main_path(dir.path())), edited);
}

#[test]
fn empty_registry_renders_embedded_default() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    // Rule 2 of section 9.3: under cfg(test) the process registry is empty.
    assert!(registry(&warnings).is_empty());
    assert_eq!(
        render(CONTEXT_ALERT_MESSAGE_ID, &alert_values(), &warnings),
        "[AC context alert] `dev-rust` in `wg-2-dev-team` reached threshold(s): 50%, 75%, 90%. No \
         action taken; you decide any follow-up."
    );
    // An id this binary does not know renders nothing at all.
    assert_eq!(render("no-such-message", &alert_values(), &warnings), "");
}

#[test]
fn reference_companion_is_regenerated_and_overwritten() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    ensure_injected_messages(dir.path(), &warnings).expect("seed");
    let canonical = read(&reference_path(dir.path()));
    assert_eq!(canonical, canonical_reference_bytes(&KNOWN_MESSAGES));
    assert!(canonical.contains("DO NOT EDIT"));
    assert!(canonical.contains(DEFAULT_CONTEXT_ALERT_TEMPLATE));
    assert!(canonical.contains(TOKEN_OBSERVED));

    let main_before = read(&main_path(dir.path()));
    std::fs::write(reference_path(dir.path()), "hand edited\n").expect("edit companion");
    ensure_injected_messages(dir.path(), &warnings).expect("regenerate");
    assert_eq!(read(&reference_path(dir.path())), canonical);
    assert_eq!(read(&main_path(dir.path())), main_before);
}

#[test]
fn reseed_id_and_all_reset_with_backup() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    ensure_injected_messages(dir.path(), &warnings).expect("seed");
    let seeded = read(&main_path(dir.path()));
    let edited = format!(
        "{}\n# an operator comment kept across reseed\nunknown_top = 3\n",
        seeded.replace(DEFAULT_CONTEXT_ALERT_TEMPLATE, "MY WORDING")
    );
    std::fs::write(main_path(dir.path()), &edited).expect("edit");

    let reset = reseed(
        dir.path(),
        ReseedTarget::Id(CONTEXT_ALERT_MESSAGE_ID.to_string()),
        &warnings,
    )
    .expect("reseed --id");
    assert_eq!(reset, vec![CONTEXT_ALERT_MESSAGE_ID.to_string()]);
    let after = read(&main_path(dir.path()));
    assert!(after.contains(DEFAULT_CONTEXT_ALERT_TEMPLATE));
    assert!(!after.contains("MY WORDING"));
    assert!(after.contains("# an operator comment kept across reseed"));
    assert!(after.contains("unknown_top = 3"));

    // The backup is located by globbing, never by reconstructing the name.
    let backups: Vec<PathBuf> = std::fs::read_dir(dir.path())
        .expect("read dir")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".bak-"))
        })
        .collect();
    assert_eq!(
        backups.len(),
        1,
        "expected exactly one backup: {:?}",
        backups
    );
    assert_eq!(read(&backups[0]), edited);

    let state: InjectedMessagesState =
        serde_json::from_str(&read(&state_path(dir.path()))).expect("sidecar");
    assert_eq!(
        state.messages[CONTEXT_ALERT_MESSAGE_ID].last_seeded_sha256,
        Some(PINNED_SHA256.to_string())
    );

    // --all goes through the same surgical writer.
    std::fs::write(
        main_path(dir.path()),
        after.replace(DEFAULT_CONTEXT_ALERT_TEMPLATE, "EDITED AGAIN"),
    )
    .expect("edit");
    let reset = reseed(dir.path(), ReseedTarget::All, &warnings).expect("reseed --all");
    // Every covered id is reset, in the sorted order `reseed` reports.
    let mut all_ids: Vec<String> = KNOWN_MESSAGES
        .iter()
        .map(|spec| spec.id.to_string())
        .collect();
    all_ids.sort();
    assert_eq!(reset, all_ids);
    let after_all = read(&main_path(dir.path()));
    assert!(after_all.contains(DEFAULT_CONTEXT_ALERT_TEMPLATE));
    assert!(after_all.contains("# an operator comment kept across reseed"));
    assert!(after_all.contains("unknown_top = 3"));

    // An unknown id exits non-zero and changes nothing.
    let before = read(&main_path(dir.path()));
    let err = reseed(
        dir.path(),
        ReseedTarget::Id("no-such-id".to_string()),
        &warnings,
    )
    .expect_err("unknown id must fail");
    assert!(err.contains("no-such-id"));
    assert!(err.contains(CONTEXT_ALERT_MESSAGE_ID));
    assert_eq!(read(&main_path(dir.path())), before);
}

#[test]
fn seeded_file_round_trips_to_pinned_default_without_extra_newline() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    ensure_injected_messages(dir.path(), &warnings).expect("seed");
    let source = read(&main_path(dir.path()));

    let root: toml::Value = toml::from_str(&source).expect("parse");
    let raw = root["messages"][CONTEXT_ALERT_MESSAGE_ID]["template"]
        .as_str()
        .expect("string");
    // TOML trims the newline after the opening ''' but not the one before
    // the closing ''', so the raw value carries exactly one.
    assert!(raw.ends_with('\n'));
    assert_eq!(raw, format!("{}\n", DEFAULT_CONTEXT_ALERT_TEMPLATE));
    assert_eq!(
        normalize_parsed_template(raw),
        DEFAULT_CONTEXT_ALERT_TEMPLATE
    );

    // Without normalization the frame would be \n...\n\n\r.
    let rendered =
        render_with_template(&normalize_parsed_template(raw), &alert_values(), &warnings);
    assert_eq!(
        format!("\n{}\n\r", rendered),
        "\n[AC context alert] `dev-rust` in `wg-2-dev-team` reached threshold(s): 50%, 75%, 90%. \
         No action taken; you decide any follow-up.\n\r"
    );
}
