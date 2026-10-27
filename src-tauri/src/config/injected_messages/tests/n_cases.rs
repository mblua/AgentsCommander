use super::*;

// ---- N1..N16 ---------------------------------------------------------

#[test]
fn crlf_edit_is_not_a_content_edit() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    ensure_injected_messages(dir.path(), &warnings).expect("seed");
    let lf = read(&main_path(dir.path()));
    let crlf = lf.replace('\n', "\r\n");
    std::fs::write(main_path(dir.path()), &crlf).expect("save as CRLF");

    let lf_model = analyze_source(&lf);
    let crlf_model = analyze_source(&crlf);
    let lf_value = lf_model.templates[CONTEXT_ALERT_MESSAGE_ID].clone();
    let crlf_value = crlf_model.templates[CONTEXT_ALERT_MESSAGE_ID].clone();
    assert_eq!(lf_value, crlf_value);
    assert_eq!(
        sha256_hex(lf_value.as_bytes()),
        sha256_hex(crlf_value.as_bytes())
    );

    // A line-ending change must not be read as an edit, and a rewrite must
    // not create a mixed-ending file.
    let specs = next_specs();
    let report = provision(dir.path(), &specs, None, &warnings).expect("provision");
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::Refreshed)
    );
    let after = read(&main_path(dir.path()));
    assert!(after.contains("'''\r\n"));
    assert_eq!(after.matches('\n').count(), after.matches("\r\n").count());
    assert_eq!(
        analyze_source(&after).templates[CONTEXT_ALERT_MESSAGE_ID],
        NEXT_CONTEXT_ALERT_TEMPLATE
    );
}

#[test]
fn entry_without_template_or_with_wrong_type_degrades_only_that_entry() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    let source = format!(
        "schema_version = 1\n\n[messages.context-alert]\n# the operator deleted the template \
         line\n\n[messages.{}]\ntemplate = 42\n\n[messages.healthy]\ntemplate = \
         '''\nHEALTHY\n'''\n",
        EXTRA_MESSAGE_ID
    );
    std::fs::write(main_path(dir.path()), &source).expect("write");

    let model = analyze_source(&source);
    assert_eq!(model.strict_invalid, None);
    assert_eq!(
        model.entries.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryValue::Malformed)
    );
    assert_eq!(
        model.entries.get(EXTRA_MESSAGE_ID),
        Some(&EntryValue::Malformed)
    );
    assert_eq!(
        model.entries.get("healthy"),
        Some(&EntryValue::Template("HEALTHY".to_string()))
    );

    let specs = two_specs();
    let report = provision(dir.path(), &specs, None, &warnings).expect("provision");
    assert_eq!(
        report.writer_disabled, None,
        "one bad entry must not disable the file"
    );
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::PreservedUserEdit)
    );
    assert_eq!(read(&main_path(dir.path())), source);
    // A table-valued `template` reports a span over the whole header and
    // body, so "present but not a string" must never become a rewrite.
    let dir = tempdir();
    let table_valued =
        "schema_version = 1\n\n[messages.context-alert.template]\nk = 1\n".to_string();
    std::fs::write(main_path(dir.path()), &table_valued).expect("write");
    let report = ensure_injected_messages(dir.path(), &warnings).expect("provision");
    assert_eq!(report.writer_disabled, None);
    // The unreadable entry stays byte-identical and the four missing ids are
    // appended after it.
    let after = read(&main_path(dir.path()));
    assert!(after.starts_with(&table_valued));
    assert_eq!(
        appended_headers(&after[table_valued.len()..]),
        expected_appended_headers()
    );
}

#[test]
fn every_shipped_default_is_literal_quotable() {
    for spec in &KNOWN_MESSAGES {
        assert!(
            is_literal_quotable(spec.default_template),
            "`{}` cannot be written as a TOML literal",
            spec.id
        );
    }
    assert!(!is_literal_quotable("has ''' inside"));
    assert!(!is_literal_quotable("has \r inside"));
    assert!(!is_literal_quotable("has \u{0}} inside"));
    assert!(is_literal_quotable("keeps \n and \t"));
}

#[test]
fn writer_emit_parse_normalize_is_identity() {
    let fixtures = [
        DEFAULT_CONTEXT_ALERT_TEMPLATE,
        "# Heading\n\n**bold**\n\n```rust\nfn main() {}\n```\n\n- a\n\n- b",
        "\nstarts with a newline",
        "ends with a newline\n",
        "interior\n\n\nblank lines",
        "trailing spaces   \nand\ttabs\t",
        "emoji 🚨 and acentos: contexto crítico",
        "ends in a quote '",
        "ends in two quotes ''",
        "%MEMBER% %UNKNOWN% 100%",
    ];
    for fixture in fixtures {
        assert!(
            is_literal_quotable(fixture),
            "fixture not quotable: {:?}",
            fixture
        );
        for crlf in [false, true] {
            let emitted = quote_literal(fixture, crlf);
            let source = format!("[messages.x]\ntemplate = {}\n", emitted);
            let root: toml::Value = toml::from_str(&source)
                .unwrap_or_else(|e| panic!("emit did not parse for {:?}: {}", fixture, e));
            let parsed = root["messages"]["x"]["template"].as_str().expect("string");
            assert_eq!(
                normalize_parsed_template(parsed),
                fixture,
                "round trip failed for {:?} (crlf={})",
                fixture,
                crlf
            );
        }
    }
}

#[test]
fn duplicate_message_header_is_strict_invalid() {
    let source = "schema_version = 1\n\n[messages.context-alert]\ntemplate = \
        'a'\n\n[messages.context-alert]\ntemplate = 'b'\n";
    let model = analyze_source(source);
    assert!(
        model.strict_invalid.is_some(),
        "a duplicate header must never become a silent last-wins"
    );
}

#[test]
fn append_into_inline_table_file_publishes_nothing() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    let source = "schema_version = 1\nmessages = { \"context-alert\" = { template = \"X\" } }\n";
    std::fs::write(main_path(dir.path()), source).expect("write");

    let specs = two_specs();
    let report = provision(dir.path(), &specs, None, &warnings).expect("provision");
    assert!(
        report.writer_disabled.is_some(),
        "an append that would invalidate the file must disable the writer"
    );
    assert_eq!(read(&main_path(dir.path())), source);
    assert!(!report.outcomes.contains_key(EXTRA_MESSAGE_ID));
}

#[test]
fn writer_never_publishes_unparseable_text() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    // The refresh path: a default that cannot be written as a TOML literal
    // is refused before anything reaches disk.
    const UNQUOTABLE: &str = "a ''' inside the value";
    let unquotable_specs = [MessageSpec {
        id: CONTEXT_ALERT_MESSAGE_ID,
        default_template: UNQUOTABLE,
        known_default_sha256: &[PINNED_SHA256],
        tokens: &[TOKEN_MEMBER],
        doc_comment: CONTEXT_ALERT_DOC_COMMENT,
    }];
    let dir = tempdir();
    ensure_injected_messages(dir.path(), &warnings).expect("seed");
    let seeded = read(&main_path(dir.path()));
    let report = provision(dir.path(), &unquotable_specs, None, &warnings).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert_eq!(read(&main_path(dir.path())), seeded);

    // The append path: a source whose `messages` is an inline table cannot
    // be extended, and the candidate is discarded on re-parse.
    let dir = tempdir();
    let source = "schema_version = 1\nmessages = { other = { template = \"X\" } }\n";
    std::fs::write(main_path(dir.path()), source).expect("write");
    let report = ensure_injected_messages(dir.path(), &warnings).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert_eq!(read(&main_path(dir.path())), source);

    // And the gate itself, exercised directly.
    let previous = BTreeSet::from([CONTEXT_ALERT_MESSAGE_ID.to_string()]);
    let intended = BTreeMap::from([(CONTEXT_ALERT_MESSAGE_ID.to_string(), "value".to_string())]);
    assert!(verify_candidate("[messages.context-alert\n", &previous, &intended).is_err());
    assert!(verify_candidate("schema_version = 1\n", &previous, &intended).is_err());
    assert!(verify_candidate(
        "[messages.context-alert]\ntemplate = 'other'\n",
        &previous,
        &intended
    )
    .is_err());
    assert!(verify_candidate(
        "[messages.context-alert]\ntemplate = 'value'\n",
        &previous,
        &intended
    )
    .is_ok());
}

#[test]
fn bom_file_round_trips() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    ensure_injected_messages(dir.path(), &warnings).expect("seed");
    let seeded = read(&main_path(dir.path()));
    let with_bom = format!("\u{feff}{}", seeded);
    std::fs::write(main_path(dir.path()), &with_bom).expect("write BOM");

    let model = analyze_source(&with_bom);
    assert_eq!(model.strict_invalid, None);
    assert_eq!(
        model.templates.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&DEFAULT_CONTEXT_ALERT_TEMPLATE.to_string()),
        "the BOM must be invisible to hashing and reconciliation"
    );

    // A refresh still targets the right span, and the BOM survives.
    let specs = next_specs();
    let report = provision(dir.path(), &specs, None, &warnings).expect("provision");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::Refreshed)
    );
    let after = read(&main_path(dir.path()));
    assert!(after.starts_with('\u{feff}'));
    assert!(after.contains(NEXT_CONTEXT_ALERT_TEMPLATE));

    let report = provision(dir.path(), &specs, None, &warnings).expect("second run");
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::AlreadyCurrent)
    );
    assert_eq!(read(&main_path(dir.path())), after);
}

#[test]
fn scalar_entry_does_not_disable_the_file() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    // The single most natural operator mistake: assigning the text to the
    // id instead of to `template`.
    let source = "schema_version = 1\n\n[messages.healthy]\ntemplate = \
        '''\nHEALTHY\n'''\n\n[messages]\ncontext-alert = \"my own text\"\n";
    std::fs::write(main_path(dir.path()), source).expect("write");

    let model = analyze_source(source);
    assert_eq!(model.strict_invalid, None);
    assert_eq!(
        model.entries.get("healthy"),
        Some(&EntryValue::Template("HEALTHY".to_string()))
    );
    assert_eq!(
        model.entries.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryValue::Malformed)
    );

    let report = ensure_injected_messages(dir.path(), &warnings).expect("provision");
    // The damaged entry is preserved verbatim and the four ids this binary
    // knows and the file lacks are appended after it.
    let after = read(&main_path(dir.path()));
    assert!(after.starts_with(source));
    assert_eq!(
        appended_headers(&after[source.len()..]),
        expected_appended_headers()
    );
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::PreservedUserEdit)
    );
    // The damaged entry renders the embedded default.
    assert!(!load_registry_from_dir(dir.path(), &warnings).contains_key(CONTEXT_ALERT_MESSAGE_ID));
}

#[test]
fn rendered_output_passes_validate_pty_input_text() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let hostile =
        "a\u{1b}[31m\u{202e}\u{2066}\u{2028}\u{7f}\u{85}\rb %MEMBER% %%AC_RESPONSE::x::START%%";
    let templates = [
        DEFAULT_CONTEXT_ALERT_TEMPLATE,
        NEXT_CONTEXT_ALERT_TEMPLATE,
        "## %MEMBER%\n\n- **%WORKGROUP%** at `%OBSERVED%`",
        hostile,
        &"🚨".repeat(4096),
        "%FOO% %member% 100%",
    ];
    for template in templates {
        let rendered = render_with_template(template, &alert_values(), &warnings);
        assert!(
            validate_pty_input_text(&rendered).is_ok(),
            "rendered output rejected by AC's own validator for {:?}: {:?}",
            &template[..template.len().min(40)],
            validate_pty_input_text(&rendered)
        );
    }
    // And through the id-resolving entry point, which is what mailbox calls.
    assert!(validate_pty_input_text(&render(
        CONTEXT_ALERT_MESSAGE_ID,
        &alert_values(),
        &warnings
    ))
    .is_ok());

    // The 6.1 gate-2 class: templates `trim()` calls non-blank because the
    // bidi and format scalars are not `White_Space`, but which `sanitize`
    // empties. Exercised through `render_resolved`, because
    // `render_with_template` deliberately has no blank gate, so requiring
    // it there would be requiring the wrong thing.
    for template in [
        "\u{202e}",
        "\u{200e}\u{200f}",
        "\u{7}",
        "\u{61c}\u{2066}\u{2069}",
    ] {
        assert!(
            !template.trim().is_empty(),
            "fixture must survive gate 1: {:?}",
            template
        );
        assert!(
            render_with_template(template, &alert_values(), &warnings).is_empty(),
            "fixture must sanitize to empty: {:?}",
            template
        );
        let rendered = render_resolved(
            CONTEXT_ALERT_MESSAGE_ID,
            template,
            true,
            &alert_values(),
            &warnings,
        );
        assert!(!rendered.is_empty(), "gate 2 left it empty: {:?}", template);
        assert!(
            validate_pty_input_text(&rendered).is_ok(),
            "gate-2 fallback rejected by AC's own validator for {:?}",
            template
        );
    }
}

#[test]
fn setext_heading_is_not_a_conflict_marker() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    let source = "schema_version = 1\n\n[messages.context-alert]\ntemplate = \
        '''\nAlerta\n=======\ncuerpo del mensaje\n'''\n";
    std::fs::write(main_path(dir.path()), source).expect("write");

    assert!(!has_conflict_markers(source));
    let report = ensure_injected_messages(dir.path(), &warnings).expect("provision");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(
        load_registry_from_dir(dir.path(), &warnings).get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&"Alerta\n=======\ncuerpo del mensaje".to_string())
    );
    // The paired rule still catches a real conflict.
    assert!(has_conflict_markers(
        "<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> other\n"
    ));
}

#[test]
fn second_provisioning_run_writes_nothing() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let _serialized = COUNTING_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempdir();
    ensure_injected_messages(dir.path(), &warnings).expect("seed");

    start_counting();
    ensure_injected_messages(dir.path(), &warnings).expect("second run");
    let writes = stop_counting(dir.path());
    assert!(
        writes.is_empty(),
        "an unchanged second run must write nothing, wrote: {:?}",
        writes
    );

    // A first run does write all three, which is what makes the assertion
    // above meaningful.
    let dir = tempdir();
    start_counting();
    ensure_injected_messages(dir.path(), &warnings).expect("first run");
    let writes = stop_counting(dir.path());
    assert_eq!(
        writes.len(),
        3,
        "expected file, sidecar and companion: {:?}",
        writes
    );
}

#[test]
fn sidecar_write_failure_does_not_freeze_entry() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    ensure_injected_messages(dir.path(), &warnings).expect("seed");
    // Force every later sidecar write to fail by occupying its path with a
    // directory. The main file still publishes.
    std::fs::remove_file(state_path(dir.path())).expect("drop sidecar");
    std::fs::create_dir(state_path(dir.path())).expect("block sidecar");

    let specs = next_specs();
    let report = provision(dir.path(), &specs, None, &warnings).expect("provision");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::Refreshed)
    );
    assert!(read(&main_path(dir.path())).contains(NEXT_CONTEXT_ALERT_TEMPLATE));

    // With the sidecar still unwritable, the next run converges through the
    // recognizer rather than freezing the entry as a user edit. The pass
    // must ship a THIRD default that lists the on-disk content among its
    // known ones: reusing `specs` would land in `AlreadyCurrent`, which
    // passes even when no recognizer exists.
    assert_eq!(
        sha256_hex(NEXT_CONTEXT_ALERT_TEMPLATE.as_bytes()),
        NEXT_SHA256,
        "the pinned hash of the second default drifted"
    );
    let third = third_specs();
    assert!(
        third[0].known_default_sha256.contains(&NEXT_SHA256),
        "the recognizer must list the on-disk content"
    );
    let report = provision(dir.path(), &third, None, &warnings).expect("second run");
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::Refreshed),
        "recognized content one version behind must refresh, not freeze"
    );
    assert!(read(&main_path(dir.path())).contains(THIRD_CONTEXT_ALERT_TEMPLATE));

    let report = provision(dir.path(), &KNOWN_MESSAGES, None, &warnings).expect("downgrade run");
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::PreservedUserEdit),
        "an unrecognized hash is still preserved"
    );
}

/// N17. What makes the single retry in gate 2 total: the fallback can never
/// itself render empty, so no second failure is reachable and the gate
/// needs no loop.
#[test]
fn every_shipped_default_renders_non_empty() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    for spec in KNOWN_MESSAGES.iter() {
        let rendered = render_with_template(spec.default_template, &alert_values(), &warnings);
        assert!(!rendered.is_empty(), "`{}` rendered empty", spec.id);
        assert!(
            validate_pty_input_text(&rendered).is_ok(),
            "`{}` rejected by AC's own validator: {:?}",
            spec.id,
            validate_pty_input_text(&rendered)
        );
    }
}

/// N18. The gate itself, on the pure seam, with no global registry involved.
#[test]
fn template_that_sanitizes_to_empty_falls_back_to_default() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let values = alert_values();
    let expected = render_with_template(DEFAULT_CONTEXT_ALERT_TEMPLATE, &values, &warnings);

    let rendered = render_resolved(
        CONTEXT_ALERT_MESSAGE_ID,
        "\u{202e}",
        true,
        &values,
        &warnings,
    );
    assert_eq!(rendered, expected);
    assert!(!rendered.is_empty());

    // It warned: the key is registered afterwards, whatever the test order.
    assert!(
        !warn_once(
            format!("empty:{}", CONTEXT_ALERT_MESSAGE_ID),
            "probe, never emitted",
            &warnings
        ),
        "gate 2 must register its warn-once key"
    );

    // With `from_registry = false` the same input returns "". The gate is
    // deliberately not applied to an embedded default: a shipped default
    // that sanitizes to nothing is a bug to be caught by N17, not a
    // condition to paper over at runtime.
    assert_eq!(
        render_resolved(
            CONTEXT_ALERT_MESSAGE_ID,
            "\u{202e}",
            false,
            &values,
            &warnings
        ),
        ""
    );
}

/// N19. `warn_once`'s return value is the only observable the mechanism has,
/// and the log is this feature's only channel to the operator.
///
/// Every key and token below is unique to this test, because the set is
/// process-global and shared across the whole test binary.
#[test]
fn warn_once_fires_once_per_key() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    assert!(warn_once(
        "n19-unique-key-alpha".to_string(),
        "first",
        &warnings
    ));
    assert!(!warn_once(
        "n19-unique-key-alpha".to_string(),
        "second",
        &warnings
    ));
    assert!(warn_once(
        "n19-unique-key-beta".to_string(),
        "different key",
        &warnings
    ));

    // The key is the token, including its `%` delimiters, so the set cannot
    // grow with the number of templates.
    let rendered =
        render_with_template("%N19_ALPHA% and %N19_BETA% and %N19_ALPHA%", &[], &warnings);
    assert_eq!(rendered, "%N19_ALPHA% and %N19_BETA% and %N19_ALPHA%");
    for token in ["%N19_ALPHA%", "%N19_BETA%"] {
        assert!(
            !warn_once(
                format!("token:{}", token),
                "probe, never emitted",
                &warnings
            ),
            "{} did not register exactly once",
            token
        );
    }
}

/// N20. The Windows reparse-point branch of `is_link_or_reparse`, exercised
/// with a junction, which needs no privileges.
///
/// Recorded residue: this pins the class "a real reparse point occupies the
/// path" and that `is_link_or_reparse` rejects it. It does not pin WHICH of
/// that function's two branches fires, since whether `std` classifies a
/// mount point as a symlink is not asserted here either way.
#[cfg(windows)]
#[test]
fn reparse_point_in_the_path_is_rejected() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    let dir = tempdir();
    let target = dir.path().join("junction-target");
    std::fs::create_dir(&target).expect("junction target");
    let link = main_path(dir.path());

    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .output()
        .expect("run mklink");
    // Failing rather than skipping, so the coverage cannot silently
    // evaporate on a machine where the junction cannot be created.
    assert!(
        status.status.success(),
        "mklink /J failed: {}{}",
        String::from_utf8_lossy(&status.stdout),
        String::from_utf8_lossy(&status.stderr)
    );

    let metadata = std::fs::symlink_metadata(&link).expect("symlink_metadata");
    assert!(
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0,
        "the path does not carry FILE_ATTRIBUTE_REPARSE_POINT"
    );
    assert!(is_link_or_reparse(&metadata));

    let report = ensure_injected_messages(dir.path(), &warnings).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert!(load_registry_from_dir(dir.path(), &warnings).is_empty());

    // Neither followed nor replaced.
    let after = std::fs::symlink_metadata(&link).expect("still there");
    assert!(is_link_or_reparse(&after));
    assert!(target.is_dir());
    assert!(
        std::fs::read_dir(&target)
            .expect("read target")
            .next()
            .is_none(),
        "nothing was written through the junction"
    );
}

#[test]
fn absent_version_keys_are_treated_as_current_and_never_written_back() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    let source = format!(
        "[messages.context-alert]\ntemplate = '''\n{}\n'''\n",
        DEFAULT_CONTEXT_ALERT_TEMPLATE
    );
    std::fs::write(main_path(dir.path()), &source).expect("write");

    let report = ensure_injected_messages(dir.path(), &warnings).expect("provision");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::AlreadyCurrent)
    );
    // Absent version keys are never written back; the four missing ids are
    // appended after the operator's own bytes, which stay a prefix.
    let after = read(&main_path(dir.path()));
    assert!(after.starts_with(&source));
    assert!(!after.contains("schema_version = "));
    assert!(!after.contains("coverage_version = "));
    assert_eq!(
        appended_headers(&after[source.len()..]),
        expected_appended_headers()
    );

    // A non-integer version key is strict-invalid, unlike an absent one.
    assert!(analyze_source("schema_version = \"1\"\n")
        .strict_invalid
        .is_some());
    assert!(analyze_source("coverage_version = true\n")
        .strict_invalid
        .is_some());
}

#[test]
fn known_messages_has_five_entries_with_unique_ids() {
    assert_eq!(KNOWN_MESSAGES.len(), 5);
    let ids: Vec<&str> = KNOWN_MESSAGES.iter().map(|spec| spec.id).collect();
    let unique: std::collections::BTreeSet<&str> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "duplicate ids: {:?}", ids);
    assert_eq!(
        ids,
        vec![
            CONTEXT_ALERT_MESSAGE_ID,
            CI_STARTED_MESSAGE_ID,
            CI_FINISHED_MESSAGE_ID,
            BRANCH_STALE_MESSAGE_ID,
            NOTICE_BLIND_GAP_MESSAGE_ID,
        ]
    );
    // Every documented token is a token the spec declares, which is what
    // keeps the generated `#   %TOKEN%` lines honest.
    for spec in &KNOWN_MESSAGES {
        assert!(!spec.default_template.is_empty(), "{}", spec.id);
        assert!(!spec.tokens.is_empty(), "{}", spec.id);
        for token in spec.tokens {
            assert!(
                spec.doc_comment.contains(token),
                "{} documents every token; {} is missing",
                spec.id,
                token
            );
        }
    }
}

/// #2141 replaced the older "exactly one digest" rule. That rule encoded "no
/// shipped default has moved yet", which stopped being true when the
/// branch-stale wording changed; the rule worth keeping is that the list ENDS
/// at what ships today, so reconciliation still recognizes every default ever
/// shipped and refreshes a pristine entry to the current one.
#[test]
fn each_default_digest_list_ends_at_the_current_template() {
    for spec in &KNOWN_MESSAGES {
        assert!(
            !spec.known_default_sha256.is_empty(),
            "`{}` must list at least the digest it ships today",
            spec.id
        );
        assert_eq!(
            spec.known_default_sha256.last().copied(),
            Some(sha256_hex(spec.default_template.as_bytes()).as_str()),
            "`{}`: the LAST digest must be the shipped default's, newest last",
            spec.id
        );
        let mut seen = std::collections::HashSet::new();
        for digest in spec.known_default_sha256 {
            assert!(
                seen.insert(*digest),
                "`{}` lists {} twice; every entry is a distinct shipped default",
                spec.id,
                digest
            );
        }
    }
}

#[test]
fn canonical_seed_bytes_and_reference_bytes_regenerated() {
    // Both pin transcribed bytes, read by a human off the regenerated files
    // and checked against the specification: a comparison against
    // `canonical_*_bytes` alone cannot fail, so a header or ordering edit
    // would drift from the spec with nothing going red.
    assert_eq!(EXPECTED_SEED.len(), 3712, "the pinned seed is 3712 bytes");
    assert_eq!(canonical_seed_bytes(&KNOWN_MESSAGES), EXPECTED_SEED);
    assert_eq!(
        EXPECTED_REFERENCE.len(),
        3444,
        "the pinned companion is 3444 bytes"
    );
    assert_eq!(
        canonical_reference_bytes(&KNOWN_MESSAGES),
        EXPECTED_REFERENCE
    );
}

#[test]
fn coverage_version_is_two() {
    assert_eq!(COVERAGE_VERSION, 2);
    assert_eq!(KNOWN_MESSAGES.len(), 5);
}

#[test]
fn newer_coverage_version_literal_raised_to_three() {
    assert_eq!(NEWER_COVERAGE_VERSION, 3);
    // A const block, so the invariant is checked when it can still be false:
    // a runtime assertion over two constants can never fail and would quietly
    // stop meaning anything.
    const {
        assert!(
            NEWER_COVERAGE_VERSION > COVERAGE_VERSION,
            "the fixture must stay strictly above COVERAGE_VERSION: `writer_blocked` is gated on \
             a strict `>`, so equality leaves the writer enabled and the test stops exercising \
             the newer-than-this-binary branch"
        );
    }
}

#[test]
fn existing_operator_file_at_coverage_one_is_appended_not_rewritten() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let dir = tempdir();
    let source =
        "schema_version = 1\ncoverage_version = 1\n\n[messages.context-alert]\ntemplate = \
        '''\nOPERATOR %MEMBER% EDITED\n'''\n";
    std::fs::write(main_path(dir.path()), source).expect("write");

    let report = ensure_injected_messages(dir.path(), &warnings).expect("provision");
    assert_eq!(report.writer_disabled, None);
    assert_eq!(
        report.outcomes.get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&EntryOutcome::PreservedUserEdit)
    );
    for spec in KNOWN_MESSAGES.iter().skip(1) {
        assert_eq!(
            report.outcomes.get(spec.id),
            Some(&EntryOutcome::Appended),
            "{}",
            spec.id
        );
    }

    let after = read(&main_path(dir.path()));
    assert!(
        after.starts_with(source),
        "an operator edit at coverage_version 1 is appended to, never rewritten"
    );
    assert!(!after.contains(DEFAULT_CONTEXT_ALERT_TEMPLATE));
    assert_eq!(
        appended_headers(&after[source.len()..]),
        expected_appended_headers()
    );
    assert_eq!(
        load_registry_from_dir(dir.path(), &warnings).get(CONTEXT_ALERT_MESSAGE_ID),
        Some(&"OPERATOR %MEMBER% EDITED".to_string())
    );
    for spec in KNOWN_MESSAGES.iter().skip(1) {
        assert_eq!(
            load_registry_from_dir(dir.path(), &warnings).get(spec.id),
            Some(&spec.default_template.to_string()),
            "{}",
            spec.id
        );
    }
}

#[test]
fn unusable_path_preserves_state_and_keeps_rendering() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    // A directory where the file should be: preserved, writer disabled,
    // rendering unaffected, and never fatal.
    let dir = tempdir();
    std::fs::create_dir(main_path(dir.path())).expect("directory in the way");
    let report = ensure_injected_messages(dir.path(), &warnings).expect("provision");
    assert!(report.writer_disabled.is_some());
    assert!(main_path(dir.path()).is_dir());
    assert!(load_registry_from_dir(dir.path(), &warnings).is_empty());
    assert_eq!(
        render(CONTEXT_ALERT_MESSAGE_ID, &alert_values(), &warnings),
        render_with_template(DEFAULT_CONTEXT_ALERT_TEMPLATE, &alert_values(), &warnings)
    );
}
