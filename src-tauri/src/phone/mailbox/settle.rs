use super::handoff::{
    next_sustained_idle_state, ForgottenSummary, SettleAction, ARCHIVE_TIMESTAMP_FORMAT,
    SELF_FORGET_SUMMARY_READ_LIMIT_BYTES, SELF_HANDOFF_ROOT_NAME,
};
use crate::config::settings::AppSettings;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
use crate::session::session::SessionStatus;
use std::io::Read;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::Manager;
use uuid::Uuid;

/// (#1001 PR2 / B, grinch P1) Pure per-tick decision for the LIVE-inject settle,
/// sourced ENTIRELY from the real-time `activity_age` snapshot - never the lagged
/// `SessionManager.waiting_for_input`, which the watcher flips idle up to ~500ms
/// after `activity_age` crosses `idle_threshold` (watcher granularity + on_idle +
/// spawned `mark_idle`). Reading `waiting_for_input` in the loop re-opened the
/// fresh-idle drop during `activity_age ∈ [idle_threshold, T_flip]` (grinch P1);
/// `activity_age` is stamped synchronously (`idle_detector.rs`), so it has no lag
/// in either direction.
///
/// - `elapsed >= max_wait`: InjectNow (cap first - never drop a delivery).
/// - recent resize (`last_resize_age < resize_grace`): `activity_age` is FROZEN
///   and untrustworthy and the TUI is repainting - Wait (never inject a repaint;
///   grinch G8). See `PurgeReadiness.last_resize_age` doc.
/// - `activity_age` unknown (untracked / just destroyed): InjectNow best-effort;
///   the inject then surfaces the real state.
/// - `activity_age < idle_threshold`: busy / mid-turn - InjectNow (bias-to-deliver).
/// - `activity_age >= settle` (`idle_threshold + FRESH_IDLE_GUARD`): long-idle -
///   InjectNow (no added latency for a genuinely-ready wake).
/// - otherwise (the `[idle_threshold, settle)` fresh-idle window): Wait.
pub(crate) fn live_settle_action(
    activity_age: Option<std::time::Duration>,
    last_resize_age: Option<std::time::Duration>,
    resize_grace: std::time::Duration,
    idle_threshold: std::time::Duration,
    settle: std::time::Duration,
    elapsed: std::time::Duration,
    max_wait: std::time::Duration,
) -> SettleAction {
    if elapsed >= max_wait {
        return SettleAction::InjectNow;
    }
    if let Some(resize_age) = last_resize_age {
        if resize_age < resize_grace {
            return SettleAction::Wait;
        }
    }
    match activity_age {
        None => SettleAction::InjectNow,
        Some(age) if age < idle_threshold => SettleAction::InjectNow,
        Some(age) if age >= settle => SettleAction::InjectNow,
        Some(_) => SettleAction::Wait,
    }
}

/// (#1001 PR2 P2 / B, option-a) How long after PTY spawn a live wake candidate is
/// still treated as "starting". Below this, a session emitting output cannot be
/// told apart from a mid-turn one by `activity_age` alone (both are
/// `< idle_threshold`), so the busy fast-path would inject into a not-paste-ready
/// startup TUI and drop (still-starting 83%, P2); such a candidate is routed to
/// the sustained-idle settle instead. At or above it, the session is established
/// and the `activity_age` busy fast-path is trusted (bias-to-deliver).
///
/// Derived from evidence, NOT guessed: the `startup_probe` harness mode measured a
/// cold Claude's `alive_age` at first sustained paste-ready on Windows/ConPTY at
/// max 13.7s first-ready / 15.7s held-ready (n=5). 20s clears the observed max
/// held-ready with ~4s margin for boot variance. The safe direction is LARGER:
/// over-estimating only adds bounded, capped latency for a session younger than
/// this that is genuinely mid-turn (rare - a just-booted agent has no work yet),
/// while under-estimating re-opens the still-starting drop. See the `startup_probe`
/// mode in tests/wake_consumption_measure.rs.
pub(crate) const STARTUP_SETTLE_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(20);

/// (#1001 PR2 P2 / B, option-a) Route a live wake by session age: a candidate whose
/// PTY was registered less than `startup_threshold` ago is `Starting` (route to the
/// sustained-idle settle - waits out startup churn, the #611 gate PR1 measured at
/// 0% drop); an older one is `Established` (keep the `activity_age` busy fast-path).
/// Pure so the classification is unit-testable without timers.
///
/// `alive_age == None` (untracked / just destroyed) -> `Established`: best-effort,
/// consistent with `live_settle_action` mapping an unknown `activity_age` to
/// InjectNow. The boundary is exclusive: `alive_age == startup_threshold` is
/// `Established`.
pub(crate) fn live_wake_route(
    alive_age: Option<std::time::Duration>,
    startup_threshold: std::time::Duration,
) -> LiveWakeRoute {
    match alive_age {
        Some(age) if age < startup_threshold => LiveWakeRoute::Starting,
        _ => LiveWakeRoute::Established,
    }
}

/// (#1001 PR2 P2 / B, option-a) Which live-wake settle path a candidate takes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LiveWakeRoute {
    /// Recently spawned; may still be emitting startup output. Route to the
    /// sustained-idle settle so the busy fast-path can't inject into a
    /// not-paste-ready TUI.
    Starting,
    /// Established session; a busy `activity_age` is genuinely mid-turn, so keep the
    /// busy fast-path (bias-to-deliver).
    Established,
}

/// #1388 - the readiness input for the cold-spawn wake settle.
///
/// Idle ALONE is satisfied by a child that has painted nothing: `IdleDetector`
/// seeds activity at spawn (`idle_detector.rs:128-145`) and the watcher flips a
/// silent session idle after `idle_threshold`, so a stalled start satisfies an
/// idle-only gate FASTER than a healthy one. Requiring a rendered cell as well
/// closes that. Both conditions are re-evaluated every tick and fed to
/// `next_sustained_idle_state`, which resets the clock when this returns false,
/// so the settle window measures idle-AND-rendered held continuously - not two
/// conditions that were each true at some point.
pub(crate) fn wake_settle_ready(
    waiting_for_input: bool,
    has_rendered_visible_content: bool,
) -> bool {
    waiting_for_input && has_rendered_visible_content
}

/// (#1001 PR2 / B) Pure per-tick decision for the COLD-SPAWN settle loop, so the
/// sustained-idle + inject-anyway-cap policy is unit-testable without timers. The
/// cold-spawn path gates on the composed `wake_settle_ready` (#1388: idle AND
/// rendered, not `SessionManager.waiting_for_input` alone - a freshly spawned
/// session has no meaningful `activity_age` during startup churn); the live path
/// uses `live_settle_action` on real-time `activity_age` instead (grinch P1).
pub(crate) fn settle_tick(
    ready: bool,
    idle_since: Option<std::time::Instant>,
    now: std::time::Instant,
    settle: std::time::Duration,
    elapsed: std::time::Duration,
    max_wait: std::time::Duration,
) -> (Option<std::time::Instant>, SettleAction) {
    let (next_idle_since, settled) = next_sustained_idle_state(ready, idle_since, now, settle);
    if settled {
        return (next_idle_since, SettleAction::InjectNow);
    }
    // Cap: never drop a delivery - inject anyway once max_wait elapses.
    if elapsed >= max_wait {
        return (next_idle_since, SettleAction::InjectNow);
    }
    (next_idle_since, SettleAction::Wait)
}

/// #626 - which leg of the self-handoff-and-clear gate we are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelfClearPhase {
    /// Waiting for sustained idle to inject provider-resolved logical-clear text.
    Clear,
    /// Logical clear already injected; waiting for a fresh sustained idle
    /// window after clear before injecting the
    /// stand-alone handoff prompt.
    Handoff,
}

/// #626 - gate state threaded by the driver across polls. Pure-function in/out, so the whole
/// two-stage policy (settle, reset, phase transition, per-phase cap, destroyed) is unit-testable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SelfClearGateState {
    pub phase: SelfClearPhase,
    /// Instant the session was first seen continuously idle in the CURRENT phase, or None.
    pub idle_since: Option<std::time::Instant>,
    /// Start of the CURRENT phase, for the per-phase MAX_DEFER cap.
    pub phase_started: std::time::Instant,
}

impl SelfClearGateState {
    /// Initial state: Phase 1 (Clear), no idle observed yet, phase clock starts at `now`.
    pub(crate) fn new(now: std::time::Instant) -> Self {
        Self {
            phase: SelfClearPhase::Clear,
            idle_since: None,
            phase_started: now,
        }
    }
}

/// #626 - the action the driver must take after one decider step.
#[derive(Debug, PartialEq)]
pub(crate) enum SelfClearGateAction {
    /// Keep polling; the driver adopts the returned state and carries it forward.
    Wait,
    /// Phase 1 settle: inject provider-resolved logical-clear text. The returned state is already
    /// advanced to Phase 2 with the idle clock reset, so the driver just injects and keeps looping.
    InjectClear,
    /// Phase 2 settle: inject the handoff prompt, then stop.
    InjectHandoff,
    /// Stop without injecting (session gone or per-phase cap reached); &str is the log reason.
    Abandon(&'static str),
}

/// (#756) Fresh-boundary events surfaced by the self-clear / self-switch
/// drivers through an injected callback, so the generic (test-injectable)
/// drivers stay free of app state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelfClearBoundary {
    /// Phase-1 logical clear (/clear or Pi /new) reached the PTY: stamp the durable fresh intent
    /// (C2).
    Cleared,
    /// Phase-2 handoff prompt reached the PTY: post-boundary content, drop it.
    ContentInjected,
}

/// #626 - pure two-stage gate step. Returns (next_state, action). `session_present == false` means
/// the session vanished between polls. The Clear->Handoff transition RESETS `idle_since` to None
/// and
/// `phase_started` to `now`, so the Phase 2 window can NOT be satisfied by pre-clear idle. Same
/// "pure decision, thin timer loop" pattern as #617's `self_clear_gate_step` (replaced here),
/// reusing the #611 `next_sustained_idle_state` settle/reset core. Unit-testable WITHOUT timers,
/// locks, or PTY.
pub(crate) fn self_clear_gate_advance(
    state: SelfClearGateState,
    session_present: bool,
    waiting_for_input: bool,
    now: std::time::Instant,
    settle: std::time::Duration,
    max_defer: std::time::Duration,
) -> (SelfClearGateState, SelfClearGateAction) {
    if !session_present {
        return (
            state,
            SelfClearGateAction::Abandon("session destroyed before sustained idle"),
        );
    }
    // Per-phase cap so each leg independently bounds a never-idle session.
    let phase_elapsed = now
        .checked_duration_since(state.phase_started)
        .unwrap_or_default();
    if phase_elapsed >= max_defer {
        let reason = match state.phase {
            SelfClearPhase::Clear => {
                "never reached sustained idle within MAX_DEFER cap (clear leg)"
            }
            SelfClearPhase::Handoff => {
                "never reached sustained idle within MAX_DEFER cap (handoff leg)"
            }
        };
        return (state, SelfClearGateAction::Abandon(reason));
    }
    let (next_idle_since, settled) =
        next_sustained_idle_state(waiting_for_input, state.idle_since, now, settle);
    if settled {
        match state.phase {
            SelfClearPhase::Clear => {
                // Advance to Handoff and RESET both clocks: pre-clear idle does not count.
                let next = SelfClearGateState {
                    phase: SelfClearPhase::Handoff,
                    idle_since: None,
                    phase_started: now,
                };
                (next, SelfClearGateAction::InjectClear)
            }
            SelfClearPhase::Handoff => (state, SelfClearGateAction::InjectHandoff),
        }
    } else {
        (
            SelfClearGateState {
                idle_since: next_idle_since,
                ..state
            },
            SelfClearGateAction::Wait,
        )
    }
}

/// #626/#629/#636 - archive `<root>/<stem>.md` to `<root>/self-clear/<timestamp>_<stem>.md`. No-op
/// (`Ok(None)`) if `<stem>.md` is absent. Creates the `self-clear/` subdir on demand. `timestamp`
/// is supplied by the caller so this is deterministic in tests. Returns the archived path on
/// success.
/// `std::fs::rename` is atomic within the same filesystem (the agent root and its `self-clear/`
/// child share it). On Windows a source held open without FILE_SHARE_DELETE yields
/// ERROR_SHARING_VIOLATION (os error 32); the caller treats any `Err` as a non-fatal warn (no
/// clobber, the source stays). NO retries (a retry loop would block the caller). Consumers:
/// SELF-FORGET.md at queue time (#626) and SELF-HANDOFF.md immediately before the Phase-2 prompt
/// inject (#749, via `archive_handoff_for_inject`).
pub(crate) fn archive_root_md(
    root: &std::path::Path,
    stem: &str,
    timestamp: &str,
) -> std::io::Result<Option<std::path::PathBuf>> {
    let src = root.join(format!("{}.md", stem));
    if !src.is_file() {
        return Ok(None); // no-op when absent
    }
    let archive_dir = root.join("self-clear");
    let dst = archive_dir.join(format!("{}_{}.md", timestamp, stem));
    if dst.exists() {
        // Same-second collision (effectively impossible: archiving happens once per >=60s cycle).
        // Refuse to clobber an existing archive; leave the source in place.
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "archive target already exists",
        ));
    }
    std::fs::create_dir_all(&archive_dir)?;
    std::fs::rename(&src, &dst)?;
    Ok(Some(dst))
}

/// #749 - archive `<root>/SELF-HANDOFF.md` -> `self-clear/<ts>_SELF-HANDOFF.md` IMMEDIATELY before
/// the Phase-2 handoff prompt is injected, so the prompt can name the file's real, final location.
/// Replaces #629's post-inject 180s grace timer, which raced the agent's read: inject `Ok` only
/// proves bytes reached the PTY, not that the prompt was consumed, so a slow, queued, or
/// unsubmitted prompt could outlive the grace window and lose its file mid-read.
///
/// Returns `(prompt_path, archived)`: `prompt_path` is what the injected prompt must tell the
/// agent to read, relative to its root (derived from the REAL destination, never re-encoded, so
/// it cannot drift from `archive_root_md`'s naming); `archived` is `Some(dst)` only when the
/// rename actually happened, so the caller can rename it BACK if the inject then fails (a
/// re-issue needs the canonical root name, see `restore_handoff_after_failed_inject`). Fallbacks
/// keep the pre-#749 behavior: a missing source (anomalous here, the queue-time gate proved it
/// existed; warn and let the prompt's "missing or empty" clause cover it) or a rename failure
/// (e.g. Windows sharing violation, os error 32; the file is still at the root) both point the
/// prompt at the root name. `timestamp` is supplied by the caller so this is deterministic in
/// tests (mirrors `archive_root_md`); `action` labels the log lines with the owning flow.
pub(crate) fn archive_handoff_for_inject(
    root: &std::path::Path,
    timestamp: &str,
    action: &str,
) -> (String, Option<std::path::PathBuf>) {
    match archive_root_md(root, "SELF-HANDOFF", timestamp) {
        Ok(Some(dst)) => {
            log::info!(
                "[mailbox] {}: archived SELF-HANDOFF.md -> {} (pre-inject)",
                action,
                dst.display()
            );
            let prompt_path = dst.strip_prefix(root).map_or_else(
                |_| format!("self-clear/{}_SELF-HANDOFF.md", timestamp),
                |rel| {
                    rel.components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/")
                },
            );
            (prompt_path, Some(dst))
        }
        Ok(None) => {
            log::warn!(
                "[mailbox] {}: SELF-HANDOFF.md vanished from {} between the queue-time gate and \
                phase 2; the prompt will point at the root file",
                action,
                root.display()
            );
            (SELF_HANDOFF_ROOT_NAME.to_string(), None)
        }
        Err(e) => {
            log::warn!(
                "[mailbox] {}: pre-inject SELF-HANDOFF.md archive failed for {} (non-fatal; the \
                prompt will point at the root file): {}",
                action,
                root.display(),
                e
            );
            (SELF_HANDOFF_ROOT_NAME.to_string(), None)
        }
    }
}

/// #749 - best-effort undo of `archive_handoff_for_inject` after a FAILED prompt inject: the
/// prompt never reached the agent, so its notes must return to the canonical root name for a
/// re-issue (the pre-#749 code expressed the same semantics by not archiving on inject failure).
/// Refuses to clobber: if a NEW entry named `SELF-HANDOFF.md` appeared at the root in the
/// interim, the archived copy stays where it is (still recoverable under `self-clear/`). The
/// exists-then-rename pair is not atomic (Windows rename replaces an existing file), but the
/// window is milliseconds on a session that just failed an inject; accepted as best-effort.
pub(crate) fn restore_handoff_after_failed_inject(
    root: &std::path::Path,
    archived: &std::path::Path,
    action: &str,
) {
    let src = root.join(SELF_HANDOFF_ROOT_NAME);
    if src.exists() {
        log::warn!(
            "[mailbox] {}: not restoring {} after failed inject; a new SELF-HANDOFF.md already \
            exists at the root",
            action,
            archived.display()
        );
        return;
    }
    match std::fs::rename(archived, &src) {
        Ok(()) => log::info!(
            "[mailbox] {}: restored {} -> {} after failed inject",
            action,
            archived.display(),
            src.display()
        ),
        Err(e) => log::warn!(
            "[mailbox] {}: failed to restore {} to the root after failed inject (non-fatal; \
            re-issue needs the file recreated or recovered from self-clear/): {}",
            action,
            archived.display(),
            e
        ),
    }
}

/// #749 - shared Phase-2 tail for both self-handoff drivers: archive the handoff FIRST, inject a
/// prompt naming the exact archived path, and rename the file back if the inject fails. Archiving
/// before the prompt exists makes the promised location final, so a late read still succeeds (the
/// old post-inject 180s timer raced slow, queued, or unsubmitted prompts); it also keeps a
/// consumed handoff from false-triggering the NEXT cycle's existence gate (#629's concern).
/// `build_prompt` is the flow's prompt builder (`build_self_clear_handoff_prompt` /
/// `build_self_switch_handoff_prompt`); `action` labels the log lines.
pub(crate) async fn inject_handoff_prompt_with_archive<Inject, InjectFut>(
    root: &std::path::Path,
    session_id: Uuid,
    action: &'static str,
    build_prompt: fn(&str, Option<&str>) -> String,
    forgotten_summary: Option<&ForgottenSummary>,
    inject: &mut Inject,
) -> bool
where
    Inject: FnMut(Uuid, String) -> InjectFut + Send + 'static,
    InjectFut: std::future::Future<Output = Result<(), String>> + Send,
{
    let ts = chrono::Local::now()
        .format(ARCHIVE_TIMESTAMP_FORMAT)
        .to_string();
    let (handoff_path, archived) = archive_handoff_for_inject(root, &ts, action);
    let prompt = build_prompt(
        &handoff_path,
        forgotten_summary.map(ForgottenSummary::as_str),
    );
    // (#756) Returns whether the prompt reached the PTY, so the drivers fire
    // their post-boundary-content event only on a real injection.
    if let Err(e) = inject(session_id, prompt).await {
        log::warn!(
            "[mailbox] {}: handoff prompt injection failed for session {}: {}",
            action,
            session_id,
            e
        );
        if let Some(archived) = archived.as_deref() {
            restore_handoff_after_failed_inject(root, archived, action);
        }
        return false;
    }
    true
}

pub(crate) fn capture_self_forget_summary(root: &std::path::Path) -> Option<ForgottenSummary> {
    let path = root.join("SELF-FORGET.md");
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::warn!(
                "[mailbox] self-handoff: failed to open SELF-FORGET.md summary from {} \
                (non-fatal): {}",
                path.display(),
                e
            );
            return None;
        }
    };

    let mut bytes = Vec::new();
    let mut bounded = file.take(SELF_FORGET_SUMMARY_READ_LIMIT_BYTES);
    if let Err(e) = bounded.read_to_end(&mut bytes) {
        log::warn!(
            "[mailbox] self-handoff: failed to read SELF-FORGET.md summary from {} (non-fatal): {}",
            path.display(),
            e
        );
        return None;
    }

    let raw = match std::str::from_utf8(&bytes) {
        Ok(raw) => raw.to_string(),
        Err(e) if e.error_len().is_none() => {
            let valid_up_to = e.valid_up_to();
            std::str::from_utf8(&bytes[..valid_up_to])
                .unwrap_or("")
                .to_string()
        }
        Err(e) => {
            log::warn!(
                "[mailbox] self-handoff: SELF-FORGET.md summary from {} is not valid UTF-8 \
                (non-fatal): {}",
                path.display(),
                e
            );
            return None;
        }
    };

    ForgottenSummary::from_raw(&raw)
}

/// Substring sniff for the `AppError::SessionNotFound` formatted message that
/// bubbles up through `pty::inject::inject_text_into_session` as a `String`.
///
/// Tight coupling: the error string format is `"Session not found: {uuid}"`
/// (see `errors.rs:5`). `inject_text_into_session` wraps it as `"PTY write
/// failed: Session not found: {uuid}"`. The substring `"Session not found:"`
/// covers both wrappings. Pinned by the
/// `err_is_pty_session_missing_matches_actual_apperror_format` test below.
///
/// If a future PR introduces typed-error plumbing through `inject_into_pty`,
/// replace this sniff with a typed match.
pub(crate) fn err_is_pty_session_missing(e: &str) -> bool {
    e.contains("Session not found:")
}

/// #1883 - pure severity classifier for a PTY injection failure. A menu-guard or
/// typing-hold deferral is a recoverable "not yet" (the dispatcher releases the
/// lease and the poller preserves the attempt and retries), so both are logged
/// at debug and never reach the Application Error sink; every other error keeps
/// the existing ERROR severity. Returns the log level plus the outcome word for
/// the line; the caller propagates the error string unchanged.
pub(crate) fn classify_injection_error(error: &str) -> (log::Level, &'static str) {
    if crate::pty::menu_guard::is_menu_guard_deferred_error(error)
        || crate::pty::menu_guard::is_typing_hold_deferred_error(error)
        || crate::api::message_store::is_response_closing_deferred(error)
    {
        (log::Level::Debug, "deferred")
    } else {
        (log::Level::Error, "FAILED")
    }
}

pub(crate) fn is_deferred_internal_delivery_error(error: &str) -> bool {
    crate::pty::menu_guard::is_menu_guard_deferred_error(error)
        || crate::pty::menu_guard::is_typing_hold_deferred_error(error)
        || crate::api::message_store::is_response_closing_deferred(error)
}

/// §224 D.3 — pure filter: session infos by exact-FQN match on
/// `working_directory`. Extracted from `find_all_sessions` so the predicate
/// can be unit-tested without a live `SessionManager` / `AppHandle`.
/// Defensive regression guard: no future change can accidentally re-introduce
/// a `was_active` / `waiting_for_input` / `status` gate without breaking
/// these tests.
pub(crate) fn filter_sessions_by_fqn<'a>(
    sessions: &'a [crate::session::session::SessionInfo],
    target: &str,
) -> Vec<&'a crate::session::session::SessionInfo> {
    sessions
        .iter()
        .filter(|s| session_cwd_matches_fqn(&s.working_directory, target))
        .collect()
}

pub(crate) fn session_cwd_matches_fqn(cwd: &str, target: &str) -> bool {
    crate::config::teams::agent_fqn_from_path(cwd) == target
}

/// #2113: case-tolerant pre-filter for internal system notices.
///
/// The target FQN is derived from the CANONICAL replica dir
/// (`InternalSystemTarget::for_context_alert`), so it carries the on-disk
/// spelling. A session CWD carries the spelling in `cfg.project_paths`, stored
/// verbatim by `SessionManager::create_session`. On a case-insensitive
/// filesystem the two can differ by case and still name the same directory,
/// which dropped the notice (#2113).
///
/// Covers a case difference in the project segment, in the agent segment, and
/// in the room segment AFTER its `room-`/`wg-` prefix. It does NOT cover a
/// case difference in the prefix letters themselves: `has_entity_prefix` is
/// case-sensitive, so `Room-1-Team` never reaches this comparison in
/// project-qualified form. That gap is out of scope; see the plan.
///
/// Deliberately over-wide: `to_lowercase` is Unicode simple lowercasing, which
/// matches neither the NTFS uppercase table nor Linux's byte comparison. That is
/// safe HERE and only here, because this function is a pre-filter, never the
/// authority. Every candidate it admits is re-checked by
/// `canonical_cwd_owned_by_replica`, which canonicalizes the CWD and requires
/// real filesystem identity with the replica dir. A false positive from this
/// function is discarded there; a false negative loses the notice.
///
/// Cross-OS risk, stated: on Linux `.../__agent_bob` and `.../__agent_Bob` are
/// two distinct real directories, and this function matches both. It does not
/// deliver to both: the canonical gate admits exactly the one whose canonical
/// path equals the target replica dir. See the `linux` test below.
pub(crate) fn internal_target_fqn_prefilter_matches(cwd: &str, target_fqn: &str) -> bool {
    let candidate = crate::config::teams::agent_fqn_from_path(cwd);
    candidate == target_fqn || candidate.to_lowercase() == target_fqn.to_lowercase()
}

pub(crate) fn resolve_wg_path_from_session_dirs(
    dirs: &[(Uuid, String)],
    agent_name: &str,
) -> Option<String> {
    let (target_project, local) = crate::config::teams::split_project_prefix(agent_name);
    let (wg_name, agent_short) = local.split_once('/')?;
    if !crate::config::entity_prefix::has_entity_prefix(wg_name) {
        return None;
    }

    let wg_marker = format!("/{}/", wg_name);
    for (_, cwd) in dirs {
        let normalized = cwd.replace('\\', "/");
        if let Some(wg_pos) = normalized.rfind(&wg_marker) {
            let wg_dir = &normalized[..wg_pos + 1 + wg_name.len()];
            let candidate = format!("{}/__agent_{}", wg_dir, agent_short);
            if !std::path::Path::new(&candidate).is_dir() {
                continue;
            }

            let candidate_fqn = crate::config::teams::agent_fqn_from_path(&candidate);
            if let Some(want) = target_project {
                let (cand_project, _) = crate::config::teams::split_project_prefix(&candidate_fqn);
                if cand_project != Some(want) {
                    continue;
                }
            }

            log::debug!(
                "[mailbox] wake: resolved WG agent path from sibling session: {}",
                candidate
            );
            return Some(
                std::path::PathBuf::from(&candidate)
                    .to_string_lossy()
                    .to_string(),
            );
        }
    }

    None
}

/// §224 A.2.5 — outcome of the daemon-restart race guard wait loop.
///
/// The production caller (`handle_close_session`) inlines the wait loop
/// because the natural probe closure captures `&self`, `app`, and `target`
/// across an `.await`, which is awkward to pass through this helper's
/// `FnMut() -> Future` shape without boxing. The helper is retained as the
/// canonical executable specification of the wait semantics for D.5a unit
/// tests — any future divergence between the inline loop and this helper is
/// a regression.
///
/// §224 G-IMPL-5 (NIT, accepted) — LOAD-BEARING COMMENT. The inlined wait
/// loop in `handle_close_session` has no direct unit test on Windows
/// (D.5b is `#[ignore]`'d pending the cross-process FS enumeration
/// investigation). Equivalence between the two implementations is
/// enforced ONLY by this comment + the helper's D.5a unit tests. Do NOT
/// change the inlined loop's semantics without updating this helper to
/// match, and vice versa.
#[allow(dead_code)]
#[derive(Debug, PartialEq)]
pub(crate) enum RestoreWaitOutcome {
    /// Deadline elapsed with the restore flag still set; caller should report
    /// `status="restore_in_progress"` to the user.
    StillInProgress,
    /// Flag cleared with the probe still returning empty — there is no live
    /// session for this FQN. Caller falls through to the `no_match` path.
    NoMatch,
}

/// §224 A.2.5 — poll `probe` for a non-empty result, OR for `flag` to clear,
/// until `deadline`. Pure async helper extracted so the race-guard logic can
/// be unit-tested without a live `SessionManager` / `AppHandle`.
///
/// Returns:
///   * `Ok(non_empty_session_ids)` — a session appeared during the wait.
///   * `Err(StillInProgress)` — deadline elapsed, flag still set.
///   * `Err(NoMatch)` — flag cleared with empty result throughout (final
///     probe also empty).
#[allow(dead_code)]
pub(crate) async fn wait_for_restore_or_session<F, Fut>(
    flag: &std::sync::atomic::AtomicBool,
    mut probe: F,
    deadline: std::time::Instant,
    poll: std::time::Duration,
) -> Result<Vec<Uuid>, RestoreWaitOutcome>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Vec<Uuid>>,
{
    use std::sync::atomic::Ordering;
    loop {
        if std::time::Instant::now() >= deadline {
            return if flag.load(Ordering::SeqCst) {
                Err(RestoreWaitOutcome::StillInProgress)
            } else {
                Err(RestoreWaitOutcome::NoMatch)
            };
        }
        tokio::time::sleep(poll).await;
        let ids = probe().await;
        if !ids.is_empty() {
            return Ok(ids);
        }
        if !flag.load(Ordering::SeqCst) {
            // Flag cleared. One final probe to catch a last-tick insertion
            // by the restore task before its guard dropped.
            let ids = probe().await;
            return if ids.is_empty() {
                Err(RestoreWaitOutcome::NoMatch)
            } else {
                Ok(ids)
            };
        }
    }
}

pub(crate) enum OutboxClassification {
    Standard(String),
    PrivilegedCandidate {
        bytes: Vec<u8>,
        identity: crate::path_identity::VerifiedPathIdentity,
    },
    InvalidDocument,
}

pub(crate) fn contains_privileged_token(bytes: &[u8]) -> bool {
    [
        b"ptyInput".as_slice(),
        b"pty-input".as_slice(),
        b"pty-input-marker".as_slice(),
    ]
    .iter()
    .any(|needle| bytes.windows(needle.len()).any(|window| window == *needle))
}

pub(crate) fn decode_loose_json_ascii_escapes(bytes: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes.get(index) == Some(&b'\\')
            && matches!(bytes.get(index + 1), Some(b'u' | b'U'))
            && index + 6 <= bytes.len()
        {
            let mut value = 0_u32;
            let mut valid = true;
            for byte in &bytes[index + 2..index + 6] {
                let Some(digit) = (*byte as char).to_digit(16) else {
                    valid = false;
                    break;
                };
                value = (value << 4) | digit;
            }
            if valid && value <= 0x7f {
                decoded.push(value as u8);
                index += 6;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    decoded
}

pub(crate) fn raw_privileged_probe(bytes: &[u8]) -> bool {
    if contains_privileged_token(bytes)
        || contains_privileged_token(&decode_loose_json_ascii_escapes(bytes))
    {
        return true;
    }

    for little_endian in [true, false] {
        let mut ascii = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.as_chunks::<2>().0 {
            let unit = if little_endian {
                u16::from_le_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_be_bytes([chunk[0], chunk[1]])
            };
            ascii.push(u8::try_from(unit).unwrap_or(b' '));
        }
        if contains_privileged_token(&ascii)
            || contains_privileged_token(&decode_loose_json_ascii_escapes(&ascii))
        {
            return true;
        }
    }
    false
}

pub(crate) fn reason_code_name(code: crate::phone::types::PtyInputReasonCode) -> &'static str {
    crate::phone::types::pty_input_reason_code_name(code)
}

pub(crate) fn reason_code_from_name(name: &str) -> crate::phone::types::PtyInputReasonCode {
    match serde_json::from_value(serde_json::Value::String(name.to_string())) {
        Ok(code) => code,
        Err(_) => crate::phone::types::PtyInputReasonCode::SenderIdentityInvalid,
    }
}

pub(crate) struct CorrelatedHostPtyMetadata {
    pub(crate) route: crate::config::teams::VerifiedPtyInputRoute,
    pub(crate) confirmation_tag: String,
    pub(crate) request_fingerprint: String,
    pub(crate) payload_sha256: String,
}

pub(crate) fn is_persistent_exited_pty_candidate(session: &SessionInfo) -> bool {
    matches!(session.status, SessionStatus::Exited(_))
        && !session
            .name
            .starts_with(crate::session::session::TEMP_SESSION_PREFIX)
}

pub(crate) fn select_persistent_exited_pty_candidate(
    matching: &[SessionInfo],
    identity_matches: impl Fn(&SessionInfo) -> bool,
) -> Option<&SessionInfo> {
    matching
        .iter()
        .filter(|session| is_persistent_exited_pty_candidate(session) && identity_matches(session))
        .max_by(|left, right| {
            let left_time = chrono::DateTime::parse_from_rfc3339(&left.created_at).ok();
            let right_time = chrono::DateTime::parse_from_rfc3339(&right.created_at).ok();
            left_time
                .cmp(&right_time)
                .then_with(|| left.id.cmp(&right.id))
        })
}

pub(crate) fn session_has_current_pty_submission_provenance(
    session: &SessionInfo,
    settings: &AppSettings,
) -> bool {
    let args = session
        .effective_shell_args
        .as_deref()
        .unwrap_or(&session.shell_args);
    if crate::session::profile::detect_pty_submission_agent(
        &session.shell,
        args,
        session.agent_kind,
    )
    .is_some()
    {
        return true;
    }
    if !session.trusted_configured_spawn {
        return false;
    }
    let Some(agent_id) = session.agent_id.as_deref() else {
        return false;
    };
    // #2716 (B3, option C) row 12: a read-only comparison, not a spawn, so it is
    // exempt from the builders' refusal. Membership is read from `settings.agents`
    // directly, as `compute_profile_outdated` does.
    if !settings.agents.iter().any(|agent| agent.id == agent_id) {
        return false;
    }
    let cwd = crate::path_utils::normalize_windows_verbatim_path(&session.working_directory);
    let Ok(spawn) = crate::config::agent_command::build_agent_spawn_command(
        settings,
        agent_id,
        Some(std::path::Path::new(&cwd)),
        session.requested_profile.as_deref(),
    ) else {
        return false;
    };
    session
        .pty_submission_agent_matches_current_spawn(&spawn)
        .is_some()
}

pub(crate) enum PtySpawnStep {
    Created(SessionInfo),
    OwnedCreated(
        SessionInfo,
        crate::session::selection::OwnedReplacementContinuation,
    ),
    Retained {
        continuation: crate::session::selection::OwnedReplacementContinuation,
        failed: bool,
    },
    RetainedUnowned,
    NeedReacquire(Option<crate::session::selection::PreparedOwnedReplacement>),
}

pub(crate) type OwnedPtyWorkTransfer = (
    crate::session::selection::OwnedReplacementContinuation,
    crate::session::selection::LifecycleEffectGuard,
);

pub(crate) async fn finish_owned_pty_work<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    work: &mut Option<OwnedPtyWorkTransfer>,
    observed: bool,
) {
    let Some((continuation, effect)) = work.take() else {
        return;
    };
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    continuation
        .finish_observed_work_transfer(&manager, &pty, effect, observed)
        .await;
}

pub(crate) async fn observe_pty_spawn_scope<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    target: &crate::config::teams::VerifiedPtyInputIdentity,
) -> (
    crate::config::sessions_persistence::CreateCarrierScope,
    Option<Uuid>,
) {
    let directory = crate::config::config_dir();
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    manager
        .with_current_carrier_snapshot(&permit, |current| {
            use crate::config::sessions_persistence::CreateCarrierScope;
            let scope = crate::config::sessions_persistence::classify_lifecycle_carrier_scope(
                &permit,
                Some(&target.canonical_fqn),
                &target.replica_identity.canonical_path,
            );
            let retained = match &scope {
                CreateCarrierScope::Protected {
                    session_id,
                    preparing_owner: None,
                    ..
                } => directory.as_ref().and_then(|directory| {
                    crate::config::sessions_persistence::retained_carrier_for_replacement(
                        &permit,
                        directory,
                        &target.canonical_fqn,
                        *session_id,
                        current,
                    )
                    .map(|_| *session_id)
                }),
                _ => None,
            };
            (scope, retained)
        })
        .await
}
