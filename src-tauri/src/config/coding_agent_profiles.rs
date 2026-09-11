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

fn read_tooling_string(agent_dir: &Path, key: &str) -> Option<String> {
    read_json_object(&agent_dir.join("config.json"))?
        .get("tooling")?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

fn write_tooling_string(agent_dir: &Path, key: &str, value: Option<&str>) -> Result<(), String> {
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
    crate::config::local_config_io::update_config_json_object(&config_path, true, |obj| {
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
    // `create_session_inner` already writes this dir's `config.json`
    // unconditionally via `set_last_coding_agent` right before this call, so the
    // gate only governs whether the hash field is added, never whether a stray
    // config.json is created. The read side is ungated, so it round-trips.
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
    write_tooling_string(launch_path, "profileContentHash", Some(hash))
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
    /// A locked replica is rejected.
    Individual,
    /// Deliberate single-replica assignment plus `selectionLocked: true`.
    /// A locked replica is rejected.
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
    /// Nothing to change and nothing published (already-unlocked / skipped).
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

fn selection_state_from_value(config: &Value, identity: &str) -> ReplicaSelectionState {
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
    crate::config::local_config_io::update_config_json_object(&config_path, true, |obj| {
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

    let marker = std::cell::Cell::new(None::<SelectionGuardMarker>);
    let transition = std::cell::Cell::new(None::<SelectionLockTransition>);

    let written =
        crate::config::local_config_io::update_config_json_object(&config_path, false, |obj| {
            let (_current_pair, current_locked) =
                validate_selection_write_state(&replica_dir, expected)?;
            if current_locked {
                match intent {
                    SelectionWriteIntent::Individual
                    | SelectionWriteIntent::IndividualAssignLock => {
                        return Err(format!(
                            "replica selection at {} is locked; unlock it or use a reviewed force",
                            replica_dir.display()
                        ))
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
/// the private callback marker before any object mutation, so nothing is
/// serialized and nothing is published (see [`SelectionWriteOutcome`]). Every
/// other error — malformed state, stale expected state, lock timeout, IO —
/// propagates unchanged and can never be reported as success.
pub fn clear_replica_selection_lock(
    settings: &AppSettings,
    replica_path: &Path,
    expected: &ReplicaSelectionExpectation,
) -> Result<SelectionWriteOutcome, String> {
    let replica_dir = validated_replica_dir(settings, replica_path)?;
    let config_path = replica_dir.join("config.json");
    let marker = std::cell::Cell::new(None::<SelectionGuardMarker>);

    let written =
        crate::config::local_config_io::update_config_json_object(&config_path, false, |obj| {
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
        });

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
/// replica is rejected with the stored bytes untouched.
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
    crate::config::local_config_io::update_config_json_object(&config_path, true, |obj| {
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

    fn settings_with_project(project: &Path) -> AppSettings {
        let mut settings = settings_with_cells(&[("codex", vec!["A", "B", "C"])]);
        settings.project_paths = vec![project.to_string_lossy().to_string()];
        settings.agents = vec![AgentConfig {
            id: "codex".to_string(),
            label: "Codex".to_string(),
            command: "codex".to_string(),
            color: "#000000".to_string(),
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
        assert_eq!(saved["tooling"]["lastCodingAgent"], "claude");
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
        assert_eq!(saved["tooling"]["profileContentHash"], "deadbeefdeadbeef");
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
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root_agent.join("config.json")).unwrap())
                .unwrap();
        assert_eq!(saved["tooling"]["profileContentHash"], "cafef00dcafef00d");
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
        assert_eq!(saved["tooling"]["lastCodingAgent"], "claude");
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
        assert_eq!(saved["tooling"]["lastCodingAgent"], "claude");
        assert_eq!(saved["tooling"]["profileContentHash"], "deadbeef");
        assert_eq!(
            saved["tooling"]["codingAgents"]["claude"]["app"],
            "Claude Code"
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

        // A directory at the known temp-config path proves the already-unlocked
        // path never even attempts a temp file.
        let temp_obstruction = fixture
            .replica
            .join(format!(".config.json.{}.tmp", std::process::id()));
        std::fs::create_dir(&temp_obstruction).unwrap();
        assert_eq!(
            clear_replica_selection_lock(&fixture.settings, &fixture.replica, &expected).unwrap(),
            SelectionWriteOutcome::UNCHANGED
        );
        assert!(temp_obstruction.is_dir());
        assert_eq!(config_bytes(&fixture.replica), before);

        // Positive control: a true flag DOES publish, reaches that obstruction
        // and fails temp creation — the error is never masked as success.
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
        let error =
            clear_replica_selection_lock(&fixture.settings, &fixture.replica, &locked_expected)
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
        assert_eq!(saved["tooling"]["lastCodingAgent"], "claude");

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
        assert!(error.contains("tooling must be a JSON object"), "{error}");
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
        assert_eq!(saved["tooling"]["profileContentHash"], "deadbeef");
    }
}
