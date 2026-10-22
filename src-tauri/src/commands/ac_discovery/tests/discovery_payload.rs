use super::*;

// ── extract_task_first_line — issue #161 ──

#[test]
fn task_no_frontmatter_with_heading() {
    assert_eq!(
        extract_task_first_line("# My Brief\n\nbody"),
        Some("My Brief".to_string())
    );
}

#[test]
fn task_no_frontmatter_plain() {
    assert_eq!(
        extract_task_first_line("My Brief\nbody"),
        Some("My Brief".to_string())
    );
}

#[test]
fn task_with_yaml_frontmatter() {
    let content = "---\ntitle: x\nauthor: y\n---\n# Real Title\nbody";
    assert_eq!(
        extract_task_first_line(content),
        Some("Real Title".to_string())
    );
}

#[test]
fn task_with_frontmatter_then_blank_lines() {
    let content = "---\ntitle: x\n---\n\n\n# Real Title";
    assert_eq!(
        extract_task_first_line(content),
        Some("Real Title".to_string())
    );
}

#[test]
fn task_empty_file() {
    assert_eq!(extract_task_first_line(""), None);
}

#[test]
fn task_only_frontmatter_no_body() {
    assert_eq!(extract_task_first_line("---\nfoo: bar\n---\n"), None);
}

#[test]
fn task_unclosed_frontmatter_returns_none() {
    // Pathological: opener with no closer drains the iterator.
    assert_eq!(extract_task_first_line("---\nfoo: bar\nno closer"), None);
}

#[test]
fn task_leading_blank_no_frontmatter() {
    assert_eq!(
        extract_task_first_line("\n\n# Title"),
        Some("Title".to_string())
    );
}

#[test]
fn task_frontmatter_delimiter_tolerates_whitespace() {
    // `---` lines may carry trailing/leading whitespace from editors.
    let content = "--- \ntitle: x\n  ---  \n# Body";
    assert_eq!(extract_task_first_line(content), Some("Body".to_string()));
}

#[test]
fn task_with_bom_no_frontmatter() {
    // Editors (notably Notepad) save UTF-8 with a BOM. Without the strip,
    // `trim_start_matches("# ")` leaves the BOM on the heading text.
    let content = "\u{FEFF}# My Brief\n\nbody";
    assert_eq!(
        extract_task_first_line(content),
        Some("My Brief".to_string())
    );
}

#[test]
fn task_with_bom_and_frontmatter() {
    // BOM in front of the `---` opener used to make the frontmatter check
    // fail (because `\u{FEFF}---` != `---`), exposing the literal frontmatter.
    let content = "\u{FEFF}---\ntitle: x\n---\n# Real Title\nbody";
    assert_eq!(
        extract_task_first_line(content),
        Some("Real Title".to_string())
    );
}

#[test]
fn task_leading_blanks_before_frontmatter() {
    // Regression: leading blank lines before `---` used to bypass the
    // frontmatter check and leak the literal `---` into the sidebar.
    let content = "\n\n---\ntitle: x\n---\n# Body";
    assert_eq!(extract_task_first_line(content), Some("Body".to_string()));
}

#[test]
fn task_leading_blanks_no_frontmatter() {
    let content = "\n\n# Body line";
    assert_eq!(
        extract_task_first_line(content),
        Some("Body line".to_string())
    );
}

#[test]
fn invalid_replica_identity_is_not_reported_as_repaired_discovery_identity() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("AgentsCommander_ac");
    let ac_root = project.join(".ac");
    let team_dir = ac_root.join("_team_dev-team");
    let matrix_dir = ac_root.join("_agent_tech-lead");
    let wg_dir = ac_root.join("wg-2-dev-team");
    let replica_dir = wg_dir.join("__agent_tech-lead");

    for dir in [&team_dir, &matrix_dir, &replica_dir] {
        std::fs::create_dir_all(dir).expect("create fixture dir");
    }
    std::fs::write(
        team_dir.join("config.json"),
        r#"{"agents":["../_agent_tech-lead"],"coordinator":"../_agent_tech-lead"}"#,
    )
    .expect("write team config");
    std::fs::write(
        replica_dir.join("config.json"),
        r#"{"identity":"tech-lead"}"#,
    )
    .expect("write invalid replica config");

    let identity_read = read_replica_config_with_valid_identity(&replica_dir);

    assert!(identity_read.invalid_identity);
    assert!(identity_read.config.is_some());
    assert!(
        identity_read.identity.is_none(),
        "invalid persisted identity must not become a repaired identity path"
    );
    assert_eq!(
        origin_project_for_replica_identity("AgentsCommander_ac", &identity_read),
        None,
        "invalid identity must not be masked as the local origin project"
    );
    assert!(
        crate::config::teams::resolve_wg_coordinator_replica(&ac_root, &wg_dir).is_none(),
        "invalid identity must not grant coordinator/root authority"
    );

    let preferred_agent_id = identity_read.identity.as_ref().and_then(|identity| {
        read_preferred_agent_id(
            &identity.matrix_dir,
            &crate::config::settings::AppSettings::default(),
        )
    });
    assert!(
        preferred_agent_id.is_none(),
        "invalid identity must not be used to read preferred coding agent"
    );
}

// --- #943 Module B2: the widened ac_discovery_branch_updated payload ---

/// Repo tuple: (path, branch, dirty). #1028 added the third column.
fn b2_payload(
    branch: Option<&str>,
    repos: &[(&str, Option<&str>, Option<bool>)],
) -> DiscoveryBranchPayload {
    DiscoveryBranchPayload {
        replica_path: "C:/proj/.ac/wg-1-team/__agent_coord".to_string(),
        branch: branch.map(str::to_string),
        repo_branches: repos
            .iter()
            .map(|(_, b, _)| b.map(str::to_string))
            .collect(),
        repo_paths: repos.iter().map(|(p, _, _)| p.to_string()).collect(),
        repo_dirty: repos.iter().map(|(_, _, d)| *d).collect(),
    }
}

/// The wire contract the frontend codes against. `#[serde(rename_all =
/// "camelCase")]` must produce exactly these five keys, `None` must serialize
/// as `null` (not omitted: the consumer distinguishes "unknown branch" from
/// "missing entry"), and `repoBranches[i]` / `repoDirty[i]` must line up with
/// `repoPaths[i]`.
///
/// #1028: this is the one test that can catch a `repo_dirty` vs `repoDirty` wire
/// name mismatch. A frontend test cannot: its payload is written to match the TS
/// interface, so if Rust emitted `repo_dirty` the frontend test would still pass
/// and every badge in production would go permanently violet.
#[test]
fn discovery_branch_payload_wire_shape_is_camel_case_with_explicit_nulls() {
    let payload = b2_payload(
        None,
        &[
            (
                "C:/wg/repo-AgentsCommander",
                Some("feature/943"),
                Some(true),
            ),
            ("C:/wg/repo-webpage", None, None),
        ],
    );

    let value = serde_json::to_value(&payload).expect("serialize");

    assert_eq!(
        value,
        serde_json::json!({
            "replicaPath": "C:/proj/.ac/wg-1-team/__agent_coord",
            "branch": null,
            "repoBranches": ["feature/943", null],
            "repoPaths": ["C:/wg/repo-AgentsCommander", "C:/wg/repo-webpage"],
            "repoDirty": [true, null],
        })
    );
}

/// The single-repo shorthand is untouched by B2: one repo still fills `branch`,
/// and `repo_branches` carries the same value so a consumer can read either.
#[test]
fn discovery_branch_payload_keeps_the_single_repo_shorthand() {
    let payload = b2_payload(Some("main"), &[("C:/wg/repo-solo", Some("main"), None)]);
    let value = serde_json::to_value(&payload).expect("serialize");

    assert_eq!(value["branch"], serde_json::json!("main"));
    assert_eq!(value["repoBranches"], serde_json::json!(["main"]));
}

/// The emission gate is `cache.get(path) != Some(&payload)`, so payload equality
/// IS the gate. This is the case B2 exists for: a multi-repo replica has
/// `branch: None` forever, so the pre-B2 gate (which compared only `branch`)
/// saw None == None and never re-emitted. A repo changing branch must now fire.
#[test]
fn discovery_branch_payload_gate_fires_on_per_repo_drift() {
    let before = b2_payload(
        None,
        &[
            ("C:/wg/repo-a", Some("main"), None),
            ("C:/wg/repo-b", Some("main"), None),
        ],
    );
    let after = b2_payload(
        None,
        &[
            ("C:/wg/repo-a", Some("feature/943"), None),
            ("C:/wg/repo-b", Some("main"), None),
        ],
    );

    assert_eq!(before.branch, after.branch, "multi-repo: both stay None");
    assert_ne!(before, after, "per-repo drift must re-emit");
}

/// #1028 - a dirty flip with every branch identical must re-emit, or the badge
/// never turns red on a cold row. `PartialEq` over the whole payload is what
/// delivers this, which is why `repo_dirty` had to join the payload rather than
/// ride a separate event: no gate change was needed, but the field must be IN the
/// compared value.
#[test]
fn discovery_branch_payload_gate_fires_on_dirty_flip() {
    let clean = b2_payload(
        None,
        &[
            ("C:/wg/repo-a", Some("main"), Some(false)),
            ("C:/wg/repo-b", Some("main"), Some(false)),
        ],
    );
    let dirtied = b2_payload(
        None,
        &[
            ("C:/wg/repo-a", Some("main"), Some(true)),
            ("C:/wg/repo-b", Some("main"), Some(false)),
        ],
    );
    let unknown = b2_payload(
        None,
        &[
            ("C:/wg/repo-a", Some("main"), None),
            ("C:/wg/repo-b", Some("main"), Some(false)),
        ],
    );

    assert_eq!(
        clean.repo_branches, dirtied.repo_branches,
        "only dirty moved"
    );
    assert_ne!(clean, dirtied, "clean -> dirty must re-emit");
    assert_ne!(dirtied, unknown, "dirty -> unknown must re-emit");
    assert_ne!(clean, unknown, "false and null are distinct on the wire");
}

/// #2064 - the room map travels with the path vector, from the same map, in
/// the same guard-released window. The room dir is the parent of the replica
/// path, and the existing path push is unchanged.
#[test]
fn with_replicas_mut_pushes_repo_rooms_alongside_paths() {
    let _guard = crate::pty::git_watcher::DISCOVERY_TEST_LOCK.blocking_lock();
    let (app, _settings, _receiver, session_mgr, _pty_mgr) =
        archive_command_app(AppSettings::default());
    let watcher = DiscoveryBranchWatcher::new(app.handle().clone(), Arc::clone(&session_mgr));

    let replica_path = "C:/proj/.ac/wg-1-team/__agent_coord";
    let repos = vec![
        ("A".to_string(), "C:/wg/repo-AgentsCommander".to_string()),
        ("B".to_string(), "C:/wg/repo-webpage".to_string()),
    ];
    watcher.with_replicas_mut(|map| {
        map.entry("C:/proj".to_string())
            .or_default()
            .push(ReplicaBranchEntry {
                replica_path: replica_path.to_string(),
                repos: repos.clone(),
                session_name: "wg-1-team/__agent_coord".to_string(),
            });
    });

    assert_eq!(
        crate::pty::git_watcher::discovery_repo_paths(),
        vec![
            "C:/wg/repo-AgentsCommander".to_string(),
            "C:/wg/repo-webpage".to_string(),
        ],
        "the existing path push is unchanged"
    );
    assert_eq!(
        crate::pty::git_watcher::discovery_repo_rooms(),
        vec![
            (
                "C:/wg/repo-AgentsCommander".to_string(),
                "C:/proj/.ac/wg-1-team".to_string(),
            ),
            (
                "C:/wg/repo-webpage".to_string(),
                "C:/proj/.ac/wg-1-team".to_string(),
            ),
        ],
        "the room map is the parent of the replica path"
    );
}

/// A repo-set change with identical branch text must also fire: swapping a repo
/// for another one that happens to sit on `main` leaves the branch vec equal, and
/// gating on branches alone would leave the consumer holding the previous repo
/// list. Pinning this is why `repo_paths` is part of the payload and the gate.
#[test]
fn discovery_branch_payload_gate_fires_on_repo_set_change() {
    let before = b2_payload(
        None,
        &[
            ("C:/wg/repo-a", Some("main"), None),
            ("C:/wg/repo-b", Some("main"), None),
        ],
    );
    let swapped = b2_payload(
        None,
        &[
            ("C:/wg/repo-a", Some("main"), None),
            ("C:/wg/repo-c", Some("main"), None),
        ],
    );
    let reordered = b2_payload(
        None,
        &[
            ("C:/wg/repo-b", Some("main"), None),
            ("C:/wg/repo-a", Some("main"), None),
        ],
    );

    assert_eq!(before.repo_branches, swapped.repo_branches);
    assert_ne!(before, swapped, "a swapped repo must re-emit");
    assert_ne!(before, reordered, "a reordered repo list must re-emit");
}
