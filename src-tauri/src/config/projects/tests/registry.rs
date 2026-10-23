use super::*;

#[test]
fn upsert_syncs_legacy_project_path_field() {
    let fix1 = FixtureRoot::new("proj-legacy-1");
    let fix2 = FixtureRoot::new("proj-legacy-2");
    std::fs::create_dir_all(fix1.path().join(".ac")).unwrap();
    std::fs::create_dir_all(fix2.path().join(".ac")).unwrap();
    let mut s = AppSettings::default();
    let r1 = register_existing_project(&mut s, fix1.path().to_str().unwrap()).unwrap();
    assert_eq!(s.project_path.as_deref(), Some(r1.path.as_str()));
    let r2 = register_existing_project(&mut s, fix2.path().to_str().unwrap()).unwrap();
    // project_path tracks the HEAD of the list (same as FE persistProjectPaths).
    assert_eq!(s.project_path.as_deref(), Some(r1.path.as_str()));
    assert_eq!(s.project_paths, vec![r1.path.clone(), r2.path.clone()]);
}

// ── #778 remove_project_path (inverse of upsert) ──────────────────────

#[test]
fn remove_project_path_removes_matching_entry_and_rederives_head() {
    let mut s = AppSettings {
        project_paths: vec!["C:/a".to_string(), "C:/b".to_string()],
        project_path: Some("C:/a".to_string()),
        ..AppSettings::default()
    };
    assert!(remove_project_path(&mut s, "C:/a"));
    assert_eq!(s.project_paths, vec!["C:/b".to_string()]);
    assert_eq!(s.project_path.as_deref(), Some("C:/b"));
}

#[test]
fn remove_project_path_last_entry_clears_head_to_none() {
    let mut s = AppSettings {
        project_paths: vec!["C:/only".to_string()],
        project_path: Some("C:/only".to_string()),
        ..AppSettings::default()
    };
    assert!(remove_project_path(&mut s, "C:/only"));
    assert!(s.project_paths.is_empty());
    assert_eq!(s.project_path, None);
}

#[test]
fn remove_project_path_is_case_slash_insensitive_and_preserves_byte_form() {
    // Stored byte-form uses backslashes + mixed case; remove via a normalized variant.
    let mut s = AppSettings {
        project_paths: vec![r"C:\Foo\".to_string(), "C:/keep".to_string()],
        project_path: Some(r"C:\Foo\".to_string()),
        ..AppSettings::default()
    };
    assert!(remove_project_path(&mut s, "c:/foo"));
    // Only C:\Foo\ removed; the surviving entry keeps its original byte-form.
    assert_eq!(s.project_paths, vec!["C:/keep".to_string()]);
    assert_eq!(s.project_path.as_deref(), Some("C:/keep"));
}

#[test]
fn remove_project_path_no_match_returns_false_and_keeps_list() {
    let mut s = AppSettings {
        project_paths: vec!["C:/a".to_string()],
        project_path: Some("C:/a".to_string()),
        ..AppSettings::default()
    };
    assert!(!remove_project_path(&mut s, "C:/nope"));
    assert_eq!(s.project_paths, vec!["C:/a".to_string()]);
    assert_eq!(s.project_path.as_deref(), Some("C:/a"));
}

#[test]
fn remove_project_path_also_drops_archived_entry() {
    let mut s = AppSettings {
        project_paths: vec!["C:/a".to_string()],
        project_path: Some("C:/a".to_string()),
        archived_project_paths: vec!["c:/a/".to_string(), "C:/b".to_string()],
        ..AppSettings::default()
    };

    assert!(remove_project_path(&mut s, "C:/a"));

    assert_eq!(s.archived_project_paths, vec!["C:/b".to_string()]);
}

// ── #881 archive_project_path ───────────────────────────────────────

#[test]
fn archive_project_path_moves_entry_and_rederives_head() {
    let mut s = AppSettings {
        project_paths: vec!["C:/a".to_string(), "C:/b".to_string()],
        project_path: Some("C:/a".to_string()),
        ..AppSettings::default()
    };

    assert!(archive_project_path(&mut s, "C:/a"));

    assert_eq!(s.project_paths, vec!["C:/b".to_string()]);
    assert_eq!(s.archived_project_paths, vec!["C:/a".to_string()]);
    assert_eq!(s.project_path.as_deref(), Some("C:/b"));
}

#[test]
fn archive_project_path_last_entry_clears_head_to_none() {
    let mut s = AppSettings {
        project_paths: vec!["C:/only".to_string()],
        project_path: Some("C:/only".to_string()),
        ..AppSettings::default()
    };

    assert!(archive_project_path(&mut s, "C:/only"));

    assert!(s.project_paths.is_empty());
    assert_eq!(s.archived_project_paths, vec!["C:/only".to_string()]);
    assert_eq!(s.project_path, None);
}

#[test]
fn archive_project_path_is_case_slash_insensitive_and_archives_stored_byte_form() {
    let mut s = AppSettings {
        project_paths: vec![r"C:\Foo\".to_string(), "C:/keep".to_string()],
        project_path: Some(r"C:\Foo\".to_string()),
        ..AppSettings::default()
    };

    assert!(archive_project_path(&mut s, "c:/foo"));

    assert_eq!(s.project_paths, vec!["C:/keep".to_string()]);
    assert_eq!(s.archived_project_paths, vec![r"C:\Foo\".to_string()]);
}

#[test]
fn archive_project_path_is_idempotent() {
    let mut s = AppSettings {
        project_paths: vec!["C:/a".to_string()],
        project_path: Some("C:/a".to_string()),
        ..AppSettings::default()
    };

    assert!(archive_project_path(&mut s, "C:/a"));
    assert!(!archive_project_path(&mut s, "c:/a/"));

    assert_eq!(s.archived_project_paths, vec!["C:/a".to_string()]);
}

#[test]
fn archive_project_path_records_unregistered_path() {
    let mut s = AppSettings::default();

    assert!(!archive_project_path(&mut s, "C:/missing"));

    assert_eq!(s.archived_project_paths, vec!["C:/missing".to_string()]);
    assert!(s.project_paths.is_empty());
}

#[test]
fn unarchive_project_path_restores_archived_byte_form_without_io() {
    let mut s = AppSettings {
        archived_project_paths: vec!["C:/MissingProject/".to_string()],
        ..AppSettings::default()
    };

    assert!(unarchive_project_path(&mut s, "C:/MissingProject"));

    assert_eq!(s.project_paths, vec!["C:/MissingProject/".to_string()]);
    assert!(s.archived_project_paths.is_empty());
    assert_eq!(s.project_path.as_deref(), Some("C:/MissingProject/"));
}

#[test]
fn upsert_project_path_unarchives_matching_entry() {
    let fix = FixtureRoot::new("proj-unarchive-upsert");
    std::fs::create_dir_all(fix.path().join(".ac")).expect("create .ac");
    let mut s = AppSettings {
        archived_project_paths: vec![fix.path().to_string_lossy().replace('\\', "/") + "/"],
        ..AppSettings::default()
    };

    let result =
        register_existing_project(&mut s, fix.path().to_str().unwrap()).expect("register existing");

    assert!(result.registered);
    assert!(s.archived_project_paths.is_empty());
    assert_eq!(s.project_paths, vec![result.path]);
}

#[cfg(windows)]
#[test]
fn upsert_project_path_unarchives_windows_verbatim_variant() {
    let mut s = AppSettings {
        archived_project_paths: vec![r"\\?\C:\Users\maria\MixedCaseProject".to_string()],
        ..AppSettings::default()
    };

    assert!(upsert_project_path(
        &mut s,
        r"C:\Users\maria\MixedCaseProject"
    ));

    assert_eq!(
        s.project_paths,
        vec![r"C:\Users\maria\MixedCaseProject".to_string()]
    );
    assert!(
        s.archived_project_paths.is_empty(),
        "verbatim and ordinary Windows paths must not remain in separate lists"
    );
}

#[test]
fn archived_projects_reports_missing_folder_and_missing_ac_root() {
    let full = FixtureRoot::new("proj-archived-full");
    std::fs::create_dir_all(full.path().join(".ac")).expect("full .ac");
    let no_ac_root = FixtureRoot::new("proj-archived-no-workspace");
    let deleted = FixtureRoot::new("proj-archived-deleted");
    let deleted_path = deleted.path().to_string_lossy().to_string();
    std::fs::remove_dir_all(deleted.path()).expect("delete fixture");

    let rows = archived_projects_from_paths(&[
        full.path().to_string_lossy().to_string(),
        no_ac_root.path().to_string_lossy().to_string(),
        deleted_path,
    ]);

    assert!(rows[0].exists);
    assert!(rows[0].has_workspace);
    assert!(rows[1].exists);
    assert!(!rows[1].has_workspace);
    assert!(!rows[2].exists);
    assert!(!rows[2].has_workspace);
}

// ── absolutise: relative + dot-dot collapse (Round-1 G4 + G13) ────────
