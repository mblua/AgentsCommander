use super::*;

#[test]
fn validate_catalog_key_accepts_allowlist_and_rejects_others() {
    for ok in ["claude", "cursor-cli", "pi", "a1", "opencode", "x-2-y"] {
        assert!(validate_catalog_key(ok).is_ok(), "should accept {ok:?}");
    }
    for bad in [
        "",
        "Claude",
        "cursor_cli",
        "cursor cli",
        "café",
        "a.b",
        "UP",
        "a/b",
    ] {
        assert!(validate_catalog_key(bad).is_err(), "should reject {bad:?}");
    }
}

#[test]
fn load_missing_manifest_is_unavailable_without_seeding() {
    // #1967 P4: a missing persisted catalog is unavailable; the embedded
    // default is NOT substituted and nothing is created on disk.
    let dir = seed_dir();
    let unavailable = load_catalog(dir.path()).expect_err("absent catalog is unavailable");
    assert_eq!(unavailable.code, "baseUnavailable");
    assert_eq!(
        unavailable.path,
        manifest_path(dir.path()).display().to_string()
    );
    assert!(unavailable.reason.contains("no persisted catalog"));
    assert!(
        !catalog_dir(dir.path()).exists(),
        "a catalog read must never create the catalog directory"
    );
}

#[test]
fn load_corrupt_manifest_is_base_invalid_and_preserves_file() {
    let dir = seed_dir();
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let garbage = b"{ this is not valid json";
    std::fs::write(&path, garbage).unwrap();

    let unavailable = load_catalog(dir.path()).expect_err("corrupt catalog is unavailable");
    assert_eq!(unavailable.code, "baseInvalid");
    assert_eq!(unavailable.path, path.display().to_string());
    // G3: the corrupt file is preserved byte-for-byte, never overwritten.
    assert_eq!(std::fs::read(&path).unwrap(), garbage);
}

#[test]
fn load_skips_invalid_entry_and_keeps_valid_rest() {
    let dir = seed_dir();
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // First entry is valid; second has an illegal key (uppercase); third is
    // valid. Only the two valid entries survive.
    let agents = concat!(
        r##"[
"##,
        r##"        {"key":"claude","label":"Claude","description":"d","color":"#000","##,
        r##""command":"claude","envs":[],"isolatedHome":false,"removable":true},
"##,
        r##"        {"key":"BAD KEY","label":"x","description":"d","color":"#000","command":"x","##,
        r##""envs":[],"isolatedHome":false,"removable":true},
"##,
        r##"        {"key":"mine","label":"Mine","description":"d","color":"#111","##,
        r##""command":"mytool","envs":[],"isolatedHome":false,"removable":true}
"##,
        r##"    ]"##,
    );
    std::fs::write(&path, manifest_json(agents)).unwrap();

    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    let keys: Vec<&str> = loaded.iter().map(|a| a.key.as_str()).collect();
    assert_eq!(keys, ["claude", "mine"]);
}

#[test]
fn load_dedups_duplicate_keys_first_wins() {
    let dir = seed_dir();
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let agents = concat!(
        r##"[
"##,
        r##"        {"key":"claude","label":"First","description":"d","color":"#000","##,
        r##""command":"claude","envs":[],"isolatedHome":false,"removable":true},
"##,
        r##"        {"key":"claude","label":"Second","description":"d","color":"#000","##,
        r##""command":"claude","envs":[],"isolatedHome":false,"removable":true}
"##,
        r##"    ]"##,
    );
    std::fs::write(&path, manifest_json(agents)).unwrap();

    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].label, "First");
}

#[test]
fn load_honors_valid_empty_agents_list() {
    // A user who removed every built-in has a valid, empty manifest. It is
    // honored verbatim; the embedded default is NOT resurrected.
    let dir = seed_dir();
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, manifest_json("[]")).unwrap();

    assert!(load_catalog(dir.path())
        .expect("persisted catalog")
        .is_empty());
}

#[test]
fn ensure_seeded_writes_when_absent_then_is_idempotent() {
    let dir = seed_dir();
    let path = manifest_path(dir.path());
    assert!(!path.exists());

    ensure_seeded(dir.path(), None);
    assert!(path.exists(), "seed writes the manifest when absent");
    assert_eq!(load_catalog(dir.path()).expect("seeded catalog").len(), 8);

    // Idempotent + never clobbers a user edit: hand-edit to a single custom
    // agent, re-seed, and confirm the edit is preserved.
    let custom = manifest_json(concat!(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","##,
        r##""command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    ));
    std::fs::write(&path, &custom).unwrap();
    ensure_seeded(dir.path(), None);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), custom);
    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].key, "mine");
}

#[test]
fn seeded_manifest_leaves_no_temp_residue() {
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    let catalog = catalog_dir(dir.path());
    let residue: Vec<_> = std::fs::read_dir(&catalog)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.contains(".tmp"))
                .unwrap_or(false)
        })
        .collect();
    assert!(residue.is_empty(), "no leftover temp files after seed");
}

#[test]
fn reseedable_commands_are_claude_codex_opencode() {
    let mut got = reseedable_command_basenames();
    got.sort();
    assert_eq!(got, vec!["claude", "codex", "opencode"]);
}

#[test]
fn every_embedded_master_maps_to_a_catalog_def_with_matching_configseed() {
    let catalog = embedded_default_catalog();
    for m in EMBEDDED_SEED_MASTERS {
        let def = catalog
            .agents
            .iter()
            .find(|d| {
                command_executable_basename(&d.command).as_deref() == Some(m.command_basename)
            })
            .unwrap_or_else(|| panic!("no catalog def for master {}", m.command_basename));
        let cs = def
            .config_seed
            .as_ref()
            .unwrap_or_else(|| panic!("{} def missing configSeed", m.command_basename));
        assert!(cs.enabled);
        assert_eq!(
            cs.dest, m.dest,
            "master dest must match the def configSeed dest"
        );
        assert_eq!(
            def.key, m.key,
            "#1912 master key must match the catalog def key it maps to"
        );
        assert!(
            !m.files.is_empty(),
            "master {} must ship >=1 file",
            m.command_basename
        );
    }
}

#[test]
fn ensure_seeded_masters_creates_nonempty_masters_and_preserves_edits() {
    let dir = seed_dir();
    ensure_seeded_masters(dir.path(), None);
    for cmd in ["claude", "codex", "opencode"] {
        let m = master(cmd);
        let md = master_dir_for_dest(dir.path(), m.dest);
        assert!(
            crate::config::config_seed::is_nonempty_seed_dir(&md),
            "{cmd} master should be non-empty"
        );
        assert_eq!(
            std::fs::read(md.join(m.files[0].rel_path)).unwrap(),
            m.files[0].bytes
        );
    }
    // Idempotent + never clobbers a user edit.
    let m = master("claude");
    let file = master_dir_for_dest(dir.path(), m.dest).join(m.files[0].rel_path);
    std::fs::write(&file, b"USER EDIT").unwrap();
    ensure_seeded_masters(dir.path(), None);
    assert_eq!(std::fs::read(&file).unwrap(), b"USER EDIT");
}

#[test]
fn reseed_installs_embedded_default_and_backs_up_current() {
    let owner = CatalogReseedState::new();
    let dir = seed_dir();
    ensure_seeded_masters(dir.path(), None);
    let m = master("claude");
    let master_dir = master_dir_for_dest(dir.path(), m.dest);
    let file = master_dir.join(m.files[0].rel_path);
    std::fs::write(&file, b"USER EDITED").unwrap();

    let result = reseed_master_for_command(&owner, dir.path(), "claude").unwrap();
    assert_eq!(result.dest, ".claude");
    assert!(!result.backup_path.is_empty());
    // Master restored to the embedded default.
    assert_eq!(std::fs::read(&file).unwrap(), m.files[0].bytes);
    // The `.bak` holds the user's prior edit (verbatim).
    let bak_file = Path::new(&result.backup_path).join(m.files[0].rel_path);
    assert_eq!(std::fs::read(&bak_file).unwrap(), b"USER EDITED");
}

#[test]
fn reseed_when_master_absent_installs_without_backup() {
    let owner = CatalogReseedState::new();
    let dir = seed_dir();
    // Masters not seeded: the _seed dir is absent.
    let m = master("codex");
    let master_dir = master_dir_for_dest(dir.path(), m.dest);
    assert!(!master_dir.exists());

    let result = reseed_master_for_command(&owner, dir.path(), "codex").unwrap();
    assert_eq!(result.dest, ".codex");
    assert!(
        result.backup_path.is_empty(),
        "no backup when master was absent"
    );
    assert_eq!(
        std::fs::read(master_dir.join(m.files[0].rel_path)).unwrap(),
        m.files[0].bytes
    );
}

#[test]
fn reseed_gating_is_exact_basename_never_startswith() {
    let owner = CatalogReseedState::new();
    let dir = seed_dir();
    ensure_seeded_masters(dir.path(), None);
    // `pi` and `agent` are real built-in commands but ship NO master; `pip`
    // and `clau` must not match `pi`/`claude` via any prefix rule; empty and
    // unknown are rejected.
    for bad in ["pi", "agent", "pip", "clau", "claudex", "notacommand", ""] {
        assert!(
            reseed_master_for_command(&owner, dir.path(), bad).is_err(),
            "should reject {bad:?}"
        );
    }
    // Path/extension forms of a real master command still resolve by basename.
    for good in [
        "claude",
        "codex",
        "opencode",
        "claude.exe",
        "codex --model x",
    ] {
        assert!(
            reseed_master_for_command(&owner, dir.path(), good).is_ok(),
            "should accept {good:?}"
        );
    }
}

#[test]
fn master_dir_for_dest_is_under_seed_subdir() {
    let dir = seed_dir();
    let p = master_dir_for_dest(dir.path(), ".claude");
    assert_eq!(
        p,
        dir.path()
            .join("coding-agents")
            .join("_seed")
            .join(".claude")
    );
}

#[test]
fn managed_catalog_migrates_instance_legacy_into_absent_project_base() {
    // #1968: an absent project base imports ONLY the instance agents.json.
    // The ownership transfer keeps the source bytes exactly in the
    // project-local backup, and the instance source itself is never written.
    let project = seed_dir();
    let legacy = legacy_dir();
    let legacy_bytes = legacy_catalog_json();
    std::fs::write(legacy.path().join("agents.json"), &legacy_bytes).unwrap();

    let published = ensure_seeded(project.path(), Some(legacy.path()));
    assert!(published.is_some(), "a first migration publishes the base");

    let catalog = catalog_dir(project.path());
    assert_eq!(
        std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
        legacy_bytes,
        "the backup keeps the source bytes exactly"
    );
    let base: serde_json::Value =
        serde_json::from_slice(&std::fs::read(manifest_path(project.path())).unwrap()).unwrap();
    assert_eq!(base["managed"]["owner"], "agentscommander");
    assert_eq!(base["managed"]["version"], 1);
    assert_eq!(base["agents"].as_array().unwrap().len(), 8);

    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(catalog.join(MIGRATION_JOURNAL_FILENAME)).unwrap())
            .unwrap();
    assert_eq!(journal["sourceKind"], "instance");
    assert_eq!(
        journal["sourcePath"],
        legacy
            .path()
            .join("agents.10.default.json")
            .display()
            .to_string()
    );
    assert_eq!(
        journal["managedBase"]["managed"]["revision"],
        base["managed"]["revision"]
    );

    // Extracted local layer: the custom entry is complete, the eight shipped
    // keys are tombstoned, and the legacy order is pinned.
    let local: serde_json::Value =
        serde_json::from_slice(&std::fs::read(local_catalog_path(project.path())).unwrap())
            .unwrap();
    assert_eq!(local["order"], serde_json::json!(["mine"]));
    let rows = local["agents"].as_array().unwrap();
    assert!(rows
        .iter()
        .any(|row| row["key"] == "mine" && row["command"] == "mytool"));
    assert_eq!(rows.iter().filter(|row| row["remove"] == true).count(), 8);

    let loaded = load_catalog(project.path()).expect("migrated catalog");
    assert_eq!(keys_of(&loaded), ["mine"]);

    // Completed migration is idempotent: no re-extraction, no rewrite.
    let base_before = std::fs::read(manifest_path(project.path())).unwrap();
    let local_before = std::fs::read(local_catalog_path(project.path())).unwrap();
    assert!(ensure_seeded(project.path(), Some(legacy.path())).is_none());
    assert_eq!(
        std::fs::read(manifest_path(project.path())).unwrap(),
        base_before
    );
    assert_eq!(
        std::fs::read(local_catalog_path(project.path())).unwrap(),
        local_before
    );
    assert_eq!(
        std::fs::read(legacy.path().join("agents.10.default.json")).unwrap(),
        legacy_bytes,
        "the instance source bytes survive the rename, never rewritten"
    );
    assert!(!legacy.path().join("agents.json").exists());
}

#[test]
fn reseed_owner_serializes_two_callers_until_release() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    let owner = Arc::new(CatalogReseedState::new());
    let active = Arc::new(AtomicBool::new(false));
    let bound = Duration::from_secs(10);
    let (holding_tx, holding_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (first_done_tx, first_done_rx) = mpsc::channel();
    let first_owner = owner.clone();
    let first_active = active.clone();
    let first = std::thread::spawn(move || {
        let result = first_owner.with_reseed(|| {
            assert!(!first_active.swap(true, Ordering::SeqCst));
            holding_tx.send(()).unwrap();
            release_rx
                .recv_timeout(bound)
                .expect("first caller released");
            first_active.store(false, Ordering::SeqCst);
            Ok(())
        });
        first_done_tx.send(result).unwrap();
    });
    holding_rx
        .recv_timeout(bound)
        .expect("first caller entered");
    let (attempt_tx, attempt_rx) = mpsc::channel();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (second_done_tx, second_done_rx) = mpsc::channel();
    let second = std::thread::spawn(move || {
        attempt_tx.send(()).unwrap();
        let result = owner.with_reseed(|| {
            assert!(
                !active.swap(true, Ordering::SeqCst),
                "reseed callers overlap"
            );
            entered_tx.send(()).unwrap();
            active.store(false, Ordering::SeqCst);
            Ok(())
        });
        second_done_tx.send(result).unwrap();
    });
    attempt_rx
        .recv_timeout(bound)
        .expect("second caller attempted entry");
    // A broken-lock detection during this negative wait depends on scheduling.
    let blocked = entered_rx.recv_timeout(Duration::from_millis(100));
    release_tx.send(()).unwrap();
    assert!(matches!(blocked, Err(mpsc::RecvTimeoutError::Timeout)));
    entered_rx
        .recv_timeout(bound)
        .expect("second caller enters after release");
    first_done_rx
        .recv_timeout(bound)
        .expect("first caller completed")
        .unwrap();
    second_done_rx
        .recv_timeout(bound)
        .expect("second caller completed")
        .unwrap();
    first.join().unwrap();
    second.join().unwrap();
}

#[test]
fn poisoned_reseed_owner_refuses_without_changing_master_or_backups() {
    let owner = std::sync::Arc::new(CatalogReseedState::new());
    let dir = seed_dir();
    ensure_seeded_masters(dir.path(), None);
    let m = master("claude");
    let master_dir = master_dir_for_dest(dir.path(), m.dest);
    let file = master_dir.join(m.files[0].rel_path);
    std::fs::write(&file, b"KEEP USER MASTER").unwrap();
    let before: std::collections::BTreeSet<_> = std::fs::read_dir(master_dir.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    let unsupported = reseed_master_for_command(&owner, dir.path(), "notacommand").unwrap_err();
    assert!(std::panic::catch_unwind(|| {
        let _: Result<(), String> = owner.with_reseed(|| panic!("poison reseed owner"));
    })
    .is_err());
    assert_eq!(
        reseed_master_for_command(&owner, dir.path(), "claude").unwrap_err(),
        "re-seed lock poisoned"
    );
    assert_eq!(
        reseed_master_for_command(&owner, dir.path(), "notacommand").unwrap_err(),
        unsupported
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"KEEP USER MASTER");
    let after: std::collections::BTreeSet<_> = std::fs::read_dir(master_dir.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        after, before,
        "poison must not create backup/staging/trash paths"
    );
}
