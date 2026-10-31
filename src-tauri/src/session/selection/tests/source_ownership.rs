use super::*;

fn production_prefix(source: &str) -> &str {
    [
        "\n#[cfg(test)]\nmod tests",
        "\n#[cfg(test)]\nimpl SessionManager",
    ]
    .into_iter()
    .filter_map(|marker| source.find(marker))
    .min()
    .map_or(source, |index| &source[..index])
}

fn function_name(line: &str) -> Option<String> {
    let start = line.find("fn ")? + 3;
    let name = line[start..]
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
        .collect::<String>();
    (!name.is_empty()).then_some(name)
}

/// #1149 - the only functions allowed to write `Session::status` in
/// production, alongside the lifecycle helper in manager/commit.rs. Each one
/// either records the working-state edge it causes or is
/// covered by the heartbeat backstop; an assignment anywhere else silently
/// reintroduces an unrecorded edge.
const STATUS_WRITER_OWNERS: [&str; 5] = [
    "mark_idle_with_carrier_permit",
    "mark_busy_with_carrier_permit",
    "prepare_pty_input_boundary_with_carrier_permit",
    "commit_selection_transition_with_carrier_permit",
    "remove_exited_sessions_with_carrier_permit",
];

fn ownership_violations(files: &[(String, String)]) -> Vec<String> {
    let mut violations = Vec::new();
    for (path, source) in files {
        let mut function = String::new();
        let normalized = source.replace("\r\n", "\n");
        for (index, line) in production_prefix(&normalized).lines().enumerate() {
            if let Some(name) = function_name(line) {
                function = name;
            }
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            // #1149 - all four variants, not only `Active`. The birth-edge
            // writer, `record.status = SessionStatus::Running` in
            // `commit_selection_transition`, was unguarded, so a future
            // assignment of exactly that shape would have passed CI and
            // silently reintroduced the unrecorded-birth defect. The guard's
            // reach ends where `production_prefix` cuts, so it protects the
            // writer table only above the `#[cfg(test)] impl SessionManager`
            // marker, which is exactly what A12 complements.
            for variant in ["Active", "Running", "Idle", "Exited"] {
                if line.contains(&format!("= SessionStatus::{variant}"))
                    && !(path.ends_with("session/manager.rs")
                        && STATUS_WRITER_OWNERS.contains(&function.as_str())
                        || path.ends_with("session/manager/commit.rs")
                            && function == "apply_lifecycle_mutations")
                {
                    violations.push(format!(
                        "{path}:{} {variant} assignment in {function}",
                        index + 1
                    ));
                }
            }

            for event in [
                "session_switched",
                "session_created",
                "session_destroyed",
                "session_communication_changed",
            ] {
                if !line.contains(&format!("\"{event}\"")) {
                    continue;
                }
                let allowed = path.ends_with("session/selection.rs")
                    && matches!(
                        function.as_str(),
                        "publish_selection"
                            | "publish_created"
                            | "publish_destroyed"
                            | "publish_communication_cleared"
                            | "publish_session_communication"
                    );
                if !allowed {
                    violations.push(format!("{path}:{} {event} owner is {function}", index + 1));
                }
            }

            if line.contains("pub async fn mark_exited(&self")
                || line.contains("pub async fn get_active(&self")
                || line.contains("pub async fn switch_session(&self")
                || line.contains("pub async fn set_active_only(&self")
                || line.contains("pub async fn clear_active(&self")
                || line.contains("pub async fn clear_active_if(&self")
            {
                violations.push(format!(
                    "{path}:{} removed production manager mutator {function}",
                    index + 1
                ));
            }
        }
    }
    violations
}

fn collect_rust_sources(dir: &Path, files: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).expect("read Rust source directory") {
        let entry = entry.expect("read Rust source entry");
        let path = entry.path();
        // Test-only files (`foo/tests.rs`, `foo/tests/`, `foo/<name>_tests.rs`) hold
        // what `production_prefix` cuts off when the tests live inline.
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("");
        if stem == "tests" || stem.ends_with("_tests") {
            continue;
        }
        if path.is_dir() {
            collect_rust_sources(&path, files);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            files.push((
                path.to_string_lossy().replace('\\', "/"),
                std::fs::read_to_string(&path).expect("read Rust source file"),
            ));
        }
    }
}

fn coordinator_job_declaration(source: &str) -> &str {
    let start = source
        .find("enum CoordinatorJob")
        .expect("CoordinatorJob declaration");
    let declaration = &source[start..];
    let open = declaration.find('{').expect("CoordinatorJob opening brace");
    let mut depth = 0usize;
    for (offset, character) in declaration[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &declaration[..open + offset + 1];
                }
            }
            _ => {}
        }
    }
    panic!("CoordinatorJob closing brace");
}

fn coordinator_job_handle_violations(source: &str) -> Vec<&'static str> {
    let declaration = coordinator_job_declaration(source);
    [
        "SessionManager",
        "PtyManager",
        "AppHandle",
        "SelectionTransaction",
        "BoxFuture",
        "dyn Future",
        "FnOnce",
    ]
    .into_iter()
    .filter(|forbidden| declaration.contains(forbidden))
    .collect()
}

#[test]
fn production_selection_and_lifecycle_sources_have_one_owner() {
    let mut files = Vec::new();
    collect_rust_sources(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let violations = ownership_violations(&files);
    assert!(
        violations.is_empty(),
        "production lifecycle ownership violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn source_ownership_sentinel_rejects_each_one_line_mutation() {
    let mutations = [
        "fn rogue() { row.status = SessionStatus::Active; }",
        "fn rogue() { app.emit(\"session_switched\", payload); }",
        "fn rogue() { app.emit(\"session_created\", payload); }",
        "fn rogue() { app.emit(\"session_destroyed\", payload); }",
        "fn rogue() { app.emit(\"session_communication_changed\", payload); }",
        "pub async fn mark_exited(&self) {}",
        "pub async fn get_active(&self) {}",
        "pub async fn switch_session(&self) {}",
        "pub async fn set_active_only(&self) {}",
        "pub async fn clear_active(&self) {}",
        "pub async fn clear_active_if(&self) {}",
    ];
    for mutation in mutations {
        let files = vec![("src/commands/session.rs".to_string(), mutation.to_string())];
        assert!(
            !ownership_violations(&files).is_empty(),
            "sentinel accepted mutation: {mutation}"
        );
    }
    let web_forgery = vec![(
        "src/web/commands.rs".to_string(),
        "fn validate_client_broadcast() { allow(\"session_switched\"); }".to_string(),
    )];
    assert!(
        !ownership_violations(&web_forgery).is_empty(),
        "sentinel accepted a lifecycle name in the client allowlist"
    );
}

#[test]
fn the_status_writer_guard_rejects_a_rogue_running_assignment() {
    for variant in ["Active", "Running", "Idle", "Exited(0)"] {
        let rogue = format!("fn rogue() {{ row.status = SessionStatus::{variant}; }}");
        for path in [
            "src/commands/session.rs",
            "src/session/manager.rs",
            "src/session/manager/commit.rs",
        ] {
            let files = vec![(path.to_string(), rogue.clone())];
            assert!(
                !ownership_violations(&files).is_empty(),
                "sentinel accepted {rogue} in {path}"
            );
        }
    }
}

#[test]
fn the_status_writer_guard_accepts_the_four_allowlisted_functions() {
    let owned = "\
fn mark_idle_with_carrier_permit() { s.status = SessionStatus::Idle; }
fn mark_busy_with_carrier_permit() { s.status = SessionStatus::Running; }
fn prepare_pty_input_boundary_with_carrier_permit() { session.status = SessionStatus::Running; }
fn commit_selection_transition_with_carrier_permit() {
    record.status = SessionStatus::Running;
    record.status = SessionStatus::Exited(exit_code);
    if previous.status == SessionStatus::Active { previous.status = SessionStatus::Running; }
    target.status = SessionStatus::Active;
}
";
    let owning = vec![("src/session/manager.rs".to_string(), owned.to_string())];
    assert_eq!(
        ownership_violations(&owning),
        Vec::<String>::new(),
        "the four allowlisted writers must pass"
    );

    // The same bodies anywhere else are violations, one per assignment plus
    // the tolerated `==` comparison the substring check also matches.
    let elsewhere = vec![("src/commands/session.rs".to_string(), owned.to_string())];
    assert_eq!(ownership_violations(&elsewhere).len(), 8);
}

#[test]
fn the_lifecycle_mutation_helper_owns_status_only_in_its_commit_module() {
    let source = "fn apply_lifecycle_mutations() {
    record.status = SessionStatus::Running;
    record.status = SessionStatus::Exited(exit_code);
}";
    let owner = vec![(
        "src/session/manager/commit.rs".to_string(),
        source.to_string(),
    )];
    assert!(ownership_violations(&owner).is_empty());
    for path in ["src/commands/session.rs", "src/session/manager.rs"] {
        let elsewhere = vec![(path.to_string(), source.to_string())];
        assert_eq!(ownership_violations(&elsewhere).len(), 2);
    }
}

#[test]
fn coordinator_jobs_are_typed_data_without_managed_handles_or_futures() {
    let source = include_str!("../../selection.rs");
    assert!(
        coordinator_job_handle_violations(source).is_empty(),
        "CoordinatorJob contains a managed handle or arbitrary executable field: {:?}",
        coordinator_job_handle_violations(source)
    );
    let commands = include_str!("../../../commands/session.rs");
    let opaque = &source[source.find("pub(crate) struct OpaqueFixtureInput").unwrap()
        ..source
            .find("impl fmt::Debug for OpaqueFixtureInput")
            .unwrap()];
    let payload = &commands[commands
        .find("pub(crate) struct FixturePtyCutLease")
        .unwrap()
        ..commands
            .find("impl std::fmt::Debug for FixtureReplacementInput")
            .unwrap()];
    for forbidden in [
        "SessionManager",
        "PtyManager",
        "AppHandle",
        "SelectionTransaction",
        "BoxFuture",
        "dyn Future",
        "FnOnce",
    ] {
        assert!(
            !payload.contains(forbidden),
            "fixture payload hides forbidden {forbidden}"
        );
        assert!(
            !opaque.contains(forbidden),
            "opaque fixture records hide forbidden {forbidden}"
        );
        let mutated = format!("enum CoordinatorJob {{ Rogue {{ value: {forbidden} }} }}");
        assert_eq!(
            coordinator_job_handle_violations(&mutated),
            vec![forbidden],
            "sentinel accepted forbidden CoordinatorJob field {forbidden}"
        );
    }
}
