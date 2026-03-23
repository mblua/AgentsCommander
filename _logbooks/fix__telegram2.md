# Logbook: fix/telegram2 - Telegram Bridge Filtering

**Branch:** `fix/telegram2`
**Started:** 2026-03-20
**Status:** In progress

---

## Problem Statement

The Telegram bridge forwards PTY output from terminal sessions to Telegram. The core challenge is **filtering out terminal noise** (spinners, ANSI artifacts, tool headers, TUI chrome) so that only meaningful content reaches Telegram.

### Root Cause (v1-v3 failures)

The initial approaches (v1 through v3) used `strip_ansi_escapes` to remove ANSI codes from raw PTY bytes. This fails fundamentally because ANSI escape codes encode **cursor movement**, not just colors. Stripping them loses positioning information, resulting in:
- Words concatenated without spaces
- Spinner characters mixed into real text
- Garbled output from cursor rewrites

### Current Solution (v4 - RowTracker + Stabilization)

Replace raw byte processing with a **virtual terminal emulator** (`vt100` crate, 50x220):
1. Feed raw PTY bytes into vt100 parser
2. Track each screen row independently (RowTracker)
3. Only emit rows that have been **unchanged for 800ms+** (spinners cycle at ~450ms, so they never stabilize)
4. Apply Claude Code-specific filters (AgentFilter) on stabilized rows
5. Buffer, deduplicate, chunk at 4000 chars, send to Telegram

### Known Fixed Issues (prior to this session)

| Commit | Issue | Fix |
|--------|-------|-----|
| 64cee49 | `strip_trailing_spinner()` cutting model responses (removed `●` at position 0, but `●` is also Claude's response indicator) | Require 5+ space gap before spinner char |
| 64cee49 | Tool headers split across vt100 rows (`─Bash(` without `●`) not filtered | Add patterns for `─Bash(`, `─Read(`, etc. |
| 64cee49 | Alphanumeric ratio check broken (included spaces in calculation) | Filter spaces before calculating ratio |
| 8f8654a | Trailing spinners at right edge of wide rows | `strip_trailing_spinner()` function |
| 8f8654a | `⎿ Running...` progress lines leaking | Filter pattern added |
| 8f8654a | `●─Bash()` tool headers leaking | Filter pattern added |
| 8f8654a | Hyphenated thinking words ("Topsy-turvying") not caught | Relaxed `is_thinking_line()` |
| a2cbe0f | Character-by-character typing flooding Telegram | Suppress bridge output while user types, auto-clear after 5s |
| e086eff | User input not visible in Telegram | Capture `❯` line on Enter, send with prefix |

---

## Current Session - Investigation Log

### 2026-03-23 - Session 1: Code review + 3 fixes

**Starting point:** Branch has 5 commits ahead of main with filter improvements and bug fixes. Merged latest main (50 files).

#### Fix 1: `is_thinking_line` false positive (CRITICAL)

**Bug:** The function filters any line starting with a spinner char and ending with `…` if under 50 chars. But `●` (U+25CF) is both a spinner char AND Claude's response indicator. A real response like `● Here is my answer…` would be stripped because:
1. `●` is in CLAUDE_SPINNERS, so it gets removed
2. `Here is my answer…` ends with `…`
3. Length 17 < 50

**Fix:** Added `!word_part.contains(' ')` check. Spinner text is always a single word (`Brewing…`, `Topsy-turvying…`). Real content has spaces.

**Result:** cargo check OK.

#### Fix 2: Generalized tool header filtering

**Bug:** Tool headers were filtered by listing individual tools: Bash, Read, Edit, Write, Grep, Glob, Agent. Claude Code has many more tools (Skill, TodoWrite, TaskCreate, WebFetch, WebSearch, NotebookEdit, etc.) that would leak through.

**Fix:** Replaced hardcoded list with generic pattern detection:
- `●─` prefix always = tool header (no change)
- `─` + uppercase letter = tool header (catches ALL tools without maintaining a list)

**Result:** cargo check OK.

#### Fix 3: Chrome patterns + cleanup

**Changes:**
- Added missing chrome patterns: "esc to interrupt", "esc to cancel", "auto-compact", "compacted conversation", "memory updated"
- Added filter for bare `⎿` character (visual bracket, no content)
- Removed `non_space` variable shadowing (was calculated twice identically in `keep_line`)

**Result:** cargo check OK.

#### Code review (feature-dev:code-reviewer)

Dos issues detectados y corregidos:

1. **`─` + uppercase heuristic demasiado amplio**: CLIs como pnpm/bun pueden imprimir lineas tipo `─ Build failed` que empiezan con `─` + mayuscula. Fix: exigir tambien un `(` en la linea, ya que tool headers siempre tienen parentesis (`─Bash(`, `─Read(`).

2. **`● Confirmed…` false positive**: Respuestas reales de una sola palabra con `●` + `…` serian filtradas. Fix: separar la logica segun si habia spinner prefix o no. Sin prefix (`Gallivanting…`) siempre es spinner. Con prefix (`● Brewing…`) se filtra solo si empieza con mayuscula (defense in depth - el mecanismo primario es stabilization).

**Ambos fixes compilan OK.**

#### Test 1: Live bridge test (3:17 AM)

**Setup:** App running in dev mode, bridge attached, chatted with Claude Code via Telegram.

**Observed problems (from screenshots):**
1. `└ (no content)` - TUI chrome leaking (tool result connector)
2. `❯ Y vos quién sos?` appears twice - user input duplication
3. Response content duplicated - partial sent first, then full response with `●` prefix
4. Extra spaces in content: "un        híbrido" (vt100 220-col artifact)

**Root cause analysis for problem 3 (content duplication):**
When Claude writes a response, middle rows stabilize before the first row (still being typed). These get sent as a partial message. Later, the screen redraws with the full response, but the same content has different whitespace (220-col wrapping produces extra spaces). The emitted_content HashSet does exact string comparison, so "un        híbrido" != "un híbrido" and the content escapes dedup.

#### Fix 4: Normalize whitespace for dedup comparison

**Change:** Added `normalize_whitespace()` function that collapses 2+ spaces into 1. Applied to:
- `harvest_stable()` - normalize before checking/inserting emitted_content
- `mark_emitted()` - normalize before inserting

**What this does NOT change:** The actual content sent to Telegram (still uses original row content). Only affects dedup comparison.

**Result:** cargo check OK.

#### Test 2: Live bridge test (3:33 AM) - after dedup fix

**Setup:** App rebuilt with normalize_whitespace dedup fix.

**Improvements observed:**
- No more content duplication (partial + full sent as separate messages). Dedup working.
- `❯ Y vos quién sos?` appears only once (was twice before)
- Response to "Repetilo" correctly deduplicated (same answer not resent)

**Remaining problems from screenshot:**
1. `⎿  (no content)` still leaking (filter only caught bare `⎿` and "Running")
2. Partial response only - first line (`● Soy Claude Code...`) and last line (`crecimiento...`) never stabilized
3. Extra spaces ("Strategist -   un híbrido") from 220-col vt100

**Evidence from diag logs (`.agentscommander-dev/telegram-bridge.log`):**
- `06:34:10 STABLE` only shows middle rows of response. First line (with `●`) never appears in any STABLE entry.
- `06:34:20-34` multiple `TYPING auto-cleared` events after the response - the user started typing, which may have prevented further rows from stabilizing.
- Root cause: Claude Code's TUI keeps the first line of a response active (spinner/cursor) longer than middle rows. These rows never reach 800ms stability.

#### Fix 5: Filter `⎿  (no content)` chrome

**Change:** Added `trimmed.contains("(no content)")` to the `⎿` filter condition.

**Review (feature-dev:code-reviewer):** Approved. No false positive risk (gated behind `starts_with("\u{23BF}")`). Follow-up: monitor if `⎿  Done` or `⎿  Received` also leak.

**Result:** cargo check OK.

#### Test 3: Live bridge test (3:45 AM) - after (no content) fix

**Improvements confirmed:**
- `⎿  (no content)` gone - Fix 5 working
- Full response delivered (including first `●` line and complete paragraph)
- No content duplication
- Response to "Repetilo" correctly deduplicated

**New problems observed:**
1. `more` - unexplained leaked content (needs diag log investigation)
2. `✻ Architecting... (thought for 1s)` + `4` - thinking duration indicator leaked

**Root cause for problem 2:** `is_thinking_line` expects ellipsis (`…`/`...`) at end of line, but `✻ Architecting... (thought for 1s)` ends with `)`. The `(thought for Ns)` is a Claude Code specific thinking duration display.

#### Fix 6: Filter `(thought for` thinking duration pattern

**Change:** Added `"(thought for"` to `CLAUDE_CHROME_PATTERNS`. Uses `contains()` matching, gated behind the per-line chrome check.

**Review (feature-dev:code-reviewer):** Approved. The opening parenthesis makes false positives extremely unlikely in real prose. Narrower than several already-accepted patterns.

**Result:** cargo check OK.

#### Test 4: Live bridge test (4:01 AM) - after (thought for) fix

**Regression observed:** Garbled TUI chrome leaking heavily. The vt100 captures Claude Code's slash commands menu during startup/redraw. Text gets concatenated without spaces, so exact-match patterns fail.

**Evidence from diag-sent.log:**
```
⏵⏵ebypassnpermissions:on (shift+tab-to cycle)ewureleaseanotesent) Update CLAUDE.md with learnings from th…
❯                                                          otesent) Update CLAUDE.md with learnings from th…
```

**Root cause:** Existing patterns `"bypass permissions"` and `"shift+tab to cycle"` require specific spacing that garbled text breaks. The `⏵` (U+23F5) character is a dead giveaway for TUI menus but wasn't filtered.

#### Fix 7: Strengthen chrome patterns for garbled TUI content

**Changes:**
1. Added `"permissions:on"` and `"permissions:off"` (catches garbled `ebypassnpermissions:on`)
2. Shortened `"shift+tab to cycle"` / `"shift+tab to change"` to just `"shift+tab"` (catches garbled hyphenated versions)
3. Added `⏵` (U+23F5) character filter (only appears in TUI menus, never in real content)
4. Added `"Checking for updates"` notification pattern

**Removed after review:** `"with learnings from th"` - redundant (already caught by patterns above) and false-positive risk in real Claude output.

**Review (feature-dev:code-reviewer):** Approved after removing redundant pattern. `⏵` filter rated 95% confidence safe. `"shift+tab"` accepted tradeoff (low-frequency in genuine output).

**Result:** cargo check OK.

#### Test 5: Live bridge test (4:09 AM) - after garbled chrome fix

**Improvements confirmed:**
- NO garbled TUI chrome (Fix 7 working)
- NO `⎿ (no content)` (Fix 5 working)
- NO `(thought for)` indicators (Fix 6 working)
- Response to "Qué modelo sos?" correct
- `❯ Y vos quién sos?` once only

**Remaining problems:**
1. Partial-then-full duplication BACK: partial at 07:10:24, full at 07:10:34 (10s gap)
2. normalize_whitespace dedup can not help here - the strings are genuinely different (partial ends at "launch", full continues with "strategy, competitive...")

#### Fix 8: Screen-settling flush logic (anti-partial)

**Problem:** Flush delay of 500ms is too short. Middle rows stabilize 10+ seconds before the first line (`●`). Buffer flushes partial, then full arrives as duplicate.

**Solution:** Don't flush while the screen is still actively being written to.
- Added `has_recently_changed_rows(threshold)` to RowTracker
- While any non-empty row changed within 1600ms (2x stabilization), hold the buffer
- Safety net: force flush after 15s from FIRST buffer addition (not last, per reviewer)
- Size guard: flush at 8000 chars to bound memory/latency

**Key insight from review:** During pure thinking (spinner only), no rows are harvested so buffer is empty - the settling check doesn't block anything. It only holds back when content IS in the buffer AND more rows are still changing.

**Review (feature-dev:code-reviewer):** Two issues caught and fixed:
1. Safety net measured from last harvest (resets continuously) - fixed to measure from first buffer add
2. `buf_len > 2000` removed without replacement - reinstated at 8000

**Result:** cargo check OK.

**Pending:** Test.

---

## Diagnostic Tools

- `~/.summongate/telegram-bridge.log` - Structured log (INIT, STABLE, SEND_TG, USER_INPUT)
- `~/.summongate/diag-raw.log` - All stabilized rows pre-filter
- `~/.summongate/diag-sent.log` - Exactly what gets sent to Telegram
