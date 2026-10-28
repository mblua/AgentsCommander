use super::*;
use crate::config::entity_creation::copies::project_directory_identity;
use crate::config::entity_creation::directories;
use crate::config::entity_creation::names::RepoAssignment;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    std::fs::create_dir_all(source.join(".ac/_agent_alpha")).unwrap();
    std::fs::create_dir_all(destination.join(".ac")).unwrap();
    std::fs::write(source.join(".ac/_agent_alpha/Role.md"), b"alpha role").unwrap();
    (temp, source, destination)
}

fn config() -> TeamConfigResult {
    TeamConfigResult {
        agents: vec!["alpha".into()],
        coordinator: "alpha".into(),
        repos: vec![RepoAssignment {
            url: "https://example.test/repo.git".into(),
            agents: vec!["alpha".into()],
        }],
        context_alert_percentages: vec![80, 20],
    }
}

fn sources(project: &Path) -> Vec<TeamAgentSource> {
    vec![TeamAgentSource {
        agent_ref: "alpha".into(),
        source_project_path: project.to_string_lossy().into_owned(),
    }]
}

#[test]
fn issue_3294_team_no_clobber_positive_control() {
    for (stage_to_inject, filename) in [
        (MatrixCopyStage::TeamPublication, Some("config.json")),
        (MatrixCopyStage::TeamConventions, Some("conventions.md")),
        (MatrixCopyStage::TeamUnlink, None),
    ] {
        let (_temp, source, destination) = fixture();
        let mut observed = false;
        let mut callback = |stage, team: &Path| {
            if stage == stage_to_inject {
                observed = true;
                if let Some(filename) = filename {
                    std::fs::write(team.join(filename), b"foreign bytes").unwrap();
                } else {
                    let value: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(team.join("config.json")).unwrap())
                            .unwrap();
                    assert_eq!(value["agents"], serde_json::json!(["_agent_alpha"]));
                    return Err("injected TeamUnlink failure".into());
                }
            }
            Ok(())
        };
        let error = create_team_from_sources(
            &destination,
            "copied",
            config(),
            Some(sources(&source)),
            &mut MatrixCopyHooks {
                callback: Some(&mut callback),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(observed);
        let team = destination.join(".ac/_team_copied");
        if let Some(filename) = filename {
            assert!(error.contains("residues"), "{error}");
            assert_eq!(
                std::fs::read(team.join(filename)).unwrap(),
                b"foreign bytes"
            );
            assert_eq!(std::fs::read_dir(&team).unwrap().count(), 1);
        } else {
            assert!(error.contains("injected TeamUnlink failure"), "{error}");
            assert!(!team.exists(), "{error}");
        }
        assert!(!destination.join(".ac/_agent_alpha").exists(), "{error}");
        assert_eq!(
            std::fs::read(source.join(".ac/_agent_alpha/Role.md")).unwrap(),
            b"alpha role"
        );
    }
}

#[test]
fn issue_3294_team_discovery_and_preflight_share_project_identity() {
    let (_temp, source, destination) = fixture();
    let (_, source_identity) = project_directory_identity(&source).unwrap();
    let (_, alias_identity) = project_directory_identity(&source.join(".")).unwrap();
    let (_, destination_identity) = project_directory_identity(&destination).unwrap();
    assert_eq!(source_identity, alias_identity);
    assert_ne!(source_identity, destination_identity);
    let mut selected = config();
    assert!(preflight_team_sources(
        &source,
        &source.join(".ac").canonicalize().unwrap(),
        &mut selected,
        sources(&source.join(".")),
    )
    .unwrap()
    .is_empty());
    let mut selected = config();
    let foreign = preflight_team_sources(
        &destination,
        &destination.join(".ac").canonicalize().unwrap(),
        &mut selected,
        sources(&source),
    )
    .unwrap();
    assert_eq!(
        foreign,
        vec![(source.canonicalize().unwrap(), "_agent_alpha".into())]
    );
    let ac = source.join(".ac");
    std::fs::create_dir(ac.join("_team_ignored")).unwrap();
    std::fs::write(ac.join("_agent_file"), b"not a directory").unwrap();
    let matrices: Vec<_> = directories::agent_matrix_candidates(&ac).collect();
    assert_eq!(matrices, vec![ac.join("_agent_alpha")]);
    assert_eq!(
        directories::agent_matrix_role(&matrices[0]).as_deref(),
        Some("alpha role")
    );
    assert!(
        directories::agent_matrix_candidates(&source.join("missing"))
            .next()
            .is_none()
    );
}

#[cfg(windows)]
#[test]
fn issue_3294_team_preflight_rejects_reparse_matrix() {
    let (temp, source, destination) = fixture();
    let target = temp.path().join("external");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("Role.md"), b"external role").unwrap();
    let junction = source.join(".ac/_agent_linked");
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(junction.to_string_lossy().replace('/', "\\"))
        .arg(target.to_string_lossy().replace('/', "\\"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(directories::agent_matrix_candidates(&source.join(".ac")).any(|path| path == junction));
    let mut selected = config();
    selected.agents = vec!["linked".into()];
    selected.coordinator = "linked".into();
    selected.repos.clear();
    let error = preflight_team_sources(
        &destination,
        &destination.join(".ac").canonicalize().unwrap(),
        &mut selected,
        vec![TeamAgentSource {
            agent_ref: "linked".into(),
            source_project_path: source.to_string_lossy().into_owned(),
        }],
    )
    .unwrap_err();
    assert!(error.contains("links/reparse points"), "{error}");
    assert_eq!(
        std::fs::read(target.join("Role.md")).unwrap(),
        b"external role"
    );
    std::fs::remove_dir(&junction).unwrap();
}
