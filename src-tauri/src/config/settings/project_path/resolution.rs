use std::path::{Path, PathBuf};

use super::codec::decode_instance_relative;
use super::identity::{
    compare_dirs, display_canonical, CandidateResolver, DirectoryIdentity, SameDir, SideOutcome,
    SideStatus,
};
use super::normalize_for_compare;

// ── #1077 resolution matrix, merge, quarantine, dedup (§3.4-§3.7) ───────────

/// Which persisted field group a logical registration belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectSource {
    /// The legacy singular `projectPath` pair.
    ProjectPath,
    /// The active plural `projectPaths` pair.
    ProjectPaths,
    /// The archived plural `archivedProjectPaths` pair.
    ArchivedProjectPaths,
}

/// Raw presence + value of one persisted string field. `present == false`
/// requires `value == None` (absent); `present == true, value == None` is an
/// explicit JSON null.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RawStringField {
    pub present: bool,
    pub value: Option<String>,
}

impl RawStringField {
    pub(crate) fn absent() -> Self {
        RawStringField {
            present: false,
            value: None,
        }
    }
    pub(crate) fn string(s: impl Into<String>) -> Self {
        RawStringField {
            present: true,
            value: Some(s.into()),
        }
    }
    pub(crate) fn null() -> Self {
        RawStringField {
            present: true,
            value: None,
        }
    }
}

/// One logical registration's raw absolute + instance-relative slots, extracted
/// from the six-field schema before resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawPair {
    pub source: ProjectSource,
    pub index: Option<usize>,
    pub absolute: RawStringField,
    pub relative: RawStringField,
}

/// Blocking issue classification for a registration that selected no path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IssueKind {
    Conflict,
    Missing,
    Invalid,
}

/// Raw presence + value of a persisted field as arbitrary JSON (absent vs null
/// vs value). Used to preserve and report structurally-corrupt fields.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RawJsonField {
    pub present: bool,
    pub value: Option<serde_json::Value>,
}

/// A structural-schema corruption in one project field group (wrong JSON type,
/// misaligned companion, orphan companion, etc.). Distinct from a per-pair
/// candidate that merely fails to load: it exposes no entry from the affected
/// list, preserves the raw bytes, and blocks all reconciliation/mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StructuralIssue {
    pub source: ProjectSource,
    pub reason: String,
    pub raw_absolute: RawJsonField,
    pub raw_relative: RawJsonField,
}

/// The write, if any, a resolved pair needs to reconcile its companion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepairKind {
    /// Clean; no write.
    None,
    /// Valid absolute, companion absent/stale → generate a fresh companion.
    PopulateCompanion,
    /// Relative won → replace the stale absolute and refresh the companion.
    ReplaceStaleAbsolute,
    /// Both valid same directory → normalize a noncanonical pair.
    NormalizePair,
}

/// The fully resolved state of one logical registration (hidden persisted state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedPair {
    pub source: ProjectSource,
    pub index: Option<usize>,
    pub raw_absolute: RawStringField,
    pub raw_relative: RawStringField,
    pub absolute_side: SideOutcome,
    pub relative_side: SideOutcome,
    /// Selected outward (display) canonical absolute path, if any.
    pub selected: Option<String>,
    /// Raw canonical (verbatim on Windows) of the selected side, for identity
    /// and component-scope comparison.
    pub selected_canonical_raw: Option<String>,
    /// Identity of the selected directory, if known.
    pub selected_identity: Option<DirectoryIdentity>,
    /// Non-`None` when this registration selected no path (or was quarantined).
    pub issue: Option<IssueKind>,
    /// The companion repair this pair needs, when it selected a path.
    pub repair: RepairKind,
}

impl ResolvedPair {
    fn is_selected(&self) -> bool {
        self.selected.is_some() && self.issue.is_none()
    }
}

/// Hidden persisted project-path state carried on `AppSettings` behind an
/// `Arc` (`#[serde(skip)]`). Retains every resolved pair's source/order/raw
/// values/selected path/identity/issue and the reconcile-eligibility bits, so
/// writers can rebuild companion arrays and the snapshot can report issues
/// without re-reading disk. Mutators use `Arc::make_mut` for copy-on-write.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ProjectPathPersistenceState {
    /// Every resolved pair (hidden state) in (active plural, genuine singular,
    /// archived) order. The first `active_registration_count` entries are the
    /// active group; the rest are archived.
    pub pairs: Vec<ResolvedPair>,
    /// Legacy singular head = first active selected path (or None).
    pub selected_head: Option<String>,
    /// Merged logical active registration count (before identity dedupe/quarantine).
    pub active_registration_count: usize,
    /// Merged logical archived registration count.
    pub archived_registration_count: usize,
    /// Whether the active plural companion array was present & aligned on disk.
    pub active_companion_present: bool,
    /// Whether the archived plural companion array was present & aligned on disk.
    pub archived_companion_present: bool,
    /// Whether a genuinely-different singular is appended to the active group.
    pub has_genuine_singular: bool,
    /// Whether the active group has a dirty, eligible repair to write.
    pub active_reconcile_eligible: bool,
    /// Whether the archived group has a dirty, eligible repair to write.
    pub archived_reconcile_eligible: bool,
    /// Structural-schema corruptions found in the raw fields. Any entry blocks
    /// all reconciliation and project-list mutation while unrelated preserve
    /// saves remain allowed.
    pub structural_issues: Vec<StructuralIssue>,
    /// Set for a state synthesized from a direct-constructed `AppSettings`'s
    /// runtime lists (no decoder-produced hidden state). Such a state is legacy
    /// runtime-authoritative: a Reconcile write emits its groups verbatim rather
    /// than copying disk, matching pre-#1077 verbatim-writer behavior until the
    /// explicit mutators maintain a real hidden state.
    pub runtime_authoritative: bool,
}

impl ProjectPathPersistenceState {
    /// The active-group resolved pairs (plural then genuine singular).
    pub(crate) fn active_pairs(&self) -> &[ResolvedPair] {
        &self.pairs[..self.active_registration_count]
    }
    /// The archived-group resolved pairs.
    pub(crate) fn archived_pairs(&self) -> &[ResolvedPair] {
        &self.pairs[self.active_registration_count..]
    }
    /// Selected canonical (display) active paths, in order, filtered of
    /// unresolved/quarantined/deduped entries.
    pub(crate) fn active_selected(&self) -> Vec<String> {
        self.active_pairs()
            .iter()
            .filter_map(|p| p.selected.clone())
            .collect()
    }
    /// Archived management projection (§3.6): every archived pair that is either
    /// selected or a non-conflicting removable stored row (missing/invalid),
    /// keeping "Remove from list" working for a project whose directory vanished
    /// while never exposing one side of an archived conflict. This is the runtime
    /// `archivedProjectPaths` projection; the active list stays selected-only so
    /// session restoration never restores an unvalidated project.
    pub(crate) fn archived_management_paths(&self) -> Vec<String> {
        self.archived_pairs()
            .iter()
            .filter_map(|p| match (&p.selected, p.issue) {
                (Some(sel), _) => Some(sel.clone()),
                (None, Some(IssueKind::Conflict)) => None,
                (None, Some(_)) => p.raw_absolute.value.clone(),
                (None, None) => None,
            })
            .collect()
    }
    /// Blocking issues (registrations that selected no path), in `pairs` order.
    pub(crate) fn issues(&self) -> impl Iterator<Item = &ResolvedPair> {
        self.pairs.iter().filter(|p| p.issue.is_some())
    }
    /// Whether any structural corruption is present (blocks all reconcile/mutation).
    pub(crate) fn has_structural(&self) -> bool {
        !self.structural_issues.is_empty()
    }
}

/// Resolve one side (absolute or relative) of a registration against the base.
pub(super) fn resolve_side(
    field: &RawStringField,
    is_relative: bool,
    base: Option<&Path>,
    resolver: &dyn CandidateResolver,
) -> SideOutcome {
    // Absent or explicit null → nothing to resolve.
    let value = match (field.present, field.value.as_deref()) {
        (false, _) | (true, None) => return SideOutcome::failed(SideStatus::Absent, None),
        (true, Some(v)) => v,
    };

    if is_relative {
        let Some(base) = base else {
            return SideOutcome::failed(SideStatus::BaseUnavailable, None);
        };
        match decode_instance_relative(value, base) {
            Ok(syntactic) => resolver.resolve(&syntactic),
            Err(_) => SideOutcome::failed(SideStatus::Malformed, None),
        }
    } else {
        let abs = PathBuf::from(value);
        if !abs.is_absolute() {
            return SideOutcome::failed(SideStatus::Malformed, Some(value.to_string()));
        }
        resolver.resolve(&abs)
    }
}

/// Classify a no-selection outcome. Returns `None` for a genuinely empty pair
/// (both sides absent), `Some(Missing)` when the only non-absent statuses are
/// definite `NotFound`, and `Some(Invalid)` otherwise.
pub(super) fn classify_no_selection(abs: SideStatus, rel: SideStatus) -> Option<IssueKind> {
    let mut any = false;
    let mut all_missing = true;
    for status in [abs, rel] {
        if status == SideStatus::Absent {
            continue;
        }
        any = true;
        if status != SideStatus::Missing {
            all_missing = false;
        }
    }
    if !any {
        None
    } else if all_missing {
        Some(IssueKind::Missing)
    } else {
        Some(IssueKind::Invalid)
    }
}

/// Combine both resolved sides into a selected path + repair need + issue,
/// per the §3.5 resolution matrix.
pub(super) fn combine_sides(pair: RawPair, abs: SideOutcome, rel: SideOutcome) -> ResolvedPair {
    let av = abs.status.is_valid();
    let rv = rel.status.is_valid();

    let mut selected = None;
    let mut selected_canonical_raw = None;
    let mut selected_identity = None;
    let mut issue = None;
    let mut repair = RepairKind::None;

    match (av, rv) {
        (true, true) => {
            let same = compare_dirs(
                abs.canonical_path.as_deref().unwrap_or_default(),
                abs.identity.as_ref(),
                rel.canonical_path.as_deref().unwrap_or_default(),
                rel.identity.as_ref(),
            );
            match same {
                SameDir::Same => {
                    // Prefer the absolute candidate's canonical form. A valid side
                    // always carries a canonical path; default defensively rather
                    // than panicking if that invariant is ever weakened.
                    let canon = abs.canonical_path.clone().unwrap_or_default();
                    selected = Some(display_canonical(&canon));
                    selected_canonical_raw = Some(canon);
                    selected_identity = abs.identity;
                    // Normalize if the persisted spellings are not already the
                    // selected canonical forms.
                    if !pair_is_canonical(&pair, selected.as_deref(), &rel) {
                        repair = RepairKind::NormalizePair;
                    }
                }
                SameDir::Different => {
                    issue = Some(IssueKind::Conflict);
                }
                SameDir::Unavailable => {
                    issue = Some(IssueKind::Invalid);
                }
            }
        }
        (true, false) => {
            let canon = abs.canonical_path.clone().unwrap_or_default();
            selected = Some(display_canonical(&canon));
            selected_canonical_raw = Some(canon);
            selected_identity = abs.identity;
            // Populate/repair the companion when the relative side did not load.
            repair = RepairKind::PopulateCompanion;
        }
        (false, true) => {
            let canon = rel.canonical_path.clone().unwrap_or_default();
            selected = Some(display_canonical(&canon));
            selected_canonical_raw = Some(canon);
            selected_identity = rel.identity;
            repair = RepairKind::ReplaceStaleAbsolute;
        }
        (false, false) => {
            issue = classify_no_selection(abs.status, rel.status);
        }
    }

    ResolvedPair {
        source: pair.source,
        index: pair.index,
        raw_absolute: pair.absolute,
        raw_relative: pair.relative,
        absolute_side: abs,
        relative_side: rel,
        selected,
        selected_canonical_raw,
        selected_identity,
        issue,
        repair,
    }
}

/// Whether the persisted pair already stores the selected absolute path (display
/// form) plus a present, non-drifting companion, so no normalization is needed.
pub(super) fn pair_is_canonical(pair: &RawPair, selected: Option<&str>, rel: &SideOutcome) -> bool {
    let Some(selected) = selected else {
        return false;
    };
    let abs_matches = pair
        .absolute
        .value
        .as_deref()
        .map(|v| v == selected)
        .unwrap_or(false);
    // The companion is fine if it is present and it resolved to the same dir.
    let rel_ok = pair.relative.present && rel.status.is_valid();
    abs_matches && rel_ok
}

/// Resolve the full six-field record set: run the §3.5 matrix per registration,
/// merge the legacy singular mirror, quarantine conflict scopes, deduplicate by
/// identity, and build the runtime selected lists plus hidden state.
///
/// `base` is the already-canonicalized instance base (or `None`). The
/// `*_companion_present` flags say whether each plural relative array was
/// present and length-aligned on disk (drives reconcile eligibility, §4.2).
pub(crate) fn resolve_registrations(
    active_plural: &[RawPair],
    singular: Option<RawPair>,
    archived: &[RawPair],
    base: Option<&Path>,
    active_companion_present: bool,
    archived_companion_present: bool,
    resolver: &dyn CandidateResolver,
) -> ProjectPathPersistenceState {
    let resolve_pair = |pair: RawPair| -> ResolvedPair {
        let abs = resolve_side(&pair.absolute, false, base, resolver);
        let rel = resolve_side(&pair.relative, true, base, resolver);
        combine_sides(pair, abs, rel)
    };

    let mut active: Vec<ResolvedPair> = active_plural.iter().cloned().map(resolve_pair).collect();

    // Merge the legacy singular mirror (§3.6).
    let mut genuine_singular = false;
    if let Some(sing) = singular {
        let resolved_singular = resolve_pair(sing);
        match active.first() {
            Some(first) if singular_mirrors_first(&resolved_singular, first) => {
                // Redundant mirror of the first plural entry; drop it.
            }
            _ => {
                genuine_singular = true;
                active.push(resolved_singular);
            }
        }
    }

    let mut archived: Vec<ResolvedPair> = archived.iter().cloned().map(resolve_pair).collect();

    // Counts BEFORE dedupe/quarantine (redundant singular already excluded; a
    // genuinely different singular counts as one extra active record).
    let active_registration_count = active.len();
    let archived_registration_count = archived.len();

    // Conflict-quarantine pass (§3.4): suppress any otherwise-selected record
    // whose identity or component scope overlaps either candidate of a conflict.
    let quarantine = build_quarantine_set(&active, &archived);
    apply_quarantine(&mut active, &quarantine);
    apply_quarantine(&mut archived, &quarantine);

    // Identity dedupe (§3.6): active in order first, then archived; active wins.
    dedupe_selected(&mut active, &mut archived);

    // Reconcile eligibility per group (§4.2).
    let active_reconcile_eligible =
        group_reconcile_eligible(&active, active_companion_present, genuine_singular);
    let archived_reconcile_eligible =
        group_reconcile_eligible(&archived, archived_companion_present, false);

    let selected_head = active.iter().find_map(|p| p.selected.clone());

    let mut pairs = active;
    pairs.extend(archived);

    ProjectPathPersistenceState {
        pairs,
        selected_head,
        active_registration_count,
        archived_registration_count,
        active_companion_present,
        archived_companion_present,
        has_genuine_singular: genuine_singular,
        active_reconcile_eligible,
        archived_reconcile_eligible,
        structural_issues: Vec::new(),
        runtime_authoritative: false,
    }
}

/// Whether a resolved singular is the redundant mirror of the first plural
/// entry: matching raw absolute spelling (platform-correct) or the same
/// validated directory identity.
pub(super) fn singular_mirrors_first(singular: &ResolvedPair, first: &ResolvedPair) -> bool {
    if let (Some(a), Some(b)) = (
        singular.raw_absolute.value.as_deref(),
        first.raw_absolute.value.as_deref(),
    ) {
        if normalize_for_compare(a) == normalize_for_compare(b) {
            return true;
        }
    }
    if let (Some(a), Some(b)) = (
        &singular.selected_canonical_raw,
        &first.selected_canonical_raw,
    ) {
        return matches!(
            compare_dirs(
                a,
                singular.selected_identity.as_ref(),
                b,
                first.selected_identity.as_ref()
            ),
            SameDir::Same
        );
    }
    false
}

/// Both canonical identities and canonical component paths of every
/// both-valid/different (conflict) pair.
pub(super) struct QuarantineSet {
    identities: Vec<DirectoryIdentity>,
    canonical_paths: Vec<String>,
}

pub(super) fn build_quarantine_set(
    active: &[ResolvedPair],
    archived: &[ResolvedPair],
) -> QuarantineSet {
    let mut identities = Vec::new();
    let mut canonical_paths = Vec::new();
    for pair in active.iter().chain(archived.iter()) {
        if pair.issue == Some(IssueKind::Conflict) {
            for side in [&pair.absolute_side, &pair.relative_side] {
                if let Some(id) = side.identity {
                    identities.push(id);
                }
                if let Some(canon) = &side.canonical_path {
                    canonical_paths.push(canon.clone());
                }
            }
        }
    }
    QuarantineSet {
        identities,
        canonical_paths,
    }
}

/// Suppress any selected pair whose identity equals a quarantined candidate or
/// whose canonical scope overlaps one component-wise (ancestor/descendant/equal).
pub(super) fn apply_quarantine(pairs: &mut [ResolvedPair], quarantine: &QuarantineSet) {
    for pair in pairs.iter_mut() {
        if !pair.is_selected() {
            continue;
        }
        let id_hit = pair
            .selected_identity
            .as_ref()
            .map(|id| quarantine.identities.contains(id))
            .unwrap_or(false);
        let scope_hit = pair
            .selected_canonical_raw
            .as_deref()
            .map(|canon| {
                quarantine
                    .canonical_paths
                    .iter()
                    .any(|q| paths_overlap(canon, q))
            })
            .unwrap_or(false);
        if id_hit || scope_hit {
            pair.selected = None;
            pair.selected_canonical_raw = None;
            pair.selected_identity = None;
            pair.repair = RepairKind::None;
            pair.issue = Some(IssueKind::Invalid);
        }
    }
}

/// Component-wise ancestor/descendant/equal overlap of two canonical paths.
pub(super) fn paths_overlap(a: &str, b: &str) -> bool {
    let (pa, pb) = (Path::new(a), Path::new(b));
    pa.starts_with(pb) || pb.starts_with(pa)
}

/// Identity dedupe across selected records: active in order then archived, active
/// winning over a duplicate archived entry. Identity-unavailable comparison of
/// two distinct-canonical selected records fails the later one closed.
pub(super) fn dedupe_selected(active: &mut [ResolvedPair], archived: &mut [ResolvedPair]) {
    let mut kept: Vec<(String, Option<DirectoryIdentity>)> = Vec::new();
    // Two passes over one logical ordering: active first (wins), then archived.
    for pair in active.iter_mut().chain(archived.iter_mut()) {
        if !pair.is_selected() {
            continue;
        }
        let canon = pair.selected_canonical_raw.clone().unwrap_or_default();
        let id = pair.selected_identity;
        let mut duplicate = false;
        let mut unavailable = false;
        for (kcanon, kid) in &kept {
            match compare_dirs(&canon, id.as_ref(), kcanon, kid.as_ref()) {
                SameDir::Same => {
                    duplicate = true;
                    break;
                }
                SameDir::Unavailable => {
                    unavailable = true;
                    break;
                }
                SameDir::Different => {}
            }
        }
        if duplicate {
            // Silent dedupe: drop the later duplicate from runtime.
            pair.selected = None;
            pair.selected_canonical_raw = None;
            pair.selected_identity = None;
            pair.repair = RepairKind::None;
        } else if unavailable {
            // Cannot rule out a duplicate → fail closed.
            pair.selected = None;
            pair.selected_canonical_raw = None;
            pair.selected_identity = None;
            pair.repair = RepairKind::None;
            pair.issue = Some(IssueKind::Invalid);
        } else {
            kept.push((canon, id));
        }
    }
}

/// Whether a field group has a dirty, write-eligible repair (§4.2). A present,
/// aligned companion array can always copy unresolved slots, so any dirty pair
/// makes the group eligible. An absent companion array requires every retained
/// entry to have selected (one unresolved legacy entry blocks the group). A
/// genuinely different unresolved active singular also blocks the active group.
pub(super) fn group_reconcile_eligible(
    pairs: &[ResolvedPair],
    companion_present: bool,
    genuine_singular: bool,
) -> bool {
    let has_dirty = pairs
        .iter()
        .any(|p| p.is_selected() && p.repair != RepairKind::None);
    if !has_dirty {
        return false;
    }
    let has_unresolved = pairs.iter().any(|p| p.issue.is_some());
    if genuine_singular && has_unresolved {
        return false;
    }
    if companion_present {
        true
    } else {
        // Absent companion: cannot invent a null slot for an unresolved entry.
        !has_unresolved
    }
}
