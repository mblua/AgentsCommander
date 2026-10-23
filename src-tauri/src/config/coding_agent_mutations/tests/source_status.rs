use super::*;

#[test]
fn single_editor_inactive_gate() {
    use crate::config::settings::{
        source_transition_effect_counts, SourceReadContext, SourceRevision,
    };
    let root = crate::config::settings::source_transition_test_root();
    let project = root.path().join("project");
    let config = root.path().join("C");
    std::fs::create_dir_all(project.join(".ac/coding-agents")).unwrap();
    std::fs::create_dir(&config).unwrap();
    let settings = AppSettings {
        project_paths: vec![project.to_string_lossy().into_owned()],
        ..AppSettings::default()
    };
    let context =
        SourceReadContext::from_settings(&settings, &config.join("settings.json")).unwrap();
    let source = crate::config::settings::SourceRef {
        family: crate::config::settings::SourceFamily::Agents,
        role: crate::config::settings::SourceRole::Catalog,
        scope: crate::config::settings::SourceScope::Project,
        kind: crate::config::settings::SourceKind::CatalogPersonal,
        context_root: std::fs::canonicalize(&project)
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    };
    let before = source_transition_effect_counts();
    let uuids = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
    let reloads = std::cell::Cell::new(0);
    let reload = || {
        reloads.set(reloads.get() + 1);
        Ok(())
    };
    for candidate in [
        serde_json::json!({"schemaVersion":2,"agents":[]}),
        serde_json::Value::Null,
    ] {
        assert!(matches!(
            commit_source_candidate(
                &settings,
                &context,
                &source,
                "request-1",
                &SourceRevision::Absent {},
                &candidate,
                &reload
            ),
            Err(SourceTransitionError::ActivationRequired)
        ));
    }
    let expected = SourceResumeExpectation {
        source: SourceRevision::Absent {},
        journal: SourceRevision::Absent {},
    };
    for operation in ["operation-1", "unknown", ""] {
        assert!(matches!(
            resume_identity_transition(&settings, &context, &source, operation, &expected, &reload),
            Err(SourceTransitionError::ActivationRequired)
        ));
    }
    let status = get_identity_transition(&context, &source, "unknown").unwrap();
    assert_eq!(status.state, IdentityTransitionReadState::NotFound);
    assert_eq!(source_transition_effect_counts(), before);
    assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
    assert_eq!(reloads.get(), 0);
    assert_eq!(std::fs::read_dir(&config).unwrap().count(), 0);
    assert_eq!(
        std::fs::read_dir(project.join(".ac/coding-agents"))
            .unwrap()
            .count(),
        0
    );
    let mut ordinary = settings.clone();
    let agent: AgentConfig = serde_json::from_value(serde_json::json!({
        "id":"custom-ordinary","label":"Ordinary","color":"#123456","command":"ordinary-command"}))
    .unwrap();
    apply_coding_agent_op(&mut ordinary, &CodingAgentOp::Add { agent }).unwrap();
    let saved = crate::config::settings::save_settings_to_path_preserving_project_paths(
        &ordinary,
        &config.join("settings.json"),
    )
    .unwrap();
    assert!(saved
        .agents
        .iter()
        .any(|agent| agent.id == "custom-ordinary"));
    let registered: serde_json::Value = serde_json::from_slice(
        &std::fs::read(config.join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME))
            .unwrap(),
    )
    .unwrap();
    assert!(registered["agents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|agent| agent["id"] == "custom-ordinary"));
    assert_eq!(source_transition_effect_counts(), before);
    assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
    assert!(crate::config::settings::read_own_source_journal(&context)
        .unwrap()
        .1
        .intents
        .is_empty());
}

#[test]
fn single_editor_status_unknown_inactive_zero_write_without_sidecars() {
    use crate::config::settings::SourceReadContext;
    let root = crate::config::settings::source_transition_test_root();
    let settings = AppSettings::default();
    let context =
        SourceReadContext::from_settings(&settings, &root.path().join("settings.json")).unwrap();
    let source = crate::config::settings::SourceRef {
        family: crate::config::settings::SourceFamily::Agents,
        role: crate::config::settings::SourceRole::Registered,
        scope: crate::config::settings::SourceScope::Instance,
        kind: crate::config::settings::SourceKind::RegisteredInstance,
        context_root: std::fs::canonicalize(root.path())
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    };
    for _ in 0..3 {
        let read = get_identity_transition(&context, &source, "unknown-operation").unwrap();
        assert_eq!(read.state, IdentityTransitionReadState::NotFound);
        assert!(read.operation_id.is_none());
        assert!(!read.active);
        assert!(read.status.summary.is_none());
        assert_eq!(
            read.status.journal_revision,
            crate::config::settings::SourceRevision::Absent {}
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn single_editor_status_invalid_present_journal_preserved_not_empty() {
    use crate::config::settings::{SourceCoordinationError, SourceReadContext};
    let root = crate::config::settings::source_transition_test_root();
    let settings = AppSettings::default();
    let context =
        SourceReadContext::from_settings(&settings, &root.path().join("settings.json")).unwrap();
    let source = crate::config::settings::SourceRef {
        family: crate::config::settings::SourceFamily::Agents,
        role: crate::config::settings::SourceRole::Registered,
        scope: crate::config::settings::SourceScope::Instance,
        kind: crate::config::settings::SourceKind::RegisteredInstance,
        context_root: std::fs::canonicalize(root.path())
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    };
    let journal = root
        .path()
        .join(crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME);
    std::fs::write(&journal, b"{}").unwrap();
    assert!(matches!(
        get_identity_transition(&context, &source, "unknown-operation"),
        Err(SourceCoordinationError::Invalid)
    ));
    assert_eq!(std::fs::read(&journal).unwrap(), b"{}");
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}
