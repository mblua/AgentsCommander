use std::collections::HashSet;
use std::io::Write as IoWrite;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::telegram::api;
use crate::telegram::types::BridgeInfo;

// ── File logger ──────────────────────────────────────────────

struct BridgeLogger {
    file: Option<std::fs::File>,
}

impl BridgeLogger {
    fn new(session_id: &str) -> Self {
        let file = crate::config::config_dir()
            .and_then(|dir| {
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

    fn log(&mut self, direction: &str, session_id: &str, text: &str) {
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
            let _ = writeln!(f, "[{}] {} sid={} | {}", now, direction, session_id, preview);
            let _ = f.flush();
        }
    }
}

// ── Diagnostic logger (full capture, no truncation) ─────────

struct DiagLogger {
    raw_file: Option<std::fs::File>,
    sent_file: Option<std::fs::File>,
}

impl DiagLogger {
    fn new() -> Self {
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

        Self { raw_file, sent_file }
    }

    /// Log stabilized rows (post-stabilization, pre-agent-filter)
    fn log_raw(&mut self, text: &str) {
        if let Some(ref mut f) = self.raw_file {
            let now = chrono::Utc::now().format("%H:%M:%S%.3f");
            let _ = writeln!(f, "--- [{}] ---", now);
            let _ = writeln!(f, "{}", text);
            let _ = f.flush();
        }
    }

    /// Log what actually gets sent to Telegram
    fn log_sent(&mut self, text: &str) {
        if let Some(ref mut f) = self.sent_file {
            let now = chrono::Utc::now().format("%H:%M:%S%.3f");
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
            let cleaned = strip_trailing_spinner(&cleaned);

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

            // Skip if we already emitted this content (scroll dedup).
            // Normalize whitespace so re-rendered content with different spacing
            // from the 220-col vt100 screen still matches.
            let normalized = normalize_whitespace(&row.content);
            if self.emitted_content.contains(&normalized) {
                continue;
            }

            // Apply agent-specific filter
            if filter.keep_line(&row.content) {
                self.emitted_content.insert(normalized);
                result.push(row.content.clone());
            }
        }

        // Prevent unbounded growth of emitted_content
        if self.emitted_content.len() > 5000 {
            self.emitted_content.clear();
        }

        result
    }

    /// Mark content as already emitted (e.g., user input captured on Enter)
    fn mark_emitted(&mut self, content: &str) {
        self.emitted_content.insert(normalize_whitespace(content));
    }

    /// Returns true if any row is unstable (changed recently, not yet emitted)
    fn has_pending(&self) -> bool {
        self.rows
            .iter()
            .any(|r| !r.emitted && !r.content.is_empty())
    }
}

/// Normalize interior whitespace: collapse 2+ spaces/tabs into a single space.
/// The vt100 220-col screen causes extra spaces when content wraps within the
/// wide virtual terminal (e.g. "un        híbrido" instead of "un híbrido").
/// Used for dedup comparison so re-rendered content with different spacing matches.
fn normalize_whitespace(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.trim().chars() {
        if c.is_whitespace() {
            if !prev_space {
                result.push(' ');
            }
            prev_space = true;
        } else {
            result.push(c);
            prev_space = false;
        }
    }
    result
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

/// Strip trailing spinner animation from a vt100 row.
/// Claude Code places spinner text (e.g. `✶ Gallivanting…`) at the right edge
/// of the 220-col screen. This gets concatenated with real content on the same row.
/// Pattern: {5+ spaces} {spinner_char} {text}{…}
///
/// IMPORTANT: ● (U+25CF) is both a spinner char AND the Claude Code response
/// indicator (e.g. `● Response text`). To avoid cutting entire response lines,
/// we require the spinner char to be preceded by at least 5 spaces (indicating
/// it's at the right edge, not at the start of content).
fn strip_trailing_spinner(s: &str) -> String {
    let mut best_cut = None;

    for (i, c) in s.char_indices() {
        if !CLAUDE_SPINNERS.contains(&c) {
            continue;
        }

        // Require significant whitespace gap before the spinner char.
        // This prevents matching ● at position 0 in response lines.
        if i < 5 {
            continue;
        }
        let preceding = &s[..i];
        let content_end = preceding.trim_end().len();
        let gap = i - content_end;
        if gap < 5 {
            continue;
        }

        // Check if the rest of the line ends with … or ...
        let after = s[i + c.len_utf8()..].trim_end();
        if after.ends_with('\u{2026}') || after.ends_with("...") {
            best_cut = Some(content_end);
        }
    }

    if let Some(cut) = best_cut {
        s[..cut].trim_end().to_string()
    } else {
        s.to_string()
    }
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
    // Interrupt/cancel hints
    "esc to interrupt",
    "esc to cancel",
    // Auto-compact notifications
    "auto-compact",
    "compacted conversation",
    // Memory updates
    "memory updated",
];

/// Claude Code spinner characters (defense in depth - stabilization is primary)
const CLAUDE_SPINNERS: &[char] = &['\u{273B}', '\u{2736}', '*', '\u{2722}', '\u{00B7}', '\u{25CF}', '\u{273D}'];
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

        // Tool execution chrome: ⎿  Running…, ⎿  Running… (3s), ⎿  (no content)
        // Also filter bare ⎿ (visual bracket with no content)
        // Note: ⎿ followed by actual tool output (file contents, command results) should pass.
        if trimmed.starts_with("\u{23BF}") {
            if trimmed == "\u{23BF}"
                || trimmed.contains("Running")
                || trimmed.contains("(no content)")
            {
                return false;
            }
        }

        // Tool headers: ●─Bash(...), ●─Read(...), ●─Skill(...), etc.
        // ●─ prefix always means tool header.
        if trimmed.starts_with("\u{25CF}\u{2500}") {
            return false;
        }
        // When ● is on a separate vt100 row, tool header starts with ─
        // followed by uppercase letter + parenthesis: ─Bash(, ─Read(, ─Agent(
        // Require ( to avoid false positives on CLI output like "─ Build failed"
        if trimmed.starts_with('\u{2500}') {
            let after = &trimmed['\u{2500}'.len_utf8()..];
            if after.starts_with(|c: char| c.is_ascii_uppercase()) && after.contains('(') {
                return false;
            }
        }

        // Collect non-space chars once for box-drawing and alphanumeric checks
        let non_space: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();

        // Box-drawing lines (separators)
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
        if trimmed.contains("(running stop hook")
            || trimmed.contains("(running start hook")
        {
            return false;
        }

        // Low alphanumeric ratio (progress bars, decorative lines, garbled rows)
        // Do NOT count spaces - garbled rows like "────────❯          ue─bajar."
        // have many spaces that inflate the ratio.
        let total_ns: usize = non_space.chars().count();
        if total_ns > 5 {
            let alnum: usize = non_space
                .chars()
                .filter(|c| c.is_alphanumeric())
                .count();
            if (alnum as f32 / total_ns as f32) < 0.30 {
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

    let had_spinner_prefix = s.chars().next().map(|c| CLAUDE_SPINNERS.contains(&c)).unwrap_or(false);

    let check = if had_spinner_prefix {
        s[s.chars().next().unwrap().len_utf8()..].trim()
    } else {
        s
    };

    if check.ends_with('\u{2026}') || check.ends_with("...") {
        let word_part = check.trim_end_matches('\u{2026}').trim_end_matches("...");
        if word_part.is_empty() || word_part.len() >= 50 || word_part.contains(' ') {
            return false;
        }
        // Without a spinner prefix (e.g. bare "Gallivanting…"), a single word
        // ending in ellipsis is always a spinner animation - safe to filter.
        if !had_spinner_prefix {
            return true;
        }
        // With a spinner prefix (e.g. "● Brewing…"), we must be careful:
        // ● is also the Claude response indicator. Short single-word responses
        // like "● Confirmed…" or "● Done…" are real content.
        // Claude's spinner verbs are invented/unusual words (Brewing, Gallivanting,
        // Pontificating). We filter only if the word starts with uppercase
        // (spinner verbs always do) and contains no common short-word pattern.
        // This is defense-in-depth: stabilization is the primary mechanism.
        if word_part.starts_with(|c: char| c.is_uppercase()) {
            return true;
        }
    }

    false
}

// ── Bridge spawn ─────────────────────────────────────────────

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
    typing_flag: Arc<AtomicBool>,
) -> BridgeHandle {
    let cancel = CancellationToken::new();
    let (tx, rx) = mpsc::channel::<Vec<u8>>(256);

    let session_id_str = session_id.to_string();

    // Output task: PTY bytes -> vt100 -> stabilize -> filter -> Telegram
    tokio::spawn(output_task(
        rx,
        bot_token.clone(),
        chat_id,
        session_id_str.clone(),
        cancel.clone(),
        app_handle.clone(),
        typing_flag,
    ));

    // Poll task: Telegram getUpdates -> write to PTY stdin
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
    typing_flag: Arc<AtomicBool>,
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

    // Typing suppression: when the terminal user is typing, suppress output
    // until they press Enter. Auto-clears after TYPING_TIMEOUT_MS as safety net.
    const TYPING_TIMEOUT_MS: u64 = 5000;
    let mut typing_since: Option<Instant> = None;
    let mut was_typing = false;

    logger.log("INIT", &session_id, &format!(
        "output_task started: filter={} stabilization={}ms tick={}ms",
        filter.name(), STABILIZATION_MS, TICK_MS,
    ));

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,

            // Periodic tick: harvest stabilized rows, flush buffer if ready
            _ = tick.tick() => {
                // Check typing suppression
                let is_typing = typing_flag.load(Ordering::Relaxed);
                if is_typing {
                    was_typing = true;
                    if let Some(since) = typing_since {
                        // Auto-clear after timeout (safety net)
                        if since.elapsed() > Duration::from_millis(TYPING_TIMEOUT_MS) {
                            typing_flag.store(false, Ordering::Relaxed);
                            typing_since = None;
                            was_typing = false;
                            logger.log("TYPING", &session_id, "auto-cleared after timeout");
                        } else {
                            // Still typing - skip harvest but keep updating tracker
                            continue;
                        }
                    } else {
                        typing_since = Some(Instant::now());
                        continue;
                    }
                } else {
                    // Transition: was typing → Enter pressed
                    // Capture user input from the vt100 screen (the ❯ line)
                    if was_typing {
                        was_typing = false;

                        // Scan screen bottom-to-top for the most recent prompt line
                        let screen = vt.screen();
                        for row_idx in (0..screen.size().0).rev() {
                            let row_text = screen.contents_between(
                                row_idx, 0, row_idx, screen.size().1,
                            );
                            let cleaned = strip_trailing_decoration(&row_text);
                            let cleaned = strip_trailing_spinner(&cleaned);
                            let trimmed = cleaned.trim();
                            if trimmed.starts_with("\u{276F} ") {
                                let user_input = trimmed["\u{276F} ".len()..].trim();
                                if !user_input.is_empty() {
                                    let msg = format!("\u{276F} {}", user_input);
                                    logger.log("USER_INPUT", &session_id, &msg);
                                    diag.log_sent(&msg);
                                    // Mark this content as emitted so it won't be sent again
                                    tracker.mark_emitted(&cleaned);
                                    buffer.push_str(&msg);
                                    buffer.push('\n');
                                    last_buffer_add = Instant::now();
                                }
                                break;
                            }
                        }
                    }
                    typing_since = None;
                }

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
            &mut buffer, &client, &token, chat_id,
            &session_id, &app, &mut logger, &mut diag,
        )
        .await;
    }
}

// ── Flush to Telegram ────────────────────────────────────────

async fn flush_buffer(
    buffer: &mut String,
    client: &reqwest::Client,
    token: &str,
    chat_id: i64,
    session_id: &str,
    app: &tauri::AppHandle,
    logger: &mut BridgeLogger,
    diag: &mut DiagLogger,
) {
    let text = std::mem::take(buffer);
    // Deduplicate consecutive identical lines
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
    let text = lines.join("\n");
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }

    for chunk in chunk_text(&text, 4000) {
        logger.log("SEND_TG", session_id, &chunk);
        diag.log_sent(&chunk);

        if let Err(e) = api::send_message(client, token, chat_id, &chunk).await {
            logger.log("SEND_ERR", session_id, &e.to_string());
            log::error!("Telegram send error for session {}: {}", session_id, e);
            let _ = app.emit(
                "telegram_bridge_error",
                serde_json::json!({
                    "sessionId": session_id,
                    "error": e.to_string(),
                }),
            );
        }
        // Rate limit: 35ms between sends
        tokio::time::sleep(Duration::from_millis(35)).await;
    }
}

fn chunk_text(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = (start + max_len).min(text.len());
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
    pty_mgr: Arc<Mutex<PtyManager>>,
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
                    &format!(
                        "skipped {} old messages, offset={}",
                        updates.len(),
                        offset
                    ),
                );
            }
        }
        Err(e) => {
            logger.log("POLL_ERR", &session_id_str, &e.to_string());
            log::warn!("Initial getUpdates failed: {}", e);
        }
    }

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            result = api::get_updates(&client, &token, offset, 5) => {
                match result {
                    Ok(updates) => {
                        for update in updates {
                            offset = update.update_id + 1;

                            if update.chat_id != chat_id {
                                logger.log("POLL_SKIP", &session_id_str, &format!("wrong chat_id={} from={}", update.chat_id, update.from_name));
                                continue;
                            }

                            logger.log("RECV_TG", &session_id_str, &format!("from={} text={}", update.from_name, update.text));

                            let input = format!("{}\r", update.text);
                            if let Ok(mgr) = pty_mgr.lock() {
                                if let Err(e) = mgr.write(session_id, input.as_bytes()) {
                                    logger.log("PTY_ERR", &session_id_str, &e.to_string());
                                    log::error!("Failed to write Telegram input to PTY: {}", e);
                                }
                            }

                            let _ = app.emit(
                                "telegram_incoming",
                                serde_json::json!({
                                    "sessionId": session_id_str,
                                    "text": update.text,
                                    "from": update.from_name,
                                }),
                            );

                            // Persist last prompt in backend + emit to all windows
                            let tg_prompt = format!("[TG] {}", update.text);
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
                        logger.log("POLL_ERR", &session_id_str, &e.to_string());
                        log::error!("Telegram poll error: {}", e);
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                }
            }
        }
    }
}
