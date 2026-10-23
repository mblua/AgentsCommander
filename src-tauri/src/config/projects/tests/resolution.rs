use super::*;
use std::collections::HashMap;
use std::path::Path;

/// Injected resolver mapping syntactic path strings to canned outcomes.
struct MapResolver {
    map: HashMap<String, SideOutcome>,
}

impl CandidateResolver for MapResolver {
    fn resolve(&self, syntactic: &Path) -> SideOutcome {
        self.map
            .get(&syntactic.to_string_lossy().to_string())
            .cloned()
            .unwrap_or_else(|| {
                SideOutcome::failed(SideStatus::Missing, syntactic.to_str().map(str::to_string))
            })
    }
}

fn valid(canonical: &str, kind: SideStatus, vol: u64, file: u128) -> SideOutcome {
    SideOutcome {
        status: kind,
        syntactic_path: Some(canonical.to_string()),
        canonical_path: Some(canonical.to_string()),
        identity: Some(DirectoryIdentity { volume: vol, file }),
    }
}

fn active_pair(idx: usize, abs: &str, rel: Option<&str>) -> RawPair {
    RawPair {
        source: ProjectSource::ProjectPaths,
        index: Some(idx),
        absolute: RawStringField::string(abs),
        relative: match rel {
            Some(r) => RawStringField::string(r),
            None => RawStringField::absent(),
        },
    }
}

// Base has a single normal component so a plain relative wire resolves
// directly under `/opt` (matching the `abs()` helper below).
fn base() -> &'static Path {
    Path::new(if cfg!(windows) { r"C:\opt" } else { "/opt" })
}

fn abs(p: &str) -> String {
    if cfg!(windows) {
        format!(r"C:\opt\{}", p.replace('/', "\\"))
    } else {
        format!("/opt/{}", p)
    }
}

#[test]
fn absolute_only_selects_and_wants_companion() {
    let a = abs("projects/alpha");
    let mut map = HashMap::new();
    map.insert(a.clone(), valid(&a, SideStatus::ValidDirectProject, 1, 10));
    let resolver = MapResolver { map };
    let report = resolve_registrations(
        &[active_pair(0, &a, None)],
        None,
        &[],
        Some(base()),
        false,
        false,
        &resolver,
    );
    assert_eq!(report.active_selected(), vec![display_canonical(&a)]);
    assert_eq!(report.pairs[0].repair, RepairKind::PopulateCompanion);
    assert_eq!(report.active_registration_count, 1);
    assert!(report.issues().next().is_none());
}

#[test]
fn relative_wins_when_absolute_stale() {
    // Absolute is missing; relative decodes to a valid dir.
    let stale = abs("old/alpha");
    let rel_target = abs("projects/alpha");
    let mut map = HashMap::new();
    map.insert(
        rel_target.clone(),
        valid(&rel_target, SideStatus::ValidDirectProject, 1, 11),
    );
    // stale absolute not in map → Missing.
    let resolver = MapResolver { map };
    let report = resolve_registrations(
        &[active_pair(0, &stale, Some("projects/alpha"))],
        None,
        &[],
        Some(base()),
        true,
        false,
        &resolver,
    );
    assert_eq!(
        report.active_selected(),
        vec![display_canonical(&rel_target)]
    );
    assert_eq!(report.pairs[0].repair, RepairKind::ReplaceStaleAbsolute);
}

#[test]
fn both_valid_different_is_conflict_selecting_neither() {
    let a = abs("projects/alpha");
    let rel_target = abs("projects/beta");
    let mut map = HashMap::new();
    map.insert(a.clone(), valid(&a, SideStatus::ValidDirectProject, 1, 20));
    map.insert(
        rel_target.clone(),
        valid(&rel_target, SideStatus::ValidDirectProject, 1, 21),
    );
    let resolver = MapResolver { map };
    let report = resolve_registrations(
        &[active_pair(0, &a, Some("projects/beta"))],
        None,
        &[],
        Some(base()),
        true,
        false,
        &resolver,
    );
    assert!(report.active_selected().is_empty());
    assert_eq!(report.pairs[0].issue, Some(IssueKind::Conflict));
}

#[test]
fn both_valid_same_identity_selects_absolute() {
    let a = abs("projects/alpha");
    let rel_target = abs("aliased/alpha"); // different canonical, same identity
    let mut map = HashMap::new();
    map.insert(a.clone(), valid(&a, SideStatus::ValidDirectProject, 7, 30));
    map.insert(
        rel_target.clone(),
        valid(&rel_target, SideStatus::ValidDirectProject, 7, 30),
    );
    let resolver = MapResolver { map };
    let report = resolve_registrations(
        &[active_pair(0, &a, Some("aliased/alpha"))],
        None,
        &[],
        Some(base()),
        true,
        false,
        &resolver,
    );
    assert_eq!(report.active_selected(), vec![display_canonical(&a)]);
    assert!(report.pairs[0].issue.is_none());
}

#[test]
fn quarantine_suppresses_duplicate_of_conflict_candidate() {
    // Registration 0 is a conflict between alpha and beta.
    // Registration 1 independently selects alpha → must be quarantined.
    let a = abs("projects/alpha");
    let b = abs("projects/beta");
    let mut map = HashMap::new();
    map.insert(a.clone(), valid(&a, SideStatus::ValidDirectProject, 1, 40));
    map.insert(b.clone(), valid(&b, SideStatus::ValidDirectProject, 1, 41));
    let resolver = MapResolver { map };
    let report = resolve_registrations(
        &[
            active_pair(0, &a, Some("projects/beta")),
            active_pair(1, &a, None),
        ],
        None,
        &[],
        Some(base()),
        true,
        false,
        &resolver,
    );
    assert!(
        report.active_selected().is_empty(),
        "alpha reintroduced despite conflict"
    );
    assert_eq!(report.pairs[0].issue, Some(IssueKind::Conflict));
    assert_eq!(report.pairs[1].issue, Some(IssueKind::Invalid));
}

#[test]
fn quarantine_scope_overlap_suppresses_child_registration() {
    // Conflict on a collection root; a child registration overlaps its scope.
    let root = abs("collection");
    let other = abs("elsewhere");
    let child = abs("collection/child");
    let mut map = HashMap::new();
    map.insert(
        root.clone(),
        valid(&root, SideStatus::ValidCollectionRoot, 1, 50),
    );
    map.insert(
        other.clone(),
        valid(&other, SideStatus::ValidDirectProject, 1, 51),
    );
    map.insert(
        child.clone(),
        valid(&child, SideStatus::ValidDirectProject, 1, 52),
    );
    let resolver = MapResolver { map };
    let report = resolve_registrations(
        &[
            active_pair(0, &root, Some("elsewhere")),
            active_pair(1, &child, None),
        ],
        None,
        &[],
        Some(base()),
        true,
        false,
        &resolver,
    );
    assert!(report.active_selected().is_empty());
    assert_eq!(report.pairs[1].issue, Some(IssueKind::Invalid));
}

#[test]
fn dedupe_keeps_first_active_and_active_beats_archived() {
    let a = abs("projects/alpha");
    let mut map = HashMap::new();
    map.insert(a.clone(), valid(&a, SideStatus::ValidDirectProject, 1, 60));
    let resolver = MapResolver { map };
    let archived = vec![RawPair {
        source: ProjectSource::ArchivedProjectPaths,
        index: Some(0),
        absolute: RawStringField::string(&a),
        relative: RawStringField::absent(),
    }];
    let report = resolve_registrations(
        &[active_pair(0, &a, None), active_pair(1, &a, None)],
        None,
        &archived,
        Some(base()),
        false,
        false,
        &resolver,
    );
    assert_eq!(report.active_selected(), vec![display_canonical(&a)]);
    assert!(
        report.archived_management_paths().is_empty(),
        "archived dup of active must drop"
    );
}

#[test]
fn identity_unavailable_dual_candidates_fail_closed() {
    // Both sides valid, different canonical, NO identities → Unavailable.
    let a = abs("projects/alpha");
    let rel_target = abs("projects/beta");
    let no_id = |canon: &str| SideOutcome {
        status: SideStatus::ValidDirectProject,
        syntactic_path: Some(canon.to_string()),
        canonical_path: Some(canon.to_string()),
        identity: None,
    };
    let mut map = HashMap::new();
    map.insert(a.clone(), no_id(&a));
    map.insert(rel_target.clone(), no_id(&rel_target));
    let resolver = MapResolver { map };
    let report = resolve_registrations(
        &[active_pair(0, &a, Some("projects/beta"))],
        None,
        &[],
        Some(base()),
        true,
        false,
        &resolver,
    );
    assert!(report.active_selected().is_empty());
    assert_eq!(report.pairs[0].issue, Some(IssueKind::Invalid));
}

#[test]
fn neither_valid_definite_notfound_is_missing() {
    let a = abs("gone");
    let resolver = MapResolver {
        map: HashMap::new(),
    }; // everything Missing
    let report = resolve_registrations(
        &[active_pair(0, &a, None)],
        None,
        &[],
        Some(base()),
        false,
        false,
        &resolver,
    );
    assert!(report.active_selected().is_empty());
    assert_eq!(report.pairs[0].issue, Some(IssueKind::Missing));
}

#[test]
fn base_unavailable_uses_absolute_only_and_preserves_relative() {
    let a = abs("projects/alpha");
    let mut map = HashMap::new();
    map.insert(a.clone(), valid(&a, SideStatus::ValidDirectProject, 1, 70));
    let resolver = MapResolver { map };
    let report = resolve_registrations(
        &[active_pair(0, &a, Some("projects/alpha"))],
        None,
        &[],
        None, // no base
        true,
        false,
        &resolver,
    );
    assert_eq!(report.active_selected(), vec![display_canonical(&a)]);
    assert_eq!(
        report.pairs[0].relative_side.status,
        SideStatus::BaseUnavailable
    );
    // The raw relative value is retained in hidden state.
    assert_eq!(
        report.pairs[0].raw_relative,
        RawStringField::string("projects/alpha")
    );
}

#[test]
fn genuine_singular_appends_and_blocks_active_when_unresolved() {
    let a = abs("projects/alpha");
    let sing = abs("gone/singular");
    let mut map = HashMap::new();
    map.insert(a.clone(), valid(&a, SideStatus::ValidDirectProject, 1, 80));
    // sing not in map → Missing → unresolved genuine singular.
    let resolver = MapResolver { map };
    let singular = RawPair {
        source: ProjectSource::ProjectPath,
        index: None,
        absolute: RawStringField::string(&sing),
        relative: RawStringField::absent(),
    };
    let report = resolve_registrations(
        &[active_pair(0, &a, None)],
        Some(singular),
        &[],
        Some(base()),
        true,
        false,
        &resolver,
    );
    assert_eq!(
        report.active_registration_count, 2,
        "genuine singular counts"
    );
    assert_eq!(report.active_selected(), vec![display_canonical(&a)]);
    // The unresolved genuine singular blocks active reconciliation.
    assert!(!report.active_reconcile_eligible);
}

#[test]
fn redundant_singular_is_dropped() {
    let a = abs("projects/alpha");
    let mut map = HashMap::new();
    map.insert(a.clone(), valid(&a, SideStatus::ValidDirectProject, 1, 90));
    let resolver = MapResolver { map };
    let singular = RawPair {
        source: ProjectSource::ProjectPath,
        index: None,
        absolute: RawStringField::string(&a),
        relative: RawStringField::absent(),
    };
    let report = resolve_registrations(
        &[active_pair(0, &a, None)],
        Some(singular),
        &[],
        Some(base()),
        false,
        false,
        &resolver,
    );
    assert_eq!(
        report.active_registration_count, 1,
        "redundant singular not counted"
    );
}
