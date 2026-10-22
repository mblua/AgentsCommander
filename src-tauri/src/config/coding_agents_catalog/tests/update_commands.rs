use super::*;

#[test]
fn load_catalog_never_invents_update_commands_for_legacy_entries() {
    // A catalog seeded before the update-command era (#1325) carries no
    // updateCommands. #1967 P4: nothing is backfilled in memory - the
    // commands stay empty until the P5 migration persists defaults - the
    // persisted bytes are untouched, and the report explains the pending
    // migration per entry.
    let json = manifest_json(concat!(
        r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","##,
        r##""command":"claude","envs":[],"isolatedHome":false,"removable":true},
"##,
        r##"         {"key":"pi","label":"Pi","description":"d","color":"#ec4899","##,
        r##""command":"pi","envs":[],"isolatedHome":false,"removable":true}]"##,
    ));
    let dir = seed_dir();
    std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
    std::fs::write(manifest_path(dir.path()), &json).unwrap();

    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(loaded.len(), 2);
    assert!(loaded.iter().all(|def| def.update_commands.is_empty()));
    // No-write proof: the manifest bytes are identical before/after the load.
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        json.as_bytes()
    );

    let report = report_for(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.warnings.len(), 2);
    assert!(report
        .warnings
        .iter()
        .all(|warning| warning.code == "migrationPending"));
}

#[test]
fn load_catalog_preserves_persisted_update_commands() {
    // User-authored sequences are data: they are served exactly as written.
    let json = manifest_json(concat!(
        r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","##,
        r##""command":"claude","envs":[],"isolatedHome":false,"removable":true,"##,
        r##""updateCommands":["claude --custom"]}]"##,
    ));
    let dir = seed_dir();
    std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
    std::fs::write(manifest_path(dir.path()), &json).unwrap();

    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(
        loaded[0].update_commands,
        vec!["claude --custom".to_string()]
    );
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        json.as_bytes()
    );
}

#[test]
fn load_catalog_custom_command_without_sequence_stays_empty() {
    // Custom command `bob` has no persisted sequence -> stays empty (never
    // prompted nor updated). No embedded table is consulted.
    let json = manifest_json(concat!(
        r##"[{"key":"bob","label":"Bob","description":"d","color":"#333","command":"bob","##,
        r##""envs":[],"isolatedHome":false,"removable":true}]"##,
    ));
    let dir = seed_dir();
    std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
    std::fs::write(manifest_path(dir.path()), &json).unwrap();

    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(loaded.len(), 1);
    assert!(loaded[0].update_commands.is_empty());
}

#[test]
fn load_catalog_duplicate_commands_keep_their_own_entries() {
    // Two profiles share the `pi` command under different keys; the persisted
    // data is served per entry. The effective (first) entry rule lives in
    // `agent_update`, which never borrows the second entry's sequence.
    let json = manifest_json(concat!(
        r##"[{"key":"pi","label":"Pi","description":"d","color":"#ec4899","command":"pi","##,
        r##""envs":[],"isolatedHome":false,"removable":true},
"##,
        r##"         {"key":"pi-max","label":"Pi Max","description":"d","color":"#ec4899","##,
        r##""command":"pi","envs":[],"isolatedHome":false,"removable":true}]"##,
    ));
    let dir = seed_dir();
    std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
    std::fs::write(manifest_path(dir.path()), &json).unwrap();

    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(loaded.len(), 2);
    assert!(loaded.iter().all(|def| def.update_commands.is_empty()));
}

#[test]
fn load_catalog_mixed_duplicate_sequences_keep_persisted_values() {
    // Data-level preservation: the FIRST `pi` entry stays empty and the
    // SECOND keeps its custom sequence untouched. The read never rewrites
    // one entry from another (the effective first-entry rule is applied by
    // the updater, not by the read).
    let json = manifest_json(concat!(
        r##"[{"key":"pi","label":"Pi","description":"d","color":"#ec4899","command":"pi","##,
        r##""envs":[],"isolatedHome":false,"removable":true},
"##,
        r##"         {"key":"pi-max","label":"Pi Max","description":"d","color":"#ec4899","##,
        r##""command":"pi","envs":[],"isolatedHome":false,"removable":true,"##,
        r##""updateCommands":["pi --custom"]}]"##,
    ));
    let dir = seed_dir();
    std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
    std::fs::write(manifest_path(dir.path()), &json).unwrap();

    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(loaded.len(), 2);
    assert!(loaded[0].update_commands.is_empty());
    assert_eq!(loaded[1].update_commands, vec!["pi --custom".to_string()]);
}

#[test]
fn embedded_default_ships_update_commands_for_all_but_cursor_and_muse() {
    // #1318/#1325/#1546 drift guard: claude, pi, codex, hermes, opencode,
    // and antigravity ship the update command; cursor, grok and Muse ship
    // none; every entry defaults autoUpdate to false.
    let catalog = embedded_default_catalog();
    assert_eq!(catalog.agents.len(), 9);
    for def in &catalog.agents {
        assert!(
            !def.auto_update,
            "{} autoUpdate must default false",
            def.key
        );
        match def.key.as_str() {
            "claude" => assert_eq!(def.update_commands, vec!["claude --update".to_string()]),
            "pi" => assert_eq!(def.update_commands, vec!["pi update".to_string()]),
            "codex" => assert_eq!(def.update_commands, vec!["codex update".to_string()]),
            "hermes" => {
                assert_eq!(def.update_commands, vec!["hermes update --yes".to_string()])
            }
            "opencode" => assert_eq!(def.update_commands, vec!["opencode upgrade".to_string()]),
            "antigravity" => assert_eq!(def.update_commands, vec!["agy update".to_string()]),
            "cursor" | "grok" | "muse" => assert!(
                def.update_commands.is_empty(),
                "{} must ship no update command",
                def.key
            ),
            other => panic!("unexpected key {other:?}"),
        }
    }
}
