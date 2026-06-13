# 06 Workgroups And Peers

These cases validate team and workgroup GUI flows, workgroup-specific agent replicas, coordinator/member visibility, peer discovery, multiple workgroup isolation, and refresh behavior after workgroup changes.

Use clearly disposable test projects, teams, workgroups, agents, peers, sessions, and settings state. Prefer creating test data in the tester's allowed scratch/evidence area. If no safe in-app cleanup exists, record residual state rather than deleting user data manually.

Visual preconditions from `README.md#visual-test-environment` apply to every case in this file. WGP-001 captures target-window identity explicitly; later cases inherit it.

Current deterministic mode: use `agentscommander_testeable.exe` with explicit placement and `window-info` verification. Run `agentscommander_testeable.exe test-reset --confirm-testeable` before cases that require clean disposable state, and only when the testable GUI is not active.

Use only disposable teams, workgroups, and agent replicas created for the current test run under the testable identity. Do not select, message, close, delete, or mutate live workgroups. If the GUI cannot create/select a safe disposable workgroup, mark dependent cases `BLOCKED`.

Required evidence categories for this suite: before and after screenshots of team/workgroup surfaces, generated project/workgroup roots, `list-peers-lean` JSON, coordinator/member session screenshots, restart/refresh state snapshots, and residual workgroup directory notes.

## Execution Log

Date: TBD

Tester: TBD

App under test: TBD

Target window: TBD

Evidence root: TBD

Test data: TBD

| Case | Result | Evidence | Notes |
| --- | --- | --- | --- |
| WGP-001 | NOT RUN | TBD | Workgroup target readiness not executed in this run. |
| WGP-002 | NOT RUN | TBD | Team/workgroup create flow not executed in this run. |
| WGP-003 | NOT RUN | TBD | Coordinator/member session checks not executed in this run. |
| WGP-004 | NOT RUN | TBD | Peer list checks not executed in this run. |
| WGP-005 | NOT RUN | TBD | Workgroup isolation checks not executed in this run. |
| WGP-006 | NOT RUN | TBD | Refresh/restart checks not executed in this run. |

Residual test data:

- TBD during execution.

Automation gaps observed:

- TBD during execution.

### WGP-001: Disposable project is ready for workgroup checks

Purpose:

Verify that the testable app, disposable project, and evidence root are ready before team/workgroup state is created.

Preconditions:

- `agentscommander_testeable.exe` is available and the testable GUI is closed if reset is needed.
- The tester has an evidence root and a disposable project path such as `ac-regression-wgp-YYYYMMDD-HHMMSS`.
- No live project or workgroup is selected for this suite.

Steps:

1. Run `agentscommander_testeable.exe test-reset --confirm-testeable` if clean state is required, and save stdout/stderr.
2. Launch `agentscommander_testeable.exe --app` with deterministic placement.
3. Run `agentscommander_testeable.exe window-info` and save the JSON output.
4. Create or open the disposable project for this suite.
5. Capture the project/sidebar state before team/workgroup creation.
6. Record the disposable project root and planned team/workgroup names.

Expected Result:

The testable app identity is verified and a disposable project context is ready for safe team/workgroup operations.

Evidence Required:

- `WGP-001-reset.log` if reset was used.
- `WGP-001-window-info.json`.
- `WGP-001-disposable-project.png`.
- Notes with disposable project root and planned team/workgroup names.

Pass/Fail Criteria:

PASS if the testable target and disposable project are verified. PARTIAL if project setup succeeds but one optional reset artifact is absent. FAIL if a live project is selected or target identity is wrong. BLOCKED if no safe disposable project can be prepared.

### WGP-002: Create or select a workgroup from the GUI

Purpose:

Verify that the GUI can create or select a disposable team/workgroup and expose its generated state.

Preconditions:

- Depends on WGP-001.
- Disposable coordinator/member agent definitions are available or can be created safely without launching real model-backed sessions.
- The team/workgroup names are unique for this run.

Steps:

1. Capture the team/workgroup surface before creation.
2. Open the GUI flow for creating a team or activating a workgroup.
3. Select only disposable coordinator/member agents and harmless repo settings.
4. Enter a unique workgroup title or team/workgroup name for the current run.
5. Confirm creation/activation.
6. Capture the visible workgroup entry, team/project association, and generated state indicators.
7. Record any generated workgroup root path such as `.ac/wg-<N>-<team>/`.

Expected Result:

A disposable workgroup is created or selected through the GUI, appears under the expected project/team, and exposes enough identity to distinguish it from live workgroups.

Evidence Required:

- `WGP-002-before-create.png`.
- `WGP-002-create-flow.png`.
- `WGP-002-created-workgroup.png`.
- Optional directory listing or state snapshot showing the generated workgroup root.

Pass/Fail Criteria:

PASS if the disposable workgroup is visible and generated under the expected project/team. PARTIAL if creation succeeds but generated directory evidence is unavailable. FAIL if a live workgroup is mutated, the wrong team is used, or the UI creates duplicate ambiguous entries. BLOCKED if safe disposable team/agent fixtures do not exist.

### WGP-003: Coordinator and member sessions are distinguishable

Purpose:

Verify that coordinator and member roles are visually distinguishable in the disposable workgroup.

Preconditions:

- Depends on WGP-002.
- The disposable workgroup has at least one coordinator and one member replica.
- Starting sessions for these replicas is safe or they can be inspected without launch.

Steps:

1. Capture the workgroup participant list before starting any sessions.
2. Start or select the coordinator session only if the launcher is a harmless fixture.
3. Capture coordinator labels, badges, session row, or role indicators.
4. Start or select one member session only if safe.
5. Capture member labels, badges, session row, or role indicators.
6. Compare the captured coordinator and member surfaces for role distinction.

Expected Result:

The GUI lets a tester distinguish the coordinator from member replicas using labels, placement, badges, permissions, or session grouping.

Evidence Required:

- `WGP-003-participants-before.png`.
- `WGP-003-coordinator-session.png` or coordinator participant screenshot.
- `WGP-003-member-session.png` or member participant screenshot.
- Optional role/config snapshot if available.

Pass/Fail Criteria:

PASS if coordinator and member identities are visually distinct. PARTIAL if distinction exists only in one surface. FAIL if roles are indistinguishable or mislabeled. BLOCKED if safe coordinator/member fixtures cannot be created or inspected.

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
2. Close `Agents Commander [TESTEABLE]` normally.
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
