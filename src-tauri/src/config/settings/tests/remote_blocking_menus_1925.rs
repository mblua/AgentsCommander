use super::*;
use serde_json::json;

/// One remote file with a single `byCommand` stem, shaped like the published file.
fn remote_json(stem: &str, entries: Value) -> String {
    let mut by_command = Map::new();
    by_command.insert(stem.to_string(), entries);
    json!({
        "schemaVersion": 1,
        "byCommand": Value::Object(by_command),
        "byAgent": {},
    })
    .to_string()
}

/// One raw entry as a remote file carries it.
fn entry(pattern: &str, notification: &str) -> Value {
    json!({
        "pattern": pattern,
        "notification": notification,
        "enabled": true,
    })
}

/// One raw entry with `enabled` false, for D6's disabled-entry clauses.
fn disabled_entry(pattern: &str, notification: &str) -> Value {
    json!({
        "pattern": pattern,
        "notification": notification,
        "enabled": false,
    })
}

/// The parsed twin of `entry`, for equality against store results.
fn parsed_entry(pattern: &str, notification: &str) -> BlockingMenuEntry {
    BlockingMenuEntry::Valid(BlockingMenuConfig {
        pattern: pattern.to_string(),
        notification: notification.to_string(),
        enabled: true,
        captured_against: None,
    })
}

fn write_remote(dir: &Path, contents: &str) {
    std::fs::write(dir.join(BLOCKING_MENUS_REMOTE_FILE_NAME), contents).unwrap();
}

fn layers(
    by_command: &[(&str, Vec<BlockingMenuEntry>)],
    by_agent: &[(&str, Vec<BlockingMenuEntry>)],
) -> BlockingMenusFile {
    BlockingMenusFile {
        by_command: by_command
            .iter()
            .map(|(stem, entries)| ((*stem).to_string(), entries.clone()))
            .collect(),
        by_agent: by_agent
            .iter()
            .map(|(id, entries)| ((*id).to_string(), entries.clone()))
            .collect(),
        ..Default::default()
    }
}

#[test]
fn remote_url_pins_the_schema_version_and_source_ref() {
    let segments: Vec<&str> = REMOTE_BLOCKING_MENUS_URL.split('/').collect();
    assert_eq!(segments.len(), 10);
    assert_eq!(
        segments[..5],
        [
            "https:",
            "",
            "raw.githubusercontent.com",
            "mblua",
            "AgentsCommander"
        ]
    );
    assert_eq!(segments[5], REMOTE_BLOCKING_MENUS_SOURCE_REF);
    assert_eq!(REMOTE_BLOCKING_MENUS_SOURCE_REF, "main");
    assert_eq!(segments[6..8], ["remote-resources", "blocking-menus"]);
    assert_eq!(segments[8], format!("v{BLOCKING_MENUS_SCHEMA_VERSION}"));
    assert_eq!(segments[9], "settings-blocking-menus.json");
}

#[test]
fn accept_response_checks_status_size_and_encoding() {
    assert_eq!(
        accept_remote_blocking_menus_response(404, &[]),
        Err("HTTP status 404".to_string())
    );
    assert_eq!(
        accept_remote_blocking_menus_response(500, &[]),
        Err("HTTP status 500".to_string())
    );

    let oversized = vec![b' '; REMOTE_BLOCKING_MENUS_MAX_BYTES + 1];
    let error = accept_remote_blocking_menus_response(200, &oversized)
        .expect_err("an oversized body is rejected");
    assert!(error.contains("larger than"), "got {error:?}");

    let error = accept_remote_blocking_menus_response(200, &[0xff, 0xfe])
        .expect_err("a non-UTF-8 body is rejected");
    assert!(error.contains("not UTF-8"), "got {error:?}");

    let published = include_str!(
        "../../../../../remote-resources/blocking-menus/v1/settings-blocking-menus.json"
    );
    assert!(accept_remote_blocking_menus_response(200, published.as_bytes()).is_ok());

    let mut padded = published.as_bytes().to_vec();
    padded.resize(REMOTE_BLOCKING_MENUS_MAX_BYTES, b' ');
    assert_eq!(padded.len(), REMOTE_BLOCKING_MENUS_MAX_BYTES);
    assert!(accept_remote_blocking_menus_response(200, &padded).is_ok());
}

#[test]
fn cache_writer_records_source_time_and_ref() {
    let temp = tempfile::TempDir::new().unwrap();
    let settings_path = temp.path().join("settings.json");
    let fetched_at = "2026-09-12T12:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let file = validate_remote_blocking_menus_file(&remote_json(
        "grok",
        json!([entry(r"^\s*Grok test menu\?", "grok is waiting")]),
    ))
    .expect("the fixture passes the validator");

    write_remote_blocking_menus_cache(&settings_path, file, REMOTE_BLOCKING_MENUS_URL, fetched_at)
        .expect("the cache write succeeds");

    let loaded = load_remote_blocking_menus_file(&settings_path);
    assert_eq!(
        loaded.by_command["grok"],
        vec![parsed_entry(r"^\s*Grok test menu\?", "grok is waiting")]
    );

    let text = std::fs::read_to_string(blocking_menus_remote_path(&settings_path)).unwrap();
    let note = serde_json::from_str::<Value>(&text).unwrap()["note"]
        .as_str()
        .expect("the written note is a string")
        .to_string();
    assert!(note.contains(REMOTE_BLOCKING_MENUS_URL), "note: {note}");
    assert!(note.contains("(source ref main)"), "note: {note}");
    assert!(note.contains("2026-09-12T12:00:00Z"), "note: {note}");
}

#[test]
fn check_stamp_round_trips_and_ignores_garbage() {
    let temp = tempfile::TempDir::new().unwrap();
    let settings_path = temp.path().join("settings.json");
    let at = "2026-09-12T12:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();

    assert_eq!(read_remote_blocking_menus_check_stamp(&settings_path), None);

    write_remote_blocking_menus_check_stamp(&settings_path, at).expect("the stamp writes");
    assert_eq!(
        read_remote_blocking_menus_check_stamp(&settings_path),
        Some(at)
    );

    std::fs::write(blocking_menus_remote_check_path(&settings_path), b"{").unwrap();
    assert_eq!(read_remote_blocking_menus_check_stamp(&settings_path), None);
}

#[test]
fn the_published_remote_file_passes_the_validator() {
    let raw = include_str!(
        "../../../../../remote-resources/blocking-menus/v1/settings-blocking-menus.json"
    );
    let file = validate_remote_blocking_menus_file(raw)
        .expect("the published remote file passes the validator");
    assert!(
        file.by_agent.is_empty(),
        "the published file carries no byAgent rows"
    );
}

#[test]
fn the_shipped_patterns_pass_the_validator() {
    validate_remote_blocking_menus_file(EMBEDDED_BLOCKING_MENUS_JSON)
        .expect("the shipped patterns pass the remote validator");
}

#[test]
fn every_rejection_names_its_check() {
    let pattern_513 = "a".repeat(513);
    let notification_201 = "n".repeat(201);
    let pattern_e_acute_257 = "é".repeat(257);
    let notification_e_acute_101 = "é".repeat(101);
    let entries_201: Vec<Value> = (0..201)
        .map(|i| entry(&format!("^pattern-{i}$"), "note"))
        .collect();
    let entries_200_enabled: Vec<Value> = (0..200)
        .map(|i| entry(&format!("^pattern-{i}$"), "note"))
        .collect();
    let mut entries_200_plus_disabled = entries_200_enabled.clone();
    entries_200_plus_disabled.push(disabled_entry("^disabled-pattern$", "note"));
    let cases: Vec<(&str, String, &str)> = vec![
        ("invalid JSON", "{".to_string(), "does not parse"),
        ("JSON array", "[]".to_string(), "is not a JSON object"),
        (
            "wrong schemaVersion",
            json!({"schemaVersion": 2, "byCommand": {}, "byAgent": {}}).to_string(),
            "schemaVersion",
        ),
        (
            "non-empty byAgent",
            json!({"schemaVersion": 1, "byCommand": {}, "byAgent": {"codex": []}}).to_string(),
            "byAgent must be empty",
        ),
        (
            "201 entries",
            remote_json("codex", Value::Array(entries_201)),
            "more than 200 entries",
        ),
        (
            "200 enabled + 1 disabled entry",
            remote_json("codex", Value::Array(entries_200_plus_disabled)),
            "more than 200 entries",
        ),
        (
            "disabled invalid entry",
            remote_json("codex", json!([{"pattern": 5, "enabled": false}])),
            "is not a valid blocking-menu entry",
        ),
        (
            "disabled uncompilable pattern",
            remote_json("codex", json!([disabled_entry("(", "note")])),
            "pattern does not compile",
        ),
        (
            "disabled empty-string pattern",
            remote_json("codex", json!([disabled_entry(".*", "note")])),
            "pattern matches the empty string",
        ),
        (
            "invalid entry",
            remote_json("codex", json!([{"pattern": 5}])),
            "is not a valid blocking-menu entry",
        ),
        (
            "513-byte pattern",
            remote_json("codex", json!([entry(&pattern_513, "note")])),
            "pattern longer than 512 bytes",
        ),
        (
            "201-byte notification",
            remote_json("codex", json!([entry("^ok$", &notification_201)])),
            "notification longer than 200 bytes",
        ),
        (
            "257 e-acute pattern",
            remote_json("codex", json!([entry(&pattern_e_acute_257, "note")])),
            "pattern longer than 512 bytes",
        ),
        (
            "101 e-acute notification",
            remote_json("codex", json!([entry("^ok$", &notification_e_acute_101)])),
            "notification longer than 200 bytes",
        ),
        (
            "newline notification",
            remote_json("codex", json!([entry("^ok$", "a\nb")])),
            "notification contains a control character",
        ),
        (
            "bell notification",
            remote_json("codex", json!([entry("^ok$", "a\u{7}b")])),
            "notification contains a control character",
        ),
        (
            "uncompilable pattern",
            remote_json("codex", json!([entry("(", "note")])),
            "pattern does not compile",
        ),
        (
            "pattern over the compile size limit",
            remote_json("codex", json!([entry(r"\w{1000}", "note")])),
            "size limit",
        ),
        (
            "empty-string pattern ^",
            remote_json("codex", json!([entry("^", "note")])),
            "pattern matches the empty string",
        ),
        (
            "empty-string pattern .*",
            remote_json("codex", json!([entry(".*", "note")])),
            "pattern matches the empty string",
        ),
        (
            "empty-string pattern \\s*",
            remote_json("codex", json!([entry(r"\s*", "note")])),
            "pattern matches the empty string",
        ),
        (
            "benign-row pattern .",
            remote_json("codex", json!([entry(".", "note")])),
            "pattern matches a benign row",
        ),
        (
            "benign-row pattern \\w+",
            remote_json("codex", json!([entry(r"\w+", "note")])),
            "pattern matches a benign row",
        ),
        (
            "benign-row pattern ^\\s*>",
            remote_json("codex", json!([entry(r"^\s*>", "note")])),
            "pattern matches a benign row",
        ),
        (
            "benign-row pattern Traceback",
            remote_json("codex", json!([entry("Traceback", "note")])),
            "pattern matches a benign row",
        ),
    ];
    for (label, contents, expected) in cases {
        let err = match validate_remote_blocking_menus_file(&contents) {
            Err(e) => e,
            Ok(_) => panic!("{label}: expected a rejection, got an accepted file"),
        };
        assert!(
            err.contains(expected),
            "{label}: expected the error to contain {expected:?}, got {err:?}"
        );
    }

    // The exact ceilings are accepted, not rejected: a 200-byte notification,
    // a 512-byte pattern, and the named pattern the evaluator test reuses.
    let mut notification_200 = "grok is waiting for you to answer the test menu".to_string();
    notification_200.push_str(&" ".repeat(200 - notification_200.len()));
    assert_eq!(notification_200.len(), 200);
    let pattern_512 = format!("^{}", "a".repeat(511));
    assert_eq!(pattern_512.len(), 512);
    let accepted = remote_json(
        "grok",
        json!([
            entry(r"^\s*Grok test menu\?", &notification_200),
            entry(&pattern_512, "note"),
        ]),
    );
    let file = validate_remote_blocking_menus_file(&accepted)
        .expect("the exact 200-byte notification and 512-byte pattern are accepted");
    assert_eq!(file.by_command["grok"].len(), 2);

    // Exactly 200 entries are accepted: the ceiling counts disabled entries too,
    // and the comparison is `> 200`, never an enabled-only count or `>=`.
    let file = validate_remote_blocking_menus_file(&remote_json(
        "codex",
        Value::Array(entries_200_enabled),
    ))
    .expect("exactly 200 entries are accepted");
    assert_eq!(file.by_command["codex"].len(), 200);
}

#[test]
fn the_cache_is_revalidated_on_every_read() {
    let temp = tempfile::TempDir::new().unwrap();
    let settings_path = temp.path().join("settings.json");
    let grok = vec![parsed_entry(r"^\s*Grok test menu\?", "grok is waiting")];

    write_remote(
        temp.path(),
        &remote_json(
            "grok",
            json!([entry(r"^\s*Grok test menu\?", "grok is waiting")]),
        ),
    );
    let store = BlockingMenusStore::load_from_settings_path(&settings_path);
    assert_eq!(store.resolve("grok-1", "grok"), grok);

    // Any rejection empties the whole layer on the next read, and the shipped
    // layer then applies again.
    write_remote(
        temp.path(),
        &json!({
            "schemaVersion": 1,
            "byCommand": {"grok": [entry(r"^\s*Grok test menu\?", "grok is waiting")]},
            "byAgent": {"grok-1": [entry(r"^\s*Grok test menu\?", "grok is waiting")]},
        })
        .to_string(),
    );
    let store = BlockingMenusStore::load_from_settings_path(&settings_path);
    assert_eq!(store.resolve("grok-1", "grok"), Vec::new());
    assert_eq!(
        store.resolve("codex-1", "codex"),
        shipped_blocking_menus().by_command["codex"].clone()
    );

    // A missing cache is silent and empty.
    std::fs::remove_file(blocking_menus_remote_path(&settings_path)).unwrap();
    let store = BlockingMenusStore::load_from_settings_path(&settings_path);
    assert_eq!(store.resolve("grok-1", "grok"), Vec::new());
}

#[test]
fn precedence_is_local_then_remote_then_shipped() {
    let r = parsed_entry("^remote-codex$", "remote");
    let g = parsed_entry(r"^\s*Grok test menu\?", "remote-grok");
    let l = parsed_entry("^local-grok$", "local");
    let a = parsed_entry("^agent-grok$", "agent");
    let x = parsed_entry("^legacy$", "legacy");
    let remote_grok = |entries: Vec<BlockingMenuEntry>| layers(&[("grok", entries)], &[]);

    // Remote codex replaces the shipped codex set.
    let store = BlockingMenusStore::with_layers(
        layers(&[("codex", vec![r.clone()])], &[]),
        BlockingMenusFile::default(),
    );
    assert_eq!(store.resolve("codex-1", "codex"), vec![r.clone()]);

    // Remote grok applies where shipped has nothing; the control proves the hit.
    let store =
        BlockingMenusStore::with_layers(remote_grok(vec![g.clone()]), BlockingMenusFile::default());
    assert_eq!(store.resolve("grok-1", "grok"), vec![g.clone()]);
    assert_eq!(
        BlockingMenusStore::shipped_only().resolve("grok-1", "grok"),
        Vec::new()
    );

    // local.byCommand beats remote.byCommand.
    let store = BlockingMenusStore::with_layers(
        remote_grok(vec![g.clone()]),
        layers(&[("grok", vec![l.clone()])], &[]),
    );
    assert_eq!(store.resolve("grok-1", "grok"), vec![l.clone()]);

    // local.byAgent beats both files.
    let store = BlockingMenusStore::with_layers(
        remote_grok(vec![g.clone()]),
        layers(&[("grok", vec![l.clone()])], &[("grok-1", vec![a.clone()])]),
    );
    assert_eq!(store.resolve("grok-1", "grok"), vec![a.clone()]);

    // A remote [] retracts the shipped patterns for that stem.
    let store = BlockingMenusStore::with_layers(
        layers(&[("codex", Vec::new())], &[]),
        BlockingMenusFile::default(),
    );
    assert_eq!(store.resolve("codex-1", "codex"), Vec::new());

    // A remote file without `pi` leaves the shipped pi entry to apply.
    let store =
        BlockingMenusStore::with_layers(remote_grok(vec![g.clone()]), BlockingMenusFile::default());
    assert_eq!(
        store.resolve("pi-1", "pi"),
        shipped_blocking_menus().by_command["pi"].clone()
    );

    // Layer 0: a legacy array on the agent still wins over every file.
    let store = BlockingMenusStore::with_layers(remote_grok(vec![g]), BlockingMenusFile::default());
    let agent = AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: "grok-1".to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: "grok-1".to_string(),
        command: "grok".to_string(),
        color: "#000000".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: Some(vec![x.clone()]),
        backend: Default::default(),
    };
    assert_eq!(store.resolve_for(&agent), vec![x]);
}

#[test]
fn default_blocking_menus_for_command_ignores_the_remote_cache() {
    let temp = tempfile::TempDir::new().unwrap();
    let settings_path = temp.path().join("settings.json");
    write_remote(
        temp.path(),
        &remote_json("codex", json!([entry("^remote-codex$", "remote")])),
    );
    let store = BlockingMenusStore::load_from_settings_path(&settings_path);
    assert_eq!(store.resolve("codex-1", "codex").len(), 1);
    let default = default_blocking_menus_for_command("codex");
    assert_eq!(
        default,
        shipped_blocking_menus().by_command["codex"].clone()
    );
    assert_eq!(default.len(), 2);
}

#[test]
fn remote_flag_defaults_true_and_round_trips() {
    assert!(AppSettings::default().remote_blocking_menus_enabled);

    let mut value =
        serde_json::to_value(AppSettings::default()).expect("serialize default to value");
    assert_eq!(
        value.get("remoteBlockingMenusEnabled"),
        Some(&Value::Bool(true))
    );

    let obj = value
        .as_object_mut()
        .expect("settings serializes to an object");
    obj.remove("remoteBlockingMenusEnabled");
    let back: AppSettings =
        serde_json::from_value(value.clone()).expect("deserialize without the new key");
    assert!(back.remote_blocking_menus_enabled);

    let mut disabled = value.clone();
    disabled["remoteBlockingMenusEnabled"] = json!(false);
    let off: AppSettings = serde_json::from_value(disabled).expect("deserialize with the flag off");
    assert!(!off.remote_blocking_menus_enabled);

    value["npmUpdateNotificationsEnabled"] = json!(false);
    let independent: AppSettings =
        serde_json::from_value(value).expect("deserialize with npm notifications off");
    assert!(!independent.npm_update_notifications_enabled);
    assert!(independent.remote_blocking_menus_enabled);
}
