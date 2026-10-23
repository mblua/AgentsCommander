use crate::session::profile::CodingAgentKind;
use std::collections::HashSet;
use tokio::time::Duration;
use tokio::time::Instant;

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

pub(super) struct RowTracker {
    rows: Vec<RowState>,
    /// Content strings already emitted (prevents re-emission on scroll)
    emitted_content: HashSet<String>,
    stabilization: Duration,
}

impl RowTracker {
    pub(super) fn new(num_rows: u16, stabilization_ms: u64) -> Self {
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
    pub(super) fn update_from_screen(&mut self, screen: &vt100::Screen) {
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
    pub(super) fn harvest_stable(&mut self, filter: &dyn AgentFilter) -> Vec<String> {
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
    pub(super) fn has_pending(&self) -> bool {
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
        concat!(
"\u{2500}\u{2501}\u{2550}\u{2502}\u{2503}\u{250C}\u{2510}\u{2514}\u{2518}\u{251C}\u{2524}\u{252C}",
"\u{2534}\u{253C}\u{2554}\u{2557}\u{255A}\u{255D}\u{2560}\u{2563}\u{2566}\u{2569}\u{256C}",
)
        .contains(c)
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

pub(super) trait AgentFilter: Send + Sync {
    fn keep_line(&self, line: &str) -> bool;
    fn name(&self) -> &str;
}

// ── Claude Code Filter ───────────────────────────────────────

pub(super) struct ClaudeCodeFilter;

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
            concat!(
"\u{2500}\u{2501}\u{2550}\u{2502}\u{2503}\u{250C}\u{2510}\u{2514}\u{2518}\u{251C}\u{2524}\u{252C}",
"\u{2534}\u{253C}\u{2554}\u{2557}\u{255A}\u{255D}\u{2560}\u{2563}\u{2566}\u{2569}\u{256C}",
)
            .contains(c)
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

pub(super) struct AgyFilter;

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
pub(super) fn filter_for_agent(agent_kind: Option<CodingAgentKind>) -> Box<dyn AgentFilter> {
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
