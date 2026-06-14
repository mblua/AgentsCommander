# 07 Terminal Sessions

These cases validate terminal creation, PTY output, keyboard input, terminal resize/reflow, session switching, detached terminal windows, and terminal cleanup when sessions stop or close.

Use clearly disposable test projects, workgroups, agents, sessions, terminal commands, and settings state. Prefer creating test data in the tester's allowed scratch/evidence area. If no safe in-app cleanup exists, record residual state rather than deleting user data manually.

Visual preconditions from `README.md#visual-test-environment` apply to every case in this file. TRM-001 captures target-window identity explicitly; later cases inherit it.

Current deterministic mode: use `agentscommander_testeable.exe` with explicit placement and `window-info` verification. Run `agentscommander_testeable.exe test-reset --confirm-testeable` before cases that require clean disposable state, and only when the testable GUI is not active.

Use only harmless local shell/no-op terminal fixtures. Do not launch real Codex, Claude, Gemini, Telegram, paid, account-backed, or real model workflows. Commands must be read-only or write only into the disposable evidence/project area.

Required evidence categories for this suite: `window-info` JSON, before and after terminal screenshots, command input/output screenshots, resize/window-state captures, detached terminal `window-info` or title evidence, and session/status snapshots if available.

## Execution Log

Date: TBD

Tester: TBD

App under test: TBD

Target window: TBD

Evidence root: TBD

Test data: TBD

| Case | Result | Evidence | Notes |
| --- | --- | --- | --- |
| TRM-001 | NOT RUN | TBD | Terminal target readiness not executed in this run. |
| TRM-002 | NOT RUN | TBD | PTY output checks not executed in this run. |
| TRM-003 | NOT RUN | TBD | Keyboard input checks not executed in this run. |
| TRM-004 | NOT RUN | TBD | Resize/reflow checks not executed in this run. |
| TRM-005 | NOT RUN | TBD | Session switching terminal preservation not executed in this run. |
| TRM-006 | NOT RUN | TBD | Detached terminal window checks not executed in this run. |
| TRM-007 | NOT RUN | TBD | Terminal cleanup checks not executed in this run. |

Residual test data:

- TBD during execution.

Automation gaps observed:

- TBD during execution.

### TRM-001: Terminal surface is visible for an active session

Purpose:

Verify that a terminal pane or terminal window is visible, readable, and associated with the selected disposable session.

Preconditions:

- `agentscommander_testeable.exe` is launched and verified with `window-info`.
- A disposable project/workgroup and harmless local shell/no-op session are available.
- The tester has not selected a live model-backed session.

Steps:

1. Run `agentscommander_testeable.exe window-info` and save the output.
2. Capture the target app window before selecting the terminal session.
3. Select or create the harmless disposable terminal session.
4. Capture the session list and terminal pane together.
5. Record the visible session name, working directory, launcher/profile label, and active indicator.
6. Confirm the terminal text is readable at the current window size.

Expected Result:

The selected disposable session has a readable terminal surface, and the UI clearly associates that terminal with the active session row.

Evidence Required:

- `TRM-001-window-info.json`.
- `TRM-001-session-terminal.png` showing active session and terminal.
- Optional `TRM-001-session-state.json` if available.

Pass/Fail Criteria:

PASS if the terminal is readable and clearly tied to the active disposable session. PARTIAL if the terminal is readable but one identity field is missing. FAIL if the terminal appears blank, overlapped, or associated with the wrong session. BLOCKED if no safe local shell/no-op session can be opened.

### TRM-002: PTY output appears in the terminal

Purpose:

Verify that harmless command output written by the PTY appears in the active terminal.

Preconditions:

- Depends on TRM-001.
- The active terminal accepts harmless local shell commands.
- The command writes only to stdout/stderr or to the disposable evidence/project area.

Steps:

1. Capture the terminal before running the command.
2. Type a harmless command that produces deterministic output, such as `echo TRM-OUTPUT-001`.
3. Submit the command.
4. Wait for the output to appear.
5. Capture the command line and resulting output.
6. If command echo is disabled by the shell, record the typed command in the notes and capture the visible output.

Expected Result:

The terminal shows the harmless command output exactly in the active disposable session and remains responsive after output renders.

Evidence Required:

- `TRM-002-before-command.png`.
- `TRM-002-command-output.png` showing command and output marker.
- Optional terminal buffer/session snapshot if available.

Pass/Fail Criteria:

PASS if the expected output marker appears in the active terminal. PARTIAL if the output appears but the command line itself is not visible. FAIL if output appears in the wrong session, is missing, or renders unreadably. BLOCKED if the safe shell fixture does not accept harmless commands.

### TRM-003: Keyboard input reaches the active PTY

Purpose:

Verify that keyboard input is delivered to the selected terminal and not to an inactive session or unrelated UI element.

Preconditions:

- Depends on TRM-001.
- The active terminal has keyboard focus or can be focused safely.
- A harmless local command or input marker is available.

Steps:

1. Capture the active session row and terminal before input.
2. Click or keyboard-focus the terminal area for the selected disposable session.
3. Type a harmless marker command such as `echo TRM-INPUT-001`.
4. Submit the input.
5. Capture the rendered input and output.
6. Confirm no other session row became active unexpectedly during the input.

Expected Result:

Keyboard input reaches the selected PTY, produces the expected harmless output, and does not alter inactive sessions.

Evidence Required:

- `TRM-003-before-input.png`.
- `TRM-003-input-output.png` showing typed marker and output.
- `TRM-003-active-session.png` if separate capture is needed to show the active row.

Pass/Fail Criteria:

PASS if input and output appear in the active terminal only. PARTIAL if output appears but focus evidence is incomplete. FAIL if input is lost, sent to the wrong session, or triggers unrelated UI controls. BLOCKED if terminal focus cannot be established safely.

### TRM-004: Terminal resize preserves readability and reflows output

Purpose:

Verify that resizing the app or terminal window keeps terminal output readable and coherent.

Preconditions:

- Depends on TRM-002.
- The terminal contains a recognizable output marker or multiline harmless output.
- The tester can resize only the testable app or detached disposable window.

Steps:

1. Capture the terminal at the baseline deterministic size.
2. Produce or display multiline harmless output, such as several `echo TRM-LINE-N` lines.
3. Resize the testable app or terminal area to a smaller but usable size.
4. Capture the resized terminal and visible output.
5. Resize back to the baseline or another documented size.
6. Capture the restored terminal and compare readability, wrapping, and cursor position.

Expected Result:

Terminal output remains readable after resize, visible content reflows or clips predictably, and no major overlap or blank terminal condition appears.

Evidence Required:

- `TRM-004-before-resize.png`.
- `TRM-004-after-small-resize.png`.
- `TRM-004-after-restore.png`.
- Optional window rectangles or `window-info` output before and after resize.

Pass/Fail Criteria:

PASS if output remains readable and resize behavior is coherent. PARTIAL if output is readable but one restore/capture step is incomplete. FAIL if terminal content becomes blank, overlaps controls, or loses association with the session. BLOCKED if resizing cannot be performed safely on the target window.

### TRM-005: Switching sessions preserves terminal buffers

Purpose:

Verify that terminal buffers remain associated with their sessions after switching between disposable sessions.

Preconditions:

- Depends on TRM-002.
- Two harmless disposable terminal sessions exist.
- Each session can display a distinct marker.

Steps:

1. In the first session, run or display `echo TRM-FIRST-BUFFER`.
2. Capture the first session's terminal buffer and active row.
3. Switch to the second disposable session.
4. Run or display `echo TRM-SECOND-BUFFER`.
5. Capture the second session's terminal buffer and active row.
6. Switch back to the first session and capture its preserved buffer.
7. Switch back to the second session and capture its preserved buffer.

Expected Result:

Each terminal buffer retains its own marker, and switching sessions does not mix output or reset visible content unexpectedly.

Evidence Required:

- `TRM-005-first-buffer.png`.
- `TRM-005-second-buffer.png`.
- `TRM-005-first-buffer-return.png`.
- `TRM-005-second-buffer-return.png`.

Pass/Fail Criteria:

PASS if each buffer is preserved and tied to the correct session. PARTIAL if markers remain correct but one return capture is missing. FAIL if buffers mix, disappear unexpectedly, or active indicators are wrong. BLOCKED if a second safe disposable terminal session cannot be created.

### TRM-006: Detached terminal window opens and remains associated with the session

Purpose:

Verify that a disposable session can be detached into its own terminal window and that the detached window remains identifiable.

Preconditions:

- Depends on TRM-001.
- A disposable session is selected.
- The detach affordance is visible or reachable by context menu/keyboard.
- The tester can capture multiple windows on the visual test monitor or with README-approved fallback evidence.

Steps:

1. Capture the selected disposable session before detaching.
2. Activate the detach affordance for the selected session.
3. Enumerate or capture visible AgentsCommander windows after detach.
4. Capture the detached terminal window title, rectangle, session content, and any visible session identity.
5. Capture the main app state showing the session's detached indicator if one exists.
6. If available, run `window-info` or use HWND capture for the detached window and save the state.

Expected Result:

A detached terminal window opens for the selected disposable session, remains identifiable by title/content/session marker, and the main app reflects detached state coherently.

Evidence Required:

- `TRM-006-before-detach.png`.
- `TRM-006-detached-window.png`.
- `TRM-006-main-after-detach.png`.
- Optional detached-window state JSON or HWND/rectangle notes.

Pass/Fail Criteria:

PASS if the detached window opens and clearly matches the selected session. PARTIAL if detached state is correct but a secondary window-info artifact is unavailable. FAIL if the wrong session detaches, the detached window is blank, or the main app state contradicts the detached window. BLOCKED if the detach affordance is unavailable for the safe fixture.

### TRM-007: Closing or stopping a session cleans up terminal state predictably

Purpose:

Verify that terminal input/output availability changes predictably when a disposable session is stopped, closed, or exits.

Preconditions:

- Depends on TRM-001.
- A disposable session is active and can be stopped or closed safely.
- If the session is detached, the tester can capture both main and detached windows.

Steps:

1. Capture the terminal and session row before stop/close.
2. Use the GUI stop/close affordance for the disposable session.
3. Capture any confirmation prompt.
4. Confirm the action.
5. Capture the terminal area and session list after the action.
6. Attempt only a harmless focus or input check if the UI indicates input might still be enabled.
7. Record whether terminal input is disabled, the session row is removed, or the state changes to exited/stopped.

Expected Result:

The terminal state after stop/close is clear: input is disabled, content remains as historical output, or the session is removed according to documented behavior.

Evidence Required:

- `TRM-007-before-close.png`.
- `TRM-007-close-action.png`.
- `TRM-007-after-close.png`.
- Optional session/status JSON snapshot after cleanup.

Pass/Fail Criteria:

PASS if terminal cleanup behavior is predictable and limited to the disposable session. PARTIAL if cleanup is correct but one confirmation capture is unavailable. FAIL if terminal remains interactively attached to a stopped process, wrong session closes, or UI state is contradictory. BLOCKED if no safe stop/close path exists for the fixture.
