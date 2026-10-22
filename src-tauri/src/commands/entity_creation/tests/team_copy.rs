use super::*;

#[test]
fn issue_3068_none_and_some_local_preserve_alerts_without_copy_events() {
    let (_tmp, project, source, destination) = issue_3068_fixture();
    // An invalid local history is irrelevant when no copy occurs.
    std::fs::write(source.join(COPY_PROVENANCE_FILE), b"invalid").unwrap();
    let before = issue_3068_inventory(&source);
    for (name, sources) in [
        ("legacy", None),
        ("explicit", Some(issue_3068_sources(&project, &["alpha"]))),
    ] {
        let team = create_team_from_sources(
            &project,
            name,
            issue_3068_team_config(&["alpha"]),
            sources,
            &mut MatrixCopyHooks::default(),
        )
        .unwrap();
        let config: TeamConfigResult =
            serde_json::from_slice(&std::fs::read(team.join("config.json")).unwrap()).unwrap();
        assert_eq!(config.agents, ["_agent_alpha"]);
        assert_eq!(config.context_alert_percentages, [20, 80]);
    }
    assert_eq!(before, issue_3068_inventory(&source));
    assert!(!destination.join("_agent_alpha").exists());
}

#[test]
fn issue_3068_foreign_team_binds_copies_and_portable_refs() {
    let (_tmp, project, source, destination) = issue_3068_fixture();
    std::fs::write(source.join("Role.md"), b"---\nname: display-only\n---\n").unwrap();
    let beta = project.join(".ac/_agent_beta");
    std::fs::create_dir(&beta).unwrap();
    std::fs::write(beta.join("payload"), [0, 255, 17]).unwrap();
    std::fs::write(beta.join("Role.md"), b"beta role").unwrap();
    let before = issue_3068_inventory(&source);
    let team = issue_3068_create(
        &destination,
        issue_3068_team_config(&["alpha", "_agent_beta"]),
        Some(issue_3068_sources(&project, &["_agent_alpha", "beta"])),
        &mut MatrixCopyHooks::default(),
    )
    .unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(team.join("config.json")).unwrap()).unwrap();
    assert_eq!(
        value["agents"],
        serde_json::json!(["_agent_alpha", "_agent_beta"])
    );
    assert_eq!(value["coordinator"], "_agent_alpha");
    assert_eq!(value["repos"][0]["agents"], value["agents"]);
    assert!(value.get("agentSources").is_none());
    assert_eq!(before, issue_3068_inventory(&source));
    for agent in ["alpha", "beta"] {
        assert_eq!(
            issue_3068_history(&destination.join(format!("_agent_{agent}")))["copies"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    assert_eq!(std::fs::read_dir(&team).unwrap().count(), 3);
    assert!(team.join("memory").is_dir());
    assert_eq!(std::fs::read(team.join("conventions.md")).unwrap(), b"");
    std::fs::write(destination.join("_agent_alpha/Role.md"), "independent").unwrap();
    assert_eq!(before, issue_3068_inventory(&source));
    assert_eq!(std::fs::read(beta.join("payload")).unwrap(), [0, 255, 17]);
}

#[test]
fn issue_3068_preflight_rejects_inexact_sources_and_nonmembers() {
    let (_tmp, project, _source, destination) = issue_3068_fixture();
    let cases = [
        (issue_3068_team_config(&["alpha"]), vec![]),
        (
            issue_3068_team_config(&["alpha"]),
            issue_3068_sources(&project, &["alpha", "beta"]),
        ),
        (
            issue_3068_team_config(&["alpha"]),
            issue_3068_sources(&project, &["alpha", "_agent_alpha"]),
        ),
        (
            issue_3068_team_config(&["alpha", "_agent_alpha"]),
            issue_3068_sources(&project, &["alpha"]),
        ),
        (
            issue_3068_team_config(&["../alpha"]),
            issue_3068_sources(&project, &["../alpha"]),
        ),
    ];
    for (config, sources) in cases {
        assert!(issue_3068_create(
            &destination,
            config,
            Some(sources),
            &mut MatrixCopyHooks::default()
        )
        .is_err());
        assert!(!destination.join("_agent_alpha").exists());
        assert!(!destination.join("_team_copied").exists());
    }
    for raw in ["..", "alpha/beta", "alpha\\beta", "alpha\0", "C:alpha"] {
        assert!(portable_source_ref(raw).is_err());
    }
    for coordinator in [true, false] {
        let mut config = issue_3068_team_config(&["alpha"]);
        if coordinator {
            config.coordinator = "outsider".into();
        } else {
            config.repos[0].agents.push("outsider".into());
        }
        assert!(issue_3068_create(
            &destination,
            config,
            Some(issue_3068_sources(&project, &["alpha"])),
            &mut MatrixCopyHooks::default()
        )
        .is_err());
        assert!(!destination.join("_agent_alpha").exists());
    }
}

#[test]
fn issue_3068_collisions_and_ambiguous_projects_preserve_existing_content() {
    for collision in ["_agent_alpha", "_team_copied"] {
        let (_tmp, project, _source, destination) = issue_3068_fixture();
        let existing = destination.join(collision);
        std::fs::create_dir(&existing).unwrap();
        std::fs::write(existing.join("foreign"), b"untouched").unwrap();
        assert!(issue_3068_create(
            &destination,
            issue_3068_team_config(&["alpha"]),
            Some(issue_3068_sources(&project, &["alpha"])),
            &mut MatrixCopyHooks::default(),
        )
        .is_err());
        assert_eq!(
            std::fs::read(existing.join("foreign")).unwrap(),
            b"untouched"
        );
    }
    let (_tmp, project, _source, destination) = issue_3068_fixture();
    let mut sources = issue_3068_sources(&project, &["alpha"]);
    sources.extend(issue_3068_sources(
        destination.parent().unwrap(),
        &["alpha"],
    ));
    assert!(issue_3068_create(
        &destination,
        issue_3068_team_config(&["alpha"]),
        Some(sources),
        &mut MatrixCopyHooks::default(),
    )
    .is_err());
    assert!(!destination.join("_agent_alpha").exists());
}

#[test]
fn issue_3068_second_matrix_failure_rolls_back_first_and_metadata() {
    for fail_metadata in [false, true] {
        let (_tmp, project, _source, destination) = issue_3068_fixture();
        std::fs::create_dir(project.join(".ac/_agent_beta")).unwrap();
        std::fs::write(project.join(".ac/_agent_beta/Role.md"), b"beta role").unwrap();
        let mut triggered = false;
        let mut callback = |stage, path: &Path| {
            let target = if fail_metadata {
                MatrixCopyStage::Provenance
            } else {
                MatrixCopyStage::Reserved
            };
            if stage == target && path.ends_with("_agent_beta") {
                triggered = true;
                return Err("injected second Matrix failure".into());
            }
            Ok(())
        };
        let error = issue_3068_create(
            &destination,
            issue_3068_team_config(&["alpha", "beta"]),
            Some(issue_3068_sources(&project, &["alpha", "beta"])),
            &mut MatrixCopyHooks {
                callback: Some(&mut callback),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(triggered);
        assert!(error.contains("injected second Matrix failure"));
        for root in ["_agent_alpha", "_agent_beta", "_team_copied"] {
            assert!(!destination.join(root).exists(), "{error}");
        }
    }
}

fn issue_3068_racing_team_file(stage_to_race: MatrixCopyStage, filename: &str) {
    let (_tmp, project, _source, destination) = issue_3068_fixture();
    let mut triggered = false;
    let mut callback = |stage, team: &Path| {
        if stage == stage_to_race {
            triggered = true;
            std::fs::write(team.join(filename), b"external bytes").unwrap();
        }
        Ok(())
    };
    let error = issue_3068_create(
        &destination,
        issue_3068_team_config(&["alpha"]),
        Some(issue_3068_sources(&project, &["alpha"])),
        &mut MatrixCopyHooks {
            callback: Some(&mut callback),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(triggered);
    assert!(error.contains("residues"), "{error}");
    let team = destination.join("_team_copied");
    assert_eq!(
        std::fs::read(team.join(filename)).unwrap(),
        b"external bytes"
    );
    assert_eq!(std::fs::read_dir(&team).unwrap().count(), 1);
    assert!(!destination.join("_agent_alpha").exists());
}

#[test]
fn issue_3068_publication_preserves_racing_config() {
    issue_3068_racing_team_file(MatrixCopyStage::TeamPublication, "config.json");
}

#[test]
fn issue_3068_creation_preserves_racing_conventions() {
    issue_3068_racing_team_file(MatrixCopyStage::TeamConventions, "conventions.md");
}

#[test]
fn issue_3068_unlink_failure_reports_error_and_rolls_back_published_config() {
    let (_tmp, project, _source, destination) = issue_3068_fixture();
    let mut saw_complete_config = false;
    let mut callback = |stage, team: &Path| {
        if stage == MatrixCopyStage::TeamUnlink {
            let value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(team.join("config.json")).unwrap()).unwrap();
            assert_eq!(value["agents"], serde_json::json!(["_agent_alpha"]));
            saw_complete_config = true;
            return Err("injected unlink failure".into());
        }
        Ok(())
    };
    let error = issue_3068_create(
        &destination,
        issue_3068_team_config(&["alpha"]),
        Some(issue_3068_sources(&project, &["alpha"])),
        &mut MatrixCopyHooks {
            callback: Some(&mut callback),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(saw_complete_config);
    assert!(error.contains("injected unlink failure"));
    assert!(!destination.join("_team_copied").exists(), "{error}");
    assert!(!destination.join("_agent_alpha").exists(), "{error}");
}

#[test]
fn issue_3068_prepublication_rechecks_copies_and_preserves_external_changes() {
    let (_tmp, project, _source, destination) = issue_3068_fixture();
    let mut callback = |stage, _path: &Path| {
        if stage == MatrixCopyStage::TeamPublication {
            std::fs::write(destination.join("_agent_alpha/Role.md"), b"external edit").unwrap();
            std::fs::write(destination.join("_agent_alpha/added"), b"external file").unwrap();
        }
        Ok(())
    };
    let error = issue_3068_create(
        &destination,
        issue_3068_team_config(&["alpha"]),
        Some(issue_3068_sources(&project, &["alpha"])),
        &mut MatrixCopyHooks {
            callback: Some(&mut callback),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(error.contains("Output modified"), "{error}");
    assert!(error.contains("residues"), "{error}");
    assert!(!destination.join("_team_copied").exists());
    assert_eq!(
        std::fs::read(destination.join("_agent_alpha/Role.md")).unwrap(),
        b"external edit"
    );
    assert_eq!(
        std::fs::read(destination.join("_agent_alpha/added")).unwrap(),
        b"external file"
    );
}

#[test]
fn issue_3068_cooperating_creators_publish_at_most_once() {
    let (_tmp, project, _source, destination) = issue_3068_fixture();
    let barrier = std::sync::Barrier::new(2);
    let successes = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    issue_3068_create(
                        &destination,
                        issue_3068_team_config(&["alpha"]),
                        Some(issue_3068_sources(&project, &["alpha"])),
                        &mut MatrixCopyHooks::default(),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(Result::is_ok)
            .count()
    });
    assert_eq!(successes, 1);
    let config: TeamConfigResult = serde_json::from_slice(
        &std::fs::read(destination.join("_team_copied/config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(config.agents, ["_agent_alpha"]);
    assert_eq!(
        issue_3068_history(&destination.join("_agent_alpha"))["copies"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn issue_3068_invalid_copy_entries_fail_real_team_core() {
    for classification in [
        (true, 0, true, false),
        (false, 0x400, true, false),
        (false, 0, false, false),
    ] {
        let (_tmp, project, _source, destination) = issue_3068_fixture();
        assert!(issue_3068_create(
            &destination,
            issue_3068_team_config(&["alpha"]),
            Some(issue_3068_sources(&project, &["alpha"])),
            &mut MatrixCopyHooks {
                classification: Some(classification),
                ..Default::default()
            },
        )
        .is_err());
        assert!(!destination.join("_agent_alpha").exists());
        assert!(!destination.join("_team_copied").exists());
    }
}

#[test]
fn issue_3068_discovery_classifies_container_identity_and_rediscovered_copy() {
    let (_tmp, project, _source, destination) = issue_3068_fixture();
    let target = destination.parent().unwrap();
    let same_name_project = target.join("source-project");
    std::fs::create_dir_all(same_name_project.join(".ac/_agent_other")).unwrap();
    let paths: Vec<String> = [&project, &same_name_project]
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let destination_alias = project.join(".");
    let listed =
        list_all_agents_for_destination(&paths, Some(destination_alias.to_str().unwrap())).unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed
        .iter()
        .all(|agent| agent.project_name == "source-project"));
    assert_eq!(
        listed
            .iter()
            .find(|agent| agent.name == "alpha")
            .unwrap()
            .is_local,
        Some(true)
    );
    assert_eq!(
        listed
            .iter()
            .find(|agent| agent.name == "other")
            .unwrap()
            .is_local,
        Some(false)
    );
    assert!(listed
        .iter()
        .all(|agent| !agent.project_path.starts_with("\\\\?\\")));
    assert!(list_all_agents_for_destination(&paths, None)
        .unwrap()
        .iter()
        .all(|agent| agent.is_local.is_none()));
    assert!(
        list_all_agents_for_destination(&paths, Some("invalid-project-that-does-not-exist"))
            .is_err()
    );
    issue_3068_create(
        &destination,
        issue_3068_team_config(&["alpha"]),
        Some(issue_3068_sources(&project, &["alpha"])),
        &mut MatrixCopyHooks::default(),
    )
    .unwrap();
    let copied = list_all_agents_for_destination(
        &[target.to_string_lossy().into_owned()],
        Some(target.to_str().unwrap()),
    )
    .unwrap();
    assert_eq!(copied.len(), 1);
    assert_eq!(copied[0].is_local, Some(true));
    assert_eq!(copied[0].project_name, "destination");
    assert_eq!(
        copied[0].project_path,
        crate::path_utils::path_to_string_without_windows_verbatim_prefix(
            &target.canonicalize().unwrap()
        )
    );
}

#[cfg(windows)]
#[test]
fn issue_3068_windows_aliases_are_local_in_discovery_and_creation() {
    let (_tmp, project, source, _destination) = issue_3068_fixture();
    let alias = project.to_string_lossy().to_uppercase().replace('\\', "/");
    let entries =
        list_all_agents_for_destination(&[project.to_string_lossy().into_owned()], Some(&alias))
            .unwrap();
    assert_eq!(entries[0].is_local, Some(true));
    create_team_from_sources(
        &project,
        "alias",
        issue_3068_team_config(&["alpha"]),
        Some(vec![TeamAgentSource {
            agent_ref: "_agent_alpha".into(),
            source_project_path: alias,
        }]),
        &mut MatrixCopyHooks::default(),
    )
    .unwrap();
    assert!(!source.join(COPY_PROVENANCE_FILE).exists());
}
