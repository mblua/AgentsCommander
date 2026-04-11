# Plan: Add configDir to AgentConfig

**Branch:** `feature/per-project-coding-agents`
**Status:** Draft
**Created:** 2026-04-11

---

## Problem Statement

When AC creates a Claude session, it auto-injects `--continue` if prior conversations exist. The check (in `session.rs` lines 77–107) hardcodes `~/.claude/projects/{mangled-cwd}/`. Custom Claude binaries (e.g. `claude-phi`) use a different config directory (e.g. `~/.claude-phi/`) set via `CLAUDE_CONFIG_DIR` in a `.cmd` wrapper. AC checks the wrong directory, finds old conversations from a different binary, injects `--continue`, and the new binary fails with "No conversation found to continue."

The current workaround (`shell_was_explicit` from commit 2307f05) disables `--continue` entirely when a project-level agent provides an explicit shell. This is too broad — it disables `--continue` for ALL project agents, including ones that should use it.

## Solution Overview

1. Add optional `configDir` field to `AgentConfig` (Rust + TS)
2. Map known binaries to their default config dirs (`claude` → `~/.claude`, etc.)
3. Use `configDir` in the `--continue` check instead of hardcoded `~/.claude`
4. Show `configDir` in the UI — auto-filled for known binaries, required for unknown ones
5. Revert the `shell_was_explicit` workaround (commit 2307f05)
6. Also fix the JSONL watcher which hardcodes `~/.claude`

---

## 1. Data Model Changes

### 1.1 Rust: `AgentConfig` in `src-tauri/src/config/settings.rs`

**Current struct (lines 8–21):**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    pub id: String,
    pub label: String,
    pub command: String,
    pub color: String,
    #[serde(default)]
    pub git_pull_before: bool,
    #[serde(default)]
    pub exclude_global_claude_md: bool,
}
```

**Add after `exclude_global_claude_md`:**
```rust
    /// Optional override for the binary's config directory (e.g. "~/.claude-phi").
    /// When None, resolved from the command's binary name via known defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<String>,
```

### 1.2 TypeScript: `AgentConfig` in `src/shared/types.ts`

**Current interface (lines 75–82):**
```typescript
export interface AgentConfig {
  id: string;
  label: string;
  command: string;
  color: string;
  gitPullBefore: boolean;
  excludeGlobalClaudeMd: boolean;
}
```

**Add after `excludeGlobalClaudeMd`:**
```typescript
  configDir?: string;
```

### 1.3 Agent Presets: `src/shared/agent-presets.ts`

Update preset configs to include `configDir`:

**Claude Code preset (line 17–23):**
```typescript
config: {
  label: "Claude Code",
  command: "claude",
  color: "#d97706",
  gitPullBefore: false,
  excludeGlobalClaudeMd: true,
  configDir: "~/.claude",
},
```

**Codex preset (line 30–36):**
```typescript
config: {
  label: "Codex",
  command: "codex",
  color: "#10b981",
  gitPullBefore: false,
  excludeGlobalClaudeMd: false,
  // configDir: undefined — codex does not use --continue
},
```

**Gemini CLI preset (line 43–49):**
```typescript
config: {
  label: "Gemini CLI",
  command: "gemini",
  color: "#4285f4",
  gitPullBefore: false,
  excludeGlobalClaudeMd: false,
  // configDir: undefined — gemini does not use --continue
},
```

**Note:** Only Claude-based agents need `configDir` since `--continue` is a Claude-specific feature. Codex and Gemini presets leave it undefined.

---

## 2. Backend Changes (Rust)

### 2.1 New Helper: `resolve_config_dir` in `src-tauri/src/commands/session.rs`

Add a new function (near `executable_basename` around line 698):

```rust
/// Resolve the config directory for a Claude-like agent.
/// Priority: explicit configDir from AgentConfig > default mapping by binary name.
/// Returns None if the agent is not Claude-based or has no known config dir.
fn resolve_config_dir(
    agent_config: Option<&AgentConfig>,
    shell: &str,
    shell_args: &[String],
) -> Option<std::path::PathBuf> {
    // 1. If agent has explicit configDir, expand ~ and return it
    if let Some(cfg) = agent_config {
        if let Some(ref dir) = cfg.config_dir {
            if !dir.is_empty() {
                return Some(expand_tilde(dir));
            }
        }
    }

    // 2. Fall back to known defaults by binary basename
    let full_cmd = format!("{} {}", shell, shell_args.join(" "));
    let basenames: Vec<String> = full_cmd
        .split_whitespace()
        .map(|t| executable_basename(t))
        .collect();

    // "claude" -> ~/.claude (the standard Claude Code binary)
    if basenames.iter().any(|b| b == "claude") {
        return dirs::home_dir().map(|h| h.join(".claude"));
    }

    // Any other binary starting with "claude" (e.g. claude-phi, claude-dev)
    // is Claude-based but config dir is unknown — return None to skip --continue
    // (user should set configDir explicitly for these)
    None
}

/// Expand leading ~ to the user's home directory.
fn expand_tilde(path: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if let Some(rest) = path.strip_prefix("~\\") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    std::path::PathBuf::from(path)
}
```

### 2.2 Rewrite `--continue` Logic in `create_session_inner`

**File:** `src-tauri/src/commands/session.rs`

**Replace lines 77–107** (the current `claude_project_exists` check + injection block) with:

```rust
// Auto-inject --continue for Claude agents when prior conversation exists
// in the CORRECT config directory for this specific agent/binary.
if is_claude && !skip_continue {
    // Look up the AgentConfig for this agent_id
    let agent_config: Option<AgentConfig> = if let Some(ref aid) = agent_id {
        let settings_state = app.state::<SettingsState>();
        let cfg = settings_state.read().await;
        cfg.agents.iter().find(|a| a.id == *aid).cloned()
    } else {
        None
    };

    let config_dir = resolve_config_dir(agent_config.as_ref(), &shell, &shell_args);

    let has_prior_conversation = config_dir
        .map(|dir| {
            let mangled = crate::session::session::mangle_cwd_for_claude(&cwd);
            dir.join("projects").join(&mangled).is_dir()
        })
        .unwrap_or(false);

    if has_prior_conversation {
        if let Some(ref aid) = agent_id {
            let already_has_continue = full_cmd.split_whitespace().any(|t| {
                let lower = t.to_lowercase();
                lower == "--continue" || lower == "-c"
            });
            if !already_has_continue {
                if executable_basename(&shell) == "cmd" {
                    if let Some(last) = shell_args.last_mut() {
                        if executable_basename(last) == "claude"
                            || last.to_lowercase().contains("claude")
                        {
                            *last = format!("{} --continue", last);
                            log::info!(
                                "Auto-injected --continue for agent '{}' (prior conversation in {:?})",
                                aid, config_dir
                            );
                        }
                    }
                } else {
                    shell_args.push("--continue".to_string());
                    log::info!(
                        "Auto-injected --continue for agent '{}' (prior conversation in {:?})",
                        aid, config_dir
                    );
                }
            }
        }
    }
}
```

**Key differences from current code:**
- Removed `!shell_was_explicit` condition — configDir solves the root cause
- Looks up the `AgentConfig` to read its `configDir`
- Uses `resolve_config_dir()` instead of hardcoded `~/.claude`
- If `configDir` resolves to `None` (unknown binary, no explicit config), skips `--continue` safely

### 2.3 Revert `shell_was_explicit` (commit 2307f05)

**Changes to revert across these files:**

#### `src-tauri/src/commands/session.rs`

1. **`create_session_inner` signature (line 37):** Remove `shell_was_explicit: bool` parameter
2. **Line 87:** Remove `&& !shell_was_explicit` from the condition (replaced by configDir logic above)
3. **`create_session` (line 340):** Remove `let shell_was_explicit = shell.is_some();`
4. **Line ~386:** Remove `shell_was_explicit` argument from `create_session_inner()` call
5. **Line ~559 (restart_session):** Remove `false, // shell_was_explicit` argument
6. **Line ~810 (spawn for mailbox/web):** Remove `false, // shell_was_explicit` argument

#### `src-tauri/src/lib.rs`

7. **Line ~526:** Remove `false, // shell_was_explicit` argument from `create_session_inner()` call

#### `src-tauri/src/phone/mailbox.rs`

8. **Line ~432:** Remove `false, // shell_was_explicit` argument
9. **Line ~515:** Remove `false, // shell_was_explicit` argument
10. **Line ~1418:** Remove `false, // shell_was_explicit` argument

#### `src-tauri/src/web/commands.rs`

11. **Line ~69:** Remove `false, // shell_was_explicit` argument

### 2.4 Also Fix: JSONL Watcher Hardcoded Path

**File:** `src-tauri/src/telegram/jsonl_watcher.rs`, line 176

**Current:**
```rust
Some(home) => home.join(".claude").join("projects").join(mangle_cwd_for_claude(&cwd)),
```

This function needs the agent's `configDir` to watch the correct JSONL directory. The watcher is started in `session.rs` when a Telegram bridge is attached.

**Approach:** The `start_jsonl_watcher` function (or its caller) needs to receive the resolved config dir path. Add a `config_dir: Option<PathBuf>` parameter to the watcher spawn call. When provided, use it instead of `~/.claude`.

**Change in jsonl_watcher.rs (line 176):**
```rust
// Before: hardcoded
Some(home) => home.join(".claude").join("projects").join(mangle_cwd_for_claude(&cwd)),

// After: use provided config_dir or fall back to ~/.claude
let project_dir = match config_dir {
    Some(dir) => dir.join("projects").join(mangle_cwd_for_claude(&cwd)),
    None => match dirs::home_dir() {
        Some(home) => home.join(".claude").join("projects").join(mangle_cwd_for_claude(&cwd)),
        None => {
            log::error!("[JSONL_ERR] Cannot resolve home directory — JSONL watcher dormant");
            cancel.cancelled().await;
            return;
        }
    },
};
```

The caller in `session.rs` that spawns the watcher must resolve and pass the `config_dir`. This follows the same `resolve_config_dir()` helper.

### 2.5 Also Fix: Agent Resolution for Project Settings

When `--continue` logic looks up the `AgentConfig`, it currently searches `settings.agents` (global). With per-project agents, it must also check project-level settings. The `agent_id` is unique across both namespaces (timestamp-based), so:

**In the `--continue` block (section 2.2 above), extend the agent lookup:**

```rust
let agent_config: Option<AgentConfig> = if let Some(ref aid) = agent_id {
    let settings_state = app.state::<SettingsState>();
    let cfg = settings_state.read().await;
    // Check global agents first
    let found = cfg.agents.iter().find(|a| a.id == *aid).cloned();
    if found.is_some() {
        found
    } else {
        // Check project-level settings if cwd is in a known project
        crate::config::project_settings::find_agent_in_project_settings(&cwd, aid)
    }
} else {
    None
};
```

Add helper in `project_settings.rs`:

```rust
/// Search project-settings.json files for an agent by ID.
/// Walks up from `cwd` looking for .ac-new/project-settings.json.
pub fn find_agent_in_project_settings(cwd: &str, agent_id: &str) -> Option<AgentConfig> {
    let mut dir = std::path::Path::new(cwd);
    loop {
        let settings_file = dir.join(".ac-new").join("project-settings.json");
        if settings_file.is_file() {
            if let Some(ps) = load_project_settings(&dir.to_string_lossy()) {
                if let Some(agent) = ps.agents.iter().find(|a| a.id == agent_id) {
                    return Some(agent.clone());
                }
            }
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent,
            _ => break,
        }
    }
    None
}
```

---

## 3. Frontend Changes

### 3.1 Known Binary Defaults Map

**File:** `src/shared/agent-presets.ts`

Add a config dir defaults map:

```typescript
/** Known config directories by binary basename */
export const KNOWN_CONFIG_DIRS: Record<string, string> = {
  claude: "~/.claude",
};

/** Check if a command's primary binary is a known agent */
export function getDefaultConfigDir(command: string): string | undefined {
  const binary = command.split(/\s+/)[0] ?? "";
  // Extract basename without extension
  const basename = binary.replace(/\\/g, "/").split("/").pop()?.replace(/\.(exe|cmd|bat)$/i, "") ?? "";
  return KNOWN_CONFIG_DIRS[basename];
}

/** Check if a command appears to be Claude-based (binary starts with "claude") */
export function isClaudeBased(command: string): boolean {
  const binary = command.split(/\s+/)[0] ?? "";
  const basename = binary.replace(/\\/g, "/").split("/").pop()?.replace(/\.(exe|cmd|bat)$/i, "") ?? "";
  return basename.startsWith("claude");
}
```

### 3.2 UI: SettingsModal Coding Agents Tab

**File:** `src/sidebar/components/SettingsModal.tsx`

After the `excludeGlobalClaudeMd` checkbox (around line 401), add a `configDir` field that appears conditionally:

```tsx
{/* Config Directory — shown for Claude-based agents */}
<Show when={isClaudeBased(agent.command)}>
  <label class="settings-check-row">
    <span class="settings-check-label">Config Directory</span>
    <input
      type="text"
      class="settings-input"
      placeholder={getDefaultConfigDir(agent.command) || "~/.claude-custom"}
      value={agent.configDir ?? getDefaultConfigDir(agent.command) ?? ""}
      onInput={(e) => updateAgent(i(), "configDir", e.currentTarget.value || undefined)}
    />
    <span class="settings-hint">
      {getDefaultConfigDir(agent.command)
        ? "Auto-detected. Override only if this binary uses a different config path."
        : "Required: this binary is not a standard Claude install. Specify its config directory."}
    </span>
  </label>
</Show>
```

**Behavior:**
- Only shown when command's basename starts with "claude" (Claude-based agents)
- For known binaries (`claude`), shows auto-detected value with hint that override is optional
- For unknown Claude binaries (`claude-phi`, `claude-dev`), shows empty field with "Required" hint
- Codex/Gemini agents don't see this field at all (not Claude-based)

### 3.3 UI: ProjectAgentsModal

**File:** `src/sidebar/components/ProjectAgentsModal.tsx`

Same change as SettingsModal — add the `configDir` field after the `excludeGlobalClaudeMd` checkbox in the agent card JSX (around line 209). Identical UI structure.

### 3.4 Validation on Save

In both SettingsModal and ProjectAgentsModal save handlers, add validation:

```typescript
// Check that Claude-based agents with unknown binary have configDir set
for (const agent of agentsToSave) {
  if (isClaudeBased(agent.command) && !getDefaultConfigDir(agent.command) && !agent.configDir) {
    setSaveError(`Agent "${agent.label}" uses a custom Claude binary — configDir is required.`);
    return;
  }
}
```

### 3.5 Update `updateAgent` Type Signature

Both `SettingsModal.tsx` and `ProjectAgentsModal.tsx` `updateAgent` functions accept `value: string | boolean`. The `configDir` field can be `string | undefined`. Ensure the type accommodates this:

```typescript
const updateAgent = (
  index: number,
  field: keyof AgentConfig,
  value: string | boolean | undefined
) => { ... };
```

---

## 4. Implementation Sequence

### Phase A: Data Model (no logic changes)

| Step | File | What |
|------|------|------|
| A1 | `src-tauri/src/config/settings.rs` | Add `config_dir: Option<String>` to `AgentConfig` |
| A2 | `src/shared/types.ts` | Add `configDir?: string` to `AgentConfig` |
| A3 | `src/shared/agent-presets.ts` | Add `configDir` to Claude preset, add `KNOWN_CONFIG_DIRS`, `getDefaultConfigDir()`, `isClaudeBased()` |
| A4 | Verify | `cargo check` + `npx tsc --noEmit` |

### Phase B: Backend Logic

| Step | File | What |
|------|------|------|
| B1 | `src-tauri/src/commands/session.rs` | Add `resolve_config_dir()` and `expand_tilde()` helper functions |
| B2 | `src-tauri/src/commands/session.rs` | Rewrite `--continue` block to use `resolve_config_dir()` |
| B3 | `src-tauri/src/config/project_settings.rs` | Add `find_agent_in_project_settings()` helper |
| B4 | Verify | `cargo check` |

### Phase C: Revert `shell_was_explicit`

| Step | File | What |
|------|------|------|
| C1 | `src-tauri/src/commands/session.rs` | Remove `shell_was_explicit` parameter from `create_session_inner` signature |
| C2 | `src-tauri/src/commands/session.rs` | Remove `let shell_was_explicit = shell.is_some();` from `create_session` |
| C3 | `src-tauri/src/commands/session.rs` | Remove `shell_was_explicit` from all `create_session_inner()` call sites in this file |
| C4 | `src-tauri/src/lib.rs` | Remove `false, // shell_was_explicit` from call site |
| C5 | `src-tauri/src/phone/mailbox.rs` | Remove `false, // shell_was_explicit` from 3 call sites |
| C6 | `src-tauri/src/web/commands.rs` | Remove `false, // shell_was_explicit` from call site |
| C7 | Verify | `cargo check` — all call sites updated |

### Phase D: Fix JSONL Watcher

| Step | File | What |
|------|------|------|
| D1 | `src-tauri/src/telegram/jsonl_watcher.rs` | Add `config_dir: Option<PathBuf>` param, use it instead of hardcoded `~/.claude` |
| D2 | `src-tauri/src/commands/session.rs` | At watcher spawn sites (~lines 414, 594, 837), resolve and pass config_dir |
| D3 | Verify | `cargo check` |

### Phase E: Frontend UI

| Step | File | What |
|------|------|------|
| E1 | `src/sidebar/components/SettingsModal.tsx` | Add configDir field to agent card (after excludeGlobalClaudeMd) |
| E2 | `src/sidebar/components/ProjectAgentsModal.tsx` | Same configDir field |
| E3 | Both modals | Add validation for required configDir on custom Claude binaries |
| E4 | Verify | `npx tsc --noEmit` + visual check |

### Phase F: Validation & Edge Cases

| Step | What |
|------|------|
| F1 | Test: standard `claude` binary → auto-detects `~/.claude`, `--continue` works |
| F2 | Test: `claude-phi` binary with explicit `configDir: "~/.claude-phi"` → checks correct dir |
| F3 | Test: `claude-phi` binary with NO `configDir` → `--continue` NOT injected (safe fallback) |
| F4 | Test: `codex` binary → `--continue` never injected (not Claude-based) |
| F5 | Test: project-level agent with custom configDir → resolution finds it |
| F6 | Test: existing settings.json without `configDir` field → loads fine (serde default = None) |
| F7 | Test: JSONL watcher uses correct directory for custom Claude binary |

---

## 5. Files Changed Summary

### New Files
None — all changes are in existing files.

### Modified Files

| File | Change |
|------|--------|
| `src-tauri/src/config/settings.rs` | Add `config_dir: Option<String>` to `AgentConfig` |
| `src-tauri/src/config/project_settings.rs` | Add `find_agent_in_project_settings()` |
| `src-tauri/src/commands/session.rs` | Add `resolve_config_dir()`, `expand_tilde()`. Rewrite --continue block. Remove `shell_was_explicit` from signature + all call sites |
| `src-tauri/src/lib.rs` | Remove `shell_was_explicit` argument |
| `src-tauri/src/phone/mailbox.rs` | Remove `shell_was_explicit` arguments (3 sites) |
| `src-tauri/src/web/commands.rs` | Remove `shell_was_explicit` argument |
| `src-tauri/src/telegram/jsonl_watcher.rs` | Accept and use `config_dir` parameter |
| `src/shared/types.ts` | Add `configDir?: string` to `AgentConfig` |
| `src/shared/agent-presets.ts` | Add `configDir` to Claude preset, add helpers |
| `src/sidebar/components/SettingsModal.tsx` | Add configDir UI field + validation |
| `src/sidebar/components/ProjectAgentsModal.tsx` | Add configDir UI field + validation |

---

## 6. Backward Compatibility

- **`config_dir` is `Option<String>` with `#[serde(default)]`**: existing `settings.json` files without this field load fine (deserialized as `None`)
- **`None` means "resolve from binary name"**: standard `claude` users get the same behavior as today with zero config changes
- **Unknown binaries get `None`→no `--continue`**: safe default — better to skip than to inject with wrong dir
- **No migration needed**: the field is purely additive

## 7. Risk Assessment

| Risk | Mitigation |
|------|-----------|
| Breaking existing --continue behavior | `resolve_config_dir` maps `claude` → `~/.claude` by default — identical behavior for standard installs |
| Reverting shell_was_explicit breaks something | The revert is safe because configDir replaces it with a more precise solution. The only case shell_was_explicit protected against was wrong-dir --continue, which configDir handles correctly |
| JSONL watcher changes break Telegram bridge | Fallback to `~/.claude` when config_dir is None preserves current behavior |
| ~ expansion edge cases on Windows | Both `~/` and `~\` handled; also handles bare `~` |
| Agent lookup misses project-level agents | `find_agent_in_project_settings` walks up the directory tree looking for `.ac-new/project-settings.json` |

---

## [DEV-RUST] Implementation Review & Enrichments

**Reviewed by:** dev-rust  
**Date:** 2026-04-11  
**Verdict:** Plan is solid. Six corrections and three additions below.

---

### [DEV-RUST] Correction 1: JSONL watcher call chain is deeper than described

Section 2.4 says "The caller in session.rs that spawns the watcher must resolve and pass the config_dir" and references lines ~414, 594, 837 as "watcher spawn sites." Those lines actually call `tg.attach()`, not the watcher directly.

**Actual call chain:**
```
session.rs tg.attach(id, &bot, pty_arc, app, jsonl_cwd)
  → telegram/manager.rs:34 TelegramBridgeManager::attach(session_id, bot, pty_mgr, app_handle, jsonl_cwd)
    → telegram/bridge.rs:413 spawn_bridge(bot_token, chat_id, session_id, info, pty_mgr, app_handle, jsonl_cwd)
      → telegram/jsonl_watcher.rs:23 spawn_watch_task(cwd, bot_token, chat_id, session_id, cancel, app)
        → telegram/jsonl_watcher.rs:167 watch_loop(cwd, token, chat_id, session_id, cancel, app)
          → line 176: HARDCODED home.join(".claude")
```

**Impact:** Adding `config_dir: Option<PathBuf>` requires changing the signature of **4 functions**, not just the watcher:
1. `TelegramBridgeManager::attach()` in `manager.rs:34`
2. `spawn_bridge()` in `bridge.rs:413`
3. `spawn_watch_task()` in `jsonl_watcher.rs:23`
4. `watch_loop()` in `jsonl_watcher.rs:167`

Plus 3 call sites in session.rs (lines 416, 596, 838) and any other callers of `tg.attach()`.

**Alternative (simpler):** Instead of threading `config_dir` through 4 functions, pre-compute the full `project_dir: Option<PathBuf>` at the session.rs level and pass THAT instead of `jsonl_cwd`. This replaces `jsonl_cwd: Option<String>` with `jsonl_project_dir: Option<PathBuf>` through the chain. The watcher then uses it directly instead of computing `home.join(".claude").join("projects").join(mangled)`. This also eliminates the `mangle_cwd_for_claude` call from inside the watcher.

---

### [DEV-RUST] Correction 2: `config_dir` moved-after-use in section 2.2 code

The proposed code in section 2.2:
```rust
let config_dir = resolve_config_dir(...);

let has_prior_conversation = config_dir
    .map(|dir| {                      // ← .map() takes ownership of config_dir
        dir.join("projects").join(&mangled).is_dir()
    })
    .unwrap_or(false);

if has_prior_conversation {
    // ...
    log::info!("... {:?}", config_dir);  // ← ERROR: config_dir already moved
}
```

**Fix:** Either clone before the map:
```rust
let config_dir_for_log = config_dir.clone();
let has_prior_conversation = config_dir.map(|dir| { ... }).unwrap_or(false);
// then use config_dir_for_log in logs
```

Or use `.as_ref()`:
```rust
let has_prior_conversation = config_dir
    .as_ref()
    .map(|dir| dir.join("projects").join(&mangled).is_dir())
    .unwrap_or(false);
// config_dir still available for logging
```

I recommend `.as_ref()` — zero allocation, idiomatic.

---

### [DEV-RUST] Correction 3: `updateAgent` type signatures differ between modals

Section 3.5 says to update the type to `string | boolean | undefined`. But the actual signatures differ:

- **SettingsModal.tsx:66** — `value: string | boolean | string[]` (already includes string[] for future fields)
- **ProjectAgentsModal.tsx:27** — `value: string | boolean` (no string[])

For `configDir`, the value is `string | undefined` (empty field → `undefined`). Both modals use `as any` casts so there's no runtime issue, but the type annotations should be updated consistently:

```typescript
// Both modals:
value: string | boolean | string[] | undefined
```

This covers all current and planned field types.

---

### [DEV-RUST] Correction 4: Line numbers are pre-`shell_was_explicit` commit

Several line numbers in the plan reference the codebase BEFORE my `shell_was_explicit` commit (2307f05). Current line numbers in `session.rs`:

| Plan reference | Actual current line |
|---|---|
| Lines 77–107 (--continue block) | Lines 77–107 (unchanged, but line 87 now has `&& !shell_was_explicit`) |
| `create_session_inner` signature line 37 | Line 36 (param at line 36) |
| `create_session` around line 367 | Line 371 (`create_session_inner` call) |
| `restart_session` line 542 | Line 548 |

These shifts are minor (1-6 lines) but worth noting for accuracy during implementation.

---

### [DEV-RUST] Correction 5: `find_agent_in_project_settings` uses `validated_settings_path` which requires `.ac-new/` to exist

The proposed `find_agent_in_project_settings()` calls `load_project_settings()` which calls `validated_settings_path()`. This function rejects paths where `.ac-new/` doesn't exist as a subdirectory.

The walk-up from a typical agent CWD like:
```
C:\Users\maria\0_repos\agentscommander\.ac-new\wg-2-dev-team\__agent_dev-rust
```

Will check:
1. `__agent_dev-rust/.ac-new/project-settings.json` — no `.ac-new` dir → load returns None
2. `wg-2-dev-team/.ac-new/project-settings.json` — no `.ac-new` dir → None  
3. `.ac-new/.ac-new/project-settings.json` — no nested `.ac-new` → None
4. `agentscommander/.ac-new/project-settings.json` — **YES** → found

This works but does 4 filesystem stat checks per session creation. Not a performance concern at this scale, but the walk-up should have a depth limit (e.g., max 10 levels) to avoid traversing all the way to filesystem root for non-AC directories.

**Suggested addition to `find_agent_in_project_settings`:**
```rust
let mut depth = 0;
const MAX_DEPTH: usize = 10;
loop {
    if depth >= MAX_DEPTH { break; }
    depth += 1;
    // ... existing walk-up logic
}
```

---

### [DEV-RUST] Correction 6: Agent preset `configDir` values

The plan shows adding `configDir: "~/.claude"` to the Claude Code preset. But `AgentConfig` has `configDir?: string` (optional). The presets use `Omit<AgentConfig, "id">` which inherits the optional nature. Since `resolve_config_dir()` already maps `claude` → `~/.claude` by default, adding it to the preset is redundant.

My recommendation: **don't add `configDir` to presets.** It's unnecessary for known binaries and would be confusing if the user changes the command but forgets to update configDir. The auto-detection from binary name is the right default. Only require explicit `configDir` for unknown Claude binaries.

---

### [DEV-RUST] Addition 1: `resolve_config_dir` should also check CLAUDE_CONFIG_DIR env var

Custom Claude binaries often use the `CLAUDE_CONFIG_DIR` env var (set in `.cmd` wrappers). Before falling back to binary-name mapping, check if `CLAUDE_CONFIG_DIR` is set in the environment:

```rust
fn resolve_config_dir(
    agent_config: Option<&AgentConfig>,
    shell: &str,
    shell_args: &[String],
) -> Option<std::path::PathBuf> {
    // 1. Explicit configDir from AgentConfig
    if let Some(cfg) = agent_config {
        if let Some(ref dir) = cfg.config_dir {
            if !dir.is_empty() {
                return Some(expand_tilde(dir));
            }
        }
    }

    // 2. Check CLAUDE_CONFIG_DIR env var
    if let Ok(env_dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !env_dir.is_empty() {
            return Some(expand_tilde(&env_dir));
        }
    }

    // 3. Fall back to known defaults by binary basename
    // ... (rest unchanged)
}
```

**Caveat:** This env var is process-wide, so it only works when AC itself is launched from a shell that sets it. For per-agent env vars, the explicit `configDir` field is the right solution. Still, checking the env var is cheap and covers some cases for free.

**Actually, on reflection: SKIP this.** The env var is process-wide — it would affect ALL agents, not just the one that should use it. If `CLAUDE_CONFIG_DIR=~/.claude-phi` is set, then standard `claude` agents would also use `~/.claude-phi`, which is wrong. The explicit `configDir` per agent is the correct granularity. Removing this suggestion.

---

### [DEV-RUST] Addition 2: Phase D call site count is understated

Phase D says "D2: At watcher spawn sites (~lines 414, 594, 837), resolve and pass config_dir." But accounting for the full call chain (Correction 1), Phase D should be:

| Step | File | What |
|------|------|------|
| D1 | `telegram/jsonl_watcher.rs` | Add `config_dir: Option<PathBuf>` to `spawn_watch_task` + `watch_loop`. Use in `watch_loop` line 176 |
| D2 | `telegram/bridge.rs` | Add `config_dir: Option<PathBuf>` to `spawn_bridge` (line 413). Pass to `spawn_watch_task` |
| D3 | `telegram/manager.rs` | Add `config_dir: Option<PathBuf>` to `attach` (line 34). Pass to `spawn_bridge` |
| D4 | `commands/session.rs` | At 3 `tg.attach()` call sites (lines ~416, 596, 838): resolve config_dir and pass it |
| D5 | Verify | `cargo check` |

---

### [DEV-RUST] Addition 3: Revert sequence matters — Phase C should come AFTER Phase B

The plan puts Phase C (revert `shell_was_explicit`) after Phase B (add configDir logic). But the revert removes the `shell_was_explicit` parameter that Phase B's rewrite replaces. If we try to `cargo check` after Phase B but before Phase C, the code will have BOTH the configDir logic AND the `shell_was_explicit` parameter — which compiles but is redundant.

**Recommended sequence:** Merge B2 and C1-C6 into a single step: "Replace the `--continue` block including `shell_was_explicit` removal." This avoids an intermediate state where both mechanisms coexist and ensures every `cargo check` sees a coherent codebase.

Concretely:
1. Phase A: Data model (unchanged)
2. Phase B: Add `resolve_config_dir()` + `expand_tilde()` + `find_agent_in_project_settings()` helpers (B1, B3)
3. **Phase B+C combined**: Rewrite `--continue` block using configDir AND simultaneously remove `shell_was_explicit` from signature + all call sites
4. Phase D: JSONL watcher (unchanged)
5. Phase E: Frontend UI (unchanged)
