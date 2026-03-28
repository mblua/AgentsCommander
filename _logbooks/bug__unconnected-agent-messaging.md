# Bug Investigation: Messaging to Unconnected/Non-Existent Agents

## Problem Statement

**What is expected:** When sending a message to an agent that is not connected or instantiated, the system should either (a) give clear feedback to the sender, (b) queue gracefully with a TTL, or (c) reject explicitly.

**What is observed:** Messages to unresolvable agents get stuck in `outbox/` with infinite retry every 3 seconds. No feedback to sender. No TTL. No explicit rejection.

**Impact:** Silent infinite retry loop. Outbox accumulates dead messages. Sender thinks message was delivered (CLI returns exit 0 + "Message queued").

---

## Investigation Log

### 2026-03-27 — Static Code Analysis

**Findings:**

1. **CLI send** (`cli/send.rs`): Writes message to `outbox/<uuid>.json`, returns success immediately. Zero destination validation.

2. **Mailbox poller** (`phone/mailbox.rs`): Polls every 3s. On delivery attempt:
   - `resolve_repo_path()` tries: active sessions → settings repo_paths → dark factory team paths
   - If all fail → returns `None` → error `"Could not resolve inbox for agent"`
   - Error bubbles up to poll loop → logged as `warn!` → message stays in outbox → retry next cycle

3. **Rejection path** only triggers for: token/spoofing failures, team membership failures. NOT for unresolvable destinations.

4. **No TTL or max-retry logic** exists anywhere in the delivery pipeline.

### 2026-03-27 — Live Testing (DEV instance v0.4.15)

**Master token:** `06e1608d-c52e-42b3-9e3c-b310150a5a47`

**Test environment:**
- DEV instance with 2 persisted sessions (AGENT1 → `3185d5ac`, AGENT2 → `1c777cf9`)
- Team `TestAgents` has AGENT1 and AGENT2 as members
- AGENT3 exists on disk but is NOT in any team and NOT a session

#### Test 1-3: Send from agentscommander_3 to AGENT1/AGENT3/FAKE_AGENT
- **Result:** All REJECTED with "Sender cannot reach destination"
- **Reason:** `agentscommander_3` is not in any team → `can_reach()` fails
- **Note:** Messages correctly moved to `outbox/rejected/` with `.reason.txt`

#### Test 4: Send from AGENT1 to AGENT2 (both in team, both have sessions)
- **Result:** DELIVERED successfully
- **Behavior:** Master token bypassed team validation → found active session `3185d5ac` → injected into PTY
- **Log:** `"Injected message into session 3185d5ac PTY"`

#### Test 5: Send from AGENT1 to AGENT3 (AGENT3 has no session, exists on disk)
- **Result:** STUCK — infinite retry loop
- **Log sequence (repeats every 3s):**
  ```
  [mailbox] Master token used — bypassing team validation
  [mailbox] No session matched for '_test_dark_factory/AGENT3'
  [mailbox] Delivering via QUEUE to _test_dark_factory/AGENT3
  Failed to process outbox message: Could not resolve inbox for agent '_test_dark_factory/AGENT3'
  ```
- **Message remains in outbox root** (`1a47becc-ed7c-4ef5-87e7-fbc0b326393c.json`)

#### Test 6: Send from AGENT1 to COMPLETELY_FAKE_AGENT (no session, no directory)
- **Result:** STUCK — identical infinite retry loop
- **Log sequence (repeats every 3s):**
  ```
  [mailbox] Master token used — bypassing team validation
  [mailbox] No session matched for 'COMPLETELY_FAKE_AGENT'
  [mailbox] Delivering via QUEUE to COMPLETELY_FAKE_AGENT
  Failed to process outbox message: Could not resolve inbox for agent 'COMPLETELY_FAKE_AGENT'
  ```
- **Message remains in outbox root** (`4094e3a6-b980-4c42-aece-57a7baec5d92.json`)

### Summary of Confirmed Bugs

| # | Bug | Severity |
|---|---|---|
| 1 | **Infinite retry on unresolvable destination** — messages stuck in outbox, retried every 3s forever | High |
| 2 | **No sender feedback** — CLI returns exit 0 even for permanently undeliverable messages | Medium |
| 3 | **No TTL/max-retry** — no mechanism to expire or give up on dead messages | Medium |
| 4 | **No explicit rejection for unresolvable agents** — only team/token failures get moved to rejected/ | Medium |
| 5 | **Log spam** — warn! logged every 3s per stuck message, accumulates indefinitely | Low |

### Cleanup Needed
- Remove stuck test messages from AGENT1 outbox:
  - `1a47becc-ed7c-4ef5-87e7-fbc0b326393c.json`
  - `4094e3a6-b980-4c42-aece-57a7baec5d92.json`

### Proposed Fix Approach (TBD)
- Add max retry count or TTL to messages
- Move permanently undeliverable messages to `rejected/` with reason
- Consider returning non-zero exit from CLI when destination is unknown
- Implement exponential backoff for retries
