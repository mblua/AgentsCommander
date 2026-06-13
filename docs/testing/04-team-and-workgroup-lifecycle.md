# 04 Team And Workgroup Lifecycle

These cases validate that a user can compose project agents into a team and activate that team into a workgroup from the GUI.

Use only disposable projects and disposable agents created for the current run. If a test adds repository access to a team, use a deliberately safe public test repository or a local disposable repository approved by the test request.

Product actions must be performed through the GUI. CLI is allowed only for harness control, semantic UI automation, screenshots, logs, and read-only verification.

## Execution Log

Date: 2026-06-13

Tester: ac-cli-and-gui-tester

Evidence root: `C:\Users\maria\0_repos\AgentsCommander_ac\.ac\wg-14-acceptance-testing\__agent_ac-cli-and-gui-tester\evidence\ui-regression-baseline-20260613-191000`

Result summary:

- WGP-000 team gating was partially exercised: empty-name disabled state and Back/Cancel no-mutation state were captured.
- WGP-001 did not pass. Keyboard entry/focus into the New Team flow was unreliable, and later evidence showed the app had returned to first-run onboarding instead of New Team step 2.
- WGP-002 and WGP-003 were not reached.
- Until New Team/New Workgroup selectors exist and onboarding persistence is fixed, this suite should be treated as blocked for automated baseline PASS.

Known automation support:

- Workgroup replica rows have some semantic selectors for quick/workgroup contexts, mainly for repo badge verification.

Known automation gaps:

- Team and Workgroup section headers, context menus, New Team modal fields, agent checkboxes, coordinator radio buttons, repository assignment controls, New Workgroup modal fields, team select, task-title input, team rows, workgroup rows, and agent rows do not yet have stable semantic selectors.
- Until those hooks exist, this suite is repeatable as a human-style GUI script with screenshots and read-only filesystem verification, not fully repeatable through `ui-click` and `ui-set`.
- Screen-rectangle screenshots can be contaminated by foreground windows. Before counting a screenshot as evidence, verify the target HWND/PID and that the target surface is unobscured.

### WGP-000: Team and workgroup creation controls guard incomplete input

Purpose:

Verify that team and workgroup creation dialogs do not create entities until required inputs are present.

Preconditions:

- A disposable project is visible.
- At least one disposable agent exists for team checks.
- Baseline `.ac/` contents have been captured.

Steps:

1. Open New Team.
2. Confirm `Next` is disabled or blocked while the team name is empty.
3. Enter a valid team name and continue.
4. Confirm `Next` is disabled or blocked until at least one agent and one coordinator are selected.
5. Use Back and Cancel before final creation.
6. Confirm no `_team_*` directory was created.
7. If a team already exists, open New Workgroup.
8. Confirm `Create` is disabled or blocked until a team and non-empty task title are present.
9. Cancel New Workgroup.
10. Confirm no new `wg-*` directory was created.

Expected Result:

Incomplete or canceled entity creation flows do not create team or workgroup state.

Evidence Required:

- Screenshots showing disabled/blocked Next or Create states.
- Screenshot of Back/Cancel path when possible.
- Read-only filesystem snapshot proving no new `_team_*` or `wg-*` directory was created.

Pass/Fail Criteria:

Pass if required inputs gate creation and cancel/back navigation does not mutate state. Fail if incomplete input can create entities or cancel/back writes persistent state.

### WGP-001: Create a team from existing agents

Purpose:

Verify that a user can create a team by selecting existing project agents and one coordinator.

Preconditions:

- At least two disposable project agents exist and are visible in the project Agents section.
- A unique team name is available, for example `regression-team-<timestamp>`.

Steps:

1. Open the project context menu or Teams section context menu.
2. Choose `New Team`.
3. Enter the unique team name.
4. Continue to agent selection.
5. Select the disposable agents.
6. Mark one selected agent as coordinator.
7. Continue to repository assignment.
8. Leave repositories empty unless the run explicitly covers repo cloning.
9. Click `Create`.
10. Wait for project sidebar refresh.
11. Expand Teams and inspect the new team.

Expected Result:

The app creates the team, shows it in the Teams section, and lists the selected members with the coordinator badge.

Evidence Required:

- Screenshot of New Team step 1.
- Screenshot of selected agents and coordinator in step 2.
- Screenshot of repository assignment step.
- Screenshot of the created team in the sidebar.
- Read-only filesystem snapshot of `.ac/_team_<name>/config.json`.

Pass/Fail Criteria:

Pass if the team is created from the GUI with the expected roster and coordinator. Fail if the roster, coordinator, or team location is wrong.

### WGP-002: Activate a workgroup from a team

Purpose:

Verify that a user can activate a team into a new workgroup from the GUI.

Preconditions:

- Depends on WGP-001.
- A unique task title is available, for example `Regression UI baseline <timestamp>`.

Steps:

1. Open the project context menu or Workgroups section context menu.
2. Choose `New Workgroup`.
3. Select the team created in WGP-001.
4. Enter the unique task title.
5. Click `Create`.
6. Wait for workgroup creation and sidebar refresh.
7. Expand the Workgroups section.
8. Inspect the new workgroup and its replicas.

Expected Result:

The app creates a new `wg-<N>-<team>` directory, copies team agents as replicas, writes `TASK.md`, refreshes the sidebar, and shows the workgroup with its members.

Evidence Required:

- Screenshot of the filled New Workgroup modal.
- Screenshot of the Workgroups section after creation.
- Read-only filesystem snapshot showing the new workgroup directory, `TASK.md`, replica directories, and messaging directory.
- Any progress or error message shown during creation.

Pass/Fail Criteria:

Pass if the workgroup is created from the GUI and appears with the expected team replicas. Fail if creation errors, clones wrong agents, writes no task, or does not refresh.

### WGP-003: Launch coordinator session from created workgroup

Purpose:

Verify that the newly created workgroup exposes a usable coordinator replica entry and session launch path.

Preconditions:

- Depends on WGP-002.
- At least one configured coding agent exists.
- The selected coordinator agent is visible in the workgroup.

Steps:

1. Click the coordinator replica row in the workgroup.
2. If prompted to choose a coding agent, choose the configured test coding agent.
3. Wait for the terminal/session surface to reflect the launched coordinator.
4. Confirm the session identity matches the workgroup replica.

Expected Result:

The coordinator replica launches or focuses a terminal session for the correct workgroup replica.

Evidence Required:

- Screenshot of the workgroup before launching.
- Screenshot of any coding-agent picker prompt.
- Screenshot of the terminal/session after launch.
- Read-only session list or app state snapshot if available.

Pass/Fail Criteria:

Pass if the coordinator session starts or focuses with the expected workgroup identity. Fail if the wrong replica launches, no session starts, or the session starts outside the workgroup.
