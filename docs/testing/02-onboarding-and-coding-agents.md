# 02 Onboarding And Coding Agents

These cases validate the first-run and settings surfaces that let a user configure the coding agents AgentsCommander can launch. Run them before project, agent, team, or workgroup journeys that depend on a configured coding agent.

Use the deterministic testable app mode from `README.md#deterministic-testable-app`. Run `agentscommander_testeable.exe test-reset --confirm-testeable` before first-run cases that require clean disposable state, and only when the testable GUI is not active.

Product actions must be performed through the GUI. CLI is allowed only for harness control, semantic UI automation, screenshots, logs, and read-only verification.

## Execution Log

Date: 2026-06-13

Tester: ac-cli-and-gui-tester

App under test: `target\release\agentscommander_testeable.exe --app --ui-automation`

Evidence root: `C:\Users\maria\0_repos\AgentsCommander_ac\.ac\wg-14-acceptance-testing\__agent_ac-cli-and-gui-tester\evidence\ui-regression-baseline-20260613-191000`

Result summary:

- Onboarding completed with Codex after selecting Claude once and then Codex once.
- Direct Codex selection initially left `onboarding.confirm` disabled; preserve this as a regression/edge case if it reproduces.
- Final settings showed a Codex coding-agent row, but `onboardingDismissed = false`; the target later returned to first-run onboarding during the longer journey.
- OCA-001 currently verifies both configured-agent persistence and dismissed-onboarding persistence before it can pass.
- Rerun evidence from `ui-regression-baseline-rerun-20260613-202450` reproduced the baseline acceptance failure: Codex row persisted and the main UI opened, but `onboardingDismissed` remained `false` after onboarding and after relaunch.
- Product intent is tracked as GitHub issue #505. If `onboardingDismissed` is confirmed to mean only "user cancelled onboarding", adjust OCA-001/003/004/005 expectations instead of treating the setup path as a product bug.

Known automation support:

- First-run onboarding has semantic selectors for `onboarding.modal`, `onboarding.agentPreset.claude`, `onboarding.agentPreset.codex`, `onboarding.agentPreset.antigravity`, `onboarding.agentPreset.custom`, `onboarding.custom.label`, `onboarding.custom.command`, `onboarding.cancel`, `onboarding.confirm`, `onboarding.done`, and `onboarding.done.close`.
- Settings has semantic selectors for `actionBar.settings`, `settings.modal`, `settings.tab.agents`, `settings.agentPreset.<presetKey>`, `settings.agent.addCustom`, `settings.agentRow.<index>.*`, `settings.save`, and `settings.cancel`.

Known automation gaps:

- Native OS file/folder pickers are outside DOM-selector automation.
- If a configured coding-agent command points to a missing executable, the GUI may still allow saving; downstream launch behavior belongs in terminal/session cases.

## Control Inventory

First-run onboarding controls:

- Preset buttons: `Claude Code`, `Codex`, `Antigravity`, `Custom Agent`.
- Custom fields: agent name and command.
- Footer actions: `Cancel` (optional, supplied by the consumer), `Set up Coding Agent`, and done-state `Get started`.

Required clean-state slices:

- OCA-001 covers Codex as the default acceptance preset.
- OCA-002 covers Cancel and verifies the app remains usable with no coding agent configured.
- OCA-003 covers Claude Code.
- OCA-004 covers Antigravity.
- OCA-005 covers Custom Agent with valid fields.
- OCA-006 and OCA-007 cover the Coding Agents settings surface after onboarding.

Each slice must start from a reset testable config unless the case explicitly says it is using an existing settings state. Do not use a passing Codex run as proof that Cancel, Claude, Antigravity, or Custom Agent works.

Dismissal semantics note: until issue #505 is resolved, setup-path cases expect `onboardingDismissed = true` after `Get started` as a conservative acceptance contract. If product intent says the flag is cancel-only, replace that assertion with the intended persistence signal.

### OCA-001: First-run onboarding selects Codex

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

### OCA-002: First-run onboarding Cancel path

Purpose:

Verify that a clean first-run user can cancel coding-agent setup, reach the normal app UI, and keep onboarding dismissed without adding a coding agent.

Preconditions:

- The testable app config has been reset.
- `agentscommander_testeable.exe` is launched with `--app --ui-automation`.
- The onboarding dialog is visible.

Steps:

1. Wait for `onboarding.modal`.
2. Click `onboarding.cancel`.
3. Wait for `main.root` and `sidebar.root`.
4. Open settings and inspect the Coding Agents tab.
5. Close and relaunch the testable app.
6. Confirm first-run onboarding does not reappear.

Expected Result:

The onboarding dialog closes, the main app is usable, no coding agent row is added by the cancel action, `onboardingDismissed` is persisted as `true`, and onboarding does not reappear after relaunch.

Evidence Required:

- Screenshot of onboarding before Cancel.
- Semantic click result for `onboarding.cancel`.
- Screenshot or semantic result showing normal main/sidebar UI.
- Settings snapshot proving no preset row was created by Cancel and `onboardingDismissed = true`.
- Post-relaunch screenshot or semantic query proving onboarding did not reappear.

Pass/Fail Criteria:

Pass if Cancel dismisses onboarding persistently without adding an agent. Fail if onboarding reappears, the app is unusable after Cancel, or Cancel creates an unintended coding-agent row.

### OCA-003: First-run onboarding selects Claude Code

Purpose:

Verify that a clean first-run user can choose the Claude Code preset and persist the expected configured agent.

Preconditions:

- The testable app config has been reset.
- `agentscommander_testeable.exe` is launched with `--app --ui-automation`.
- The onboarding dialog is visible.

Steps:

1. Wait for `onboarding.modal`.
2. Select `Claude Code`.
3. Confirm the selection.
4. Wait for the done state.
5. Close the done dialog.
6. Open settings and inspect the Coding Agents tab.
7. Close and relaunch the testable app.
8. Confirm first-run onboarding does not reappear.

Expected Result:

Claude Code is configured with command `claude`, onboarding is dismissed persistently, and the app reaches normal UI after both completion and relaunch.

Evidence Required:

- Semantic query/click result for `onboarding.agentPreset.claude`.
- Screenshot of the selected preset and done state.
- Settings snapshot showing a Claude Code row with command `claude`.
- Settings or state snapshot proving `onboardingDismissed = true`.
- Post-relaunch screenshot or semantic query proving onboarding did not reappear.

Pass/Fail Criteria:

Pass if the Claude Code preset persists and onboarding is dismissed. Fail if the wrong agent is created, dismissal remains false, or onboarding reappears.

### OCA-004: First-run onboarding selects Antigravity

Purpose:

Verify that a clean first-run user can choose the Antigravity preset and persist the expected configured agent.

Preconditions:

- The testable app config has been reset.
- `agentscommander_testeable.exe` is launched with `--app --ui-automation`.
- The onboarding dialog is visible.

Steps:

1. Wait for `onboarding.modal`.
2. Select `Antigravity`.
3. Confirm the selection.
4. Wait for the done state.
5. Close the done dialog.
6. Open settings and inspect the Coding Agents tab.
7. Close and relaunch the testable app.
8. Confirm first-run onboarding does not reappear.

Expected Result:

Antigravity is configured with command `agy`, onboarding is dismissed persistently, and the app reaches normal UI after both completion and relaunch.

Evidence Required:

- Semantic query/click result for `onboarding.agentPreset.antigravity`.
- Screenshot of the selected preset and done state.
- Settings snapshot showing an Antigravity row with command `agy`.
- Settings or state snapshot proving `onboardingDismissed = true`.
- Post-relaunch screenshot or semantic query proving onboarding did not reappear.

Pass/Fail Criteria:

Pass if the Antigravity preset persists and onboarding is dismissed. Fail if the wrong agent is created, dismissal remains false, or onboarding reappears.

### OCA-005: First-run onboarding creates a custom coding agent

Purpose:

Verify that a clean first-run user can choose Custom Agent, enter a name and command, and persist that agent while dismissing onboarding.

Preconditions:

- The testable app config has been reset.
- `agentscommander_testeable.exe` is launched with `--app --ui-automation`.
- The onboarding dialog is visible.
- The test run has unique test data, for example label `Onboarding Custom Agent <timestamp>` and command `codex --help`.

Steps:

1. Wait for `onboarding.modal`.
2. Select `Custom Agent`.
3. Confirm that `onboarding.confirm` remains disabled while either required custom field is empty.
4. Fill `onboarding.custom.label`.
5. Fill `onboarding.custom.command`.
6. Confirm the selection.
7. Wait for the done state.
8. Close the done dialog.
9. Open settings and inspect the Coding Agents tab.
10. Close and relaunch the testable app.
11. Confirm first-run onboarding does not reappear.

Expected Result:

The custom coding-agent row persists with the provided label and command, onboarding is dismissed persistently, and the app reaches normal UI after relaunch.

Evidence Required:

- Semantic query/click result for `onboarding.agentPreset.custom`.
- Semantic set results for `onboarding.custom.label` and `onboarding.custom.command`.
- Semantic query showing disabled confirm before valid custom input and ready confirm after valid input.
- Screenshot of the selected custom preset and done state.
- Settings snapshot showing the custom row with the expected label and command.
- Settings or state snapshot proving `onboardingDismissed = true`.
- Post-relaunch screenshot or semantic query proving onboarding did not reappear.

Pass/Fail Criteria:

Pass if required-field gating works, the custom row persists, and onboarding is dismissed. Fail if incomplete custom input can be confirmed, the row is wrong or missing, dismissal remains false, or onboarding reappears.

### OCA-006: Coding Agents settings preserve preset configuration

Purpose:

Verify that a user can inspect and save the Coding Agents settings without losing the preset agent.

Preconditions:

- Depends on OCA-001 or an equivalent state with a configured preset.

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

### OCA-007: Add and save a custom coding agent

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
