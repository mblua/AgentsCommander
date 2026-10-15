use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::ac_root::{
    ensure_authoritative_ac_root, is_ac_root_name, CANONICAL_AC_ROOT_DIR,
};
use crate::config::settings::{
    empty_profile_cell, normalize_profile_letter, AppSettings, ProfileCellConfig,
};

#[derive(Debug, Clone)]
pub struct ProfileResolutionRequest<'a> {
    pub coding_agent_id: &'a str,
    pub launch_path: Option<&'a Path>,
    pub agent_matrix_name: Option<&'a str>,
    pub requested_profile: Option<&'a str>,
    /// When true and `requested_profile` is present, the explicit request
    /// outranks the instance override (replica pin) for this resolution only.
    /// Wake dispatch sets this; all other callers leave it false.
    pub requested_profile_authoritative: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileResolution {
    pub requested_profile: String,
    pub effective_profile: String,
    pub fallback_chain: Vec<String>,
    pub fallback_applied: bool,
    pub cell: ProfileCellConfig,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedProfileAgentPath {
    pub launch_path: PathBuf,
    pub origin_matrix_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSelectionResolution {
    pub requested_profile_input: Option<String>,
    pub instance_profile_override: Option<String>,
    pub origin_default_profile: Option<String>,
    pub agent_default_profile: Option<String>,
    pub resolution: ProfileResolution,
}

fn read_json_object(path: &Path) -> Option<Value> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Value>(&content).ok()
}

/// #2786 (C1) - through the shared loader, so a state key such as
/// `profileContentHash` is served from the state file first.
fn read_tooling_string(agent_dir: &Path, key: &str) -> Option<String> {
    crate::config::agent_config::read_agent_local_config_json(agent_dir)
        .ok()??
        .get("tooling")?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

/// #2816 (C3) - which file of the config pair a `tooling` string lives in.
#[derive(Clone, Copy)]
enum ToolingSide {
    Decisions,
    State,
}

fn write_tooling_string(
    agent_dir: &Path,
    side: ToolingSide,
    key: &str,
    value: Option<&str>,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(agent_dir).map_err(|e| {
        format!(
            "Agent config dir '{}' is not readable: {}",
            agent_dir.display(),
            e
        )
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "Agent config dir '{}' is not a real directory",
            agent_dir.display()
        ));
    }
    let config_path = agent_dir.join("config.json");
    crate::config::agent_config::update_agent_config(&config_path, |decisions, state| {
        let obj = match side {
            ToolingSide::Decisions => decisions,
            ToolingSide::State => state,
        };
        let tooling_value = obj
            .entry("tooling".to_string())
            .or_insert_with(|| serde_json::json!({}));
        // #1939 - a malformed `tooling` is never silently reset: the write fails
        // with the stored bytes untouched (shape-based, like agent_config).
        let tooling = tooling_value
            .as_object_mut()
            .ok_or_else(|| format!("tooling must be a JSON object at {}", config_path.display()))?;
        match value {
            Some(value) => {
                tooling.insert(key.to_string(), Value::String(value.to_string()));
            }
            None => {
                tooling.remove(key);
            }
        }
        Ok(())
    })?;
    Ok(())
}

fn strip_extended_prefix(path: PathBuf) -> PathBuf {
    crate::path_utils::normalize_windows_verbatim_path_buf(&path)
}

fn canonical_existing_dir(path: &Path, label: &str) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("{} '{}' is not readable: {}", label, path.display(), e))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "{} '{}' is not a real directory",
            label,
            path.display()
        ));
    }
    std::fs::canonicalize(path)
        .map(strip_extended_prefix)
        .map_err(|e| {
            format!(
                "Failed to canonicalize {} '{}': {}",
                label,
                path.display(),
                e
            )
        })
}

#[cfg(windows)]
fn same_canonical_path(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(not(windows))]
fn same_canonical_path(left: &Path, right: &Path) -> bool {
    left == right
}

fn collect_ac_root_candidate(out: &mut Vec<PathBuf>, candidate: &Path) {
    if !candidate.is_dir() || ensure_authoritative_ac_root(candidate).is_err() {
        return;
    }
    let Ok(canonical) = canonical_existing_dir(candidate, "Project AC Root") else {
        return;
    };
    if !out
        .iter()
        .any(|existing| same_canonical_path(existing, &canonical))
    {
        out.push(canonical);
    }
}

pub(crate) fn configured_ac_roots(settings: &AppSettings) -> Vec<PathBuf> {
    let mut ac_roots = Vec::new();
    for project_path in &settings.project_paths {
        let base = Path::new(project_path);
        if base
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_ac_root_name)
        {
            collect_ac_root_candidate(&mut ac_roots, base);
        }

        collect_ac_root_candidate(&mut ac_roots, &base.join(CANONICAL_AC_ROOT_DIR));

        let Ok(entries) = std::fs::read_dir(base) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                collect_ac_root_candidate(&mut ac_roots, &entry.path().join(CANONICAL_AC_ROOT_DIR));
            }
        }
    }
    ac_roots
}

fn ensure_ac_root_is_configured(settings: &AppSettings, ac_root: &Path) -> Result<(), String> {
    let ac_root = canonical_existing_dir(ac_root, "Project AC Root")?;
    let configured = configured_ac_roots(settings);
    if configured
        .iter()
        .any(|candidate| same_canonical_path(candidate, &ac_root))
    {
        return Ok(());
    }

    Err(format!(
        "Agent path is outside configured AC project roots: {}",
        ac_root.display()
    ))
}

fn validate_authoritative_matrix_dir(matrix_dir: &Path) -> Result<PathBuf, String> {
    let matrix_dir = canonical_existing_dir(matrix_dir, "Agent Matrix")?;
    let dir_name = matrix_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Agent Matrix '{}' has no valid name", matrix_dir.display()))?;
    if !dir_name.starts_with("_agent_") {
        return Err(format!(
            "Agent Matrix '{}' must be named '_agent_<name>'",
            matrix_dir.display()
        ));
    }
    let ac_root = matrix_dir.parent().ok_or_else(|| {
        format!(
            "Agent Matrix '{}' has no parent Project AC Root",
            matrix_dir.display()
        )
    })?;
    ensure_authoritative_ac_root(ac_root)?;
    Ok(matrix_dir)
}

fn validated_replica_origin(replica_dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    let replica_dir = canonical_existing_dir(replica_dir, "Room replica")?;
    let dir_name = replica_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Room replica '{}' has no valid name", replica_dir.display()))?;
    if !dir_name.starts_with("__agent_") {
        return Err(format!(
            "Room replica '{}' must be named '__agent_<name>'",
            replica_dir.display()
        ));
    }
    let wg_dir = replica_dir.parent().ok_or_else(|| {
        format!(
            "Room replica '{}' has no parent room",
            replica_dir.display()
        )
    })?;
    let wg_name = wg_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !crate::config::entity_prefix::has_entity_prefix(wg_name) {
        return Err(format!(
            "Room replica '{}' is not inside a `room-*` or legacy `wg-*` Room directory",
            replica_dir.display()
        ));
    }

    let persisted_identity =
        read_json_object(&replica_dir.join("config.json")).and_then(|config| {
            config
                .get("identity")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        });
    let identity = crate::config::replica_identity::validate_or_repair_wg_replica_identity(
        &replica_dir,
        persisted_identity.as_deref(),
    )?;
    let origin = validate_authoritative_matrix_dir(&identity.matrix_dir)?;
    Ok((replica_dir, origin))
}

pub fn validate_profile_selection_agent_path(
    settings: &AppSettings,
    launch_path: &Path,
) -> Result<ValidatedProfileAgentPath, String> {
    let dir_name = launch_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Agent path '{}' has no valid name", launch_path.display()))?;

    if dir_name.starts_with("_agent_") {
        let launch_path = validate_authoritative_matrix_dir(launch_path)?;
        let ac_root = launch_path.parent().ok_or_else(|| {
            format!(
                "Agent Matrix '{}' has no parent Project AC Root",
                launch_path.display()
            )
        })?;
        ensure_ac_root_is_configured(settings, ac_root)?;
        return Ok(ValidatedProfileAgentPath {
            origin_matrix_dir: launch_path.clone(),
            launch_path,
        });
    }

    if dir_name.starts_with("__agent_") {
        let (launch_path, origin_matrix_dir) = validated_replica_origin(launch_path)?;
        let ac_root = origin_matrix_dir.parent().ok_or_else(|| {
            format!(
                "Agent Matrix '{}' has no parent Project AC Root",
                origin_matrix_dir.display()
            )
        })?;
        ensure_ac_root_is_configured(settings, ac_root)?;
        return Ok(ValidatedProfileAgentPath {
            launch_path,
            origin_matrix_dir,
        });
    }

    Err(format!(
        "Agent path '{}' is not an Agent Matrix or Room replica",
        launch_path.display()
    ))
}

pub(crate) fn agent_name_from_dir(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix("_agent_")
        .or_else(|| name.strip_prefix("__agent_"))
        .map(str::to_string)
}

fn origin_matrix_dir_for_launch_path(launch_path: &Path) -> Result<Option<PathBuf>, String> {
    let Some(dir_name) = launch_path.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };

    if dir_name.starts_with("_agent_") {
        return validate_authoritative_matrix_dir(launch_path).map(Some);
    }

    if !dir_name.starts_with("__agent_") {
        return Ok(None);
    }

    validated_replica_origin(launch_path).map(|(_, origin)| Some(origin))
}

fn normalize_profile_from_source(
    raw: Option<String>,
    warnings: &mut Vec<String>,
    source: &str,
) -> Option<String> {
    match raw {
        Some(value) => match normalize_profile_letter(&value) {
            Some(letter) => Some(letter),
            None => {
                warnings.push(format!(
                    "Ignoring invalid profile letter '{}' from {}",
                    value, source
                ));
                None
            }
        },
        None => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaProfileRead {
    pub profile: Option<String>,
    pub warning: Option<String>,
}

pub fn read_replica_profile_result(launch_path: &Path) -> ReplicaProfileRead {
    let v2 = read_tooling_string(launch_path, "profile");
    let legacy = read_tooling_string(launch_path, "instanceProfileOverride");
    let v2_normalized = v2.as_deref().and_then(normalize_profile_letter);
    let legacy_normalized = legacy.as_deref().and_then(normalize_profile_letter);
    let warning = match (&v2_normalized, &legacy_normalized) {
        (Some(v2), Some(legacy)) if v2 != legacy => Some(format!(
            "tooling.profile ({}) differs from legacy instanceProfileOverride ({}) at {}",
            v2,
            legacy,
            launch_path.display()
        )),
        _ => None,
    };
    ReplicaProfileRead {
        profile: v2_normalized.or(legacy_normalized),
        warning,
    }
}

pub fn read_replica_profile(launch_path: &Path) -> Option<String> {
    read_replica_profile_result(launch_path).profile
}

pub fn read_replica_current_coding_agent(launch_path: &Path) -> Option<String> {
    read_tooling_string(launch_path, "currentCodingAgent")
}

/// #592 - read the loaded profile content-hash persisted at last spawn.
pub fn read_replica_profile_content_hash(launch_path: &Path) -> Option<String> {
    read_tooling_string(launch_path, "profileContentHash")
}

/// #592 - persist the loaded profile content-hash. No-op for non-replica /
/// non-matrix / non-root-agent launch roots (a normal repo has no per-agent
/// config.json and must not be littered with one). Best-effort: callers log +
/// ignore errors.
pub fn set_replica_profile_content_hash(launch_path: &Path, hash: &str) -> Result<(), String> {
    let is_agent_dir = launch_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("__agent_") || name.starts_with("_agent_"))
        .unwrap_or(false);
    // #592 - the Root Agent (`ac-root-agent`) is a legitimate replica that runs a
    // coding agent and must support drift like any other; its dir name does not
    // match the `__agent_`/`_agent_` prefix, so accept it explicitly. Name-only
    // (vs path-validated `is_root_agent_path`) is sufficient here: the sole caller
    // `create_session_inner` already writes this dir's `config.state.no-git.json`
    // unconditionally via `set_last_coding_agent` right before this call, so the
    // gate only governs whether the hash field is added, never whether a stray
    // state file is created. The read side is ungated, so it round-trips.
    let is_root_agent = launch_path
        .to_str()
        .map(crate::config::root_agent::is_root_agent_dir_name)
        .unwrap_or(false);
    if !is_agent_dir && !is_root_agent {
        return Ok(());
    }
    log::debug!(
        "[profile-hash] persist: writing profileContentHash={} to {}",
        hash,
        launch_path.display(),
    );
    write_tooling_string(
        launch_path,
        ToolingSide::State,
        "profileContentHash",
        Some(hash),
    )
}

pub fn set_agent_default_profile(
    settings: &AppSettings,
    launch_path: &Path,
    profile: &str,
) -> Result<(), String> {
    let profile = normalize_profile_letter(profile)
        .ok_or_else(|| "Profile must be a single letter A through Z".to_string())?;
    let validated = validate_profile_selection_agent_path(settings, launch_path)?;
    write_tooling_string(
        &validated.origin_matrix_dir,
        ToolingSide::Decisions,
        "defaultProfile",
        Some(&profile),
    )
}

pub fn set_instance_profile_override(
    settings: &AppSettings,
    launch_path: &Path,
    profile: Option<&str>,
) -> Result<(), String> {
    let normalized = match profile {
        Some(profile) => Some(
            normalize_profile_letter(profile)
                .ok_or_else(|| "Profile must be a single letter A through Z".to_string())?,
        ),
        None => None,
    };
    let validated = validate_profile_selection_agent_path(settings, launch_path)?;
    write_profile_to_launch_path(&validated.launch_path, normalized.as_deref())?;
    Ok(())
}

// ── #1939 replica selection lock state ─────────────────────────────────────
//
// Stored contract: replica top-level `tooling.selectionLocked` (optional;
// absent means false) protects the saved pair — `tooling.currentCodingAgent`
// plus the requested `tooling.profile` letter. It never protects the resolved
// command/environment/model/content, which stay resolver-owned: a fallback is
// never written back over the requested letter.
//
// Matrix contract: top-level `tooling.replicaSelectionDefault` is the creation
// default {codingAgentId, requestedProfile, selectionLocked}. Absent keeps the
// old creation behavior; a present one must be complete and valid.

/// The protected replica selection pair: coding agent id plus the requested
/// profile letter. The resolved command/environment/model/content are
/// deliberately not part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaSelectionPair {
    pub coding_agent_id: String,
    pub requested_profile: String,
}

/// A prior selection state a write must still observe (CAS). Built from
/// [`read_replica_selection_state`] so a write can never overwrite a state it
/// did not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaSelectionExpectation {
    /// Canonical replica identity (`../../_agent_<name>`), as validated by the
    /// strict read.
    pub identity: String,
    /// The complete saved pair, or `None` for a legacy config without one.
    pub pair: Option<ReplicaSelectionPair>,
    pub locked: bool,
}

/// #1939 - strict, read-only view of a replica's stored selection.
///
/// `Invalid` is a diagnostic, never a permissive state: invalid/duplicate JSON,
/// a non-object `tooling`, a non-boolean `selectionLocked`, or `true` with an
/// incomplete/invalid pair can never be read as `Unlocked`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplicaSelectionState {
    Unlocked {
        identity: String,
        /// `None` when no complete pair is stored (legacy unlocked config, which
        /// remains valid and may be deliberately assigned).
        pair: Option<ReplicaSelectionPair>,
        /// Present when `tooling.profile` diverges from the legacy
        /// `instanceProfileOverride`; the modern field wins, as before.
        warning: Option<String>,
    },
    Locked {
        identity: String,
        pair: ReplicaSelectionPair,
        warning: Option<String>,
    },
    Invalid {
        diagnostic: String,
    },
}

impl ReplicaSelectionState {
    pub fn identity(&self) -> Option<&str> {
        match self {
            Self::Unlocked { identity, .. } | Self::Locked { identity, .. } => Some(identity),
            Self::Invalid { .. } => None,
        }
    }

    pub fn pair(&self) -> Option<&ReplicaSelectionPair> {
        match self {
            Self::Unlocked { pair, .. } => pair.as_ref(),
            Self::Locked { pair, .. } => Some(pair),
            Self::Invalid { .. } => None,
        }
    }

    pub fn is_locked(&self) -> bool {
        matches!(self, Self::Locked { .. })
    }

    /// The CAS value a write must pass. An `Invalid` state has none: it can
    /// never be force-repaired implicitly.
    pub fn expectation(&self) -> Result<ReplicaSelectionExpectation, String> {
        match self {
            Self::Unlocked { identity, pair, .. } => Ok(ReplicaSelectionExpectation {
                identity: identity.clone(),
                pair: pair.clone(),
                locked: false,
            }),
            Self::Locked { identity, pair, .. } => Ok(ReplicaSelectionExpectation {
                identity: identity.clone(),
                pair: Some(pair.clone()),
                locked: true,
            }),
            Self::Invalid { diagnostic } => Err(format!(
                "replica selection state is invalid: {}",
                diagnostic
            )),
        }
    }
}

/// #1939 - which policy a selection write applies. There is deliberately no
/// free `bool`: every caller names an intent, so none can bypass the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionWriteIntent {
    /// Deliberate single-replica assignment; preserves the existing flag.
    /// A locked replica is rejected unless the request equals its saved pair,
    /// which returns UNCHANGED without publishing.
    Individual,
    /// Deliberate single-replica assignment plus `selectionLocked: true`.
    /// A locked replica is rejected unless the request equals its saved pair,
    /// which returns UNCHANGED without publishing.
    IndividualAssignLock,
    /// Bulk assignment preserving flags; a locked replica is skipped.
    BulkOrdinary,
    /// Bulk assign-lock for unlocked replicas only; a locked replica is skipped.
    BulkAssignLockUnlockedOnly,
    /// Bulk assign-lock after an explicit review; locked replicas are written
    /// too (forced), and the flag ends true.
    BulkForceReviewed,
}

/// #1939 - a lock flip, tracked separately from the publication itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionLockTransition {
    UnlockedToLocked,
    LockedToUnlocked,
}

/// #1939 - outcome of a guarded selection mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionWriteOutcome {
    /// True when the guarded callback applied a semantic change.
    pub changed: bool,
    /// True when the funnel published a new config file.
    pub published: bool,
    /// The lock flip this write caused, when any.
    pub lock_transition: Option<SelectionLockTransition>,
}

impl SelectionWriteOutcome {
    /// Nothing to change and nothing of the caller's intent published
    /// (already-unlocked / skipped / same locked pair); a pending state
    /// migration may already have been.
    const UNCHANGED: Self = Self {
        changed: false,
        published: false,
        lock_transition: None,
    };
    const WRITTEN: Self = Self {
        changed: true,
        published: true,
        lock_transition: None,
    };
}

/// #1939 - call-local typed marker carrying a no-publish guard outcome back
/// through the #1938 funnel's `Err` channel. The marker decides; the error text
/// is never inspected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionGuardMarker {
    AlreadyUnlocked,
    SkippedLocked,
}

/// Private sentinel for the marker-only funnel exit; never displayed and never
/// string-matched.
const NO_PUBLISH_SENTINEL: &str = "#1939 guarded selection no-publish exit";

pub(crate) fn selection_state_from_value(config: &Value, identity: &str) -> ReplicaSelectionState {
    let tooling = match config.get("tooling") {
        None => {
            return ReplicaSelectionState::Unlocked {
                identity: identity.to_string(),
                pair: None,
                warning: None,
            }
        }
        Some(Value::Object(tooling)) => tooling,
        Some(_) => {
            return ReplicaSelectionState::Invalid {
                diagnostic: "tooling must be a JSON object".to_string(),
            }
        }
    };

    let locked = match tooling.get("selectionLocked") {
        None => false,
        Some(Value::Bool(locked)) => *locked,
        Some(_) => {
            return ReplicaSelectionState::Invalid {
                diagnostic: "tooling.selectionLocked must be a boolean".to_string(),
            }
        }
    };

    let profile = tooling
        .get("profile")
        .and_then(Value::as_str)
        .and_then(normalize_profile_letter);
    let legacy = tooling
        .get("instanceProfileOverride")
        .and_then(Value::as_str)
        .and_then(normalize_profile_letter);
    let warning = match (&profile, &legacy) {
        (Some(profile), Some(legacy)) if profile != legacy => Some(format!(
            "tooling.profile ({}) differs from legacy instanceProfileOverride ({})",
            profile, legacy
        )),
        _ => None,
    };
    let coding_agent = tooling
        .get("currentCodingAgent")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    let pair = match (coding_agent, profile.or(legacy)) {
        (Some(coding_agent_id), Some(requested_profile)) => Some(ReplicaSelectionPair {
            coding_agent_id,
            requested_profile,
        }),
        _ => None,
    };

    match (locked, pair) {
        (true, None) => ReplicaSelectionState::Invalid {
            diagnostic: "tooling.selectionLocked is true but the saved selection pair is incomplete or invalid"
                .to_string(),
        },
        (true, Some(pair)) => ReplicaSelectionState::Locked {
            identity: identity.to_string(),
            pair,
            warning,
        },
        (false, pair) => ReplicaSelectionState::Unlocked {
            identity: identity.to_string(),
            pair,
            warning,
        },
    }
}

/// #1939 - strict read-only view of `replica_dir`'s stored selection. Uses the
/// existing strict reader (bounded path-safe read, duplicate-free JSON, object
/// and identity validation); every strict-read failure becomes `Invalid`.
pub fn read_replica_selection_state(replica_dir: &Path) -> ReplicaSelectionState {
    match crate::config::replica_identity::read_wg_replica_config_read_only(replica_dir) {
        Ok((config, identity)) => selection_state_from_value(&config, &identity.identity),
        Err(error) => ReplicaSelectionState::Invalid {
            diagnostic: format!("{}: {}", replica_dir.display(), error),
        },
    }
}

/// #1939 - Matrix creation default: `tooling.replicaSelectionDefault`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaSelectionDefault {
    pub coding_agent_id: String,
    pub requested_profile: String,
    pub selection_locked: bool,
}

impl ReplicaSelectionDefault {
    fn normalized(self) -> Result<Self, String> {
        let requested_profile =
            normalize_profile_letter(&self.requested_profile).ok_or_else(|| {
                "replicaSelectionDefault.requestedProfile must be a single letter A through Z"
                    .to_string()
            })?;
        let coding_agent_id = self.coding_agent_id.trim().to_string();
        if coding_agent_id.is_empty() {
            return Err(
                "replicaSelectionDefault.codingAgentId must be a non-empty string".to_string(),
            );
        }
        Ok(Self {
            coding_agent_id,
            requested_profile,
            selection_locked: self.selection_locked,
        })
    }
}

fn parse_replica_selection_default(
    config: &Value,
) -> Result<Option<ReplicaSelectionDefault>, String> {
    let Some(tooling) = config.get("tooling") else {
        return Ok(None);
    };
    let tooling = tooling
        .as_object()
        .ok_or_else(|| "tooling must be a JSON object".to_string())?;
    let Some(default) = tooling.get("replicaSelectionDefault") else {
        return Ok(None);
    };
    let default = default
        .as_object()
        .ok_or_else(|| "tooling.replicaSelectionDefault must be a JSON object".to_string())?;
    let coding_agent_id = default
        .get("codingAgentId")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "tooling.replicaSelectionDefault.codingAgentId must be a non-empty string".to_string()
        })?
        .to_string();
    let requested_profile = default
        .get("requestedProfile")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "tooling.replicaSelectionDefault.requestedProfile must be a single letter A through Z"
                .to_string()
        })?
        .to_string();
    let selection_locked = default
        .get("selectionLocked")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            "tooling.replicaSelectionDefault.selectionLocked must be a boolean".to_string()
        })?;
    let normalized = ReplicaSelectionDefault {
        coding_agent_id,
        requested_profile,
        selection_locked,
    }
    .normalized()?;
    Ok(Some(normalized))
}

/// #1939 - read the Matrix creation default from one atomic JSON snapshot.
///
/// A missing Matrix config, missing `tooling`, or missing
/// `tooling.replicaSelectionDefault` all mean "old creation behavior"
/// (`Ok(None)`). A present default must be complete and valid; an invalid one
/// is an error, never silently ignored.
pub fn read_replica_selection_default(
    matrix_dir: &Path,
) -> Result<Option<ReplicaSelectionDefault>, String> {
    let config_path = matrix_dir.join("config.json");
    let bytes = match std::fs::read(&config_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Failed to read Matrix config {}: {}",
                config_path.display(),
                error
            ))
        }
    };
    let config: Value = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "Matrix config {} is not valid JSON: {}",
            config_path.display(),
            error
        )
    })?;
    if !config.is_object() {
        return Err(format!(
            "Matrix config {} must be a JSON object",
            config_path.display()
        ));
    }
    parse_replica_selection_default(&config)
        .map_err(|error| format!("Matrix config {}: {}", config_path.display(), error))
}

/// #1939 - validate a default and write `tooling.replicaSelectionDefault` under
/// the #1938 sidecar. `expected_prior` is the CAS value read before the call; a
/// mismatch is a stale error and publishes nothing.
pub fn write_replica_selection_default(
    matrix_dir: &Path,
    default: &ReplicaSelectionDefault,
    expected_prior: Option<&ReplicaSelectionDefault>,
) -> Result<SelectionWriteOutcome, String> {
    let normalized = default.clone().normalized()?;
    let config_path = matrix_dir.join("config.json");
    crate::config::agent_config::update_agent_config(&config_path, |obj, _state| {
        let current = read_replica_selection_default(matrix_dir)?;
        if current.as_ref() != expected_prior {
            return Err(format!(
                "stale replicaSelectionDefault at {}: expected {}, found {}",
                config_path.display(),
                describe_selection_default(expected_prior),
                describe_selection_default(current.as_ref()),
            ));
        }
        let tooling_value = obj
            .entry("tooling".to_string())
            .or_insert_with(|| serde_json::json!({}));
        let tooling = tooling_value
            .as_object_mut()
            .ok_or_else(|| format!("tooling must be a JSON object at {}", config_path.display()))?;
        tooling.insert(
            "replicaSelectionDefault".to_string(),
            serde_json::json!({
                "codingAgentId": normalized.coding_agent_id,
                "requestedProfile": normalized.requested_profile,
                "selectionLocked": normalized.selection_locked,
            }),
        );
        Ok(())
    })?;
    Ok(SelectionWriteOutcome::WRITTEN)
}

/// #1939 - materialize a validated Matrix default into a first-creation replica
/// config object. The caller owns the surrounding guarded write.
pub(crate) fn apply_replica_selection_default(
    config: &mut Value,
    default: &ReplicaSelectionDefault,
) -> Result<(), String> {
    let root = config
        .as_object_mut()
        .ok_or_else(|| "Replica config must be a JSON object".to_string())?;
    let tooling_value = root
        .entry("tooling".to_string())
        .or_insert_with(|| serde_json::json!({}));
    let tooling = tooling_value
        .as_object_mut()
        .ok_or_else(|| "Replica tooling must be a JSON object".to_string())?;
    tooling.insert(
        "currentCodingAgent".to_string(),
        Value::String(default.coding_agent_id.clone()),
    );
    tooling.insert(
        "profile".to_string(),
        Value::String(default.requested_profile.clone()),
    );
    tooling.insert(
        "instanceProfileOverride".to_string(),
        Value::String(default.requested_profile.clone()),
    );
    tooling.insert(
        "instanceProfileOverrideSource".to_string(),
        Value::String("manual".to_string()),
    );
    tooling.insert(
        "selectionLocked".to_string(),
        Value::Bool(default.selection_locked),
    );
    Ok(())
}

#[derive(Clone, Debug)]
pub(crate) struct SourceSelectionTarget {
    pub(crate) root: PathBuf,
    pub(crate) decisions: PathBuf,
    pub(crate) state: PathBuf,
    pub(crate) reservation: PathBuf,
    pub(crate) physical_revision: [crate::config::settings::IdentityReferencePhysicalState; 2],
}

/// One backend-derived locator tuple serves read, inventory and PAIR recovery.
/// No caller supplies a new path when the original operation resumes.
pub(crate) fn read_source_selection_target(
    settings: &AppSettings,
    agent_path: &Path,
) -> Result<SourceSelectionTarget, crate::config::settings::IdentityReferenceError> {
    use crate::config::settings::{read_identity_reference_physical, IdentityReferenceError};
    let validated_launch_path = validate_profile_selection_agent_path(settings, agent_path)
        .map_err(IdentityReferenceError::Preparation)?
        .launch_path;
    let root = std::fs::canonicalize(&validated_launch_path)
        .map_err(|error| IdentityReferenceError::Preparation(error.to_string()))?;
    let (decisions, state, reservation) =
        crate::config::settings::identity_reference_pair_paths(&root);
    let physical_revision = [
        read_identity_reference_physical(&decisions)?,
        read_identity_reference_physical(&state)?,
    ];
    Ok(SourceSelectionTarget {
        root,
        decisions,
        state,
        reservation,
        physical_revision,
    })
}

/// Strict finite source discovery. The ordinary best-effort helper is unchanged.
#[allow(dead_code)]
pub(crate) fn discover_source_inventory(
    settings: &AppSettings,
) -> Result<
    (
        crate::config::settings::SourceDiscoveryWitness,
        Vec<SourceSelectionTarget>,
    ),
    crate::config::settings::IdentityReferenceError,
> {
    use crate::config::settings::{IdentityReferenceError, SourceDiscoveryWitness};
    let fail = |error: std::io::Error| IdentityReferenceError::Preparation(error.to_string());
    let mut witness = SourceDiscoveryWitness {
        registry: settings.project_paths.clone(),
        parents: Vec::new(),
        slots: Vec::new(),
        membership: Vec::new(),
    };
    let mut candidates = Vec::new();
    for project in &settings.project_paths {
        let base = PathBuf::from(project);
        if !std::fs::symlink_metadata(&base).map_err(&fail)?.is_dir() {
            return Err(IdentityReferenceError::Conflict);
        }
        witness.parents.push(base.clone());
        if base
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_ac_root_name)
        {
            candidates.push(base.clone());
        }
        candidates.push(base.join(CANONICAL_AC_ROOT_DIR));
        let mut children = Vec::new();
        source_discovery_cut("readDir").map_err(&fail)?;
        for item in std::fs::read_dir(&base).map_err(&fail)? {
            source_discovery_cut("entry").map_err(&fail)?;
            let item = item.map_err(&fail)?;
            source_discovery_cut("fileType").map_err(&fail)?;
            let kind = item.file_type().map_err(&fail)?;
            if kind.is_dir() {
                witness.parents.push(item.path());
                children.push(item.path());
                candidates.push(item.path().join(CANONICAL_AC_ROOT_DIR));
            }
        }
        children.sort();
        witness.membership.push((base, children));
    }
    candidates.sort();
    candidates.dedup();
    let mut ac_roots: Vec<PathBuf> = Vec::new();
    for candidate in candidates {
        match std::fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                witness.slots.push((candidate, false))
            }
            Err(error) => return Err(fail(error)),
            Ok(metadata) => {
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(IdentityReferenceError::Conflict);
                }
                ensure_authoritative_ac_root(&candidate)
                    .map_err(IdentityReferenceError::Preparation)?;
                let canonical = canonical_existing_dir(&candidate, "Source AC Root")
                    .map_err(IdentityReferenceError::Preparation)?;
                witness.slots.push((candidate, true));
                if !ac_roots
                    .iter()
                    .any(|root| same_canonical_path(root, &canonical))
                {
                    ac_roots.push(canonical);
                }
            }
        }
    }
    ac_roots.sort();
    let mut targets = Vec::new();
    for root in ac_roots {
        witness.parents.push(root.clone());
        let mut selected = Vec::new();
        source_discovery_cut("readDir").map_err(&fail)?;
        for item in std::fs::read_dir(&root).map_err(&fail)? {
            source_discovery_cut("entry").map_err(&fail)?;
            let item = item.map_err(&fail)?;
            source_discovery_cut("fileType").map_err(&fail)?;
            let kind = item.file_type().map_err(&fail)?;
            let name = item
                .file_name()
                .into_string()
                .map_err(|_| IdentityReferenceError::Conflict)?;
            if name.starts_with("_agent_") {
                if !kind.is_dir() {
                    return Err(IdentityReferenceError::Conflict);
                }
                selected.push(item.path());
                targets.push(read_source_selection_target(settings, &item.path())?);
            } else if crate::config::entity_prefix::has_entity_prefix(&name) {
                if !kind.is_dir() {
                    return Err(IdentityReferenceError::Conflict);
                }
                selected.push(item.path());
                witness.parents.push(item.path());
                let mut replicas = Vec::new();
                source_discovery_cut("readDir").map_err(&fail)?;
                for replica in std::fs::read_dir(item.path()).map_err(&fail)? {
                    source_discovery_cut("entry").map_err(&fail)?;
                    let replica = replica.map_err(&fail)?;
                    source_discovery_cut("fileType").map_err(&fail)?;
                    let kind = replica.file_type().map_err(&fail)?;
                    let name = replica
                        .file_name()
                        .into_string()
                        .map_err(|_| IdentityReferenceError::Conflict)?;
                    if !name.starts_with("__agent_") {
                        continue;
                    }
                    if !kind.is_dir() {
                        return Err(IdentityReferenceError::Conflict);
                    }
                    replicas.push(replica.path());
                    targets.push(read_source_selection_target(settings, &replica.path())?);
                }
                replicas.sort();
                witness.membership.push((item.path(), replicas));
            }
        }
        selected.sort();
        witness.membership.push((root, selected));
    }
    witness
        .parents
        .extend(targets.iter().map(|target| target.root.clone()));
    witness.parents.sort();
    witness.parents.dedup();
    witness.slots.sort();
    witness.membership.sort();
    targets.sort_by(|left, right| left.decisions.cmp(&right.decisions));
    if targets
        .windows(2)
        .any(|pair| same_canonical_path(&pair[0].decisions, &pair[1].decisions))
    {
        return Err(IdentityReferenceError::Conflict);
    }
    Ok((witness, targets))
}

/// Source-aware original linkage is retained independently of the requested diff.
/// Unknown fields remain untouched; inspect the same semantic domains as patch.
#[allow(dead_code)]
pub(crate) fn original_source_selections(
    target: &SourceSelectionTarget,
) -> Result<
    Vec<crate::config::settings::ConfigurationRef>,
    crate::config::settings::IdentityReferenceError,
> {
    Ok(source_original_slots(target)?
        .into_iter()
        .map(|(_, reference)| reference)
        .collect())
}

// #2897 P03: read-only source companions, without enrollment or repair.
#[allow(dead_code)]
pub(crate) fn read_source_selection_state(
    settings: &AppSettings,
    agent_path: &Path,
) -> Result<
    crate::config::settings::SourceSelectionState<
        Option<ReplicaSelectionPair>,
        [crate::config::settings::IdentityReferencePhysicalState; 2],
    >,
    crate::config::settings::IdentityReferenceError,
> {
    use crate::config::settings::{IdentityReferenceError, SourceSelectionState};
    let target = read_source_selection_target(settings, agent_path)?;
    let physical_revision = target.physical_revision;
    let raw = identity_reference_value(&physical_revision[0])?;
    let tooling = raw.get("tooling");
    if tooling.is_some_and(|value| !value.is_object()) {
        return Err(IdentityReferenceError::Conflict);
    }
    let configuration_ref = tooling
        .and_then(|value| value.get("configurationRef"))
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|_| IdentityReferenceError::Conflict)?;
    let legacy_pair = match tooling.and_then(|value| value.get("currentCodingAgent")) {
        None => None,
        Some(Value::String(id)) if !id.trim().is_empty() => {
            let profile = match tooling.and_then(|value| value.get("profile")) {
                None => "A".into(),
                Some(Value::String(profile)) => {
                    normalize_profile_letter(profile).ok_or(IdentityReferenceError::Conflict)?
                }
                _ => return Err(IdentityReferenceError::Conflict),
            };
            Some(ReplicaSelectionPair {
                coding_agent_id: id.clone(),
                requested_profile: profile,
            })
        }
        _ => return Err(IdentityReferenceError::Conflict),
    };
    Ok(SourceSelectionState {
        legacy_pair,
        configuration_ref,
        physical_revision,
    })
}

fn identity_reference_value(
    physical: &crate::config::settings::IdentityReferencePhysicalState,
) -> Result<Value, crate::config::settings::IdentityReferenceError> {
    use crate::config::settings::{IdentityReferenceError, IdentityReferencePhysicalState};
    match physical {
        IdentityReferencePhysicalState::Absent => Ok(serde_json::json!({})),
        IdentityReferencePhysicalState::Bytes { bytes, .. } => {
            let value: Value =
                serde_json::from_slice(bytes).map_err(|_| IdentityReferenceError::Conflict)?;
            if !value.is_object()
                || &IdentityReferencePhysicalState::from_bytes(bytes.clone()) != physical
            {
                return Err(IdentityReferenceError::Conflict);
            }
            Ok(value)
        }
    }
}

/// Pure caller images; the durable PAIR packet remains the execution authority.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct IdentityReferencePatch {
    pub(crate) before: [crate::config::settings::IdentityReferencePhysicalState; 2],
    pub(crate) after: [Value; 2],
}

fn patch_identity_companion(
    value: &mut Value,
    source: &crate::config::settings::SourceRef,
    mappings: &[crate::config::settings::SourceIdentityMapping],
) -> Result<(), crate::config::settings::IdentityReferenceError> {
    use crate::config::settings::{ConfigurationRef, IdentityReferenceError};
    let Some(raw) = value.get_mut("configurationRef") else {
        return Ok(());
    };
    let mut reference: ConfigurationRef =
        serde_json::from_value(raw.clone()).map_err(|_| IdentityReferenceError::Conflict)?;
    if reference.version != 1 {
        return Err(IdentityReferenceError::Conflict);
    }
    if &reference.source != source {
        return Ok(());
    }
    // The existing local hint narrows an explicit source reference. It never
    // manufactures source authority for a legacy selection or neighboring row.
    let matches: Vec<_> = mappings
        .iter()
        .filter(|mapping| {
            mapping.old_calculated_id.as_ref() == Some(&reference.calculated_id)
                && reference
                    .legacy_entry_hint
                    .as_ref()
                    .is_none_or(|hint| hint == &mapping.entry_hint)
        })
        .collect();
    if matches.len() > 1 {
        return Err(IdentityReferenceError::Conflict);
    }
    let Some(mapping) = matches.first() else {
        return Ok(());
    };
    // Keep removed/disabled references, requested letters and locks unchanged.
    if mapping.unavailable || mapping.new_calculated_id.is_none() {
        return Ok(());
    }
    if let Some(letter) = reference.requested_profile.as_ref() {
        if let Some((before, after)) = mapping.profiles.get(letter) {
            if reference.profile_calculated_id.as_ref() != before.as_ref() {
                return Err(IdentityReferenceError::Conflict);
            }
            let Some(after) = after else {
                return Ok(());
            };
            reference.profile_calculated_id = Some(after.clone());
        }
    }
    reference.calculated_id = mapping
        .new_calculated_id
        .clone()
        .ok_or(IdentityReferenceError::Conflict)?;
    let encoded = serde_json::to_value(reference).map_err(|_| IdentityReferenceError::Conflict)?;
    let object = raw
        .as_object_mut()
        .ok_or(IdentityReferenceError::Conflict)?;
    for key in ["calculatedId", "profileCalculatedId"] {
        match encoded.get(key) {
            Some(value) => {
                object.insert(key.into(), value.clone());
            }
            None => {
                object.remove(key);
            }
        }
    }
    Ok(())
}

/// Prepares exact raw companions in memory, without enrollment or disk writes.
#[allow(dead_code)]
pub(crate) fn plan_identity_reference_patch(
    before: [crate::config::settings::IdentityReferencePhysicalState; 2],
    source: &crate::config::settings::SourceRef,
    mappings: &[crate::config::settings::SourceIdentityMapping],
) -> Result<IdentityReferencePatch, crate::config::settings::IdentityReferenceError> {
    use crate::config::settings::IdentityReferenceError;
    let mut after = [
        identity_reference_value(&before[0])?,
        identity_reference_value(&before[1])?,
    ];
    for (side_index, side) in after.iter_mut().enumerate() {
        let Some(tooling) = side.get_mut("tooling") else {
            continue;
        };
        if !tooling.is_object() {
            return Err(IdentityReferenceError::Conflict);
        }
        if side_index == 0 {
            patch_identity_companion(tooling, source, mappings)?;
            if let Some(default) = tooling.get_mut("replicaSelectionDefault") {
                if !default.is_object() {
                    return Err(IdentityReferenceError::Conflict);
                }
                patch_identity_companion(default, source, mappings)?;
            }
        } else {
            if let Some(history) = tooling.get_mut("codingAgents") {
                let history = history
                    .as_object_mut()
                    .ok_or(IdentityReferenceError::Conflict)?;
                for entry in history.values_mut() {
                    if !entry.is_object() {
                        return Err(IdentityReferenceError::Conflict);
                    }
                    patch_identity_companion(entry, source, mappings)?;
                }
            }
            if let Some(history) = tooling.get_mut("sourceReferences") {
                let history = history
                    .as_array_mut()
                    .ok_or(IdentityReferenceError::Conflict)?;
                for entry in history {
                    if !entry.is_object() {
                        return Err(IdentityReferenceError::Conflict);
                    }
                    patch_identity_companion(entry, source, mappings)?;
                }
            }
        }
    }
    Ok(IdentityReferencePatch { before, after })
}

/// The caller has durably recorded the authorized inventory before this call.
/// PAIR owns cleanup, timestamp protection and the original recovery images.
#[allow(dead_code)]
pub(crate) fn prepare_identity_reference_patch(
    decisions: &Path,
    request: &crate::config::settings::IdentityReferenceRequest,
    source: &crate::config::settings::SourceRef,
    mappings: &[crate::config::settings::SourceIdentityMapping],
    validate: &dyn Fn() -> Result<(), crate::config::settings::IdentityReferenceError>,
) -> Result<
    crate::config::settings::IdentityReferencePacket,
    crate::config::settings::IdentityReferenceError,
> {
    use crate::config::settings::IdentityReferencePhysicalState;
    crate::config::agent_config::prepare_and_reserve_identity_reference_pair_guarded(
        decisions,
        request,
        |decisions, state| {
            // Cleanup has already run under the same PAIR locks. Patch only
            // companions in these maps; never overwrite the loader's cleanup
            // with snapshots taken before lock acquisition.
            let physical = [decisions.clone(), state.clone()].map(|map| {
                serde_json::to_vec(&map).map(IdentityReferencePhysicalState::from_bytes)
            });
            let [decisions_before, state_before] = physical;
            let patch = plan_identity_reference_patch(
                [
                    decisions_before.map_err(|e| e.to_string())?,
                    state_before.map_err(|e| e.to_string())?,
                ],
                source,
                mappings,
            )
            .map_err(|e| e.to_string())?;
            let [decisions_after, state_after] = patch.after;
            *decisions = decisions_after
                .as_object()
                .ok_or("invalid decisions object")?
                .clone();
            *state = state_after
                .as_object()
                .ok_or("invalid state object")?
                .clone();
            Ok(())
        },
        validate,
    )
}

/// Resume executes the original packet; it never prepares new caller images.
/// The pure validation callback borrows existing C/source/target authority.
#[allow(dead_code, clippy::too_many_arguments)]
pub(crate) fn apply_identity_reference_patch(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    plan_digest: &str,
    validate: &dyn Fn() -> Result<(), crate::config::settings::IdentityReferenceError>,
) -> Result<
    [crate::config::settings::IdentityReferencePhysicalState; 2],
    crate::config::settings::IdentityReferenceError,
> {
    use crate::config::settings::{
        execute_identity_reference_pair_guarded, verify_identity_reference_pair_guarded,
    };
    execute_identity_reference_pair_guarded(decisions, state, reservation, operation_id, validate)?;
    verify_identity_reference_pair_guarded(
        decisions,
        state,
        reservation,
        operation_id,
        plan_digest,
        validate,
    )
}

fn describe_selection_default(default: Option<&ReplicaSelectionDefault>) -> String {
    match default {
        Some(default) => format!(
            "agent '{}' profile {} locked={}",
            default.coding_agent_id, default.requested_profile, default.selection_locked
        ),
        None => "absent".to_string(),
    }
}

fn describe_selection_pair(pair: Option<&ReplicaSelectionPair>) -> String {
    match pair {
        Some(pair) => format!(
            "agent '{}' profile {}",
            pair.coding_agent_id, pair.requested_profile
        ),
        None => "absent".to_string(),
    }
}

fn validated_replica_dir(settings: &AppSettings, replica_path: &Path) -> Result<PathBuf, String> {
    let validated = validate_profile_selection_agent_path(settings, replica_path)?;
    let name = validated
        .launch_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !name.starts_with("__agent_") {
        return Err(format!(
            "Profile assignment target '{}' must be a Room replica",
            validated.launch_path.display()
        ));
    }
    Ok(validated.launch_path)
}

/// #1939 - strict reread plus expected-state comparison, executed INSIDE the
/// #1938 guarded callback (read-only; it never takes the mutation guard
/// recursively). Every mismatch is a stale error before any mutation.
fn validate_selection_write_state(
    replica_dir: &Path,
    expected: &ReplicaSelectionExpectation,
) -> Result<(Option<ReplicaSelectionPair>, bool), String> {
    let (config, identity) = crate::config::replica_identity::read_wg_replica_config_read_only(
        replica_dir,
    )
    .map_err(|error| {
        format!(
            "stale replica selection at {}: {}",
            replica_dir.display(),
            error
        )
    })?;
    let (current_pair, current_locked) =
        match selection_state_from_value(&config, &identity.identity) {
            ReplicaSelectionState::Unlocked { pair, .. } => (pair, false),
            ReplicaSelectionState::Locked { pair, .. } => (Some(pair), true),
            ReplicaSelectionState::Invalid { diagnostic } => {
                return Err(format!(
                    "invalid replica selection at {}: {}",
                    replica_dir.display(),
                    diagnostic
                ))
            }
        };
    if identity.identity != expected.identity {
        return Err(format!(
            "stale replica selection at {}: expected identity '{}', found '{}'",
            replica_dir.display(),
            expected.identity,
            identity.identity
        ));
    }
    if current_pair != expected.pair {
        return Err(format!(
            "stale replica selection at {}: expected pair {}, found {}",
            replica_dir.display(),
            describe_selection_pair(expected.pair.as_ref()),
            describe_selection_pair(current_pair.as_ref())
        ));
    }
    if current_locked != expected.locked {
        return Err(format!(
            "stale replica selection at {}: expected selectionLocked={}, found selectionLocked={}",
            replica_dir.display(),
            expected.locked,
            current_locked
        ));
    }
    Ok((current_pair, current_locked))
}

/// #1939 - guarded selection write. The pair is written as one publish
/// (`currentCodingAgent`, `profile`, legacy `instanceProfileOverride`, source
/// `manual`); assign-lock intents add `selectionLocked: true` (a false->true
/// flip is reported in [`SelectionWriteOutcome::lock_transition`]) while
/// ordinary intents preserve the stored flag exactly. Unknown keys,
/// `lastCodingAgent` and `profileContentHash` are preserved.
pub fn write_replica_selection(
    settings: &AppSettings,
    replica_path: &Path,
    pair: &ReplicaSelectionPair,
    intent: SelectionWriteIntent,
    expected: &ReplicaSelectionExpectation,
) -> Result<SelectionWriteOutcome, String> {
    let profile = normalize_profile_letter(&pair.requested_profile)
        .ok_or_else(|| "Profile must be a single letter A through Z".to_string())?;
    if pair.coding_agent_id.trim().is_empty() {
        return Err("Coding agent id must not be empty".to_string());
    }
    let replica_dir = validated_replica_dir(settings, replica_path)?;
    let config_path = replica_dir.join("config.json");
    let requested_pair = ReplicaSelectionPair {
        coding_agent_id: pair.coding_agent_id.clone(),
        requested_profile: profile.clone(),
    };

    let marker = std::cell::Cell::new(None::<SelectionGuardMarker>);
    let transition = std::cell::Cell::new(None::<SelectionLockTransition>);

    let written =
        crate::config::agent_config::update_existing_agent_config(&config_path, |obj, _state| {
            let (current_pair, current_locked) =
                validate_selection_write_state(&replica_dir, expected)?;
            if current_locked {
                match intent {
                    SelectionWriteIntent::Individual
                    | SelectionWriteIntent::IndividualAssignLock => {
                        // #2010 - KEEP: the same stored pair is a no-op, not an
                        // error; the lock stays and nothing is published.
                        if current_pair.as_ref() == Some(&requested_pair) {
                            marker.set(Some(SelectionGuardMarker::SkippedLocked));
                            return Err(NO_PUBLISH_SENTINEL.to_string());
                        }
                        return Err(format!(
                            "replica selection at {} is locked; unlock it or use a reviewed force",
                            replica_dir.display()
                        ));
                    }
                    SelectionWriteIntent::BulkOrdinary
                    | SelectionWriteIntent::BulkAssignLockUnlockedOnly => {
                        // Skip locked replicas without publishing: the same
                        // private marker path as the already-unlocked clear.
                        marker.set(Some(SelectionGuardMarker::SkippedLocked));
                        return Err(NO_PUBLISH_SENTINEL.to_string());
                    }
                    SelectionWriteIntent::BulkForceReviewed => {}
                }
            }
            let target_locked = match intent {
                SelectionWriteIntent::Individual | SelectionWriteIntent::BulkOrdinary => {
                    current_locked
                }
                SelectionWriteIntent::IndividualAssignLock
                | SelectionWriteIntent::BulkAssignLockUnlockedOnly
                | SelectionWriteIntent::BulkForceReviewed => true,
            };
            if !current_locked && target_locked {
                transition.set(Some(SelectionLockTransition::UnlockedToLocked));
            }
            let tooling_value = obj
                .entry("tooling".to_string())
                .or_insert_with(|| serde_json::json!({}));
            let tooling = tooling_value.as_object_mut().ok_or_else(|| {
                format!(
                    "invalid replica selection at {}: tooling must be a JSON object",
                    replica_dir.display()
                )
            })?;
            tooling.insert(
                "currentCodingAgent".to_string(),
                Value::String(pair.coding_agent_id.clone()),
            );
            tooling.insert("profile".to_string(), Value::String(profile.clone()));
            tooling.insert(
                "instanceProfileOverride".to_string(),
                Value::String(profile.clone()),
            );
            tooling.insert(
                "instanceProfileOverrideSource".to_string(),
                Value::String("manual".to_string()),
            );
            if target_locked {
                tooling.insert("selectionLocked".to_string(), Value::Bool(true));
            }
            Ok(())
        });

    match written {
        Ok(_) => Ok(SelectionWriteOutcome {
            changed: true,
            published: true,
            lock_transition: transition.get(),
        }),
        Err(error) => match marker.get() {
            Some(SelectionGuardMarker::SkippedLocked) => Ok(SelectionWriteOutcome::UNCHANGED),
            _ => Err(error),
        },
    }
}

/// #1939 - guarded unlock. Only the flag is written (`false`); the pair and
/// every other field stay untouched. An already-false/absent flag exits through
/// the private callback marker before any object mutation, so nothing of the
/// caller's intent is serialized or published, though a pending state migration
/// may already have been (see [`SelectionWriteOutcome`]). Every
/// other error — malformed state, stale expected state, lock timeout, IO —
/// propagates unchanged and can never be reported as success.
pub fn clear_replica_selection_lock(
    settings: &AppSettings,
    replica_path: &Path,
    expected: &ReplicaSelectionExpectation,
) -> Result<SelectionWriteOutcome, String> {
    clear_replica_selection_lock_with_publish(settings, replica_path, expected, None)
}

fn clear_replica_selection_lock_with_publish(
    settings: &AppSettings,
    replica_path: &Path,
    expected: &ReplicaSelectionExpectation,
    publish: Option<crate::config::agent_config::ConfigPublisher<'_>>,
) -> Result<SelectionWriteOutcome, String> {
    let replica_dir = validated_replica_dir(settings, replica_path)?;
    let config_path = replica_dir.join("config.json");
    let marker = std::cell::Cell::new(None::<SelectionGuardMarker>);

    let written = crate::config::agent_config::update_existing_agent_config_with_publish(
        &config_path,
        |obj, _state| {
            let (_current_pair, current_locked) =
                validate_selection_write_state(&replica_dir, expected)?;
            if !current_locked {
                marker.set(Some(SelectionGuardMarker::AlreadyUnlocked));
                return Err(NO_PUBLISH_SENTINEL.to_string());
            }
            let tooling = obj
                .get_mut("tooling")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| {
                    format!(
                        "invalid replica selection at {}: tooling must be a JSON object",
                        replica_dir.display()
                    )
                })?;
            tooling.insert("selectionLocked".to_string(), Value::Bool(false));
            Ok(())
        },
        publish,
    );

    match written {
        Ok(_) => Ok(SelectionWriteOutcome {
            changed: true,
            published: true,
            lock_transition: Some(SelectionLockTransition::LockedToUnlocked),
        }),
        Err(error) => match marker.get() {
            Some(SelectionGuardMarker::AlreadyUnlocked) => Ok(SelectionWriteOutcome::UNCHANGED),
            _ => Err(error),
        },
    }
}

/// #1939 - deliberate per-replica assignment. Preserves the stored flag through
/// [`SelectionWriteIntent::Individual`] and cannot bypass a lock: a locked
/// replica with a different pair is rejected with the stored bytes untouched,
/// while the same stored pair is a no-op that returns without writing.
pub fn set_replica_coding_agent_selection(
    settings: &AppSettings,
    replica_path: &Path,
    coding_agent_id: &str,
    profile: &str,
) -> Result<(), String> {
    if !settings
        .agents
        .iter()
        .any(|agent| agent.id == coding_agent_id)
    {
        return Err(format!("Agent '{}' is not configured", coding_agent_id));
    }
    let profile = normalize_profile_letter(profile)
        .ok_or_else(|| "Profile must be a single letter A through Z".to_string())?;
    let replica_dir = validated_replica_dir(settings, replica_path)?;
    let state = read_replica_selection_state(&replica_dir);
    let expected = state.expectation()?;
    let pair = ReplicaSelectionPair {
        coding_agent_id: coding_agent_id.to_string(),
        requested_profile: profile,
    };
    write_replica_selection(
        settings,
        &replica_dir,
        &pair,
        SelectionWriteIntent::Individual,
        &expected,
    )?;
    Ok(())
}

fn write_profile_to_launch_path(launch_path: &Path, profile: Option<&str>) -> Result<(), String> {
    let config_path = launch_path.join("config.json");
    crate::config::agent_config::update_agent_config(&config_path, |obj, _state| {
        // #1939 - shape-based, like agent_config: absent tooling becomes an
        // object, a present non-object is an error, never silently reset.
        let tooling_value = obj
            .entry("tooling".to_string())
            .or_insert_with(|| serde_json::json!({}));
        let tooling = tooling_value
            .as_object_mut()
            .ok_or_else(|| format!("tooling must be a JSON object at {}", config_path.display()))?;
        match profile {
            Some(profile) => {
                // A new valid requested letter is permitted; the Coding Agent
                // and the lock flag are preserved exactly as stored.
                tooling.insert("profile".to_string(), Value::String(profile.to_string()));
                tooling.insert(
                    "instanceProfileOverride".to_string(),
                    Value::String(profile.to_string()),
                );
                tooling.insert(
                    "instanceProfileOverrideSource".to_string(),
                    Value::String("manual".to_string()),
                );
            }
            None => {
                if protected_selection_pair_present(tooling) {
                    return Err(format!(
                        "Cannot clear the profile at {} while a protected replica selection pair exists",
                        launch_path.display()
                    ));
                }
                tooling.remove("profile");
                tooling.remove("instanceProfileOverride");
                tooling.remove("instanceProfileOverrideSource");
            }
        }
        Ok(())
    })?;
    Ok(())
}

/// #1939 - a complete saved pair (`currentCodingAgent` plus a valid profile
/// letter) is the protected unit; a null clear must not erase half of it.
fn protected_selection_pair_present(tooling: &serde_json::Map<String, Value>) -> bool {
    let coding_agent = tooling
        .get("currentCodingAgent")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty());
    let profile = tooling
        .get("profile")
        .and_then(Value::as_str)
        .and_then(normalize_profile_letter)
        .or_else(|| {
            tooling
                .get("instanceProfileOverride")
                .and_then(Value::as_str)
                .and_then(normalize_profile_letter)
        });
    coding_agent && profile.is_some()
}

pub fn resolve_profile_selection(
    settings: &AppSettings,
    launch_path: Option<&Path>,
    coding_agent_id: &str,
    requested_profile: Option<&str>,
) -> Result<ProfileSelectionResolution, String> {
    if !settings
        .agents
        .iter()
        .any(|agent| agent.id == coding_agent_id)
    {
        return Err(format!("Agent '{}' is not configured", coding_agent_id));
    }

    let validated = launch_path
        .map(|path| validate_profile_selection_agent_path(settings, path))
        .transpose()?;
    let launch_path = validated
        .as_ref()
        .map(|validated| validated.launch_path.as_path());
    let origin_matrix_dir = validated
        .as_ref()
        .map(|validated| validated.origin_matrix_dir.as_path());
    let agent_name = launch_path.and_then(agent_name_from_dir);
    let instance_read = launch_path.map(read_replica_profile_result);
    let instance_profile_override = instance_read.as_ref().and_then(|read| read.profile.clone());
    let origin_default_profile = origin_matrix_dir
        .and_then(|path| read_tooling_string(path, "defaultProfile"))
        .and_then(|value| normalize_profile_letter(&value));
    let agent_default_profile = agent_name
        .as_ref()
        .and_then(|name| {
            settings
                .coding_agent_profiles
                .default_profile_by_agent
                .get(name)
        })
        .and_then(|value| normalize_profile_letter(value));
    let requested_profile_input = requested_profile.map(str::to_string);
    let mut resolution = resolve_profile(
        settings,
        ProfileResolutionRequest {
            coding_agent_id,
            launch_path,
            agent_matrix_name: None,
            requested_profile,
            requested_profile_authoritative: false,
        },
    );
    if let Some(warning) = instance_read.and_then(|read| read.warning) {
        resolution.warnings.push(warning);
    }

    Ok(ProfileSelectionResolution {
        requested_profile_input,
        instance_profile_override,
        origin_default_profile,
        agent_default_profile,
        resolution,
    })
}

fn cell_for_letter(
    settings: &AppSettings,
    coding_agent_id: &str,
    letter: &str,
) -> Option<ProfileCellConfig> {
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .get(coding_agent_id)
        .and_then(|cells| cells.get(letter))
        .filter(|cell| cell.enabled)
        .cloned()
}

fn fallback_letters_from(requested: &str) -> Vec<String> {
    let mut letters = Vec::new();
    let start = requested.as_bytes()[0];
    for byte in (b'A'..=start).rev() {
        letters.push((byte as char).to_string());
    }
    letters
}

pub fn resolve_profile(
    settings: &AppSettings,
    request: ProfileResolutionRequest<'_>,
) -> ProfileResolution {
    let mut warnings = Vec::new();

    let launch_path = request.launch_path;
    let agent_name = request
        .agent_matrix_name
        .map(str::to_string)
        .or_else(|| launch_path.and_then(agent_name_from_dir));

    let instance_override = launch_path.and_then(|path| {
        let read = read_replica_profile_result(path);
        if let Some(warning) = read.warning {
            warnings.push(warning);
        }
        normalize_profile_from_source(read.profile, &mut warnings, "instance override")
    });

    let origin_default =
        launch_path.and_then(|path| match origin_matrix_dir_for_launch_path(path) {
            Ok(Some(origin)) => normalize_profile_from_source(
                read_tooling_string(&origin, "defaultProfile"),
                &mut warnings,
                "origin default",
            ),
            Ok(None) => None,
            Err(e) => {
                warnings.push(format!("Ignoring origin default profile: {}", e));
                None
            }
        });

    let explicit = request.requested_profile.and_then(|letter| {
        normalize_profile_from_source(Some(letter.to_string()), &mut warnings, "launch request")
    });

    let agent_default = agent_name
        .as_ref()
        .and_then(|name| {
            settings
                .coding_agent_profiles
                .default_profile_by_agent
                .get(name)
        })
        .and_then(|letter| {
            normalize_profile_from_source(Some(letter.clone()), &mut warnings, "agent default")
        });

    // #1635-2 D2: a wake dispatch request outranks the replica pin for that
    // spawn only; every other caller keeps the historical ranking (instance
    // override wins over the explicit request).
    let requested_profile = if request.requested_profile_authoritative {
        explicit
            .or(instance_override)
            .or(origin_default)
            .or(agent_default)
            .unwrap_or_else(|| "A".to_string())
    } else {
        instance_override
            .or(explicit)
            .or(origin_default)
            .or(agent_default)
            .unwrap_or_else(|| "A".to_string())
    };

    let mut fallback_chain = Vec::new();
    let mut effective_profile = "A".to_string();
    let mut effective_cell = empty_profile_cell();

    for letter in fallback_letters_from(&requested_profile) {
        fallback_chain.push(letter.clone());
        let cell = if letter == "A" {
            cell_for_letter(settings, request.coding_agent_id, &letter)
                .unwrap_or_else(empty_profile_cell)
        } else if let Some(cell) = cell_for_letter(settings, request.coding_agent_id, &letter) {
            cell
        } else {
            continue;
        };
        effective_profile = letter;
        effective_cell = cell;
        break;
    }

    let fallback_applied = effective_profile != requested_profile;
    ProfileResolution {
        requested_profile,
        effective_profile,
        fallback_chain,
        fallback_applied,
        cell: effective_cell,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::{AgentConfig, ProfileCellConfig, ProfileSlotConfig};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    fn settings_with_cells(cells: &[(&str, Vec<&str>)]) -> AppSettings {
        let mut settings = AppSettings::default();
        settings.coding_agent_profiles.profile_slots = BTreeMap::from([
            (
                "A".to_string(),
                ProfileSlotConfig {
                    label: String::new(),
                },
            ),
            (
                "B".to_string(),
                ProfileSlotConfig {
                    label: String::new(),
                },
            ),
            (
                "C".to_string(),
                ProfileSlotConfig {
                    label: String::new(),
                },
            ),
            (
                "D".to_string(),
                ProfileSlotConfig {
                    label: String::new(),
                },
            ),
        ]);
        settings.coding_agent_profiles.profiles_by_agent = cells
            .iter()
            .map(|(agent_id, letters)| {
                (
                    (*agent_id).to_string(),
                    letters
                        .iter()
                        .map(|letter| {
                            (
                                (*letter).to_string(),
                                ProfileCellConfig {
                                    preflight: None,
                                    preflight_timeout_seconds: None,
                                    enabled: true,
                                    command: format!("codex --{}", letter.to_ascii_lowercase()),
                                    env: BTreeMap::new(),
                                    notes: String::new(),
                                },
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        settings
    }

    #[test]
    #[cfg(windows)]
    fn strip_extended_prefix_converts_verbatim_unc() {
        assert_eq!(
            strip_extended_prefix(PathBuf::from(r"\\?\UNC\server\share\repo")),
            PathBuf::from(r"\\server\share\repo")
        );
    }

    #[test]
    fn transition_inventory_locator_real_matrix_and_replica_readonly() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let ac_root = project.join(".ac");
        let matrix = ac_root.join("_agent_dev-rust");
        let replica = ac_root.join("wg-7-dev-team").join("__agent_dev-rust");
        std::fs::create_dir_all(&matrix).unwrap();
        std::fs::create_dir_all(&replica).unwrap();
        let bytes = br#"{"identity":"../../_agent_dev-rust"}"#;
        std::fs::write(replica.join("config.json"), bytes).unwrap();
        let settings = settings_with_project(&project);
        for path in [&matrix, &replica] {
            let target = read_source_selection_target(&settings, path).unwrap();
            assert_eq!(target.root, std::fs::canonicalize(path).unwrap());
            assert_eq!(target.decisions, target.root.join("config.json"));
            assert_eq!(
                target.state,
                target
                    .root
                    .join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
            );
            assert_eq!(
                target.reservation,
                target
                    .root
                    .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
            );
            assert_eq!(
                target.physical_revision[1],
                crate::config::settings::IdentityReferencePhysicalState::Absent
            );
            assert!(!target.state.exists());
            assert!(!target.reservation.exists());
        }
        assert!(!matrix.join("config.json").exists());
        assert_eq!(std::fs::read(replica.join("config.json")).unwrap(), bytes);
    }

    #[test]
    fn transition_inventory_locator_rejects_unregistered_matrix_without_writes() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let foreign = temp.path().join("foreign/.ac/_agent_dev-rust");
        std::fs::create_dir_all(project.join(".ac")).unwrap();
        std::fs::create_dir_all(&foreign).unwrap();
        let settings = settings_with_project(&project);
        assert!(read_source_selection_target(&settings, &foreign).is_err());
        assert_eq!(std::fs::read_dir(&foreign).unwrap().count(), 0);
    }

    #[test]
    fn transition_strict_discovery_proves_empty_and_detects_new_candidate_without_writes() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let settings = settings_with_project(&project);
        let (before, targets) = discover_source_inventory(&settings).unwrap();
        assert!(targets.is_empty());
        assert!(before.slots.contains(&(project.join(".ac"), false)));
        assert_eq!(std::fs::read_dir(&project).unwrap().count(), 0);
        std::fs::create_dir(project.join(".ac")).unwrap();
        let (after, targets) = discover_source_inventory(&settings).unwrap();
        assert!(targets.is_empty());
        assert_ne!(before, after);
        assert_eq!(std::fs::read_dir(project.join(".ac")).unwrap().count(), 0);
    }

    #[test]
    fn transition_strict_discovery_refuses_invalid_candidate_and_missing_registry_base() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let settings = settings_with_project(&project);
        let candidate = project.join(".ac");
        std::fs::write(&candidate, b"external-not-directory").unwrap();
        assert!(discover_source_inventory(&settings).is_err());
        assert_eq!(
            std::fs::read(&candidate).unwrap(),
            b"external-not-directory"
        );
        assert!(
            discover_source_inventory(&settings_with_project(&root.path().join("missing")))
                .is_err()
        );
        assert!(!root.path().join("missing").exists());
    }

    #[test]
    fn source_discovery_actual_iterator_type_and_directory_failures_never_prove_empty() {
        let root = crate::config::settings::source_transition_test_root();
        let project = root.path().join("P");
        let matrix = project.join(".ac/_agent_N");
        let nested = project.join("child/.ac/_agent_nested");
        let replica = project.join(".ac/room-7-dev-team/__agent_N");
        for path in [&matrix, &nested, &replica] {
            std::fs::create_dir_all(path).unwrap();
        }
        let settings = AppSettings {
            project_paths: vec![project.to_string_lossy().into_owned()],
            ..AppSettings::default()
        };
        let target = matrix.join("config.json");
        let bytes = br#"{"unknown":null}"#;
        std::fs::write(&target, bytes).unwrap();
        let replica_bytes = br#"{"identity":"../../_agent_N"}"#;
        std::fs::write(replica.join("config.json"), replica_bytes).unwrap();
        SOURCE_DISCOVERY_FAILURE.with(|failure| failure.set(None));
        SOURCE_DISCOVERY_CALLS.with(|calls| calls.borrow_mut().clear());
        let baseline = discover_source_inventory(&settings).unwrap();
        assert_eq!(baseline.1.len(), 3);
        let counts = SOURCE_DISCOVERY_CALLS.with(|calls| calls.borrow().clone());
        for stage in ["readDir", "entry", "fileType"] {
            assert!(
                counts[stage] > 1,
                "fixture must reach multiple discovery loops"
            );
            for ordinal in 1..=counts[stage] {
                SOURCE_DISCOVERY_FAILURE.with(|failure| failure.set(Some((stage, ordinal))));
                assert!(
                    discover_source_inventory(&settings).is_err(),
                    "{stage}/{ordinal} silently became empty"
                );
                SOURCE_DISCOVERY_FAILURE
                    .with(|failure| assert!(failure.get().is_none(), "cut not reached"));
                assert_eq!(std::fs::read(&target).unwrap(), bytes);
                assert_eq!(
                    std::fs::read(replica.join("config.json")).unwrap(),
                    replica_bytes
                );
                let recovered = discover_source_inventory(&settings).unwrap();
                assert_eq!(recovered.0, baseline.0);
                assert_eq!(recovered.1.len(), baseline.1.len());
            }
        }
    }

    fn settings_with_project(project: &Path) -> AppSettings {
        let mut settings = settings_with_cells(&[("codex", vec!["A", "B", "C"])]);
        settings.project_paths = vec![project.to_string_lossy().to_string()];
        settings.agents = vec![AgentConfig {
            preflight: None,
            preflight_timeout_seconds: None,
            id: "codex".to_string(),
            id_hash: None,
            label: "Codex".to_string(),
            command: "codex".to_string(),
            color: "#000000".to_string(),
            order: None,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        }];
        settings
    }

    #[test]
    fn falls_back_to_nearest_lower_available_profile() {
        let settings = settings_with_cells(&[("codex", vec!["A", "C"])]);
        let resolved = resolve_profile(
            &settings,
            ProfileResolutionRequest {
                coding_agent_id: "codex",
                launch_path: None,
                agent_matrix_name: Some("dev-rust"),
                requested_profile: Some("D"),
                requested_profile_authoritative: false,
            },
        );

        assert_eq!(resolved.requested_profile, "D");
        assert_eq!(resolved.effective_profile, "C");
        assert!(resolved.fallback_applied);
        assert_eq!(resolved.fallback_chain, vec!["D", "C"]);
        assert_eq!(resolved.cell.command, "codex --c");
    }

    #[test]
    fn synthesizes_a_cell_when_missing() {
        let settings = settings_with_cells(&[("codex", vec![])]);
        let resolved = resolve_profile(
            &settings,
            ProfileResolutionRequest {
                coding_agent_id: "codex",
                launch_path: None,
                agent_matrix_name: None,
                requested_profile: Some("B"),
                requested_profile_authoritative: false,
            },
        );

        assert_eq!(resolved.effective_profile, "A");
        assert!(resolved.cell.command.is_empty());
    }

    #[test]
    fn instance_override_wins_over_explicit_request() {
        let temp = tempfile::tempdir().unwrap();
        let agent_dir = temp.path().join(".ac").join("_agent_dev-rust");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("config.json"),
            r#"{"tooling":{"instanceProfileOverride":"C"}}"#,
        )
        .unwrap();

        let settings = settings_with_cells(&[("codex", vec!["A", "B", "C"])]);
        let resolved = resolve_profile(
            &settings,
            ProfileResolutionRequest {
                coding_agent_id: "codex",
                launch_path: Some(&agent_dir),
                agent_matrix_name: None,
                requested_profile: Some("B"),
                requested_profile_authoritative: false,
            },
        );

        assert_eq!(resolved.requested_profile, "C");
        assert_eq!(resolved.effective_profile, "C");
    }

    #[test]
    fn dispatch_authoritative_request_outranks_instance_override() {
        let temp = tempfile::tempdir().unwrap();
        let agent_dir = temp.path().join(".ac").join("_agent_dev-rust");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("config.json"),
            r#"{"tooling":{"instanceProfileOverride":"C"}}"#,
        )
        .unwrap();

        let settings = settings_with_cells(&[("codex", vec!["A", "B", "C"])]);
        // Historical ranking (picker, self-switch, restart, drift): the replica
        // pin wins over the explicit request.
        let pinned = resolve_profile(
            &settings,
            ProfileResolutionRequest {
                coding_agent_id: "codex",
                launch_path: Some(&agent_dir),
                agent_matrix_name: None,
                requested_profile: Some("B"),
                requested_profile_authoritative: false,
            },
        );
        assert_eq!(pinned.requested_profile, "C");
        assert_eq!(pinned.effective_profile, "C");

        // Wake dispatch: the explicit request outranks the pin for this spawn.
        let dispatched = resolve_profile(
            &settings,
            ProfileResolutionRequest {
                coding_agent_id: "codex",
                launch_path: Some(&agent_dir),
                agent_matrix_name: None,
                requested_profile: Some("B"),
                requested_profile_authoritative: true,
            },
        );
        assert_eq!(dispatched.requested_profile, "B");
        assert_eq!(dispatched.effective_profile, "B");
    }

    #[test]
    fn selection_write_dual_writes_profile_and_preserves_last_coding_agent() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let ac_root = project.join(".ac");
        let matrix = ac_root.join("_agent_dev-rust");
        let replica = ac_root.join("wg-7-dev-team").join("__agent_dev-rust");
        std::fs::create_dir_all(&matrix).unwrap();
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust","tooling":{"lastCodingAgent":"claude"}}"#,
        )
        .unwrap();
        let settings = settings_with_project(&project);

        set_replica_coding_agent_selection(&settings, &replica, "codex", "b").unwrap();

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(replica.join("config.json")).unwrap())
                .unwrap();
        assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
        assert_eq!(saved["tooling"]["profile"], "B");
        assert_eq!(saved["tooling"]["instanceProfileOverride"], "B");
        assert_eq!(
            state_value(&replica)["tooling"]["lastCodingAgent"],
            "claude"
        );
        assert_untracked(&replica, &["lastCodingAgent"]);
    }

    #[test]
    fn divergent_profile_and_legacy_override_returns_warning_and_prefers_v2() {
        let temp = tempfile::tempdir().unwrap();
        let agent_dir = temp.path().join("__agent_dev-rust");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("config.json"),
            r#"{"tooling":{"profile":"B","instanceProfileOverride":"C"}}"#,
        )
        .unwrap();

        let read = read_replica_profile_result(&agent_dir);

        assert_eq!(read.profile.as_deref(), Some("B"));
        assert!(read.warning.unwrap().contains("differs"));
    }

    #[test]
    fn replica_origin_default_is_followed_when_identity_is_valid() {
        let temp = tempfile::tempdir().unwrap();
        let ac_root = temp.path().join("project").join(".ac");
        let matrix = ac_root.join("_agent_dev-rust");
        let replica = ac_root.join("wg-7-dev-team").join("__agent_dev-rust");
        std::fs::create_dir_all(&matrix).unwrap();
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::write(
            matrix.join("config.json"),
            r#"{"tooling":{"defaultProfile":"B"}}"#,
        )
        .unwrap();
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust"}"#,
        )
        .unwrap();

        let settings = settings_with_cells(&[("codex", vec!["A", "B"])]);
        let resolved = resolve_profile(
            &settings,
            ProfileResolutionRequest {
                coding_agent_id: "codex",
                launch_path: Some(&replica),
                agent_matrix_name: None,
                requested_profile: None,
                requested_profile_authoritative: false,
            },
        );

        assert_eq!(resolved.requested_profile, "B");
        assert_eq!(resolved.effective_profile, "B");
    }

    #[test]
    fn profile_selection_rejects_arbitrary_agent_dir_outside_ac_roots() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir_all(project.join(".ac")).unwrap();
        let fake = temp.path().join("_agent_fake");
        std::fs::create_dir_all(&fake).unwrap();
        let settings = settings_with_project(&project);

        let err = validate_profile_selection_agent_path(&settings, &fake).unwrap_err();

        assert!(
            err.contains("Project AC Root") || err.contains("configured AC project roots"),
            "{err}"
        );
        assert!(!fake.join("config.json").exists());
    }

    #[test]
    fn profile_selection_accepts_real_matrix_and_replica_paths() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let ac_root = project.join(".ac");
        let matrix = ac_root.join("_agent_dev-rust");
        let replica = ac_root.join("wg-7-dev-team").join("__agent_dev-rust");
        std::fs::create_dir_all(&matrix).unwrap();
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust"}"#,
        )
        .unwrap();
        let settings = settings_with_project(&project);

        let matrix_validated = validate_profile_selection_agent_path(&settings, &matrix).unwrap();
        let replica_validated = validate_profile_selection_agent_path(&settings, &replica).unwrap();
        let matrix_canonical = canonical_existing_dir(&matrix, "Agent Matrix").unwrap();

        assert!(same_canonical_path(
            &matrix_validated.origin_matrix_dir,
            &matrix_canonical
        ));
        assert!(same_canonical_path(
            &replica_validated.origin_matrix_dir,
            &matrix_canonical
        ));
    }

    #[test]
    fn profile_selection_rejects_matrix_outside_configured_project_paths() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let other_project = temp.path().join("other");
        std::fs::create_dir_all(project.join(".ac")).unwrap();
        let fake = other_project.join(".ac").join("_agent_fake");
        std::fs::create_dir_all(&fake).unwrap();
        let settings = settings_with_project(&project);

        let err = validate_profile_selection_agent_path(&settings, &fake).unwrap_err();

        assert!(err.contains("configured AC project roots"), "{err}");
        assert!(!fake.join("config.json").exists());
    }

    // #592 - profile content-hash replica persistence.

    #[test]
    fn profile_content_hash_round_trips_and_preserves_profile() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let ac_root = project.join(".ac");
        let matrix = ac_root.join("_agent_dev-rust");
        let replica = ac_root.join("wg-7-dev-team").join("__agent_dev-rust");
        std::fs::create_dir_all(&matrix).unwrap();
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust","tooling":{"lastCodingAgent":"claude"}}"#,
        )
        .unwrap();
        let settings = settings_with_project(&project);

        // Assign a profile first (writes tooling.profile = "B").
        set_replica_coding_agent_selection(&settings, &replica, "codex", "b").unwrap();
        // Persist a content-hash.
        set_replica_profile_content_hash(&replica, "deadbeefdeadbeef").unwrap();

        assert_eq!(
            read_replica_profile_content_hash(&replica).as_deref(),
            Some("deadbeefdeadbeef")
        );
        // The profile assignment is untouched by the hash write.
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(replica.join("config.json")).unwrap())
                .unwrap();
        assert_eq!(saved["tooling"]["profile"], "B");
        assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
        assert_eq!(
            state_value(&replica)["tooling"]["profileContentHash"],
            "deadbeefdeadbeef"
        );
        assert_untracked(&replica, &["profileContentHash", "lastCodingAgent"]);
    }

    #[test]
    fn profile_content_hash_write_is_noop_off_replica() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo-thing");
        std::fs::create_dir_all(&repo).unwrap();

        // A normal repo dir is not a replica/matrix: the write is a silent no-op
        // and must NOT create a config.json.
        assert!(set_replica_profile_content_hash(&repo, "x").is_ok());
        assert!(!repo.join("config.json").exists());
        assert_eq!(read_replica_profile_content_hash(&repo), None);
    }

    #[test]
    fn profile_content_hash_write_persists_for_root_agent_dir() {
        let temp = tempfile::tempdir().unwrap();
        // The Root Agent dir name does NOT start with `__agent_`/`_agent_`, but it
        // is a legitimate replica running a coding agent: the hash must persist
        // and round-trip so drift survives an AC restart (#592 Root Agent fix).
        let root_agent = temp
            .path()
            .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
        std::fs::create_dir_all(&root_agent).unwrap();

        set_replica_profile_content_hash(&root_agent, "cafef00dcafef00d").unwrap();

        assert_eq!(
            read_replica_profile_content_hash(&root_agent).as_deref(),
            Some("cafef00dcafef00d")
        );
        assert_eq!(
            state_value(&root_agent)["tooling"]["profileContentHash"],
            "cafef00dcafef00d"
        );
        assert_untracked(&root_agent, &["profileContentHash"]);
    }

    // ------------------------------------------------------------------
    // #1939 replica selection state, intents, defaults and guarded writes.
    // ------------------------------------------------------------------

    const REPLICA_IDENTITY: &str = "../../_agent_dev-rust";

    struct SelectionFixture {
        _temp: tempfile::TempDir,
        matrix: PathBuf,
        replica: PathBuf,
        settings: AppSettings,
    }

    /// A minimal valid replica/matrix pair (`_agent_dev-rust` plus
    /// `wg-7-dev-team/__agent_dev-rust`) so the strict read and the settings
    /// path validation both accept the replica.
    fn selection_fixture() -> SelectionFixture {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let ac_root = project.join(".ac");
        let matrix = ac_root.join("_agent_dev-rust");
        let replica = ac_root.join("wg-7-dev-team").join("__agent_dev-rust");
        std::fs::create_dir_all(&matrix).unwrap();
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::write(matrix.join("Role.md"), "# Role\n").unwrap();
        let settings = settings_with_project(&project);
        SelectionFixture {
            _temp: temp,
            matrix,
            replica,
            settings,
        }
    }

    fn seed_selection_config(tooling_json: &str) -> String {
        format!(r#"{{"identity":"{REPLICA_IDENTITY}","tooling":{tooling_json}}}"#)
    }

    fn write_config(replica: &Path, bytes: &str) {
        std::fs::write(replica.join("config.json"), bytes).unwrap();
    }

    fn config_bytes(replica: &Path) -> String {
        std::fs::read_to_string(replica.join("config.json")).unwrap()
    }

    fn config_value(replica: &Path) -> Value {
        serde_json::from_str(&config_bytes(replica)).unwrap()
    }

    /// #2816 (C3) - the state file beside `dir`'s `config.json`.
    fn state_path(dir: &Path) -> PathBuf {
        dir.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
    }

    fn state_value(dir: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(state_path(dir)).unwrap()).unwrap()
    }

    /// #2816 (C3) - none of `keys` may remain in `dir`'s tracked `config.json`;
    /// an absent tracked file holds none.
    fn assert_untracked(dir: &Path, keys: &[&str]) {
        let Ok(raw) = std::fs::read_to_string(dir.join("config.json")) else {
            return;
        };
        let tracked: Value = serde_json::from_str(&raw).unwrap();
        for key in keys {
            assert!(
                tracked["tooling"].get(key).is_none(),
                "{key} is still tracked in {}",
                dir.display()
            );
        }
    }

    fn pair(coding_agent_id: &str, profile: &str) -> ReplicaSelectionPair {
        ReplicaSelectionPair {
            coding_agent_id: coding_agent_id.to_string(),
            requested_profile: profile.to_string(),
        }
    }

    #[test]
    fn issue_1937_selection_state_read_absent_invalid_and_duplicate_json() {
        let fixture = selection_fixture();

        // Absent config.json is Invalid, never Unlocked.
        assert!(matches!(
            read_replica_selection_state(&fixture.replica),
            ReplicaSelectionState::Invalid { .. }
        ));

        // Invalid JSON.
        write_config(&fixture.replica, "{ not json");
        assert!(matches!(
            read_replica_selection_state(&fixture.replica),
            ReplicaSelectionState::Invalid { .. }
        ));

        // Duplicate keys are rejected by the strict duplicate-free parse.
        write_config(
            &fixture.replica,
            r#"{"identity":"../../_agent_dev-rust","identity":"../../_agent_dev-rust"}"#,
        );
        let duplicate = read_replica_selection_state(&fixture.replica);
        assert!(
            matches!(duplicate, ReplicaSelectionState::Invalid { .. }),
            "{duplicate:?}"
        );
    }

    #[test]
    fn issue_1937_selection_state_read_malformed_shapes_are_never_unlocked() {
        let fixture = selection_fixture();

        for tooling in [
            "5",
            r#"{"selectionLocked":"yes"}"#,
            r#"{"selectionLocked":true}"#,
            r#"{"selectionLocked":true,"currentCodingAgent":"codex"}"#,
            r#"{"selectionLocked":true,"profile":"B","currentCodingAgent":""}"#,
        ] {
            write_config(&fixture.replica, &seed_selection_config(tooling));
            let state = read_replica_selection_state(&fixture.replica);
            assert!(
                matches!(state, ReplicaSelectionState::Invalid { .. }),
                "tooling {tooling} must be Invalid, got {state:?}"
            );
            assert!(!state.is_locked());
        }
    }

    #[test]
    fn issue_1937_selection_state_read_missing_pair_on_legacy_unlocked_is_valid_and_assignable() {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(r#"{"lastCodingAgent":"claude"}"#),
        );

        let state = read_replica_selection_state(&fixture.replica);
        match &state {
            ReplicaSelectionState::Unlocked { pair, warning, .. } => {
                assert!(pair.is_none());
                assert!(warning.is_none());
            }
            other => panic!("expected Unlocked with no pair, got {other:?}"),
        }

        // A missing pair on a legacy unlocked config may be deliberately
        // assigned.
        let expected = state.expectation().unwrap();
        let outcome = write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "B"),
            SelectionWriteIntent::Individual,
            &expected,
        )
        .unwrap();
        assert!(outcome.changed && outcome.published);
        assert_eq!(outcome.lock_transition, None);
        let saved = config_value(&fixture.replica);
        assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
        assert_eq!(saved["tooling"]["profile"], "B");
        assert_eq!(saved["tooling"]["instanceProfileOverride"], "B");
        assert_eq!(saved["tooling"]["instanceProfileOverrideSource"], "manual");
        assert!(saved["tooling"].get("selectionLocked").is_none());
        assert_eq!(
            state_value(&fixture.replica)["tooling"]["lastCodingAgent"],
            "claude"
        );
        assert_untracked(&fixture.replica, &["lastCodingAgent"]);
    }

    #[test]
    fn issue_1937_selection_state_read_coherent_and_divergent_duals() {
        let fixture = selection_fixture();

        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"codex","profile":"B","instanceProfileOverride":"B"}"#,
            ),
        );
        match read_replica_selection_state(&fixture.replica) {
            ReplicaSelectionState::Unlocked { pair, warning, .. } => {
                assert_eq!(pair, Some(self::pair("codex", "B")));
                assert!(warning.is_none());
            }
            other => panic!("expected Unlocked pair, got {other:?}"),
        }

        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"codex","profile":"B","instanceProfileOverride":"C"}"#,
            ),
        );
        match read_replica_selection_state(&fixture.replica) {
            ReplicaSelectionState::Unlocked { pair, warning, .. } => {
                assert_eq!(pair, Some(self::pair("codex", "B")), "profile wins");
                let warning = warning.expect("divergent dual fields warn");
                assert!(warning.contains("differs"), "{warning}");
            }
            other => panic!("expected Unlocked pair, got {other:?}"),
        }
    }

    #[test]
    fn issue_1937_selection_state_write_preserves_unknown_history_and_hash() {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"claude","profile":"A","instanceProfileOverride":"A","lastCodingAgent":"claude","profileContentHash":"deadbeef","codingAgents":{"claude":{"app":"Claude Code","lastUsed":"2026-09-02T01:00:00+00:00"}}}"#,
            ),
        );
        let mut seeded: Value = serde_json::from_str(&config_bytes(&fixture.replica)).unwrap();
        seeded["customTopLevel"] = serde_json::json!({"keep": true});
        write_config(&fixture.replica, &seeded.to_string());

        let state = read_replica_selection_state(&fixture.replica);
        let expected = state.expectation().unwrap();
        let outcome = write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "C"),
            SelectionWriteIntent::Individual,
            &expected,
        )
        .unwrap();
        assert!(outcome.changed && outcome.published);

        let saved = config_value(&fixture.replica);
        assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
        assert_eq!(saved["tooling"]["profile"], "C");
        let state = state_value(&fixture.replica);
        assert_eq!(state["tooling"]["lastCodingAgent"], "claude");
        assert_eq!(state["tooling"]["profileContentHash"], "deadbeef");
        assert_eq!(
            state["tooling"]["codingAgents"]["claude"]["app"],
            "Claude Code"
        );
        assert_untracked(
            &fixture.replica,
            &["lastCodingAgent", "profileContentHash", "codingAgents"],
        );
        assert_eq!(saved["customTopLevel"]["keep"], true);
    }

    #[test]
    fn issue_1937_selection_state_expected_state_cas_is_stale_without_overwrite() {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(r#"{"currentCodingAgent":"codex","profile":"B"}"#),
        );
        let stale_expected = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();

        // A concurrent writer moves the pair; the stale expectation must not
        // overwrite it.
        write_config(
            &fixture.replica,
            &seed_selection_config(r#"{"currentCodingAgent":"claude","profile":"A"}"#),
        );
        let before = config_bytes(&fixture.replica);
        let error = write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "C"),
            SelectionWriteIntent::Individual,
            &stale_expected,
        )
        .unwrap_err();
        assert!(error.contains("stale"), "{error}");
        assert_eq!(config_bytes(&fixture.replica), before);

        let current = read_replica_selection_state(&fixture.replica);
        let mut wrong_identity = current.expectation().unwrap();
        wrong_identity.identity = "../../_agent_other".to_string();
        let error = write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "C"),
            SelectionWriteIntent::Individual,
            &wrong_identity,
        )
        .unwrap_err();
        assert!(error.contains("stale"), "{error}");

        let mut wrong_flag = current.expectation().unwrap();
        wrong_flag.locked = true;
        let error = write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "C"),
            SelectionWriteIntent::Individual,
            &wrong_flag,
        )
        .unwrap_err();
        assert!(error.contains("stale"), "{error}");
        assert_eq!(config_bytes(&fixture.replica), before);
    }

    #[test]
    fn issue_1937_selection_state_intent_table() {
        let cases = [
            (SelectionWriteIntent::Individual, true, false, None),
            (
                SelectionWriteIntent::IndividualAssignLock,
                true,
                true,
                Some(SelectionLockTransition::UnlockedToLocked),
            ),
            (SelectionWriteIntent::BulkOrdinary, true, false, None),
            (
                SelectionWriteIntent::BulkAssignLockUnlockedOnly,
                true,
                true,
                Some(SelectionLockTransition::UnlockedToLocked),
            ),
            (
                SelectionWriteIntent::BulkForceReviewed,
                true,
                true,
                Some(SelectionLockTransition::UnlockedToLocked),
            ),
        ];
        for (intent, expect_change, expect_flag_true, transition) in cases {
            let fixture = selection_fixture();
            write_config(
                &fixture.replica,
                &seed_selection_config(r#"{"currentCodingAgent":"claude","profile":"A"}"#),
            );
            let expected = read_replica_selection_state(&fixture.replica)
                .expectation()
                .unwrap();
            let outcome = write_replica_selection(
                &fixture.settings,
                &fixture.replica,
                &pair("codex", "B"),
                intent,
                &expected,
            )
            .unwrap();
            assert_eq!(outcome.changed, expect_change, "{intent:?}");
            assert_eq!(outcome.published, expect_change, "{intent:?}");
            assert_eq!(outcome.lock_transition, transition, "{intent:?}");
            let saved = config_value(&fixture.replica);
            assert_eq!(
                saved["tooling"]["currentCodingAgent"], "codex",
                "{intent:?}"
            );
            assert_eq!(saved["tooling"]["profile"], "B", "{intent:?}");
            let stored_flag = saved["tooling"]
                .get("selectionLocked")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            assert_eq!(stored_flag, expect_flag_true, "{intent:?}");
        }

        // Locked replicas: individual intents are rejected (bytes untouched);
        // bulk intents skip without publishing; force applies and keeps locked.
        for intent in [
            SelectionWriteIntent::Individual,
            SelectionWriteIntent::IndividualAssignLock,
        ] {
            let fixture = selection_fixture();
            write_config(
                &fixture.replica,
                &seed_selection_config(
                    r#"{"currentCodingAgent":"claude","profile":"A","selectionLocked":true}"#,
                ),
            );
            let before = config_bytes(&fixture.replica);
            let expected = read_replica_selection_state(&fixture.replica)
                .expectation()
                .unwrap();
            let error = write_replica_selection(
                &fixture.settings,
                &fixture.replica,
                &pair("codex", "B"),
                intent,
                &expected,
            )
            .unwrap_err();
            assert!(error.contains("locked"), "{intent:?}: {error}");
            assert_eq!(config_bytes(&fixture.replica), before, "{intent:?}");
        }
        for intent in [
            SelectionWriteIntent::BulkOrdinary,
            SelectionWriteIntent::BulkAssignLockUnlockedOnly,
        ] {
            let fixture = selection_fixture();
            write_config(
                &fixture.replica,
                &seed_selection_config(
                    r#"{"currentCodingAgent":"claude","profile":"A","selectionLocked":true}"#,
                ),
            );
            let before = config_bytes(&fixture.replica);
            let expected = read_replica_selection_state(&fixture.replica)
                .expectation()
                .unwrap();
            let outcome = write_replica_selection(
                &fixture.settings,
                &fixture.replica,
                &pair("codex", "B"),
                intent,
                &expected,
            )
            .unwrap();
            assert_eq!(outcome, SelectionWriteOutcome::UNCHANGED, "{intent:?}");
            assert_eq!(config_bytes(&fixture.replica), before, "{intent:?}");
        }

        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"claude","profile":"A","selectionLocked":true}"#,
            ),
        );
        let before = config_bytes(&fixture.replica);
        let expected = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();
        let outcome = write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "B"),
            SelectionWriteIntent::BulkForceReviewed,
            &expected,
        )
        .unwrap();
        assert!(outcome.changed && outcome.published);
        assert_eq!(outcome.lock_transition, None, "already locked");
        assert_ne!(config_bytes(&fixture.replica), before);
        let saved = config_value(&fixture.replica);
        assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
        assert_eq!(saved["tooling"]["selectionLocked"], true);
    }

    #[test]
    fn issue_1937_selection_state_malformed_is_never_force_repaired() {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":"yes"}"#,
            ),
        );
        let before = config_bytes(&fixture.replica);
        let invalid = read_replica_selection_state(&fixture.replica);
        assert!(matches!(invalid, ReplicaSelectionState::Invalid { .. }));
        assert!(invalid.expectation().is_err());

        let error = write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "B"),
            SelectionWriteIntent::BulkForceReviewed,
            &ReplicaSelectionExpectation {
                identity: REPLICA_IDENTITY.to_string(),
                pair: Some(pair("codex", "B")),
                locked: false,
            },
        )
        .unwrap_err();
        assert!(error.contains("invalid"), "{error}");
        assert_eq!(config_bytes(&fixture.replica), before);

        let error = clear_replica_selection_lock(
            &fixture.settings,
            &fixture.replica,
            &ReplicaSelectionExpectation {
                identity: REPLICA_IDENTITY.to_string(),
                pair: Some(pair("codex", "B")),
                locked: true,
            },
        )
        .unwrap_err();
        assert!(error.contains("invalid"), "{error}");
        assert_eq!(config_bytes(&fixture.replica), before);
    }

    #[test]
    fn issue_1937_selection_state_requested_letter_survives_resolution_and_content_changes() {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(r#"{"currentCodingAgent":"codex"}"#),
        );
        let expected = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();
        // Settings only enable A..C, so resolving the stored D falls back; the
        // stored requested letter must stay D.
        write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "D"),
            SelectionWriteIntent::Individual,
            &expected,
        )
        .unwrap();
        let resolution = resolve_profile(
            &fixture.settings,
            ProfileResolutionRequest {
                coding_agent_id: "codex",
                launch_path: Some(&fixture.replica),
                agent_matrix_name: None,
                requested_profile: None,
                requested_profile_authoritative: false,
            },
        );
        assert_eq!(resolution.requested_profile, "D");
        assert_eq!(resolution.effective_profile, "C");

        // Content/config changes do not rewrite the stored requested letter.
        let changed_settings = settings_with_cells(&[("codex", vec![])]);
        let resolution = resolve_profile(
            &changed_settings,
            ProfileResolutionRequest {
                coding_agent_id: "codex",
                launch_path: Some(&fixture.replica),
                agent_matrix_name: None,
                requested_profile: None,
                requested_profile_authoritative: false,
            },
        );
        assert_eq!(resolution.requested_profile, "D");
        assert_eq!(resolution.effective_profile, "A");
        assert_eq!(
            read_replica_selection_state(&fixture.replica).pair(),
            Some(&pair("codex", "D"))
        );
    }

    #[test]
    fn issue_1937_selection_state_clear_locked_to_unlocked_publishes() {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}"#,
            ),
        );
        let expected = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();
        let outcome =
            clear_replica_selection_lock(&fixture.settings, &fixture.replica, &expected).unwrap();
        assert_eq!(
            outcome,
            SelectionWriteOutcome {
                changed: true,
                published: true,
                lock_transition: Some(SelectionLockTransition::LockedToUnlocked),
            }
        );
        let saved = config_value(&fixture.replica);
        assert_eq!(saved["tooling"]["selectionLocked"], false);
        assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
        assert_eq!(saved["tooling"]["profile"], "B");
        match read_replica_selection_state(&fixture.replica) {
            ReplicaSelectionState::Unlocked { pair, .. } => {
                assert_eq!(pair, Some(self::pair("codex", "B")));
            }
            other => panic!("expected Unlocked, got {other:?}"),
        }
    }

    #[test]
    fn issue_1937_selection_state_already_unlocked_no_publish() {
        let fixture = selection_fixture();

        // Absent flag: exact deliberately non-pretty bytes retained.
        write_config(
            &fixture.replica,
            &seed_selection_config(r#"{"currentCodingAgent":"codex","profile":"B"}"#),
        );
        let before = config_bytes(&fixture.replica);
        let expected = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();
        assert_eq!(
            clear_replica_selection_lock(&fixture.settings, &fixture.replica, &expected).unwrap(),
            SelectionWriteOutcome::UNCHANGED
        );
        assert_eq!(config_bytes(&fixture.replica), before);

        // Explicit false: same no-publish path.
        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":false}"#,
            ),
        );
        let before = config_bytes(&fixture.replica);
        let expected = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();
        assert_eq!(
            clear_replica_selection_lock(&fixture.settings, &fixture.replica, &expected).unwrap(),
            SelectionWriteOutcome::UNCHANGED
        );
        assert_eq!(config_bytes(&fixture.replica), before);

        // Legacy debris selects the explicit refusal callback for decisions only.
        // The already-unlocked path must bypass that callback and preserve bytes.
        let temp_obstruction = validated_replica_dir(&fixture.settings, &fixture.replica)
            .unwrap()
            .join(format!(".config.json.{}.tmp", std::process::id()));
        std::fs::create_dir(&temp_obstruction).unwrap();
        let publish = |tmp: &std::path::Path, target: &std::path::Path| {
            assert!(
                tmp.is_file(),
                "fresh staging acquired before injected refusal"
            );
            if crate::config::agent_config::temp_config_path(target) == temp_obstruction {
                assert!(temp_obstruction.is_dir());
                Err("fixture temp config publish failure".into())
            } else {
                crate::config::agent_config::publish_temp_config(tmp, target)
            }
        };
        assert_eq!(
            clear_replica_selection_lock_with_publish(
                &fixture.settings,
                &fixture.replica,
                &expected,
                Some(&publish)
            )
            .unwrap(),
            SelectionWriteOutcome::UNCHANGED
        );
        assert!(temp_obstruction.is_dir());
        assert_eq!(config_bytes(&fixture.replica), before);

        // Positive control: a true flag reaches the publisher and is refused
        // after fresh acquisition — the error is never masked as success.
        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}"#,
            ),
        );
        let locked_before = config_bytes(&fixture.replica);
        let locked_expected = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();
        let error = clear_replica_selection_lock_with_publish(
            &fixture.settings,
            &fixture.replica,
            &locked_expected,
            Some(&publish),
        )
        .unwrap_err();
        assert!(error.contains("temp config"), "{error}");
        assert!(temp_obstruction.is_dir());
        assert_eq!(config_bytes(&fixture.replica), locked_before);
    }

    #[test]
    fn issue_1937_selection_state_clear_stale_race_is_not_already_unlocked() {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(r#"{"currentCodingAgent":"codex","profile":"B"}"#),
        );
        let unlocked_expectation = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();

        // The flag races to true after the read.
        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}"#,
            ),
        );
        let before = config_bytes(&fixture.replica);
        let error = clear_replica_selection_lock(
            &fixture.settings,
            &fixture.replica,
            &unlocked_expectation,
        )
        .unwrap_err();
        assert!(error.contains("stale"), "{error}");
        assert_eq!(config_bytes(&fixture.replica), before);
    }

    #[test]
    fn issue_1937_selection_state_lock_timeout_is_not_success() {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(r#"{"currentCodingAgent":"codex","profile":"B"}"#),
        );
        let expected = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();

        // Hold the #1938 sidecar: the guarded call must time out and propagate,
        // never translate into AlreadyUnlocked success.
        let sidecar = std::fs::canonicalize(&fixture.replica)
            .unwrap()
            .join(".config.json.lock");
        let holder = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&sidecar)
            .unwrap();
        holder.lock().unwrap();

        let before = config_bytes(&fixture.replica);
        let error = clear_replica_selection_lock(&fixture.settings, &fixture.replica, &expected)
            .unwrap_err();
        assert!(error.contains("configLockTimeout"), "{error}");
        assert_eq!(config_bytes(&fixture.replica), before);
        drop(holder);
    }

    #[test]
    fn issue_1937_selection_state_default_read_shapes() {
        let fixture = selection_fixture();

        assert_eq!(
            read_replica_selection_default(&fixture.matrix).unwrap(),
            None
        );

        std::fs::write(
            fixture.matrix.join("config.json"),
            r#"{"context":["Role.md"]}"#,
        )
        .unwrap();
        assert_eq!(
            read_replica_selection_default(&fixture.matrix).unwrap(),
            None
        );
        std::fs::write(
            fixture.matrix.join("config.json"),
            r#"{"tooling":{"defaultProfile":"B"}}"#,
        )
        .unwrap();
        assert_eq!(
            read_replica_selection_default(&fixture.matrix).unwrap(),
            None
        );

        std::fs::write(
            fixture.matrix.join("config.json"),
            r#"{"tooling":{"replicaSelectionDefault":{"codingAgentId":"codex","requestedProfile":"b","selectionLocked":true}}}"#,
        )
        .unwrap();
        assert_eq!(
            read_replica_selection_default(&fixture.matrix).unwrap(),
            Some(ReplicaSelectionDefault {
                coding_agent_id: "codex".to_string(),
                requested_profile: "B".to_string(),
                selection_locked: true,
            })
        );

        for tooling in [
            r#"{"replicaSelectionDefault":5}"#,
            r#"{"replicaSelectionDefault":{"requestedProfile":"B","selectionLocked":false}}"#,
            r#"{"replicaSelectionDefault":{"codingAgentId":"","requestedProfile":"B","selectionLocked":false}}"#,
            r#"{"replicaSelectionDefault":{"codingAgentId":"codex","requestedProfile":"AB","selectionLocked":false}}"#,
            r#"{"replicaSelectionDefault":{"codingAgentId":"codex","requestedProfile":"B"}}"#,
            r#"{"replicaSelectionDefault":{"codingAgentId":"codex","requestedProfile":"B","selectionLocked":"yes"}}"#,
        ] {
            std::fs::write(
                fixture.matrix.join("config.json"),
                format!(r#"{{"tooling":{tooling}}}"#),
            )
            .unwrap();
            let error = read_replica_selection_default(&fixture.matrix).unwrap_err();
            assert!(
                error.contains("replicaSelectionDefault"),
                "{tooling}: {error}"
            );
        }

        std::fs::write(fixture.matrix.join("config.json"), r#"{"tooling":5}"#).unwrap();
        assert!(read_replica_selection_default(&fixture.matrix).is_err());
        std::fs::write(fixture.matrix.join("config.json"), "{ not json").unwrap();
        assert!(read_replica_selection_default(&fixture.matrix).is_err());
    }

    #[test]
    fn issue_1937_selection_state_default_write_cas_and_preservation() {
        let fixture = selection_fixture();
        let default = ReplicaSelectionDefault {
            coding_agent_id: "codex".to_string(),
            requested_profile: "b".to_string(),
            selection_locked: false,
        };

        let outcome = write_replica_selection_default(&fixture.matrix, &default, None).unwrap();
        assert!(outcome.changed && outcome.published);
        assert_eq!(outcome.lock_transition, None);
        let saved = config_value(&fixture.matrix);
        assert_eq!(
            saved["tooling"]["replicaSelectionDefault"]["requestedProfile"],
            "B"
        );

        // An expected-prior of None is now stale.
        let stale = write_replica_selection_default(&fixture.matrix, &default, None).unwrap_err();
        assert!(stale.contains("stale"), "{stale}");

        // Update with the exact prior: other tooling and unknown keys survive.
        let mut matrix = config_value(&fixture.matrix);
        matrix["tooling"]["defaultProfile"] = serde_json::json!("A");
        matrix["customProjectKey"] = serde_json::json!(["keep"]);
        std::fs::write(
            fixture.matrix.join("config.json"),
            serde_json::to_string(&matrix).unwrap(),
        )
        .unwrap();
        let prior = read_replica_selection_default(&fixture.matrix).unwrap();
        let next = ReplicaSelectionDefault {
            coding_agent_id: "claude".to_string(),
            requested_profile: "C".to_string(),
            selection_locked: true,
        };
        write_replica_selection_default(&fixture.matrix, &next, prior.as_ref()).unwrap();
        let saved = config_value(&fixture.matrix);
        assert_eq!(
            saved["tooling"]["replicaSelectionDefault"]["codingAgentId"],
            "claude"
        );
        assert_eq!(
            saved["tooling"]["replicaSelectionDefault"]["selectionLocked"],
            true
        );
        assert_eq!(saved["tooling"]["defaultProfile"], "A");
        assert_eq!(saved["customProjectKey"][0], "keep");

        // A wrong prior is stale and publishes nothing.
        let before = config_bytes(&fixture.matrix);
        let wrong = write_replica_selection_default(&fixture.matrix, &default, None).unwrap_err();
        assert!(wrong.contains("stale"), "{wrong}");
        assert_eq!(config_bytes(&fixture.matrix), before);
    }

    #[test]
    fn issue_1937_selection_state_null_clear_rejected_for_protected_pair() {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}"#,
            ),
        );
        let before = config_bytes(&fixture.replica);
        let error =
            set_instance_profile_override(&fixture.settings, &fixture.replica, None).unwrap_err();
        assert!(error.contains("protected"), "{error}");
        assert_eq!(config_bytes(&fixture.replica), before);

        // A new valid requested letter is permitted and preserves Coding Agent
        // and the lock flag.
        set_instance_profile_override(&fixture.settings, &fixture.replica, Some("d")).unwrap();
        let saved = config_value(&fixture.replica);
        assert_eq!(saved["tooling"]["profile"], "D");
        assert_eq!(saved["tooling"]["instanceProfileOverride"], "D");
        assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
        assert_eq!(saved["tooling"]["selectionLocked"], true);

        // A legacy unlocked config without a pair may still be cleared.
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(r#"{"instanceProfileOverride":"C"}"#),
        );
        set_instance_profile_override(&fixture.settings, &fixture.replica, None).unwrap();
        let saved = config_value(&fixture.replica);
        assert!(saved["tooling"].get("profile").is_none());
        assert!(saved["tooling"].get("instanceProfileOverride").is_none());
    }

    #[test]
    fn issue_1937_selection_state_wrapper_preserves_flag_and_rejects_locked() {
        let fixture = selection_fixture();

        // Unlocked: the pair is written, the absent flag stays absent.
        write_config(
            &fixture.replica,
            &seed_selection_config(r#"{"lastCodingAgent":"claude"}"#),
        );
        set_replica_coding_agent_selection(&fixture.settings, &fixture.replica, "codex", "b")
            .unwrap();
        let saved = config_value(&fixture.replica);
        assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
        assert_eq!(saved["tooling"]["profile"], "B");
        assert!(saved["tooling"].get("selectionLocked").is_none());
        assert_eq!(
            state_value(&fixture.replica)["tooling"]["lastCodingAgent"],
            "claude"
        );
        assert_untracked(&fixture.replica, &["lastCodingAgent"]);

        // Locked: rejected, stored bytes untouched.
        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"codex","profile":"A","selectionLocked":true}"#,
            ),
        );
        let before = config_bytes(&fixture.replica);
        let error =
            set_replica_coding_agent_selection(&fixture.settings, &fixture.replica, "codex", "B")
                .unwrap_err();
        assert!(error.contains("locked"), "{error}");
        assert_eq!(config_bytes(&fixture.replica), before);
    }

    #[test]
    fn issue_1937_selection_state_content_hash_write_does_not_reset_malformed_tooling() {
        let fixture = selection_fixture();

        write_config(
            &fixture.replica,
            r#"{"identity":"../../_agent_dev-rust","tooling":5}"#,
        );
        let before = config_bytes(&fixture.replica);
        let error = set_replica_profile_content_hash(&fixture.replica, "deadbeef").unwrap_err();
        assert!(
            error.contains("'tooling' must be a JSON object in the tracked config file"),
            "{error}"
        );
        assert_eq!(config_bytes(&fixture.replica), before);

        // A malformed selectionLocked inside a valid tooling object stays exactly
        // as stored while the hash is updated.
        write_config(
            &fixture.replica,
            r#"{"identity":"../../_agent_dev-rust","tooling":{"selectionLocked":"yes","currentCodingAgent":"codex"}}"#,
        );
        set_replica_profile_content_hash(&fixture.replica, "deadbeef").unwrap();
        let saved = config_value(&fixture.replica);
        assert_eq!(saved["tooling"]["selectionLocked"], "yes");
        assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
        assert_eq!(
            state_value(&fixture.replica)["tooling"]["profileContentHash"],
            "deadbeef"
        );
        assert_untracked(&fixture.replica, &["profileContentHash"]);
    }

    // ------------------------------------------------------------------
    // #2010 KEEP with the same locked pair (unchanged, no publish).
    // ------------------------------------------------------------------

    // ------------------------------------------------------------------
    // #2816 (C3) - the split writer and the require-existing sites.
    // ------------------------------------------------------------------

    /// E3.
    #[test]
    fn the_two_tooling_keys_go_to_opposite_files() {
        let fixture = selection_fixture();
        write_config(&fixture.matrix, "{}");

        // The decision first: a later decision write would migrate a misplaced
        // hash and hide it.
        set_agent_default_profile(&fixture.settings, &fixture.matrix, "b").unwrap();
        set_replica_profile_content_hash(&fixture.matrix, "cafef00d").unwrap();

        let state = state_value(&fixture.matrix);
        let tracked = config_value(&fixture.matrix);
        assert_eq!(state["tooling"]["profileContentHash"], "cafef00d");
        assert!(state["tooling"].get("defaultProfile").is_none(), "{state}");
        assert_eq!(tracked["tooling"]["defaultProfile"], "B");
        assert_untracked(&fixture.matrix, &["profileContentHash"]);
    }

    /// E3b (a): with both files absent, sites 9 and 10 reject earlier, on the
    /// replica identity, and create nothing. The seeded second leg proves the
    /// rows are sensitive to the missing identity and nothing more.
    #[test]
    fn an_absent_config_still_refuses_and_creates_nothing() {
        const IDENTITY_TAIL: &str =
            "__agent_dev-rust' has no config.json identity; expected '../../_agent_dev-rust'";
        let is_identity_rejection = |result: &Result<SelectionWriteOutcome, String>| {
            matches!(result, Err(error)
                if error.starts_with("Room replica '") && error.ends_with(IDENTITY_TAIL))
        };
        let expected = ReplicaSelectionExpectation {
            identity: REPLICA_IDENTITY.to_string(),
            pair: None,
            locked: false,
        };

        // Site 9.
        let fixture = selection_fixture();
        let write = || {
            write_replica_selection(
                &fixture.settings,
                &fixture.replica,
                &pair("codex", "B"),
                SelectionWriteIntent::Individual,
                &expected,
            )
        };
        let result = write();
        assert!(is_identity_rejection(&result), "site 9: {result:?}");
        assert!(!fixture.replica.join("config.json").exists(), "site 9");
        assert!(!state_path(&fixture.replica).exists(), "site 9");
        write_config(&fixture.replica, &seed_selection_config("{}"));
        let result = write();
        assert!(!is_identity_rejection(&result), "site 9 seeded: {result:?}");

        // Site 10.
        let fixture = selection_fixture();
        let clear = || clear_replica_selection_lock(&fixture.settings, &fixture.replica, &expected);
        let result = clear();
        assert!(is_identity_rejection(&result), "site 10: {result:?}");
        assert!(!fixture.replica.join("config.json").exists(), "site 10");
        assert!(!state_path(&fixture.replica).exists(), "site 10");
        write_config(&fixture.replica, &seed_selection_config("{}"));
        let result = clear();
        assert!(
            !is_identity_rejection(&result),
            "site 10 seeded: {result:?}"
        );
    }

    /// E3b, consequence 4: a new effect of C2's validation, not a preserved
    /// contract.
    #[test]
    fn a_malformed_state_tooling_now_fails_the_selection_write() {
        let fixture = selection_fixture();
        write_config(&fixture.replica, &seed_selection_config("{}"));
        std::fs::write(state_path(&fixture.replica), r#"{"tooling":5}"#).unwrap();
        let before = config_bytes(&fixture.replica);
        let expected = read_replica_selection_state(&fixture.replica)
            .expectation()
            .unwrap();

        let error = write_replica_selection(
            &fixture.settings,
            &fixture.replica,
            &pair("codex", "B"),
            SelectionWriteIntent::Individual,
            &expected,
        )
        .unwrap_err();

        assert!(error.starts_with("Config pair "), "{error}");
        assert!(
            error.ends_with("cleanup failed: 'tooling' must be a JSON object in the state file"),
            "{error}"
        );
        assert_eq!(config_bytes(&fixture.replica), before);
        assert_eq!(
            std::fs::read_to_string(state_path(&fixture.replica)).unwrap(),
            r#"{"tooling":5}"#
        );
    }

    /// E4, site 11, through its nearest reachable caller.
    #[test]
    fn the_profile_override_writes_the_decisions_file_and_no_state_file() {
        let fixture = selection_fixture();
        write_config(&fixture.replica, &seed_selection_config("{}"));

        set_instance_profile_override(&fixture.settings, &fixture.replica, Some("c")).unwrap();

        let tooling = config_value(&fixture.replica)["tooling"].clone();
        assert_eq!(tooling["profile"], "C");
        assert_eq!(tooling["instanceProfileOverride"], "C");
        assert_eq!(tooling["instanceProfileOverrideSource"], "manual");
        assert!(
            !state_path(&fixture.replica).exists(),
            "site 11 created a state file"
        );
    }

    #[test]
    fn issue_2010_locked_same_pair_individual_intents_are_unchanged() {
        for intent in [
            SelectionWriteIntent::Individual,
            SelectionWriteIntent::IndividualAssignLock,
        ] {
            let fixture = selection_fixture();
            write_config(
                &fixture.replica,
                &seed_selection_config(
                    r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}"#,
                ),
            );
            let before = config_bytes(&fixture.replica);
            let expected = read_replica_selection_state(&fixture.replica)
                .expectation()
                .unwrap();

            // A lowercase request proves the letter is normalized before the
            // equality check and still matches the stored pair.
            let outcome = write_replica_selection(
                &fixture.settings,
                &fixture.replica,
                &pair("codex", "b"),
                intent,
                &expected,
            )
            .unwrap();
            assert_eq!(outcome, SelectionWriteOutcome::UNCHANGED, "{intent:?}");
            assert_eq!(config_bytes(&fixture.replica), before, "{intent:?}");
            assert_eq!(
                config_value(&fixture.replica)["tooling"]["selectionLocked"],
                true,
                "{intent:?}"
            );

            // A directory at the known temp-config path proves the unchanged
            // path never even attempts a temp file.
            let temp_obstruction = fixture
                .replica
                .join(format!(".config.json.{}.tmp", std::process::id()));
            std::fs::create_dir(&temp_obstruction).unwrap();
            let outcome = write_replica_selection(
                &fixture.settings,
                &fixture.replica,
                &pair("codex", "b"),
                intent,
                &expected,
            )
            .unwrap();
            assert_eq!(outcome, SelectionWriteOutcome::UNCHANGED, "{intent:?}");
            assert!(temp_obstruction.is_dir());
            assert_eq!(config_bytes(&fixture.replica), before, "{intent:?}");
        }
    }

    #[test]
    fn issue_2010_locked_different_agent_rejected() {
        for intent in [
            SelectionWriteIntent::Individual,
            SelectionWriteIntent::IndividualAssignLock,
        ] {
            let fixture = selection_fixture();
            write_config(
                &fixture.replica,
                &seed_selection_config(
                    r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}"#,
                ),
            );
            let before = config_bytes(&fixture.replica);
            let expected = read_replica_selection_state(&fixture.replica)
                .expectation()
                .unwrap();
            let error = write_replica_selection(
                &fixture.settings,
                &fixture.replica,
                &pair("claude", "B"),
                intent,
                &expected,
            )
            .unwrap_err();
            assert!(error.contains("locked"), "{intent:?}: {error}");
            assert_eq!(config_bytes(&fixture.replica), before, "{intent:?}");
        }
    }

    #[test]
    fn issue_2010_locked_different_profile_rejected() {
        for intent in [
            SelectionWriteIntent::Individual,
            SelectionWriteIntent::IndividualAssignLock,
        ] {
            let fixture = selection_fixture();
            write_config(
                &fixture.replica,
                &seed_selection_config(
                    r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}"#,
                ),
            );
            let before = config_bytes(&fixture.replica);
            let expected = read_replica_selection_state(&fixture.replica)
                .expectation()
                .unwrap();
            let error = write_replica_selection(
                &fixture.settings,
                &fixture.replica,
                &pair("codex", "C"),
                intent,
                &expected,
            )
            .unwrap_err();
            assert!(error.contains("locked"), "{intent:?}: {error}");
            assert_eq!(config_bytes(&fixture.replica), before, "{intent:?}");
        }
    }

    #[test]
    fn issue_2010_locked_same_pair_stale_expectation_rejected() {
        for intent in [
            SelectionWriteIntent::Individual,
            SelectionWriteIntent::IndividualAssignLock,
        ] {
            let fixture = selection_fixture();
            write_config(
                &fixture.replica,
                &seed_selection_config(
                    r#"{"currentCodingAgent":"codex","profile":"B","selectionLocked":true}"#,
                ),
            );
            let stale = read_replica_selection_state(&fixture.replica)
                .expectation()
                .unwrap();

            // A concurrent writer moves the pair; the stale expectation must
            // fail the CAS before the same-pair rule can apply.
            let rewritten = seed_selection_config(
                r#"{"currentCodingAgent":"codex","profile":"A","selectionLocked":true}"#,
            );
            write_config(&fixture.replica, &rewritten);
            let error = write_replica_selection(
                &fixture.settings,
                &fixture.replica,
                &pair("codex", "A"),
                intent,
                &stale,
            )
            .unwrap_err();
            assert!(error.contains("stale"), "{intent:?}: {error}");
            assert_eq!(config_bytes(&fixture.replica), rewritten, "{intent:?}");
        }
    }

    #[test]
    fn issue_2010_wrapper_locked_same_pair_is_no_write() {
        let fixture = selection_fixture();
        write_config(
            &fixture.replica,
            &seed_selection_config(
                r#"{"currentCodingAgent":"codex","profile":"A","selectionLocked":true}"#,
            ),
        );
        let before = config_bytes(&fixture.replica);

        // Lowercase letter: the wrapper normalizes it to the stored pair and
        // returns Ok without writing.
        set_replica_coding_agent_selection(&fixture.settings, &fixture.replica, "codex", "a")
            .unwrap();
        assert_eq!(config_bytes(&fixture.replica), before);

        // Obstructed temp path: still no write attempt and still Ok.
        let temp_obstruction = fixture
            .replica
            .join(format!(".config.json.{}.tmp", std::process::id()));
        std::fs::create_dir(&temp_obstruction).unwrap();
        set_replica_coding_agent_selection(&fixture.settings, &fixture.replica, "codex", "a")
            .unwrap();
        assert!(temp_obstruction.is_dir());
        assert_eq!(config_bytes(&fixture.replica), before);
    }

    /// #2786 (C1) E8 - `profileContentHash` has no struct field, so it is the
    /// one state key a typed reader cannot route; it must still go state-first.
    #[test]
    fn the_content_hash_reads_through_the_loader() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path();
        std::fs::write(
            dir.join("config.json"),
            r#"{"tooling":{"profileContentHash":"tracked-hash"}}"#,
        )
        .expect("seed decisions");
        assert_eq!(
            read_replica_profile_content_hash(dir).as_deref(),
            Some("tracked-hash")
        );

        std::fs::write(
            dir.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
            r#"{"tooling":{"profileContentHash":"state-hash"}}"#,
        )
        .expect("seed state");
        assert_eq!(
            read_replica_profile_content_hash(dir).as_deref(),
            Some("state-hash")
        );
    }

    /// C4 (#2470): the seeded state file of every row that seeds one. All four
    /// state keys, plus the exact D7 marker `stamp_split_marker` writes.
    #[rustfmt::skip]
    const C4_SEEDED_STATE: &str =
        r#"{"tooling":{"lastCodingAgent":"codex","codingAgents":{},"lastAgentMessageAt":"2026-01-01T00:00:00Z","profileContentHash":"h"},"split":{"v":1,"keys":["lastCodingAgent","codingAgents","lastAgentMessageAt","profileContentHash"]}}"#;

    /// C4 (#2470): the state file beside a tracked `config.json`.
    fn c4_state_path(tracked: &std::path::Path) -> std::path::PathBuf {
        tracked.with_file_name(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
    }

    /// C4 (#2470): after one writer, the tracked file carries none of the four
    /// state keys. This says nothing at all about the state file: `c4_state_is`
    /// is the only helper that reads it, and the row calls them in that order.
    fn c4_tracked_is_clean(tracked: &std::path::Path, site: &str) {
        let text = std::fs::read_to_string(tracked).expect("tracked file");
        let value: serde_json::Value = serde_json::from_str(&text).expect("tracked json");
        let tooling = value.get("tooling");
        c4_absent(tooling, "lastCodingAgent", site, &value);
        c4_absent(tooling, "codingAgents", site, &value);
        c4_absent(tooling, "lastAgentMessageAt", site, &value);
        c4_absent(tooling, "profileContentHash", site, &value);
    }

    fn c4_absent(t: Option<&serde_json::Value>, key: &str, site: &str, all: &serde_json::Value) {
        assert!(
            t.and_then(|t| t.get(key)).is_none(),
            "{site} left {key}: {all}"
        );
    }

    /// C4 (#2470): the state file EXISTS and is exactly `expected`, compared
    /// as parsed JSON so the pretty-printing of `publish_pair_side` is never
    /// the subject. `expected` is always a literal written in the row. Absence
    /// is NOT admitted here and no longer reads as JSON `null`:
    /// `c4_no_state_file` is the only helper that may conclude absence.
    fn c4_state_is(state: &std::path::Path, expected: &str, site: &str) {
        let want: serde_json::Value = serde_json::from_str(expected).expect("expected json");
        let raw = std::fs::read_to_string(state).expect("state file");
        let got: serde_json::Value = serde_json::from_str(&raw).expect("state json");
        assert_eq!(got, want, "{site} state file");
    }

    /// C4 (#2470): no state file exists beside the tracked file. Absence is
    /// `NotFound` from `symlink_metadata` and nothing else: an existing entry
    /// of any kind is a failure, a JSON `null`, an empty file, a directory and
    /// a dangling symlink included, and so is every other error. Round 4's
    /// expectation `"null"` through `c4_state_is` admitted the JSON `null`, the
    /// directory and every read error, and failed an empty file by the wrong
    /// assertion, a parse panic rather than an absence check.
    fn c4_no_state_file(state: &std::path::Path, site: &str) {
        match std::fs::symlink_metadata(state) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Ok(m) => panic!(
                "{site} left a state entry, is_dir={}: {state:?}",
                m.is_dir()
            ),
            Err(e) => panic!("{site} could not observe {state:?}: {e}"),
        }
    }

    /// C4 (#2470): `c4_no_state_file` must reject `state`. The panic message of
    /// the rejected call reaches stderr and is expected output, not a failure.
    fn c4_rejects(state: &std::path::Path, case: &str) {
        let probe = state.to_path_buf();
        let outcome = std::panic::catch_unwind(move || c4_no_state_file(&probe, "control"));
        assert!(outcome.is_err(), "{case}: c4_no_state_file accepted it");
    }

    /// C4 (#2470): the four negative controls of the absence contract, on a
    /// directory of the row's own, so no site fixture is disturbed.
    fn c4_absence_controls(dir: &std::path::Path) {
        let state = c4_state_path(&dir.join("config.json"));
        c4_no_state_file(&state, "control: nothing at the path");
        for bytes in ["null", ""] {
            std::fs::write(&state, bytes).expect("plant a state file");
            c4_rejects(&state, "control: an existing state file");
        }
        std::fs::remove_file(&state).expect("remove the planted file");
        std::fs::create_dir(&state).expect("plant a state directory");
        c4_rejects(&state, "control: a state directory");
    }

    #[rustfmt::skip]
    const C4_STATE_AFTER_SITE_3A: &str =
        r#"{"tooling":{"lastCodingAgent":"codex","codingAgents":{},"lastAgentMessageAt":"2026-01-01T00:00:00Z","profileContentHash":"c4hash"},"split":{"v":1,"keys":["lastCodingAgent","codingAgents","lastAgentMessageAt","profileContentHash"]}}"#;

    /// C4 (#2470) E12, sites 3a, 3b, 8, 9, 10 and 11, each on its own fixture
    /// except 9 and 10, which need the same replica.
    #[test]
    fn c4_sites_3_8_9_10_11_leave_no_state_key_in_the_tracked_file() {
        let journal = tempfile::tempdir().unwrap();
        let _journal = crate::config::agent_config::journal_redirect::set(journal.path());
        let seed = |dir: &Path| -> PathBuf {
            write_config(dir, &seed_selection_config("{}"));
            let tracked = dir.join("config.json");
            std::fs::write(c4_state_path(&tracked), C4_SEEDED_STATE).unwrap();
            tracked
        };
        let tooling = |dir: &Path| config_value(dir)["tooling"].clone();

        let fx = selection_fixture();
        let tracked = seed(&fx.replica);
        assert_eq!(
            set_replica_profile_content_hash(&fx.replica, "c4hash"),
            Ok(())
        );
        c4_tracked_is_clean(&tracked, "site 3a");
        c4_state_is(&c4_state_path(&tracked), C4_STATE_AFTER_SITE_3A, "site 3a");
        assert_eq!(tooling(&fx.replica), serde_json::json!({}));

        let fx = selection_fixture();
        let tracked = seed(&fx.matrix);
        assert_eq!(
            set_agent_default_profile(&fx.settings, &fx.matrix, "b"),
            Ok(())
        );
        c4_tracked_is_clean(&tracked, "site 3b");
        c4_state_is(&c4_state_path(&tracked), C4_SEEDED_STATE, "site 3b");
        assert_eq!(
            tooling(&fx.matrix)["defaultProfile"],
            serde_json::json!("B")
        );

        let fx = selection_fixture();
        let default = ReplicaSelectionDefault {
            coding_agent_id: "codex".to_string(),
            requested_profile: "B".to_string(),
            selection_locked: false,
        };
        let outcome = write_replica_selection_default(&fx.matrix, &default, None);
        assert!(outcome.is_ok(), "site 8: {outcome:?}");
        c4_tracked_is_clean(&fx.matrix.join("config.json"), "site 8");
        c4_no_state_file(&c4_state_path(&fx.matrix.join("config.json")), "site 8");
        assert_eq!(
            tooling(&fx.matrix)["replicaSelectionDefault"],
            serde_json::json!({ "codingAgentId": "codex", "requestedProfile": "B", "selectionLocked": false })
        );

        let fx = selection_fixture();
        let tracked = seed(&fx.replica);
        let pair = ReplicaSelectionPair {
            coding_agent_id: "codex".to_string(),
            requested_profile: "B".to_string(),
        };
        let expected = read_replica_selection_state(&fx.replica)
            .expectation()
            .expect("a valid unlocked replica");
        assert_eq!(
            write_replica_selection(
                &fx.settings,
                &fx.replica,
                &pair,
                SelectionWriteIntent::IndividualAssignLock,
                &expected,
            ),
            Ok(SelectionWriteOutcome {
                changed: true,
                published: true,
                lock_transition: Some(SelectionLockTransition::UnlockedToLocked),
            })
        );
        c4_tracked_is_clean(&tracked, "site 9");
        c4_state_is(&c4_state_path(&tracked), C4_SEEDED_STATE, "site 9");
        assert_eq!(
            tooling(&fx.replica)["currentCodingAgent"],
            serde_json::json!("codex")
        );
        assert_eq!(tooling(&fx.replica)["profile"], serde_json::json!("B"));
        assert_eq!(
            tooling(&fx.replica)["selectionLocked"],
            serde_json::json!(true)
        );

        let expected = read_replica_selection_state(&fx.replica)
            .expectation()
            .expect("a valid locked replica");
        assert_eq!(
            clear_replica_selection_lock(&fx.settings, &fx.replica, &expected),
            Ok(SelectionWriteOutcome {
                changed: true,
                published: true,
                lock_transition: Some(SelectionLockTransition::LockedToUnlocked),
            })
        );
        c4_tracked_is_clean(&tracked, "site 10");
        c4_state_is(&c4_state_path(&tracked), C4_SEEDED_STATE, "site 10");
        assert_eq!(
            tooling(&fx.replica)["selectionLocked"],
            serde_json::json!(false)
        );

        let fx = selection_fixture();
        let tracked = seed(&fx.replica);
        assert_eq!(
            set_instance_profile_override(&fx.settings, &fx.replica, Some("c")),
            Ok(())
        );
        c4_tracked_is_clean(&tracked, "site 11");
        c4_state_is(&c4_state_path(&tracked), C4_SEEDED_STATE, "site 11");
        assert_eq!(tooling(&fx.replica)["profile"], serde_json::json!("C"));
        assert_eq!(
            tooling(&fx.replica)["instanceProfileOverride"],
            serde_json::json!("C")
        );
        assert_eq!(
            tooling(&fx.replica)["instanceProfileOverrideSource"],
            serde_json::json!("manual")
        );

        let control_dir = tempfile::tempdir().unwrap();
        c4_absence_controls(control_dir.path());
    }
}

#[cfg(test)]
mod transition_reference_tests {
    use super::*;
    use crate::config::settings::{
        IdentityReferencePhysicalState, SourceIdentityMapping, SourceRef,
    };

    fn fixture() -> (
        SourceRef,
        [IdentityReferencePhysicalState; 2],
        Vec<SourceIdentityMapping>,
    ) {
        let source: SourceRef = serde_json::from_value(serde_json::json!({
            "family": "agents", "role": "registered", "scope": "instance",
            "kind": "registeredInstance", "contextRoot": "fixture-instance"
        }))
        .unwrap();
        let reference = |cid: &str, pid: &str, key: &str| {
            serde_json::json!({
                "version": 1, "source": source, "calculatedId": cid,
                "requestedProfile": "B", "profileCalculatedId": pid, "legacyEntryHint": key
            })
        };
        let first = reference("cid1:old-a", "pid1:old-a", "a");
        let second = reference("cid1:old-b", "pid1:old-b", "b");
        let third = reference("cid1:stable-c", "pid1:old-c", "c");
        let decisions = serde_json::json!({"unknown": {"null": null}, "tooling": {
            "currentCodingAgent": "a", "profile": "B", "selectionLocked": true,
            "configurationRef": first, "replicaSelectionDefault": {
                "codingAgentId": "b", "requestedProfile": "B", "selectionLocked": true,
                "configurationRef": second
            }
        }});
        let state = serde_json::json!({"unknown": [null, "keep"], "tooling": {"codingAgents": {"c": {"configurationRef": third, "unknown": null}}, "sourceReferences": [{"configurationRef": first, "unknown": true}]}});
        let physical = [decisions, state].map(|value| {
            IdentityReferencePhysicalState::from_bytes(serde_json::to_vec(&value).unwrap())
        });
        let mapping = |key: &str, before: &str, after: &str| SourceIdentityMapping {
            entry_hint: key.into(),
            old_calculated_id: Some(before.into()),
            new_calculated_id: Some(after.into()),
            unavailable: false,
            profiles: std::collections::BTreeMap::from([(
                "B".into(),
                (
                    Some(format!("pid1:old-{key}")),
                    Some(format!("pid1:new-{key}")),
                ),
            )]),
        };
        (
            source,
            physical,
            vec![
                mapping("a", "cid1:old-a", "cid1:new-a"),
                mapping("b", "cid1:old-b", "cid1:new-b"),
                mapping("c", "cid1:stable-c", "cid1:stable-c"),
            ],
        )
    }

    #[test]
    fn transition_reference_batch_two_cid_pid_only_default_lock_history_unknown() {
        let (source, before, mappings) = fixture();
        let patch = plan_identity_reference_patch(before.clone(), &source, &mappings).unwrap();
        assert_eq!(patch.before, before);
        assert_eq!(
            patch.after[0].pointer("/tooling/configurationRef/calculatedId"),
            Some(&serde_json::json!("cid1:new-a"))
        );
        assert_eq!(
            patch.after[0]
                .pointer("/tooling/replicaSelectionDefault/configurationRef/calculatedId"),
            Some(&serde_json::json!("cid1:new-b"))
        );
        assert_eq!(
            patch.after[1].pointer("/tooling/codingAgents/c/configurationRef/profileCalculatedId"),
            Some(&serde_json::json!("pid1:new-c"))
        );
        assert_eq!(
            patch.after[1].pointer("/tooling/sourceReferences/0/configurationRef/calculatedId"),
            Some(&serde_json::json!("cid1:new-a"))
        );
        let mut expected = [
            identity_reference_value(&before[0]).unwrap(),
            identity_reference_value(&before[1]).unwrap(),
        ];
        for (side, pointer, cid, pid) in [
            (0, "/tooling/configurationRef", "cid1:new-a", "pid1:new-a"),
            (
                0,
                "/tooling/replicaSelectionDefault/configurationRef",
                "cid1:new-b",
                "pid1:new-b",
            ),
            (
                1,
                "/tooling/codingAgents/c/configurationRef",
                "cid1:stable-c",
                "pid1:new-c",
            ),
            (
                1,
                "/tooling/sourceReferences/0/configurationRef",
                "cid1:new-a",
                "pid1:new-a",
            ),
        ] {
            let object = expected[side]
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap();
            object.insert("calculatedId".into(), serde_json::json!(cid));
            object.insert("profileCalculatedId".into(), serde_json::json!(pid));
        }
        assert_eq!(patch.after, expected);
    }

    #[test]
    fn transition_reference_disabled_removed_and_other_source_preserve_every_field() {
        let (source, before, mut mappings) = fixture();
        mappings[0].unavailable = true;
        mappings[1].new_calculated_id = None;
        mappings[2]
            .profiles
            .insert("B".into(), (Some("pid1:old-c".into()), None));
        let original = [
            identity_reference_value(&before[0]).unwrap(),
            identity_reference_value(&before[1]).unwrap(),
        ];
        assert_eq!(
            plan_identity_reference_patch(before.clone(), &source, &mappings)
                .unwrap()
                .after,
            original
        );
        let mut other = source;
        other.context_root = "other-instance".into();
        assert_eq!(
            plan_identity_reference_patch(before, &other, &mappings)
                .unwrap()
                .after,
            original
        );
    }

    #[test]
    fn transition_reference_stale_profile_conflict_leaves_input_physical_unchanged() {
        let (source, before, mut mappings) = fixture();
        let original = before.clone();
        mappings[2].profiles.insert(
            "B".into(),
            (Some("pid1:stale".into()), Some("pid1:new-c".into())),
        );
        assert!(matches!(
            plan_identity_reference_patch(before.clone(), &source, &mappings),
            Err(crate::config::settings::IdentityReferenceError::Conflict)
        ));
        assert_eq!(before, original);
    }

    #[test]
    fn transition_reference_metadata_only_and_absent_sides_remain_unchanged() {
        let (source, before, _) = fixture();
        let original = [
            identity_reference_value(&before[0]).unwrap(),
            identity_reference_value(&before[1]).unwrap(),
        ];
        assert_eq!(
            plan_identity_reference_patch(before, &source, &[])
                .unwrap()
                .after,
            original
        );
        let absent = [
            IdentityReferencePhysicalState::Absent,
            IdentityReferencePhysicalState::Absent,
        ];
        let patch = plan_identity_reference_patch(absent.clone(), &source, &[]).unwrap();
        assert_eq!(patch.before, absent);
        assert_eq!(patch.after, [serde_json::json!({}), serde_json::json!({})]);
    }
}

/// Complete semantic slots captured from the SAME original physical snapshots.
#[allow(dead_code)]
pub(crate) fn source_original_slots(
    target: &SourceSelectionTarget,
) -> Result<
    Vec<(
        crate::config::settings::SourceOriginalSlot,
        crate::config::settings::ConfigurationRef,
    )>,
    crate::config::settings::IdentityReferenceError,
> {
    use crate::config::settings::{ConfigurationRef, IdentityReferenceError, SourceOriginalSlot};
    let mut slots = Vec::new();
    let mut capture = |slot, value: &Value| -> Result<(), IdentityReferenceError> {
        if !value.is_object() {
            return Err(IdentityReferenceError::Conflict);
        }
        if let Some(reference) = value.get("configurationRef") {
            let reference: ConfigurationRef = serde_json::from_value(reference.clone())
                .map_err(|_| IdentityReferenceError::Conflict)?;
            if reference.version != 1 {
                return Err(IdentityReferenceError::Conflict);
            }
            slots.push((slot, reference));
        }
        Ok(())
    };
    let decisions = identity_reference_value(&target.physical_revision[0])?;
    let state = identity_reference_value(&target.physical_revision[1])?;
    if let Some(tooling) = decisions.get("tooling") {
        capture(SourceOriginalSlot::Decision, tooling)?;
        if let Some(default) = tooling.get("replicaSelectionDefault") {
            let slot = if target
                .root
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("_agent_"))
            {
                SourceOriginalSlot::MatrixDefault
            } else {
                SourceOriginalSlot::ReplicaDefault
            };
            capture(slot, default)?;
        }
    }
    if let Some(tooling) = state.get("tooling") {
        if !tooling.is_object() {
            return Err(IdentityReferenceError::Conflict);
        }
        if let Some(history) = tooling.get("codingAgents") {
            for (key, entry) in history
                .as_object()
                .ok_or(IdentityReferenceError::Conflict)?
            {
                capture(SourceOriginalSlot::HistoryKey(key.clone()), entry)?;
            }
        }
        if let Some(history) = tooling.get("sourceReferences") {
            for (index, entry) in history
                .as_array()
                .ok_or(IdentityReferenceError::Conflict)?
                .iter()
                .enumerate()
            {
                capture(SourceOriginalSlot::HistoryIndex(index), entry)?;
            }
        }
    }
    Ok(slots)
}

#[allow(dead_code)]
pub(crate) fn source_target_is_affected(
    target: &SourceSelectionTarget,
    source: &crate::config::settings::SourceRef,
    mappings: &[crate::config::settings::SourceIdentityMapping],
) -> Result<bool, crate::config::settings::IdentityReferenceError> {
    let patch = plan_identity_reference_patch(target.physical_revision.clone(), source, mappings)?;
    let original = [
        identity_reference_value(&target.physical_revision[0])?,
        identity_reference_value(&target.physical_revision[1])?,
    ];
    Ok(patch.after != original)
}

#[cfg(test)]
thread_local! {
    static SOURCE_DISCOVERY_CALLS: std::cell::RefCell<std::collections::BTreeMap<&'static str, usize>> = const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
    static SOURCE_DISCOVERY_FAILURE: std::cell::Cell<Option<(&'static str, usize)>> = const { std::cell::Cell::new(None) };
}

fn source_discovery_cut(_stage: &'static str) -> Result<(), std::io::Error> {
    #[cfg(test)]
    SOURCE_DISCOVERY_CALLS.with(|calls| *calls.borrow_mut().entry(_stage).or_default() += 1);
    #[cfg(test)]
    SOURCE_DISCOVERY_FAILURE.with(|failure| {
        if let Some((stage, remaining)) = failure.get() {
            if stage == _stage {
                if remaining == 1 {
                    failure.set(None);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "source inventory fail-only cut",
                    ));
                }
                failure.set(Some((stage, remaining - 1)));
            }
        }
        Ok(())
    })?;
    Ok(())
}
