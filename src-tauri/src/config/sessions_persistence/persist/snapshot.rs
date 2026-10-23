use super::super::carrier::{all_carrier_protections, ProtectionPhase};
use super::super::record::{deduplicate, PersistedSession};
use crate::session::manager::SessionManager;
use crate::session::profile::CodingAgentKind;
use crate::session::session::{SessionInfo, TEMP_SESSION_PREFIX};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Snapshot current live sessions into the persisted format.
/// Applies provider-specific persistence cleanup to the configured recipe.
/// Pi runtime injection never enters this field, so configured Pi args are preserved.
pub async fn snapshot_sessions(mgr: &SessionManager) -> Vec<PersistedSession> {
    let aggregate = mgr.aggregate_snapshot().await;
    let Some(protections) = all_carrier_protections() else {
        // An unreadable publication registry cannot authorize a snapshot that
        // silently refills A over an already-published replacement B.
        return std::future::pending().await;
    };
    let omitted = protections
        .iter()
        .filter_map(|protection| match protection.phase {
            ProtectionPhase::Preparing {
                previous,
                replacement,
                ..
            } if protection.session_id == replacement && previous != replacement => Some(previous),
            _ => None,
        })
        .collect::<HashSet<_>>();
    snapshot_aggregate(&aggregate, &omitted)
}

pub(in super::super) fn snapshot_aggregate(
    aggregate: &crate::session::manager::ManagerAggregateSnapshot,
    omitted: &HashSet<Uuid>,
) -> Vec<PersistedSession> {
    let sessions = aggregate
        .sessions
        .iter()
        .filter(|row| !omitted.contains(&row.id))
        .map(SessionInfo::from)
        .collect::<Vec<_>>();
    let active_id = aggregate.selection.id().map(|id| id.to_string());

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
            coding_agent_binding_id: Uuid::parse_str(&s.id)
                .ok()
                .and_then(|id| aggregate.coding_agent_bindings.get(&id).cloned()),
            coding_agent_binding_seed: Uuid::parse_str(&s.id)
                .ok()
                .and_then(|id| aggregate.coding_agent_bindings.get(&id))
                .and_then(|id| aggregate.binding_seeds.get(id))
                .and_then(|seed| serde_json::from_value(seed.clone()).ok()),
            coding_agent_binding_origin: Uuid::parse_str(&s.id)
                .ok()
                .and_then(|id| aggregate.coding_agent_bindings.get(&id))
                .and_then(|id| aggregate.binding_origins.get(id).cloned()),
            coding_agent_binding_conflict: false,
            name: s.name.clone(),
            shell: s.shell.clone(),
            shell_args: strip_auto_injected_args(&s.shell, &s.shell_args),
            working_directory: s.working_directory.clone(),
            was_active: active_id.as_deref() == Some(&s.id),
            git_repos: s.git_repos.clone(),
            is_coordinator: s.is_coordinator,
            is_root_agent: s.is_root_agent,
            agent_id: s.agent_id.clone(),
            agent_label: s.agent_label.clone(),
            requested_profile: s.requested_profile.clone(),
            effective_profile: s.effective_profile.clone(),
            telegram_bot_id: s.telegram_bot_id.clone(),
            // Fix A: read detach state directly from the Session (via SessionInfo). The
            // `DetachedSessionsState` set is NOT consulted at persist time — the Destroyed
            // handler clears the set before `RunEvent::Exit` runs the final persist.
            was_detached: s.was_detached,
            last_prompt: s.last_prompt.clone(),
            detached_geometry: s.detached_geometry.clone(),
            start_fresh_on_restore: s.start_fresh_on_restore,
            // Legacy fields are always None on new saves; skip_serializing_if elides them.
            git_branch_source: None,
            git_branch_prefix: None,
            // Runtime fields for CLI consumption
            id: Some(s.id.clone()),
            status: Some(s.status.clone()),
            waiting_for_input: Some(s.waiting_for_input),
            communication: s.communication.clone(),
            created_at: Some(s.created_at.clone()),
            context_percent: s.context_percent,
        })
        .collect();

    deduplicate_preserving_affected(
        all,
        &aggregate.failed_binding_recipes,
        &aggregate.binding_origins,
    )
}

pub(in super::super) fn deduplicate_preserving_affected(
    sessions: Vec<PersistedSession>,
    affected: &HashMap<String, serde_json::Value>,
    origins: &HashMap<String, serde_json::Value>,
) -> Vec<PersistedSession> {
    let mut result = deduplicate(sessions);
    for (binding, recipe) in affected {
        if result
            .iter()
            .any(|row| row.coding_agent_binding_id.as_ref() == Some(binding))
        {
            continue;
        }
        match serde_json::from_value::<PersistedSession>(recipe.clone()) {
            Ok(mut recipe) => {
                recipe.coding_agent_binding_origin = origins.get(binding).cloned();
                result.push(sanitize_failed_recoverable(&recipe));
            }
            Err(error) => log::error!(
                "[sessions] invalid affected binding recipe {}: {}",
                binding,
                error
            ),
        }
    }
    result
}

/// Strip AC-managed provider args from saved shell arguments.
/// Current launch-time injections are Claude's `--continue`, Codex's
/// `resume --last`, Antigravity's `--continue`, and Pi's `--continue`.
/// The first three are stripped so they cannot self-perpetuate across restarts.
/// Pi is preserved because this boundary receives the configured recipe, where
/// a `--continue` token is user-authored and has no injection provenance.
///
/// Handles two injection modes:
/// - **Direct-exec**: args are separate tokens like `["--continue", ...]` or
///   `["resume", "--last", ...]`
/// - **cmd.exe wrapper**: tokens may be separate args (`["/C", "codex", "resume", "--last"]`)
///   or embedded in a single arg string (`["/K", "git pull && codex resume --last"]`)
pub(crate) fn strip_auto_injected_args(shell: &str, args: &[String]) -> Vec<String> {
    fn strip_claude_tokens(tokens: &mut Vec<String>, start: usize) {
        // #260: Claude's resume flag from the CodingAgentProfile. resume_tokens
        // is a 1-element const for Claude, so [0] is provably in bounds.
        let continue_flag = CodingAgentKind::Claude.profile().resume_tokens[0];
        let mut idx = start;
        while idx < tokens.len() {
            if tokens[idx].eq_ignore_ascii_case(continue_flag) {
                tokens.remove(idx);
                continue;
            }
            // (#756) Rider belt: strip the launcher-minted identity so it can
            // never self-perpetuate; a replayed stale --session-id hard-fails
            // the spawn (transcript collision), unlike a stale --continue.
            // UUID-gated: only the AC-injected `--session-id <v4>` shape is
            // removed; user-authored non-UUID values win.
            let lower = tokens[idx].to_lowercase();
            if lower == "--session-id"
                && tokens
                    .get(idx + 1)
                    .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
            {
                tokens.remove(idx);
                tokens.remove(idx); // paired UUID value
                continue;
            }
            if lower
                .strip_prefix("--session-id=")
                .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
            {
                tokens.remove(idx);
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

    fn strip_antigravity_tokens(tokens: &mut Vec<String>, start: usize) {
        // #260/#1482 — resume token from the CodingAgentProfile. Only the
        // AC-injected `--continue` is stripped; `-c` and `--conversation <ID>`
        // are user-authored resume forms and are preserved. resume_tokens is a
        // 1-element const for Antigravity, so [0] is provably in bounds.
        let continue_flag = CodingAgentKind::Antigravity.profile().resume_tokens[0];
        let mut idx = start;
        while idx < tokens.len() {
            if tokens[idx].eq_ignore_ascii_case(continue_flag) {
                tokens.remove(idx);
                continue;
            }
            idx += 1;
        }
    }

    // #260 — consult the single detector (session/profile.rs) instead of
    // re-deriving agent identity here. Guarantees this stripper agrees with
    // the `agent_kind` that `create_session_inner` stamped on the session.
    let (is_claude, is_codex, is_antigravity) = match CodingAgentKind::detect(shell, args) {
        // #1873 - Muse takes Pi's preserve path: `resume --last` in a persisted
        // recipe is user-authored (AC tokens live only in the effective vector).
        Some(CodingAgentKind::Pi | CodingAgentKind::Muse) => return args.to_vec(),
        Some(CodingAgentKind::Claude) => (true, false, false),
        Some(CodingAgentKind::Codex) => (false, true, false),
        Some(CodingAgentKind::Antigravity) => (false, false, true),
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
        if is_antigravity {
            if let Some(idx) = result.iter().position(|arg| {
                matches!(
                    crate::commands::session::executable_basename(arg).as_str(),
                    "agy" | "antigravity"
                )
            }) {
                strip_antigravity_tokens(&mut result, idx + 1);
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

            if is_antigravity {
                if let Some(idx) = tokens.iter().position(|token| {
                    matches!(
                        crate::commands::session::executable_basename(token).as_str(),
                        "agy" | "antigravity"
                    )
                }) {
                    let before = tokens.len();
                    strip_antigravity_tokens(&mut tokens, idx + 1);
                    changed |= tokens.len() != before;
                }
            }

            if changed {
                *arg = tokens.join(" ");
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

            if is_antigravity && a.eq_ignore_ascii_case("--continue") {
                continue;
            }
            if is_claude && a.eq_ignore_ascii_case("--continue") {
                continue;
            }
            // (#756) Rider belt, direct-exec form: strip the launcher-minted
            // `--session-id <v4>` pair / `--session-id=<v4>` joined token.
            // UUID-gated so user-authored non-UUID values are preserved.
            if is_claude {
                let lower = a.to_lowercase();
                if lower == "--session-id"
                    && args
                        .get(idx + 1)
                        .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
                {
                    skip_next = true; // consume the paired UUID value too
                    continue;
                }
                if lower
                    .strip_prefix("--session-id=")
                    .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
                {
                    continue;
                }
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
///
/// `communication` is deliberately PRESERVED (#747): a raised hand is durable
/// intent, not prior-run runtime state, so a transiently failed restore keeps
/// it for the next startup attempt. `list-sessions` still reports
/// `raisedHand: false` for these rows because `id` and `status` are stripped
/// (both the `id.is_some()` row filter and the `status.is_none()` gate exclude
/// them). Best-effort with the SAME LIFETIME AS THE RECIPE ROW: the row (hand
/// included) survives only until the next `persist_current_state` (§224 G5).
pub(crate) fn sanitize_failed_recoverable(ps: &PersistedSession) -> PersistedSession {
    let mut clean = ps.clone();
    clean.id = None;
    clean.status = None;
    clean.waiting_for_input = None;
    clean.created_at = None;
    clean
}
