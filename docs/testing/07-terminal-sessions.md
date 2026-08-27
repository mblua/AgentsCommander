# 07 Terminal Sessions

These cases validate terminal creation, PTY output, keyboard input, terminal resize/reflow, session switching, detached terminal windows, terminal cleanup, and authorized JSON/PNG backend viewport snapshots that do not mutate the target.

Use clearly disposable test projects, workgroups, agents, sessions, terminal commands, and settings state. Prefer creating test data in the tester's allowed scratch/evidence area. If no safe in-app cleanup exists, record residual state rather than deleting user data manually.

Visual preconditions from `README.md#visual-test-environment` apply to every case in this file. TRM-001 captures target-window identity explicitly; later cases inherit it.

Current deterministic mode: use `agentscommander_testeable.exe` with explicit placement and `window-info` verification. Run `agentscommander_testeable.exe test-reset --confirm-testeable` before cases that require clean disposable state, and only when the testable GUI is not active.

Use only harmless local shell/no-op terminal fixtures. Do not launch real Codex, Claude, Gemini, Telegram, paid, account-backed, or real model workflows. Commands must be read-only or write only into the disposable evidence/project area.

Required evidence categories for this suite: `window-info` JSON, before and after terminal screenshots, command input/output screenshots, resize/window-state captures, detached terminal `window-info` or title evidence, session/status snapshots if available, raw snapshot JSON, PNG receipts and files, decoded PNG metadata and hashes, command stdout/stderr/exit logs, and non-mutation state comparisons.

## Execution Log

Date: TBD

Tester: TBD

App under test: TBD

Target window: TBD

Evidence root: TBD

Test data: TBD

| Case | Result | Evidence | Notes |
| --- | --- | --- | --- |
| TRM-001 | NOT RUN | No evidence because NOT RUN. | Terminal target readiness not executed in this run. |
| TRM-002 | NOT RUN | No evidence because NOT RUN. | PTY output checks not executed in this run. |
| TRM-003 | NOT RUN | No evidence because NOT RUN. | Keyboard input checks not executed in this run. |
| TRM-004 | NOT RUN | No evidence because NOT RUN. | Resize/reflow checks not executed in this run. |
| TRM-005 | NOT RUN | No evidence because NOT RUN. | Session switching terminal preservation not executed in this run. |
| TRM-006 | NOT RUN | No evidence because NOT RUN. | Detached terminal window checks not executed in this run. |
| TRM-007 | NOT RUN | No evidence because NOT RUN. | Terminal cleanup checks not executed in this run. |
| TRM-008 | NOT RUN | No evidence because NOT RUN. | Authorized JSON terminal snapshot checks not executed in this run. |
| TRM-009 | NOT RUN | No evidence because NOT RUN. | Deterministic PNG terminal snapshot checks not executed in this run. |
| TRM-010 | NOT RUN | No evidence because NOT RUN. | Snapshot non-mutation and hidden-frontend checks not executed in this run. |

Residual test data:

- None recorded; suite not executed yet.

Automation gaps observed:

- No automation gaps recorded; suite not executed yet.

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

### TRM-008: Authorized JSON snapshot represents the backend viewport

Purpose:

Verify that an authorized host requester receives one complete version-1 JSON model of the live target's current backend viewport, including fidelity metadata, without ANSI or non-ASCII control injection on stdout.

Preconditions:

- A disposable local-process or protocol-fake container target has one eligible persistent live session.
- The requester is either a verified same-workgroup Orchestrator or canonical host Root with a live session UUID-v4 token.
- `terminalSnapshotsEnabled` is explicitly enabled for this disposable run.
- The target viewport contains only harmless deterministic markers and no account-backed or personal content.
- The evidence directory is private and approved for terminal content.

Steps:

1. Write a deterministic harmless marker such as `TRM-SNAPSHOT-JSON-001` to the target and let the prompt become stable.
2. Record the target session ID, backend, status, dimensions, and visible frontend state if one exists.
3. Run `list-peers-lean --snapshot-targets` from the requester and save stdout, stderr, and exit code.
4. Confirm the returned exact target FQN without using runtime fields as a liveness claim.
5. Run `agentscommander terminal-snapshot --token <live-token> --root <exact-root> --to <exact-fqn> --format json --timeout 15` through a direct noninteractive process invocation. Save stdout as bytes before parsing.
6. Assert exit 0, empty stderr, exactly one LF-terminated ASCII-only JSON document, and no update or logger notice.
7. Parse the document and assert `schemaVersion == 1`, canonical request/session IDs and millisecond UTC capture time, exact requester and target, expected backend, nonzero dimensions, `lines.length == rows`, and every `cells.length == columns`.
8. Assert color, width, wide-pair, cursor, sequence, parser error, wrap, and style values are structurally valid. Confirm blank cells are present instead of truncated.
9. Assert the full `fidelity` object equals the documented version-1 constants, including `scope=currentBackendViewport`, `backendParser=vt100-0.15.2`, zero backend scrollback, `applicationFrameAtomic=false`, and the exact ordered `omitted` and `unsupported` arrays.
10. Confirm the harmless marker is represented when it remains inside the current viewport. Do not fail a coherent capture merely because concurrent output moved the marker before the parser-lock boundary.

Expected Result:

The command returns one closed, complete, bounded JSON model of one active backend viewport at its reported sequence. It contains no raw ANSI replay, frontend state, transcript, or omitted blank-cell compression.

Evidence Required:

- `TRM-008-target-before.png` or equivalent safe target-state evidence.
- `TRM-008-snapshot-targets.stdout.json`, stderr, command, and exit record.
- `TRM-008-terminal-snapshot.stdout.json`, raw-byte hash, stderr, command, and exit record.
- `TRM-008-schema-validation.json` with each asserted count and constant.
- The exact app version, commit, platform, requester kind, target backend, and setting state.

Pass/Fail Criteria:

PASS if every structural, identity, fidelity, ASCII, and output assertion succeeds. FAIL if fields are missing or extra, counts mismatch, raw controls leak, unauthorized data appears, or the model contradicts one capture boundary. BLOCKED if no safe authorized disposable route can be provisioned. Do not mark PARTIAL for a schema or privacy assertion.

### TRM-009: PNG snapshot follows the fixed renderer contract

Purpose:

Verify that PNG output is validated before create-new persistence, that stdout contains metadata only, and that checked-in renderer goldens retain their fixed portable classification.

Preconditions:

- Depends on TRM-008 authorization and safe target data.
- The chosen absolute output path has an existing non-linked parent and the leaf does not exist.
- The platform can decode RGB8 PNG without rewriting it.
- The repository's renderer fixtures and hashes are unchanged.

Steps:

1. Record that the output leaf does not exist.
2. Run `agentscommander terminal-snapshot` with `--format png --output <absolute-new-file.png> --timeout 15`. Save stdout, stderr, and exit code separately.
3. Assert exit 0, empty stderr, one ASCII JSON metadata line on stdout, and no PNG signature or base64 payload on stdout.
4. Assert the output now exists as one regular non-linked file. On Unix assert mode `0600`.
5. Parse the receipt. Assert `schemaVersion == 1`, `format == png`, exact request/requester/target/session metadata, the full fidelity constants, `renderer.id == ac-terminal-png-v1`, palette `ac-dark-v1`, fixed DejaVu font metadata, cell 10 by 20, baseline 15, padding 8, and a nonnegative `fallbackGlyphCount`.
6. Assert file length equals `png.bytes`, width equals `columns * 10 + 16`, and height equals `rows * 20 + 16`.
7. Scan the PNG bytes. Require RGB8, noninterlaced IHDR; one IHDR first; one or more consecutive IDAT chunks; one zero-length IEND last; correct CRCs; no ancillary or trailing chunks; and successful full decoder finish.
8. Decode and inspect the image without resaving it. Classify cursor, backgrounds, wide spans, clipping, underline, and any fallback according to the visual classes below.
9. Run `cargo test --locked -p terminal-snapshot-renderer`. Record the checked-in hashes `97bac516626c41f8253afd6958607943274a58785b7afd5ec2bb158707dbe06b` for `blank-cursor.png` and `756915b2b24f0f092dbc0e171b9867c18f0756eb1fbfb8a43dc57103e83cfc05` for `style-grid.png`.
10. Attempt the same existing output path again. Assert failure and no overwrite. Separately test a new path for any retry.

Expected Result:

The client validates one deterministic bounded PNG before creating the caller-owned output, prints only metadata, and preserves byte-exact renderer goldens across supported portable runners.

Evidence Required:

- `TRM-009-command.txt`, stdout receipt, empty stderr, and exit record.
- Original `TRM-009-snapshot.png`, SHA-256, size, mode/identity data, and decoded PNG report.
- `TRM-009-visual-classification.md` naming Class A, B, C, or D with reasons.
- Complete locked renderer test output and runner architecture.
- Existing-path negative result proving no overwrite.

Pass/Fail Criteria:

PASS if output, metadata, PNG structure, path behavior, golden hashes, and visual classification satisfy the contract. FAIL for any Class A regression, malformed PNG, stdout payload leak, overwrite, or output opened before validation. A deliberate Class B fixture passes only when its fallback count matches. Class C is documented and does not fail. Class D invalidates the evidence and requires recollection.

### TRM-010: Snapshot capture does not mutate the target

Purpose:

Verify that successful and failed snapshots do not focus, select, resize, wake, spawn, repaint, write to, or otherwise mutate a target, including when no frontend terminal is mounted.

Preconditions:

- One harmless target session can remain stable without producing autonomous output.
- The tester can observe session IDs, backend route, dimensions, sequence, focus/window state, and session count.
- A hidden, minimized, detached, or never-mounted frontend state is available where supported.
- Separate authorized, unauthorized, disabled, and invalid-output fixtures are available without real model workflows.

Steps:

1. Record target session count, exact selected session ID/backend, dimensions, sequence, status, working state, focus, active UI selection, window state, and harmless viewport marker.
2. Hide, minimize, detach, or avoid mounting the target frontend. Record the chosen condition.
3. Capture JSON, then capture PNG to a new safe path while no target output is expected.
4. Record the same target fields immediately after each request. Confirm session ID/backend and dimensions did not change; sequence remains unchanged unless independently observed PTY output occurred.
5. Confirm the frontend did not focus, raise, select, repaint, or switch active session because of the request.
6. Confirm no input marker, Enter, wake, spawn, resize, screenshot overlay, notification, ordinary message, conversation entry, PTY-input row, or standard delivered/rejected artifact was created.
7. Repeat with the setting disabled, an unauthorized shape-valid route, and an unsafe existing PNG path. Confirm zero snapshot content bytes and the same no-mutation properties.
8. Inspect only metadata-safe audit and logs. Confirm snapshot content, ANSI, target title, token, nonce, PNG/base64 prefix, and output path are absent.
9. Confirm a consumed host response is removed. After 60 seconds, confirm identity-stable protocol files are swept where the requester directories remain discoverable. Record, rather than conceal, any documented crash/unregistration residual.

Expected Result:

Authorized reads succeed from backend state regardless of frontend visibility. Every success and failure leaves target lifecycle, PTY input, dimensions, focus, UI selection, and ordinary messaging unchanged.

Evidence Required:

- `TRM-010-before-state.json`, `TRM-010-after-json-state.json`, and `TRM-010-after-png-state.json`.
- Frontend hidden/minimized/detached/unmounted evidence that does not serve as renderer golden evidence.
- Authorized and negative command stdout/stderr/exit records.
- Metadata-only audit/log sentinel report.
- Dedicated protocol-directory before, consumed, and TTL observations.

Pass/Fail Criteria:

PASS if all successful and negative paths are non-mutating and leak no content outside the declared output. FAIL on any target write, lifecycle action, focus/select/resize side effect, OS capture invocation, ordinary-message artifact, or disabled/unauthorized content byte. BLOCKED if target state cannot be observed safely.

## Terminal snapshot evidence contract

### Automated portable evidence

The PR workflow must run these locked package tests on `windows-latest`, `ubuntu-latest`, `macos-15`, and `macos-15-intel`:

```text
cargo test --locked -p terminal-snapshot-renderer
cargo test --locked -p session-bridge --bin agentscommander-api-helper terminal_snapshot
```

The renderer test decodes each golden, asserts exact RGB8 dimensions and allowed chunks, verifies embedded font/license hashes, samples contract pixels, and compares the same checked-in PNG hashes on every runner. The helper tests cover strict request/response, no-proxy/no-redirect/no-retry behavior, bounds, corruption, deadlines, and validate-before-create output.

Windows also retains full daemon/CLI/API Rust gates, frontend gates, and release CLI smoke. The release smoke directly invokes root `--help` and `terminal-snapshot --help` under both `powershell.exe` and `pwsh.exe` for both release binaries. It asserts exit 0, expected syntax, and empty stderr without attempting a live capture.

### Manual platform record

Record each tested OS image and architecture separately:

| Platform | Required manual observations |
|---|---|
| Windows 10 1809+ or Windows 11 x86_64 | ConPTY local target; Docker target/requester when available; hidden/minimized/unmounted UI; NTFS create-new, reparse, ACL, and cleanup behavior; PNG decode and classification. |
| Ubuntu latest x86_64 | Unix PTY local target; optional Docker target/requester; headless capture; protocol directory `0700` and file/output `0600`; PNG decode and classification. |
| macOS arm64 and x86_64 | Unix PTY local target; headless capture; file mode and cleanup; PNG decode and classification. Record the current project support caveat instead of claiming broad macOS certification. |

Docker-dependent manual evidence may be `SKIP` only when Docker is unavailable, with the exact reason. Pure in-process local/container backend automation may not skip.

### Visual review classes

- **Class A, contract regression:** Byte hash, dimensions, cell placement, palette, cursor, wide pairing, clipping, or chunk set differs. This blocks merge.
- **Class B, declared fallback:** U+FFFD or the fixed hollow-box replacement appears and `fallbackGlyphCount` matches. Accept only for a fixture intentionally missing a font glyph.
- **Class C, documented frontend difference:** The fixed font, palette, or cursor differs from xterm/WebGL, or unsupported images, selection, or ligatures are absent while fidelity is correct. This is not a renderer defect.
- **Class D, invalid evidence:** The claimed golden came from OS window, monitor, desktop, or WebView capture, or depends on an installed font. Reject and recollect the evidence from renderer bytes.

Do not silently update a golden hash. A deliberate renderer change requires a renderer-version change, reviewed golden update, third-party notice review, and schema/documentation review.

A normal app screenshot can document frontend visibility or non-mutation, but it cannot prove PNG golden fidelity. Preserve the generated PNG bytes, hash, decoder report, runner identity, and classification as the renderer evidence.
