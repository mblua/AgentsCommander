use super::*;
use crate::config::settings::{
    configuration_identity, configure_source_factory_guard_cut, list_active_sources,
    prepare_source_candidate, prepare_source_candidate_with_original_links, profile_identity,
    publish_source_intent_with_validation, read_identity_reference_physical,
    read_own_source_intent, read_own_source_journal, read_source_snapshot,
    source_factory_guard_calls, source_factory_guard_cut_armed, source_transition_digest,
    source_transition_digest_bytes, source_transition_effect_counts,
    source_transition_publication_counts, source_transition_test_locator,
    update_own_source_journal_with_validation, ConfigurationRef, IdentityReferenceError,
    IdentityReferencePhysicalState, IdentityWriteLease, LegacyCompatibilityOrigin, OriginInputs,
    PreparedSourceCandidate, SourceAvailability, SourceCoordinationError, SourceInventoryInputs,
    SourceInventoryProof, SourceKind, SourceLocalInventory, SourceLocalProgress,
    SourcePreparationError, SourceReadContext, SourceReceipt, SourceRef, SourceRevision,
    SourceWriteContext,
};

mod driver;
mod durable_resume;
mod existing_target;
mod r3_checkpoint;

struct DriverFixture {
    controls: IdentityTransitionTestControls,
    _root: tempfile::TempDir,
    settings: AppSettings,
    context: SourceReadContext,
    source: SourceRef,
    project: std::path::PathBuf,
    initial_revision: SourceRevision,
}

impl DriverFixture {
    fn new() -> Self {
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
        let seed = serde_json::json!({"schemaVersion":2,"agents":[{
            "key":"fixture-seed","label":"Fixture seed","description":"Owned","color":"#123456",
            "command":"fixture-before-command","envs":[]}]});
        assert_ne!(seed, Self::candidate());
        let target = source_transition_test_locator(&context, &source).unwrap();
        std::fs::write(&target, serde_json::to_vec(&seed).unwrap()).unwrap();
        let snapshot = read_source_snapshot(&context, &source).unwrap();
        assert_eq!(
            snapshot.snapshot.availability,
            SourceAvailability::Available
        );
        assert!(snapshot.snapshot.capabilities.editable_save);
        let initial_revision = snapshot.snapshot.revision;
        Self {
            controls: IdentityTransitionTestControls::default(),
            _root: root,
            settings,
            context,
            source,
            project,
            initial_revision,
        }
    }
    fn candidate() -> serde_json::Value {
        serde_json::json!({"schemaVersion": 2, "agents": []})
    }
    fn start(&self, request: &str) -> IdentityTransitionRead {
        let revision = read_source_snapshot(&self.context, &self.source)
            .unwrap()
            .snapshot
            .revision;
        commit_source_candidate_internal(
            &self.settings,
            &self.context,
            &self.source,
            request,
            &revision,
            &Self::candidate(),
            (&|| Ok(()), &self.controls),
        )
        .unwrap()
    }
    fn expectation(&self) -> SourceResumeExpectation {
        let identity =
            IdentityWriteLease::acquire(&self.context, std::time::Duration::ZERO).unwrap();
        let lease = SourceWriteContext::acquire(&self.context, &self.source, &identity).unwrap();
        SourceResumeExpectation {
            source: lease.source_revision().unwrap(),
            journal: read_own_source_journal(&self.context).unwrap().0,
        }
    }
    fn operation(&self) -> String {
        read_own_source_journal(&self.context)
            .unwrap()
            .1
            .intents
            .keys()
            .next()
            .unwrap()
            .clone()
    }
}

impl DriverFixture {
    fn with_reference_targets() -> (
        Self,
        serde_json::Value,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let fixture = Self::new();
        let before = serde_json::json!({"schemaVersion":2,"agents":[{
            "key":"custom-k","label":"Custom K","description":"Owned","color":"#123456",
            "command":"original-command","envs":[]}],
            "codingAgentProfiles":{"custom-k":{"A":{"command":"--profile", "env":{}}}}});
        let path = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
        std::fs::write(path, serde_json::to_vec(&before).unwrap()).unwrap();
        let snapshot = read_source_snapshot(&fixture.context, &fixture.source).unwrap();
        let selection = ConfigurationRef {
            version: 1,
            source: fixture.source.clone(),
            calculated_id: configuration_identity("original-command", &[]),
            requested_profile: Some("A".into()),
            profile_calculated_id: Some(profile_identity(
                &snapshot.snapshot.profiles["custom-k"]["A"],
            )),
            legacy_entry_hint: Some("custom-k".into()),
        };
        let target = fixture.project.join(".ac/_agent_N");
        let disjoint = fixture.project.join(".ac/_agent_U");
        std::fs::create_dir(&target).unwrap();
        std::fs::create_dir(&disjoint).unwrap();
        std::fs::write(
            target.join("config.json"),
            serde_json::to_vec(&serde_json::json!({"unknown":null,
            "tooling":{"configurationRef":selection.clone(), "unknown":{"keep":[null,false]}}}))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
            serde_json::to_vec(
                &serde_json::json!({"unknown":[null],"tooling":{"sourceReferences":[{
                "configurationRef":selection.clone(), "unknown":true}]}}),
            )
            .unwrap(),
        )
        .unwrap();
        let mut foreign = selection;
        foreign.source = crate::config::settings::SourceRef {
            family: crate::config::settings::SourceFamily::Agents,
            role: crate::config::settings::SourceRole::Catalog,
            scope: crate::config::settings::SourceScope::Project,
            kind: crate::config::settings::SourceKind::CatalogBase,
            context_root: std::fs::canonicalize(&fixture.project)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        };
        std::fs::write(
            disjoint.join("config.json"),
            serde_json::to_vec(&serde_json::json!({"unknown":true,
            "tooling":{"configurationRef":foreign}}))
            .unwrap(),
        )
        .unwrap();
        let mut candidate = before;
        candidate["agents"][0]["command"] = serde_json::json!("updated-command");
        (fixture, candidate, target, disjoint)
    }
}

impl DriverFixture {
    fn resume_at(
        &self,
        context: &SourceReadContext,
        operation: &str,
    ) -> Result<IdentityTransitionRead, SourceTransitionError> {
        self.resume_at_with_controls(context, operation, &self.controls)
    }

    fn resume_at_with_controls(
        &self,
        context: &SourceReadContext,
        operation: &str,
        controls: &IdentityTransitionTestControls,
    ) -> Result<IdentityTransitionRead, SourceTransitionError> {
        let expected = {
            let identity = IdentityWriteLease::acquire(context, std::time::Duration::ZERO).unwrap();
            let lease = SourceWriteContext::acquire(context, &self.source, &identity).unwrap();
            SourceResumeExpectation {
                source: lease.source_revision().unwrap(),
                journal: read_own_source_journal(context).unwrap().0,
            }
        };
        resume_identity_transition_internal(
            &self.settings,
            context,
            &self.source,
            operation,
            &expected,
            &|| Ok(()),
            controls,
        )
    }
}
