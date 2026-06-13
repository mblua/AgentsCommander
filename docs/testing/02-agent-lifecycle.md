# 02 Agent Lifecycle

These cases validate creating agent sessions from the GUI, identifying their lifecycle state, switching between sessions, stopping sessions, and confirming that session records behave predictably across restart or reset boundaries.

Use clearly disposable test projects, workgroups, agents, sessions, and settings state. Prefer creating test data in the tester's allowed scratch/evidence area. If no safe in-app cleanup exists, record residual state rather than deleting user data manually.

Visual preconditions from `README.md#visual-test-environment` apply to every case in this file. AGT-001 captures target-window identity explicitly; later cases inherit it.

Current deterministic mode: use `agentscommander_testeable.exe` with explicit placement and `window-info` verification. Run `agentscommander_testeable.exe test-reset --confirm-testeable` before cases that require clean disposable state, and only when the testable GUI is not active.

Use only a disposable testable project/workgroup and a harmless local shell or no-op agent fixture. Do not launch real Codex, Claude, Gemini, Agency, Telegram, paid, account-backed, or real model workflows. If no harmless safe fixture is available, mark create/start cases `BLOCKED`.

Required evidence categories for this suite: `window-info` JSON, before and after screenshots of the session list and terminal area, session/status JSON snapshots if available, and notes identifying the disposable project/workgroup root.

## Execution Log

Date: TBD

Tester: TBD

App under test: TBD

Target window: TBD

Evidence root: TBD

Test data: TBD

| Case | Result | Evidence | Notes |
| --- | --- | --- | --- |
| AGT-001 | NOT RUN | TBD | Target app identity and clean project/workgroup context not executed in this run. |
| AGT-002 | NOT RUN | TBD | Agent create flow not executed in this run. |
| AGT-003 | NOT RUN | TBD | Agent status transition checks not executed in this run. |
| AGT-004 | NOT RUN | TBD | Session switching checks not executed in this run. |
| AGT-005 | NOT RUN | TBD | Stop/close checks not executed in this run. |
| AGT-006 | NOT RUN | TBD | Restart persistence checks not executed in this run. |

Residual test data:

- TBD during execution.

Automation gaps observed:

- TBD during execution.

### AGT-001: App target and disposable project context are ready

Purpose:

Verify that the tester is operating against the deterministic testable app and a disposable project/workgroup context before any agent lifecycle action mutates state.

Preconditions:

- `agentscommander_testeable.exe` is available from the build under test.
- The testable GUI is closed if a reset is needed.
- The tester has an evidence root and a disposable project path such as `ac-regression-agt-YYYYMMDD-HHMMSS`.
- No live user project or live workgroup is selected for this suite.

Steps:

1. Run `agentscommander_testeable.exe test-reset --confirm-testeable` if a clean identity is required, and save stdout/stderr as reset evidence.
2. Launch `agentscommander_testeable.exe --app` with explicit placement flags or `AC_TEST_WINDOW_PLACEMENT`.
3. Run `agentscommander_testeable.exe window-info` from a separate process.
4. Confirm the reported `processPath` exactly matches the launched `agentscommander_testeable.exe`.
5. Capture the target window and record HWND, PID, title, rectangle, maximized state, and capture method.
6. Create or open the disposable project context that later cases will use, then capture the visible project/workgroup area.

Expected Result:

The target window is `Agents Commander [TESTEABLE]`, the executable identity is verified by `window-info`, and a disposable project/workgroup context is ready for safe agent lifecycle checks.

Evidence Required:

- `AGT-001-window-info.json` containing process path, PID, HWND, rectangle, and maximized state.
- `AGT-001-target-window.png` showing the testable app.
- `AGT-001-disposable-context.png` showing the selected disposable project/workgroup.
- Optional `AGT-001-reset.log` if reset was used.

Pass/Fail Criteria:

PASS if the testable app identity and disposable context are both verified with evidence. PARTIAL if the app is verified but the disposable context needed fallback setup. FAIL if the selected window is not the testable app or a live project is used. BLOCKED if the testable binary, safe evidence root, or disposable context cannot be prepared.

### AGT-002: Create an agent session from the GUI

Purpose:

Verify that the GUI can create or start a disposable agent session without invoking real model-backed providers.

Preconditions:

- Depends on AGT-001.
- A harmless local shell or no-op agent fixture is available in the disposable project/workgroup.
- The selected launcher or session profile is confirmed not to start Codex, Claude, Gemini, Telegram, Agency, paid, account-backed, or real model workflows.

Steps:

1. Capture the baseline session list and terminal area before creating the agent session.
2. Open the GUI action that creates or starts an agent/session for the disposable context.
3. Select the harmless local shell or no-op fixture.
4. Confirm the action, then wait for a new session row or terminal pane associated with the disposable agent.
5. Capture the created session row, displayed label, launcher/profile choice, and terminal area.
6. Record any visible session id, name, working directory, or status text.

Expected Result:

A new disposable agent/session is visible in the session list and associated terminal area, and the UI makes its identity distinct from any existing sessions.

Evidence Required:

- `AGT-002-before-create.png` showing the pre-create state.
- `AGT-002-create-surface.png` showing the selected harmless fixture.
- `AGT-002-created-session.png` showing the resulting session row and terminal area.
- Optional session/status JSON snapshot if available from existing app tooling.

Pass/Fail Criteria:

PASS if a harmless disposable session is created and visibly associated with the selected fixture. PARTIAL if creation succeeds but one transient modal cannot be captured. FAIL if the wrong provider starts, the session appears under a live project, or the session identity is ambiguous. BLOCKED if no safe local/no-op fixture exists.

### AGT-003: Agent lifecycle status transitions are visible

Purpose:

Verify that lifecycle state changes are visible to a tester while a disposable agent/session starts, runs, waits, idles, or exits.

Preconditions:

- Depends on AGT-002.
- The disposable session can execute a harmless local command or no-op workflow whose expected end state is known.
- The terminal/status area is readable in the target capture.

Steps:

1. Capture the session row before triggering the status transition.
2. Trigger a harmless local command or no-op action that causes the session to move through a running or waiting state.
3. Capture the visible running, waiting, idle, or exited indicator when it appears.
4. Wait for the expected steady state for the fixture.
5. Capture the final status indicator and terminal/output area.
6. Record whether status text, icon color, pending-review state, or equivalent visual state communicated the transition.

Expected Result:

The session visibly moves through the expected lifecycle states for the harmless fixture, and the final state matches the command or no-op behavior.

Evidence Required:

- `AGT-003-status-before.png` showing baseline state.
- `AGT-003-status-transition.png` showing the intermediate state when possible.
- `AGT-003-status-final.png` showing the final state.
- Optional terminal output or session/status JSON snapshot proving the transition.

Pass/Fail Criteria:

PASS if the transition and final state are observable and match the fixture. PARTIAL if the final state is correct but the intermediate state is too brief to capture. FAIL if the UI reports the wrong state or never updates. BLOCKED if the safe fixture cannot produce an observable status transition.

### AGT-004: Switching between agent sessions preserves session identity

Purpose:

Verify that switching between two disposable sessions keeps active-session identity, terminal content, and status indicators coherent.

Preconditions:

- Depends on AGT-002.
- Two disposable harmless sessions exist, or one existing disposable session plus a second safe fixture can be created.
- Each session can display distinct harmless terminal content or labels.

Steps:

1. Capture both visible session rows with one session selected.
2. Enter or display a harmless distinct marker in the first session, such as `echo AGT-FIRST`.
3. Switch to the second disposable session and capture the active indicator.
4. Enter or display a different harmless marker in the second session, such as `echo AGT-SECOND`.
5. Switch back to the first session and capture the terminal buffer and active indicator.
6. Switch again to the second session and capture its terminal buffer and active indicator.

Expected Result:

The active indicator follows the selected session, and each session retains its own distinct terminal content and identity.

Evidence Required:

- `AGT-004-first-active.png` showing the first selected session and marker.
- `AGT-004-second-active.png` showing the second selected session and marker.
- `AGT-004-return-first.png` showing the first session content after switching back.
- `AGT-004-return-second.png` showing the second session content after switching again.

Pass/Fail Criteria:

PASS if selection, labels, and terminal content remain tied to the correct sessions. PARTIAL if identity is correct but one marker cannot be visually captured. FAIL if content crosses sessions, active indicators are wrong, or switching selects an unexpected session. BLOCKED if a second safe disposable session cannot be created.

### AGT-005: Stop or close an agent session from the GUI

Purpose:

Verify that a disposable session can be stopped or closed from the GUI and that the resulting state is understandable.

Preconditions:

- Depends on AGT-002.
- A disposable session is active or listed.
- The stop, close, or context-menu affordance is visible or reachable by keyboard.

Steps:

1. Capture the disposable session before opening the stop/close affordance.
2. Open the GUI stop, close, or context-menu action for that session.
3. Capture the confirmation surface if one appears.
4. Confirm the stop/close action only for the disposable session.
5. Capture the resulting session list and terminal area.
6. Record whether the session disappeared, remains listed as stopped/exited, or disables input.

Expected Result:

The GUI applies the stop/close action only to the selected disposable session and presents a predictable stopped, exited, removed, or disabled-input state.

Evidence Required:

- `AGT-005-before-stop.png` showing the target disposable session.
- `AGT-005-stop-action.png` showing the selected stop/close affordance or confirmation.
- `AGT-005-after-stop.png` showing the resulting session state.
- Optional session/status JSON snapshot after the action.

Pass/Fail Criteria:

PASS if the selected disposable session stops or closes with clear UI state. PARTIAL if the action succeeds but the confirmation surface cannot be captured. FAIL if the wrong session changes, the UI hangs, or input remains enabled for an exited session unexpectedly. BLOCKED if no safe stop/close affordance is available for the disposable fixture.

### AGT-006: Agent session records after app restart are predictable

Purpose:

Verify that disposable session records and selected-session state behave predictably across a normal testable app restart.

Preconditions:

- Depends on AGT-002 and AGT-005 if stopped-session persistence is being checked.
- The current suite has at least one disposable session with known running, idle, stopped, or exited state.
- The tester can close and relaunch only the testable app without terminating unrelated AgentsCommander windows.

Steps:

1. Capture the pre-restart session list, active session, and terminal/status area.
2. Save any available session/status JSON snapshot from existing app tooling.
3. Close the `Agents Commander [TESTEABLE]` window normally.
4. Relaunch `agentscommander_testeable.exe --app` with the same deterministic placement.
5. Run `agentscommander_testeable.exe window-info` and capture the relaunched target window.
6. Capture the post-restart session list and active/selected state.
7. Compare visible session records, statuses, and selection against the pre-restart evidence.

Expected Result:

After restart, disposable session records either restore or stay absent according to documented app behavior, and the UI does not invent duplicate or ambiguous session rows.

Evidence Required:

- `AGT-006-pre-restart.png` and optional `AGT-006-pre-restart-sessions.json`.
- `AGT-006-post-window-info.json`.
- `AGT-006-post-restart.png` and optional `AGT-006-post-restart-sessions.json`.
- Notes identifying any expected non-restored stopped/exited state.

Pass/Fail Criteria:

PASS if the post-restart state matches documented behavior and no duplicates appear. PARTIAL if state is coherent but one optional JSON snapshot is unavailable. FAIL if sessions duplicate, attach to the wrong project, or display contradictory statuses. BLOCKED if restart cannot be performed without affecting live user windows.
