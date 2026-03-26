# PLAN: Direct Messaging + --root Flag

## Status: IN PROGRESS (branch: fix/mailbox-delivery-to-shipper)

## What's Already Done (committed + pushed)

### 1. `--root` param for send/list-peers CLI
**Files changed:** `src-tauri/src/cli/send.rs`, `src-tauri/src/cli/list_peers.rs`

- `--root <path>` explicitly sets the agent's root directory
- Derives sender name (`parent/folder`) from `--root` instead of CWD
- Derives outbox path as `<root>/.agentscommander/outbox/`
- Eliminates fragile CWD walk-up for agent-initiated sends
- Without `--root`, falls back to walk-up (manual human usage), but `execute()` prints error asking for `--root`

### 2. Init prompt uses `--root`
**File changed:** `src-tauri/src/commands/session.rs`

- Command templates in the init prompt now include `--root "{root}"` baked in
- No `cd` prefix needed — the command is CWD-agnostic

### 3. Anti-spoofing validation in mailbox
**File changed:** `src-tauri/src/phone/mailbox.rs`

- When a message has a token, the mailbox validates that `msg.from` matches the `working_directory` of the session that owns the token
- Mismatch → rejected with "Token-root mismatch" reason
- Added `agent_name_from_path()` helper to MailboxPoller

### 4. teams.json fixed (PROD config, not in repo)
**File:** `~/.agentscommander/teams.json`

- All member names updated to `parent/folder` format (e.g., `"agentscommander_2"` → `"0_repos/agentscommander_2"`)
- This was a data fix, not a code fix

---

## What Needs To Be Done

### 5. `--direct` flag for team-bypass messaging
**Goal:** Allow sending a message to ANY session by name, without team membership validation.

**Why:** Currently, agents not in the same team can't communicate. The `--direct` flag tells the mailbox to skip `can_reach()` and deliver by matching the session's working directory directly.

**Files to change:**

#### `src-tauri/src/cli/send.rs`
- Add `--direct` boolean flag to `SendArgs`
- Add `direct: bool` field to `OutboxMessage` (with `#[serde(default)]`)
- Pass it through to the outbox JSON

#### `src-tauri/src/phone/mailbox.rs`
- Add `direct: bool` field to the mailbox's `OutboxMessage` struct (with `#[serde(default)]`)
- In `process_message()`, after token validation but BEFORE `can_reach()`:
  ```
  if msg.direct {
      // Find session by matching msg.to against session working directories
      // Use existing find_active_session() which already does name→session matching
      // Deliver using the standard mode logic (wake/queue/active-only)
      // Skip can_reach() entirely
  }
  ```
- The delivery logic for direct messages should reuse the existing mode-based delivery (`deliver_wake`, `deliver_queue`, etc.) — not a separate path

#### `src-tauri/src/commands/session.rs`
- Update init prompt to include `--direct` in the send command template
- The init prompt should always be injected (not gated on `has_teams`), since `--direct` works without teams
- Template: `"<bin>" send --token {token} --root "{root}" --to "<agent_name>" --message "..." --mode wake --direct`

---

## How to Test in Dev

### Prerequisites
- Branch: `fix/mailbox-delivery-to-shipper`
- Dev app running: `npm run tauri dev`
- Two test agents visible in sidebar: `_test_dark_factory/AGENT1` and `_test_dark_factory/AGENT2`

### Test 1: Send direct message from CLI to AGENT1
```bash
# Use the dev binary with --direct to bypass teams
"./src-tauri/target/debug/agentscommander.exe" send \
  --root "C:/Users/maria/0_repos/agentscommander_2" \
  --to "_test_dark_factory/AGENT1" \
  --message "Qué hora es?" \
  --mode wake \
  --direct
```
- Verify: message appears in AGENT1's terminal
- Verify: NOT rejected (check outbox/rejected/ is empty for this message)

### Test 2: AGENT1 sends to AGENT2
- AGENT1 should have received its token and root in the init prompt
- Ask AGENT1 to send a message to AGENT2 using the command from its init prompt with `--direct`:
```bash
"<bin>" send --token <AGENT1_TOKEN> --root "<AGENT1_ROOT>" \
  --to "_test_dark_factory/AGENT2" \
  --message "Hola AGENT2, te escribo desde AGENT1" \
  --mode wake --direct
```
- Verify: message appears in AGENT2's terminal

### Test 3: Anti-spoofing still works
```bash
# Try to send with AGENT1's token but claiming to be from a different root
"./src-tauri/target/debug/agentscommander.exe" send \
  --token <AGENT1_TOKEN> \
  --root "C:/Users/maria/0_repos/some_other_repo" \
  --to "_test_dark_factory/AGENT2" \
  --message "spoofed" \
  --mode wake \
  --direct
```
- Verify: rejected with "Token-root mismatch"

### Test 4: Team-based messaging still works
```bash
# From PROD binary to a PROD team member (no --direct)
"/c/Users/maria/AppData/Local/Agents Commander/agentscommander.exe" send \
  --root "C:/Users/maria/0_repos/agentscommander_2" \
  --to "Agents/Shipper" \
  --message "test" \
  --mode wake
```
- Verify: delivered via normal team routing

---

## Architecture Notes

- `--direct` and `--to` (team-based) are not mutually exclusive — `--direct` just skips the team check
- `--root` is always required for CLI sends (no more walk-up for agent-initiated sends)
- Token validation + anti-spoofing runs regardless of `--direct`
- The mailbox uses `find_active_session()` which matches by working directory suffix — same logic for both direct and team-based delivery
