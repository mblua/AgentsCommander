# 04 Team, Workgroup, And Peer Lifecycle

These cases validate that a user can compose project agents into a team, activate that team into a workgroup from the GUI, and verify workgroup-specific peer discovery and isolation.

Use only disposable projects and disposable agents created for the current run. If a test adds repository access to a team, use a deliberately safe public test repository or a local disposable repository approved by the test request.

Product actions must be performed through the GUI. CLI is allowed only for harness control, semantic UI automation, screenshots, logs, and read-only verification.

For peer discovery checks, run `list-peers-lean` only from disposable sender roots and compare canonical peer names with GUI-visible participants. Never infer valid peers from filesystem directory names.

## Execution Log

Date: 2026-06-13

Tester: ac-cli-and-gui-tester

Evidence root: `C:\Users\maria\0_repos\AgentsCommander_ac\.ac\wg-14-acceptance-testing\__agent_ac-cli-and-gui-tester\evidence\ui-regression-baseline-20260613-191000`

Result summary:

- WGP-000 team gating was partially exercised: empty-name disabled state and Back/Cancel no-mutation state were captured.
- WGP-001 did not pass. Keyboard entry/focus into the New Team flow was unreliable, and later evidence showed the app had returned to first-run onboarding instead of New Team step 2.
- WGP-002 and WGP-003 were not reached.
- WGP-004 through WGP-006 are documented #485 peer/isolation cases and were not executed in the 2026-06-13 baseline run.
- Until New Team/New Workgroup selectors exist and onboarding persistence is fixed, this suite should be treated as blocked for automated baseline PASS.

Known automation support:

- Workgroup replica rows have some semantic selectors for quick/workgroup contexts, mainly for repo badge verification.
- `list-peers-lean` provides canonical peer names and reachability for cross-checking GUI-visible workgroup participants.

Known automation gaps:

- Team and Workgroup section headers, context menus, New Team modal fields, agent checkboxes, orchestrator radio buttons, repository assignment controls, New Workgroup modal fields, team select, task-title input, team rows, workgroup rows, and agent rows do not yet have stable semantic selectors.
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
4. Confirm `Next` is disabled or blocked until at least one agent and one orchestrator are selected.
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

Verify that a user can create a team by selecting existing project agents and one orchestrator.

Preconditions:

- At least two disposable project agents exist and are visible in the project Agents section.
- A unique team name is available, for example `regression-team-<timestamp>`.

Steps:

1. Open the project context menu or Teams section context menu.
2. Choose `New Team`.
3. Enter the unique team name.
4. Continue to agent selection.
5. Select the disposable agents.
6. Mark one selected agent as orchestrator.
7. Continue to repository assignment.
8. Leave repositories empty unless the run explicitly covers repo cloning.
9. Click `Create`.
10. Wait for project sidebar refresh.
11. Expand Teams and inspect the new team.

Expected Result:

The app creates the team, shows it in the Teams section, and lists the selected members with the orchestrator badge.

Evidence Required:

- Screenshot of New Team step 1.
- Screenshot of selected agents and orchestrator in step 2.
- Screenshot of repository assignment step.
- Screenshot of the created team in the sidebar.
- Read-only filesystem snapshot of `.ac/_team_<name>/config.json`.

Pass/Fail Criteria:

Pass if the team is created from the GUI with the expected roster and orchestrator. Fail if the roster, orchestrator, or team location is wrong.

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

### WGP-003: Launch orchestrator session from created workgroup

Purpose:

Verify that the newly created workgroup exposes a usable orchestrator replica entry and session launch path.

Preconditions:

- Depends on WGP-002.
- At least one configured coding agent exists.
- The selected orchestrator agent is visible in the workgroup.

Steps:

1. Click the orchestrator replica row in the workgroup.
2. If prompted to choose a coding agent, choose the configured test coding agent.
3. Wait for the terminal/session surface to reflect the launched orchestrator.
4. Confirm the session identity matches the workgroup replica.

Expected Result:

The orchestrator replica launches or focuses a terminal session for the correct workgroup replica.

Evidence Required:

- Screenshot of the workgroup before launching.
- Screenshot of any coding-agent picker prompt.
- Screenshot of the terminal/session after launch.
- Read-only session list or app state snapshot if available.

Pass/Fail Criteria:

Pass if the orchestrator session starts or focuses with the expected workgroup identity. Fail if the wrong replica launches, no session starts, or the session starts outside the workgroup.

### WGP-004: Peer discovery matches visible workgroup participants

Purpose:

Verify that CLI peer discovery aligns with GUI-visible disposable workgroup participants.

Preconditions:

- Depends on WGP-002.
- At least one disposable sender session/root is available.
- Visible participant names are captured from the GUI.

Steps:

1. Capture the GUI participant list for the selected disposable workgroup.
2. From the disposable sender root, run `list-peers-lean` with the current token/root.
3. Save peer discovery stdout/stderr.
4. Confirm each tested peer JSON entry uses a canonical `name` and belongs to the expected workgroup/team.
5. Confirm any recipient candidate has `reachable:true` before considering it addressable.
6. Compare the peer JSON with visible GUI participants and record any expected exclusions.

Expected Result:

Peer discovery returns canonical names for reachable disposable peers that match the visible workgroup participants and routing rules.

Evidence Required:

- `WGP-004-visible-participants.png`.
- `WGP-004-list-peers-lean.json` or command log.
- Notes mapping GUI participants to canonical peer names and `reachable:true` state.

Pass/Fail Criteria:

PASS if peer JSON matches visible disposable participants and reachable peers are clear. PARTIAL if expected exclusions are documented but one GUI participant is not capturable. FAIL if peer names must be inferred from directories, live peers appear as test targets, or reachability is wrong. BLOCKED if no disposable sender root is available.

### WGP-005: Multiple workgroups remain isolated

Purpose:

Verify that two disposable workgroups do not bleed peer/session state into each other.

Preconditions:

- Depends on WGP-002.
- The tester can create or select a second disposable workgroup without mutating live state.
- Each workgroup has distinct names or participant markers.

Steps:

1. Capture the first disposable workgroup's participants, sessions, and peer JSON.
2. Create or select a second disposable workgroup with a distinct title/name.
3. Capture the second workgroup's participants, sessions, and peer JSON.
4. Switch back to the first workgroup and capture its participant/session list again.
5. Compare visible workgroup names, roots, peer names, and sessions.
6. Record any shared team membership that is expected but confirm replica/session identity remains workgroup-specific.

Expected Result:

Each workgroup retains its own visible participants, replicas, sessions, peer names, and workgroup root. State from one workgroup does not appear as active state in the other.

Evidence Required:

- `WGP-005-first-workgroup.png`.
- `WGP-005-first-peers.json`.
- `WGP-005-second-workgroup.png`.
- `WGP-005-second-peers.json`.
- `WGP-005-return-first-workgroup.png`.

Pass/Fail Criteria:

PASS if peer/session state remains isolated by workgroup. PARTIAL if isolation is clear but one optional peer JSON artifact is missing. FAIL if sessions, peers, or roots bleed across workgroups. BLOCKED if a second disposable workgroup cannot be created safely.

### WGP-006: Workgroup and peer state refreshes after restart

Purpose:

Verify that visible workgroups and peer/session state remain predictable after restarting the testable app.

Preconditions:

- Depends on WGP-002 and WGP-004.
- The disposable workgroup state has been captured before restart.
- The tester can close and relaunch only the testable app.

Steps:

1. Capture the pre-restart workgroup list, selected workgroup, participants, sessions, and peer JSON.
2. Close `AC [TESTEABLE]` normally.
3. Relaunch `agentscommander_testeable.exe --app` with deterministic placement.
4. Run `agentscommander_testeable.exe window-info` and capture the relaunched target.
5. Capture the post-restart workgroup list and selected/active workgroup.
6. Run `list-peers-lean` again from the disposable sender root if the sender session/root is available.
7. Compare post-restart GUI and peer state against pre-restart evidence.

Expected Result:

After restart, disposable workgroups, selected workgroup behavior, participant visibility, and peer discovery state are coherent and do not introduce live-state bleed or duplicate entries.

Evidence Required:

- `WGP-006-pre-restart-workgroups.png`.
- `WGP-006-pre-restart-peers.json`.
- `WGP-006-post-window-info.json`.
- `WGP-006-post-restart-workgroups.png`.
- Optional `WGP-006-post-restart-peers.json`.

Pass/Fail Criteria:

PASS if post-restart state matches documented recovery behavior and remains isolated. PARTIAL if GUI state is coherent but peer discovery cannot be rerun after restart. FAIL if workgroups duplicate, disappear unexpectedly, or mix state. BLOCKED if restart cannot be performed without affecting live windows.
