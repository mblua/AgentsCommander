# 05 End-To-End User Journey

This suite stitches the earlier functional suites into a single user-style path. It is intended for baseline seeding and future selective reruns when a change affects first-run setup, project registration, agent creation, team creation, workgroup activation, or session launch.

Run this only in a disposable testable app identity and a disposable project. Product actions must be performed through the GUI. CLI is allowed only for harness control, semantic UI automation, screenshots, logs, and read-only verification.

Because several project panel and entity modal surfaces do not yet have stable semantic selectors, this suite currently produces a human-style acceptance baseline and an automation-gap report. It should not be claimed as fully semantically repeatable until the gaps are closed.

## Execution Log

Date: 2026-06-13

Tester: ac-cli-and-gui-tester

Evidence root: `C:\Users\maria\0_repos\AgentsCommander_ac\.ac\wg-14-acceptance-testing\__agent_ac-cli-and-gui-tester\evidence\ui-regression-baseline-20260613-191000`

Result: PARTIAL. This run seeds useful baseline evidence for onboarding, project creation, and agent creation, but it is not a baseline PASS.

Summary:

- Harness reset/window gate passed.
- Onboarding passed only after preset reselection and later exposed `onboardingDismissed = false`.
- Project creation loaded directly from an empty selected folder; no create-confirm modal appeared in the clean no-Project-AC-Root path.
- Two project agents were created through the GUI after one focus-contaminated attempt.
- Team creation was blocked before valid team/member completion.
- Workgroup creation and final full-state restart persistence were not reached.
- `screenshots\41-new-team-real-step2.png` is preserved as invalid New Team evidence: target metadata matched the testable app, but visible content was first-run onboarding and the screenshot was partly obscured by a foreground terminal.

### E2E-001: Clean user creates a project team workgroup

Purpose:

Validate the core new-user path from clean app launch to an active workgroup.

Preconditions:

- `agentscommander_testeable.exe test-reset --confirm-testeable` has succeeded.
- The testable app is launched with `--app --ui-automation` and deterministic placement.
- Unique disposable names are prepared for project, agents, team, and task title.
- Any external coding-agent executables used by the run are either available or the launch step is explicitly treated as a bounded downstream check.

Steps:

1. Complete first-run onboarding and select Codex or another configured coding agent.
2. Create a new disposable project from the UI.
3. Confirm the project appears in the sidebar.
4. Create two project agents from the UI.
5. Confirm both agents appear under Agents.
6. Create a team from the UI using those agents.
7. Mark one agent as coordinator.
8. Create or activate a workgroup from that team through the UI.
9. Confirm the workgroup appears in the sidebar with replica entries.
10. Launch or focus the coordinator session from the workgroup.
11. Close and relaunch the app.
12. Confirm the project, agents, team, and workgroup remain visible.

Expected Result:

A clean user can configure a coding agent, create a project, create agents, compose a team, activate a workgroup, and return to the same state after restart.

Evidence Required:

- `window-info` before interaction and after relaunch.
- Screenshot for each major UI stage: onboarding, project created, agents created, team created, workgroup created, coordinator launched, post-restart state.
- Semantic `ui-query` or `ui-wait` results for every available selector used.
- Read-only filesystem snapshots for the disposable project `.ac/`, agent directories, team config, workgroup directory, `TASK.md`, and messaging directory.
- A selector gap report listing each action that required coordinates, keyboard navigation, native picker automation, or visual-only confirmation.
- Negative diagnostic evidence for canceled project creation, invalid agent names, incomplete team creation, and incomplete workgroup creation.

Pass/Fail Criteria:

Pass if all user-visible milestones and mandatory negative diagnostics succeed through the GUI and persistence holds after restart. Partial if the user journey succeeds but one or more states remain visual-only due to missing selectors. Fail if any core product milestone cannot be completed through the GUI or a negative diagnostic mutates state unexpectedly.

Current baseline note:

As of the 2026-06-13 run, E2E-001 must not be reported as PASS until onboarding dismissal persistence, team creation, workgroup creation, final restart persistence, and unobscured target-window evidence are all verified.

### E2E-002: Rerun only affected journey segment

Purpose:

Provide a repeatable pattern for future changes that affect only one part of the journey.

Preconditions:

- A prior full E2E-001 run exists with evidence.
- The change under test identifies the affected surface.

Steps:

1. Map the change to one or more earlier suites:
   - Onboarding or coding agents: rerun `02-onboarding-and-coding-agents.md`.
   - Project registration: rerun the relevant `PRJ-###` cases.
   - Agent creation: rerun the relevant `AGT-###` cases.
   - Team or workgroup activation: rerun the relevant `WGP-###` cases.
   - Cross-surface persistence: rerun E2E-001 from the first affected milestone forward.
2. Start from clean disposable runtime state unless the contract explicitly requires continuity.
3. Preserve prior evidence read-only and create a new evidence directory for the rerun.
4. Record whether prior failures or automation gaps still reproduce.

Expected Result:

Future acceptance can rerun only the relevant slice while preserving enough evidence to compare against the baseline.

Evidence Required:

- Link or path to the prior baseline evidence.
- New evidence directory.
- Change-to-case mapping notes.
- PASS/FAIL/PARTIAL decision for the rerun slice.

Pass/Fail Criteria:

Pass if the affected slice validates the change without regressing the baseline assumptions. Fail if the slice breaks or invalidates downstream assumptions.

### E2E-003: Selector gap report remains actionable

Purpose:

Ensure every UI action that could not be performed semantically is recorded as an actionable automation gap.

Preconditions:

- E2E-001 or a functional slice rerun has completed.

Steps:

1. Review every product action performed during the run.
2. For each action, classify the action mechanism:
   - semantic `ui-*` selector;
   - keyboard navigation;
   - native picker automation;
   - coordinate/visual click;
   - read-only verification only.
3. For each non-semantic product action, record the missing selector/action family.
4. Cross-check gaps against `semantic-ui-automation-affordance-matrix.md`.

Expected Result:

The run produces a clear list of missing selectors/actions that can be used to improve future repeatability.

Evidence Required:

- Selector/action gap report.
- References to screenshots or logs for each non-semantic action.
- Proposed selector family when obvious.

Pass/Fail Criteria:

Pass if all non-semantic actions are accounted for. Fail if the report claims semantic repeatability while product actions still required untracked visual or coordinate fallback.
