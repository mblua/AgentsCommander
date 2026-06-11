# AgentsCommander Regression Testing

This directory contains repeatable, versioned regression suites for AgentsCommander. The suites are written for human-style GUI and CLI validation: each case describes the setup, action sequence, expected result, required evidence, and pass/fail criteria.

The goal is to make future rounds reproducible without turning every run into one large ad hoc report. Each functional area has its own file and should be executed in order unless a case explicitly states that it is independent.

## Visual Test Environment

- The app under test must be the workgroup-specific build for the run. Example:
  `C:\Users\maria\0_mmb\0_AC\agentscommander_standalone_wg-1.exe --app`
- The target window must be identified by title. Example:
  `Agents Commander [STANDALONE_WG-1]`
- If multiple AgentsCommander windows are open, never assume the active window is the target. Select the window whose title matches the workgroup under test.
- For GUI and human-style checks, the app should be maximized on the monitor designated by the user for visual validation.
- Before each execution, record the target HWND/PID, window rectangle, whether the window is maximized, and the capture method used.
- In multi-monitor setups, modals and menus can appear outside the crop for the target monitor. Use virtual desktop capture, HWND capture, adjacent crop, or relative-window coordinate capture as fallback evidence.
- Prefer coordinates relative to the detected target window. Do not hardcode absolute screen coordinates except when documenting a concrete execution.

## Future Testability Support Tracked In #475

Issue #475 tracks deterministic GUI test support. It is not implemented yet, so current visual tests still require manual placement or external automation to locate and maximize `Agents Commander [STANDALONE_WG-1]`.

The expected future support is a dedicated testable binary, `agentscommander_testeable.exe`, that can launch with deterministic placement. When available, this suite should use an invocation equivalent to:

```powershell
agentscommander_testeable.exe --app `
  --window-x <x> `
  --window-y <y> `
  --window-width <w> `
  --window-height <h> `
  --window-maximized
```

Even in that future mode, each case must record the real HWND, PID, and window rectangle before clicking. The suite should fail early if the actual target window does not land on the expected monitor or rectangle.

Issue #475 also proposes a safe reset command. This command is planned only; do not use or document it as available until implemented. The intended contract is:

- It is valid only for a binary/app identity named exactly `agentscommander_testeable.exe`.
- It completely deletes only `.agentscommander_testeable`.
- It completely deletes only the disposable project folder named `agentscommander_testeable`.
- It refuses to run from `agentscommander_standalone.exe`, `agentscommander_standalone_wg-1.exe`, or other normal app binaries.

Until #475 lands, tests that need clean state must create clearly disposable folders and document residual state instead of assuming a reset command exists.

## Case IDs

Use a three-letter functional prefix followed by a zero-padded number:

- `PRJ-###`: Project lifecycle.
- `AGT-###`: Agent lifecycle.
- `TPL-###`: Agent templates and agency setup.
- `TRM-###`: Terminal sessions.
- `MSG-###`: Inter-agent messaging.
- `WGP-###`: Workgroups and peers.
- `SET-###`: Settings and persistence.
- `WIN-###`: Windowing and multi-monitor behavior.

Cases should become progressively more complex inside each file. If a case depends on an earlier case, state the dependency in the preconditions.

## Evidence Rules

Evidence should be enough for another tester to verify the observed state without rerunning the case immediately.

- Store screenshots, JSON state snapshots, command logs, and notes in the executing tester's allowed evidence directory.
- Reference evidence paths from the execution notes or final report.
- For GUI checks, capture the target window before and after the main action.
- For persistence checks, capture both the pre-restart and post-restart state.
- Record any fallback used, such as HWND capture, virtual desktop capture, keyboard navigation, or relative coordinates.
- Do not delete user data to clean up after tests. If a test creates a project and the app has no safe cleanup path, document the residual test project.

## Execution Order

Recommended phase order:

1. `01-project-lifecycle.md`
2. `02-agent-lifecycle.md`
3. `03-agent-templates-agency.md`
4. `04-terminal-sessions.md`
5. `05-inter-agent-messaging.md`
6. `06-workgroups-and-peers.md`
7. `07-settings-and-persistence.md`
8. `08-windowing-and-multimonitor.md`

Within a functional file, run cases in ascending ID order unless the case says it is independent.
