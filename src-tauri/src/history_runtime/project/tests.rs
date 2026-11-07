use super::*;

use uuid::Uuid;

use crate::context_history::{
    InitialKind, MatrixType, MetadataSource, RecordKind, TeamAttribution,
};

fn record(index: u128, project_path: &str, started: i64) -> HistoryRecord {
    HistoryRecord {
        schema_version: 2,
        record_id: Uuid::from_u128(index),
        app_run_id: Uuid::from_u128(1),
        runtime_session_id: Uuid::from_u128(2),
        logical_session_id: Uuid::from_u128(index),
        sequence: 1,
        ordinal: 1,
        observed_at_utc_ms: started,
        logical_started_at_utc_ms: Some(started),
        partial_start: false,
        partial_end: false,
        initial_record_id: Some(Uuid::from_u128(index)),
        initial_kind: InitialKind::FirstObserved,
        continuity_known: true,
        project_path: project_path.to_string(),
        project_name: "demo".to_string(),
        agent_key: format!("agent-{index:03}"),
        instance_fqn: format!("demo:room-1/agent-{index:03}"),
        room: Some("room-1".to_string()),
        matrix_type: MatrixType::Replica,
        team_attribution: TeamAttribution::Known,
        team_ids: vec!["team-a".to_string()],
        coding_agent_id: None,
        model: None,
        effort: None,
        metadata_source: MetadataSource::Unavailable,
        context_window: None,
        kind: RecordKind::Change,
        percent: Some(10),
        boundary_reason: None,
    }
}

#[test]
fn verbatim_unc_paths_stay_absolute_unc_paths() {
    assert_eq!(
        plain_path(r"\\?\UNC\server\share\proj"),
        r"\\server\share\proj"
    );
    assert_eq!(
        plain_path("//?/UNC/server/share/proj"),
        "//server/share/proj"
    );
    assert_eq!(plain_path(r"\\?\C:\proj"), r"C:\proj");
    assert_eq!(plain_path(r"C:\proj"), r"C:\proj");
}

/// A `subst` drive canonicalizes to its real folder; a record captured under
/// the drive spelling still matches the verified `--project` text.
#[test]
fn records_captured_under_the_typed_spelling_still_match() {
    let (real, typed) = if cfg!(windows) {
        (r"C:\real\proj", r"Q:\proj")
    } else {
        ("/real/proj", "/mnt/q/proj")
    };
    let spellings = [PathBuf::from(real), Path::new(typed).components().collect()];
    let mut records = vec![
        record(1, typed, 0),
        record(2, &format!("{typed}/."), 0),
        record(3, real, 0),
        record(4, "C:/elsewhere/other", 0),
    ];
    align_project_paths(&mut records, &spellings, real);
    let paths: Vec<&str> = records.iter().map(|r| r.project_path.as_str()).collect();
    assert_eq!(paths, [real, real, real, "C:/elsewhere/other"]);
}

#[test]
fn other_projects_and_other_cases_follow_the_platform_rule() {
    let real = if cfg!(windows) {
        r"C:\Real\Proj"
    } else {
        "/real/proj"
    };
    let upper = real.to_uppercase();
    let spellings = [PathBuf::from(real), PathBuf::from(real)];
    let mut records = vec![
        record(1, &upper, 0),
        record(2, &format!("{real}/sub"), 0),
        record(3, &format!("{real}/../proj"), 0),
        record(4, &format!(r"\\?\{real}"), 0),
    ];
    align_project_paths(&mut records, &spellings, real);
    let paths: Vec<String> = records.iter().map(|r| r.project_path.clone()).collect();
    let case = if cfg!(windows) {
        real.to_string()
    } else {
        upper.clone()
    };
    let expected = [
        case,
        format!("{real}/sub"),
        format!("{real}/../proj"),
        real.to_string(),
    ];
    assert_eq!(paths, expected);
}

#[test]
fn a_verified_project_exposes_canonical_typed_and_ac_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(tmp.path().join(".ac")).expect("create .ac");
    let typed = tmp.path().join(".");
    let project = verified_project(&typed).expect("verified");
    assert!(!project.canonical.to_string_lossy().starts_with(r"\\?\"));
    assert_eq!(project.spellings[0], project.canonical);
    assert_eq!(
        project.spellings[1],
        tmp.path().components().collect::<PathBuf>()
    );
    assert!(paths_equivalent(
        &project.ac_root,
        &project.canonical.join(".ac")
    ));
}

#[test]
fn unverifiable_projects_are_errors() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let missing = tmp.path().join("missing");
    assert!(matches!(
        verified_project(&missing),
        Err(ProjectError::NotVerified { .. })
    ));
    assert_eq!(
        verified_project(tmp.path()),
        Err(ProjectError::NoAcRoot {
            project: tmp.path().to_path_buf()
        })
    );
}
