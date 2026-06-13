# 02 Onboarding And Coding Agents

These cases validate the first-run and settings surfaces that let a user configure the coding agents AgentsCommander can launch. Run them before project, agent, team, or workgroup journeys that depend on a configured coding agent.

Use the deterministic testable app mode from `README.md#deterministic-testable-app`. Run `agentscommander_testeable.exe test-reset --confirm-testeable` before first-run cases that require clean disposable state, and only when the testable GUI is not active.

Product actions must be performed through the GUI. CLI is allowed only for harness control, semantic UI automation, screenshots, logs, and read-only verification.

## Execution Log

Date: 2026-06-13

Tester: ac-cli-and-gui-tester

App under test: `src-tauri\target\release\agentscommander_testeable.exe --app --ui-automation`

Evidence root: `C:\Users\maria\0_repos\AgentsCommander_ac\.ac\wg-14-acceptance-testing\__agent_ac-cli-and-gui-tester\evidence\ui-regression-baseline-20260613-191000`

Result summary:

- Onboarding completed with Codex after selecting Claude once and then Codex once.
- Direct Codex selection initially left `onboarding.confirm` disabled; preserve this as a regression/edge case if it reproduces.
- Final settings showed a Codex coding-agent row, but `onboardingDismissed = false`; the target later returned to first-run onboarding during the longer journey.
- SET-001 must verify both configured-agent persistence and dismissed-onboarding persistence before it can pass.

Known automation support:

- First-run onboarding has semantic selectors for `onboarding.modal`, `onboarding.agentPreset.claude`, `onboarding.agentPreset.codex`, `onboarding.agentPreset.gemini`, `onboarding.confirm`, `onboarding.done`, and `onboarding.done.close`.
- Settings has semantic selectors for `actionBar.settings`, `settings.modal`, `settings.tab.agents`, `settings.agentPreset.<presetKey>`, `settings.agent.addCustom`, `settings.agentRow.<index>.*`, `settings.save`, and `settings.cancel`.

Known automation gaps:

- Native OS file/folder pickers are outside DOM-selector automation.
- If a configured coding-agent command points to a missing executable, the GUI may still allow saving; downstream launch behavior belongs in terminal/session cases.

### SET-001: First-run onboarding selects Codex

Purpose:

Verify that a clean first-run user can choose Codex as the coding-agent preset and dismiss onboarding.

Preconditions:

- The testable app config has been reset.
- `agentscommander_testeable.exe` is launched with `--app --ui-automation`.
- The onboarding dialog is visible.

Steps:

1. Wait for `onboarding.modal`.
2. Select `Codex`.
3. Confirm the selection. If `onboarding.confirm` remains disabled, select another preset once, reselect Codex, and preserve the initial disabled state as evidence.
4. Wait for the done state.
5. Close the done dialog.
6. Open settings and inspect the Coding Agents tab.
7. Close and relaunch the testable app.
8. Confirm first-run onboarding does not reappear.

Expected Result:

Codex is configured as a coding agent, onboarding is dismissed persistently, and the app reaches the normal main/sidebar UI after both initial completion and relaunch.

Evidence Required:

- `window-info` JSON for the target app instance.
- Screenshot of onboarding before selection.
- Semantic query result for the Codex preset.
- Screenshot or semantic result after the done dialog closes.
- Settings snapshot showing the Codex row.
- Settings or state snapshot proving onboarding dismissal is persisted.
- Post-relaunch screenshot or semantic query proving onboarding did not reappear.

Pass/Fail Criteria:

Pass if onboarding completes through the GUI, Codex appears in Coding Agents settings, dismissal is persisted, and onboarding does not reappear after relaunch. Fail if onboarding cannot be completed, settings do not persist the preset, dismissal remains false, or the app does not reach normal UI. Partial if the flow completes but one transient state cannot be captured.

### SET-002: Coding Agents settings preserve preset configuration

Purpose:

Verify that a user can inspect and save the Coding Agents settings without losing the preset agent.

Preconditions:

- Depends on SET-001 or an equivalent state with a configured preset.

Steps:

1. Open settings from the action bar.
2. Switch to the Coding Agents tab.
3. Inspect the existing Codex row.
4. Save without changes.
5. Reopen settings and inspect the same row again.

Expected Result:

The Codex row remains visible and unchanged after saving and reopening settings.

Evidence Required:

- Semantic query results for `settings.modal`, `settings.tab.agents`, `settings.agentRow.0`, and `settings.agentPreset.codex`.
- Screenshot of the Coding Agents tab before and after save/reopen.

Pass/Fail Criteria:

Pass if the preset row is stable. Fail if saving removes, duplicates, or corrupts the row.

### SET-003: Add and save a custom coding agent

Purpose:

Verify that a user can add a custom coding-agent entry from settings.

Preconditions:

- Settings opens successfully.
- The test run has unique test data, for example label `Regression Custom Agent <timestamp>`.

Steps:

1. Open settings.
2. Switch to the Coding Agents tab.
3. Click add custom.
4. Fill label, command, and color.
5. Save settings.
6. Reopen settings and confirm the custom row remains.

Expected Result:

The custom coding-agent row persists after saving and reopening settings.

Evidence Required:

- Semantic query result for `settings.agent.addCustom`.
- Semantic query results for the new row fields.
- Screenshot before save and after reopen.
- Read-only settings snapshot if needed to verify persistence.

Pass/Fail Criteria:

Pass if the custom row persists with the expected label, command, and color. Fail if it disappears, duplicates, or blocks later app use.
