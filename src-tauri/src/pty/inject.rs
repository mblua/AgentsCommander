use std::fmt;
use std::sync::{Arc, Mutex};

use tauri::Manager;
use uuid::Uuid;

use crate::pty::backend::PTY_INPUT_MAX_BYTES;
use crate::pty::manager::{PtyInputPermit, PtyManager, PtyRouteWriteGuard};
use crate::session::manager::SessionManager;
use crate::session::session::{Session, SessionStatus};

// Internal mailbox suspension, consumed only after the writer/lifecycle guards
// have returned. It is never a delivered outcome or public status.
pub(crate) const OWNED_WORK_SCOPE_WAIT: &str = "owned_work_scope_wait";

/// Stable, payload-free validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyInputTextErrorKind {
    Empty,
    TooLarge,
    ForbiddenScalar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtyInputTextError {
    pub kind: PtyInputTextErrorKind,
    pub byte_offset: usize,
    pub scalar_offset: usize,
    pub code_point: Option<u32>,
}

impl fmt::Display for PtyInputTextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            PtyInputTextErrorKind::Empty => formatter.write_str("invalid_text:empty"),
            PtyInputTextErrorKind::TooLarge => formatter.write_str("payload_too_large"),
            PtyInputTextErrorKind::ForbiddenScalar => write!(
                formatter,
                "invalid_text:byte={}:scalar={}:codepoint=U+{:04X}",
                self.byte_offset,
                self.scalar_offset,
                self.code_point.unwrap_or_default()
            ),
        }
    }
}

impl std::error::Error for PtyInputTextError {}

/// #1157 - the forbidden-scalar set of [`validate_pty_input_text`], extracted so
/// the injected-message sanitizer (`config::injected_messages::sanitize`) strips
/// exactly what this validator rejects. Deriving both from one predicate is what
/// keeps the two from drifting; there must be no second copy of this list.
///
/// Covers C0 except `\t` and `\n`, DEL and C1, and the whole bidi and separator
/// class (U+061C, U+200E, U+200F, U+2028, U+2029, U+202A-U+202E, U+2066-U+2069).
pub(crate) fn is_forbidden_pty_scalar(ch: char) -> bool {
    matches!(
        ch,
        '\u{0000}'..='\u{0008}'
            | '\u{000b}'
            | '\u{000c}'
            | '\u{000d}'
            | '\u{000e}'..='\u{001f}'
            | '\u{007f}'..='\u{009f}'
            | '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

/// The single authoritative in-process exact-text validator.
///
/// Accepted text is returned unchanged by callers. This function performs no
/// trimming, normalization, line-ending conversion, or wrapping.
pub fn validate_pty_input_text(text: &str) -> Result<(), PtyInputTextError> {
    if text.is_empty() {
        return Err(PtyInputTextError {
            kind: PtyInputTextErrorKind::Empty,
            byte_offset: 0,
            scalar_offset: 0,
            code_point: None,
        });
    }
    if text.len() > PTY_INPUT_MAX_BYTES {
        return Err(PtyInputTextError {
            kind: PtyInputTextErrorKind::TooLarge,
            byte_offset: PTY_INPUT_MAX_BYTES,
            scalar_offset: text
                .char_indices()
                .take_while(|(offset, _)| *offset < PTY_INPUT_MAX_BYTES)
                .count(),
            code_point: None,
        });
    }

    for (scalar_offset, (byte_offset, ch)) in text.char_indices().enumerate() {
        if is_forbidden_pty_scalar(ch) {
            return Err(PtyInputTextError {
                kind: PtyInputTextErrorKind::ForbiddenScalar,
                byte_offset,
                scalar_offset,
                code_point: Some(ch as u32),
            });
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PtyInjectionProfile {
    Established,
    Cursor,
    Pi,
    /// Exact-stem `hermes`, `opencode`, and `grok` CLIs: canonical delayed
    /// submit sequence, but no clear/compact, maintenance, or handoff
    /// capabilities.
    ExplicitSubmit,
    Unsupported,
}

fn shell_file_stem(shell: &str) -> String {
    std::path::Path::new(shell.trim())
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(shell.trim())
        .to_lowercase()
}

/// Classify a direct shell from its trimmed file stem. `claude*`/`codex*` use
/// prefix matching; `agy`/`antigravity`, `agent`, `pi`, and the exact-stem
/// `hermes`/`opencode`/`grok` use exact matching.
fn pty_injection_profile(shell: &str) -> PtyInjectionProfile {
    let stem = shell_file_stem(shell);
    if stem.starts_with("claude")
        || stem.starts_with("codex")
        || matches!(stem.as_str(), "agy" | "antigravity")
    {
        PtyInjectionProfile::Established
    } else if stem == "agent" {
        PtyInjectionProfile::Cursor
    } else if stem == "pi" {
        PtyInjectionProfile::Pi
    } else if matches!(stem.as_str(), "hermes" | "opencode" | "grok") {
        PtyInjectionProfile::ExplicitSubmit
    } else {
        PtyInjectionProfile::Unsupported
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogicalPtyCommand {
    Clear,
    Compact,
}

impl LogicalPtyCommand {
    pub(crate) fn from_wire_value(value: &str) -> Option<Self> {
        match value {
            "clear" => Some(Self::Clear),
            "compact" => Some(Self::Compact),
            _ => None,
        }
    }

    pub(crate) fn creates_fresh_boundary(self) -> bool {
        matches!(self, Self::Clear)
    }
}

/// Returns true when the direct shell command uses the canonical delayed Enter
/// sequence for pasted text blocks: Claude, Codex, Antigravity, Cursor `agent`,
/// and the exact-stem `pi`, `hermes`, `opencode`, and `grok` CLIs.
/// Classification is lexical: it uses only the trimmed shell file stem and does
/// not inspect shell arguments or wrapper contents.
pub(crate) fn needs_explicit_enter(shell: &str) -> bool {
    !matches!(
        pty_injection_profile(shell),
        PtyInjectionProfile::Unsupported
    )
}

/// Resolve a logical PTY action to provider text for a directly launched shell.
pub(crate) fn resolve_logical_command_text(
    shell: &str,
    command: LogicalPtyCommand,
) -> Option<&'static str> {
    match (pty_injection_profile(shell), command) {
        (
            PtyInjectionProfile::Established | PtyInjectionProfile::Cursor,
            LogicalPtyCommand::Clear,
        ) => Some("/clear"),
        (
            PtyInjectionProfile::Established | PtyInjectionProfile::Cursor,
            LogicalPtyCommand::Compact,
        ) => Some("/compact"),
        (PtyInjectionProfile::Pi, LogicalPtyCommand::Clear) => Some("/new"),
        _ => None,
    }
}

pub(crate) fn supports_auto_self_maintenance(shell: &str) -> bool {
    matches!(
        pty_injection_profile(shell),
        PtyInjectionProfile::Established | PtyInjectionProfile::Pi
    )
}

pub(crate) fn supports_self_handoff_switch(shell: &str) -> bool {
    matches!(
        pty_injection_profile(shell),
        PtyInjectionProfile::Established | PtyInjectionProfile::Cursor | PtyInjectionProfile::Pi
    )
}

/// #2586 D3 - the readiness decision that preceded an injection, carried only
/// so the submit-seam observation can say whether the settle gate timed out.
/// `Unknown` is for non-wake injection paths, which run no settle gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettleReadiness {
    Ready,
    TimedOut,
    Unknown,
}

/// #2586 D2 - chars in one redaction window. A row sharing any run of this many
/// consecutive chars with the payload (after whitespace collapsing) is redacted.
const REDACTION_WINDOW: usize = 8;

/// #2586 D2 - rows kept per observation: the last non-empty ones.
const SEAM_OBSERVATION_ROWS: usize = 6;

/// Trim, then collapse every whitespace run (including `\n` and `\r`) to one
/// space, so the payload compare form is a single line.
fn seam_compare_form(text: &str) -> Vec<char> {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .collect()
}

fn contains_chars(haystack: &[char], needle: &[char]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// A row leaks when any `REDACTION_WINDOW`-char window of it occurs in the
/// payload, or, for a shorter row, when the whole row is a payload substring.
/// Windows are taken over chars, never bytes, so no multi-byte char is split.
fn seam_row_leaks(row: &[char], payload: &[char]) -> bool {
    if row.len() >= REDACTION_WINDOW {
        row.windows(REDACTION_WINDOW)
            .any(|window| contains_chars(payload, window))
    } else {
        contains_chars(payload, row)
    }
}

/// Trailing whitespace and trailing box-drawing noise removed. Restates the
/// trim of `telegram::bridge::strip_trailing_decoration` locally on purpose:
/// importing it across module boundaries would add an arc for one helper.
fn strip_seam_row_decoration(row: &str) -> &str {
    row.trim_end()
        .trim_end_matches(|c: char| {
            "\u{2500}\u{2501}\u{2550}\u{2502}\u{2503}\u{250C}\u{2510}\u{2514}\u{2518}\u{251C}\
             \u{2524}\u{252C}\u{2534}\u{253C}\u{2554}\u{2557}\u{255A}\u{255D}\u{2560}\u{2563}\
             \u{2566}\u{2569}\u{256C}"
                .contains(c)
        })
        .trim_end()
}

/// #2586 D2 - render one submit-seam observation. Pure: no timer, no PTY.
///
/// Stated leak limit, not a no-leak guarantee: no run of `REDACTION_WINDOW` or
/// more consecutive payload chars (modulo whitespace collapsing) survives, and
/// no row that is itself a payload substring survives. A row sharing only a
/// shorter fragment with the payload is printed verbatim.
pub(crate) fn format_submit_seam_observation(
    instant: &str,
    settle: SettleReadiness,
    rows: Option<&[String]>,
    written_payload: &str,
) -> String {
    let Some(rows) = rows else {
        return format!("[inject] seam={instant} settle={settle:?} rows=unavailable");
    };
    let payload = seam_compare_form(written_payload);
    let mut kept: Vec<(usize, &str)> = rows
        .iter()
        .enumerate()
        .map(|(idx, row)| (idx, strip_seam_row_decoration(row)))
        .filter(|(_, row)| !row.trim().is_empty())
        .collect();
    let first = kept.len().saturating_sub(SEAM_OBSERVATION_ROWS);
    kept.drain(..first);

    let mut payload_rows = 0usize;
    let mut rendered = Vec::with_capacity(kept.len());
    for (idx, row) in kept {
        let text = row.trim();
        if seam_row_leaks(&seam_compare_form(text), &payload) {
            payload_rows += 1;
            let marker = matches!(text.chars().next(), Some('\u{276F}' | '>'));
            rendered.push(format!(
                "idx={idx}:<payload> chars={} marker={marker}",
                text.chars().count()
            ));
        } else {
            rendered.push(format!("idx={idx}:{text}"));
        }
    }
    let mut out = format!(
        "[inject] seam={instant} settle={settle:?} rows={} \
         payload_on_screen={} payload_rows={payload_rows}",
        rows.len(),
        payload_rows > 0
    );
    for row in rendered {
        out.push_str(" | ");
        out.push_str(&row);
    }
    out
}

/// #2586 D2 - one observation of the submit seam. The `PtyManager` mutex is
/// taken and released inside this synchronous fn, so no guard crosses an
/// await; a poisoned mutex answers `None`, never a panic or an error.
fn observe_submit_seam(
    pty_manager: &Arc<Mutex<PtyManager>>,
    session_id: Uuid,
    instant: &str,
    settle: SettleReadiness,
    written_payload: &str,
) {
    let rows = match pty_manager.lock() {
        Ok(pty) => pty.screen_rows_snapshot(session_id),
        Err(_) => None,
    };
    let observation =
        format_submit_seam_observation(instant, settle, rows.as_deref(), written_payload);
    log::info!("{} session={}", observation, session_id);
    #[cfg(test)]
    tests::record_seam_observation(session_id, observation);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSubmitOutcome {
    TextWriteFailed,
    RequiredEnterFailed,
    Submitted { redundant_enter_failed: bool },
}

/// Perform the synchronous, linearized first write and consume the non-Send
/// lifecycle guard before any await can occur.
pub fn write_exact_agent_input_first(route_guard: PtyRouteWriteGuard<'_>, bytes: &[u8]) -> bool {
    route_guard.write(bytes).is_ok()
}

/// Finish an exact submission while the same per-session input permit remains
/// held. Backend error strings are deliberately discarded at the phase seam.
pub async fn submit_exact_agent_input_with_permit(
    permit: &PtyInputPermit,
    text_write_succeeded: bool,
) -> AgentSubmitOutcome {
    if !text_write_succeeded {
        return AgentSubmitOutcome::TextWriteFailed;
    }
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    if PtyManager::write_with_permit(permit, b"\r").is_err() {
        return AgentSubmitOutcome::RequiredEnterFailed;
    }
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    AgentSubmitOutcome::Submitted {
        redundant_enter_failed: PtyManager::write_with_permit(permit, b"\r").is_err(),
    }
}

/// Inject a text block into a session's PTY stdin.
///
/// Direct Claude, Codex, Antigravity, Cursor agent, and the exact-stem Pi,
/// Hermes, OpenCode, and Grok Build shells receive `\r` twice, at 1500 ms and
/// 2000 ms after the text write, as a reliability measure against Enter not
/// registering on the first attempt. Plain shells do not receive an added
/// Enter.
///
/// This is the ONLY function that should be used for text-block injection.
/// Direct keystrokes from xterm.js bypass this and call PtyManager::write()
/// directly via the pty_write Tauri command.
pub async fn inject_text_into_session<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
    text: &str,
) -> Result<(), String> {
    inject_text_into_session_with_pre_write_check(app, session_id, text, || Ok(())).await
}

/// #2336 - peer-wake text injection: subject to the per-session typing hold.
/// The mailbox's standard message delivery and its logical remote command
/// delivery call this. The follow-up body after a logical command deliberately
/// keeps the plain `inject_text_into_session` above: that body was already
/// marked delivered when the command half succeeded, so a hold armed during the
/// detached idle wait must not drop it.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )
)]
pub(crate) async fn inject_peer_wake_text_into_session<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
    text: &str,
    message_id: &str,
    settle: SettleReadiness,
) -> Result<(), String> {
    inject_peer_wake_text_into_session_with_pre_write_check(
        app,
        session_id,
        text,
        message_id,
        settle,
        |_| Ok(()),
    )
    .await
}

/// Peer delivery retains its hold policy and completion cleanup while allowing
/// the caller to validate immediately before the serialized first write.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )
)]
pub(crate) async fn inject_peer_wake_text_into_session_with_pre_write_check<R, F>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
    text: &str,
    message_id: &str,
    settle: SettleReadiness,
    pre_write_check: F,
) -> Result<(), String>
where
    R: tauri::Runtime,
    F: FnOnce(&Session) -> Result<(), String>,
{
    inject_peer_wake_with_current_facts(app, session_id, text, message_id, settle, |session, _| {
        pre_write_check(session)
    })
    .await
}

/// Internal extension of the same peer boundary: upper policy admission sees
/// the interior snapshot already held by the injector, without manager reentry.
pub(crate) async fn inject_peer_wake_with_current_facts<R, F>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
    text: &str,
    message_id: &str,
    settle: SettleReadiness,
    pre_write_check: F,
) -> Result<(), String>
where
    R: tauri::Runtime,
    F: FnOnce(&Session, &crate::session::manager::CarrierSnapshot) -> Result<(), String>,
{
    let result = inject_text_into_session_impl(
        app,
        session_id,
        text,
        HoldPolicy::PeerWake(message_id),
        settle,
        |session, current| {
            let session = session.ok_or_else(|| format!("Session not found: {}", session_id))?;
            let current = current.expect("peer boundary supplies current facts");
            pre_write_check(session, current)
        },
    )
    .await;
    if result.is_ok() {
        // The message left the deferred set the moment its text was written; a
        // repeated clear is a no-op.
        crate::commands::pty::clear_held_wake(app, session_id, message_id);
    }
    result
}

pub(crate) async fn inject_text_into_session_with_pre_write_check<R, F>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
    text: &str,
    pre_write_check: F,
) -> Result<(), String>
where
    R: tauri::Runtime,
    F: FnOnce() -> Result<(), String>,
{
    inject_text_into_session_impl(
        app,
        session_id,
        text,
        HoldPolicy::Exempt,
        SettleReadiness::Unknown,
        move |session, _| {
            session.ok_or_else(|| format!("Session not found: {}", session_id))?;
            pre_write_check()
        },
    )
    .await
}

/// Canonical injector for trusted internal notices. This adds root, exited,
/// agentless, and plain-shell rejection and gives the caller the resolved
/// session snapshot for its final canonical-path and authorization check.
fn validate_supported_agent_session(session: &Session, session_id: Uuid) -> Result<(), String> {
    if session.is_root_agent {
        return Err(format!(
            "Session {} is a root session, not a supported orchestrator agent",
            session_id
        ));
    }
    if matches!(session.status, SessionStatus::Exited(_)) {
        return Err(format!("Session {} exited before injection", session_id));
    }
    if session.agent_id.is_none() {
        return Err(format!(
            "Session {} has no configured coding-agent identity",
            session_id
        ));
    }
    if !needs_explicit_enter(&session.shell) {
        return Err(format!(
            "Session {} shell '{}' is not a supported coding-agent CLI",
            session_id, session.shell
        ));
    }
    Ok(())
}

pub(crate) async fn inject_text_into_supported_agent_session_with_pre_write_check<R, F>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
    text: &str,
    pre_write_check: F,
) -> Result<(), String>
where
    R: tauri::Runtime,
    F: FnOnce(&Session) -> Result<(), String>,
{
    inject_text_into_session_impl(
        app,
        session_id,
        text,
        HoldPolicy::Exempt,
        SettleReadiness::Unknown,
        move |session, _| {
            let session = session.ok_or_else(|| {
                format!(
                    "Session {} is missing before supported-agent injection",
                    session_id
                )
            })?;
            validate_supported_agent_session(session, session_id)?;
            pre_write_check(session)
        },
    )
    .await
}

enum HoldPolicy<'a> {
    Exempt,
    PeerWake(&'a str),
    InternalNotice,
}

/// Read-only preflight; the injector repeats this under the writer permit.
pub(crate) async fn internal_notice_block_reason<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
) -> Option<String> {
    if let Some(menu) = app.try_state::<Arc<crate::pty::menu_guard::MenuGuard>>() {
        if menu.is_blocked(session_id) {
            return Some(format!(
                "{}: session {} is blocked by interactive menu",
                crate::pty::menu_guard::ERR_MENU_GUARD_DEFERRED,
                session_id
            ));
        }
    }
    if crate::commands::pty::typing_hold_defers_injection(app, session_id).await {
        return Some(format!(
            "{}: session {} is holding internal notice injection while the user is typing",
            crate::pty::menu_guard::ERR_TYPING_HOLD_DEFERRED,
            session_id
        ));
    }
    None
}

pub(crate) async fn inject_internal_notice_into_supported_agent_session<R, F>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
    text: &str,
    pre_write_check: F,
) -> Result<(), String>
where
    R: tauri::Runtime,
    F: FnOnce(&Session) -> Result<(), String>,
{
    inject_text_into_session_impl(
        app,
        session_id,
        text,
        HoldPolicy::InternalNotice,
        SettleReadiness::Unknown,
        move |session, _| {
            let session = session.ok_or_else(|| {
                format!(
                    "Session {} is missing before supported-agent injection",
                    session_id
                )
            })?;
            validate_supported_agent_session(session, session_id)?;
            pre_write_check(session)
        },
    )
    .await
}

async fn write_injected_text_checked<F>(
    manager: &SessionManager,
    pty_manager: &Arc<Mutex<PtyManager>>,
    permit: &crate::pty::manager::PtyInputPermit,
    session_id: uuid::Uuid,
    text: &str,
    legacy_check: Option<F>,
) -> Result<(), String>
where
    F: FnOnce(
        Option<&Session>,
        Option<&crate::session::manager::CarrierSnapshot>,
    ) -> Result<(), String>,
{
    if let Some(check) = legacy_check {
        // Snapshotting a typed route is pure registry access. The verified
        // guard validates that same kind/generation/identity against our
        // already-held writer; it writes directly, without reacquiring itself.
        let route = PtyManager::snapshot_route_proof(pty_manager, session_id).ok();
        manager
            .with_response_close_facts(|current| {
                let route_guard = match route
                    .as_ref()
                    .and_then(|route| route.saved_replica().map(|replica| (route, replica)))
                {
                    Some((route, replica)) => PtyManager::lock_route_for_verified_write(
                        permit,
                        route.backend_kind(),
                        route.saved_cwd(),
                        replica,
                    ),
                    None => PtyManager::lock_route_for_write(permit),
                };
                let route_guard =
                    route_guard.map_err(|error| format!("PTY write failed: {}", error))?;
                let session = current.rows.iter().find(|session| session.id == session_id);
                check(session, Some(current))?;
                route_guard
                    .write(text.as_bytes())
                    .map_err(|error| format!("PTY write failed: {}", error))
            })
            .await
    } else {
        PtyManager::write_with_permit(permit, text.as_bytes())
            .map_err(|error| format!("PTY write failed: {}", error))
    }
}

async fn check_injection_hold<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: uuid::Uuid,
    hold_policy: &HoldPolicy<'_>,
) -> Result<(), String> {
    if let Some(menu_guard) = app.try_state::<Arc<crate::pty::menu_guard::MenuGuard>>() {
        if menu_guard.is_blocked(session_id) {
            return Err(format!(
                "{}: session {} is blocked by interactive menu",
                crate::pty::menu_guard::ERR_MENU_GUARD_DEFERRED,
                session_id
            ));
        }
    }

    // #2336 - the typing hold gate, still under the per-session writer permit so
    // a key written against this session is observed before any payload byte.
    // Peer wakes record their IDs; internal notices defer without registering
    // an ID. Plain, restart-resume and logical follow-up writes remain exempt.
    if let HoldPolicy::PeerWake(message_id) = hold_policy {
        if crate::commands::pty::typing_hold_defers_injection(app, session_id).await {
            crate::commands::pty::note_held_wake(app, session_id, message_id);
            return Err(format!(
                "{}: session {} is holding peer wake injection while the user is typing",
                crate::pty::menu_guard::ERR_TYPING_HOLD_DEFERRED,
                session_id
            ));
        }
    }

    if matches!(hold_policy, HoldPolicy::InternalNotice) {
        if let Some(reason) = internal_notice_block_reason(app, session_id).await {
            return Err(reason);
        }
    }
    Ok(())
}

async fn inject_text_into_session_impl<R, F>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
    text: &str,
    hold_policy: HoldPolicy<'_>,
    settle: SettleReadiness,
    pre_write_check: F,
) -> Result<(), String>
where
    R: tauri::Runtime,
    F: FnOnce(
        Option<&Session>,
        Option<&crate::session::manager::CarrierSnapshot>,
    ) -> Result<(), String>,
{
    // Resolve one public snapshot without retaining a manager guard across an await.
    let manager = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let manager = session_mgr.read().await.clone();
        manager
    };
    let session = manager.get_session(session_id).await;
    let shell = session.as_ref().map(|session| session.shell.clone());
    let send_enter = shell.as_deref().map(needs_explicit_enter).unwrap_or(false);
    log::info!(
        "[inject] session={} shell={:?} send_enter={}",
        session_id,
        shell,
        send_enter
    );

    // Acquire the per-session input permit before running the pre-write closure,
    // so the closure and the first checked write happen together at the real
    // serialized write boundary (plan 7.5). The permit is held across the whole
    // text-then-Enter seam, which gives legacy/logical injection writer
    // serialization (invariant 3) without changing its public byte sequence.
    let pty_manager = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let permit = PtyManager::acquire_input_writer(&pty_manager, session_id)
        .await
        .map_err(|error| error.to_string())?;

    check_injection_hold(app, session_id, &hold_policy).await?;

    // Deliberately synchronous and immediately adjacent to the serialized write
    // boundary. Callers of the supported variant perform their final
    // filesystem/config guard here.
    // Peer callbacks use the fresh interior snapshot below, after all waits.
    let peer_boundary = matches!(hold_policy, HoldPolicy::PeerWake(_));
    let legacy_check = if peer_boundary {
        Some(pre_write_check)
    } else {
        // Internal notices must not admit a stale row after waiting for the writer.
        // Refresh without retaining a shared state guard during caller I/O.
        let current = if matches!(hold_policy, HoldPolicy::InternalNotice) {
            manager.get_session(session_id).await
        } else {
            session
        };
        pre_write_check(current.as_ref(), None)?;
        None
    };

    // #2586 D1 - on a `send_enter` shell the two later lone `\r` are the submit;
    // the wake render's trailing `\n\r` rides inside the pasted burst and can
    // only add a blank line. Strip exactly that two-byte suffix and nothing
    // else, never down to empty. Plain shells keep every byte.
    let text = match text.strip_suffix("\n\r") {
        Some(stripped) if send_enter && !stripped.is_empty() => stripped,
        _ => text,
    };

    // Write the text block through the held permit.
    let written = write_injected_text_checked(
        &manager,
        &pty_manager,
        &permit,
        session_id,
        text,
        legacy_check,
    )
    .await;
    written.map_err(|error| {
        if error == OWNED_WORK_SCOPE_WAIT {
            return error;
        }
        log::error!(
            "[inject] PTY write FAILED session={}: {}",
            session_id,
            error
        );
        error
    })?;
    log::info!(
        "[inject] PTY write OK session={} bytes={}",
        session_id,
        text.len()
    );
    crate::commands::pty::mark_successful_pty_write_busy(app, session_id, text.len()).await;

    // Supported interactive agent CLIs receive two staggered Enters. The
    // second is nonfatal because the first may already have submitted the text.
    if send_enter {
        submit_injected_enters(&permit, &pty_manager, session_id, settle, text).await?;
    }

    // #1682 - an injected text block is a message to the agent, submitted on
    // every branch but R8, so it arms this session and the busy->idle edges
    // that follow stamp `tooling.lastAgentMessageAt`. Single funnel for every
    // injection path (inter-agent wake, Loop delivery, the self-clear, self-switch
    // and self-restart resume prompts, internal system notices, Telegram inject),
    // so no caller needs its own site. Placed here rather than beside the
    // `mark_successful_pty_write_busy` call above so an early `Err` return
    // leaves the session unarmed. NOT "an undelivered message never arms": R8.
    {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let manager = session_mgr.read().await.clone();
        manager.arm_agent_turn(session_id).await;
        // #1682 - the text write above marked this session through
        // `mark_successful_pty_write_busy` (`:388`); this clear cancels that mark.
        // It keys on ARMING, not on proven delivery: R8 arms and clears with
        // nothing submitted. Self-cancelling here, so no caller needs its own site.
        crate::commands::pty::clear_control_write_mark(app, session_id);
    }

    Ok(())
}

async fn submit_injected_enters(
    permit: &PtyInputPermit,
    pty_manager: &Arc<Mutex<PtyManager>>,
    session_id: Uuid,
    settle: SettleReadiness,
    text: &str,
) -> Result<(), String> {
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    observe_submit_seam(pty_manager, session_id, "T0", settle, text);
    log::info!("[inject] sending Enter (1/2) for session {}", session_id);
    PtyManager::write_with_permit(permit, b"\r")
        .map_err(|error| format!("PTY Enter (1/2) write failed: {}", error))?;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    observe_submit_seam(pty_manager, session_id, "T1", settle, text);
    log::info!("[inject] sending Enter (2/2) for session {}", session_id);
    if let Err(error) = PtyManager::write_with_permit(permit, b"\r") {
        log::warn!(
            "[inject] Enter (2/2) failed for session {} (non-fatal): {}",
            session_id,
            error
        );
    }

    // #2586 D2 - T2: one added wait after the second Enter, observation only.
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    observe_submit_seam(pty_manager, session_id, "T2", settle, text);

    Ok(())
}

#[cfg(test)]
mod tests;
