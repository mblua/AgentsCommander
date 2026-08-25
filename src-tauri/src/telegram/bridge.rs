use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::settings::TelegramNetworkPollErrorLogging;
use crate::network::{NetworkBackoff, OutboundNetwork};
use crate::pty::manager::PtyManager;
use crate::session::profile::CodingAgentKind;
use crate::telegram::api;
use crate::telegram::output::{flush_buffer, BridgeLogger, DiagLogger, TelegramErrKind};
use crate::telegram::types::BridgeInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollNetworkLogAction {
    Failure {
        level: log::Level,
        suppressed_since_last_emit: u32,
        sustained: bool,
    },
    Suppressed {
        level: log::Level,
    },
    Recovery {
        level: log::Level,
        outage_seconds: u64,
        suppressed_total: u32,
        sustained: bool,
    },
    None,
}

struct PollNetworkErrorState {
    first_failure_at: Option<Instant>,
    last_emit_at: Option<Instant>,
    suppressed_since_last_emit: u32,
    suppressed_total: u32,
    emitted_any_failure: bool,
    emitted_sustained: bool,
}

impl PollNetworkErrorState {
    fn new() -> Self {
        Self {
            first_failure_at: None,
            last_emit_at: None,
            suppressed_since_last_emit: 0,
            suppressed_total: 0,
            emitted_any_failure: false,
            emitted_sustained: false,
        }
    }

    fn record_network_failure(
        &mut self,
        policy: &TelegramNetworkPollErrorLogging,
    ) -> PollNetworkLogAction {
        let now = Instant::now();

        let Some(first) = self.first_failure_at else {
            self.first_failure_at = Some(now);
            self.last_emit_at = Some(now);
            self.emitted_any_failure = true;

            let sustained = policy.sustained_after_seconds == 0;
            if sustained {
                self.emitted_sustained = true;
            }

            return PollNetworkLogAction::Failure {
                level: if sustained {
                    policy.sustained_level.as_log_level()
                } else {
                    policy.first_failure_level.as_log_level()
                },
                suppressed_since_last_emit: 0,
                sustained,
            };
        };

        let sustained = now.saturating_duration_since(first)
            >= Duration::from_secs(policy.sustained_after_seconds);
        let repeat_ready = self
            .last_emit_at
            .map(|last| {
                now.saturating_duration_since(last)
                    >= Duration::from_secs(policy.sustained_repeat_seconds)
            })
            .unwrap_or(true);

        if sustained && repeat_ready {
            let suppressed = self.suppressed_since_last_emit;
            self.suppressed_since_last_emit = 0;
            self.last_emit_at = Some(now);
            self.emitted_any_failure = true;
            self.emitted_sustained = true;
            PollNetworkLogAction::Failure {
                level: policy.sustained_level.as_log_level(),
                suppressed_since_last_emit: suppressed,
                sustained: true,
            }
        } else {
            self.suppressed_since_last_emit = self.suppressed_since_last_emit.saturating_add(1);
            self.suppressed_total = self.suppressed_total.saturating_add(1);
            PollNetworkLogAction::Suppressed {
                level: policy.transient_repeat_level.as_log_level(),
            }
        }
    }

    fn record_success(&mut self, policy: &TelegramNetworkPollErrorLogging) -> PollNetworkLogAction {
        let Some(first) = self.first_failure_at else {
            return PollNetworkLogAction::None;
        };

        let outage_seconds = Instant::now().saturating_duration_since(first).as_secs();
        let should_log_recovery = self.emitted_any_failure;
        let suppressed_total = self.suppressed_total;
        let sustained = self.emitted_sustained;
        *self = Self::new();

        if should_log_recovery {
            PollNetworkLogAction::Recovery {
                level: policy.recovery_level.as_log_level(),
                outage_seconds,
                suppressed_total,
                sustained,
            }
        } else {
            PollNetworkLogAction::None
        }
    }

    fn reset_without_recovery(&mut self) {
        *self = Self::new();
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
        if is_box_drawing_line(&non_space) {
            // ─━═│┃┌┐└┘├┤┬┴┼╔╗╚╝╠╣╦╩╬
            return false;
        }

        // Braille spinners (U+2800..U+28FF)
        if starts_with_braille(trimmed) {
            return false;
        }

        // Hook notifications
        if trimmed.contains("(running stop hook") || trimmed.contains("(running start hook") {
            return false;
        }

        // Low alphanumeric ratio (progress bars, decorative lines)
        if is_low_alnum_line(trimmed) {
            return false;
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

/// Shared decoration check: a line whose non-space characters are ALL box-drawing
/// characters (U+2500..U+256C set) and longer than 5 chars is a separator line.
/// Extracted from `ClaudeCodeFilter::keep_line`; used by both filters with
/// identical semantics.
fn is_box_drawing_line(non_space: &str) -> bool {
    non_space.len() > 5
        && non_space.chars().all(|c| {
            "\u{2500}\u{2501}\u{2550}\u{2502}\u{2503}\u{250C}\u{2510}\u{2514}\u{2518}\u{251C}\u{2524}\u{252C}\u{2534}\u{253C}\u{2554}\u{2557}\u{255A}\u{255D}\u{2560}\u{2563}\u{2566}\u{2569}\u{256C}".contains(c)
        })
}

/// Shared decoration check: first char is a braille spinner glyph (U+2800..U+28FF).
/// Extracted from `ClaudeCodeFilter::keep_line`; used by both filters with
/// identical semantics.
fn starts_with_braille(s: &str) -> bool {
    s.chars()
        .next()
        .map(|c| ('\u{2800}'..='\u{28FF}').contains(&c))
        .unwrap_or(false)
}

/// Shared decoration check: low alphanumeric+space ratio (< 0.30, len > 5)
/// catches progress bars and decorative lines. Extracted from
/// `ClaudeCodeFilter::keep_line`; used by both filters with identical semantics.
fn is_low_alnum_line(trimmed: &str) -> bool {
    let total: usize = trimmed.chars().count();
    if total > 5 {
        let alnum: usize = trimmed
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == ' ')
            .count();
        return (alnum as f32 / total as f32) < 0.30;
    }
    false
}

// ── Antigravity (agy) Filter ────────────────────────────────

/// Chrome patterns for the Antigravity (agy) TUI, contains-based on the raw
/// line. `Google AI Pro` covers the vertical-logo account row
/// (`mariano.blua@gmail.com (Google AI Pro)`) — stable plan name, low collision.
const AGY_CHROME_PATTERNS: &[&str] = &[
    "? for shortcuts",
    "esc to cancel",
    "Antigravity CLI",
    "How's the CLI experience so far?",
    "[0] Skip",
    "Use /feedback",
    "Google AI Pro",
];

/// Status line patterns (model · effort), contains-based on the TRIMMED line in
/// BOTH spellings: middot (`Gemini 3.7 Flash · high`) and parenthesized
/// (`Gemini 3.7 Flash (High)`, vertical logo). Covers the counter variant
/// (`… · high · 1 task(s) · /tasks`), rows fused with hints, and the vertical
/// logo model row. Tradeoff: real content containing ` · high|medium|low` or
/// ` (High)|(Medium)|(Low)` anywhere is also dropped (incl. the ` · higher` /
/// ` · highly` substrings) — accepted and documented (plan §7/§10).
const AGY_STATUS_PATTERNS: &[&str] = &[
    " \u{00B7} high",
    " \u{00B7} medium",
    " \u{00B7} low",
    " (High)",
    " (Medium)",
    " (Low)",
];

struct AgyFilter;

impl AgentFilter for AgyFilter {
    fn keep_line(&self, line: &str) -> bool {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            return false;
        }

        // TUI chrome patterns (raw line contains)
        for pattern in AGY_CHROME_PATTERNS {
            if line.contains(pattern) {
                return false;
            }
        }

        // Status line (model · effort): contains-based on the trimmed line,
        // both spellings; covers counter and fused-hint variants.
        for pattern in AGY_STATUS_PATTERNS {
            if trimmed.contains(pattern) {
                return false;
            }
        }

        // Thought/status marker rows (`▸ Thought for 2s, 236 tokens`, incl.
        // the fused `▸ ThougUse /feedback…` row).
        if trimmed.starts_with("\u{25B8} ") {
            return false;
        }

        // Bare single non-alphanumeric glyph (loose `●` / `○` decoration).
        if trimmed.chars().count() == 1 && !trimmed.chars().next().unwrap().is_alphanumeric() {
            return false;
        }

        // Half-block/block glyph logo rows (U+2580..U+259F): horizontal logo
        // rows like `▄▀▀▄  Antigravity CLI 1.1.19`.
        if trimmed
            .chars()
            .next()
            .map(|c| ('\u{2580}'..='\u{259F}').contains(&c))
            .unwrap_or(false)
        {
            return false;
        }

        // Input echo. Deliberate extension over ClaudeCodeFilter's rule
        // (`❯`/`>`/`❯ `-prefixed only): agy echoes `> `-prefixed lines.
        if trimmed == ">" || trimmed.starts_with("> ") {
            return false;
        }

        // Shared decoration checks (identical predicates as ClaudeCodeFilter).
        let non_space: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
        if is_box_drawing_line(&non_space) {
            return false;
        }
        if starts_with_braille(trimmed) {
            return false;
        }
        if is_low_alnum_line(trimmed) {
            return false;
        }

        true
    }

    fn name(&self) -> &str {
        "antigravity"
    }
}

/// Per-agent filter selection for the PTY output path.
/// `Antigravity` sessions get `AgyFilter`; every other kind (`Claude`, `Codex`,
/// `Pi`) and undetected sessions (`None`) keep `ClaudeCodeFilter` exactly as
/// before — bit-identical behavior for all non-agy sessions.
fn filter_for_agent(agent_kind: Option<CodingAgentKind>) -> Box<dyn AgentFilter> {
    match agent_kind {
        Some(CodingAgentKind::Antigravity) => Box::new(AgyFilter),
        _ => Box::new(ClaudeCodeFilter),
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
}

pub struct BridgeHandle {
    pub info: BridgeInfo,
    pub cancel: CancellationToken,
    pub output_sender: mpsc::Sender<Vec<u8>>,
    pub tasks: Vec<JoinHandle<()>>,
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_bridge<R: tauri::Runtime>(
    bot_token: String,
    chat_id: i64,
    session_id: Uuid,
    info: BridgeInfo,
    pty_mgr: Arc<Mutex<PtyManager>>,
    network: OutboundNetwork,
    app_handle: tauri::AppHandle<R>,
    reader: Option<SessionReaderKind>,
    agent_kind: Option<CodingAgentKind>,
) -> BridgeHandle {
    let cancel = CancellationToken::new();
    let (tx, rx) = mpsc::channel::<Vec<u8>>(256);
    let mut tasks = Vec::new();

    let session_id_str = session_id.to_string();

    match reader {
        Some(SessionReaderKind::Claude { project_dir }) => {
            drop(rx);
            tasks.push(super::claude_watcher::spawn_watch_task(
                project_dir,
                network.clone(),
                bot_token.clone(),
                chat_id,
                session_id_str.clone(),
                cancel.clone(),
                app_handle.clone(),
            ));
        }
        Some(SessionReaderKind::Codex {
            search_root,
            cwd,
            attach_time,
        }) => {
            drop(rx);
            tasks.push(super::codex_watcher::spawn_watch_task(
                search_root,
                cwd,
                attach_time,
                network.clone(),
                bot_token.clone(),
                chat_id,
                session_id_str.clone(),
                cancel.clone(),
                app_handle.clone(),
            ));
        }
        None => {
            tasks.push(tokio::spawn(output_task(
                rx,
                network.clone(),
                bot_token.clone(),
                chat_id,
                session_id_str.clone(),
                cancel.clone(),
                app_handle.clone(),
                agent_kind,
            )));
        }
    }

    // Poll task: Telegram getUpdates -> write to PTY stdin (runs in BOTH modes)
    tasks.push(tokio::spawn(poll_task(
        bot_token,
        chat_id,
        session_id,
        session_id_str,
        pty_mgr,
        network,
        cancel.clone(),
        app_handle,
    )));

    BridgeHandle {
        info,
        cancel,
        output_sender: tx,
        tasks,
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

// The 8-argument signature is the frozen plan spec (#1549 §5.4): the PTY-bridge
// chain threads `agent_kind` as a loose parameter by design (no struct grouping).
#[allow(clippy::too_many_arguments)]
async fn output_task<R: tauri::Runtime>(
    mut rx: mpsc::Receiver<Vec<u8>>,
    network: OutboundNetwork,
    token: String,
    chat_id: i64,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle<R>,
    agent_kind: Option<CodingAgentKind>,
) {
    let mut logger = BridgeLogger::new(&session_id);
    let mut diag = DiagLogger::new();
    let mut buffer = String::new();
    let mut last_buffer_add = Instant::now();
    let flush_delay = Duration::from_millis(FLUSH_DELAY_MS);

    // Phase 2: Virtual terminal parser
    let mut vt = vt100::Parser::new(VT_ROWS, VT_COLS, 0);

    // Phase 3: Row stabilization tracker
    let mut tracker = RowTracker::new(VT_ROWS, STABILIZATION_MS);

    // Phase 4: Agent-specific filter, selected by the session's agent kind.
    // INIT log line below shows `filter=antigravity` for agy sessions and
    // `filter=claude-code` for everything else (live evidence hook).
    let filter: Box<dyn AgentFilter> = filter_for_agent(agent_kind);

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
                            &mut buffer, &network, &token, chat_id,
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
            &network,
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

// ── Poll task (Telegram -> PTY) ──────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn poll_task<R: tauri::Runtime>(
    token: String,
    chat_id: i64,
    session_id: Uuid,
    session_id_str: String,
    _pty_mgr: Arc<Mutex<PtyManager>>,
    network: OutboundNetwork,
    cancel: CancellationToken,
    app: tauri::AppHandle<R>,
) {
    let mut logger = BridgeLogger::new(&session_id_str);
    let mut offset: i64 = 0;
    let network_error_policy = {
        let settings = app.state::<crate::config::settings::SettingsState>();
        let cfg = settings.read().await;
        cfg.telegram_network_poll_error_logging.clone()
    };
    let mut network_error_state = PollNetworkErrorState::new();
    let mut poll_backoff = new_poll_backoff(&session_id_str);

    // Skip old messages
    match api::get_updates(&network, &token, 0, 0).await {
        Ok(updates) => {
            poll_backoff.reset();
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
            let msg = crate::telegram::redact::redact(&e.to_string());
            let kind = TelegramErrKind::classify(&msg);

            if matches!(kind, TelegramErrKind::Network) {
                match network_error_state.record_network_failure(&network_error_policy) {
                    PollNetworkLogAction::Failure {
                        level,
                        suppressed_since_last_emit,
                        sustained,
                    } => {
                        logger.log("POLL_ERR", &session_id_str, &msg);
                        let suffix = if suppressed_since_last_emit == 0 {
                            String::new()
                        } else {
                            format!(" suppressed_repeats={}", suppressed_since_last_emit)
                        };
                        log::log!(
                            level,
                            "[bridge] Initial getUpdates network error kind={} session_id={} sustained={} err={}{}",
                            kind.as_str(),
                            session_id_str,
                            sustained,
                            msg,
                            suffix
                        );
                    }
                    PollNetworkLogAction::Suppressed { level } => {
                        logger.log(
                            "POLL_ERR_SUPPRESSED",
                            &session_id_str,
                            &format!("kind={} phase=initial", kind.as_str()),
                        );
                        log::log!(
                            level,
                            "[bridge] Initial getUpdates network error suppressed kind={} session_id={}",
                            kind.as_str(),
                            session_id_str
                        );
                    }
                    PollNetworkLogAction::Recovery { .. } | PollNetworkLogAction::None => {
                        unreachable!("record_network_failure only returns Failure or Suppressed")
                    }
                }
            } else {
                logger.log("POLL_ERR", &session_id_str, &msg);
                log::error!(
                    "[bridge] Initial getUpdates failed kind={} session_id={} err={}",
                    kind.as_str(),
                    session_id_str,
                    msg
                );
            }
            if !sleep_backoff_or_cancel(&cancel, &mut poll_backoff, &mut logger, &session_id_str)
                .await
            {
                return;
            }
        }
    }

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            result = api::get_updates(&network, &token, offset, 5) => {
                match result {
                    Ok(updates) => {
                        poll_backoff.reset();
                        match network_error_state.record_success(&network_error_policy) {
                            PollNetworkLogAction::Recovery {
                                level,
                                outage_seconds,
                                suppressed_total,
                                sustained,
                            } => {
                                logger.log(
                                    "POLL_RECOVERY",
                                    &session_id_str,
                                    &format!(
                                        "outage_seconds={} suppressed_total={} sustained={}",
                                        outage_seconds, suppressed_total, sustained
                                    ),
                                );
                                log::log!(
                                    level,
                                    "[bridge] Telegram poll recovered session_id={} outage_seconds={} suppressed_total={} sustained={}",
                                    session_id_str,
                                    outage_seconds,
                                    suppressed_total,
                                    sustained
                                );
                            }
                            PollNetworkLogAction::None => {}
                            PollNetworkLogAction::Failure { .. }
                            | PollNetworkLogAction::Suppressed { .. } => {
                                unreachable!("record_success only returns Recovery or None")
                            }
                        }
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
                                        let _ = api::send_message(&network, &token, chat_id, "Cannot transcribe voice: Gemini API key not configured").await;
                                        continue;
                                    }

                                    let file_path = match api::get_file(&network, &token, &file_id).await {
                                        Ok(fp) => fp,
                                        Err(e) => {
                                            logger.log("VOICE_ERR", &session_id_str, &format!("get_file failed: {}", e));
                                            let _ = api::send_message(&network, &token, chat_id, &format!("Failed to get voice file: {}", e)).await;
                                            continue;
                                        }
                                    };

                                    let audio_bytes = match api::download_file(&network, &token, &file_path).await {
                                        Ok(bytes) => bytes,
                                        Err(e) => {
                                            logger.log("VOICE_ERR", &session_id_str, &format!("download failed: {}", e));
                                            let _ = api::send_message(&network, &token, chat_id, &format!("Failed to download voice: {}", e)).await;
                                            continue;
                                        }
                                    };

                                    match crate::commands::voice::transcribe_audio_with_network(&network, &audio_bytes, "audio/ogg", &api_key, &model).await {
                                        Ok(text) => {
                                            logger.log("VOICE_OK", &session_id_str, &format!("transcribed {} chars", text.len()));
                                            let _ = api::send_message(&network, &token, chat_id, &format!("Transcribed: {}", text)).await;
                                            text
                                        }
                                        Err(e) => {
                                            logger.log("VOICE_ERR", &session_id_str, &format!("transcription failed: {}", e));
                                            let _ = api::send_message(&network, &token, chat_id, &format!("Transcription failed: {}", e)).await;
                                            continue;
                                        }
                                    }
                                }
                            };

                            if let Err(e) = crate::pty::inject::inject_text_into_session(&app, session_id, &inject_text).await {
                                logger.log("PTY_ERR", &session_id_str, &e.to_string());
                                log::error!("Failed to write Telegram input to PTY: {}", e);
                            }

                            // #552 a Telegram message is a real user message to this
                            // coordinator: reset the badge clock + auto-close silence
                            // (covers text and transcribed-voice inbound; both converge here).
                            crate::commands::pty::note_user_message_to_session(
                                &app,
                                session_id,
                                crate::commands::pty::UserInputSource::CompleteMessage,
                            )
                            .await;

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
                        let msg = crate::telegram::redact::redact(&e.to_string());
                        let kind = TelegramErrKind::classify(&msg);
                        let pid = std::process::id();
                        let token_prefix = token.split(':').next().unwrap_or("?");

                        if !matches!(kind, TelegramErrKind::Network) {
                            network_error_state.reset_without_recovery();
                            logger.log("POLL_ERR", &session_id_str, &msg);
                            log::error!(
                                "[bridge] Telegram poll error kind={} session_id={} pid={} bot_id={} err={}",
                                kind.as_str(),
                                session_id_str,
                                pid,
                                token_prefix,
                                msg
                            );
                            if !sleep_backoff_or_cancel(
                                &cancel,
                                &mut poll_backoff,
                                &mut logger,
                                &session_id_str,
                            )
                            .await
                            {
                                break;
                            }
                            continue;
                        }

                        match network_error_state.record_network_failure(&network_error_policy) {
                            PollNetworkLogAction::Failure {
                                level,
                                suppressed_since_last_emit,
                                sustained,
                            } => {
                                logger.log("POLL_ERR", &session_id_str, &msg);
                                let suffix = if suppressed_since_last_emit == 0 {
                                    String::new()
                                } else {
                                    format!(" suppressed_repeats={}", suppressed_since_last_emit)
                                };
                                log::log!(
                                    level,
                                    "[bridge] Telegram poll network error kind={} session_id={} pid={} bot_id={} sustained={} err={}{}",
                                    kind.as_str(),
                                    session_id_str,
                                    pid,
                                    token_prefix,
                                    sustained,
                                    msg,
                                    suffix
                                );
                            }
                            PollNetworkLogAction::Suppressed { level } => {
                                logger.log(
                                    "POLL_ERR_SUPPRESSED",
                                    &session_id_str,
                                    &format!("kind={} phase=poll", kind.as_str()),
                                );
                                log::log!(
                                    level,
                                    "[bridge] Telegram poll network error suppressed kind={} session_id={}",
                                    kind.as_str(),
                                    session_id_str
                                );
                            }
                            PollNetworkLogAction::Recovery { .. } | PollNetworkLogAction::None => {
                                unreachable!("record_network_failure only returns Failure or Suppressed")
                            }
                        }
                        if !sleep_backoff_or_cancel(
                            &cancel,
                            &mut poll_backoff,
                            &mut logger,
                            &session_id_str,
                        )
                        .await
                        {
                            break;
                        }
                    }
                }
            }
        }
    }
}

async fn sleep_backoff_or_cancel(
    cancel: &CancellationToken,
    backoff: &mut NetworkBackoff,
    logger: &mut BridgeLogger,
    session_id: &str,
) -> bool {
    let delay = backoff.next_delay();
    let sleep_ms = delay.as_millis();
    logger.log("POLL_BACKOFF", session_id, &format!("sleep_ms={sleep_ms}"));
    log::debug!(
        "[bridge] POLL_BACKOFF session_id={} sleep_ms={}",
        session_id,
        sleep_ms
    );
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(delay) => true,
    }
}

fn new_poll_backoff(session_id: &str) -> NetworkBackoff {
    NetworkBackoff::new(
        Duration::from_secs(3),
        Duration::from_secs(60),
        NetworkBackoff::salt_from_str(session_id),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_backoff_uses_plan_bounds_and_session_salt() {
        let mut first = new_poll_backoff("session-a");
        let mut second = new_poll_backoff("session-b");

        let first_delay = first.next_delay();
        let second_delay = second.next_delay();

        assert!(first_delay >= Duration::from_secs(3));
        assert!(second_delay >= Duration::from_secs(3));
        assert!(first_delay <= Duration::from_secs(60));
        assert!(second_delay <= Duration::from_secs(60));
        assert_ne!(first_delay, second_delay);
    }

    #[test]
    fn network_policy_first_failure_uses_warn_default() {
        let mut state = PollNetworkErrorState::new();
        let policy = TelegramNetworkPollErrorLogging::default();

        assert_eq!(
            state.record_network_failure(&policy),
            PollNetworkLogAction::Failure {
                level: log::Level::Warn,
                suppressed_since_last_emit: 0,
                sustained: false,
            }
        );
    }

    #[test]
    fn network_policy_repeated_transient_uses_debug_default() {
        let mut state = PollNetworkErrorState::new();
        let policy = TelegramNetworkPollErrorLogging::default();

        let _ = state.record_network_failure(&policy);
        assert_eq!(
            state.record_network_failure(&policy),
            PollNetworkLogAction::Suppressed {
                level: log::Level::Debug
            }
        );
    }

    #[test]
    fn network_policy_recovery_emits_once_after_failure() {
        let mut state = PollNetworkErrorState::new();
        let policy = TelegramNetworkPollErrorLogging::default();

        let _ = state.record_network_failure(&policy);
        let action = state.record_success(&policy);
        assert!(matches!(
            action,
            PollNetworkLogAction::Recovery {
                level: log::Level::Info,
                ..
            }
        ));
        assert_eq!(state.record_success(&policy), PollNetworkLogAction::None);
    }

    #[test]
    fn network_policy_recovery_reports_suppressed_total_once() {
        let mut state = PollNetworkErrorState::new();
        let policy = TelegramNetworkPollErrorLogging::default();

        let _ = state.record_network_failure(&policy);
        let _ = state.record_network_failure(&policy);
        let _ = state.record_network_failure(&policy);

        assert!(matches!(
            state.record_success(&policy),
            PollNetworkLogAction::Recovery {
                suppressed_total: 2,
                ..
            }
        ));
    }

    #[test]
    fn network_policy_zero_sustained_threshold_escalates_first_failure() {
        let mut state = PollNetworkErrorState::new();
        let policy = TelegramNetworkPollErrorLogging {
            sustained_after_seconds: 0,
            ..TelegramNetworkPollErrorLogging::default()
        };

        assert_eq!(
            state.record_network_failure(&policy),
            PollNetworkLogAction::Failure {
                level: log::Level::Error,
                suppressed_since_last_emit: 0,
                sustained: true,
            }
        );
    }

    #[test]
    fn network_policy_zero_sustained_threshold_recovers_as_sustained() {
        let mut state = PollNetworkErrorState::new();
        let policy = TelegramNetworkPollErrorLogging {
            sustained_after_seconds: 0,
            ..TelegramNetworkPollErrorLogging::default()
        };

        let _ = state.record_network_failure(&policy);

        assert!(matches!(
            state.record_success(&policy),
            PollNetworkLogAction::Recovery {
                sustained: true,
                ..
            }
        ));
    }

    #[test]
    fn network_policy_zero_threshold_and_zero_repeat_logs_every_failure_at_sustained_level() {
        let mut state = PollNetworkErrorState::new();
        let policy = TelegramNetworkPollErrorLogging {
            sustained_after_seconds: 0,
            sustained_repeat_seconds: 0,
            ..TelegramNetworkPollErrorLogging::default()
        };

        assert!(matches!(
            state.record_network_failure(&policy),
            PollNetworkLogAction::Failure {
                level: log::Level::Error,
                sustained: true,
                ..
            }
        ));
        assert!(matches!(
            state.record_network_failure(&policy),
            PollNetworkLogAction::Failure {
                level: log::Level::Error,
                sustained: true,
                ..
            }
        ));
    }

    #[test]
    fn network_policy_initial_failure_flow_recovers_on_first_success() {
        let mut state = PollNetworkErrorState::new();
        let policy = TelegramNetworkPollErrorLogging::default();

        let _initial = state.record_network_failure(&policy);

        assert!(matches!(
            state.record_success(&policy),
            PollNetworkLogAction::Recovery {
                suppressed_total: 0,
                sustained: false,
                ..
            }
        ));
    }

    #[test]
    fn network_policy_reset_makes_next_network_failure_fresh() {
        let mut state = PollNetworkErrorState::new();
        let policy = TelegramNetworkPollErrorLogging::default();

        let _ = state.record_network_failure(&policy);
        state.reset_without_recovery();

        assert_eq!(
            state.record_network_failure(&policy),
            PollNetworkLogAction::Failure {
                level: log::Level::Warn,
                suppressed_since_last_emit: 0,
                sustained: false,
            }
        );
    }

    #[test]
    fn network_policy_reset_prevents_stale_recovery() {
        let mut state = PollNetworkErrorState::new();
        let policy = TelegramNetworkPollErrorLogging::default();

        let _ = state.record_network_failure(&policy);
        state.reset_without_recovery();

        assert_eq!(state.record_success(&policy), PollNetworkLogAction::None);
    }

    // ── AgyFilter unit tests (#1549) ────────────────────────────
    //
    // Fixtures are the exact rows observed in diag-sent.log (session
    // 19de56d2-…, frames 03:14-03:23 UTC 2026-08-25). DROPPED = chrome,
    // KEPT = real content / documented residual.

    #[test]
    fn agy_filter_keep_line_drops_logo_rows() {
        let filter = AgyFilter;

        // Horizontal logo, frame 03:14:29.703 (all rows).
        let horizontal = [
            "▄▀▀▄        Antigravity CLI 1.1.19",
            "     ▀▀▀▀▀▀       mariano.blua@gmail.com",
            "    ▀▀▀▀▀▀▀▀      Gemini 3.7 Flash (High)",
            "   ▄▀▀    ▀▀▄     D:/0_repos/AgentsCommander_iac/.ac/wg-21-ac-dev-team-v3/__agent_ac-cli-tester-v3",
        ];
        for line in horizontal {
            assert!(!filter.keep_line(line), "logo row should be dropped: {line:?}");
        }

        // Vertical logo text rows, frame 03:15:27.898 (Blocker C: no block
        // glyph, 2-space indent — dropped by patterns, not by glyph rules).
        let vertical = [
            "  Antigravity CLI 1.1.19",
            "  mariano.blua@gmail.com (Google AI Pro)",
            "  Gemini 3.7 Flash (High)",
        ];
        for line in vertical {
            assert!(!filter.keep_line(line), "vertical logo row should be dropped: {line:?}");
        }
    }

    #[test]
    fn agy_filter_keep_line_pins_frame_031527_rows() {
        let filter = AgyFilter;

        // The 4 exact rows of the vertical-logo response frame (L19-33),
        // with explicit outcomes.
        assert!(!filter.keep_line(
            "  mariano.blua@gmail.com (Google AI Pro)"
        )); // DROPPED: account row by `Google AI Pro` pattern
        assert!(!filter.keep_line("  Gemini 3.7 Flash (High)")); // DROPPED: model row by ` (High)` pattern
        assert!(filter.keep_line(
            "  D:/0_repos/AgentsCommander_iac/.ac/wg-21-ac-dev-team-v3/__agent_ac-cli-tester-v3"
        )); // KEPT: path row — documented residual (plan §10.4)
        assert!(filter.keep_line(
            "  ¡Hola! Soy ac-cli-tester-v3. Estoy listo para ayudarte con la validación"
        )); // KEPT: real content
    }

    #[test]
    fn agy_filter_keep_line_drops_hint_rows() {
        let filter = AgyFilter;

        assert!(!filter.keep_line(
            "? for shortcuts                                                                                                                          Gemini 3.7 Flash · high"
        )); // frame 03:14:29.703
        assert!(!filter.keep_line(
            "esc to cancel                                                                                                                            Gemini 3.7 Flash · high"
        )); // frame 03:14:47.504
    }

    #[test]
    fn agy_filter_keep_line_drops_status_line() {
        let filter = AgyFilter;

        // Bare status lines (middot spelling).
        assert!(!filter.keep_line("Gemini 3.7 Flash · high"));
        assert!(!filter.keep_line("Gemini 3.7 Flash · medium"));
        assert!(!filter.keep_line("Gemini 3.7 Flash · low"));

        // Blocker A: counter variant, bare and fused (frame 03:23:42.495, L208
        // — exact content, 15 spaces).
        assert!(!filter.keep_line("Gemini 3.7 Flash · high · 1 task(s) · /tasks"));
        assert!(!filter.keep_line(
            "esc to cancel               Gemini 3.7 Flash · high · 1 task(s) · /tasks"
        ));
    }

    #[test]
    fn agy_filter_keep_line_drops_thought_lines() {
        let filter = AgyFilter;

        // L96 / L~122 / L140 and the fused row L~201.
        assert!(!filter.keep_line("▸ Thought for 2s, 236 tokens"));
        assert!(!filter.keep_line("▸ Thought for 14s, 2.8k tokens"));
        assert!(!filter.keep_line("▸ Thought for 4s, 1.8k tokens"));
        assert!(!filter.keep_line(
            "▸ ThougUse /feedback to share your experience with the team."
        ));
    }

    #[test]
    fn agy_filter_keep_line_drops_single_glyph() {
        let filter = AgyFilter;

        assert!(!filter.keep_line("●")); // frame 03:22:47.105
        assert!(!filter.keep_line("○")); // frame 03:23:37.897
    }

    #[test]
    fn agy_filter_keep_line_drops_survey() {
        let filter = AgyFilter;

        // L240/L241 and the standalone feedback hint.
        assert!(!filter.keep_line("How's the CLI experience so far? Help us improve:"));
        assert!(!filter.keep_line("[1] Good  [2] Fine  [3] Bad  [0] Skip"));
        assert!(!filter.keep_line("Use /feedback to share your experience with the team."));
    }

    #[test]
    fn agy_filter_keep_line_drops_input_echo() {
        let filter = AgyFilter;

        assert!(!filter.keep_line("> Hola")); // frame 03:14:45.492
        assert!(!filter.keep_line(">"));
    }

    #[test]
    fn agy_filter_keep_line_keeps_content() {
        let filter = AgyFilter;

        // Real response content, frame 03:15:27 and 03:23:46.
        assert!(filter.keep_line(
            "  ¡Hola! Soy ac-cli-tester-v3. Estoy listo para ayudarte con la validación"
        ));
        assert!(filter.keep_line(
            "He ejecutado la validación en vivo de las 5 filas PENDING asignadas"
        ));
        // A normal code/tool line.
        assert!(filter.keep_line("  let info = BridgeInfo { bot_id: bot.id.clone() };"));
        // Tool-call activity rows pass intentionally (informative, not chrome).
        assert!(filter.keep_line(
            "● Read(D:/0_repos/AgentsCommander_iac...t-compatibility-antigravity.md) (ctrl+o to expand)"
        ));
    }

    #[test]
    fn agy_filter_keep_line_passes_step_labels() {
        let filter = AgyFilter;

        // Step labels are a documented residual (plan §10.4): task-dependent
        // vocabulary, no generalizable structural marker. KEPT deliberately.
        for line in [
            "  Analyzing Shell Arguments",
            "  Clarifying Authorization Boundaries",
            "  Investigating System Behavior",
            "  Observing System Activity",
        ] {
            assert!(filter.keep_line(line), "step label should be kept: {line:?}");
        }
    }

    #[test]
    fn agy_filter_keep_line_tradeoff_drops_midline_effort() {
        let filter = AgyFilter;

        // Contains-based tradeoff of Blockers A and C (plan §10.7): real
        // content carrying the effort marker anywhere is dropped, incl. the
        // substring false positives ` · higher` / ` · highly`.
        assert!(!filter.keep_line("El modelo rinde · high en esta prueba"));
        assert!(!filter.keep_line("El modo (High) es el más potente"));
        assert!(!filter.keep_line("Rendimiento · higher que antes"));
    }

    #[test]
    fn filter_for_agent_maps_antigravity() {
        let filter = filter_for_agent(Some(CodingAgentKind::Antigravity));
        assert_eq!(filter.name(), "antigravity");
    }

    #[test]
    fn filter_for_agent_maps_everything_else_to_claude() {
        // Regression pin: every non-agy session keeps ClaudeCodeFilter
        // bit-identically (Claude/Codex/Pi/None).
        assert_eq!(
            filter_for_agent(Some(CodingAgentKind::Claude)).name(),
            "claude-code"
        );
        assert_eq!(
            filter_for_agent(Some(CodingAgentKind::Codex)).name(),
            "claude-code"
        );
        assert_eq!(filter_for_agent(Some(CodingAgentKind::Pi)).name(), "claude-code");
        assert_eq!(filter_for_agent(None).name(), "claude-code");
    }

    #[test]
    fn agy_filter_shared_predicates_identical_to_claude_filter() {
        // Same decoration inputs behave identically through both filters.
        let agy = AgyFilter;
        let claude = ClaudeCodeFilter;

        for line in [
            "────────────────────", // box-drawing separator (U+2500)
            "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏", // braille spinner row (U+2800..U+28FF)
            "▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪", // low-alnum decorative line
        ] {
            assert!(!agy.keep_line(line), "agy should drop: {line:?}");
            assert!(!claude.keep_line(line), "claude should drop: {line:?}");
        }

        // Normal content passes both.
        for line in ["Hello world", "He ejecutado la validación"] {
            assert!(agy.keep_line(line), "agy should keep: {line:?}");
            assert!(claude.keep_line(line), "claude should keep: {line:?}");
        }
    }
}
