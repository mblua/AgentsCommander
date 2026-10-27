use super::Path;

fn normalized_production_source(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let normalized = body.split_whitespace().collect::<String>();
    let end = match normalized.find("#[cfg(test)]modtests{") {
        Some(end) => end,
        None if !normalized.contains("modtests{") => normalized.len(),
        None => panic!("{relative}: no `#[cfg(test)] mod tests` block found to split off"),
    };
    normalized[..end].to_string()
}

fn production_token_count(production: &str) -> usize {
    production
        .matches("ManifestActivationToken::production()")
        .count()
}

#[test]
fn session_rs_threads_production_tokens_for_config_seed_and_context() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands");
    let names = [
        "session.rs",
        "session/create/core.rs",
        "session/create/core/context.rs",
        "session/create/core/backend.rs",
        "session/create/core/publication.rs",
    ];
    let split = root.join(names[1]).exists();
    let load = |name| normalized_production_source(&format!("src/commands/{name}"));
    let mut map: Vec<_> = names[..if split { 5 } else { 1 }]
        .iter()
        .map(|name| (*name, load(name)))
        .collect();
    let context_start = "//#930-resolvethehost-credentialcopy-inplan";
    let backend_start = "//CapturetheeffectiveargvectorBEFOREspawn";
    let publication_start = "ifletSome(continuation)=owned_replacement.as_ref(){letactual";
    let context_call = concat!(
        "materialize_agent_context_file_with_filename_activated(&cwd,",
        "&target_filename,&managed_filenames,is_coordinator,auto_self_clear,",
        "container_repos.as_ref(),activation.as_ref()"
    );
    let seed_call = "perform_config_seed_recorded(&seed,&id.to_string(),activation.as_ref()";
    let predicates = |map: &[(&str, String)]| {
        let expected = &names[..if map.len() == 1 { 1 } else { 5 }];
        if !map.iter().map(|(name, _)| name).eq(expected.iter()) {
            return false;
        }
        let count: usize = map
            .iter()
            .map(|(_, body)| production_token_count(body))
            .sum();
        let context = &map[usize::from(map.len() > 1) * 2].1;
        let seed = &map[usize::from(map.len() > 1) * 3].1;
        count == 2 && seed.contains(seed_call) && context.contains(context_call)
    };
    assert!(predicates(&map), "actual production map");
    if !split {
        let mut body = std::mem::take(&mut map[0].1);
        let mut publication = body.split_off(body.find(publication_start).unwrap());
        let backend = body.split_off(body.find(backend_start).unwrap());
        let context = body.split_off(body.find(context_start).unwrap());
        let end = publication.find("letresult=create_body.await;").unwrap();
        body.push_str(&publication.split_off(end));
        let slices = [body, context, backend, publication];
        map.extend(names[1..].iter().copied().zip(slices));
    }
    assert!(predicates(&map), "complete statement split map");
    let token = "ManifestActivationToken::production()";
    for (index, needle, replacement) in [
        (2, token, "None"),
        (3, token, "None"),
        (2, "activation.as_ref()", "None"),
        (3, "activation.as_ref()", "None"),
        (3, "perform_config_seed_recorded", "perform_config_seed"),
    ] {
        let mut mutant = map.clone();
        mutant[index].1 = mutant[index].1.replacen(needle, replacement, 1);
        assert!(!predicates(&mutant));
    }
    let missing = map[..4].to_vec();
    assert!(!predicates(&missing), "missing leaf");
    let mut duplicate = map.clone();
    duplicate.push(map[0].clone());
    assert!(!predicates(&duplicate), "duplicate leaf");
}

// (b) `commands/entity_creation/`: GUI Tauri delete commands no test calls - delete_team
// (LifecycleTeamDeletion), delete_agent_matrix (LifecycleAgentMatrixDeletion + staged-rollback
// prune, PendingInclusiveDeleteProtection), delete_workgroup (GUI LifecycleWorkgroupRemoval; CLI
// covered above). Each OUTER command must build and thread the token into its inner transaction.
#[test]
fn entity_creation_rs_threads_production_tokens_through_the_delete_commands() {
    let production = ["team_delete", "agent_matrix", "workgroup_delete"]
        .map(|c| normalized_production_source(&format!("src/commands/entity_creation/{c}.rs")))
        .concat();

    assert_eq!(
        production_token_count(&production),
        3,
        "entity_creation.rs must construct exactly three production activation tokens \
         (delete_team + delete_agent_matrix + delete_workgroup)"
    );

    // The inner-fn names also appear in unit calls and definitions, so the token
    // argument is what pins each assertion to the production command.
    assert!(
        production.contains(
            "delete_team_inner(&app,session_mgr.inner(),&project_path,&team_name,\
             Some(ManifestActivationToken::production()),"
        ),
        "delete_team (LifecycleTeamDeletion) must thread Some(production()) into delete_team_inner"
    );
    assert!(
        production.contains(
            "delete_agent_matrix_inner(&app,session_mgr.inner(),settings.inner(),&project_path,\
             &agent_path,Some(ManifestActivationToken::production()),"
        ),
        "delete_agent_matrix (LifecycleAgentMatrixDeletion + PendingInclusiveDeleteProtection) \
         must thread Some(production()) into delete_agent_matrix_inner"
    );
    assert!(
        production.contains(
            "letactivation=ManifestActivationToken::production();gated_workgroup_delete_with(\
             &project_root,&workgroup,Some(&activation),"
        ),
        "delete_workgroup (LifecycleWorkgroupRemoval GUI site) must construct and thread the token \
         into gated_workgroup_delete_with"
    );
}

// ===========================================================================
// #1614 section 9.1 "Rust integration tests" / section 15.4.
//
// Round 5 measured ZERO new test functions in src-tauri/tests/ across the whole
// 12-commit range, so AC3, AC4 and AC6 rested on unrecorded manual checks. The
// four below are those gates, written rather than substituted for.
// ===========================================================================
