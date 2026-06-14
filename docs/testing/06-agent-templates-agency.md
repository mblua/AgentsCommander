# 06 Agent Templates And Agency

These cases validate the GUI surfaces that expose local agent templates, agency-provided templates, install or update prompts, template metadata, filtering, and launching an agent from a selected template.

Use clearly disposable test projects, workgroups, agents, templates, sessions, and settings state. Prefer creating test data in the tester's allowed scratch/evidence area. If no safe in-app cleanup exists, record residual state rather than deleting user data manually.

Visual preconditions from `README.md#visual-test-environment` apply to every case in this file. TPL-001 captures target-window identity explicitly; later cases inherit it.

Current deterministic mode: use `agentscommander_testeable.exe` with explicit placement and `window-info` verification. Run `agentscommander_testeable.exe test-reset --confirm-testeable` before cases that require clean disposable state, and only when the testable GUI is not active.

Use a harmless local/no-op template fixture only. Do not install, update, launch, or depend on paid, cloud-backed, account-backed, real model, Codex, Claude, Gemini, Telegram, or external Agency assets as part of this suite. If the UI only exposes real external templates, mark launch/install cases `BLOCKED` and capture the missing safe-fixture evidence.

Required evidence categories for this suite: before and after screenshots of template lists and detail views, selected template metadata, agency prompt/status screenshots, settings/template-path evidence, and session/status evidence only when a harmless template launch is available.

## Execution Log

Date: TBD

Tester: TBD

App under test: TBD

Target window: TBD

Evidence root: TBD

Test data: TBD

| Case | Result | Evidence | Notes |
| --- | --- | --- | --- |
| TPL-001 | NOT RUN | No evidence because NOT RUN. | Template entry point checks not executed in this run. |
| TPL-002 | NOT RUN | No evidence because NOT RUN. | Local template metadata checks not executed in this run. |
| TPL-003 | NOT RUN | No evidence because NOT RUN. | Agency install/update prompt checks not executed in this run. |
| TPL-004 | NOT RUN | No evidence because NOT RUN. | Template search/filter checks not executed in this run. |
| TPL-005 | NOT RUN | No evidence because NOT RUN. | Launch-from-template checks not executed in this run. |
| TPL-006 | NOT RUN | No evidence because NOT RUN. | Persistence after restart checks not executed in this run. |

Residual test data:

- None recorded; suite not executed yet.

Automation gaps observed:

- No automation gaps recorded; suite not executed yet.

### TPL-001: Template and agency entry points are visible

Purpose:

Verify that the GUI exposes the template and agency surfaces from the deterministic testable app without requiring a real external provider.

Preconditions:

- `agentscommander_testeable.exe` is launched and verified with `window-info`.
- A disposable project is selected.
- The tester has an evidence root for template screenshots and settings snapshots.
- No real Agency update or model-backed launch is required for this case.

Steps:

1. Capture the target app window and project/sidebar state.
2. Open the GUI entry point for creating an agent, such as the project header agent action.
3. Capture the New Agent or equivalent creation surface.
4. Identify visible template picker, local template, agency status, or agency setup entry points.
5. Open the agency or template status surface only far enough to show available options without starting an install/update.
6. Capture the visible labels, disabled states, prompts, or status messages.

Expected Result:

The GUI exposes a discoverable path to template-based agent creation and any agency setup/status surface, and the tester can identify whether local templates or agency templates are available.

Evidence Required:

- `TPL-001-window-info.json` proving the testable target.
- `TPL-001-create-entry.png` showing the create-agent entry point.
- `TPL-001-template-surface.png` showing template or agency options.
- Optional settings snapshot showing template path or agency status fields if available.

Pass/Fail Criteria:

PASS if template and agency entry points are visible and understandable. PARTIAL if only one entry point is visible but the absence of the other is captured. FAIL if the UI offers no discoverable template/agency path when it should. BLOCKED if the target app cannot be verified or the disposable project is unavailable.

### TPL-002: Local template metadata is readable

Purpose:

Verify that a harmless local template fixture displays enough metadata for a tester to understand what role would be created.

Preconditions:

- Depends on TPL-001.
- A harmless local/no-op template fixture exists in the configured local template path or can be selected through safe testable settings.
- The fixture does not require network access, paid services, or real model-backed execution.

Steps:

1. Capture the configured local template path or settings surface if visible.
2. Open the template picker from the create-agent flow.
3. Select the harmless local/no-op template fixture without creating an agent.
4. Capture the visible template name, description, category, source, accent color, role body preview, or other metadata exposed by the UI.
5. Capture any empty or missing metadata fields that would affect tester confidence.
6. Return to the template list without launching a session.

Expected Result:

The selected local template displays readable metadata and can be distinguished from agency-provided or unavailable templates.

Evidence Required:

- `TPL-002-template-list.png` showing the local template in the list.
- `TPL-002-template-detail.png` showing name, description, source, or role metadata.
- Optional `TPL-002-template-settings.png` showing the local template path.

Pass/Fail Criteria:

PASS if the harmless local template metadata is readable and source identity is clear. PARTIAL if metadata is readable but one expected field is absent. FAIL if the template appears but cannot be inspected or is mislabeled. BLOCKED if no harmless local/no-op template fixture exists.

### TPL-003: Agency install or update prompt is understandable

Purpose:

Verify that agency status messaging is clear without requiring an actual external install or update during this suite.

Preconditions:

- Depends on TPL-001.
- The tester can open the agency status or template picker surface in disposable testable state.
- The tester must not start a real external download, install, update, or paid/account-backed workflow.

Steps:

1. Open the template or agency status surface.
2. Capture the current agency state before interacting with any install/update control.
3. Identify whether the UI says agency templates are installed, available, unavailable, stale, or need update.
4. If an install/update control is present, capture it without activating it.
5. If the UI has a safe dry-run or disabled state, capture that state.
6. Close the surface without changing agency state.

Expected Result:

The UI clearly communicates the current agency state and any action available to the user without forcing a real external operation.

Evidence Required:

- `TPL-003-agency-status.png` showing agency status text or indicators.
- `TPL-003-agency-action.png` showing install/update controls if present.
- Notes stating whether an action was not executed because it would use external or account-backed resources.

Pass/Fail Criteria:

PASS if agency status is understandable and no real external action is required. PARTIAL if status is readable but action consequences are unclear. FAIL if the UI encourages an unsafe install/update without warning or state clarity. BLOCKED if agency status cannot be opened in disposable testable state.

### TPL-004: Template search or filtering narrows visible choices

Purpose:

Verify that the template picker search/filter behavior is testable when the UI exposes a safe filtering control.

Preconditions:

- Depends on TPL-001.
- The template picker shows at least two visible choices, or the absence of enough choices can be captured.
- A harmless search term such as the local fixture name is known.

Steps:

1. Capture the template list before filtering.
2. Locate the search, filter, category, or source control if one exists.
3. Enter or select a harmless filter value that should match the local/no-op fixture.
4. Capture the filtered result list.
5. Clear the filter and capture the restored result list.
6. If no filter control exists, capture the picker surface proving the control is absent.

Expected Result:

When a filter control exists, visible choices narrow to matching templates and restore when cleared. If no filter control exists, the evidence clearly supports a `BLOCKED` result for filtering coverage.

Evidence Required:

- `TPL-004-before-filter.png` showing the unfiltered list.
- `TPL-004-filtered.png` showing narrowed results or empty state.
- `TPL-004-filter-cleared.png` showing restored results.
- Or `TPL-004-no-filter-control.png` showing no available filter/search affordance.

Pass/Fail Criteria:

PASS if filtering narrows and restores choices correctly. PARTIAL if filtering works but empty-state messaging is incomplete. FAIL if filtering returns unrelated choices or does not clear correctly. BLOCKED if no safe filter/search control or sufficient fixture choices exist.

### TPL-005: Launching an agent from a selected template creates the expected session

Purpose:

Verify the end-to-end relationship between a selected harmless template and the created disposable agent/session.

Preconditions:

- Depends on TPL-002.
- A harmless local/no-op template fixture exists and is safe to launch without real model, external network, paid, or account-backed execution.
- The selected launch profile is a harmless local shell/no-op fixture, not Codex, Claude, Gemini, Telegram, or a real Agency workflow.

Steps:

1. Capture the selected template detail view before creation.
2. Enter a unique disposable agent name such as `agt-template-YYYYMMDD-HHMMSS`.
3. Confirm the harmless local/no-op launch profile.
4. Create the agent/session from the selected template.
5. Capture the resulting agent row, session row, and any generated role/template labels.
6. Capture the terminal/status area only after confirming it is the harmless fixture.
7. Record the generated agent root or session working directory if visible.

Expected Result:

The created disposable agent/session is visibly tied to the selected harmless template and does not start a real model-backed or external workflow.

Evidence Required:

- `TPL-005-selected-template.png` showing the template metadata before launch.
- `TPL-005-create-confirmation.png` showing the agent name and safe launch profile.
- `TPL-005-created-agent-session.png` showing the resulting agent/session.
- Optional generated role or session/status snapshot proving template linkage.

Pass/Fail Criteria:

PASS if a harmless template launch creates the expected disposable agent/session with clear template linkage. PARTIAL if the agent is created but one linkage artifact is unavailable. FAIL if the wrong template is used, a live provider starts, or generated identity is ambiguous. BLOCKED if no safe launchable local/no-op template fixture exists.

### TPL-006: Template/agency state after restart is predictable

Purpose:

Verify that template picker state, local template configuration, and agency status remain coherent after a normal testable app restart.

Preconditions:

- Depends on TPL-001 and TPL-002.
- The current testable identity has known local template or agency status evidence.
- The tester can close and relaunch only `Agents Commander [TESTEABLE]`.

Steps:

1. Capture the pre-restart template picker state and any local template path or agency status.
2. Close the target testable app normally.
3. Relaunch `agentscommander_testeable.exe --app` with deterministic placement.
4. Run `agentscommander_testeable.exe window-info` and capture the relaunched target.
5. Open the same template or agency surface.
6. Capture the post-restart template picker state and compare visible local template and agency status with the pre-restart evidence.

Expected Result:

After restart, local template visibility and agency status remain predictable, with no duplicate prompts, lost local fixture, or unsafe automatic external operation.

Evidence Required:

- `TPL-006-pre-restart-template-state.png`.
- `TPL-006-post-window-info.json`.
- `TPL-006-post-restart-template-state.png`.
- Optional settings/template JSON snapshot before and after restart.

Pass/Fail Criteria:

PASS if template and agency state after restart matches expected persisted behavior. PARTIAL if state is coherent but optional settings snapshots are unavailable. FAIL if local templates disappear unexpectedly, prompts duplicate, or external actions start automatically. BLOCKED if restart cannot be performed safely in the testable identity.
