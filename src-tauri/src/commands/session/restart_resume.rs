use crate::config::agent_config;
use crate::config::settings::AppSettings;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
use crate::session::session::SessionStatus;
use std::sync::Arc;
use tauri::AppHandle;
use tauri::Manager;
use uuid::Uuid;

/// #1682 - called from the idle callback after `mark_idle`. Persists the stamp
/// for an armed session, then announces it on both transports. A no-op when a
/// control write less than `USER_WRITE_STAMP_WINDOW` old re-opened the busy
/// period this edge closed, when the user has unsubmitted input less than the
/// same window old in this session, when the session was never armed
/// or has no coding agent, when the stored value is already at or after this
/// instant, or when the write failed.
pub(crate) async fn record_agent_turn_completed<R: tauri::Runtime>(
    app: &AppHandle<R>,
    mgr: &SessionManager,
    id: Uuid,
    control_write_age: Option<std::time::Duration>,
) {
    // #1682 gate 1 - do not stamp when a PTY write we made, and that was not a
    // delivered message, is what re-opened the busy period this edge just
    // closed. `lib.rs` read the age off the idle detector at the edge instant;
    // see this phase's D1a for why it is a parameter and not a `try_state` here.
    // `None` means no mark stands, which lets the stamp proceed. The mark is a
    // near-exact answer to that question and not an exact one: phase 03's D1
    // owns the property, and its R4 and R6 are the two ways a mark can stand
    // for a write that re-opened nothing.
    if control_write_age
        .is_some_and(|age| age <= crate::pty::input_activity::USER_WRITE_STAMP_WINDOW)
    {
        return;
    }

    // #1682 gate 2 - do not stamp while the user has a RECENT half-typed,
    // unsubmitted line in this terminal. The agent CLI's echo of those
    // keystrokes is real PTY output, and `commands/pty.rs:70` marks activity
    // for the keystroke itself before `substantive` is known at `:75`, so both
    // mechanisms would otherwise advance a stamp that is meant to track this
    // agent's own output rather than the person's typing. `pending_nonspace`
    // (#871) is the repository's existing per-session answer to "is there
    // unsubmitted content"; the age bound is #1682's, and it is required
    // rather than defensive, because outside `pty::input_activity` the only
    // clear is `reset`, reached from a session destroy or restart and from a
    // fresh conversation boundary, and neither reaches an agent driven by
    // inter-agent messages: an injection's Enter never reaches the classifier
    // and the plain message payload stamps no boundary.
    //
    // Fails open when the tracker is not managed. This is the same
    // `try_state` fail-open PATTERN as `classify_substantive` in
    // `commands/pty.rs`, with the value that lets THIS function's
    // primary action proceed. The two values are opposite on purpose:
    // `classify_substantive` fails open to `true` to preserve #871's
    // clear contract, this gate to `false` so the stamp still lands.
    //
    // Bound to a `bool` in its own scope so the `std::sync::Mutex` guard is
    // dropped before any await: `clippy::await_holding_lock` does not recognise
    // a later explicit `drop`, and there must be no await while it is held.
    let typing_pending = match app.try_state::<crate::pty::input_activity::SubstantiveInputState>()
    {
        Some(state) => {
            let tracker = state.lock().unwrap_or_else(|e| e.into_inner());
            tracker.pending_within(id, crate::pty::input_activity::USER_WRITE_STAMP_WINDOW)
        }
        None => false,
    };
    if typing_pending {
        return;
    }

    // #1682 gate 3 - armed, known, and carrying a coding agent.
    let Some((cwd, _agent_id)) = mgr.agent_stamp_target(id).await else {
        return;
    };

    // #1682 gate 4 - the monotonic write reports whether it actually landed.
    let at = chrono::Utc::now().to_rfc3339();
    match agent_config::set_last_agent_message_at(&cwd, &at) {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            log::warn!("Failed to persist lastAgentMessageAt for {}: {}", id, e);
            return;
        }
    }

    crate::web::commands::broadcast_all_r(
        app,
        "session_agent_message",
        &serde_json::json!({ "sessionId": id.to_string(), "at": at }),
    );
}

/// (#1793) Settle before the first readiness poll. NOT a readiness guard: the
/// idle detector SEEDS `activity` at PTY spawn (#260), so `waiting_for_input`
/// reaches `true` for a session that has printed nothing, and waiting longer
/// makes that more likely, not less. What makes readiness mean "the agent
/// painted its prompt and then went quiet" is the second conjunct,
/// `commands::pty::session_printed_since`. This settle covers only the agent
/// that prints, pauses longer than `idle_threshold` mid-load, then prints
/// again, and it keeps the pass off a machine still spawning a fleet of PTYs.
/// Same magnitude as `auto_close::WAKE_GRACE` but NOT its rationale, which is
/// kill-eligibility, ordering against `REPAINT_GRACE`, and a spawn race.
/// The value is engineering judgement.
pub(crate) const RESTART_RESUME_SETTLE: std::time::Duration = std::time::Duration::from_secs(30);

/// (#1793) Poll cadence, matching the idle detector's own tick.
pub(crate) const RESTART_RESUME_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// (#1793) Budget after the settle. A target that never reaches its prompt in
/// this window gets NOTHING: fails closed on purpose, diverging from
/// `loops::delivery::wait_for_session_idle`, which waits 90 s and fails OPEN.
/// The direction is forced by the feature; the 300 s is engineering judgement
/// (a restored fleet replays conversations concurrently), bounded above by the
/// risk of nudging an agent the user has already picked up by hand.
pub(crate) const RESTART_RESUME_DEADLINE: std::time::Duration = std::time::Duration::from_secs(300);

/// (#1793) One agent the pass will nudge. Built inside the restore loop, where
/// the persisted row and its new session id are both in hand, so no correlation
/// key is needed. `working_directory` is for log lines only.
#[derive(Debug, Clone)]
pub(crate) struct RestartResumeTarget {
    pub session_id: Uuid,
    pub name: String,
    pub working_directory: String,
    pub prompt: String,
    /// The instant the restore loop paired this persisted row with its new
    /// session id. The PTY was already spawned by then, so
    /// `IdleDetector::register_session` had already seeded `activity[id]` and
    /// that seed stamp is at or before this instant. Readiness therefore asks
    /// for printable output STRICTLY after this instant, which the spawn seed
    /// alone can never satisfy.
    pub collected_at: std::time::Instant,
}

/// (#1793) Pure: the exact text a restored row earns at restart, or `None`.
pub(crate) fn restart_resume_prompt_for(
    settings: &AppSettings,
    is_coordinator: bool,
    start_fresh_on_restore: bool,
    persisted_working: bool,
) -> Option<&str> {
    if start_fresh_on_restore || !persisted_working {
        return None;
    }
    let prompt = if is_coordinator {
        settings.restart_resume_orchestrator_prompt.as_str()
    } else {
        settings.restart_resume_agent_prompt.as_str()
    };
    if prompt.is_empty() {
        None
    } else {
        Some(prompt)
    }
}

/// (#1793) Pure: the FIRST readiness conjunct. The session must still exist,
/// must not have exited, and must be parked at its prompt. On its own this is
/// NOT enough: `waiting_for_input` is also true for a session that has printed
/// nothing since spawn (#260 seeds the activity clock). The second conjunct is
/// applied by `partition_restart_resume_ready`.
pub(crate) fn restart_resume_session_is_ready(session: Option<&SessionInfo>) -> bool {
    match session {
        Some(session) => {
            session.waiting_for_input && !matches!(session.status, SessionStatus::Exited(_))
        }
        None => false,
    }
}

/// (#1793) Pure: split the working set into the targets that stay and the
/// targets that may be written to NOW. Both readiness conjuncts are applied
/// here. Every input target is returned in EXACTLY ONE of the two vectors, and
/// the caller assigns `remaining` back over its working set, so selection and
/// removal cannot drift apart and a target can never be offered twice.
pub(crate) fn partition_restart_resume_ready(
    targets: Vec<RestartResumeTarget>,
    live: &[SessionInfo],
    printed_since_collection: impl Fn(&RestartResumeTarget) -> bool,
) -> (Vec<RestartResumeTarget>, Vec<RestartResumeTarget>) {
    let mut remaining: Vec<RestartResumeTarget> = Vec::with_capacity(targets.len());
    let mut ready: Vec<RestartResumeTarget> = Vec::new();
    for target in targets {
        let id = target.session_id.to_string();
        let session = live.iter().find(|s| s.id == id);
        if restart_resume_session_is_ready(session) && printed_since_collection(&target) {
            ready.push(target);
        } else {
            remaining.push(target);
        }
    }
    (remaining, ready)
}

/// (#1793) Spawned once per app start from the restore tail in `lib.rs`, after
/// the idle observer is running, with the targets the restore loop collected.
/// Sequential by construction: at most one PTY write is in flight, so a fleet
/// restart cannot produce a write storm.
pub(crate) async fn run_restart_resume(app: &AppHandle, mut targets: Vec<RestartResumeTarget>) {
    if targets.is_empty() {
        return;
    }
    log::info!(
        "[restart-resume] {} target(s); settling {}s before the first readiness poll",
        targets.len(),
        RESTART_RESUME_SETTLE.as_secs()
    );
    tokio::time::sleep(RESTART_RESUME_SETTLE).await;
    let start = std::time::Instant::now();
    let mut injected: usize = 0;
    while !targets.is_empty() && start.elapsed() < RESTART_RESUME_DEADLINE {
        tokio::time::sleep(RESTART_RESUME_POLL).await;
        let live = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.list_sessions().await
        };
        let (remaining, ready) =
            partition_restart_resume_ready(std::mem::take(&mut targets), &live, |target| {
                crate::commands::pty::session_printed_since(
                    app,
                    target.session_id,
                    target.collected_at,
                )
            });
        targets = remaining;
        for target in &ready {
            inject_restart_resume_prompt(app, target).await;
            injected += 1;
        }
    }
    for target in &targets {
        log::warn!(
            "[restart-resume] '{}' ({}) never reached its prompt within {}s; nothing injected",
            target.name,
            target.working_directory,
            RESTART_RESUME_DEADLINE.as_secs()
        );
    }
    log::info!(
        "[restart-resume] complete: {} injected, {} timed out",
        injected,
        targets.len()
    );
}

/// (#1793) One injection. Root, exited, agentless and plain-shell rejection, the
/// write permit, the menu guard, the two staggered Enters and the agent-turn
/// arming all come from the canonical trusted-notice injector. This adds only
/// the exact-text validation that injector leaves to its callers.
pub(crate) async fn inject_restart_resume_prompt(app: &AppHandle, target: &RestartResumeTarget) {
    if let Err(error) = crate::pty::inject::validate_pty_input_text(&target.prompt) {
        log::warn!(
            "[restart-resume] '{}' prompt rejected by the PTY text validator ({}); nothing \
             injected",
            target.name,
            error
        );
        return;
    }
    let result = crate::pty::inject::inject_text_into_supported_agent_session_with_pre_write_check(
        app,
        target.session_id,
        &target.prompt,
        |_session| Ok(()),
    )
    .await;
    match result {
        Ok(()) => log::info!(
            "[restart-resume] injected into '{}' ({})",
            target.name,
            target.working_directory
        ),
        Err(error) => log::warn!(
            "[restart-resume] injection into '{}' failed: {}",
            target.name,
            error
        ),
    }
}
