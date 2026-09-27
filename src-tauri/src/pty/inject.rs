use std::fmt;
use std::sync::{Arc, Mutex};

use tauri::Manager;
use uuid::Uuid;

use crate::pty::backend::PTY_INPUT_MAX_BYTES;
use crate::pty::manager::{PtyInputPermit, PtyManager, PtyRouteWriteGuard};
use crate::session::manager::SessionManager;
use crate::session::session::{Session, SessionStatus};

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
            "\u{2500}\u{2501}\u{2550}\u{2502}\u{2503}\u{250C}\u{2510}\u{2514}\u{2518}\u{251C}\u{2524}\u{252C}\u{2534}\u{253C}\u{2554}\u{2557}\u{255A}\u{255D}\u{2560}\u{2563}\u{2566}\u{2569}\u{256C}".contains(c)
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
        "[inject] seam={instant} settle={settle:?} rows={} payload_on_screen={} payload_rows={payload_rows}",
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
pub(crate) async fn inject_peer_wake_text_into_session<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
    text: &str,
    message_id: &str,
    settle: SettleReadiness,
) -> Result<(), String> {
    let result =
        inject_text_into_session_impl(app, session_id, text, Some(message_id), settle, |session| {
            session.ok_or_else(|| format!("Session not found: {}", session_id))?;
            Ok(())
        })
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
        None,
        SettleReadiness::Unknown,
        move |session| {
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
        None,
        SettleReadiness::Unknown,
        move |session| {
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

async fn inject_text_into_session_impl<R, F>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
    text: &str,
    hold_message_id: Option<&str>,
    settle: SettleReadiness,
    pre_write_check: F,
) -> Result<(), String>
where
    R: tauri::Runtime,
    F: FnOnce(Option<&Session>) -> Result<(), String>,
{
    // Resolve one public snapshot without retaining a manager guard across an await.
    let session = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let manager = session_mgr.read().await.clone();
        manager.get_session(session_id).await
    };
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
    // Only the peer-wake entry point sets `hold_message_id`; internal notices,
    // self-maintenance, Telegram and the logical-command follow-up stay on the
    // plain injector and are never held. Deferring records the message ID and
    // writes no payload and no Enter byte.
    if let Some(message_id) = hold_message_id {
        if crate::commands::pty::typing_hold_defers_injection(app, session_id).await {
            crate::commands::pty::note_held_wake(app, session_id, message_id);
            return Err(format!(
                "{}: session {} is holding peer wake injection while the user is typing",
                crate::pty::menu_guard::ERR_TYPING_HOLD_DEFERRED,
                session_id
            ));
        }
    }

    // Deliberately synchronous and immediately adjacent to the serialized write
    // boundary. Callers of the supported variant perform their final
    // filesystem/config guard here.
    pre_write_check(session.as_ref())?;

    // #2586 D1 - on a `send_enter` shell the two later lone `\r` are the submit;
    // the wake render's trailing `\n\r` rides inside the pasted burst and can
    // only add a blank line. Strip exactly that two-byte suffix and nothing
    // else, never down to empty. Plain shells keep every byte.
    let text = match text.strip_suffix("\n\r") {
        Some(stripped) if send_enter && !stripped.is_empty() => stripped,
        _ => text,
    };

    // Write the text block through the held permit.
    PtyManager::write_with_permit(&permit, text.as_bytes()).map_err(|error| {
        log::error!(
            "[inject] PTY write FAILED session={}: {}",
            session_id,
            error
        );
        format!("PTY write failed: {}", error)
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
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        observe_submit_seam(&pty_manager, session_id, "T0", settle, text);
        log::info!("[inject] sending Enter (1/2) for session {}", session_id);
        PtyManager::write_with_permit(&permit, b"\r")
            .map_err(|error| format!("PTY Enter (1/2) write failed: {}", error))?;

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        observe_submit_seam(&pty_manager, session_id, "T1", settle, text);
        log::info!("[inject] sending Enter (2/2) for session {}", session_id);
        if let Err(error) = PtyManager::write_with_permit(&permit, b"\r") {
            log::warn!(
                "[inject] Enter (2/2) failed for session {} (non-fatal): {}",
                session_id,
                error
            );
        }

        // #2586 D2 - T2: one added wait after the second Enter, observation only.
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        observe_submit_seam(&pty_manager, session_id, "T2", settle, text);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
    use chrono::Utc;
    use std::collections::VecDeque;

    #[derive(serde::Deserialize)]
    struct Fixture {
        name: String,
        text: String,
        valid: bool,
    }

    #[test]
    fn shared_validation_fixture_is_authoritative() {
        let rows: Vec<Fixture> = serde_json::from_str(include_str!(
            "../../../crates/session-bridge/tests/fixtures/pty_input_validation.json"
        ))
        .unwrap();
        for row in rows {
            assert_eq!(
                validate_pty_input_text(&row.text).is_ok(),
                row.valid,
                "fixture {}",
                row.name
            );
        }
    }

    #[test]
    fn validator_rejects_every_forbidden_control_and_bidi_scalar() {
        let mut forbidden: Vec<u32> = (0x00..=0x1f)
            .filter(|code| !matches!(code, 0x09 | 0x0a))
            .chain(0x7f..=0x9f)
            .collect();
        forbidden.extend([
            0x061c, 0x200e, 0x200f, 0x2028, 0x2029, 0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x2066,
            0x2067, 0x2068, 0x2069,
        ]);
        for code in forbidden {
            let scalar = char::from_u32(code).expect("valid scalar");
            let text = format!("a{scalar}b");
            assert!(
                validate_pty_input_text(&text).is_err(),
                "U+{code:04X} must reject"
            );
        }
        for accepted in [" \t\n ", "-x; $(echo) | cat", "héllo 世界"] {
            assert!(validate_pty_input_text(accepted).is_ok(), "{accepted:?}");
        }
    }

    #[test]
    fn validator_enforces_utf8_byte_boundary() {
        assert!(validate_pty_input_text(&"x".repeat(PTY_INPUT_MAX_BYTES)).is_ok());
        let error = validate_pty_input_text(&"x".repeat(PTY_INPUT_MAX_BYTES + 1)).unwrap_err();
        assert_eq!(error.kind, PtyInputTextErrorKind::TooLarge);
        assert!(validate_pty_input_text(&"é".repeat(PTY_INPUT_MAX_BYTES / 2)).is_ok());
        assert!(
            validate_pty_input_text(&format!("{}é", "x".repeat(PTY_INPUT_MAX_BYTES - 1))).is_err()
        );
    }

    #[test]
    fn validator_reports_offset_without_preview() {
        let error = validate_pty_input_text("ok\u{001b}bad").unwrap_err();
        assert_eq!(error.byte_offset, 2);
        assert_eq!(error.scalar_offset, 2);
        assert_eq!(error.code_point, Some(0x1b));
        assert!(!error.to_string().contains("bad"));
    }

    #[test]
    fn agent_clis_require_explicit_enter() {
        for shell in [
            "codex",
            "codex.exe",
            "C:\\Users\\maria\\.codex\\codex.exe",
            "/usr/local/bin/claude",
            "agy",
            "agy.exe",
            "C:\\tools\\agy.cmd",
            "antigravity",
            "agent.exe",
            "hermes",
            "hermes.exe",
            "hermes.cmd",
            "hermes.ps1",
            "HERMES",
            "  hermes  ",
            "/usr/local/bin/hermes",
            "opencode",
            "opencode.exe",
            "opencode.cmd",
            "opencode.ps1",
            "OPENCODE",
            "  opencode  ",
            "/usr/local/bin/opencode",
            "grok",
            "grok.exe",
            "grok.cmd",
            "grok.ps1",
            "GROK",
            "  grok  ",
            "/usr/local/bin/grok",
        ] {
            assert!(needs_explicit_enter(shell), "shell={shell:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn explicit_submit_windows_native_paths() {
        // `shell_file_stem` relies on `std::path::Path::file_stem`, which does
        // not treat `\` as a separator off Windows, so these native path shapes
        // are asserted only where that behaviour holds.
        for shell in [
            r"C:\Tools\hermes.exe",
            r"\\server\share\hermes.cmd",
            r"\\?\C:\Tools\hermes.exe",
            r"C:\Tools\opencode.exe",
            r"\\server\share\opencode.cmd",
            r"\\?\C:\Tools\opencode.exe",
            r"C:\Tools\grok.exe",
            r"\\server\share\grok.cmd",
            r"\\?\C:\Tools\grok.exe",
        ] {
            assert!(needs_explicit_enter(shell), "shell={shell:?}");
        }
    }

    #[test]
    fn plain_shells_do_not_require_explicit_enter() {
        for shell in ["bash", "powershell.exe", "cmd.exe", "agentctl", ""] {
            assert!(!needs_explicit_enter(shell), "shell={shell:?}");
        }
    }

    struct ScriptedBackend {
        outcomes: Mutex<VecDeque<Result<(), ()>>>,
        calls: Mutex<Vec<Vec<u8>>>,
    }

    impl ScriptedBackend {
        fn new(outcomes: Vec<Result<(), ()>>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl crate::pty::backend::PtyBackend for ScriptedBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            _spec: crate::pty::backend::BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
            Box::pin(async { Ok(()) })
        }

        fn write(
            &self,
            _authority: &crate::pty::manager::BackendWriteAuthority,
            _id: Uuid,
            data: &[u8],
        ) -> Result<(), crate::errors::AppError> {
            self.calls.lock().unwrap().push(data.to_vec());
            match self.outcomes.lock().unwrap().pop_front().unwrap_or(Ok(())) {
                Ok(()) => Ok(()),
                Err(()) => Err(crate::errors::AppError::PtyError(
                    "scripted write failure".to_string(),
                )),
            }
        }

        fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn kill(&self, _id: Uuid) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn has_session(&self, _id: Uuid) -> bool {
            true
        }

        fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }

        fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
            None
        }

        fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::SessionOver
        }

        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: std::path::PathBuf,
        ) {
        }

        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
    }

    async fn scripted_exact_submission(
        outcomes: Vec<Result<(), ()>>,
    ) -> (AgentSubmitOutcome, Vec<Vec<u8>>) {
        let id = Uuid::new_v4();
        let backend = Arc::new(ScriptedBackend::new(outcomes));
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        manager
            .lock()
            .unwrap()
            .try_record_route(id, crate::pty::backend::SessionBackendKind::LocalProcess)
            .unwrap();
        let permit = PtyManager::acquire_input_writer(&manager, id)
            .await
            .unwrap();
        let route = PtyManager::lock_route_for_write(&permit).unwrap();
        let text_succeeded = write_exact_agent_input_first(route, b"exact text");
        let outcome = submit_exact_agent_input_with_permit(&permit, text_succeeded).await;
        let calls = backend.calls.lock().unwrap().clone();
        (outcome, calls)
    }

    #[tokio::test]
    async fn a_waiting_user_write_cannot_splice_between_text_and_enters() {
        let id = Uuid::new_v4();
        let backend = Arc::new(ScriptedBackend::new(vec![Ok(()); 4]));
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        manager
            .lock()
            .unwrap()
            .try_record_route(id, crate::pty::backend::SessionBackendKind::LocalProcess)
            .unwrap();
        let privileged = PtyManager::acquire_input_writer(&manager, id)
            .await
            .unwrap();
        let waiting_manager = Arc::clone(&manager);
        let user = tokio::spawn(async move {
            let permit = PtyManager::acquire_input_writer(&waiting_manager, id)
                .await
                .unwrap();
            PtyManager::write_with_permit(&permit, b"user").unwrap();
        });
        tokio::task::yield_now().await;
        let route = PtyManager::lock_route_for_write(&privileged).unwrap();
        assert!(write_exact_agent_input_first(route, b"exact text"));
        let outcome = submit_exact_agent_input_with_permit(&privileged, true).await;
        assert_eq!(
            outcome,
            AgentSubmitOutcome::Submitted {
                redundant_enter_failed: false
            }
        );
        assert!(!user.is_finished());
        drop(privileged);
        user.await.unwrap();
        assert_eq!(
            backend.calls.lock().unwrap().as_slice(),
            [
                b"exact text".to_vec(),
                b"\r".to_vec(),
                b"\r".to_vec(),
                b"user".to_vec(),
            ]
        );
    }

    #[tokio::test]
    async fn exact_submission_phase_outcomes_and_backend_calls_are_pinned() {
        let (text_failed, calls) = scripted_exact_submission(vec![Err(())]).await;
        assert_eq!(text_failed, AgentSubmitOutcome::TextWriteFailed);
        assert_eq!(calls, vec![b"exact text".to_vec()]);

        let (required_failed, calls) = scripted_exact_submission(vec![Ok(()), Err(())]).await;
        assert_eq!(required_failed, AgentSubmitOutcome::RequiredEnterFailed);
        assert_eq!(calls, vec![b"exact text".to_vec(), b"\r".to_vec()]);

        let (redundant_failed, calls) =
            scripted_exact_submission(vec![Ok(()), Ok(()), Err(())]).await;
        assert_eq!(
            redundant_failed,
            AgentSubmitOutcome::Submitted {
                redundant_enter_failed: true
            }
        );
        assert_eq!(
            calls,
            vec![b"exact text".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
        );

        let (submitted, calls) = scripted_exact_submission(vec![Ok(()), Ok(()), Ok(())]).await;
        assert_eq!(
            submitted,
            AgentSubmitOutcome::Submitted {
                redundant_enter_failed: false
            }
        );
        assert_eq!(
            calls,
            vec![b"exact text".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
        );
    }

    fn supported_session() -> Session {
        Session {
            id: Uuid::new_v4(),
            name: "coordinator".to_string(),
            shell: "codex".to_string(),
            shell_args: Vec::new(),
            backend_kind: SessionBackendKind::LocalProcess,
            effective_shell_args: None,
            created_at: Utc::now(),
            working_directory: "C:/replica".to_string(),
            status: SessionStatus::Running,
            waiting_for_input: false,
            communication: None,
            pending_review: false,
            last_prompt: None,
            agent_id: Some("codex-profile".to_string()),
            agent_label: Some("Codex".to_string()),
            git_repos: Vec::new(),
            is_coordinator: true,
            is_root_agent: false,
            git_repos_gen: 0,
            agent_turn_armed: false,
            token: Uuid::new_v4(),
            agent_kind: None,
            requested_profile: None,
            effective_profile: None,
            profile_fallback_chain: Vec::new(),
            profile_fallback_applied: false,
            match_tier: None,
            original_profile_letter: None,
            effective_codex_home: None,
            resolved_claude_projects_dir: None,
            profile_content_hash: None,
            trusted_configured_spawn: false,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            start_fresh_on_restore: false,
            context_percent: None,
        }
    }

    #[test]
    fn supported_agent_final_snapshot_rejects_unsafe_recipient_records() {
        let valid = supported_session();
        assert!(validate_supported_agent_session(&valid, valid.id).is_ok());

        for shell in ["hermes", "opencode", "grok"] {
            let mut widened = supported_session();
            widened.shell = shell.to_string();
            assert!(
                validate_supported_agent_session(&widened, widened.id).is_ok(),
                "shell={shell:?}"
            );
        }

        let mut root = supported_session();
        root.is_root_agent = true;
        assert!(validate_supported_agent_session(&root, root.id).is_err());

        let mut exited = supported_session();
        exited.status = SessionStatus::Exited(0);
        assert!(validate_supported_agent_session(&exited, exited.id).is_err());

        let mut agentless = supported_session();
        agentless.agent_id = None;
        assert!(validate_supported_agent_session(&agentless, agentless.id).is_err());

        let mut shell = supported_session();
        shell.shell = "pwsh".to_string();
        assert!(validate_supported_agent_session(&shell, shell.id).is_err());
    }

    #[test]
    fn direct_shell_capability_matrix() {
        let pi_positive = [
            "pi",
            "PI",
            "pi.exe",
            "Pi.CMD",
            "pi.ps1",
            r"C:\Tools\pi.exe",
            r"\\server\share\pi.cmd",
            r"\\?\C:\Tools\pi.exe",
            "/usr/local/bin/pi",
            "  pi  ",
        ];
        for shell in pi_positive {
            assert!(needs_explicit_enter(shell), "Pi positive: {shell:?}");
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Clear),
                Some("/new"),
                "Pi clear mapping: {shell:?}"
            );
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Compact),
                None,
                "Pi compact remains unsupported: {shell:?}"
            );
            assert!(supports_auto_self_maintenance(shell));
            assert!(
                supports_self_handoff_switch(shell),
                "Pi switch source: {shell:?}"
            );
        }

        let explicit_submit_positive = [
            "hermes",
            "Hermes",
            "hermes.exe",
            "HERMES.CMD",
            "hermes.ps1",
            "opencode",
            "OpenCode",
            "opencode.exe",
            "OpenCode.CMD",
            "opencode.ps1",
            "grok",
            "GROK",
            "grok.exe",
            "Grok.CMD",
            "grok.ps1",
            "  grok  ",
            "/usr/local/bin/hermes",
            "/usr/local/bin/opencode",
        ];
        for shell in explicit_submit_positive {
            assert!(
                needs_explicit_enter(shell),
                "ExplicitSubmit positive: {shell:?}"
            );
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Clear),
                None,
                "ExplicitSubmit clear remains unsupported: {shell:?}"
            );
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Compact),
                None,
                "ExplicitSubmit compact remains unsupported: {shell:?}"
            );
            assert!(
                !supports_auto_self_maintenance(shell),
                "ExplicitSubmit maintenance stays off: {shell:?}"
            );
            assert!(
                !supports_self_handoff_switch(shell),
                "ExplicitSubmit switch stays off: {shell:?}"
            );
        }

        let unsupported = [
            "pip",
            "pipx",
            "ping",
            "pixel",
            "pi-agent",
            "pi2",
            "pi-claude",
            "hermes-wrapper",
            "hermes-cli",
            "my-hermes",
            "opencode-proxy",
            "opencode2",
            "opencode-tui",
            "grok-build",
            "grokx",
            "grok-cli",
            r"C:\pi\runner.exe",
            "agy-proxy",
            "agyctl",
            "cmd.exe",
            "pwsh",
            "",
            "   ",
            "bash",
        ];
        for shell in unsupported {
            assert!(!needs_explicit_enter(shell), "unsupported: {shell:?}");
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Clear),
                None
            );
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Compact),
                None
            );
            assert!(!supports_auto_self_maintenance(shell));
            assert!(!supports_self_handoff_switch(shell));
        }

        for shell in [
            "claude",
            "claude-pi",
            "codex-wrapper.cmd",
            "codex-proxy.exe",
            "agy",
            "antigravity.exe",
        ] {
            assert!(needs_explicit_enter(shell));
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Clear),
                Some("/clear")
            );
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Compact),
                Some("/compact")
            );
            assert!(supports_auto_self_maintenance(shell));
            assert!(supports_self_handoff_switch(shell));
        }

        for shell in ["agent", "agent.exe", r"C:\Cursor\agent.cmd"] {
            assert!(needs_explicit_enter(shell));
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Clear),
                Some("/clear")
            );
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Compact),
                Some("/compact")
            );
            assert!(!supports_auto_self_maintenance(shell));
            assert!(supports_self_handoff_switch(shell));
        }
        assert!(!needs_explicit_enter("agentctl"));
        assert!(!needs_explicit_enter("agentic"));
    }

    #[test]
    fn logical_command_parser_and_boundary_matrix() {
        assert_eq!(
            LogicalPtyCommand::from_wire_value("clear"),
            Some(LogicalPtyCommand::Clear)
        );
        assert_eq!(
            LogicalPtyCommand::from_wire_value("compact"),
            Some(LogicalPtyCommand::Compact)
        );
        for value in ["Clear", "COMPACT", "", "new"] {
            assert_eq!(LogicalPtyCommand::from_wire_value(value), None);
        }
        assert!(LogicalPtyCommand::Clear.creates_fresh_boundary());
        assert!(!LogicalPtyCommand::Compact.creates_fresh_boundary());
    }

    #[derive(Default)]
    struct RecordingBackend {
        writes: Mutex<Vec<(Uuid, Vec<u8>)>>,
        /// #2586 - scripted screen rows. `None` keeps the `SessionOver` answer,
        /// `Some(None)` answers `Unavailable`, `Some(Some(rows))` answers `Rows`.
        screen_rows: Mutex<Option<Option<Vec<String>>>>,
    }

    impl PtyBackend for RecordingBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            _spec: BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
            Box::pin(async { Ok(()) })
        }

        fn write(
            &self,
            _authority: &crate::pty::manager::BackendWriteAuthority,
            id: Uuid,
            data: &[u8],
        ) -> Result<(), crate::errors::AppError> {
            self.writes.lock().unwrap().push((id, data.to_vec()));
            Ok(())
        }

        fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn kill(&self, _id: Uuid) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn has_session(&self, _id: Uuid) -> bool {
            true
        }

        fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }

        fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
            None
        }

        fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            use crate::pty::context_scrape::ScreenRowsRead;
            match self.screen_rows.lock().unwrap().clone() {
                None => ScreenRowsRead::SessionOver,
                Some(None) => ScreenRowsRead::Unavailable,
                Some(Some(rows)) => ScreenRowsRead::Rows(rows),
            }
        }

        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: std::path::PathBuf,
        ) {
        }

        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
    }

    #[tokio::test]
    async fn missing_session_record_with_lingering_route_writes_nothing() {
        let id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(id, SessionBackendKind::LocalProcess);
        let app = tauri::test::mock_builder()
            .manage(Arc::new(tokio::sync::RwLock::new(SessionManager::new())))
            .manage(pty)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();

        let err = inject_text_into_session(app.handle(), id, "/new")
            .await
            .unwrap_err();

        assert_eq!(err, format!("Session not found: {id}"));
        assert!(backend.writes.lock().unwrap().is_empty());
    }

    async fn recorded_injection_writes_for_shell(shell: &str) -> Vec<Vec<u8>> {
        let session_manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = session_manager
            .read()
            .await
            .create_session(
                shell.to_string(),
                Vec::new(),
                "C:\\test".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let id = session.id;
        let backend = Arc::new(RecordingBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(id, SessionBackendKind::LocalProcess);
        let app = tauri::test::mock_builder()
            .manage(session_manager)
            .manage(pty)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();

        inject_text_into_session(app.handle(), id, "arbitrary payload")
            .await
            .unwrap();

        let all_writes = backend.writes.lock().unwrap().clone();
        all_writes
            .into_iter()
            .filter(|(write_id, _)| *write_id == id)
            .map(|(_, bytes)| bytes)
            .collect()
    }

    #[tokio::test]
    async fn explicit_submit_stems_write_text_then_two_enters() {
        let (hermes, opencode, grok) = tokio::join!(
            recorded_injection_writes_for_shell("hermes"),
            recorded_injection_writes_for_shell("opencode"),
            recorded_injection_writes_for_shell("grok"),
        );
        for (shell, writes) in [("hermes", hermes), ("opencode", opencode), ("grok", grok)] {
            assert_eq!(
                writes,
                vec![
                    b"arbitrary payload".to_vec(),
                    b"\r".to_vec(),
                    b"\r".to_vec()
                ],
                "shell={shell}"
            );
        }
    }

    #[tokio::test]
    async fn unsupported_stem_writes_text_without_enter() {
        let writes = recorded_injection_writes_for_shell("muse").await;
        assert_eq!(writes, vec![b"arbitrary payload".to_vec()]);
    }

    #[tokio::test]
    async fn test_injection_blocked_when_menu_guard_active() {
        let session_manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = session_manager
            .read()
            .await
            .create_session(
                "claude".to_string(),
                Vec::new(),
                "C:\\test".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let id = session.id;

        let backend = Arc::new(RecordingBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(id, SessionBackendKind::LocalProcess);

        let menu_guard = Arc::new(crate::pty::menu_guard::MenuGuard::new());
        let entries = vec![crate::config::settings::BlockingMenuEntry::Valid(
            crate::config::settings::BlockingMenuConfig {
                pattern: "Do you trust".to_string(),
                notification: "trust dialog".to_string(),
                enabled: true,
                captured_against: None,
            },
        )];
        let eval = menu_guard.evaluate_logical_rows(
            id,
            &[crate::pty::watchers::frame::LogicalRow {
                text: "Do you trust the authors of this file?".to_string(),
                start: 0,
                end: 0,
            }],
            &entries,
        );
        assert!(eval.is_blocked);
        assert!(menu_guard.is_blocked(id));

        let app = tauri::test::mock_builder()
            .manage(session_manager)
            .manage(pty)
            .manage(menu_guard)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();

        let err = inject_text_into_session(app.handle(), id, "echo hello")
            .await
            .unwrap_err();

        assert!(crate::pty::menu_guard::is_menu_guard_deferred_error(&err));
        assert!(err.contains(&id.to_string()));
        assert!(backend.writes.lock().unwrap().is_empty());
    }

    async fn typing_hold_app(
        shell: &str,
    ) -> (
        tauri::App<tauri::test::MockRuntime>,
        Uuid,
        Arc<RecordingBackend>,
    ) {
        let session_manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = session_manager
            .read()
            .await
            .create_session(
                shell.to_string(),
                Vec::new(),
                "C:\\test".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let id = session.id;
        let backend = Arc::new(RecordingBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(id, SessionBackendKind::LocalProcess);
        let settings_state: crate::config::settings::SettingsState = Arc::new(
            tokio::sync::RwLock::new(crate::config::settings::AppSettings::default()),
        );
        let app = tauri::test::mock_builder()
            .manage(session_manager)
            .manage(pty)
            .manage(crate::pty::input_activity::new_typing_hold_state())
            .manage(settings_state)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        (app, id, backend)
    }

    /// #2336 - a peer wake is deferred with zero bytes written while the typing
    /// hold is active; a repeated poll does not grow the unique count; a manual
    /// release lets the very next attempt deliver and clears the id.
    #[tokio::test]
    async fn test_peer_wake_injection_blocked_while_typing_hold_active() {
        let (app, id, backend) = typing_hold_app("claude").await;
        let window = std::time::Duration::from_secs(30);
        let hold = app.state::<crate::pty::input_activity::TypingHoldState>();
        hold.lock().unwrap().note_qualifying_key(id);

        let err = inject_peer_wake_text_into_session(
            app.handle(),
            id,
            "echo hello",
            "msg-1",
            SettleReadiness::Unknown,
        )
        .await
        .unwrap_err();
        assert!(crate::pty::menu_guard::is_typing_hold_deferred_error(&err));
        assert!(err.contains(&id.to_string()));
        assert!(
            backend.writes.lock().unwrap().is_empty(),
            "payload and Enter bytes must not be written while held"
        );
        assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 1);

        // A repeated poll of the same message never increases the unique count.
        let _ = inject_peer_wake_text_into_session(
            app.handle(),
            id,
            "echo hello",
            "msg-1",
            SettleReadiness::Unknown,
        )
        .await;
        assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 1);

        // The closed-click release suppresses the window, so the next attempt
        // delivers and the observed id drops out of the count.
        let released = hold.lock().unwrap().toggle_manual(id, window);
        assert!(!released.closed);
        inject_peer_wake_text_into_session(
            app.handle(),
            id,
            "echo hello",
            "msg-1",
            SettleReadiness::Unknown,
        )
        .await
        .unwrap();
        let writes: Vec<Vec<u8>> = backend
            .writes
            .lock()
            .unwrap()
            .iter()
            .filter(|(write_id, _)| *write_id == id)
            .map(|(_, bytes)| bytes.clone())
            .collect();
        assert_eq!(
            writes,
            vec![b"echo hello".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
        );
        assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 0);
    }

    /// #2336 - internal notices, self-maintenance and Telegram keep the plain
    /// injector: a hold never delays them.
    #[tokio::test]
    async fn test_internal_injection_bypasses_typing_hold() {
        let (app, id, backend) = typing_hold_app("claude").await;
        app.state::<crate::pty::input_activity::TypingHoldState>()
            .lock()
            .unwrap()
            .note_qualifying_key(id);

        inject_text_into_session(app.handle(), id, "internal notice")
            .await
            .unwrap();

        let writes: Vec<Vec<u8>> = backend
            .writes
            .lock()
            .unwrap()
            .iter()
            .filter(|(write_id, _)| *write_id == id)
            .map(|(_, bytes)| bytes.clone())
            .collect();
        assert_eq!(writes.len(), 3);
        assert_eq!(writes[0], b"internal notice".to_vec());
    }

    fn recorded_writes(backend: &Arc<RecordingBackend>, id: Uuid) -> Vec<Vec<u8>> {
        backend
            .writes
            .lock()
            .unwrap()
            .iter()
            .filter(|(write_id, _)| *write_id == id)
            .map(|(_, bytes)| bytes.clone())
            .collect()
    }

    /// #2336 - the serialized-write-boundary race: a peer wake queued behind the
    /// held per-session writer permit re-checks the hold when it finally acquires
    /// the permit, so a qualifying desktop key recorded while the permit was held
    /// defers it with the typed marker and zero payload/Enter bytes. After
    /// release the same message delivers exactly once and leaves the count.
    #[tokio::test]
    async fn typing_hold_peer_wake_queued_behind_writer_permit_rechecks_and_defers() {
        let (app, id, backend) = typing_hold_app("claude").await;
        let window = std::time::Duration::from_secs(30);
        let hold = app.state::<crate::pty::input_activity::TypingHoldState>();

        // Hold the writer permit, then queue a peer wake behind it.
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        let permit = PtyManager::acquire_input_writer(&pty, id).await.unwrap();
        let app_clone = app.handle().clone();
        let queued = tokio::spawn(async move {
            inject_peer_wake_text_into_session(
                &app_clone,
                id,
                "echo hello",
                "msg-race",
                SettleReadiness::Unknown,
            )
            .await
        });
        // Let the queued wake reach the permit wait.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // A qualifying desktop key lands while the permit is still held.
        hold.lock().unwrap().note_qualifying_key(id);
        drop(permit);

        let err = queued.await.unwrap().unwrap_err();
        assert!(crate::pty::menu_guard::is_typing_hold_deferred_error(&err));
        assert!(
            recorded_writes(&backend, id).is_empty(),
            "a deferred queued wake must write no payload or Enter byte"
        );
        assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 1);

        // Release: the next attempt delivers exactly once and clears the count.
        assert!(!hold.lock().unwrap().toggle_manual(id, window).closed);
        inject_peer_wake_text_into_session(
            app.handle(),
            id,
            "echo hello",
            "msg-race",
            SettleReadiness::Unknown,
        )
        .await
        .unwrap();
        assert_eq!(
            recorded_writes(&backend, id),
            vec![b"echo hello".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
        );
        assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 0);
    }

    /// #2336 - natural expiry with no manual action: once the configured window
    /// has passed since the last qualifying key, the next attempt delivers.
    #[tokio::test]
    async fn typing_hold_peer_wake_delivers_after_natural_window_expiry() {
        let (app, id, backend) = typing_hold_app("claude").await;
        let window = std::time::Duration::from_secs(30);
        let hold = app.state::<crate::pty::input_activity::TypingHoldState>();
        hold.lock().unwrap().note_qualifying_key(id);

        let err = inject_peer_wake_text_into_session(
            app.handle(),
            id,
            "echo hello",
            "msg-expiry",
            SettleReadiness::Unknown,
        )
        .await
        .unwrap_err();
        assert!(crate::pty::menu_guard::is_typing_hold_deferred_error(&err));
        assert!(recorded_writes(&backend, id).is_empty());
        assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 1);

        // Age the key past the configured window with no release of any kind.
        hold.lock()
            .unwrap()
            .backdate_last_key_for_test(id, window + std::time::Duration::from_secs(1));

        inject_peer_wake_text_into_session(
            app.handle(),
            id,
            "echo hello",
            "msg-expiry",
            SettleReadiness::Unknown,
        )
        .await
        .unwrap();
        assert_eq!(
            recorded_writes(&backend, id),
            vec![b"echo hello".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
        );
        assert_eq!(hold.lock().unwrap().snapshot(id, window).held_count, 0);
    }

    // #2586 - submit-seam observation sink. Keyed by session id so parallel
    // tests never read each other's observations.
    static SEAM_OBSERVATIONS: Mutex<Vec<(Uuid, String)>> = Mutex::new(Vec::new());

    pub(super) fn record_seam_observation(session_id: Uuid, observation: String) {
        SEAM_OBSERVATIONS
            .lock()
            .unwrap()
            .push((session_id, observation));
    }

    fn seam_observations(session_id: Uuid) -> Vec<String> {
        SEAM_OBSERVATIONS
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| *id == session_id)
            .map(|(_, observation)| observation.clone())
            .collect()
    }

    /// The wake render shape of `phone::messaging::format_pty_wrap`, restated so
    /// this test module adds no `pty::inject` -> `phone::messaging` reference.
    fn wake_wrap(from: &str, body: &str) -> String {
        format!("\n[Message from {}] {}\n\r", from, body)
    }

    const SEAM_FROM: &str = "proj:room-1/tech-lead";

    /// Drive one peer-wake injection with `settle` and optional scripted rows
    /// (`Some(None)` = `Unavailable`, `None` = the default `SessionOver`).
    async fn seam_run(
        shell: &str,
        text: &str,
        settle: SettleReadiness,
        rows: Option<Option<Vec<String>>>,
    ) -> (Result<(), String>, Vec<Vec<u8>>, Vec<String>) {
        let (app, id, backend) = typing_hold_app(shell).await;
        *backend.screen_rows.lock().unwrap() = rows;
        let result =
            inject_peer_wake_text_into_session(app.handle(), id, text, "msg-seam", settle).await;
        (result, recorded_writes(&backend, id), seam_observations(id))
    }

    /// T1 - send_enter: the wrap's final `\n\r` is removed, then two Enters.
    #[tokio::test]
    async fn seam_send_enter_strips_the_wrap_suffix_then_two_enters() {
        let payload = wake_wrap(SEAM_FROM, "please review the plan");
        let (result, writes, _) = seam_run("claude", &payload, SettleReadiness::Ready, None).await;
        result.unwrap();
        assert_eq!(writes.len(), 3);
        assert_eq!(writes[0], payload.as_bytes()[..payload.len() - 2].to_vec());
        assert_eq!(payload.len() - writes[0].len(), 2);
        assert!(!writes[0].ends_with(b"\r") && !writes[0].ends_with(b"\n"));
        assert_eq!(writes[1], b"\r".to_vec());
        assert_eq!(writes[2], b"\r".to_vec());
    }

    /// T2 - plain shell: byte-identical single write, no Enter, no observation.
    #[tokio::test]
    async fn seam_plain_shell_writes_payload_byte_for_byte() {
        let payload = wake_wrap(SEAM_FROM, "please review the plan");
        let (result, writes, observations) = seam_run(
            "muse",
            &payload,
            SettleReadiness::Ready,
            Some(Some(vec!["chrome".to_string()])),
        )
        .await;
        result.unwrap();
        assert_eq!(writes, vec![payload.as_bytes().to_vec()]);
        assert!(observations.is_empty(), "{observations:?}");
    }

    async fn assert_send_enter_first_write_unchanged(text: &str) {
        let (result, writes, _) = seam_run("claude", text, SettleReadiness::Unknown, None).await;
        result.unwrap();
        assert_eq!(writes.len(), 3, "text={text:?}");
        assert_eq!(writes[0], text.as_bytes().to_vec(), "text={text:?}");
    }

    /// T2a - Telegram-shaped text ending in a single `\n`.
    #[tokio::test]
    async fn seam_send_enter_keeps_a_trailing_lone_newline() {
        assert_send_enter_first_write_unchanged("hello\n").await;
    }

    /// T2b - text ending in a single `\r`.
    #[tokio::test]
    async fn seam_send_enter_keeps_a_trailing_lone_carriage_return() {
        assert_send_enter_first_write_unchanged("hello\r").await;
    }

    /// T2c - Loop-prompt-shaped text with no trailing newline.
    #[tokio::test]
    async fn seam_send_enter_keeps_text_without_trailing_newline() {
        assert_send_enter_first_write_unchanged("Run the scheduled loop step now.").await;
    }

    /// T2d - the suffix is present but not final.
    #[tokio::test]
    async fn seam_send_enter_keeps_a_non_final_wrap_suffix() {
        assert_send_enter_first_write_unchanged("hello\n\r\n").await;
    }

    /// T3 - interior newlines survive the strip.
    #[tokio::test]
    async fn seam_send_enter_keeps_interior_newlines() {
        let payload = wake_wrap(SEAM_FROM, "a\nb");
        let (result, writes, _) = seam_run("claude", &payload, SettleReadiness::Ready, None).await;
        result.unwrap();
        assert!(payload.ends_with("a\nb\n\r"));
        assert_eq!(
            writes[0],
            payload.trim_end_matches("\n\r").as_bytes().to_vec()
        );
        assert!(writes[0].ends_with(b"a\nb"));
    }

    /// T4 - an all-newline payload is never stripped to empty; Enters follow.
    #[tokio::test]
    async fn seam_send_enter_never_strips_to_empty() {
        let (result, writes, _) = seam_run("claude", "\n\r", SettleReadiness::Ready, None).await;
        result.unwrap();
        assert_eq!(
            writes,
            vec![b"\n\r".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
        );
    }

    /// T5 - three observations, in order T0, T1, T2, each with the settle.
    #[tokio::test]
    async fn seam_observations_are_emitted_in_order() {
        let payload = wake_wrap(SEAM_FROM, "please review the plan");
        let (result, _, observations) = seam_run(
            "claude",
            &payload,
            SettleReadiness::TimedOut,
            Some(Some(vec!["? for shortcuts".to_string()])),
        )
        .await;
        result.unwrap();
        assert_eq!(observations.len(), 3, "{observations:?}");
        for (observation, instant) in observations.iter().zip(["T0", "T1", "T2"]) {
            assert!(
                observation
                    .starts_with(&format!("[inject] seam={instant} settle=TimedOut rows=1 ")),
                "{observation}"
            );
            assert!(
                observation.ends_with("idx=0:? for shortcuts"),
                "{observation}"
            );
        }
    }

    /// T6 - unavailable rows are logged and the injection still succeeds.
    #[tokio::test]
    async fn seam_unavailable_rows_never_fail_the_injection() {
        let payload = wake_wrap(SEAM_FROM, "please review the plan");
        let (result, writes, observations) =
            seam_run("claude", &payload, SettleReadiness::Unknown, Some(None)).await;
        assert_eq!(result, Ok(()));
        assert_eq!(writes.len(), 3);
        assert_eq!(observations.len(), 3);
        assert_eq!(
            observations[0],
            "[inject] seam=T0 settle=Unknown rows=unavailable"
        );
    }

    fn rows(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn observe(body: &str, screen: &[String]) -> String {
        format_submit_seam_observation(
            "T0",
            SettleReadiness::Ready,
            Some(screen),
            &wake_wrap(SEAM_FROM, body),
        )
    }

    fn assert_fixture_is_screen_shaped(screen: &[String]) {
        for row in screen {
            assert!(!row.contains('\n') && !row.contains('\r'), "{row:?}");
        }
    }

    /// T8 - the last six non-empty rows, top to bottom, with real indices and
    /// trailing box-drawing noise removed.
    #[test]
    fn seam_observation_keeps_the_last_six_non_empty_rows() {
        let screen: Vec<String> = (0..20)
            .map(|idx| {
                if idx % 2 == 1 {
                    "   ".to_string()
                } else {
                    format!("chrome {idx} \u{2500}\u{2500}  ")
                }
            })
            .collect();
        let rendered = observe("zzzzzzzzzzzzzz", &screen);
        assert_eq!(
            rendered,
            "[inject] seam=T0 settle=Ready rows=20 payload_on_screen=false payload_rows=0 \
             | idx=8:chrome 8 | idx=10:chrome 10 | idx=12:chrome 12 | idx=14:chrome 14 \
             | idx=16:chrome 16 | idx=18:chrome 18"
        );
    }

    /// T9 - the settle readiness is rendered as given.
    #[test]
    fn seam_observation_renders_settle_readiness() {
        let payload = wake_wrap(SEAM_FROM, "body");
        assert!(
            format_submit_seam_observation("T1", SettleReadiness::TimedOut, None, &payload)
                .contains(" settle=TimedOut ")
        );
        assert_eq!(
            format_submit_seam_observation("T1", SettleReadiness::Unknown, None, &payload),
            "[inject] seam=T1 settle=Unknown rows=unavailable"
        );
        assert!(
            format_submit_seam_observation("T1", SettleReadiness::Ready, Some(&[]), &payload)
                .starts_with("[inject] seam=T1 settle=Ready rows=0 ")
        );
    }

    /// T7a - a multirow body: each body line is its own row and is redacted.
    #[test]
    fn seam_redaction_hides_each_row_of_a_multirow_body() {
        let body = "line one of the body\nline two of the body";
        let screen = rows(&[
            "line one of the body",
            "line two of the body",
            "? for shortcuts",
            "\u{273B} Welcome to Claude Code!",
        ]);
        assert_fixture_is_screen_shaped(&screen);
        assert!(body.contains(&screen[0]) && body.contains(&screen[1]));
        let rendered = observe(body, &screen);
        assert!(!rendered.contains("line one") && !rendered.contains("line two"));
        assert!(rendered.contains(" payload_on_screen=true payload_rows=2 "));
        assert!(rendered.contains("idx=0:<payload> chars=20 marker=false"));
        assert!(rendered.contains("idx=1:<payload> chars=20 marker=false"));
        assert!(rendered.contains("idx=2:? for shortcuts"));
        assert!(rendered.contains("idx=3:\u{273B} Welcome to Claude Code!"));
    }

    /// T7b - a wrapped row: a 120-char body cut at char 47 into two rows.
    #[test]
    fn seam_redaction_hides_both_halves_of_a_wrapped_row() {
        let body: String = "the quick brown fox jumps over a lazy dog while seven wizards \
                            box nimbly and quartz judges vex the grumpy sphinx of black"
            .chars()
            .take(120)
            .collect();
        assert_eq!(body.chars().count(), 120);
        let head: String = body.chars().take(47).collect();
        let tail: String = body.chars().skip(47).collect();
        let screen = vec![head.clone(), tail.clone()];
        assert_fixture_is_screen_shaped(&screen);
        assert!(body.contains(&head) && body.contains(&tail));
        let rendered = observe(&body, &screen);
        assert!(rendered.contains(" payload_rows=2 "), "{rendered}");
        assert!(!rendered.contains(tail.trim()) && !rendered.contains(head.trim()));
    }

    /// T7c - a short body is redacted by the substring clause.
    #[test]
    fn seam_redaction_hides_a_short_body_row() {
        let screen = rows(&["ok"]);
        assert!(wake_wrap(SEAM_FROM, "ok").contains(&screen[0]));
        let rendered = observe("ok", &screen);
        assert!(rendered.contains(" payload_on_screen=true payload_rows=1 "));
        assert!(rendered.ends_with("idx=0:<payload> chars=2 marker=false"));
    }

    /// T7d - chrome plus the first 12 body chars: not a payload substring, but
    /// it shares an 8-char window, so it is redacted.
    #[test]
    fn seam_redaction_hides_a_suffix_only_overlap() {
        let body = "deliver the parcel to the north gate";
        let row = format!("* Thinking... {}", &body[..12]);
        let payload = wake_wrap(SEAM_FROM, body);
        assert!(row.ends_with("deliver the "));
        assert!(!payload.contains(row.trim()));
        let rendered = observe(body, std::slice::from_ref(&row));
        assert!(rendered.contains(" payload_rows=1 "), "{rendered}");
        assert!(!rendered.contains("deliver"));
    }

    /// T7e - the STATED leak limit, not a no-leak guarantee: a row sharing only
    /// six consecutive body chars is printed verbatim. Strengthening the rule
    /// must edit this test deliberately.
    #[test]
    fn seam_redaction_leak_limit_prints_a_six_char_fragment() {
        let body = "deliver the parcel to the north gate";
        let screen = rows(&["Tokens:parcel|42"]);
        assert!(screen[0].contains("parcel") && body.contains("parcel"));
        let rendered = observe(body, &screen);
        assert!(rendered.contains(" payload_on_screen=false payload_rows=0 "));
        assert!(rendered.ends_with("idx=0:Tokens:parcel|42"), "{rendered}");
    }

    /// T7f - a redacted row keeps its composer-marker signal.
    #[test]
    fn seam_redaction_keeps_the_composer_marker() {
        let body = "deliver the parcel to the north gate";
        let screen = rows(&["\u{276F} deliver the parcel", "deliver the parcel to"]);
        assert!(body.contains("deliver the parcel"));
        let rendered = observe(body, &screen);
        assert!(
            rendered.contains("idx=0:<payload> chars=20 marker=true"),
            "{rendered}"
        );
        assert!(
            rendered.contains("idx=1:<payload> chars=21 marker=false"),
            "{rendered}"
        );
    }

    /// T7g - the absence leg: chrome-only rows all survive verbatim.
    #[test]
    fn seam_redaction_leaves_chrome_only_rows_verbatim() {
        let body = "deliver the parcel to the north gate";
        let screen = rows(&[
            "? for shortcuts",
            "\u{273B} Welcome to Claude Code!",
            "\u{276F}",
        ]);
        let payload = wake_wrap(SEAM_FROM, body);
        for row in &screen {
            assert!(!payload.contains(row.as_str()), "{row}");
        }
        assert_eq!(
            observe(body, &screen),
            "[inject] seam=T0 settle=Ready rows=3 payload_on_screen=false payload_rows=0 \
             | idx=0:? for shortcuts | idx=1:\u{273B} Welcome to Claude Code! | idx=2:\u{276F}"
        );
    }
}
