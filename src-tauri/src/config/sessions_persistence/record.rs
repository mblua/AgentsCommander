use crate::config::settings::WindowGeometry;
use crate::session::session::{SessionCommunication, SessionStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;

/// Minimal session data needed to restore a session on next app start.
/// No UUID, just the "recipe" to re-create it.
///
/// The optional runtime fields (id, waiting_for_input, communication,
/// created_at) are populated during live snapshots so the CLI can read session
/// state from the file without requiring an HTTP request. They are ignored on
/// restore.
///
/// `status` is also populated during live snapshots for CLI consumption AND is
/// now **consumed on restore** by the issue #248 startup wake policy: the
/// restore-task closure in `lib.rs` reads `status` to decide whether a
/// coordinator should be auto-woken (was awake at shutdown) or left dormant
/// (was asleep at shutdown). See `should_wake_on_restore` in `lib.rs`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedSession {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coding_agent_binding_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coding_agent_binding_seed: Option<CodingAgentBindingSeed>,
    /// Original byte witness transported in RAM, never persisted.
    #[serde(skip)]
    pub coding_agent_binding_origin: Option<serde_json::Value>,
    #[serde(skip)]
    pub coding_agent_binding_conflict: bool,
    pub name: String,
    pub shell: String,
    pub shell_args: Vec<String>,
    pub working_directory: String,
    /// True for the session that was active when the app closed
    #[serde(default)]
    pub was_active: bool,
    /// Authoritative repo list. Empty = no repo badge rendered.
    #[serde(default)]
    pub git_repos: Vec<crate::session::session::SessionRepo>,
    /// Recomputed on restore; persisted for forward-compat only.
    #[serde(default)]
    pub is_coordinator: bool,
    /// True for the global Root Agent session. Defaults false for old sessions.json.
    #[serde(default)]
    pub is_root_agent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_profile: Option<String>,
    /// Effective profile letter the session actually spawned with (after the
    /// cell fallback walk). Absent for plain-shell sessions and for rows
    /// written before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_profile: Option<String>,
    /// Telegram bot id that was ON for this session at the last successful
    /// bridge attach. None means Telegram was OFF.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_bot_id: Option<String>,

    /// True if the session was detached into its own window at snapshot time.
    /// Phase 3 restore re-spawns a detached window for every persisted row with
    /// `was_detached=true` (except deferred sessions — see plan §R.9). Sourced
    /// from `Session::was_detached` under Fix A — NOT from `DetachedSessionsState`.
    #[serde(default)]
    pub was_detached: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_prompt: Option<String>,

    /// Last-known geometry of this session's detached window. `None` for sessions
    /// that were never detached, or detached without any drag/resize yet. Auto-GC'd
    /// when the session is destroyed (field travels with the PersistedSession row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detached_geometry: Option<WindowGeometry>,

    /// (#630/#631) Durable resume intent. `true` => start fresh on restore
    /// ("Restart Session"); `false` => resume. `#[serde(default)]` so pre-existing
    /// records deserialize to `false` = resume (polarity is deliberate: the safe
    /// serde/Default value must mean "resume"). No `skip_serializing_if`: always
    /// written so the on-disk record is explicit after the first save.
    #[serde(default)]
    pub start_fresh_on_restore: bool,

    // ── Legacy fields — read-only, consumed by the upgrade pass in load_sessions. ──
    // `skip_serializing_if = "Option::is_none"` means snapshot_sessions never writes them
    // back, and the first save after upgrade retires them from disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch_prefix: Option<String>,

    // ── Runtime fields (populated during live snapshots; `status` consumed on
    //    restore per issue #248 and `communication` consumed on restore per
    //    issue #747, the others ignored on restore) ──
    /// Session UUID (only present in live snapshots)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Current session status. Populated during live snapshots; **consumed on
    /// restore** by the issue #248 startup wake policy (see
    /// `should_wake_on_restore` in `lib.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SessionStatus>,
    /// Whether the session is waiting for user input (only present in live snapshots)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for_input: Option<bool>,
    /// Current visible session communication state. Populated during live
    /// snapshots; **consumed on restore** since issue #747 (a persisted raised
    /// hand re-applies to the restored record), and preserved on
    /// failed-recoverable rows by `sanitize_failed_recoverable` so the
    /// next-startup retry can restore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub communication: Option<SessionCommunication>,
    /// ISO 8601 creation timestamp (only present in live snapshots)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// Live context-usage percent (0-100), the same figure the Sidebar badge
    /// shows. Populated during live snapshots so the CLI (`list-peers` /
    /// `list-peers-lean`) can read it from disk without a running daemon;
    /// ignored on restore. `None` (absent key) means no reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_percent: Option<u8>,
}

/// Remove duplicate sessions by name AND working_directory.
/// When duplicates share the same key (name or CWD), keep the one with
/// `was_active=true`; if none (or both) are active, keep the last occurrence.
/// Note: callers are expected to filter out temp sessions before calling this.
pub(super) fn deduplicate(sessions: Vec<PersistedSession>) -> Vec<PersistedSession> {
    let total = sessions.len();
    let mut name_index: HashMap<String, usize> = HashMap::new();
    let mut cwd_index: HashMap<String, usize> = HashMap::new();
    let mut root_index: Option<usize> = None;
    let mut result: Vec<PersistedSession> = Vec::with_capacity(total);

    for session in sessions {
        let norm_cwd = normalized_cwd_key(&session.working_directory);
        let is_root_agent = session.is_root_agent
            || crate::config::root_agent::is_root_agent_path(&session.working_directory);

        if is_root_agent {
            if let Some(idx) = root_index {
                log::warn!(
                    "[sessions] Dropping duplicate root agent session '{}' at '{}'",
                    session.name,
                    session.working_directory
                );
                if !result[idx].was_active || session.was_active {
                    let old_cwd = normalized_cwd_key(&result[idx].working_directory);
                    name_index.remove(&result[idx].name);
                    cwd_index.remove(&old_cwd);
                    name_index.insert(session.name.clone(), idx);
                    cwd_index.insert(norm_cwd, idx);
                    result[idx] = session;
                    result[idx].is_root_agent = true;
                } else {
                    result[idx].is_root_agent = true;
                }
                continue;
            }
        }

        // Check name-based duplicate
        if let Some(&idx) = name_index.get(&session.name) {
            log::warn!(
                "[sessions] Dropping duplicate session by name '{}'",
                session.name
            );
            if !result[idx].was_active || session.was_active {
                // Patch cwd_index if the CWD changed
                let old_cwd = normalized_cwd_key(&result[idx].working_directory);
                if old_cwd != norm_cwd {
                    cwd_index.remove(&old_cwd);
                    cwd_index.insert(norm_cwd, idx);
                }
                result[idx] = session;
            }
            continue;
        }

        // Check CWD-based duplicate
        if let Some(&idx) = cwd_index.get(&norm_cwd) {
            log::warn!(
                "[sessions] Dropping duplicate session by CWD '{}' (existing='{}', incoming='{}')",
                session.working_directory,
                result[idx].name,
                session.name
            );
            if !result[idx].was_active || session.was_active {
                name_index.remove(&result[idx].name);
                name_index.insert(session.name.clone(), idx);
                result[idx] = session;
            }
            continue;
        }

        // New unique session
        name_index.insert(session.name.clone(), result.len());
        cwd_index.insert(norm_cwd, result.len());
        if is_root_agent {
            root_index = Some(result.len());
        }
        result.push(session);
        if is_root_agent {
            if let Some(last) = result.last_mut() {
                last.is_root_agent = true;
            }
        }
    }

    if result.len() < total {
        log::info!(
            "[sessions] Deduplicated: {} → {} sessions",
            total,
            result.len()
        );
    }

    result
}

pub(super) fn normalized_cwd_key(cwd: &str) -> String {
    crate::path_utils::normalize_windows_verbatim_path(cwd)
        .replace('\\', "/")
        .to_lowercase()
}

pub(super) fn normalize_persisted_session_paths(session: &mut PersistedSession) {
    session.working_directory =
        crate::path_utils::normalize_windows_verbatim_path(&session.working_directory);
    for repo in &mut session.git_repos {
        repo.source_path = crate::path_utils::normalize_windows_verbatim_path(&repo.source_path);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentBindingSeed {
    pub identity_revision: String,
    pub original_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_ref: Option<crate::config::settings::ConfigurationRef>,
}

impl CodingAgentBindingSeed {
    pub(super) fn validate_source(&self) -> Result<(), String> {
        let Some(reference) = &self.configuration_ref else {
            return if self.identity_revision.starts_with("s1:") {
                Err("invalid_coding_agent_binding_seed".into())
            } else {
                Ok(())
            };
        };
        let suffix = format!(":{}", reference.calculated_id);
        let digest = self
            .identity_revision
            .strip_prefix("s1:")
            .and_then(|value| value.strip_suffix(&suffix));
        let valid_digest = digest.is_some_and(|value| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        });
        let full_identity = |value: &str| {
            value.strip_prefix("cid1:").is_some_and(|value| {
                value.len() == 64
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        };
        if !valid_digest
            || reference.version != 1
            || reference.source.context_root.is_empty()
            || self.original_agent_id.is_empty()
            || reference.legacy_entry_hint.as_deref() != Some(self.original_agent_id.as_str())
            || !full_identity(&reference.calculated_id)
            || !reference
                .profile_calculated_id
                .as_deref()
                .is_some_and(full_identity)
            || reference.requested_profile.as_ref().is_some_and(|letter| {
                crate::config::settings::normalize_profile_letter(letter).as_ref() != Some(letter)
            })
        {
            return Err("invalid_coding_agent_binding_seed".into());
        }
        Ok(())
    }
}

pub(super) fn capture_binding_origins(
    path: &Path,
    bytes: &[u8],
    sessions: &mut [PersistedSession],
) {
    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(bytes));
    let mut counts = HashMap::new();
    for ps in sessions.iter_mut() {
        if ps.agent_id.is_some() {
            ps.coding_agent_binding_id
                .get_or_insert_with(|| Uuid::new_v4().to_string());
        }
        if let Some(id) = ps.coding_agent_binding_id.as_ref() {
            let key = Uuid::parse_str(id)
                .map(|id| id.to_string())
                .unwrap_or_else(|_| id.clone());
            *counts.entry(key).or_insert(0usize) += 1;
        }
    }
    for (ordinal, ps) in sessions.iter_mut().enumerate() {
        if let Some(id) = ps.coding_agent_binding_id.as_ref() {
            ps.coding_agent_binding_origin = Some(serde_json::json!({
                "sessionsPath": path.to_string_lossy(),
                "rawFileSHA256": digest,
                "originalOrdinal": ordinal,
            }));
            let key = Uuid::parse_str(id)
                .map(|id| id.to_string())
                .unwrap_or_else(|_| id.clone());
            ps.coding_agent_binding_conflict = counts.get(&key).copied().unwrap_or(0) > 1;
        }
    }
}

impl PersistedSession {
    pub(crate) fn capture_binding_seed(
        &mut self,
        settings: &crate::config::settings::AppSettings,
    ) -> Result<(), String> {
        if let Some(seed) = &self.coding_agent_binding_seed {
            seed.validate_source()?;
        }
        if self.coding_agent_binding_seed.is_none() {
            if let Some(agent_id) = self.agent_id.as_ref() {
                self.coding_agent_binding_seed = Some(CodingAgentBindingSeed {
                    identity_revision: crate::config::settings::bootstrap_binding_revision(
                        settings, agent_id,
                    )?,
                    original_agent_id: agent_id.clone(),
                    configuration_ref: None,
                });
            }
        }
        Ok(())
    }
}
