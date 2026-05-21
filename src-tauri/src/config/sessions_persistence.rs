use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::settings::WindowGeometry;
use crate::session::manager::SessionManager;
use crate::session::session::{SessionStatus, TEMP_SESSION_PREFIX};
use crate::session::profile::CodingAgentKind;

/// Minimal session data needed to restore a session on next app start.
/// No UUID, just the "recipe" to re-create it.
///
/// The optional runtime fields (id, waiting_for_input, created_at) are
/// populated during live snapshots so the CLI can read session state from the
/// file without requiring an HTTP request. They are ignored on restore.
///
/// `status` is also populated during live snapshots for CLI consumption AND is
/// now **consumed on restore** by the issue #248 startup wake policy: the
/// restore-task closure in `lib.rs` reads `status` to decide whether a
/// coordinator should be auto-woken (was awake at shutdown) or left dormant
/// (was asleep at shutdown). See `should_wake_on_restore` in `lib.rs`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedSession {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_label: Option<String>,

    /// True if the session was detached into its own window at snapshot time.
    /// Phase 3 restore re-spawns a detached window for every persisted row with
    /// `was_detached=true` (except deferred sessions — see plan §R.9). Sourced
    /// from `Session::was_detached` under Fix A — NOT from `DetachedSessionsState`.
    #[serde(default)]
    pub was_detached: bool,

    /// Last-known geometry of this session's detached window. `None` for sessions
    /// that were never detached, or detached without any drag/resize yet. Auto-GC'd
    /// when the session is destroyed (field travels with the PersistedSession row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detached_geometry: Option<WindowGeometry>,

    // ── Legacy fields — read-only, consumed by the upgrade pass in load_sessions. ──
    // `skip_serializing_if = "Option::is_none"` means snapshot_sessions never writes them
    // back, and the first save after upgrade retires them from disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch_prefix: Option<String>,

    // ── Runtime fields (populated during live snapshots; `status` consumed on
    //    restore per issue #248, the others ignored on restore) ──
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
    /// ISO 8601 creation timestamp (only present in live snapshots)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

fn sessions_path() -> Option<PathBuf> {
    super::config_dir().map(|d| d.join("sessions.json"))
}

/// Remove duplicate sessions by name AND working_directory.
/// When duplicates share the same key (name or CWD), keep the one with
/// `was_active=true`; if none (or both) are active, keep the last occurrence.
/// Note: callers are expected to filter out temp sessions before calling this.
fn deduplicate(sessions: Vec<PersistedSession>) -> Vec<PersistedSession> {
    let total = sessions.len();
    let mut name_index: HashMap<String, usize> = HashMap::new();
    let mut cwd_index: HashMap<String, usize> = HashMap::new();
    let mut result: Vec<PersistedSession> = Vec::with_capacity(total);

    for session in sessions {
        let norm_cwd = session.working_directory.replace('\\', "/").to_lowercase();

        // Check name-based duplicate
        if let Some(&idx) = name_index.get(&session.name) {
            log::warn!(
                "[sessions] Dropping duplicate session by name '{}'",
                session.name
            );
            if !result[idx].was_active || session.was_active {
                // Patch cwd_index if the CWD changed
                let old_cwd = result[idx]
                    .working_directory
                    .replace('\\', "/")
                    .to_lowercase();
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
        result.push(session);
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

/// Load sessions from disk without deduplication or temp-session filtering.
/// Used by the CLI to read the live snapshot as-is.
pub fn load_sessions_raw() -> Vec<PersistedSession> {
    let path = match sessions_path() {
        Some(p) => p,
        None => return vec![],
    };
    if !path.exists() {
        return vec![];
    }
    match std::fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => vec![],
    }
}

/// Load persisted sessions from the app config directory (see config_dir()).
/// Returns empty vec on any error (missing file, corrupt JSON, etc.)
pub fn load_sessions() -> Vec<PersistedSession> {
    let path = match sessions_path() {
        Some(p) => p,
        None => {
            log::warn!("Could not determine home directory for session restore");
            return vec![];
        }
    };

    if !path.exists() {
        return vec![];
    }

    match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<Vec<PersistedSession>>(&contents) {
            Ok(sessions) => {
                // Safety net: filter out [temp] sessions that should never survive a restart
                let temp_count = sessions
                    .iter()
                    .filter(|s| s.name.starts_with(TEMP_SESSION_PREFIX))
                    .count();
                let filtered: Vec<PersistedSession> = sessions
                    .into_iter()
                    .filter(|s| {
                        if s.name.starts_with(TEMP_SESSION_PREFIX) {
                            log::warn!(
                                "[sessions] Filtering out temp session '{}' from persistence",
                                s.name
                            );
                            false
                        } else {
                            true
                        }
                    })
                    .collect();
                if temp_count > 0 {
                    log::info!(
                        "[sessions] Removed {} temp sessions from persistence file",
                        temp_count
                    );
                }
                let mut deduped = deduplicate(filtered);

                // Legacy-schema upgrade: run AFTER deduplicate() so each row's legacy
                // payload travels with its own entry. `.take()` clears the Options and
                // `skip_serializing_if` in PersistedSession elides them on next save.
                for ps in deduped.iter_mut() {
                    if !ps.git_repos.is_empty() {
                        // Already new-schema; drop any ghost legacy values.
                        ps.git_branch_source = None;
                        ps.git_branch_prefix = None;
                        continue;
                    }
                    match (ps.git_branch_source.take(), ps.git_branch_prefix.take()) {
                        (Some(source), Some(prefix)) if prefix != "multi-repo" => {
                            log::info!(
                                "[sessions] Upgrading legacy single-repo session '{}' → git_repos[1]={{label:{}, source:{}}}",
                                ps.name, prefix, source
                            );
                            ps.git_repos.push(crate::session::session::SessionRepo {
                                label: prefix,
                                source_path: source,
                                branch: None,
                            });
                        }
                        (Some(source), None) => {
                            // Shouldn't happen in data this codebase produces, but serde(default)
                            // + hand-edited files can land here. Synthesize label from dir name.
                            let dir = source
                                .replace('\\', "/")
                                .split('/')
                                .next_back()
                                .unwrap_or("")
                                .to_string();
                            let label =
                                dir.strip_prefix("repo-").map(str::to_string).unwrap_or(dir);
                            log::warn!(
                                "[sessions] Upgrading legacy session '{}' with source but no prefix; synthesized label '{}'",
                                ps.name, label
                            );
                            ps.git_repos.push(crate::session::session::SessionRepo {
                                label,
                                source_path: source,
                                branch: None,
                            });
                        }
                        (None, Some(prefix)) if prefix == "multi-repo" => {
                            log::info!(
                                "[sessions] Legacy multi-repo session '{}' → git_repos left empty; DiscoveryBranchWatcher will backfill",
                                ps.name
                            );
                        }
                        (None, Some(other)) => {
                            log::warn!(
                                "[sessions] Legacy session '{}' has unknown prefix '{}' without source; dropping",
                                ps.name, other
                            );
                        }
                        (None, None) => {}
                        (Some(_), Some(_)) => {
                            // prefix == "multi-repo" with a source — ambiguous legacy shape.
                            log::warn!(
                                "[sessions] Legacy session '{}' had source + multi-repo prefix; leaving git_repos empty for discovery backfill",
                                ps.name
                            );
                        }
                    }
                }

                log::info!(
                    "Loaded {} persisted sessions from {:?}",
                    deduped.len(),
                    path
                );
                deduped
            }
            Err(e) => {
                log::error!("Failed to parse sessions file: {}", e);
                vec![]
            }
        },
        Err(e) => {
            log::error!("Failed to read sessions file: {}", e);
            vec![]
        }
    }
}

/// Save current sessions to the app config directory (see config_dir()).
pub fn save_sessions(sessions: &[PersistedSession]) -> Result<(), String> {
    let dir = super::config_dir().ok_or("Could not determine home directory")?;
    let path = dir.join("sessions.json");

    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create config directory: {}", e))?;

    let json = serde_json::to_string_pretty(sessions)
        .map_err(|e| format!("Failed to serialize sessions: {}", e))?;

    // Atomic write: write to .tmp then rename, so a crash mid-write
    // cannot corrupt the existing sessions.json.
    let tmp_path = dir.join("sessions.json.tmp");
    std::fs::write(&tmp_path, &json)
        .map_err(|e| format!("Failed to write temp sessions file: {}", e))?;
    std::fs::rename(&tmp_path, &path)
        .map_err(|e| format!("Failed to rename sessions file: {}", e))?;

    log::info!("Saved {} sessions to {:?}", sessions.len(), path);
    Ok(())
}

/// Snapshot current live sessions into the persisted format.
/// Strips auto-injected resume flags so they are re-evaluated on next restore.
pub async fn snapshot_sessions(mgr: &SessionManager) -> Vec<PersistedSession> {
    let sessions = mgr.list_sessions().await;
    let active_id = mgr.get_active().await.map(|id| id.to_string());

    let all: Vec<PersistedSession> = sessions
        .iter()
        .filter(|s| {
            if s.name.starts_with(TEMP_SESSION_PREFIX) {
                log::debug!(
                    "[sessions] Excluding temp session '{}' from snapshot",
                    s.name
                );
                false
            } else {
                true
            }
        })
        .map(|s| PersistedSession {
            name: s.name.clone(),
            shell: s.shell.clone(),
            shell_args: strip_auto_injected_args(&s.shell, &s.shell_args),
            working_directory: s.working_directory.clone(),
            was_active: active_id.as_deref() == Some(&s.id),
            git_repos: s.git_repos.clone(),
            is_coordinator: s.is_coordinator,
            agent_id: s.agent_id.clone(),
            agent_label: s.agent_label.clone(),
            // Fix A: read detach state directly from the Session (via SessionInfo). The
            // `DetachedSessionsState` set is NOT consulted at persist time — the Destroyed
            // handler clears the set before `RunEvent::Exit` runs the final persist.
            was_detached: s.was_detached,
            detached_geometry: s.detached_geometry.clone(),
            // Legacy fields are always None on new saves; skip_serializing_if elides them.
            git_branch_source: None,
            git_branch_prefix: None,
            // Runtime fields for CLI consumption
            id: Some(s.id.clone()),
            status: Some(s.status.clone()),
            waiting_for_input: Some(s.waiting_for_input),
            created_at: Some(s.created_at.clone()),
        })
        .collect();

    deduplicate(all)
}

/// Strip auto-injected provider args from saved shell arguments.
/// Removes Claude's `--continue` / `--append-system-prompt-file <path>` and Codex's
/// `resume --last`, which are auto-injected at session creation time (see commands/session.rs).
/// These must not be baked into the saved "recipe" — otherwise they self-perpetuate
/// across app restarts (or session restarts) even when the conditions change.
///
/// Handles two injection modes:
/// - **Direct-exec**: args are separate tokens like `["--continue", ...]` or `["resume", "--last", ...]`
/// - **cmd.exe wrapper**: tokens may be separate args (`["/C", "codex", "resume", "--last"]`)
///   or embedded in a single arg string (`["/K", "git pull && codex resume --last"]`)
pub(crate) fn strip_auto_injected_args(shell: &str, args: &[String]) -> Vec<String> {
    fn token_has_unclosed_quote(token: &str, quote: char) -> bool {
        token.chars().filter(|c| *c == quote).count() % 2 == 1
    }

    fn advance_past_quoted_value(tokens: &[String], start: usize) -> usize {
        if start >= tokens.len() {
            return start;
        }

        let mut idx = start;
        let mut in_single = false;
        let mut in_double = false;

        while idx < tokens.len() {
            let token = &tokens[idx];
            if token_has_unclosed_quote(token, '\'') {
                in_single = !in_single;
            }
            if token_has_unclosed_quote(token, '"') {
                in_double = !in_double;
            }
            idx += 1;
            if !in_single && !in_double {
                break;
            }
        }

        idx
    }

    fn strip_claude_tokens(tokens: &mut Vec<String>, start: usize) {
        // #260 — Claude's resume flag from the CodingAgentProfile. resume_tokens
        // is a 1-element const for Claude, so [0] is provably in bounds. The
        // `--append-system-prompt-file` flag below is the context-file flag,
        // NOT a resume token — intentionally kept as a literal.
        let continue_flag = CodingAgentKind::Claude.profile().resume_tokens[0];
        let mut idx = start;
        while idx < tokens.len() {
            if tokens[idx].eq_ignore_ascii_case(continue_flag) {
                tokens.remove(idx);
                continue;
            }
            if tokens[idx].eq_ignore_ascii_case("--append-system-prompt-file") {
                tokens.remove(idx);
                let end = advance_past_quoted_value(tokens, idx);
                for _ in idx..end {
                    tokens.remove(idx);
                }
                continue;
            }
            idx += 1;
        }
    }

    fn strip_codex_tokens(tokens: &mut Vec<String>, start: usize) {
        // #260 — resume tokens from the CodingAgentProfile. G6: slice-pattern
        // destructure, never index; a wrong-arity slice no-ops gracefully.
        let &[resume_subcmd, resume_flag] = CodingAgentKind::Codex.profile().resume_tokens else {
            debug_assert!(false, "Codex resume_tokens must have exactly 2 elements");
            return;
        };
        if tokens
            .get(start)
            .is_some_and(|token| token.eq_ignore_ascii_case(resume_subcmd))
            && tokens
                .get(start + 1)
                .is_some_and(|token| token.eq_ignore_ascii_case(resume_flag))
        {
            tokens.remove(start);
            tokens.remove(start);
        }
    }

    fn strip_gemini_tokens(tokens: &mut Vec<String>, start: usize) {
        // #260 — resume tokens from the CodingAgentProfile. G6: slice-pattern
        // destructure, never index. The joined `--resume=latest` variant is
        // derived from the same two tokens.
        let &[resume_flag, resume_value] = CodingAgentKind::Gemini.profile().resume_tokens else {
            debug_assert!(false, "Gemini resume_tokens must have exactly 2 elements");
            return;
        };
        let joined = format!("{}={}", resume_flag, resume_value);
        if tokens
            .get(start)
            .is_some_and(|token| token.eq_ignore_ascii_case(resume_flag))
            && tokens
                .get(start + 1)
                .is_some_and(|token| token.eq_ignore_ascii_case(resume_value))
        {
            tokens.remove(start);
            tokens.remove(start);
        } else if tokens
            .get(start)
            .is_some_and(|token| token.to_lowercase() == joined)
        {
            tokens.remove(start);
        }
    }

    // #260 — consult the single detector (session/profile.rs) instead of
    // re-deriving agent identity here. Guarantees this stripper agrees with
    // the `agent_kind` that `create_session_inner` stamped on the session.
    let (is_claude, is_codex, is_gemini) = match CodingAgentKind::detect(shell, args) {
        Some(CodingAgentKind::Claude) => (true, false, false),
        Some(CodingAgentKind::Codex) => (false, true, false),
        Some(CodingAgentKind::Gemini) => (false, false, true),
        None => return args.to_vec(),
    };

    let is_cmd = crate::commands::session::executable_basename(shell) == "cmd";

    if is_cmd {
        let mut result = args.to_vec();

        if is_claude {
            if let Some(idx) = result.iter().position(|arg| {
                crate::commands::session::executable_basename(arg).starts_with("claude")
            }) {
                strip_claude_tokens(&mut result, idx + 1);
            }
        }
        if is_codex {
            if let Some(idx) = result
                .iter()
                .position(|arg| crate::commands::session::executable_basename(arg) == "codex")
            {
                strip_codex_tokens(&mut result, idx + 1);
            }
        }
        if is_gemini {
            if let Some(idx) = result
                .iter()
                .position(|arg| crate::commands::session::executable_basename(arg) == "gemini")
            {
                strip_gemini_tokens(&mut result, idx + 1);
            }
        }

        for arg in &mut result {
            let mut tokens: Vec<String> = arg
                .split_whitespace()
                .map(|token| token.to_string())
                .collect();
            let mut changed = false;

            if is_claude {
                if let Some(idx) = tokens.iter().position(|token| {
                    crate::commands::session::executable_basename(token).starts_with("claude")
                }) {
                    let before = tokens.len();
                    strip_claude_tokens(&mut tokens, idx + 1);
                    changed |= tokens.len() != before;
                }
            }

            if is_codex {
                if let Some(idx) = tokens.iter().position(|token| {
                    crate::commands::session::executable_basename(token) == "codex"
                }) {
                    let before = tokens.len();
                    strip_codex_tokens(&mut tokens, idx + 1);
                    changed |= tokens.len() != before;
                }
            }

            if is_gemini {
                if let Some(idx) = tokens.iter().position(|token| {
                    crate::commands::session::executable_basename(token) == "gemini"
                }) {
                    let before = tokens.len();
                    strip_gemini_tokens(&mut tokens, idx + 1);
                    changed |= tokens.len() != before;
                }
            }

            if changed {
                *arg = tokens.join(" ");
            }
        }
        if is_gemini {
            if let Some(idx) = result
                .iter()
                .position(|arg| crate::commands::session::executable_basename(arg) == "gemini")
            {
                strip_gemini_tokens(&mut result, idx + 1);
            }
        }

        result
    } else {
        let mut result = Vec::with_capacity(args.len());
        let mut skip_next = false;
        for (idx, a) in args.iter().enumerate() {
            if skip_next {
                skip_next = false;
                continue;
            }
            if is_codex
                && idx == 0
                && a.eq_ignore_ascii_case("resume")
                && args
                    .get(1)
                    .is_some_and(|next| next.eq_ignore_ascii_case("--last"))
            {
                continue;
            }
            if is_codex
                && idx == 1
                && args
                    .first()
                    .is_some_and(|first| first.eq_ignore_ascii_case("resume"))
                && a.eq_ignore_ascii_case("--last")
            {
                continue;
            }

            if is_gemini && idx == 0 {
                if a.eq_ignore_ascii_case("--resume") {
                    if args
                        .get(1)
                        .is_some_and(|next| next.eq_ignore_ascii_case("latest"))
                    {
                        continue;
                    }
                } else if a.to_lowercase() == "--resume=latest" {
                    continue;
                }
            }
            if is_gemini
                && idx == 1
                && args
                    .first()
                    .is_some_and(|first| first.eq_ignore_ascii_case("--resume"))
                && a.eq_ignore_ascii_case("latest")
            {
                continue;
            }

            if is_claude && a.eq_ignore_ascii_case("--continue") {
                continue;
            }
            if is_claude && a.eq_ignore_ascii_case("--append-system-prompt-file") {
                skip_next = true; // skip the next arg (the file path)
                continue;
            }
            result.push(a.clone());
        }
        result
    }
}

/// Pure: produce a sanitized copy of a failed-recoverable PersistedSession
/// suitable for merging into a fresh snapshot. Drops runtime fields (`id`,
/// `status`, `waiting_for_input`, `created_at`) since those describe the
/// PRIOR run's state; the session is no longer live, and persisting them
/// would make `list-sessions` (which filters on `id.is_some()`) report the
/// session as alive when it is not. See §224.
pub(crate) fn sanitize_failed_recoverable(ps: &PersistedSession) -> PersistedSession {
    let mut clean = ps.clone();
    clean.id = None;
    clean.status = None;
    clean.waiting_for_input = None;
    clean.created_at = None;
    clean
}

/// Persist live sessions plus stripped recipes for entries that failed to
/// restore. Stripped recipes survive on disk only until the next
/// `persist_current_state` call (any session-lifecycle event) overwrites the
/// snapshot — so retry-on-next-startup is best-effort. §224 G5/G8.
pub async fn persist_merging_failed(mgr: &SessionManager, failed: &[PersistedSession]) {
    let mut snapshot = snapshot_sessions(mgr).await;
    // §224 — strip stale runtime fields (`id`, `status`, `waiting_for_input`,
    // `created_at`) from failed-recoverable entries. Without this, the prior
    // run's runtime fields travel into the new snapshot, and `list-sessions`
    // reports a session as alive (its `s.id.is_some()` filter passes) while
    // the in-memory `SessionManager` does not contain it. `close-session`
    // then can't find the session and rejects with "No active session found".
    // Stripping enforces the invariant: any persisted row with `id.is_some()`
    // is guaranteed to be live in SessionManager.
    //
    // NOTE: this strip preserves the recipe (working_directory, shell,
    // agent_id, was_active, was_detached, etc.) so the next-startup restore
    // can retry. However, "retry-on-next-startup" persistence is best-effort:
    // any subsequent idle/busy event after this call invokes
    // `persist_current_state`, which rebuilds the snapshot from the live
    // SessionManager ONLY and silently drops failed-recoverable entries (§224
    // G5). Proper plumb of `failed_recoverable` into SessionManager is filed
    // as a follow-up.
    snapshot.extend(failed.iter().map(sanitize_failed_recoverable));
    let snapshot = deduplicate(snapshot);
    if let Err(e) = save_sessions(&snapshot) {
        log::error!("Failed to persist sessions (with merge): {}", e);
    }
}

/// Convenience: snapshot and save in one call. Logs errors but never fails.
pub async fn persist_current_state(mgr: &SessionManager) {
    let snapshot = snapshot_sessions(mgr).await;
    if let Err(e) = save_sessions(&snapshot) {
        log::error!("Failed to persist sessions: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::{sanitize_failed_recoverable, strip_auto_injected_args, PersistedSession};

    /// §224 D.2 — the strip drops every runtime field but preserves the recipe
    /// fields needed for the next-startup restore attempt.
    #[test]
    fn sanitize_failed_recoverable_drops_runtime_fields() {
        use crate::session::session::SessionStatus;
        let ps = PersistedSession {
            name: "alice".into(),
            shell: "claude".into(),
            shell_args: vec!["--continue".into()],
            working_directory: r"C:\proj\.ac-new\wg-1-devs\__agent_alice".into(),
            was_active: false,
            git_repos: vec![],
            is_coordinator: false,
            agent_id: Some("aid-1".into()),
            agent_label: Some("Claude Code".into()),
            was_detached: false,
            detached_geometry: None,
            git_branch_source: None,
            git_branch_prefix: None,
            // Stale runtime fields from a prior run:
            id: Some("uuid-prior-run".into()),
            status: Some(SessionStatus::Idle),
            waiting_for_input: Some(true),
            created_at: Some("2026-05-15T00:00:00Z".into()),
        };

        let clean = sanitize_failed_recoverable(&ps);

        // Runtime fields cleared:
        assert!(clean.id.is_none(), "id must be cleared");
        assert!(clean.status.is_none(), "status must be cleared");
        assert!(
            clean.waiting_for_input.is_none(),
            "waiting_for_input must be cleared"
        );
        assert!(clean.created_at.is_none(), "created_at must be cleared");

        // Recipe fields preserved (so next-run restore can retry):
        assert_eq!(clean.name, "alice");
        assert_eq!(clean.shell, "claude");
        assert_eq!(clean.shell_args, vec!["--continue".to_string()]);
        assert_eq!(clean.working_directory, ps.working_directory);
        assert_eq!(clean.agent_id.as_deref(), Some("aid-1"));
        assert_eq!(clean.agent_label.as_deref(), Some("Claude Code"));
        assert!(!clean.was_active);
        assert!(!clean.was_detached);
    }

    /// §224 D.2 — idempotence: stripping an entry that already has None
    /// runtime fields is a no-op (does not flip recipe fields).
    #[test]
    fn sanitize_failed_recoverable_is_idempotent() {
        let ps = PersistedSession {
            name: "bob".into(),
            shell: "cmd".into(),
            shell_args: vec![],
            working_directory: "C:/x".into(),
            was_active: false,
            git_repos: vec![],
            is_coordinator: false,
            agent_id: None,
            agent_label: None,
            was_detached: false,
            detached_geometry: None,
            git_branch_source: None,
            git_branch_prefix: None,
            id: None,
            status: None,
            waiting_for_input: None,
            created_at: None,
        };
        let once = sanitize_failed_recoverable(&ps);
        let twice = sanitize_failed_recoverable(&once);
        assert!(twice.id.is_none());
        assert!(twice.status.is_none());
        assert!(twice.waiting_for_input.is_none());
        assert!(twice.created_at.is_none());
        assert_eq!(twice.name, "bob");
    }


    #[test]
    fn strip_auto_injected_args_removes_direct_gemini_resume_latest() {
        let stripped = strip_auto_injected_args(
            "gemini",
            &[
                "--resume".to_string(),
                "latest".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
            ],
        );
        assert_eq!(stripped, vec!["-m".to_string(), "gpt-5".to_string()]);
    }

    #[test]
    fn strip_auto_injected_args_removes_cmd_gemini_resume_latest() {
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/C".to_string(),
                "gemini".to_string(),
                "--resume".to_string(),
                "latest".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec![
                "/C".to_string(),
                "gemini".to_string(),
                "-m".to_string(),
                "gpt-5".to_string()
            ]
        );
    }

    #[test]
    fn strip_auto_injected_args_removes_embedded_cmd_gemini_resume_latest() {
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/K".to_string(),
                "git pull && gemini --resume latest -m gpt-5".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec!["/K".to_string(), "git pull && gemini -m gpt-5".to_string()]
        );
    }

    #[test]
    fn strip_auto_injected_args_removes_direct_claude_continue() {
        let stripped = strip_auto_injected_args(
            "claude",
            &["--continue".to_string(), "--search".to_string()],
        );
        assert_eq!(stripped, vec!["--search".to_string()]);
    }

    #[test]
    fn strip_auto_injected_args_removes_cmd_claude_continue() {
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/C".to_string(),
                "claude".to_string(),
                "--continue".to_string(),
            ],
        );
        assert_eq!(stripped, vec!["/C".to_string(), "claude".to_string()]);
    }

    #[test]
    fn strip_auto_injected_args_removes_direct_claude_context_file() {
        let stripped = strip_auto_injected_args(
            "claude",
            &[
                "--append-system-prompt-file".to_string(),
                "C:\\temp\\ctx.md".to_string(),
                "--search".to_string(),
            ],
        );
        assert_eq!(stripped, vec!["--search".to_string()]);
    }

    #[test]
    fn strip_auto_injected_args_removes_embedded_claude_context_file_with_spaces() {
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/K".to_string(),
                "claude --continue --append-system-prompt-file \"C:\\Program Files\\ctx.md\" --search".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec!["/K".to_string(), "claude --search".to_string(),]
        );
    }

    #[test]
    fn strip_auto_injected_args_removes_direct_codex_resume_last() {
        let stripped = strip_auto_injected_args(
            "codex",
            &[
                "resume".to_string(),
                "--last".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
            ],
        );
        assert_eq!(stripped, vec!["-m".to_string(), "gpt-5".to_string()]);
    }

    #[test]
    fn strip_auto_injected_args_removes_cmd_codex_resume_last() {
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/C".to_string(),
                "codex".to_string(),
                "resume".to_string(),
                "--last".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec![
                "/C".to_string(),
                "codex".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
            ]
        );
    }

    #[test]
    fn strip_auto_injected_args_removes_embedded_cmd_codex_resume_last() {
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/K".to_string(),
                "git pull && codex resume --last -m gpt-5".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec!["/K".to_string(), "git pull && codex -m gpt-5".to_string(),]
        );
    }

    #[test]
    fn strip_auto_injected_args_leaves_unrelated_commands_unchanged() {
        let args = vec!["-NoLogo".to_string()];
        assert_eq!(strip_auto_injected_args("powershell.exe", &args), args);
    }

    // ── Issue #186 — wrapper-basename Claude detection in the stripper ──

    #[test]
    fn strip_auto_injected_args_strips_continue_for_wrapper_basename() {
        // claude-mb invoked directly: `--continue` must be stripped from the
        // saved recipe even though the executable's stem is "claude-mb".
        let stripped = strip_auto_injected_args(
            "claude-mb",
            &[
                "--dangerously-skip-permissions".to_string(),
                "--effort".to_string(),
                "max".to_string(),
                "--continue".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec![
                "--dangerously-skip-permissions".to_string(),
                "--effort".to_string(),
                "max".to_string(),
            ]
        );
    }

    #[test]
    fn strip_auto_injected_args_strips_continue_for_cmd_wrapped_basename() {
        // cmd.exe /K claude-mb ... --continue → strip --continue.
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/K".to_string(),
                "claude-mb".to_string(),
                "--effort".to_string(),
                "max".to_string(),
                "--continue".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec![
                "/K".to_string(),
                "claude-mb".to_string(),
                "--effort".to_string(),
                "max".to_string(),
            ]
        );
    }

    #[test]
    fn strip_auto_injected_args_strips_continue_for_embedded_cmd_wrapped_basename() {
        // cmd.exe /K "claude-mb --effort max --continue" → strip --continue.
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/K".to_string(),
                "claude-mb --effort max --continue".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec!["/K".to_string(), "claude-mb --effort max".to_string(),]
        );
    }

    /// Validation #17: single-repo legacy → one SessionRepo; legacy fields cleared.
    #[test]
    fn legacy_migration_single_repo_shape() {
        let mut ps = PersistedSession {
            name: "sess-a".into(),
            shell: "cmd".into(),
            shell_args: vec![],
            working_directory: "C:/x".into(),
            was_active: false,
            git_repos: vec![],
            is_coordinator: false,
            agent_id: None,
            agent_label: None,
            was_detached: false,
            detached_geometry: None,
            git_branch_source: Some("C:/repos/agentscommander".into()),
            git_branch_prefix: Some("agentscommander".into()),
            id: None,
            status: None,
            waiting_for_input: None,
            created_at: None,
        };

        // Mimic the upgrade pass in load_sessions (single-repo branch).
        if ps.git_repos.is_empty() {
            match (ps.git_branch_source.take(), ps.git_branch_prefix.take()) {
                (Some(source), Some(prefix)) if prefix != "multi-repo" => {
                    ps.git_repos.push(crate::session::session::SessionRepo {
                        label: prefix,
                        source_path: source,
                        branch: None,
                    });
                }
                _ => {}
            }
        }

        assert_eq!(ps.git_repos.len(), 1);
        assert_eq!(ps.git_repos[0].label, "agentscommander");
        assert_eq!(ps.git_repos[0].source_path, "C:/repos/agentscommander");
        assert!(ps.git_branch_source.is_none());
        assert!(ps.git_branch_prefix.is_none());
    }

    /// Issue #248 — status round-trips through serialize/deserialize.
    /// Locks the field against future "ignored on restore" misreads now that
    /// `should_wake_on_restore` in `lib.rs` consumes it.
    #[test]
    fn issue_248_status_round_trips_through_persistence() {
        use crate::session::session::SessionStatus;
        let cases = [SessionStatus::Exited(0), SessionStatus::Running];
        for status in cases {
            let ps = PersistedSession {
                name: "coord-x".into(),
                shell: "claude".into(),
                shell_args: vec![],
                working_directory: "C:/proj/.ac-new/_agent_architect".into(),
                was_active: true,
                git_repos: vec![],
                is_coordinator: true,
                agent_id: Some("aid-arch".into()),
                agent_label: Some("Architect".into()),
                was_detached: false,
                detached_geometry: None,
                git_branch_source: None,
                git_branch_prefix: None,
                id: Some("uuid".into()),
                status: Some(status.clone()),
                waiting_for_input: Some(false),
                created_at: Some("2026-05-17T00:00:00Z".into()),
            };
            let json = serde_json::to_string(&ps).expect("serialize");
            let back: PersistedSession = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back.status, Some(status));
        }
    }

    /// Legacy "multi-repo" prefix → git_repos stays empty; legacy fields cleared.
    #[test]
    fn legacy_migration_multi_repo_shape() {
        let mut ps = PersistedSession {
            name: "sess-multi".into(),
            shell: "cmd".into(),
            shell_args: vec![],
            working_directory: "C:/x".into(),
            was_active: false,
            git_repos: vec![],
            is_coordinator: false,
            agent_id: None,
            agent_label: None,
            was_detached: false,
            detached_geometry: None,
            git_branch_source: None,
            git_branch_prefix: Some("multi-repo".into()),
            id: None,
            status: None,
            waiting_for_input: None,
            created_at: None,
        };

        if ps.git_repos.is_empty() {
            match (ps.git_branch_source.take(), ps.git_branch_prefix.take()) {
                (Some(source), Some(prefix)) if prefix != "multi-repo" => {
                    ps.git_repos.push(crate::session::session::SessionRepo {
                        label: prefix,
                        source_path: source,
                        branch: None,
                    });
                }
                _ => {}
            }
        }

        assert!(ps.git_repos.is_empty());
        assert!(ps.git_branch_source.is_none());
        assert!(ps.git_branch_prefix.is_none());
    }
}
