# 10 Windowing And Multimonitor

These cases validate deterministic window placement, multi-monitor capture, modal/menu visibility, detached terminal placement, restart geometry, and capture fallback conventions on Windows.

Use clearly disposable test projects, workgroups, sessions, detached windows, and settings state. Prefer creating test data in the tester's allowed scratch/evidence area. If no safe in-app cleanup exists, record residual state rather than deleting user data manually.

Visual preconditions from `README.md#visual-test-environment` apply to every case in this file. WIN-001 captures target-window identity explicitly; later cases inherit it.

Current deterministic mode: use `agentscommander_testeable.exe` with explicit placement and `window-info` verification. Run `agentscommander_testeable.exe test-reset --confirm-testeable` before cases that require clean disposable state, and only when the testable GUI is not active.

Use only `agentscommander_testeable.exe` placement flags or `AC_TEST_WINDOW_PLACEMENT` for deterministic positioning. Do not use the standalone wg-1 executable baseline from the already-executed project lifecycle suite.

Required evidence categories for this suite: `window-info` JSON, target-window screenshots, virtual desktop or HWND fallback captures when needed, modal/menu captures, detached terminal window state, restart geometry comparisons, and notes identifying the capture method.

## Execution Log

Date: TBD

Tester: TBD

App under test: TBD

Target window: TBD

Evidence root: TBD

Test data: TBD

| Case | Result | Evidence | Notes |
| --- | --- | --- | --- |
| WIN-001 | NOT RUN | No evidence because NOT RUN. | Deterministic placement checks not executed in this run. |
| WIN-002 | NOT RUN | No evidence because NOT RUN. | Target window capture checks not executed in this run. |
| WIN-003 | NOT RUN | No evidence because NOT RUN. | Modal/menu placement checks not executed in this run. |
| WIN-004 | NOT RUN | No evidence because NOT RUN. | Detached terminal placement checks not executed in this run. |
| WIN-005 | NOT RUN | No evidence because NOT RUN. | Restart geometry checks not executed in this run. |
| WIN-006 | NOT RUN | No evidence because NOT RUN. | Capture fallback checks not executed in this run. |

Residual test data:

- None recorded; suite not executed yet.

Automation gaps observed:

- No automation gaps recorded; suite not executed yet.

### WIN-001: Testable app launches at the requested monitor rectangle

Purpose:

Verify that `agentscommander_testeable.exe` honors deterministic placement input and can be identified on the intended monitor.

Preconditions:

- `agentscommander_testeable.exe` is available.
- The tester knows the target visual-test rectangle from `README.md#visual-test-environment` or a measured replacement baseline.
- The testable GUI is closed before launch.

Steps:

1. Set explicit placement using launch flags or `AC_TEST_WINDOW_PLACEMENT`.
2. Launch `agentscommander_testeable.exe --app`.
3. Run `agentscommander_testeable.exe window-info` from a separate process.
4. Save process path, PID, HWND, rectangle, and maximized state.
5. Capture the target window on the selected monitor.
6. Compare the reported rectangle and maximized state to the requested placement.

Expected Result:

The testable app launches as `AC [TESTEABLE]`, `window-info` reports the testable executable path, and the rectangle/maximized state matches the requested monitor placement within expected DPI variance.

Evidence Required:

- `WIN-001-placement-command.txt` or environment value used for placement.
- `WIN-001-window-info.json`.
- `WIN-001-target-window.png`.
- Notes comparing requested and reported rectangles.

Pass/Fail Criteria:

PASS if the target identity and placement are verified. PARTIAL if identity is verified but DPI variance requires documented adjustment. FAIL if the app ignores placement or reports the wrong process. BLOCKED if deterministic placement cannot be requested in the environment.

### WIN-002: Target window capture is readable on the selected monitor

Purpose:

Verify that the chosen capture method produces readable evidence for the target testable window.

Preconditions:

- Depends on WIN-001.
- The app is visible on the selected monitor or capturable through README-approved fallback.
- The tester can record capture method details.

Steps:

1. Capture the target window using the primary planned method.
2. Save `window-info` output taken close to the capture time.
3. Inspect the screenshot for readable sidebar, titlebar, and main content.
4. Record capture method, HWND, PID, process path, rectangle, maximized state, and monitor baseline.
5. If the primary capture is unreadable, retry with an approved fallback such as HWND capture or virtual desktop capture.
6. Save both failed and successful captures when fallback is needed.

Expected Result:

At least one capture method produces readable evidence for the verified target window, and the method details are recorded for repeatability.

Evidence Required:

- `WIN-002-window-info.json`.
- `WIN-002-primary-capture.png`.
- Optional `WIN-002-fallback-capture.png`.
- `WIN-002-capture-method-notes.md` describing method, monitor, and rectangle.

Pass/Fail Criteria:

PASS if the primary capture is readable and tied to the target HWND/PID. PARTIAL if fallback capture is needed but produces readable evidence. FAIL if captures are unreadable or target identity is ambiguous. BLOCKED if no approved capture method is available.

### WIN-003: Modals and menus remain capturable or have documented fallback evidence

Purpose:

Verify that representative menus and modals can be captured or documented with fallback evidence when multi-monitor placement affects crops.

Preconditions:

- Depends on WIN-002.
- A safe modal or menu can be opened without mutating data, such as settings, a create dialog canceled before completion, or a context menu on disposable state.
- The tester can close the modal/menu without applying changes.

Steps:

1. Capture the target app before opening the menu or modal.
2. Open the representative menu or modal.
3. Capture it using the primary target-window crop.
4. If the menu/modal appears outside the crop, capture it with virtual desktop, HWND, adjacent crop, or relative-window coordinate fallback.
5. Record which fallback was needed and why.
6. Close the modal/menu without saving changes.
7. Capture the app after closure to prove state returned safely.

Expected Result:

Menus and modals are either captured in the target evidence or have a documented fallback capture that shows their placement and content.

Evidence Required:

- `WIN-003-before-modal.png`.
- `WIN-003-modal-primary-capture.png`.
- Optional `WIN-003-modal-fallback-capture.png`.
- `WIN-003-after-close.png`.
- Notes identifying fallback method and relative placement if used.

Pass/Fail Criteria:

PASS if modal/menu content is captured by the primary method. PARTIAL if fallback evidence is needed and clearly documented. FAIL if modal/menu cannot be captured or cannot be closed safely. BLOCKED if no safe menu/modal can be opened.

### WIN-004: Detached terminal window placement is identifiable

Purpose:

Verify that a detached terminal created from a disposable session is identifiable and capturable as its own window.

Preconditions:

- Depends on WIN-002.
- A harmless disposable session exists and can be detached safely.
- The tester can capture multiple windows or use approved fallback capture.

Steps:

1. Capture the main app and selected disposable session before detach.
2. Trigger the detach action for the disposable session.
3. Enumerate visible AgentsCommander windows or capture window titles after detach.
4. Capture the detached terminal window, including title, content marker, and rectangle when possible.
5. Capture the main app showing detached state or session association.
6. Record the detached window capture method, HWND/PID if available, and associated session marker.

Expected Result:

The detached terminal opens as a distinct window associated with the disposable session, and its placement/title/content are identifiable in evidence.

Evidence Required:

- `WIN-004-before-detach.png`.
- `WIN-004-detached-terminal.png`.
- `WIN-004-main-after-detach.png`.
- Optional detached-window info or window enumeration output.
- Notes linking detached content to the session marker.

Pass/Fail Criteria:

PASS if detached placement and session association are clear. PARTIAL if association is clear but HWND/PID evidence is unavailable. FAIL if the detached window opens off-screen, is blank, or cannot be tied to the session. BLOCKED if no safe detach fixture exists.

### WIN-005: Window geometry after restart is predictable

Purpose:

Verify that app window geometry after restart follows documented placement or persistence behavior.

Preconditions:

- Depends on WIN-001.
- The tester can restart only the testable app.
- If manual movement/resizing is used, it is limited to the testable window.

Steps:

1. Capture pre-restart `window-info` and target screenshot.
2. If testing manual persistence, move or resize the testable window and capture changed `window-info`.
3. Close `AC [TESTEABLE]` normally.
4. Relaunch `agentscommander_testeable.exe --app` with the same placement method or documented no-placement condition.
5. Run `window-info` immediately after relaunch.
6. Capture the relaunched window.
7. Compare the pre-restart, changed, and post-restart geometry according to documented behavior.

Expected Result:

Post-restart geometry is predictable: it either follows explicit test placement or documented persisted geometry rules, with no off-screen or wrong-monitor surprise.

Evidence Required:

- `WIN-005-pre-restart-window-info.json`.
- `WIN-005-pre-restart.png`.
- Optional `WIN-005-changed-window-info.json`.
- `WIN-005-post-restart-window-info.json`.
- `WIN-005-post-restart.png`.

Pass/Fail Criteria:

PASS if post-restart geometry matches documented behavior. PARTIAL if geometry is usable but requires a documented fallback to interpret DPI variance. FAIL if the app reopens off-screen, on the wrong monitor, or with contradictory `window-info`. BLOCKED if safe restart cannot be performed.

### WIN-006: Multi-monitor fallback evidence is recorded consistently

Purpose:

Verify that at least one README-approved fallback capture method is exercised and documented consistently.

Preconditions:

- Depends on WIN-002.
- The tester can intentionally exercise or simulate a case where primary target crop is insufficient, such as a menu crossing monitor bounds.
- Fallback capture is limited to visual evidence and does not mutate data.

Steps:

1. Record the primary capture method and target window rectangle.
2. Open or position a safe UI surface that demonstrates why fallback evidence is needed.
3. Capture the primary view showing the limitation, if possible.
4. Capture fallback evidence using virtual desktop capture, HWND capture, adjacent crop, or relative-window coordinates.
5. Save notes explaining why the fallback was needed and how it relates to the target window.
6. Close the UI surface and capture the restored target app.

Expected Result:

Fallback evidence is readable, tied to the verified target window, and documented with enough method detail for another tester to reproduce or evaluate the capture.

Evidence Required:

- `WIN-006-primary-limitation.png`.
- `WIN-006-fallback-capture.png`.
- `WIN-006-fallback-notes.md`.
- `WIN-006-restored-target.png`.

Pass/Fail Criteria:

PASS if fallback evidence is clear and method notes are complete. PARTIAL if fallback is readable but one method detail is missing. FAIL if fallback evidence cannot be tied to the target or is unreadable. BLOCKED if no fallback method can be exercised safely in the environment.
