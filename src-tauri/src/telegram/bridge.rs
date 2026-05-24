use std::collections::HashSet;
use std::io::Write as IoWrite;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::telegram::api;
use crate::telegram::types::BridgeInfo;

// ── #280 §3.2 — error classification + throttling ─────────────

/// Coarse classification of Telegram API failure modes. Used to tag log
/// lines so an operator can grep by kind and to drive the throttle below
/// (`Network` errors burst; the rare kinds emit unthrottled).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TelegramErrKind {
    Network,
    Unauthorized,
    Conflict,
    RateLimited,
    Other,
}

impl TelegramErrKind {
    /// Substring-match classification against `AppError::Telegram` strings
    /// (already redacted by api.rs §1.3 — token shape never affects
    /// classification). Order matters: precise API-side errors must be
    /// tested before the generic transport-error catchall.
    fn classify(msg: &str) -> Self {
        let lc = msg.to_lowercase();
        if lc.contains("unauthorized") {
            Self::Unauthorized
        } else if lc.contains("conflict") {
            Self::Conflict
        } else if lc.contains("too many requests") || lc.contains("429") {
            Self::RateLimited
        } else if lc.contains("error sending request") || lc.contains("timed out") {
            Self::Network
        } else {
            Self::Other
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Unauthorized => "unauthorized",
            Self::Conflict => "conflict",
            Self::RateLimited => "rate_limited",
            Self::Other => "other",
        }
    }
}

/// Per-task throttle. Coalesces bursts of same-kind errors into a single
/// ERROR plus a periodic re-emit with `(suppressed_repeats=N)` or
/// `(previously_suppressed=N kind=<prev>)` so the count is attributed to
/// the right kind even when the dominant kind changes mid-burst
/// (G-LOW-2 fix). Owned by `poll_task`'s local scope; never shared
/// across tasks.
struct ErrorThrottle {
    window: Duration,
    last_emit: Mutex<Option<(TelegramErrKind, Instant)>>,
    suppressed: AtomicU32,
}

impl ErrorThrottle {
    fn new(window: Duration) -> Self {
        Self {
            window,
            last_emit: Mutex::new(None),
            suppressed: AtomicU32::new(0),
        }
    }

    /// Returns `(should_emit, prior_kind)`:
    ///
    /// * `should_emit = true` — emit the ERROR line, reset the suppressed
    ///   counter when formatting the suffix.
    /// * `should_emit = false` — increment the suppressed counter and emit
    ///   at DEBUG instead.
    ///
    /// `prior_kind` is the kind that was in the slot before this call,
    /// captured so the suffix can correctly attribute a count when the
    /// dominant kind changes across the window boundary (G-LOW-2).
    fn check(&self, kind: TelegramErrKind) -> (bool, Option<TelegramErrKind>) {
        let mut guard = self.last_emit.lock().unwrap_or_else(|e| e.into_inner());
        let prior_kind = guard.map(|(k, _)| k);
        let should_emit = match *guard {
            Some((prev_kind, t)) if prev_kind == kind && t.elapsed() < self.window => false,
            _ => {
                *guard = Some((kind, Instant::now()));
                true
            }
        };
        (should_emit, prior_kind)
    }
}

// ── File logger ──────────────────────────────────────────────

pub(super) struct BridgeLogger {
    file: Option<std::fs::File>,
}

impl BridgeLogger {
    pub(super) fn new(session_id: &str) -> Self {
        let file = crate::config::config_dir().and_then(|dir| {
            std::fs::create_dir_all(&dir).ok()?;
            let path = dir.join("telegram-bridge.log");
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok()
        });

        if let Some(ref f) = file {
            let path = f.metadata().ok();
            log::info!(
                "Bridge logger active for session {} ({} bytes)",
                session_id,
                path.map(|m| m.len()).unwrap_or(0)
            );
        }

        Self { file }
    }

    pub(super) fn log(&mut self, direction: &str, session_id: &str, text: &str) {
        if let Some(ref mut f) = self.file {
            let now = chrono::Utc::now().format("%H:%M:%S%.3f");
            let preview = if text.len() > 500 {
                let mut end = 500;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}...[{}b total]", &text[..end], text.len())
            } else {
                text.to_string()
            };
            // #280 G-MED-3 — telegram-bridge.log is outside env_logger's
            // sink scrub. Scrub here so secret-bearing error strings from
            // any future caller cannot land in the bridge log.
            let preview = crate::telegram::redact::redact(&preview);
            let _ = writeln!(
                f,
                "[{}] {} sid={} | {}",
                now, direction, session_id, preview
            );
            let _ = f.flush();
        }
    }
}

// ── Diagnostic logger (full capture, no truncation) ─────────

pub(super) struct DiagLogger {
    raw_file: Option<std::fs::File>,
    sent_file: Option<std::fs::File>,
}

impl DiagLogger {
    pub(super) fn new() -> Self {
        let dir = crate::config::config_dir();

        let open = |name: &str| -> Option<std::fs::File> {
            let dir = dir.as_ref()?;
            std::fs::create_dir_all(dir).ok()?;
            let path = dir.join(name);
            std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)
                .ok()
        };

        let raw_file = open("diag-raw.log");
        let sent_file = open("diag-sent.log");

        if raw_file.is_some() && sent_file.is_some() {
            log::info!("Diagnostic logger active: diag-raw.log + diag-sent.log");
        }

        Self {
            raw_file,
            sent_file,
        }
    }

    /// Log stabilized rows (post-stabilization, pre-agent-filter)
    pub(super) fn log_raw(&mut self, text: &str) {
        if let Some(ref mut f) = self.raw_file {
            let now = chrono::Utc::now().format("%H:%M:%S%.3f");
            // #280 G-MED-3 — diag-raw.log lives outside env_logger's sink
            // scrub. Scrub here so secret-bearing content (agent output
            // containing a Telegram URL, or any future call site that
            // bypassed api.rs) cannot land in the diag log.
            let text = crate::telegram::redact::redact(text);
            let _ = writeln!(f, "--- [{}] ---", now);
            let _ = writeln!(f, "{}", text);
            let _ = f.flush();
        }
    }

    /// Log what actually gets sent to Telegram
    pub(super) fn log_sent(&mut self, text: &str) {
        if let Some(ref mut f) = self.sent_file {
            let now = chrono::Utc::now().format("%H:%M:%S%.3f");
            // #280 G-MED-3 — same scrub as log_raw; diag-sent.log bypasses
            // env_logger.
            let text = crate::telegram::redact::redact(text);
            let _ = writeln!(f, "--- [{}] ---", now);
            let _ = writeln!(f, "{}", text);
            let _ = f.flush();
        }
    }
}

// ── Row Tracker (stabilization-based diffing) ────────────────
//
// Instead of HashSet diffing (which emits every character change),
// track each screen row by position and only emit when the row
// has been STABLE (unchanged) for a configurable duration.
//
// This naturally filters:
//   - Spinner animations: change every ~450ms, never stabilize at 800ms
//   - Character-by-character streaming: only final line emitted
//   - TUI redraws: transient states never stabilize

struct RowState {
    content: String,
    last_changed: Instant,
    emitted: bool,
}

struct RowTracker {
    rows: Vec<RowState>,
    /// Content strings already emitted (prevents re-emission on scroll)
    emitted_content: HashSet<String>,
    stabilization: Duration,
}

impl RowTracker {
    fn new(num_rows: u16, stabilization_ms: u64) -> Self {
        let now = Instant::now();
        let mut rows = Vec::with_capacity(num_rows as usize);
        for _ in 0..num_rows {
            rows.push(RowState {
                content: String::new(),
                last_changed: now,
                emitted: true,
            });
        }
        Self {
            rows,
            emitted_content: HashSet::new(),
            stabilization: Duration::from_millis(stabilization_ms),
        }
    }

    /// Update row states from current vt100 screen
    fn update_from_screen(&mut self, screen: &vt100::Screen) {
        let now = Instant::now();
        for row_idx in 0..screen.size().0 {
            let row_text = screen.contents_between(row_idx, 0, row_idx, screen.size().1);
            let cleaned = strip_trailing_decoration(&row_text);

            let idx = row_idx as usize;
            if idx < self.rows.len() && self.rows[idx].content != cleaned {
                self.rows[idx].content = cleaned.to_string();
                self.rows[idx].last_changed = now;
                self.rows[idx].emitted = false;
            }
        }
    }

    /// Harvest rows that have been stable long enough.
    /// Applies agent filter and deduplicates against previously emitted content.
    /// Returns lines ready for Telegram.
    fn harvest_stable(&mut self, filter: &dyn AgentFilter) -> Vec<String> {
        let now = Instant::now();
        let mut result = Vec::new();

        for row in &mut self.rows {
            if row.emitted || row.content.is_empty() {
                continue;
            }
            if now.duration_since(row.last_changed) < self.stabilization {
                continue;
            }

            row.emitted = true;

            // Skip if we already emitted this exact content (scroll dedup)
            if self.emitted_content.contains(&row.content) {
                continue;
            }

            // Apply agent-specific filter
            if filter.keep_line(&row.content) {
                self.emitted_content.insert(row.content.clone());
                result.push(row.content.clone());
            }
        }

        // Prevent unbounded growth of emitted_content
        if self.emitted_content.len() > 5000 {
            self.emitted_content.clear();
        }

        result
    }

    /// Returns true if any row is unstable (changed recently, not yet emitted)
    fn has_pending(&self) -> bool {
        self.rows
            .iter()
            .any(|r| !r.emitted && !r.content.is_empty())
    }
}

/// Strip trailing box-drawing characters and whitespace from a vt100 row.
/// Claude Code's TUI places separators (─━═) at the right edge of the screen.
/// When vt100 reads the full 220-col row, these get concatenated with content.
fn strip_trailing_decoration(s: &str) -> String {
    let trimmed = s.trim_end();
    let result = trimmed.trim_end_matches(|c: char| {
        // Box-drawing: ─━═│┃┌┐└┘├┤┬┴┼╔╗╚╝╠╣╦╩╬
        "\u{2500}\u{2501}\u{2550}\u{2502}\u{2503}\u{250C}\u{2510}\u{2514}\u{2518}\u{251C}\u{2524}\u{252C}\u{2534}\u{253C}\u{2554}\u{2557}\u{255A}\u{255D}\u{2560}\u{2563}\u{2566}\u{2569}\u{256C}".contains(c)
    });
    result.trim_end().to_string()
}

// ── Agent Filter (pluggable per coding agent) ────────────────
//
// The AgentFilter trait allows different filtering rules for
// different coding agents (Claude Code, Codex, Aider, etc.)
//
// With stabilization in place, spinners are already eliminated
// (they never stabilize). The agent filter handles static noise:
// TUI chrome, status bars, prompt markers, etc.

trait AgentFilter: Send + Sync {
    fn keep_line(&self, line: &str) -> bool;
    fn name(&self) -> &str;
}

// ── Claude Code Filter ───────────────────────────────────────

struct ClaudeCodeFilter;

/// Patterns that indicate Claude Code TUI chrome
///
/// IMPORTANT: Do NOT add model names like "Opus 4" here - they match
/// conversation content when Claude mentions its own model. Use status-bar-
/// specific patterns instead (e.g. "] │" which only appears in the header).
const CLAUDE_CHROME_PATTERNS: &[&str] = &[
    "bypass permissions",
    "shift+tab to cycle",
    "shift+tab to change",
    "ctrl+b to run in background",
    "/doctor for",
    "settings issue",
    "Tip: ",
    "Context \u{2591}", // ░ progress bar
    "Context \u{2588}", // █ usage bar
    "Usage \u{2591}",
    "Usage \u{2588}",
    "(syncing...)",
    "(resets in",
    "Claude in Chrome enabled",
    "Claude Code v",
    // Status bar header: "[Model (context) | Plan] │ branch"
    // The "] │" pattern catches this without matching conversation content
    "] \u{2502}",
];

/// Claude Code spinner characters (defense in depth - stabilization is primary)
const CLAUDE_SPINNERS: &[char] = &[
    '\u{273B}', '\u{2736}', '*', '\u{2722}', '\u{00B7}', '\u{25CF}', '\u{273D}',
];
// ✻ ✶ * ✢ · ● ✽

impl AgentFilter for ClaudeCodeFilter {
    fn keep_line(&self, line: &str) -> bool {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            return false;
        }

        // TUI chrome patterns
        for pattern in CLAUDE_CHROME_PATTERNS {
            if trimmed.contains(pattern) {
                return false;
            }
        }

        // Box-drawing lines (separators)
        let non_space: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
        if non_space.len() > 5
            && non_space
                .chars()
                .all(|c| "\u{2500}\u{2501}\u{2550}\u{2502}\u{2503}\u{250C}\u{2510}\u{2514}\u{2518}\u{251C}\u{2524}\u{252C}\u{2534}\u{253C}\u{2554}\u{2557}\u{255A}\u{255D}\u{2560}\u{2563}\u{2566}\u{2569}\u{256C}".contains(c))
        {
            // ─━═│┃┌┐└┘├┤┬┴┼╔╗╚╝╠╣╦╩╬
            return false;
        }

        // Braille spinners (U+2800..U+28FF)
        if trimmed
            .chars()
            .next()
            .map(|c| ('\u{2800}'..='\u{28FF}').contains(&c))
            .unwrap_or(false)
        {
            return false;
        }

        // Hook notifications
        if trimmed.contains("(running stop hook") || trimmed.contains("(running start hook") {
            return false;
        }

        // Low alphanumeric ratio (progress bars, decorative lines)
        let total: usize = trimmed.chars().count();
        if total > 5 {
            let alnum: usize = trimmed
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == ' ')
                .count();
            if (alnum as f32 / total as f32) < 0.30 {
                return false;
            }
        }

        // Prompt markers and user input echo.
        // Lines starting with ❯ are user input - the user already knows what
        // they typed (either from Telegram or from the terminal).
        // Filtering these also prevents streaming partial lines from being sent
        // (user pauses while typing cause partial lines to stabilize).
        if trimmed == "\u{276F}" || trimmed == ">" || trimmed.starts_with("\u{276F} ") {
            // ❯ or ❯ followed by text
            return false;
        }

        // ASCII art logo
        if trimmed.contains("\u{2590}\u{259B}")
            || trimmed.contains("\u{259D}\u{259C}")
            || trimmed.contains("\u{2598}\u{2598}")
        {
            // ▐▛ ▝▜ ▘▘
            return false;
        }

        // Defense in depth: thinking/spinner lines that somehow stabilized
        if is_thinking_line(trimmed) {
            return false;
        }

        true
    }

    fn name(&self) -> &str {
        "claude-code"
    }
}

/// Detect spinner/thinking animation lines.
/// Pattern: optional spinner char + single capitalized word + "..." or "\u{2026}"
/// Defense in depth - stabilization is the primary mechanism.
fn is_thinking_line(s: &str) -> bool {
    let s = s.trim();

    if s.contains("(thinking)") || s.contains("\u{27E1} thinking") {
        return true;
    }

    let check = if let Some(first) = s.chars().next() {
        if CLAUDE_SPINNERS.contains(&first) {
            s[first.len_utf8()..].trim()
        } else {
            s
        }
    } else {
        return false;
    };

    if check.ends_with('\u{2026}') || check.ends_with("...") {
        let word_part = check.trim_end_matches('\u{2026}').trim_end_matches("...");
        if !word_part.is_empty()
            && word_part
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
            && word_part.chars().all(|c| c.is_alphabetic())
        {
            return true;
        }
    }

    false
}

// ── Bridge spawn ─────────────────────────────────────────────

/// Which session-reader pipeline to spawn for this bridge. `None` (the bridge
/// signature) falls back to the PTY 6-phase pipeline (legacy noisy mode).
#[derive(Debug, Clone)]
pub enum SessionReaderKind {
    /// Watch Claude Code's append-only JSONL session log at the resolved projects dir.
    Claude { project_dir: PathBuf },
    /// Watch Codex CLI's append-only `rollout-*.jsonl` under `~/.codex/sessions/`,
    /// filtering candidates by `session_meta.cwd` match.
    Codex {
        search_root: PathBuf,
        cwd: String,
        attach_time: chrono::DateTime<chrono::Utc>,
    },
    /// Watch Gemini CLI's append-only `session-*.jsonl` under
    /// `~/.gemini/tmp/<slug>/chats/`. The slug is resolved lazily by the
    /// watcher on each poll via `lookup_chats_dir_for_cwd` until it succeeds.
    Gemini {
        gemini_home: PathBuf,
        cwd: String,
        attach_time: chrono::DateTime<chrono::Utc>,
    },
}

pub struct BridgeHandle {
    pub info: BridgeInfo,
    pub cancel: CancellationToken,
    pub output_sender: mpsc::Sender<Vec<u8>>,
}

pub fn spawn_bridge(
    bot_token: String,
    chat_id: i64,
    session_id: Uuid,
    info: BridgeInfo,
    pty_mgr: Arc<Mutex<PtyManager>>,
    app_handle: tauri::AppHandle,
    reader: Option<SessionReaderKind>,
) -> BridgeHandle {
    let cancel = CancellationToken::new();
    let (tx, rx) = mpsc::channel::<Vec<u8>>(256);

    let session_id_str = session_id.to_string();

    match reader {
        Some(SessionReaderKind::Claude { project_dir }) => {
            drop(rx); // not needed — no PTY bytes feed
            super::claude_watcher::spawn_watch_task(
                project_dir,
                bot_token.clone(),
                chat_id,
                session_id_str.clone(),
                cancel.clone(),
                app_handle.clone(),
            );
        }
        Some(SessionReaderKind::Codex {
            search_root,
            cwd,
            attach_time,
        }) => {
            drop(rx);
            super::codex_watcher::spawn_watch_task(
                search_root,
                cwd,
                attach_time,
                bot_token.clone(),
                chat_id,
                session_id_str.clone(),
                cancel.clone(),
                app_handle.clone(),
            );
        }
        Some(SessionReaderKind::Gemini {
            gemini_home,
            cwd,
            attach_time,
        }) => {
            drop(rx);
            super::gemini_watcher::spawn_watch_task(
                gemini_home,
                cwd,
                attach_time,
                bot_token.clone(),
                chat_id,
                session_id_str.clone(),
                cancel.clone(),
                app_handle.clone(),
            );
        }
        None => {
            // PTY mode: existing 6-phase pipeline
            tokio::spawn(output_task(
                rx,
                bot_token.clone(),
                chat_id,
                session_id_str.clone(),
                cancel.clone(),
                app_handle.clone(),
            ));
        }
    }

    // Poll task: Telegram getUpdates -> write to PTY stdin (runs in BOTH modes)
    tokio::spawn(poll_task(
        bot_token,
        chat_id,
        session_id,
        session_id_str,
        pty_mgr,
        cancel.clone(),
        app_handle,
    ));

    BridgeHandle {
        info,
        cancel,
        output_sender: tx,
    }
}

// ── Output task (PTY -> Telegram) ────────────────────────────
//
// Pipeline phases:
//   Phase 1: RAW BYTES   - PTY stdout chunks (Vec<u8>)
//   Phase 2: VT100 PARSE - vt100::Parser renders to virtual screen
//   Phase 3: STABILIZE   - RowTracker: emit only rows stable for 800ms+
//   Phase 4: FILTER      - AgentFilter: remove TUI chrome (agent-specific)
//   Phase 5: BUFFER      - Accumulate + dedup consecutive lines
//   Phase 6: SEND        - Chunk at 4000 chars, rate-limit, send to Telegram

const VT_ROWS: u16 = 50;
const VT_COLS: u16 = 220;
const STABILIZATION_MS: u64 = 800;
const TICK_MS: u64 = 200;
const FLUSH_DELAY_MS: u64 = 500;

async fn output_task(
    mut rx: mpsc::Receiver<Vec<u8>>,
    token: String,
    chat_id: i64,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle,
) {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let mut logger = BridgeLogger::new(&session_id);
    let mut diag = DiagLogger::new();
    let mut buffer = String::new();
    let mut last_buffer_add = Instant::now();
    let flush_delay = Duration::from_millis(FLUSH_DELAY_MS);

    // Phase 2: Virtual terminal parser
    let mut vt = vt100::Parser::new(VT_ROWS, VT_COLS, 0);

    // Phase 3: Row stabilization tracker
    let mut tracker = RowTracker::new(VT_ROWS, STABILIZATION_MS);

    // Phase 4: Agent-specific filter (currently only Claude Code)
    let filter: Box<dyn AgentFilter> = Box::new(ClaudeCodeFilter);

    // Tick interval for harvesting stabilized rows
    let mut tick = tokio::time::interval(Duration::from_millis(TICK_MS));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    logger.log(
        "INIT",
        &session_id,
        &format!(
            "output_task started: filter={} stabilization={}ms tick={}ms",
            filter.name(),
            STABILIZATION_MS,
            TICK_MS,
        ),
    );

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,

            // Periodic tick: harvest stabilized rows, flush buffer if ready
            _ = tick.tick() => {
                let stable_lines = tracker.harvest_stable(filter.as_ref());

                if !stable_lines.is_empty() {
                    let raw_text = stable_lines.join("\n");
                    diag.log_raw(&raw_text);
                    logger.log("STABLE", &session_id, &raw_text);

                    for line in &stable_lines {
                        buffer.push_str(line);
                        buffer.push('\n');
                    }
                    last_buffer_add = Instant::now();
                }

                // Flush buffer if enough time has passed since last addition
                if !buffer.is_empty() {
                    let since_last = last_buffer_add.elapsed();
                    let buf_len = buffer.trim().len();
                    if since_last >= flush_delay || buf_len > 2000 {
                        flush_buffer(
                            &mut buffer, &client, &token, chat_id,
                            &session_id, &app, &mut logger, &mut diag,
                            false,
                        ).await;
                    }
                }
            }

            // Phase 1: Receive raw PTY bytes
            maybe_data = rx.recv() => {
                match maybe_data {
                    Some(data) => {
                        // Phase 2: Process through virtual terminal
                        vt.process(&data);

                        // Phase 3: Update row tracker from screen state
                        tracker.update_from_screen(vt.screen());
                    }
                    None => break,
                }
            }
        }
    }

    // Final harvest + flush
    // Give a moment for any remaining rows to stabilize
    if tracker.has_pending() {
        tokio::time::sleep(Duration::from_millis(STABILIZATION_MS + 100)).await;
        let stable_lines = tracker.harvest_stable(filter.as_ref());
        if !stable_lines.is_empty() {
            for line in &stable_lines {
                buffer.push_str(line);
                buffer.push('\n');
            }
        }
    }
    if !buffer.is_empty() {
        flush_buffer(
            &mut buffer,
            &client,
            &token,
            chat_id,
            &session_id,
            &app,
            &mut logger,
            &mut diag,
            false,
        )
        .await;
    }
}

// ── Flush to Telegram ────────────────────────────────────────

// Threads through the full bridge state on each flush; collapsing into a
// struct would only move the fields, not reduce them.
#[allow(clippy::too_many_arguments)]
pub(super) async fn flush_buffer(
    buffer: &mut String,
    client: &reqwest::Client,
    token: &str,
    chat_id: i64,
    session_id: &str,
    app: &tauri::AppHandle,
    logger: &mut BridgeLogger,
    diag: &mut DiagLogger,
    skip_dedup: bool,
) {
    let text = std::mem::take(buffer);
    // Deduplicate consecutive identical lines (PTY mode only — screen redraws cause duplicates).
    // JSONL mode skips dedup because content is clean and legitimate repeated lines are valid.
    let text = if skip_dedup {
        text
    } else {
        let mut lines: Vec<&str> = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if lines.last().map(|l: &&str| l.trim()) != Some(trimmed) {
                lines.push(line);
            }
        }
        lines.join("\n")
    };
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }

    for chunk in chunk_text(&text, 4000) {
        logger.log("SEND_TG", session_id, &chunk);
        diag.log_sent(&chunk);

        if let Err(e) = api::send_message(client, token, chat_id, &chunk).await {
            let msg = e.to_string();
            let kind = TelegramErrKind::classify(&msg);
            let pid = std::process::id();
            // `token_prefix` is the numeric bot user id (e.g. "8336197840"),
            // not the secret. Telegram bot user ids are public via
            // @BotInfoBot — they're identifiers, useful for correlating
            // multi-bot deployments. The secret is the `:AAGB…` tail.
            let token_prefix = token.split(':').next().unwrap_or("?");
            logger.log("SEND_ERR", session_id, &msg);
            log::error!(
                "[bridge] Telegram send failed — kind={} session_id={} pid={} bot_id={} err={}",
                kind.as_str(),
                session_id,
                pid,
                token_prefix,
                msg
            );
            let _ = app.emit(
                "telegram_bridge_error",
                serde_json::json!({
                    "sessionId": session_id,
                    "error": msg,
                    "kind": kind.as_str(),
                }),
            );
        }
        // Rate limit: 35ms between sends
        tokio::time::sleep(Duration::from_millis(35)).await;
    }
}

pub(super) fn chunk_text(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max_len).min(text.len());
        // Snap backward to nearest char boundary to avoid UTF-8 slicing panic
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        let actual_end = if end < text.len() {
            text[start..end]
                .rfind('\n')
                .map(|i| start + i + 1)
                .unwrap_or(end)
        } else {
            end
        };
        chunks.push(text[start..actual_end].to_string());
        start = actual_end;
    }
    chunks
}

// ── Poll task (Telegram -> PTY) ──────────────────────────────

async fn poll_task(
    token: String,
    chat_id: i64,
    session_id: Uuid,
    session_id_str: String,
    _pty_mgr: Arc<Mutex<PtyManager>>,
    cancel: CancellationToken,
    app: tauri::AppHandle,
) {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();

    let mut logger = BridgeLogger::new(&session_id_str);
    let mut offset: i64 = 0;

    // Skip old messages
    match api::get_updates(&client, &token, 0, 0).await {
        Ok(updates) => {
            if let Some(last) = updates.last() {
                offset = last.update_id + 1;
                logger.log(
                    "POLL_INIT",
                    &session_id_str,
                    &format!("skipped {} old messages, offset={}", updates.len(), offset),
                );
            }
        }
        Err(e) => {
            let msg = e.to_string();
            let kind = TelegramErrKind::classify(&msg);
            logger.log("POLL_ERR", &session_id_str, &msg);
            log::warn!(
                "[bridge] Initial getUpdates failed — kind={} session_id={} err={}",
                kind.as_str(),
                session_id_str,
                msg
            );
        }
    }

    // #280 §3.2 — throttle bursty network errors in the poll loop. Created
    // once per task so the state is naturally per-bridge-instance; drops
    // when `poll_task` exits.
    let throttle = ErrorThrottle::new(Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            result = api::get_updates(&client, &token, offset, 5) => {
                match result {
                    Ok(updates) => {
                        for update in updates {
                            offset = update.update_id + 1;

                            if update.chat_id != chat_id {
                                logger.log("POLL_SKIP", &session_id_str, &format!("wrong chat_id={}", update.chat_id));
                                continue;
                            }

                            let inject_text = match update.content {
                                api::TelegramContent::Text(text) => {
                                    logger.log("RECV_TG", &session_id_str, &format!("from={} text={}", update.from_name, text));
                                    text
                                }
                                api::TelegramContent::Voice { file_id } => {
                                    logger.log("RECV_TG_VOICE", &session_id_str, &format!("from={} file_id={}", update.from_name, file_id));

                                    let settings = app.state::<crate::config::settings::SettingsState>();
                                    let cfg = settings.read().await;
                                    let api_key = cfg.gemini_api_key.clone();
                                    let model_raw = cfg.gemini_model.clone();
                                    drop(cfg);

                                    let model = if model_raw.is_empty() { "gemini-2.5-flash".to_string() } else { model_raw };

                                    if api_key.is_empty() {
                                        log::warn!("[bridge] Voice message received but no Gemini API key configured");
                                        let _ = api::send_message(&client, &token, chat_id, "Cannot transcribe voice: Gemini API key not configured").await;
                                        continue;
                                    }

                                    let file_path = match api::get_file(&client, &token, &file_id).await {
                                        Ok(fp) => fp,
                                        Err(e) => {
                                            logger.log("VOICE_ERR", &session_id_str, &format!("get_file failed: {}", e));
                                            let _ = api::send_message(&client, &token, chat_id, &format!("Failed to get voice file: {}", e)).await;
                                            continue;
                                        }
                                    };

                                    let audio_bytes = match api::download_file(&client, &token, &file_path).await {
                                        Ok(bytes) => bytes,
                                        Err(e) => {
                                            logger.log("VOICE_ERR", &session_id_str, &format!("download failed: {}", e));
                                            let _ = api::send_message(&client, &token, chat_id, &format!("Failed to download voice: {}", e)).await;
                                            continue;
                                        }
                                    };

                                    // Dedicated client with 30s timeout for Gemini (poll client is 15s)
                                    let gemini_client = reqwest::Client::builder()
                                        .timeout(std::time::Duration::from_secs(30))
                                        .build()
                                        .unwrap_or_default();

                                    match crate::commands::voice::transcribe_audio(&gemini_client, &audio_bytes, "audio/ogg", &api_key, &model).await {
                                        Ok(text) => {
                                            logger.log("VOICE_OK", &session_id_str, &format!("transcribed {} chars", text.len()));
                                            let _ = api::send_message(&client, &token, chat_id, &format!("Transcribed: {}", text)).await;
                                            text
                                        }
                                        Err(e) => {
                                            logger.log("VOICE_ERR", &session_id_str, &format!("transcription failed: {}", e));
                                            let _ = api::send_message(&client, &token, chat_id, &format!("Transcription failed: {}", e)).await;
                                            continue;
                                        }
                                    }
                                }
                            };

                            if let Err(e) = crate::pty::inject::inject_text_into_session(&app, session_id, &inject_text).await {
                                logger.log("PTY_ERR", &session_id_str, &e.to_string());
                                log::error!("Failed to write Telegram input to PTY: {}", e);
                            }

                            let _ = app.emit(
                                "telegram_incoming",
                                serde_json::json!({
                                    "sessionId": session_id_str,
                                    "text": inject_text,
                                    "from": update.from_name,
                                }),
                            );

                            let tg_prompt = format!("[TG] {}", inject_text);
                            {
                                let mgr_state = app.state::<std::sync::Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
                                let mgr = mgr_state.read().await;
                                if let Ok(uuid) = uuid::Uuid::parse_str(&session_id_str) {
                                    mgr.set_last_prompt(uuid, tg_prompt.clone()).await;
                                }
                            }
                            let _ = app.emit(
                                "last_prompt",
                                serde_json::json!({
                                    "text": tg_prompt,
                                    "sessionId": session_id_str,
                                }),
                            );
                        }
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        let kind = TelegramErrKind::classify(&msg);
                        let pid = std::process::id();
                        let token_prefix = token.split(':').next().unwrap_or("?");
                        logger.log("POLL_ERR", &session_id_str, &msg);

                        // #280 §3.2 — throttle Network errors (the burst
                        // shape that produced 7,872 of 11,140 ERROR lines
                        // in the live log). Unauthorized/Conflict/Rate-
                        // Limited are rare and signal-rich → always emit.
                        let should_throttle = matches!(kind, TelegramErrKind::Network);
                        let (should_emit, prior_kind) = throttle.check(kind);
                        if !should_throttle || should_emit {
                            let suppressed = throttle.suppressed.swap(0, Ordering::Relaxed);
                            // G-LOW-2 — attribute the suppressed count to
                            // the kind that was being suppressed, not the
                            // current emit's kind. When `prior_kind` and
                            // `kind` differ, surface that explicitly.
                            let suffix = match (suppressed, prior_kind) {
                                (0, _) => String::new(),
                                (n, Some(pk)) if pk != kind => format!(
                                    " (previously_suppressed={} kind={})",
                                    n,
                                    pk.as_str()
                                ),
                                (n, _) => format!(" (suppressed_repeats={})", n),
                            };
                            log::error!(
                                "[bridge] Telegram poll error — kind={} session_id={} pid={} bot_id={} err={}{}",
                                kind.as_str(),
                                session_id_str,
                                pid,
                                token_prefix,
                                msg,
                                suffix
                            );
                        } else {
                            throttle.suppressed.fetch_add(1, Ordering::Relaxed);
                            log::debug!(
                                "[bridge] Telegram poll error suppressed — kind={} session_id={} (in throttle window)",
                                kind.as_str(),
                                session_id_str
                            );
                        }
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── #280 §3.2 — TelegramErrKind::classify ────────────────────

    #[test]
    fn classify_unauthorized() {
        assert_eq!(
            TelegramErrKind::classify("Telegram error: Unauthorized"),
            TelegramErrKind::Unauthorized
        );
    }

    #[test]
    fn classify_conflict() {
        assert_eq!(
            TelegramErrKind::classify("Telegram error: Conflict: terminated by other getUpdates request"),
            TelegramErrKind::Conflict
        );
    }

    #[test]
    fn classify_rate_limited() {
        assert_eq!(
            TelegramErrKind::classify("Telegram error: Too Many Requests: retry after 5"),
            TelegramErrKind::RateLimited
        );
        assert_eq!(
            TelegramErrKind::classify("HTTP 429 Too Many Requests"),
            TelegramErrKind::RateLimited
        );
    }

    #[test]
    fn classify_network() {
        assert_eq!(
            TelegramErrKind::classify("error sending request for url (https://api.telegram.org/bot***/getUpdates)"),
            TelegramErrKind::Network
        );
        assert_eq!(
            TelegramErrKind::classify("operation timed out"),
            TelegramErrKind::Network
        );
    }

    #[test]
    fn classify_other_falls_through() {
        assert_eq!(
            TelegramErrKind::classify("something unexpected happened"),
            TelegramErrKind::Other
        );
    }

    // ── #280 §3.2 — ErrorThrottle ────────────────────────────────

    #[test]
    fn throttle_emits_on_first_kind_sighting() {
        let t = ErrorThrottle::new(Duration::from_secs(60));
        let (emit, prior) = t.check(TelegramErrKind::Network);
        assert!(emit, "first sighting must emit");
        assert!(prior.is_none());
    }

    #[test]
    fn throttle_suppresses_same_kind_within_window() {
        let t = ErrorThrottle::new(Duration::from_secs(60));
        let (emit1, _) = t.check(TelegramErrKind::Network);
        assert!(emit1);
        let (emit2, prior) = t.check(TelegramErrKind::Network);
        assert!(!emit2, "second sighting in window must suppress");
        assert_eq!(prior, Some(TelegramErrKind::Network));
    }

    /// G-LOW-2 — when a different kind breaks the window, the prior kind
    /// is returned so the caller can attribute the suppressed count to it
    /// rather than the new kind.
    #[test]
    fn throttle_returns_prior_kind_across_kind_change() {
        let t = ErrorThrottle::new(Duration::from_secs(60));
        let _ = t.check(TelegramErrKind::Network);
        let (emit, prior) = t.check(TelegramErrKind::Conflict);
        assert!(emit, "different kind must always emit");
        assert_eq!(
            prior,
            Some(TelegramErrKind::Network),
            "prior kind must reflect what was in the slot before this call"
        );
    }

    /// After the window elapses for the same kind, the throttle releases
    /// the next sighting. We use a very short window so the test is fast.
    #[test]
    fn throttle_emits_again_after_window_elapses() {
        let t = ErrorThrottle::new(Duration::from_millis(20));
        let (emit1, _) = t.check(TelegramErrKind::Network);
        assert!(emit1);
        std::thread::sleep(Duration::from_millis(40));
        let (emit2, prior) = t.check(TelegramErrKind::Network);
        assert!(emit2, "after window, same kind must re-emit");
        assert_eq!(prior, Some(TelegramErrKind::Network));
    }
}
