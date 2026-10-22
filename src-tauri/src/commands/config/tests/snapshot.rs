use super::super::{settings_snapshot_from, ProjectPathIssue};
use crate::config::projects::{
    DirectoryIdentity, IssueKind, ProjectPathPersistenceState, ProjectSource, RawJsonField,
    RawStringField, RepairKind, ResolvedPair, SideOutcome, SideStatus, StructuralIssue,
};
use crate::config::settings::AppSettings;

fn side(status: SideStatus, canonical: Option<&str>) -> SideOutcome {
    SideOutcome {
        status,
        syntactic_path: canonical.map(str::to_string),
        canonical_path: canonical.map(str::to_string),
        identity: canonical.map(|_| DirectoryIdentity { volume: 1, file: 1 }),
    }
}

fn conflict_pair() -> ResolvedPair {
    ResolvedPair {
        source: ProjectSource::ProjectPaths,
        index: Some(0),
        raw_absolute: RawStringField::string("/abs/alpha"),
        raw_relative: RawStringField::string("../rel/beta"),
        absolute_side: side(SideStatus::ValidDirectProject, Some("/abs/alpha")),
        relative_side: side(SideStatus::ValidDirectProject, Some("/rel/beta")),
        selected: None,
        selected_canonical_raw: None,
        selected_identity: None,
        issue: Some(IssueKind::Conflict),
        repair: RepairKind::None,
    }
}

fn missing_pair() -> ResolvedPair {
    ResolvedPair {
        source: ProjectSource::ProjectPaths,
        index: Some(1),
        raw_absolute: RawStringField::string("/gone/x"),
        raw_relative: RawStringField::absent(),
        absolute_side: SideOutcome {
            status: SideStatus::Missing,
            syntactic_path: Some("/gone/x".to_string()),
            canonical_path: None,
            identity: None,
        },
        relative_side: SideOutcome {
            status: SideStatus::Absent,
            syntactic_path: None,
            canonical_path: None,
            identity: None,
        },
        selected: None,
        selected_canonical_raw: None,
        selected_identity: None,
        issue: Some(IssueKind::Missing),
        repair: RepairKind::None,
    }
}

fn state_with(
    pairs: Vec<ResolvedPair>,
    structural: Vec<StructuralIssue>,
) -> ProjectPathPersistenceState {
    let active_registration_count = pairs.len();
    ProjectPathPersistenceState {
        pairs,
        selected_head: None,
        active_registration_count,
        archived_registration_count: 0,
        active_companion_present: true,
        archived_companion_present: false,
        has_genuine_singular: false,
        active_reconcile_eligible: false,
        archived_reconcile_eligible: false,
        structural_issues: structural,
        runtime_authoritative: false,
    }
}

fn settings_with_state(state: ProjectPathPersistenceState) -> AppSettings {
    AppSettings {
        project_path_state: std::sync::Arc::new(state),
        ..AppSettings::default()
    }
}

#[test]
fn snapshot_serializes_camelcase_variants_and_omits_root_token() {
    let settings = AppSettings {
        root_token: Some("SUPER-SECRET-TOKEN-VALUE".to_string()),
        project_path_state: std::sync::Arc::new(state_with(
            vec![conflict_pair(), missing_pair()],
            vec![],
        )),
        ..AppSettings::default()
    };

    let snap = settings_snapshot_from(&settings, None);
    let json = serde_json::to_value(&snap).unwrap();

    // rootToken absent from both the flattened settings and anywhere else.
    let text = serde_json::to_string(&json).unwrap();
    assert!(!text.contains("rootToken"), "rootToken leaked: {text}");
    assert!(
        !text.contains("SUPER-SECRET-TOKEN-VALUE"),
        "token value leaked"
    );

    let resolution = &json["projectPathResolution"];
    assert_eq!(resolution["activeRegistrationCount"], 2);
    assert_eq!(resolution["archivedRegistrationCount"], 0);
    let issues = resolution["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 2);

    let conflict = &issues[0];
    assert_eq!(conflict["kind"], "conflict");
    assert_eq!(conflict["source"], "projectPaths");
    assert_eq!(conflict["index"], 0);
    assert!(conflict["absoluteResolvedPath"].is_string());
    assert!(conflict["instanceRelativeResolvedPath"].is_string());
    // id is a full lowercase 64-char hex.
    let id = conflict["id"].as_str().unwrap();
    assert_eq!(id.len(), 64);
    assert!(id
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));

    let missing = &issues[1];
    assert_eq!(missing["kind"], "missing");
    // Tagged raw states: absent relative candidate is present:false, value:null.
    assert_eq!(missing["instanceRelativeCandidate"]["present"], false);
    assert!(missing["instanceRelativeCandidate"]["value"].is_null());
    assert_eq!(missing["absoluteCandidate"]["present"], true);
    assert_eq!(missing["absoluteCandidate"]["value"], "/gone/x");
}

#[test]
fn structural_issue_forces_minimum_count_and_invalid_kind() {
    let structural = StructuralIssue {
        source: ProjectSource::ProjectPaths,
        reason: "plural primary is not an array of strings".to_string(),
        raw_absolute: RawJsonField {
            present: true,
            value: Some(serde_json::json!("not-an-array")),
        },
        raw_relative: RawJsonField {
            present: false,
            value: None,
        },
    };
    let settings = settings_with_state(state_with(vec![], vec![structural]));
    let snap = settings_snapshot_from(&settings, None);
    let json = serde_json::to_value(&snap).unwrap();
    let resolution = &json["projectPathResolution"];
    // Corruption-only startup is never pristine: min count of 1.
    assert_eq!(resolution["activeRegistrationCount"], 1);
    let issues = resolution["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0]["kind"], "invalid");
    // Invalid candidates are tagged RawJsonFieldState (wrong-typed value).
    assert_eq!(issues[0]["absoluteCandidate"]["present"], true);
    assert_eq!(issues[0]["absoluteCandidate"]["value"], "not-an-array");
}

#[test]
fn issue_id_is_stable_across_index_and_resolved_paths() {
    // Same kind/list/raw fields but different index → same id (index excluded).
    let mut a = missing_pair();
    let mut b = missing_pair();
    b.index = Some(9);
    let settings_a = settings_with_state(state_with(vec![a.clone()], vec![]));
    let settings_b = settings_with_state(state_with(vec![b], vec![]));
    let id_a = issue_id(&settings_snapshot_from(&settings_a, None));
    let id_b = issue_id(&settings_snapshot_from(&settings_b, None));
    assert_eq!(id_a, id_b);
    // A different raw absolute → different id.
    a.raw_absolute = RawStringField::string("/gone/y");
    let settings_c = settings_with_state(state_with(vec![a], vec![]));
    assert_ne!(id_a, issue_id(&settings_snapshot_from(&settings_c, None)));
}

#[test]
fn snapshot_carries_the_instance_settings_file_path() {
    // #1347: the UI names the file that holds the plaintext secrets. Assert
    // agreement with config_dir() rather than a literal path, so the test is
    // independent of where the test binary happens to live.
    let snap = settings_snapshot_from(&AppSettings::default(), None);
    match (crate::config::config_dir(), snap.settings_file_path) {
        (Some(dir), Some(path)) => {
            assert_eq!(
                path,
                dir.join("settings.30.instance.no-git.json")
                    .to_string_lossy()
            )
        }
        (None, None) => {}
        (dir, path) => {
            panic!("settings_file_path disagrees with config_dir: {dir:?} vs {path:?}")
        }
    }
}

fn issue_id(snap: &super::super::SettingsSnapshot) -> String {
    match &snap.project_path_resolution.issues[0] {
        ProjectPathIssue::Conflict { id, .. }
        | ProjectPathIssue::Missing { id, .. }
        | ProjectPathIssue::Invalid { id, .. } => id.clone(),
    }
}
