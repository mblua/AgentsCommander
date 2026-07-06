# 01 Project Lifecycle

These cases validate the first project-management surface a user touches: identifying the correct app instance, creating or opening an AC project from the UI, seeing it in the project list, and confirming that project registration persists across an app restart.

Use a clearly disposable project folder for create-flow cases, for example `ac-regression-prj-YYYYMMDD-HHMMSS`. Prefer creating that folder in the tester's allowed scratch/evidence area so the app does not mutate user data. If no safe in-app cleanup exists, record the residual folder and project registration.

Visual preconditions from `README.md#visual-test-environment` apply to every case in this file. PRJ-001 captures them explicitly; later cases inherit them.

Current deterministic mode: use `agentscommander_testeable.exe` with explicit placement and `window-info` verification. Run `agentscommander_testeable.exe test-reset --confirm-testeable` before PRJ cases that require clean disposable state, and only when the testable GUI is not active.

## Execution Log

Date: 2026-06-13

Tester: ac-cli-and-gui-tester

App under test: `target\release\agentscommander_testeable.exe --app --ui-automation`

Evidence root: `C:\Users\maria\0_repos\AgentsCommander_ac\.ac\wg-14-acceptance-testing\__agent_ac-cli-and-gui-tester\evidence\ui-regression-baseline-20260613-191000`

Result summary:

- Native folder picker cancel path passed without mutating settings.
- Creating from a selected empty disposable folder in clean/no-workspace state loaded the project directly; no `project.createConfirm.*` modal appeared.
- Current source behavior supports that observation: the no-workspace `New Project` path creates and loads directly after folder selection.
- PRJ-008 is therefore a conditional confirmation-modal case, not part of the clean no-workspace happy path unless a future UI change reintroduces that modal.

Date: 2026-06-11

Tester: ac-cli-tester

App under test: `C:\Users\maria\0_mmb\0_AC\agentscommander_standalone_wg-1.exe --app`

Target window: `AC [STANDALONE_WG-1]`

Evidence root: `C:\Users\maria\0_repos\AgentsCommander_ac\.ac\wg-1-dev-team\__agent_ac-cli-tester\evidence\testing-phase-1`

Test project: `C:\Users\maria\0_repos\AgentsCommander_ac\.ac\wg-1-dev-team\__agent_ac-cli-tester\evidence\testing-phase-1\ac-regression-prj-20260611-020008`

| Case | Result | Evidence | Notes |
| --- | --- | --- | --- |
| PRJ-001 | PARTIAL | `PRJ-001-window-identifiable-after-relaunch.png`, `PRJ-001-window-state-after-relaunch.json` | Target was identifiable and maximized after relaunch. Initial capture from a negative-coordinate monitor produced a bad crop; relaunch recovered capture. |
| PRJ-002 | PASS | `PRJ-002-new-open-menu.png`, `PRJ-002-after-new-project.png`, `PRJ-002-003-settings-registration.json` | UI `New Project` flow created `.ac/` and registered the disposable project once. Native folder picker evidence was limited; UIA confirmed picker state. |
| PRJ-003 | PARTIAL | `PRJ-002-003-settings-registration.json`, `PRJ-003-sidebar-after-scroll-restored.png` | Settings show the created project is registered once. Sidebar visual capture did not show the new entry after a geometry/sidebar anomaly. |
| PRJ-004 | PASS | `PRJ-004-post-relaunch-window-state.json`, `PRJ-004-post-relaunch-registration.json`, `PRJ-004-post-relaunch.png` | Project registration survived normal close/relaunch. |
| PRJ-005 | PARTIAL | `PRJ-005-open-existing-dedupe.json` | Existing-project dedupe was verified with CLI fallback because native picker automation was unstable. UI-only coverage remains pending. |
| PRJ-006 | BLOCKED | `PRJ-003-sidebar-after-scroll-restored.png` | Basic project navigation could not be verified because the sidebar was not reliably visible/capturable after the window geometry anomaly. |
| PRJ-007 | PARTIAL | `PRJ-007-cancel-open-flow-settings-compare.json` | UI open flow was started and canceled with Escape; settings were unchanged. Picker screenshot evidence was not captured. |

Residual test data:

- The disposable project folder remains at `C:\Users\maria\0_repos\AgentsCommander_ac\.ac\wg-1-dev-team\__agent_ac-cli-tester\evidence\testing-phase-1\ac-regression-prj-20260611-020008`.
- The folder contains a generated `.ac/` Project AC Root.
- The project remains registered in `C:\Users\maria\0_mmb\0_AC\.agentscommander_standalone_wg-1\settings.json`.
- No cleanup was performed because the current app has no safe reset command for normal binaries; planned support is tracked by #475.

Automation gaps observed:

- Window placement is not deterministic in the current wg-specific app. The first target window was on a negative-coordinate monitor and produced a bad crop.
- Moving/restoring the Tauri window can leave the webview/capture geometry inconsistent.
- Native folder picker automation is unstable without deterministic placement and a resettable test app identity.
- Sidebar verification needs either deterministic window sizing or an app-level test hook to query loaded project UI state.

### PRJ-001: App launches and target wg-1 window is identifiable

Purpose:

Verify that the workgroup-specific app instance is running, visually identifiable, and ready for GUI validation.

Preconditions:

- App wg-1 is open or launchable with `C:\Users\maria\0_mmb\0_AC\agentscommander_standalone_wg-1.exe --app`.
- Window title `AC [STANDALONE_WG-1]` is detectable.
- Window is maximized on the monitor designated by the user for visual validation.
- Initial capture is saved as evidence.
- If the window is not maximized or is on a different monitor, the case must report `BLOCKED`/`PARTIAL` or move/maximize the window if the test plan permits, leaving evidence.

Steps:

1. Start the app if it is not already running.
2. Enumerate visible AgentsCommander windows and select only the one whose title is `AC [STANDALONE_WG-1]`.
3. Record PID, HWND, executable path, window rectangle, maximized state, and capture method.
4. Save a screenshot of the target window.
5. Confirm the header shows the `STANDALONE_WG-1` badge and the main/sidebar UI is readable.

Expected Result:

The target wg-1 window is uniquely identifiable, maximized on the intended monitor, and visually readable.

Evidence Required:

- Window-state snapshot with PID/HWND/title/rect/maximized flag.
- Screenshot of the target window.

Pass/Fail Criteria:

Pass if the correct window is detected, maximized, and the screenshot clearly shows the wg-1 UI. Partial if the app is detected but required repositioning/relaunch to become capturable. Fail if the app cannot be launched or identified.

### PRJ-002: Create a new test project from UI

Purpose:

Verify that a user can create/register an AC project from an empty folder through the UI open-project flow.

Preconditions:

- Depends on PRJ-001.
- A disposable empty folder exists for this run.
- The folder name is unique and clearly disposable, such as `ac-regression-prj-YYYYMMDD-HHMMSS`.

Steps:

1. Click `New / Open` in the target window.
2. Choose the open-project action.
3. In the folder picker, select the disposable empty folder.
4. If the app shows a create confirmation for the empty folder, confirm creation.
5. If no confirmation appears, wait for direct project creation/loading to complete.
6. Wait for the sidebar to refresh.

Expected Result:

The app creates the Project AC Root in the selected folder, registers the project, and loads it into the sidebar without disturbing existing projects.

Evidence Required:

- Screenshot before opening the picker.
- Screenshot of the create confirmation prompt when present, or evidence that no prompt appeared and the project loaded directly.
- Screenshot after creation showing the newly loaded project.
- Filesystem or CLI state showing that `.ac/` exists in the disposable folder.

Pass/Fail Criteria:

Pass if the project is created through the UI and appears in the app. Partial if creation succeeds but one transient modal could not be captured. Fail if the UI cannot create or register the project.

### PRJ-003: Created project appears in project list/sidebar

Purpose:

Verify that the newly created project is visible in the sidebar project list.

Preconditions:

- Depends on PRJ-002.

Steps:

1. Inspect the sidebar project list.
2. Locate the project entry matching the disposable folder name.
3. Expand the project entry if needed.
4. Confirm project sections such as workgroups, agents, or teams are visible.

Expected Result:

The newly created project appears as a project entry in the sidebar and can be expanded or inspected.

Evidence Required:

- Sidebar screenshot showing the disposable project entry.

Pass/Fail Criteria:

Pass if the project entry is visible and matches the selected folder. Fail if the project was created on disk but never appears in the sidebar.

### PRJ-004: Close/reopen app and confirm project persists

Purpose:

Verify that project registration survives an app restart.

Preconditions:

- Depends on PRJ-003.
- The app can be closed normally without terminating unrelated AgentsCommander windows.

Steps:

1. Capture the sidebar state with the disposable project visible.
2. Close the target `AC [STANDALONE_WG-1]` window normally.
3. Relaunch `C:\Users\maria\0_mmb\0_AC\agentscommander_standalone_wg-1.exe --app`.
4. Detect and maximize the target window again.
5. Capture the sidebar state after relaunch.
6. Confirm the disposable project is still listed.

Expected Result:

The disposable project remains registered and visible after the app closes and reopens.

Evidence Required:

- Pre-close screenshot.
- Post-relaunch window-state snapshot.
- Post-relaunch screenshot showing the project entry.

Pass/Fail Criteria:

Pass if the project is visible after relaunch. Partial if relaunch required geometry recovery but the project persisted. Fail if the project disappears or the app cannot reopen.

### PRJ-005: Open an existing project from UI

Purpose:

Verify that a user can open/register an existing AC project folder from the UI.

Preconditions:

- Depends on PRJ-001.
- An existing folder with a Project AC Root (`.ac/`) is available.
- Prefer using the disposable project created in PRJ-002 after it has a `.ac/` directory.

Steps:

1. Click `New / Open`.
2. Choose the open-project action.
3. Select an existing AC project folder.
4. Wait for the sidebar refresh.
5. Confirm the project is present and not duplicated.

Expected Result:

The existing AC project loads without a create confirmation and without duplicate project entries.

Evidence Required:

- Screenshot after opening the existing project.
- Optional settings/project state snapshot showing a single registration for the path.

Pass/Fail Criteria:

Pass if the project opens and deduplicates correctly. Fail if the project duplicates, prompts for creation despite `.ac/`, or does not load.

### PRJ-006: Project selection survives basic navigation

Purpose:

Verify that loaded project state remains stable while the user performs simple in-app navigation.

Preconditions:

- Depends on PRJ-003 or PRJ-005.
- At least one project entry is visible in the sidebar.

Steps:

1. Select or expand the disposable project entry.
2. Click another safe sidebar section or project entry.
3. Return to the disposable project entry.
4. Confirm the project remains visible and its expanded/collapsed state is coherent.

Expected Result:

The project list remains stable during basic navigation and the user can return to the same project entry.

Evidence Required:

- Screenshot before navigation.
- Screenshot after returning to the project entry.

Pass/Fail Criteria:

Pass if the project remains visible and navigable. Fail if the entry disappears, changes identity, or navigation selects the wrong project unexpectedly.

### PRJ-007: Cancel create/open flow does not mutate project list

Purpose:

Verify that canceling an open/create flow leaves the sidebar project list unchanged.

Preconditions:

- Depends on PRJ-001.
- Baseline project list has been captured.

Steps:

1. Capture the baseline project list.
2. Click `New / Open`.
3. Start the open-project flow.
4. Cancel the folder picker or creation confirmation before accepting any folder.
5. Capture the project list again.
6. Compare the project list to the baseline.

Expected Result:

Canceling the flow does not add, remove, duplicate, or reorder projects unexpectedly.

Evidence Required:

- Baseline screenshot.
- Post-cancel screenshot.

Pass/Fail Criteria:

Pass if the visible project list is unchanged. Partial if a native picker prevented screenshot capture but post-cancel state is unchanged. Fail if canceling mutates project registration or visible project list.

### PRJ-008: Cancel empty-folder create confirmation when present

Purpose:

Verify that canceling the create confirmation for an empty folder leaves the folder and project registration unchanged when that confirmation exists in the active UI path.

Preconditions:

- Depends on PRJ-001.
- A disposable empty folder exists and does not contain `.ac/`.
- Baseline project registration state has been captured.
- The active product path shows a create confirmation after selecting an empty folder. If a clean no-workspace run creates directly instead, record that as "not applicable for this path" and rely on PRJ-007 for cancel-before-selection coverage.

Steps:

1. Click `New / Open`.
2. Choose the new/open project action that opens the folder picker.
3. Select the disposable empty folder.
4. When the create confirmation appears, choose cancel.
5. Wait for the dialog to close.
6. Capture the project registration state and inspect the disposable folder.
7. Repeat PRJ-002 afterward if the same run needs the happy path.

Expected Result:

Canceling the create confirmation does not create `.ac/`, does not register the folder, and does not mutate the visible project list unexpectedly.

Evidence Required:

- Baseline project registration snapshot.
- Screenshot of the create confirmation prompt when possible.
- Screenshot or state snapshot after cancel.
- Read-only filesystem snapshot proving `.ac/` was not created.

Pass/Fail Criteria:

Pass if cancel leaves registration and filesystem unchanged. Not applicable if the active UI path does not show a create confirmation for the selected folder. Fail if a visible cancel action creates `.ac/` or registers the project despite canceling.
