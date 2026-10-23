use super::*;

// ── #778: disk-authoritative project_paths (Design S) ────────────────────

fn settings_with_project_paths(paths: &[&str]) -> AppSettings {
    AppSettings {
        project_paths: paths.iter().map(|p| p.to_string()).collect(),
        project_path: paths.first().map(|p| p.to_string()),
        ..AppSettings::default()
    }
}

fn settings_with_project_and_archived_paths(paths: &[&str], archived: &[&str]) -> AppSettings {
    AppSettings {
        project_paths: paths.iter().map(|p| p.to_string()).collect(),
        project_path: paths.first().map(|p| p.to_string()),
        archived_project_paths: archived.iter().map(|p| p.to_string()).collect(),
        ..AppSettings::default()
    }
}

/// #1077: create a real AC project dir (`<parent>/<name>/.ac`) and return its
/// display-canonical path, so the six-field decoder validates it and the
/// SELECTED runtime path equals the stored string. Used by preserve/reconcile
/// tests that assert the returned settings mirror the preserved disk list.
pub(super) fn real_ac_project_path(parent: &std::path::Path, name: &str) -> String {
    let dir = parent.join(name);
    std::fs::create_dir_all(dir.join(".ac")).unwrap();
    let canon = std::fs::canonicalize(&dir).unwrap();
    crate::config::projects::display_canonical(&canon.to_string_lossy())
}

#[test]
fn read_project_paths_from_disk_returns_list_and_head() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    super::super::save_settings_to_path(&settings_with_project_paths(&["C:/a", "C:/b"]), &path)
        .unwrap();
    let lists = super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap();
    assert_eq!(
        lists.project_paths,
        vec!["C:/a".to_string(), "C:/b".to_string()]
    );
    assert_eq!(lists.project_path.as_deref(), Some("C:/a"));
}

#[test]
fn read_project_paths_from_disk_returns_none_when_file_absent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("does-not-exist.json");
    // NotFound means there is no disk truth to substitute.
    assert_eq!(
        super::super::read_project_paths_from_disk(&path).unwrap(),
        None
    );
}

#[test]
fn read_project_paths_from_disk_returns_empty_when_key_absent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"themeLight":true}"#).unwrap();
    assert!(super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap()
        .project_paths
        .is_empty());
}

#[test]
fn read_project_paths_from_disk_returns_empty_when_key_null() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"projectPaths":null}"#).unwrap();
    assert!(super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap()
        .project_paths
        .is_empty());
}

#[test]
fn read_project_paths_from_disk_aborts_when_key_wrong_type() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"projectPaths":"C:/a"}"#).unwrap();
    assert!(super::super::read_project_paths_from_disk(&path).is_err());
}

#[test]
fn read_project_paths_from_disk_accepts_legit_empty_list() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"projectPaths":[]}"#).unwrap();
    let lists = super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap();
    assert!(lists.project_paths.is_empty());
    assert_eq!(lists.project_path, None);
}

#[test]
fn read_project_paths_from_disk_returns_archived_list() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    super::super::save_settings_to_path(
        &settings_with_project_and_archived_paths(&["C:/a"], &["C:/old"]),
        &path,
    )
    .unwrap();

    let lists = super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap();

    assert_eq!(
        lists.archived_project_paths,
        Some(vec!["C:/old".to_string()])
    );
}

#[test]
fn read_project_paths_from_disk_returns_archived_none_when_key_absent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"projectPaths":["C:/a"],"projectPath":"C:/a"}"#).unwrap();

    let lists = super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap();

    assert_eq!(lists.project_paths, vec!["C:/a".to_string()]);
    assert_eq!(lists.project_path.as_deref(), Some("C:/a"));
    assert_eq!(lists.archived_project_paths, None);
}

#[test]
fn read_project_paths_from_disk_returns_archived_none_when_key_null() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"projectPaths":["C:/a"],"archivedProjectPaths":null}"#,
    )
    .unwrap();

    let lists = super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap();

    assert_eq!(lists.archived_project_paths, None);
}

#[test]
fn read_project_paths_from_disk_returns_archived_some_empty_when_key_is_empty_list() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"projectPaths":["C:/a"],"archivedProjectPaths":[]}"#,
    )
    .unwrap();

    let lists = super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap();

    assert_eq!(lists.archived_project_paths, Some(vec![]));
}

#[test]
fn read_project_paths_from_disk_aborts_when_archived_key_wrong_type() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"projectPaths":["C:/a"],"archivedProjectPaths":"C:/old"}"#,
    )
    .unwrap();

    assert!(super::super::read_project_paths_from_disk(&path).is_err());
}

#[test]
fn read_project_paths_from_disk_aborts_when_archived_list_has_non_string_element() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"projectPaths":["C:/a"],"archivedProjectPaths":["C:/old",123]}"#,
    )
    .unwrap();

    assert!(super::super::read_project_paths_from_disk(&path).is_err());
}

#[test]
fn read_project_paths_from_disk_drops_non_string_project_path_elements() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"projectPaths":["C:/a",123],"archivedProjectPaths":["C:/old"]}"#,
    )
    .unwrap();

    let lists = super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap();

    assert_eq!(lists.project_paths, vec!["C:/a".to_string()]);
    assert_eq!(
        lists.archived_project_paths,
        Some(vec!["C:/old".to_string()])
    );
}

#[test]
fn read_project_paths_from_disk_aborts_on_unparseable_json() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, "{ not valid json").unwrap();
    // G2: unparseable is NOT NotFound, so the reader aborts (Err), never disk=[].
    assert!(super::super::read_project_paths_from_disk(&path).is_err());
}

#[test]
fn read_project_paths_from_disk_aborts_on_non_notfound_read_error() {
    // G2: a directory at the path yields a non-NotFound read error; the reader
    // must abort (Err), not degrade to empty (which would silently drop a project).
    let temp = tempfile::tempdir().unwrap();
    let dir_as_path = temp.path().join("iam-a-dir");
    std::fs::create_dir_all(&dir_as_path).unwrap();
    assert!(super::super::read_project_paths_from_disk(&dir_as_path).is_err());
}

#[test]
fn save_settings_preserve_keeps_disk_project_paths_on_passthrough() {
    // Marquee preserve: disk has [A, X] (X = an append this in-memory candidate
    // never saw); a whole-object save that changed only an unrelated field must
    // leave project_paths = [A, X] on disk (fail-safe) AND persist the field.
    // #1077: real AC dirs so the selected runtime paths equal the preserved list.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    let x = real_ac_project_path(temp.path(), "X");
    super::super::save_settings_to_path(&settings_with_project_paths(&[&a, &x]), &path).unwrap();

    let mut candidate = settings_with_project_paths(&[&a]); // stale, missing X
    candidate.sidebar_style = "deep-space".to_string(); // unrelated GUI field
    let written =
        super::super::save_settings_to_path_preserving_project_paths(&candidate, &path).unwrap();

    let reloaded: AppSettings = super::super::load_settings_from_path(&path);
    assert_eq!(reloaded.project_paths, vec![a.clone(), x.clone()]); // X preserved, not clobbered
    assert_eq!(reloaded.project_path.as_deref(), Some(a.as_str()));
    assert_eq!(reloaded.sidebar_style, "deep-space"); // unrelated field persisted

    // #1077: the returned settings carry the SELECTED (validated) projection,
    // which equals the preserved disk list because both dirs are real.
    assert_eq!(written.project_paths, vec![a.clone(), x.clone()]);
    assert_eq!(written.project_path.as_deref(), Some(a.as_str()));
}

#[test]
fn refresh_project_paths_from_path_clears_stale_archived_when_authority_omits_key() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"projectPaths":["A"],"projectPath":"A"}"#).unwrap();
    let mut settings = settings_with_project_and_archived_paths(&["stale"], &["Archived"]);

    super::super::refresh_project_paths_from_path(&mut settings, &path).unwrap();

    assert_eq!(settings.project_paths, vec!["A".to_string()]);
    assert_eq!(settings.project_path.as_deref(), Some("A"));
    assert!(settings.archived_project_paths.is_empty());
}

#[test]
fn refresh_project_paths_from_path_clears_archived_list_when_disk_says_empty() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"projectPaths":["A"],"projectPath":"A","archivedProjectPaths":[]}"#,
    )
    .unwrap();
    let mut settings = settings_with_project_and_archived_paths(&["stale"], &["Archived"]);

    super::super::refresh_project_paths_from_path(&mut settings, &path).unwrap();

    assert_eq!(settings.project_paths, vec!["A".to_string()]);
    assert_eq!(settings.project_path.as_deref(), Some("A"));
    assert!(settings.archived_project_paths.is_empty());
}

#[test]
fn save_settings_preserves_disk_archived_list() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    let archived_a = real_ac_project_path(temp.path(), "ArchivedA");
    super::super::save_settings_to_path(
        &settings_with_project_and_archived_paths(&[&a], &[&archived_a]),
        &path,
    )
    .unwrap();
    let candidate = settings_with_project_and_archived_paths(&[&a], &["ArchivedB"]);

    let written =
        super::super::save_settings_to_path_preserving_project_paths(&candidate, &path).unwrap();

    let reloaded: AppSettings =
        crate::config::settings::load_settings_from_path(std::path::Path::new(&path));
    assert_eq!(reloaded.archived_project_paths, vec![archived_a.clone()]);
    assert_eq!(written.archived_project_paths, vec![archived_a.clone()]);
}

#[test]
fn save_settings_discards_archived_draft_when_authority_omits_key() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    let archived = real_ac_project_path(temp.path(), "Archived");
    // Seed a whole, valid AppSettings object but with NO archivedProjectPaths
    // key (the preserve writer's disk gate requires a valid whole object).
    let mut seed = serde_json::to_value(settings_with_project_paths(&[&a])).unwrap();
    seed.as_object_mut().unwrap().remove("archivedProjectPaths");
    std::fs::write(&path, serde_json::to_string_pretty(&seed).unwrap()).unwrap();

    let candidate = settings_with_project_and_archived_paths(&[&a], &[&archived]);
    let written =
        super::super::save_settings_to_path_preserving_project_paths(&candidate, &path).unwrap();

    let reloaded: AppSettings =
        crate::config::settings::load_settings_from_path(std::path::Path::new(&path));
    assert_eq!(written.project_paths, vec![a.clone()]);
    assert_eq!(written.project_path.as_deref(), Some(a.as_str()));
    // An absent authoritative group cannot be replaced by a caller draft.
    assert!(written.archived_project_paths.is_empty());
    assert!(reloaded.archived_project_paths.is_empty());
}

#[test]
fn save_settings_with_project_paths_writes_verbatim() {
    // The explicit writer persists the in-memory list as-is (deliberate mutation).
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    super::super::save_settings_to_path(&settings_with_project_paths(&["A"]), &path).unwrap();
    super::super::save_settings_to_path(&settings_with_project_paths(&["A", "Y"]), &path).unwrap();
    let lists = super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap();
    assert_eq!(lists.project_paths, vec!["A".to_string(), "Y".to_string()]);
}

#[test]
fn save_settings_with_project_paths_writes_archived_verbatim() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    super::super::save_settings_with_project_paths_to_path(
        &settings_with_project_and_archived_paths(&["A"], &["Archived"]),
        &path,
    )
    .unwrap();

    let lists = super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap();

    assert_eq!(
        lists.archived_project_paths,
        Some(vec!["Archived".to_string()])
    );
}

#[test]
fn resync_reconciles_missing_and_conflict_without_duplication() {
    // Grinch Defect 1: a decoded state carrying a MISSING and a CONFLICT
    // record, mutated across register/remove, must never duplicate/resurrect
    // those records, and remove must actually remove.
    use crate::config::projects::{
        IssueKind, ProjectPathPersistenceState, ProjectSource, RawStringField, RepairKind,
        ResolvedPair,
    };
    let pair =
        |idx: usize, raw: &str, selected: Option<&str>, issue: Option<IssueKind>| ResolvedPair {
            source: ProjectSource::ProjectPaths,
            index: Some(idx),
            raw_absolute: RawStringField::string(raw),
            raw_relative: RawStringField::absent(),
            absolute_side: super::super::absent_side(),
            relative_side: super::super::absent_side(),
            selected: selected.map(String::from),
            selected_canonical_raw: selected.map(String::from),
            selected_identity: None,
            issue,
            repair: RepairKind::None,
        };
    let state = ProjectPathPersistenceState {
        pairs: vec![
            pair(0, "A", Some("A"), None),
            pair(1, "M", None, Some(IssueKind::Missing)),
            pair(2, "C", None, Some(IssueKind::Conflict)),
        ],
        selected_head: Some("A".to_string()),
        active_registration_count: 3,
        archived_registration_count: 0,
        active_companion_present: true,
        archived_companion_present: false,
        has_genuine_singular: false,
        active_reconcile_eligible: false,
        archived_reconcile_eligible: false,
        structural_issues: Vec::new(),
        runtime_authoritative: false,
    };
    let mut s = AppSettings {
        project_path_state: std::sync::Arc::new(state),
        ..AppSettings::default()
    };
    // Raw runtime (3-field refresh) carries all three stored absolutes.
    s.project_paths = vec!["A".to_string(), "M".to_string(), "C".to_string()];

    // Register D.
    s.project_paths.push("D".to_string());
    super::super::resync_project_state_from_runtime(&mut s);
    {
        let st = &s.project_path_state;
        assert_eq!(
            st.active_registration_count, 4,
            "no duplication on register"
        );
        let keys: Vec<String> = st
            .active_pairs()
            .iter()
            .map(|p| p.raw_absolute.value.clone().unwrap())
            .collect();
        assert_eq!(
            keys,
            vec!["A", "M", "C", "D"],
            "order preserved, no duplicates"
        );
        assert_eq!(st.active_pairs()[1].issue, Some(IssueKind::Missing));
        assert_eq!(st.active_pairs()[2].issue, Some(IssueKind::Conflict));
        assert!(st.active_pairs()[3].issue.is_none());
    }

    // Remove M ("Remove from list" on the missing entry).
    s.project_paths = vec!["A".to_string(), "C".to_string(), "D".to_string()];
    super::super::resync_project_state_from_runtime(&mut s);
    {
        let st = &s.project_path_state;
        assert_eq!(
            st.active_registration_count, 3,
            "the missing record must actually be removed, not resurrected"
        );
        let keys: Vec<String> = st
            .active_pairs()
            .iter()
            .map(|p| p.raw_absolute.value.clone().unwrap())
            .collect();
        assert_eq!(keys, vec!["A", "C", "D"]);
        assert_eq!(
            st.active_pairs()[1].issue,
            Some(IssueKind::Conflict),
            "unrelated conflict preserved through the removal"
        );
    }
}

#[test]
fn resync_persists_mutation_against_decoded_state() {
    // Regression: a mutator that changes only the runtime lists against a
    // DECODED (non-synthesized) hidden state must resync so the reconcile
    // write records the new project instead of copying the stale disk list.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    let b = real_ac_project_path(temp.path(), "B");
    super::super::save_settings_to_path(&settings_with_project_paths(&[&a]), &path).unwrap();

    // A decoded hidden state as the loader produces (runtime_authoritative=false).
    let value = serde_json::json!({ "projectPaths": [a.clone()], "projectPath": a.clone() });
    let state = super::super::decode_project_state(
        value.as_object().unwrap(),
        None,
        &crate::config::projects::FsCandidateResolver,
    );
    assert!(!state.runtime_authoritative);
    let mut settings = settings_with_project_paths(&[&a]);
    settings.project_path_state = std::sync::Arc::new(state);

    // Register B into the runtime list, then resync + reconcile-write.
    settings.project_paths.push(b.clone());
    super::super::resync_project_state_from_runtime(&mut settings);
    super::super::save_settings_with_project_paths_to_path(&settings, &path).unwrap();

    let lists = super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap();
    assert_eq!(lists.project_paths, vec![a, b]);
}

/// #1171, 9.2.17 - **the regression a plain `BTreeMap<String, WatcherConfig>` would let
/// through, and whose failure mode is the whole settings file.**
///
/// Three hand-written mistakes, one per entry: a capitalized enum variant, a scalar where
/// a list belongs, and a quoted number. Without the per-entry wrapper, ONE of them makes
/// `serde_json::from_value::<AppSettings>` fail, `load_settings_from_path` replaces
/// everything with `AppSettings::default()`, and AgentsCommander starts with no agents
/// configured and one log line - after which every save is refused by the #1077 gate.
#[test]
fn one_malformed_watcher_does_not_take_the_settings_file_down() {
    let contents = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": ["-NoLogo"],
        "agents": [
            { "id": "a1", "label": "Claude", "command": "claude", "color": "#fff" },
            { "id": "a2", "label": "Codex", "command": "codex", "color": "#000" }
        ],
        "watchers": {
            "bad-mode": { "mode": "State", "pattern": "x" },
            "bad-commands": { "mode": "occurrence", "pattern": "x", "commands": "claude" },
            "bad-window": { "mode": "occurrence", "pattern": "x", "dedupeWindowMs": "2000" },
            "good": { "mode": "state", "pattern": "Permission required" }
        }
    })
    .to_string();

    let (settings, _) = super::super::parse_settings_json(&contents, "test", None).expect(
        "a malformed watcher must never fail the whole file: that is what the wrapper is for",
    );

    // Every OTHER setting survived.
    assert_eq!(settings.agents.len(), 2);
    assert_eq!(settings.default_shell, "powershell.exe");

    // Every entry is still there, and exactly one of them resolved.
    assert_eq!(settings.watchers.len(), 4);
    let valid: Vec<&str> = settings
        .watchers
        .iter()
        .filter(|(_, entry)| entry.valid().is_some())
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(valid, vec!["good"]);

    // ...and the three bad ones are kept VERBATIM, so a save writes back the user's bytes
    // instead of deleting what it could not read.
    let round_tripped = serde_json::to_value(&settings.watchers).expect("serializes");
    assert_eq!(
        round_tripped["bad-mode"],
        serde_json::json!({ "mode": "State", "pattern": "x" })
    );
    assert_eq!(
        round_tripped["bad-commands"],
        serde_json::json!({ "mode": "occurrence", "pattern": "x", "commands": "claude" })
    );
    assert_eq!(
        round_tripped["bad-window"],
        serde_json::json!({ "mode": "occurrence", "pattern": "x", "dedupeWindowMs": "2000" })
    );
}

/// #1171, 9.2.18 - a user who configures nothing never sees either new key appear.
#[test]
fn a_settings_file_with_no_watchers_round_trips_without_the_new_keys() {
    let contents = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": [],
        "agents": []
    })
    .to_string();

    let (settings, _) = super::super::parse_settings_json(&contents, "test", None).expect("parses");
    assert!(settings.watchers.is_empty());
    assert!(settings.watchers_geometry.is_none());

    let written = serde_json::to_value(&settings).expect("serializes");
    let root = written.as_object().expect("object");
    assert!(
        !root.contains_key("watchers"),
        "an empty map must not appear"
    );
    assert!(
        !root.contains_key("watchersGeometry"),
        "this is why watchersGeometry carries skip_serializing_if and mainGeometry does not"
    );
}

/// #1171, 9.2.19 - a configured watcher round-trips value for value, INCLUDING the
/// absent-against-`[]` distinction for `commands`, which is the one place where the two
/// are opposites: absent reaches every agent, `[]` reaches none.
#[test]
fn watchers_round_trip_through_save_and_load_including_absent_versus_empty() {
    let contents = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": [],
        "agents": [],
        "watchers": {
            "all-agents": { "mode": "state", "pattern": "  Permission" },
            "nobody": { "mode": "occurrence", "pattern": "Read", "commands": [] },
            "claude-only": {
                "enabled": false,
                "mode": "occurrence",
                "pattern": "Read \\((.+)\\)",
                "commands": ["claude"],
                "dedupe": "capture",
                "dedupeWindowMs": 5000,
                "capturedAgainst": "claude 2.1.212"
            }
        }
    })
    .to_string();

    let (settings, _) = super::super::parse_settings_json(&contents, "test", None).expect("parses");

    let all = settings.watchers["all-agents"].valid().expect("valid");
    assert!(all.enabled, "enabled defaults to true");
    assert_eq!(all.mode, super::super::WatcherMode::State);
    assert_eq!(all.pattern, "  Permission");
    assert!(all.commands.is_none(), "absent means every agent");
    assert_eq!(all.dedupe, super::super::WatcherDedupe::Row);
    assert_eq!(all.dedupe_window_ms, 2000);

    let nobody = settings.watchers["nobody"].valid().expect("valid");
    assert_eq!(
        nobody.commands.as_deref(),
        Some(&[] as &[String]),
        "`[]` must survive as `[]` and never collapse into absent"
    );

    let claude = settings.watchers["claude-only"].valid().expect("valid");
    assert!(!claude.enabled);
    assert_eq!(claude.mode, super::super::WatcherMode::Occurrence);
    assert_eq!(claude.dedupe, super::super::WatcherDedupe::Capture);
    assert_eq!(claude.dedupe_window_ms, 5000);
    assert_eq!(claude.captured_against.as_deref(), Some("claude 2.1.212"));

    // Byte-stable: what comes back out is what went in, key for key.
    let written = serde_json::to_value(&settings.watchers).expect("serializes");
    let original: serde_json::Value = serde_json::from_str(&contents).expect("parses");
    let mut expected = original["watchers"].clone();
    // The two defaulted fields the input omitted are written explicitly, since neither
    // carries `skip_serializing_if`: they are settings with a value, not absent ones.
    expected["all-agents"]["enabled"] = serde_json::json!(true);
    expected["all-agents"]["dedupe"] = serde_json::json!("row");
    expected["all-agents"]["dedupeWindowMs"] = serde_json::json!(2000);
    expected["nobody"]["enabled"] = serde_json::json!(true);
    expected["nobody"]["dedupe"] = serde_json::json!("row");
    expected["nobody"]["dedupeWindowMs"] = serde_json::json!(2000);
    assert_eq!(written, expected);
}

/// #2482 - absent `quotaSources` stays absent on save; an unknown `kind` costs one
/// skipped entry and round-trips verbatim.
#[test]
fn quota_sources_round_trip_and_an_unknown_kind_is_kept_verbatim() {
    let bare = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": [],
        "agents": []
    })
    .to_string();
    let (settings, _) = super::super::parse_settings_json(&bare, "test", None).expect("parses");
    assert!(settings.quota_sources.is_empty());
    let written = serde_json::to_value(&settings).expect("serializes");
    assert!(!written.as_object().unwrap().contains_key("quotaSources"));

    let contents = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": [],
        "agents": [],
        "quotaSources": {
            "claude": { "kind": "screenRegex", "pattern": r"Weekly (\d+)%" },
            "codex": { "kind": "fromTheFuture", "endpoint": "x" }
        }
    })
    .to_string();
    let (settings, _) = super::super::parse_settings_json(&contents, "test", None).expect("parses");

    assert_eq!(
        settings.quota_sources["claude"].valid(),
        Some(&super::super::QuotaSourceConfig::ScreenRegex {
            pattern: r"Weekly (\d+)%".to_string(),
            enabled: true,
        })
    );
    assert!(settings.quota_sources["codex"].valid().is_none());

    let written = serde_json::to_value(&settings.quota_sources).expect("serializes");
    assert_eq!(
        written["codex"],
        serde_json::json!({ "kind": "fromTheFuture", "endpoint": "x" })
    );
    assert_eq!(
        written["claude"],
        serde_json::json!({ "kind": "screenRegex", "pattern": r"Weekly (\d+)%", "enabled": true })
    );
}

/// #2686 - the remaining kind round-trips, defaults `enabled` to true, and the tag
/// addition does not capture a `screenRegex` entry.
#[test]
fn a_remaining_kind_round_trips_and_defaults_enabled_true() {
    let contents = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": [],
        "agents": [],
        "quotaSources": {
            "codex": { "kind": "screenRegexRemaining", "pattern": r"Weekly (\d+)% left" },
            "claude": { "kind": "screenRegex", "pattern": r"Weekly (\d+)%" }
        }
    })
    .to_string();
    let (settings, _) = super::super::parse_settings_json(&contents, "test", None).expect("parses");

    assert_eq!(
        settings.quota_sources["codex"].valid(),
        Some(&super::super::QuotaSourceConfig::ScreenRegexRemaining {
            pattern: r"Weekly (\d+)% left".to_string(),
            enabled: true,
        })
    );
    assert_eq!(
        settings.quota_sources["claude"].valid(),
        Some(&super::super::QuotaSourceConfig::ScreenRegex {
            pattern: r"Weekly (\d+)%".to_string(),
            enabled: true,
        })
    );

    let written = serde_json::to_value(&settings.quota_sources).expect("serializes");
    assert_eq!(
        written["codex"]["kind"],
        serde_json::json!("screenRegexRemaining")
    );
    assert_eq!(written["codex"]["enabled"], serde_json::json!(true));
    assert_eq!(written["claude"]["kind"], serde_json::json!("screenRegex"));
}

#[test]
fn save_settings_preserve_twice_succeeds_no_sharing_violation() {
    // G4a: the preserve-read closes its handle before the #774 rename, so two
    // preserve-saves in a row both succeed on Windows (a lingering read handle
    // would trip MoveFileEx with os 32).
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    super::super::save_settings_to_path(&settings_with_project_paths(&["A"]), &path).unwrap();
    super::super::save_settings_to_path_preserving_project_paths(
        &settings_with_project_paths(&["A"]),
        &path,
    )
    .unwrap();
    super::super::save_settings_to_path_preserving_project_paths(
        &settings_with_project_paths(&["A"]),
        &path,
    )
    .unwrap();
    let lists = super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap();
    assert_eq!(lists.project_paths, vec!["A".to_string()]);
}

#[test]
fn remove_project_disk_composition_preserves_unseen_entry() {
    // remove_project command body against a tempdir: disk [A, X] (X unseen by the
    // sidebar), reconcile from disk, remove A, write verbatim => disk ends [X].
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    super::super::save_settings_to_path(&settings_with_project_paths(&["A", "X"]), &path).unwrap();

    let mut s = settings_with_project_paths(&["A"]); // stale in-memory
    let lists = super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap();
    s.project_paths = lists.project_paths;
    s.project_path = lists.project_path; // reconciled to [A, X]
    assert!(crate::config::projects::remove_project_path(&mut s, "A"));
    super::super::save_settings_to_path(&s, &path).unwrap(); // verbatim

    let lists = super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap();
    assert_eq!(lists.project_paths, vec!["X".to_string()]); // A gone, X preserved
    assert_eq!(lists.project_path.as_deref(), Some("X"));
}

#[test]
fn add_reconciles_from_disk_before_upsert() {
    // Add command body: in-memory stale [A], disk [A, X]; reconcile then append Y
    // and write verbatim => disk ends [A, X, Y] (the CLI append X is not lost).
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    super::super::save_settings_to_path(&settings_with_project_paths(&["A", "X"]), &path).unwrap();

    let mut s = settings_with_project_paths(&["A"]); // stale
    let lists = super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap();
    s.project_paths = lists.project_paths;
    s.project_path = lists.project_path; // [A, X]
    s.project_paths.push("Y".to_string()); // stand-in for register upsert
    s.project_path = s.project_paths.first().cloned();
    super::super::save_settings_to_path(&s, &path).unwrap();

    let lists = super::super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap();
    assert_eq!(
        lists.project_paths,
        vec!["A".to_string(), "X".to_string(), "Y".to_string()]
    );
}

#[test]
fn main_sidebar_side_round_trips_through_serde() {
    let mut s = AppSettings::default();
    assert_eq!(s.main_sidebar_side, MainSidebarSide::Right);
    s.main_sidebar_side = MainSidebarSide::Left;
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"mainSidebarSide\":\"left\""));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.main_sidebar_side, MainSidebarSide::Left);
}

#[test]
fn main_sidebar_side_defaults_to_right_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert_eq!(s.main_sidebar_side, MainSidebarSide::Right);
}

#[test]
fn main_resource_monitor_attached_round_trips_and_defaults_false() {
    // #587 - round-trips through serde as camelCase.
    let mut s = AppSettings::default();
    assert!(!s.main_resource_monitor_attached);
    s.main_resource_monitor_attached = true;
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"mainResourceMonitorAttached\":true"));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(back.main_resource_monitor_attached);

    // Absent from an older settings.json => serde default false.
    let json_without = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
    let from_old: AppSettings = serde_json::from_str(json_without).expect("deserialize old json");
    assert!(!from_old.main_resource_monitor_attached);
}
