# 03 Agent Lifecycle

These cases validate creation and basic visibility of project-level Agent Matrix entries through the GUI.

Use a disposable project created through `01-project-lifecycle.md` or a fresh disposable project created as part of the current run. Do not create agents in a user project unless the test request explicitly allows it.

Product actions must be performed through the GUI. CLI is allowed only for harness control, semantic UI automation, screenshots, logs, and read-only verification.

## Execution Log

Date: 2026-06-13

Tester: ac-cli-and-gui-tester

Evidence root: `C:\Users\maria\0_repos\AgentsCommander_ac\.ac\room-14-acceptance-testing\__agent_ac-cli-and-gui-tester\evidence\ui-regression-baseline-20260613-191000`

Result summary:

- Invalid-name checks for space and slash did not create bad `_agent_*` directories.
- An orchestrator agent and worker agent were created through the GUI.
- One worker attempt produced focus-contaminated screenshots and was retried after reacquiring the intended target.
- Agent creation is useful baseline evidence, but AGT-003 restart persistence is not passed because the longer run later returned to onboarding and final target state could not verify persisted project/agent UI.

Known automation support:

- The project panel is visible in the sidebar after project registration.
- The UI exposes project context menus and section context menus for `New Agent`, but these do not currently have stable semantic selectors.

Known automation gaps:

- Project rows, section headers, context menus, the New Agent modal, template rows, name/description fields, and created agent rows do not yet have stable `data-ac-testid` selectors.
- Until those hooks exist, this suite is repeatable as a human-style GUI script with screenshots and read-only filesystem verification, not fully repeatable through `ui-click` and `ui-set`.
- Screen-rectangle screenshots can be contaminated by foreground windows. Preserve bad evidence, reacquire the intended target HWND/PID, and rerun the affected visual step before counting it as passed.

### AGT-000: Invalid agent name cannot create an agent

Purpose:

Verify that the New Agent modal prevents invalid names before the happy-path creation case runs.

Preconditions:

- A disposable project is visible in the sidebar.
- Baseline `.ac/` contents for the project have been captured.

Steps:

1. Open New Agent from the project UI.
2. Enter an invalid name containing a space, for example `bad agent`.
3. Observe whether `Create` is disabled or an error is shown.
4. Enter an invalid name containing a slash, for example `bad/agent`.
5. Observe whether `Create` is disabled or an error is shown.
6. Cancel the modal.
7. Inspect the project `.ac/` directory.

Expected Result:

Invalid names cannot create `_agent_*` directories, and canceling the modal leaves the project unchanged.

Evidence Required:

- Screenshot of each invalid-name state.
- Screenshot after cancel.
- Read-only filesystem snapshot proving no new `_agent_*` directory was created.

Pass/Fail Criteria:

Pass if invalid names cannot create an agent and no filesystem mutation occurs. Fail if the UI creates an agent with an invalid name or mutates state after cancel.

### AGT-001: Open New Agent from a project

Purpose:

Verify that a user can reach the project-scoped New Agent modal from the loaded project.

Preconditions:

- A disposable project is visible in the sidebar.
- The project has no required cleanup-sensitive user data.

Steps:

1. Locate the disposable project entry in the sidebar.
2. Open the project context menu or the Agents section context menu.
3. Choose `New Agent`.
4. Confirm the `New Agent` modal appears.

Expected Result:

The project-scoped New Agent modal opens and is ready for template selection, name entry, and description entry.

Evidence Required:

- Screenshot of the project entry before opening the menu.
- Screenshot of the context menu showing `New Agent`.
- Screenshot of the New Agent modal.
- Notes identifying whether coordinates, keyboard navigation, or semantic automation were used.

Pass/Fail Criteria:

Pass if the modal opens from the project UI. Fail if the user cannot reach New Agent from the project. Partial if a context menu appears but cannot be captured reliably.

### AGT-002: Create a blank project agent

Purpose:

Verify that a user can create a blank Agent Matrix entry from the GUI.

Preconditions:

- Depends on AGT-001.
- Unique test agent name is available, for example `regression-orchestrator-<timestamp>`.

Steps:

1. In New Agent, choose `No template`.
2. Enter the unique agent name.
3. Enter a short description.
4. Click `Create`.
5. Wait for the project sidebar to refresh.
6. Confirm the new agent appears under the project Agents section.

Expected Result:

The app creates `.ac/_agent_<name>/`, writes role files, refreshes the sidebar, and shows the new agent in the project Agents section.

Evidence Required:

- Screenshot of the filled New Agent modal before create.
- Screenshot of the sidebar after refresh showing the new agent.
- Read-only filesystem snapshot showing `.ac/_agent_<name>/Role.md` exists.
- Any GUI error text if creation fails.

Pass/Fail Criteria:

Pass if the agent is created from the GUI and visible after refresh. Fail if creation errors, creates the wrong location, or does not appear in the sidebar.

### AGT-003: Agent visibility survives app restart

Purpose:

Verify that a project-level agent created through the GUI remains visible after app restart.

Preconditions:

- Depends on AGT-002.

Steps:

1. Capture the sidebar with the created agent visible.
2. Close the testable app normally.
3. Relaunch the testable app.
4. Wait for the disposable project to load.
5. Confirm the created agent is still listed.

Expected Result:

The created agent remains registered and visible after restart.

Evidence Required:

- Pre-close sidebar screenshot.
- Post-relaunch `window-info`.
- Post-relaunch sidebar screenshot.
- Read-only filesystem snapshot of the agent directory.

Pass/Fail Criteria:

Pass if the agent remains visible and points to the same path. Fail if the agent disappears or changes identity.
