# AgentsCommander Regression Testing

This directory contains repeatable, versioned regression suites for AgentsCommander. The suites are written for human-style GUI and CLI validation: each case describes the setup, action sequence, expected result, required evidence, and pass/fail criteria.

The goal is to make future rounds reproducible without turning every run into one large ad hoc report. Each functional area has its own file and should be executed in order unless a case explicitly states that it is independent.

## Visual Test Environment

- The app under test should be `agentscommander_testeable.exe` for deterministic GUI regression runs.
- The testable app uses a disposable config directory next to the binary: `.agentscommander_testeable`.
- The target window title for the testable app is `Agents Commander [TESTEABLE]`.
- If multiple AgentsCommander windows are open, never assume the active window is the target. Select the window whose title matches the workgroup under test.
- For GUI and human-style checks, launch the app at the monitor rectangle designated by the user for visual validation.
- Current visual-test baseline:
  - Logical display: `\\.\DISPLAY9`
  - Physical rect for `window-info` / DWM: `Left=-1920 Top=0 Width=1920 Height=1080`
  - Logical rect observed via Windows Forms: `Left=-1920 Top=0 Width=1280 Height=720`
  - Physical and logical rectangles can differ because DPI scaling changes the coordinate space reported by different Windows APIs.
- Before each execution, record the target HWND/PID, process path, window rectangle, whether the window is maximized, and the capture method used.
- In multi-monitor setups, modals and menus can appear outside the crop for the target monitor. Use virtual desktop capture, HWND capture, adjacent crop, or relative-window coordinate capture as fallback evidence.
- If screenshot capture is based on a screen rectangle, verify the target window is foreground and unobscured before counting the image as product evidence. Foreground terminals or other windows can contaminate otherwise correct target-rectangle captures.
- Prefer coordinates relative to the detected target window. Do not hardcode absolute screen coordinates except when documenting a concrete execution.

## Deterministic Testable App

Production Windows builds create a raw testable executable alongside the normal production executable:

```text
src-tauri/target/release/agentscommander.exe
src-tauri/target/release/agentscommander_testeable.exe
```

Launch with explicit virtual-desktop placement in physical pixels:

```powershell
.\agentscommander_testeable.exe --app `
  --window-x <x> `
  --window-y <y> `
  --window-width <w> `
  --window-height <h> `
  --window-maximized
```

Current default visual-test example:

```powershell
.\agentscommander_testeable.exe --app `
  --window-x -1920 `
  --window-y 0 `
  --window-width 1920 `
  --window-height 1080 `
  --window-maximized
```

The same placement can be provided through `AC_TEST_WINDOW_PLACEMENT`:

```powershell
$env:AC_TEST_WINDOW_PLACEMENT='{"x":-1920,"y":0,"width":1920,"height":1080,"maximized":true}'
.\agentscommander_testeable.exe --app
```

Placement flags and `AC_TEST_WINDOW_PLACEMENT` are accepted only by `agentscommander_testeable.exe`. Normal, stage, and workgroup binaries fail closed when they receive test placement input.

After launch, query the target window from a separate process:

```powershell
.\agentscommander_testeable.exe window-info
```

`window-info` returns JSON containing `processPath`, PID, HWND, rectangle, and maximized state. Tests must assert that `processPath` exactly equals the launched `agentscommander_testeable.exe` path before clicking or capturing. A passing visual gate should also assert that `window-info` reports approximately `left=-1920`, `top=0`, `width=1920`, `height=1080`, and `maximized:true`. If the user moves the app to a different monitor, update this baseline after measuring the new target with `window-info`.

Historical reference: the old placement `--window-x -2891 --window-y -11 --window-width 1942 --window-height 1102` is obsolete and should not be used as a live visual-test baseline.

## Semantic UI Automation

When the testable GUI is launched with `--ui-automation`, prefer semantic `ui-*` commands over coordinate clicks for WebView controls. Stable targets use exact `data-ac-testid` selectors; screenshots remain evidence, not the action mechanism.

The selector seed and missing-affordance tracker lives in [semantic-ui-automation-affordance-matrix.md](semantic-ui-automation-affordance-matrix.md). If acceptance can perform a behavior with screen and mouse but cannot inspect or operate it semantically, report the missing selector/action there.

## Test Reset

Use the explicit reset command only from `agentscommander_testeable.exe` and only when the testable GUI is not running:

```powershell
.\agentscommander_testeable.exe test-reset --confirm-testeable
```

The command deletes only these sibling paths under the executable directory:

- `.agentscommander_testeable`
- `agentscommander_testeable`

It refuses files, symlinks, junctions, mount points, Windows reparse-point directories, normal binaries, and active testable GUI instances. Refusal cases return structured JSON on stderr. Successful runs print newline-delimited JSON on stdout, with a planned-delete record followed by the final result.

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
- `E2E-###`: Cross-surface user journeys that stitch multiple suites together.

Cases should become progressively more complex inside each file. If a case depends on an earlier case, state the dependency in the preconditions.

## Evidence Rules

Evidence should be enough for another tester to verify the observed state without rerunning the case immediately.

- Store screenshots, JSON state snapshots, command logs, and notes in the executing tester's allowed evidence directory.
- Reference evidence paths from the execution notes or final report.
- For GUI checks, capture the target window before and after the main action.
- For persistence checks, capture both the pre-restart and post-restart state.
- Record any fallback used, such as HWND capture, virtual desktop capture, keyboard navigation, or relative coordinates.
- Do not delete user data to clean up after tests. Use `test-reset --confirm-testeable` only for the disposable testable app identity. If a test creates data outside that identity, document the residual test project.

## Current Baseline Seed

The 2026-06-13 end-to-end seeding run produced a PARTIAL result, not a baseline PASS.

Evidence root:

```text
C:\Users\maria\0_repos\AgentsCommander_ac\.ac\wg-14-acceptance-testing\__agent_ac-cli-and-gui-tester\evidence\ui-regression-baseline-20260613-191000
```

Use that evidence for future targeted reruns of onboarding, project creation, agent creation, team creation, workgroup activation, and restart persistence. Do not treat team/workgroup coverage as passed from that run.

## Execution Order

Recommended phase order:

1. `01-project-lifecycle.md`
2. `02-onboarding-and-coding-agents.md`
3. `03-agent-lifecycle.md`
4. `04-team-and-workgroup-lifecycle.md`
5. `05-end-to-end-user-journey.md`
6. `06-agent-templates-agency.md`
7. `07-terminal-sessions.md`
8. `08-inter-agent-messaging.md`
9. `09-settings-and-persistence.md`
10. `10-windowing-and-multimonitor.md`

Within a functional file, run cases in ascending ID order unless the case says it is independent.
