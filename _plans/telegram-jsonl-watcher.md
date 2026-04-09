# Plan: Telegram JSONL Watcher for Claude Code Sessions

**Branch:** `feature/telegram-jsonl-watcher`
**Status:** AWAITING ARCHITECT REVIEW

---

## Problem

The current Telegram bridge captures PTY output via a 6-phase pipeline:

```
PTY bytes -> vt100::Parser -> 800ms stabilization -> AgentFilter -> Buffer -> Telegram
```

This is extremely complex and still produces noisy output because the source is raw terminal bytes full of ANSI escapes, cursor movement, spinner animations, TUI chrome, and box-drawing characters.

## Discovery

Claude Code writes **clean structured session logs** as JSONL files:

- **Location:** `~/.claude/projects/{mangled-cwd}/<session-id>.jsonl`
- **Path mangling:** Replace non-alphanumeric, non-hyphen chars with `-` (already implemented in `commands/session.rs:72-74`)
- **Format:** One JSON object per line with structured message data:

```json
{"type": "permission-mode", "permissionMode": "bypassPermissions", "sessionId": "..."}
{"type": "user", "message": {"role": "user", "content": "..."}}
{"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": "clean model output"}, {"type": "tool_use", "name": "Bash"}]}}
{"type": "summary", ...}
```

The `message.content` field for assistant messages contains **clean text without any ANSI codes or terminal artifacts** -- exactly what we want to send to Telegram.

## Requirement

Build a **JSONL file watcher** as an **alternative output source** for the Telegram bridge, specifically for Claude Code sessions. When a session is detected as Claude Code, the bridge should watch the JSONL session file instead of using the PTY-based pipeline.

### Must Have

1. **JSONL Watcher module** (`src-tauri/src/telegram/jsonl_watcher.rs` or similar):
   - Watch the session's JSONL file for new lines appended
   - Parse each new line, extract `role: "assistant"` messages
   - Extract text content from `message.content` (handle both `string` and `[{type: "text", text: "..."}]` formats)
   - Skip tool_use blocks, tool_result blocks, system messages, permission-mode entries
   - Feed extracted text to the existing buffer/send pipeline (Phase 5-6 of current bridge)

2. **Session JSONL path resolution:**
   - Reuse the existing mangling logic from `commands/session.rs:72-74`
   - Find the most recently modified `.jsonl` file in the project directory
   - The session file changes when Claude Code restarts, so must handle file rotation

3. **Integration with existing bridge:**
   - When `spawn_bridge()` is called and the session is detected as Claude Code:
     - Spawn the JSONL watcher task **instead of** the PTY output_task
     - The poll_task (Telegram -> PTY input) remains unchanged
   - When the session is NOT Claude Code, fall back to the existing PTY pipeline
   - The `BridgeHandle`, `BridgeInfo`, `TelegramBridgeManager` interfaces should NOT change

4. **Detection of Claude Code session:**
   - The `is_claude` flag already exists in `commands/session.rs:65`
   - Need to propagate this flag to the session metadata so the bridge can check it at attach time
   - Alternatively, check if the JSONL project dir exists at attach time

5. **File watching strategy:**
   - Use `notify` crate (already common in Rust ecosystem) OR simple polling (e.g., poll every 500ms, check file size, read new bytes)
   - Given that Claude Code writes infrequently (only on message completion), **polling is acceptable and simpler**
   - Track file position (byte offset) to only read new content

### Nice to Have (NOT in scope for v1)

- Watching multiple JSONL files if Claude starts a new session (file rotation)
- Sending user messages to Telegram (only assistant for now)
- Support for other coding agents (Codex, Cursor, etc.) -- this is explicitly deferred

### Architecture Constraint

The JSONL watcher must be **well-isolated** as a self-contained module. The intent is to later add similar watchers for other coding agents that have their own log formats. Think of it as:

```
telegram/
  bridge.rs          -- existing, orchestrates which output source to use
  jsonl_watcher.rs   -- NEW: Claude Code JSONL file watcher
  manager.rs         -- existing, unchanged interface
  ...
```

Future agents would add their own watcher modules (e.g., `codex_watcher.rs`) with the same output interface.

## Existing Code References

- **Path mangling:** `src-tauri/src/commands/session.rs:72-74`
- **Claude detection:** `src-tauri/src/commands/session.rs:65`
- **Bridge spawn:** `src-tauri/src/telegram/bridge.rs:413-452`
- **Output task (to be replaced for Claude):** `src-tauri/src/telegram/bridge.rs:470-499`
- **Buffer + send phases:** reuse from existing bridge.rs (Phase 5-6)
- **AgentFilter trait:** already exists in bridge.rs, may not be needed for JSONL (text is already clean)
- **Telegram API send:** `src-tauri/src/telegram/api.rs`

## JSONL Parsing Details

Each line is independent JSON. Relevant message types:

| type | role | action |
|------|------|--------|
| `"user"` | `"user"` | SKIP (v1) |
| `"assistant"` | `"assistant"` | EXTRACT text, send to Telegram |
| `"permission-mode"` | - | SKIP |
| `"summary"` | - | SKIP |

For `message.content`:
- If `string`: use directly
- If `array`: iterate, collect all `{type: "text", text: "..."}` blocks, join with newline
- Skip `{type: "tool_use", ...}` and `{type: "tool_result", ...}` blocks

## Open Questions for Architect

1. Should the watcher trait be generic from the start (e.g., `trait SessionLogWatcher`) or just a concrete struct for now?
2. Should we add a `notify` crate dependency or use simple polling? The JSONL is written infrequently.
3. Where should the `is_claude` flag be stored in session metadata? Currently it's a local variable in `create_session`. Options: add to `Session` struct, or re-derive at attach time from the session's CWD.

---

## Architect Design

**Author:** Architect Agent
**Date:** 2025-04-09

### Open Questions — Decisions

**Q1: Trait vs concrete struct?**
**Decision: Concrete struct, no trait for v1.** The plan's architecture constraint ("well-isolated, self-contained module") is satisfied by a standalone `jsonl_watcher.rs` module with a clean public API. A `trait SessionLogWatcher` adds coupling without a second implementation to validate it against. When `codex_watcher.rs` arrives, extracting a trait from two concrete implementations is trivial and will produce a better abstraction than one designed speculatively.

**Q2: `notify` crate vs polling?**
**Decision: Simple polling at 500ms.** Reasons:
- Claude Code writes JSONL infrequently (one line per completed message, not character-by-character)
- 500ms poll matches the existing `FLUSH_DELAY_MS` and is plenty responsive for Telegram delivery
- The file is append-only — polling is just: compare file size vs last offset, read delta, split by `\n`
- `notify` on Windows has known quirks (double events, file-lock interactions, missed events on some filesystems)
- Zero new dependencies

**Q3: Where to store `is_claude`?**
**Decision: Add `is_claude: bool` to the `Session` struct** in `session/session.rs`, with `#[serde(default)]` for backward compatibility with persisted sessions. Rationale:
- Already computed at creation time (`commands/session.rs:65`) — just persist the result
- Re-deriving from `shell`/`shell_args` at attach time is fragile (args get mutated with `--continue`, `--append-system-prompt-file`, etc.)
- The bridge attach path needs this flag, and the Session struct is the canonical place for session metadata
- `#[serde(default)]` means existing persisted sessions default to `false` (PTY mode) — correct behavior

---

### Architecture Overview

```
CURRENT (PTY pipeline):
  PTY bytes → output_senders[session_id] → rx channel → output_task (6 phases) → Telegram

NEW (JSONL mode, Claude Code sessions only):
  ~/.claude/projects/{mangled_cwd}/*.jsonl → poll → parse → buffer → Telegram
  (PTY bytes still flow to xterm.js, just not to Telegram)
```

The JSONL watcher **replaces** `output_task` (phases 1-4 are unnecessary — JSONL content is already clean text). It **reuses** the buffer/send logic (phases 5-6: `flush_buffer`, `chunk_text`).

---

### File Changes

#### 1. New file: `src-tauri/src/telegram/jsonl_watcher.rs`

Self-contained module. Public API:

```rust
/// Spawn a JSONL file watcher task that polls for new assistant messages
/// and sends them to Telegram via the shared buffer/send pipeline.
pub fn spawn_watch_task(
    cwd: String,
    bot_token: String,
    chat_id: i64,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle,
) -> tokio::task::JoinHandle<()>
```

Internal components:

```rust
/// Mangle a CWD path the same way Claude Code does.
/// Reuses the same logic from commands/session.rs:72-74.
fn mangle_cwd(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect()
}

/// Find the most recently modified .jsonl file in the project directory.
fn find_latest_jsonl(project_dir: &Path) -> Option<PathBuf> {
    // Read dir, filter *.jsonl, sort by modified time desc, return first
}

/// Parse a single JSONL line and extract assistant text content.
/// Returns None for non-assistant messages, tool_use blocks, system messages, etc.
fn extract_assistant_text(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    // Only process type == "assistant"
    // Handle content as String or Array of {type: "text", text: "..."} blocks
    // Skip tool_use, tool_result blocks
}

/// Read new bytes from a file starting at the given byte offset.
/// Returns parsed lines and updates the offset.
fn read_new_lines(path: &Path, offset: &mut u64) -> io::Result<Vec<String>> {
    // Seek to offset, read to end, split by \n
    // Handle partial last line (keep remainder for next poll)
}
```

**Main loop structure:**

```rust
async fn watch_loop(...) {
    let project_dir = home_dir().join(".claude/projects").join(mangle_cwd(&cwd));
    let mut current_file: Option<PathBuf> = None;
    let mut file_offset: u64 = 0;
    let mut line_remainder = String::new();  // partial line buffer
    
    // Buffer/send state (same as output_task phases 5-6)
    let client = reqwest::Client::builder()...;
    let mut buffer = String::new();
    let mut last_buffer_add = Instant::now();
    let mut logger = BridgeLogger::new(&session_id);
    let mut diag = DiagLogger::new();
    
    let mut poll_interval = tokio::time::interval(Duration::from_millis(500));
    
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = poll_interval.tick() => {
                let latest = find_latest_jsonl(&project_dir);
                
                // Handle file rotation
                if latest != current_file {
                    if current_file.is_none() {
                        // First attach: skip existing content
                        file_offset = latest.as_ref()
                            .and_then(|p| std::fs::metadata(p).ok())
                            .map(|m| m.len())
                            .unwrap_or(0);
                    } else {
                        // File rotation (new Claude session): read from start
                        file_offset = 0;
                    }
                    current_file = latest;
                    line_remainder.clear();
                }
                
                if let Some(ref path) = current_file {
                    if let Ok(new_lines) = read_new_lines(path, &mut file_offset, &mut line_remainder) {
                        for line in new_lines {
                            if let Some(text) = extract_assistant_text(&line) {
                                logger.log("JSONL_EXTRACT", &session_id, &text);
                                buffer.push_str(&text);
                                buffer.push('\n');
                                last_buffer_add = Instant::now();
                            }
                        }
                    }
                }
                
                // Flush decision (shared logic with output_task)
                if !buffer.is_empty() {
                    let elapsed = last_buffer_add.elapsed();
                    if elapsed >= flush_delay || buffer.len() > 2000 {
                        flush_buffer(&mut buffer, &client, &token, chat_id,
                            &session_id, &app, &mut logger, &mut diag).await;
                    }
                }
            }
        }
    }
}
```

#### 2. Modified: `src-tauri/src/telegram/mod.rs`

Add module declaration:
```rust
pub mod jsonl_watcher;  // NEW
```

#### 3. Modified: `src-tauri/src/telegram/bridge.rs`

**Extract shared utilities** — make `flush_buffer`, `chunk_text`, `BridgeLogger`, and `DiagLogger` visible to sibling modules:

```rust
// Change from private to pub(super):
pub(super) struct BridgeLogger { ... }
pub(super) struct DiagLogger { ... }
pub(super) async fn flush_buffer(...) { ... }
pub(super) fn chunk_text(...) -> Vec<String> { ... }
```

**Modify `spawn_bridge` signature** — add `jsonl_cwd` parameter:

```rust
pub fn spawn_bridge(
    bot_token: String,
    chat_id: i64,
    session_id: Uuid,
    info: BridgeInfo,
    pty_mgr: Arc<Mutex<PtyManager>>,
    app_handle: tauri::AppHandle,
    jsonl_cwd: Option<String>,  // NEW: if Some, use JSONL watcher instead of PTY pipeline
) -> BridgeHandle {
    let cancel = CancellationToken::new();
    let (tx, rx) = mpsc::channel::<Vec<u8>>(256);
    let session_id_str = session_id.to_string();

    if let Some(cwd) = jsonl_cwd {
        // JSONL mode: watch Claude Code session log
        drop(rx);  // not needed — no PTY bytes feed
        super::jsonl_watcher::spawn_watch_task(
            cwd,
            bot_token.clone(),
            chat_id,
            session_id_str.clone(),
            cancel.clone(),
            app_handle.clone(),
        );
    } else {
        // PTY mode: existing 6-phase pipeline
        tokio::spawn(output_task(rx, bot_token.clone(), chat_id,
            session_id_str.clone(), cancel.clone(), app_handle.clone()));
    }

    // poll_task runs in BOTH modes (Telegram → PTY input is always needed)
    tokio::spawn(poll_task(bot_token, chat_id, session_id, session_id_str,
        pty_mgr, cancel.clone(), app_handle));

    BridgeHandle { info, cancel, output_sender: tx }
}
```

Note: `tx` is still created and stored in `BridgeHandle` even in JSONL mode. It's unused but harmless — keeps `BridgeHandle` unchanged.

#### 4. Modified: `src-tauri/src/telegram/manager.rs`

**Modify `attach` signature** — add `jsonl_cwd` and conditionally skip `output_senders` registration:

```rust
pub fn attach(
    &mut self,
    session_id: Uuid,
    bot: &TelegramBotConfig,
    pty_mgr: Arc<Mutex<PtyManager>>,
    app_handle: tauri::AppHandle,
    jsonl_cwd: Option<String>,  // NEW
) -> Result<BridgeInfo, AppError> {
    // ... existing validation unchanged ...

    let handle = bridge::spawn_bridge(
        bot.token.clone(), bot.chat_id, session_id,
        info.clone(), pty_mgr, app_handle,
        jsonl_cwd.clone(),  // NEW: pass through
    );

    // Only register output sender for PTY mode.
    // In JSONL mode, the watcher reads directly from file — no PTY byte feed needed.
    if jsonl_cwd.is_none() {
        if let Ok(mut senders) = self.output_senders.lock() {
            senders.insert(session_id, handle.output_sender.clone());
        }
    }

    // ... rest unchanged ...
}
```

#### 5. Modified: `src-tauri/src/session/session.rs`

Add `is_claude` field to `Session` struct:

```rust
pub struct Session {
    // ... existing fields ...
    
    /// True if this session runs Claude Code (detected at creation time).
    /// Used by the Telegram bridge to choose JSONL watcher vs PTY pipeline.
    #[serde(default)]
    pub is_claude: bool,
}
```

#### 6. Modified: `src-tauri/src/commands/session.rs`

At session creation (after `is_claude` is computed on line 65), set it on the session:

```rust
// After session is created, set the flag:
session.is_claude = is_claude;
```

At all `tg.attach(...)` call sites (lines 397, 575, 815), pass the CWD when session is Claude:

```rust
let jsonl_cwd = if session.is_claude {
    Some(session.working_directory.clone())
} else {
    None
};
tg.attach(id, &bot, pty_arc, app.clone(), jsonl_cwd)
```

#### 7. Modified: `src-tauri/src/commands/telegram.rs`

The `telegram_attach` command (line 34) needs to look up the session to check `is_claude`:

```rust
// Look up session to check is_claude flag
let mgr = session_mgr.read().await;
let session = mgr.get_session(uuid).ok_or("Session not found")?;
let jsonl_cwd = if session.is_claude {
    Some(session.working_directory.clone())
} else {
    None
};
drop(mgr);

let info = tg.attach(uuid, &bot, pty_arc, app.clone(), jsonl_cwd)
    .map_err(|e| e.to_string())?;
```

---

### Design Decisions — Details

#### Initial File Offset Strategy

When the JSONL watcher first starts (bridge attached to an existing session):
- **Seek to END of file** — skip historical messages. The user attached the bridge "now"; they want future messages, not a dump of the entire conversation so far.

When a file rotation occurs (Claude restarts, new session file):
- **Start from offset 0** — this is a new conversation; capture everything.

#### Partial Line Handling

JSONL files are written one complete line at a time by Claude Code, but the OS may buffer writes. The watcher must handle reading a partial line at the end of a file:
- `read_new_lines` keeps a `line_remainder: String` buffer
- Bytes read are appended to `line_remainder`, then split by `\n`
- Only complete lines (terminated by `\n`) are returned for parsing
- The unterminated tail stays in `line_remainder` for the next poll

#### No `output_senders` Registration for JSONL Mode

In PTY mode, the PTY read loop sends bytes to `output_senders[session_id]`, which feeds the bridge's `output_task`. In JSONL mode, the watcher reads directly from the filesystem — no byte channel needed. Skipping registration avoids PTY bytes being sent to a channel that nobody reads (the `rx` is dropped).

#### Logging

The JSONL watcher uses the same `BridgeLogger` and `DiagLogger` as `output_task`. Log tags:
- `JSONL_INIT` — watcher started, project dir path
- `JSONL_FILE` — current file path and offset
- `JSONL_ROTATE` — file rotation detected
- `JSONL_EXTRACT` — assistant text extracted (truncated in log)
- `JSONL_ERR` — file read or parse errors

---

### Files Summary

| File | Action | Description |
|------|--------|-------------|
| `telegram/jsonl_watcher.rs` | **CREATE** | New module: JSONL file watcher with polling, parsing, buffer/send |
| `telegram/mod.rs` | MODIFY | Add `pub mod jsonl_watcher;` |
| `telegram/bridge.rs` | MODIFY | Add `jsonl_cwd` param to `spawn_bridge`; make `flush_buffer`, `chunk_text`, `BridgeLogger`, `DiagLogger` `pub(super)` |
| `telegram/manager.rs` | MODIFY | Add `jsonl_cwd` param to `attach`; conditional `output_senders` registration |
| `session/session.rs` | MODIFY | Add `is_claude: bool` field with `#[serde(default)]` |
| `commands/session.rs` | MODIFY | Set `session.is_claude` at creation; pass `jsonl_cwd` to all `tg.attach()` calls |
| `commands/telegram.rs` | MODIFY | Look up session to derive `jsonl_cwd`; pass to `tg.attach()` |

### Dependencies

No new crate dependencies. Uses only: `serde_json` (already in Cargo.toml), `std::fs`, `std::io::Seek`, `tokio::time`, `tokio_util::sync::CancellationToken` (already used).

### Risk Assessment

- **Low risk**: `Session` struct change is backward-compatible (`#[serde(default)]`)
- **Low risk**: PTY pipeline unchanged — JSONL is an alternative path, not a modification
- **Medium risk**: JSONL file path resolution depends on Claude Code's undocumented mangling convention — but we already implement it (`commands/session.rs:72-74`) and it works
- **Low risk**: Polling approach has no race conditions (append-only file, single reader)

---

## Dev-Rust Review

**Author:** Dev-Rust Agent
**Date:** 2026-04-09

After reading the full plan, the architect design, and all referenced source files, here are implementation-critical additions. Each item includes the reasoning behind it.

---

### 1. CRITICAL — `Session.is_claude` Won't Propagate to SessionManager

The architect says to set `session.is_claude = is_claude` in `create_session_inner` after the session is created. But `SessionManager::create_session()` (manager.rs:26-71) returns a **clone** of the `Session` — the original lives inside `SessionManager.sessions: Arc<RwLock<HashMap<Uuid, Session>>>`.

Setting `is_claude` on the returned clone does NOT update the HashMap. When `telegram.rs:telegram_attach` later calls `mgr.get_session(uuid)`, it reads from the HashMap — where `is_claude` is still `false`.

**Fix options (pick one):**
- **(A) Add `set_is_claude` method to `SessionManager`** — cleanest, follows existing pattern (`set_last_prompt`, `set_git_branch`):
  ```rust
  pub async fn set_is_claude(&self, id: Uuid, val: bool) {
      let mut sessions = self.sessions.write().await;
      if let Some(s) = sessions.get_mut(&id) { s.is_claude = val; }
  }
  ```
  Then in `create_session_inner`: `mgr.set_is_claude(id, is_claude).await;`

- **(B) Pass `is_claude` to `SessionManager::create_session`** — adds a parameter to the function signature but avoids the extra async call.

**Recommendation: Option A** — it doesn't change `create_session`'s signature (which is called from manager.rs too) and follows the established pattern.

---

### 2. `chunk_text` Has a Latent UTF-8 Panic

`bridge.rs:634-636`:
```rust
let end = (start + max_len).min(text.len());
let actual_end = if end < text.len() {
    text[start..end].rfind('\n')  // ← panics if `end` not on char boundary
```

`text[start..end]` will panic at runtime if `end` lands in the middle of a multi-byte UTF-8 character. The PTY pipeline rarely hits this because `vt100::Screen::contents_between` produces clean ASCII-heavy strings. But JSONL content from Claude contains arbitrary Unicode (emoji, CJK, accented chars, code with Unicode identifiers).

**Why this matters now:** The JSONL watcher feeds raw UTF-8 text from JSONL directly into `flush_buffer` → `chunk_text`. A 4001-byte assistant message with a 3-byte emoji near the 4000 boundary WILL panic.

**Fix:** Snap `end` backward to the nearest char boundary before slicing:
```rust
let mut end = (start + max_len).min(text.len());
while end > start && !text.is_char_boundary(end) {
    end -= 1;
}
```

This fix benefits both the existing PTY pipeline and the new JSONL path.

---

### 3. Windows File Sharing — Open Mode Matters

The architect says "polling is simple: compare file size vs last offset, read delta." True, but on Windows, file reads can fail with `ERROR_SHARING_VIOLATION` if the writer (Claude Code) holds a conflicting lock.

`std::fs::File::open()` on Windows uses `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE` by default, so our reads will succeed even while Claude writes. **However**, `std::fs::metadata()` (used for checking file size) can also contend with the writer.

**Implementation guideline:** Use `File::open()` + `file.metadata()` on the already-opened handle, rather than calling `std::fs::metadata(path)` separately. The handle-based metadata avoids a second filesystem access that could race.

**Recommended `read_new_lines` pattern:**
```rust
fn read_new_lines(path: &Path, offset: &mut u64, remainder: &mut String) -> io::Result<Vec<String>> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();  // metadata on open handle, not path
    if file_len <= *offset { return Ok(vec![]); }
    file.seek(SeekFrom::Start(*offset))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    *offset = file_len;
    // ... split lines, handle remainder ...
}
```

Re-opening the file each poll cycle (instead of keeping a persistent handle) is actually preferable here:
- Avoids stale handles across file rotation
- Avoids holding a handle that could interfere with Claude Code
- Simpler state management (no handle in the watcher struct)

---

### 4. Project Directory May Not Exist Yet

When the Telegram bridge is attached to a freshly created Claude session, Claude Code may not have written any output yet. The directory `~/.claude/projects/{mangled_cwd}/` might not exist at attach time.

The architect's `watch_loop` initializes `project_dir` and immediately starts polling, but `find_latest_jsonl` will fail on a non-existent directory.

**Fix:** `find_latest_jsonl` should return `None` gracefully when the directory doesn't exist (not error). The watcher should keep polling — the directory will appear once Claude writes its first message. Log `JSONL_WAIT` on first poll where the directory doesn't exist, then `JSONL_INIT` when it appears.

---

### 5. Extract `mangle_cwd` as Shared Utility

The mangling logic exists in `commands/session.rs:72-74` as inline code:
```rust
let mangled: String = cwd.chars().map(|c| {
    if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }
}).collect();
```

The architect's plan places a second copy in `jsonl_watcher.rs`. Two copies = guaranteed drift when Claude Code changes its mangling convention.

**Fix:** Extract to a shared utility function (e.g., in `session/session.rs` or a `utils.rs`):
```rust
pub fn mangle_cwd_for_claude(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect()
}
```

Then both `commands/session.rs` and `jsonl_watcher.rs` call the same function.

---

### 6. File Rotation Flicker Risk

`find_latest_jsonl` sorts by modified time and returns the most recent. If Claude Code creates a new session file while the old one's modified time is within the same second (filesystem granularity), the "latest" could flicker between files across polls.

**Mitigation:** Once tracking a file, only switch to a different file if:
1. The new file has a strictly newer modified time, AND
2. The current file's modified time is older than 3 seconds (stale)

This prevents oscillation during file transitions. Simple to implement as a guard in the `if latest != current_file` block.

---

### 7. Session Restore Must Also Pass `jsonl_cwd`

The app restores sessions on startup via `create_session_inner` → auto-attach Telegram (in `create_session`, lines 378-403 and `create_root_agent_session`, lines 801-821). All three `tg.attach()` call sites need the `jsonl_cwd` parameter.

Currently identified call sites for `tg.attach()`:
1. `commands/session.rs:397` — `create_session` (auto-attach after creation)
2. `commands/session.rs:575` — `restart_session` (re-attach after restart)
3. `commands/session.rs:815` — `create_root_agent_session` (auto-attach)
4. `commands/telegram.rs:34` — `telegram_attach` (manual attach from UI)

**All four** need the `jsonl_cwd` parameter. The architect only explicitly mentions 1, 2, 3, and 4 but the implementation must not miss any. Use `grep -rn "tg.attach\|\.attach("` on the telegram-related files to catch any future additions.

**Additionally:** The `is_claude` flag must be available at all attach sites. For sites 1-3, `create_session_inner` already computes `is_claude`. For site 4, the architect correctly notes we need to look up the session. But since `is_claude` needs to be stored on the `Session` in the manager (see point #1), site 4 becomes:
```rust
let session = mgr.get_session(uuid).await.ok_or("Session not found")?;
let jsonl_cwd = if session.is_claude { Some(session.working_directory.clone()) } else { None };
```

---

### 8. `read_to_string` Can Fail on Invalid UTF-8

JSONL files from Claude Code should always be valid UTF-8, but if a file gets corrupted or truncated mid-write (crash, power loss), `read_to_string` will return `Err`. The watcher should handle this gracefully:
- If `read_to_string` fails, log the error and skip this poll cycle
- Do NOT reset `offset` — retry from the same position next time
- Consider using `read_to_end` (returns bytes) + `String::from_utf8_lossy` as a more resilient alternative

---

### 9. Visibility Changes Should Be `pub(crate)` Not `pub(super)`

The architect proposes making `flush_buffer`, `chunk_text`, `BridgeLogger`, `DiagLogger` as `pub(super)`. This limits visibility to the `telegram` module. While correct for the current design, using `pub(crate)` instead costs nothing extra and allows future modules outside `telegram/` to reuse the buffer/send logic without a second visibility change. Minor preference — `pub(super)` is also acceptable.

---

### 10. Additional Risk — `home_dir()` Returns `None`

`dirs::home_dir()` can return `None` on unusual system configurations (no `USERPROFILE` on Windows, containerized environments). The existing code in `commands/session.rs:71` handles this by short-circuiting. The watcher must do the same — if `home_dir()` returns `None`, the watcher should log an error and enter a dormant state (keep running but skip polling), not panic.

---

### Implementation Sequence Recommendation

Based on the dependency graph, I recommend this build order:

1. **Session struct + manager** — Add `is_claude: bool` to `Session`, add `set_is_claude()` to `SessionManager`, update `SessionManager::create_session` struct literal
2. **Shared utility** — Extract `mangle_cwd_for_claude()`, update `commands/session.rs` to use it
3. **Set `is_claude` in `create_session_inner`** — After session creation, call `mgr.set_is_claude(id, is_claude).await`
4. **`chunk_text` UTF-8 fix** — Fix char boundary bug (benefits both pipelines)
5. **Visibility changes in `bridge.rs`** — Make `flush_buffer`, `chunk_text`, `BridgeLogger`, `DiagLogger` pub(super)
6. **New module `jsonl_watcher.rs`** — Core implementation with polling, parsing, buffer/send
7. **Wire into `bridge.rs`** — Add `jsonl_cwd` to `spawn_bridge`
8. **Wire into `manager.rs`** — Add `jsonl_cwd` to `attach`, conditional sender registration
9. **Wire into all `tg.attach()` call sites** — `commands/session.rs` (3 sites) + `commands/telegram.rs` (1 site)
10. **Register module in `telegram/mod.rs`**

Each step should compile independently. Steps 1-5 can be done as a preparatory commit before the main feature commit (6-10).

---

## Grinch Review

**Author:** Dev-Rust-Grinch Agent
**Date:** 2026-04-09

I reviewed the full plan (requirement + architect design + dev-rust review) AND read all referenced source files (`bridge.rs`, `manager.rs`, `session/session.rs`, `commands/session.rs`, `commands/telegram.rs`, `telegram/types.rs`, `telegram/api.rs`). Below are problems that the previous reviews missed or insufficiently addressed. Severity ratings reflect production impact likelihood.

---

### G1. HIGH — No Final Flush on Cancel (Buffer Content Silently Lost)

The existing `output_task` (bridge.rs:556-574) has explicit **final harvest + final flush** logic after the main loop breaks:

```rust
// Final harvest + flush
if tracker.has_pending() {
    tokio::time::sleep(Duration::from_millis(STABILIZATION_MS + 100)).await;
    let stable_lines = tracker.harvest_stable(filter.as_ref());
    // ... accumulate to buffer
}
if !buffer.is_empty() {
    flush_buffer(&mut buffer, ...).await;
}
```

The architect's `watch_loop` design does NOT include any post-loop flush. When `cancel.cancelled()` fires (bridge detach or app exit), the loop breaks immediately and any content sitting in `buffer` is discarded.

**Production scenario:** Claude finishes writing a response → JSONL line is parsed and buffered → user detaches bridge within the 500ms flush window → last message never reaches Telegram. This will happen frequently because detach typically follows "I see Claude finished."

**Fix:** Add post-loop final poll + flush after the `loop { ... }` block:

```rust
// After loop breaks:
// One final read in case content arrived between last tick and cancel
if let Some(ref path) = current_file {
    if let Ok(new_lines) = read_new_lines(path, &mut file_offset, &mut line_remainder) {
        for line in new_lines {
            if let Some(text) = extract_assistant_text(&line) {
                buffer.push_str(&text);
                buffer.push('\n');
            }
        }
    }
}
if !buffer.is_empty() {
    flush_buffer(&mut buffer, &client, &token, chat_id,
        &session_id, &app, &mut logger, &mut diag).await;
}
```

---

### G2. HIGH — Offset Tracking Bug: `*offset = file_len` Can Skip Data

The dev-rust review (#3) recommends this `read_new_lines` pattern:

```rust
let file_len = file.metadata()?.len();
if file_len <= *offset { return Ok(vec![]); }
file.seek(SeekFrom::Start(*offset))?;
let mut buf = String::new();
file.read_to_string(&mut buf)?;
*offset = file_len;       // ← BUG
```

Setting `*offset = file_len` assumes `buf.len() == file_len - *offset`. This is NOT guaranteed:

1. **File grows during read:** `read_to_string` reads until actual EOF (which may be beyond `file_len` if Claude wrote more data between the `metadata()` call and the `read`). Setting `offset = file_len` means next poll re-reads those extra bytes → **duplicate messages to Telegram**.

2. **Buffered writes on Windows:** NTFS can report `metadata().len()` reflecting unflushed write buffers. If the OS reports a larger size than what's actually readable at that instant, `read_to_string` returns fewer bytes than `file_len - offset`. Setting `offset = file_len` → **skipped data**.

**Fix:** Track offset by actual bytes read, not reported file length:

```rust
*offset += buf.len() as u64;
```

This is always correct regardless of filesystem timing quirks.

---

### G3. HIGH — File Truncation/Shrink Not Handled (Watcher Gets Stuck)

The architect's design only handles two cases: `current_file == None` (first attach) and `latest != current_file` (file rotation by name change). A third case is missed:

**Same filename, smaller file.** If Claude Code deletes and recreates a session file with the same name, or if the file is truncated (crash recovery, manual cleanup), the file path stays the same but the file length drops below `offset`. The `file_len <= *offset` check in `read_new_lines` returns early with no new lines — forever. The watcher appears alive but never produces output.

**Fix:** Detect shrink and reset:

```rust
if file_len < *offset {
    // File was truncated or replaced — reset to beginning
    log::warn!("[JSONL_TRUNCATE] File shrank ({} < {}), resetting offset", file_len, *offset);
    *offset = 0;
    remainder.clear();
}
```

---

### G4. HIGH — `thinking` Blocks Not Filtered (Telegram Spam)

The JSONL parsing table (Section "JSONL Parsing Details") only mentions filtering `tool_use` and `tool_result` from the content array. Claude Code's assistant messages also include `{type: "thinking", thinking: "..."}` blocks in `message.content`.

These blocks contain the model's internal chain-of-thought reasoning. They are:
- **Enormous** — frequently 5-20KB of dense text per message
- **Not user-facing** — Claude Code collapses these in its own UI
- **Potentially sensitive** — raw reasoning may include discarded approaches, security considerations, or confused intermediate logic

If not filtered, every Claude response sends the full thinking block to Telegram first, followed by the actual response. A 15KB thinking block becomes 4 Telegram messages of internal monologue before the 200-byte actual answer.

**Fix:** Add `thinking` to the filtered block types in `extract_assistant_text`:

```rust
// Skip non-text content blocks
if block_type != "text" {
    continue; // Filters: tool_use, tool_result, thinking, and any future block types
}
```

Using a whitelist (`== "text"`) instead of a blacklist is safer — it automatically filters any new block types Claude adds in the future.

---

### G5. MEDIUM — No `telegram_bridge_error` Events (Silent Failures for the UI)

The existing `output_task` emits `telegram_bridge_error` events when Telegram sends fail (bridge.rs:614-620). The frontend uses these to show error indicators in the UI.

The architect's JSONL watcher design mentions logging (JSONL_ERR, etc.) but does NOT mention emitting `telegram_bridge_error` events. If the JSONL file can't be read, or Telegram sends fail from the watcher, the UI will show the bridge as "Active" with no error indication. The user won't know output has stopped.

**Fix:** The JSONL watcher should emit the same events as `output_task`. Since `flush_buffer` already handles Telegram send errors with event emission, the main gap is file I/O errors. Add event emission when:
- `File::open()` fails (permission error, file locked)
- `read_to_string()` fails (invalid UTF-8, I/O error)
- Project directory doesn't exist after extended period (>30s of polling with no directory)

---

### G6. MEDIUM — Memory Spikes on Large JSONL Lines

`extract_assistant_text` parses each line with `serde_json::from_str::<Value>(line)`. This builds a full DOM tree in memory. JSONL lines vary wildly in size:

- `permission-mode` entries: ~100 bytes
- `user` messages: 100 bytes - 10KB
- `assistant` messages: 200 bytes - 50KB
- **`tool_result` entries: can be 1-10MB** (contain full file contents from Read/Grep tool calls, base64 images, large command outputs)

A 5MB `tool_result` line → ~15MB serde_json::Value DOM allocation → parsed → found to be non-assistant → immediately dropped. This creates a memory spike every time Claude reads a large file.

**Mitigation options (pick one):**
- **(A) Pre-filter by line prefix** — Before parsing JSON, check if the line contains `"type":"assistant"` or `"type": "assistant"` with a simple string search. Skip parsing entirely if not found. ~95% of lines are filtered without allocation.
- **(B) Use `serde_json::from_str::<RawValue>`** and only parse the `type` field first, then parse the full value only for assistant messages.

**Recommendation: Option A** — simplest, zero dependencies, handles the 99% case. A `tool_result` line will never contain `"type":"assistant"` or `"type": "assistant"` as a substring.

```rust
fn extract_assistant_text(line: &str) -> Option<String> {
    // Fast-path: skip lines that can't be assistant messages
    if !line.contains("\"type\":\"assistant\"") && !line.contains("\"type\": \"assistant\"") {
        return None;
    }
    // Full parse only for candidate lines
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    // ... extract text content
}
```

---

### G7. MEDIUM — Panicked Watcher Task = Silently Dead Bridge

If `spawn_watch_task` panics at runtime (malformed JSON hitting an `unwrap`, integer overflow, etc.), the tokio task dies silently. The `CancellationToken` is never cancelled. The `BridgeHandle` remains in `TelegramBridgeManager::bridges` with status `Active`. The UI shows an active bridge, but no output reaches Telegram.

This is the same problem the existing `output_task` has — neither stores JoinHandles. But the JSONL watcher introduces new code paths (file I/O, JSON parsing) with more surface area for panics than the well-tested PTY pipeline.

**Fix:** Wrap the watch loop in `catch_unwind` or, better, use a `tokio::spawn` wrapper that logs panics and emits a `telegram_bridge_error` event:

```rust
pub fn spawn_watch_task(...) -> JoinHandle<()> {
    tokio::spawn(async move {
        watch_loop(...).await;
        // If we get here normally (cancel), fine.
        // If the task panicked, tokio::spawn catches it.
        // Log either way:
        log::info!("[JSONL_EXIT] Watcher task ended for session {}", session_id);
    })
}
```

And in the caller, optionally monitor the JoinHandle for panic:

```rust
let handle = spawn_watch_task(...);
tokio::spawn(async move {
    if let Err(e) = handle.await {
        log::error!("[JSONL_PANIC] Watcher panicked: {:?}", e);
        app.emit("telegram_bridge_error", ...);
    }
});
```

This is a **defense-in-depth** measure. The primary fix is to ensure no `unwrap()`s exist in the watcher code.

---

### G8. MEDIUM — `flush_buffer` Line Dedup Strips Legitimate Repeated Content

`flush_buffer` (bridge.rs:591-600) deduplicates consecutive identical lines:

```rust
if lines.last().map(|l: &&str| l.trim()) != Some(trimmed) {
    lines.push(line);
}
```

In the PTY pipeline, this is valuable — screen redraws cause duplicate rows. But in JSONL mode, the text is Claude's actual response content. Legitimate repeated lines (code examples with identical lines, numbered lists, ASCII art, table rows) will be silently stripped.

**Example that breaks:**

```
def func_a():
    pass

def func_b():
    pass
```

The two `    pass` lines are consecutive and identical → second one is stripped → Telegram receives syntactically broken Python.

**Fix:** Either:
- **(A)** Skip the dedup logic in JSONL mode (add a `dedup: bool` parameter to `flush_buffer`), or
- **(B)** Remove the dedup logic entirely from `flush_buffer` and add it only in the PTY pipeline's buffer accumulation step (where it belongs — PTY screen redraws are the source of duplicates, not the send pipeline)

**Recommendation: Option A** — least invasive.

---

### G9. LOW — `result` Type JSONL Entries Not Documented

The JSONL parsing table lists: `user`, `assistant`, `permission-mode`, `summary`. Claude Code also writes entries with `type: "result"` that contain the final result metadata for the conversation turn. These include a `result` field (not `message`), and the existing extract logic will correctly return `None` for them (no `message.content` path). But they should be explicitly documented in the parsing table so implementers don't wonder if they're missing content.

Additionally, Claude Code may write `type: "system"` entries for system prompts. These should also be explicitly listed as SKIP in the table.

Updated table:

| type | action |
|------|--------|
| `"user"` | SKIP (v1) |
| `"assistant"` | EXTRACT text blocks from `message.content` |
| `"permission-mode"` | SKIP |
| `"summary"` | SKIP |
| `"result"` | SKIP (metadata, no user-facing content) |
| `"system"` | SKIP (system prompt) |
| unknown | SKIP (future-proof) |

---

### G10. LOW — TOCTOU in `find_latest_jsonl` + `read_new_lines`

`find_latest_jsonl` reads the directory listing and selects the most recent `.jsonl` file. Then `read_new_lines` opens that file. Between these two calls, the file could be deleted (if Claude Code session cleanup runs). The `File::open()` would fail with `NotFound`.

This is already handled gracefully if `read_new_lines` returns `Err` and the watcher skips the poll cycle (as dev-rust suggested in #8). But worth confirming the implementer doesn't `unwrap()` the `File::open()` result.

---

### Summary Table

| # | Severity | Issue | Who Missed It |
|---|----------|-------|---------------|
| G1 | HIGH | No final flush on cancel — buffer content lost | Architect |
| G2 | HIGH | Offset = file_len skips data on race | Dev-Rust (introduced in their own fix) |
| G3 | HIGH | File truncation → watcher stuck forever | Architect + Dev-Rust |
| G4 | HIGH | `thinking` blocks flood Telegram (5-20KB per message) | Architect + Dev-Rust |
| G5 | MEDIUM | No `telegram_bridge_error` events from JSONL path | Architect + Dev-Rust |
| G6 | MEDIUM | serde_json::Value DOM spike on multi-MB lines | Architect + Dev-Rust |
| G7 | MEDIUM | Panicked watcher = silently dead bridge | Architect + Dev-Rust |
| G8 | MEDIUM | `flush_buffer` dedup strips legitimate repeated lines | Architect + Dev-Rust |
| G9 | LOW | `result`/`system` JSONL types undocumented | Architect |
| G10 | LOW | TOCTOU between find_latest and open | Everyone (minor) |
