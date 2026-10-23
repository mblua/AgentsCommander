use super::*;

#[test]
fn absolutise_resolves_relative_path_against_cwd() {
    let _cwd_lock = cwd_lock();
    let fix = FixtureRoot::new("proj-rel");
    std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
    let prev = std::env::current_dir().unwrap();
    let _guard = CwdGuard(prev);
    std::env::set_current_dir(fix.path()).unwrap();
    let mut s = AppSettings::default();
    let r = register_existing_project(&mut s, ".").unwrap();
    // Persisted path must be absolute and equal to fix.path() lexically
    // (after `std::path::absolute(".")` collapses the trailing `.`).
    assert!(Path::new(&r.path).is_absolute(), "not absolute: {}", r.path);
    let normalized_persisted = r.path.replace('\\', "/").to_lowercase();
    let normalized_fixture = fix
        .path()
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase();
    assert_eq!(normalized_persisted, normalized_fixture);
}

/// `..` collapse is Windows-only behaviour of `std::path::absolute`
/// (POSIX preserves `..` for symlink-safety). On Windows the persisted
/// path must contain no `..` component.
#[cfg(windows)]
#[test]
fn absolutise_collapses_dotdot_segments_on_windows() {
    let _cwd_lock = cwd_lock();
    let fix = FixtureRoot::new("proj-dotdot");
    let project = fix.path().join("project");
    std::fs::create_dir_all(project.join(".ac")).unwrap();
    let sibling = fix.path().join("sibling");
    std::fs::create_dir_all(&sibling).unwrap();
    let prev = std::env::current_dir().unwrap();
    let _guard = CwdGuard(prev);
    let sibling = std::fs::canonicalize(&sibling).unwrap();
    std::env::set_current_dir(&sibling).unwrap();
    let mut s = AppSettings::default();
    let r = register_existing_project(&mut s, "..\\project").unwrap();
    assert!(
        !r.path.contains(".."),
        "persisted path should not contain `..` on Windows: {}",
        r.path
    );
}

// ── trailing-separator dedup (Round-1 IR.3.2) ────────────────────────

#[test]
fn upsert_dedup_strips_trailing_separator() {
    let fix = FixtureRoot::new("proj-trailing");
    std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
    let mut s = AppSettings::default();
    let original = fix.path().to_string_lossy().to_string();
    let _ = register_existing_project(&mut s, &original).unwrap();
    // Add `\` (or `/` on POSIX) to simulate shell tab-completion.
    let with_trailing = if cfg!(windows) {
        format!("{}\\", original)
    } else {
        format!("{}/", original)
    };
    let r2 = register_existing_project(&mut s, &with_trailing).unwrap();
    assert!(
        !r2.registered,
        "trailing-separator variant should dedup: {} vs {}",
        original, with_trailing
    );
    assert_eq!(s.project_paths.len(), 1);
}

// ── resolve_project_reference ───────────────────────────────────────

#[test]
fn resolve_project_reference_matches_registered_folder_name() {
    let fix = FixtureRoot::new("proj-resolve-name");
    let project = fix.path().join("ProjectAlpha");
    create_ac_project(&project);
    let project_paths = vec![project.to_string_lossy().to_string()];

    let resolved = resolve_project_reference(&project_paths, "ProjectAlpha").unwrap();

    assert_eq!(resolved.path, project);
    assert_eq!(resolved.folder_name, "ProjectAlpha");
    assert!(resolved.registered);
}

#[test]
fn resolve_project_reference_matches_child_project_from_registered_parent() {
    let fix = FixtureRoot::new("proj-resolve-parent");
    let project = fix.path().join("ProjectAlpha");
    create_ac_project(&project);
    let project_paths = vec![fix.path().to_string_lossy().to_string()];

    let resolved = resolve_project_reference(&project_paths, "ProjectAlpha").unwrap();

    assert_eq!(resolved.path, project);
}

#[test]
fn resolve_project_reference_rejects_non_ac_child_project() {
    let fix = FixtureRoot::new("proj-resolve-invalid-workspace");
    let project = fix.path().join("ProjectAlpha");
    std::fs::create_dir_all(project.join(".workspace")).unwrap();
    let project_paths = vec![fix.path().to_string_lossy().to_string()];

    let err = resolve_project_reference(&project_paths, "ProjectAlpha").unwrap_err();

    assert!(matches!(err, ProjectResolveError::NotFound(_)));
}

#[test]
fn resolve_project_reference_rejects_ambiguous_folder_name() {
    let fix = FixtureRoot::new("proj-resolve-ambiguous");
    let parent_a = fix.path().join("a");
    let parent_b = fix.path().join("b");
    let project_a = parent_a.join("ProjectAlpha");
    let project_b = parent_b.join("ProjectAlpha");
    create_ac_project(&project_a);
    create_ac_project(&project_b);
    let project_paths = vec![
        parent_a.to_string_lossy().to_string(),
        parent_b.to_string_lossy().to_string(),
    ];

    let err = resolve_project_reference(&project_paths, "ProjectAlpha").unwrap_err();

    match err {
        ProjectResolveError::Ambiguous { candidates, .. } => {
            assert_eq!(candidates.len(), 2);
            assert!(candidates.iter().any(|p| p.contains("ProjectAlpha")));
        }
        other => panic!("expected ambiguous error, got {other:?}"),
    }
}

#[test]
fn resolve_project_reference_rejects_unregistered_direct_path() {
    let fix = FixtureRoot::new("proj-resolve-unregistered");
    let project = fix.path().join("ProjectAlpha");
    create_ac_project(&project);

    let err = resolve_project_reference(&[], &project.to_string_lossy()).unwrap_err();

    assert!(matches!(err, ProjectResolveError::NotFound(_)));
}

#[test]
fn resolve_project_reference_rejects_registered_direct_path_input() {
    let fix = FixtureRoot::new("proj-resolve-direct");
    let project = fix.path().join("ProjectAlpha");
    create_ac_project(&project);
    let project_paths = vec![project.to_string_lossy().to_string()];

    let err = resolve_project_reference(&project_paths, &project.to_string_lossy()).unwrap_err();

    assert!(matches!(err, ProjectResolveError::NotFound(_)));
}

#[test]
fn resolve_project_reference_skips_dot_prefixed_child_projects() {
    let fix = FixtureRoot::new("proj-resolve-dot-child");
    let hidden = fix.path().join(".HiddenProject");
    create_ac_project(&hidden);
    let project_paths = vec![fix.path().to_string_lossy().to_string()];

    let err = resolve_project_reference(&project_paths, ".HiddenProject").unwrap_err();

    assert!(matches!(err, ProjectResolveError::NotFound(_)));
}

#[test]
fn enumerate_registered_project_candidates_yields_a_nested_child_of_a_registered_parent() {
    let fix = FixtureRoot::new("proj-enumerate-nested");
    create_ac_project(fix.path());
    let child = fix.path().join("child");
    create_ac_project(&child);
    let project_paths = vec![fix.path().to_string_lossy().to_string()];

    let candidates = enumerate_registered_project_candidates(&project_paths);

    assert!(
        candidates.iter().any(|c| c.path == child),
        "nested child with .ac must remain a project candidate"
    );
}

// ── serde camelCase shape lock (Round-1 G14) ─────────────────────────

#[test]
fn project_registration_serializes_camel_case() {
    // Today's fields are already lowercase single-words, so no rename
    // happens. This test locks the invariant: a future field like
    // `ac_root` must serialize to `acRoot`. If the
    // `#[serde(rename_all = "camelCase")]` attribute is ever dropped,
    // this test catches it before the FE silently breaks.
    let r = ProjectRegistration {
        path: "X".to_string(),
        registered: true,
        created: false,
    };
    let json = serde_json::to_string(&r).unwrap();
    assert!(json.contains("\"path\""), "missing path field: {}", json);
    assert!(
        json.contains("\"registered\""),
        "missing registered field: {}",
        json
    );
    assert!(
        json.contains("\"created\""),
        "missing created field: {}",
        json
    );
    // No snake_case relics from any current field name.
    assert!(
        !json.contains("ac_root"),
        "snake_case field name leaked: {}",
        json
    );

    let archived = ArchivedProject {
        path: "X".to_string(),
        folder_name: "X".to_string(),
        exists: true,
        has_workspace: false,
    };
    let json = serde_json::to_string(&archived).unwrap();
    assert!(
        json.contains("\"folderName\""),
        "missing folderName field: {}",
        json
    );
    assert!(
        json.contains("\"hasWorkspace\""),
        "missing hasWorkspace field: {}",
        json
    );
    assert!(
        !json.contains("folder_name") && !json.contains("has_workspace"),
        "snake_case archived field leaked: {}",
        json
    );
}
