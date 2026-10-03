# 02 Onboarding And Coding Agents

These cases validate the first-run and settings surfaces that let a user configure the coding agents AgentsCommander can launch. Run them before project, agent, team, or room journeys that depend on a configured coding agent.

Use the deterministic testable app mode from `README.md#deterministic-testable-app`. Run `agentscommander_testeable.exe test-reset --confirm-testeable` before first-run cases that require clean disposable state, and only when the testable GUI is not active.

For OCA-001 through OCA-008, product actions must use the GUI; CLI is allowed only for harness control, semantic UI automation, screenshots, logs, and read-only verification. Their existing permissions and fixture requirements remain in force. Native installer acceptance below uses a non-GUI runner in disposable environments and is exempt from this OCA-only GUI rule.

## Execution Log

Date: 2026-06-13

Tester: ac-cli-and-gui-tester

App under test: `target\release\agentscommander_testeable.exe --app --ui-automation`

Evidence root: `C:\Users\maria\0_repos\AgentsCommander_ac\.ac\room-14-acceptance-testing\__agent_ac-cli-and-gui-tester\evidence\ui-regression-baseline-20260613-191000`

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
- Catalog status selectors on both registration surfaces: `settings.catalog.loading`, `settings.catalog.empty`, `settings.catalog.error`, `settings.catalog.warning.<index>`, `settings.catalog.reload`, `onboarding.catalog.loading`, `onboarding.catalog.empty`, `onboarding.catalog.error`, `onboarding.catalog.warning.<index>`, and `onboarding.catalog.reload`.
- Welcome identity line (#2784, Welcome modal only): `onboarding.agentVendor.<presetKey>` reads `by <Vendor>`, derived from the text after the last `by ` of the catalog description, or the raw description when it has no `by `. It does not render on the flag-off surfaces (New Agent picker, Settings).

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

### OCA-008: Catalog status on both registration surfaces

Status: PENDING - not run. This case needs GUI interaction and was not executed in the documentation phase.

Purpose:

Verify that Settings > Coding Agents and the New Agent picker both show the persisted catalog's status and never substitute selectable embedded defaults.

Preconditions:

- A disposable testable identity with a disposable project. Back up the project's `.ac/coding-agents/agents.10.default.json` and `agents.50.personal.no-git.json` (when present) before preparing each fixture; restore those originals afterwards instead of deleting them.
- Fixtures are prepared only while the GUI is closed. Relaunch with `agentscommander_testeable.exe --app --ui-automation`.

Steps:

1. Valid empty: close the GUI, keep the persisted managed base in place, and replace `agents.50.personal.no-git.json` with a valid layer that tombstones every key in `agents.10.default.json` (the shipped keys are `claude`, `codex`, `hermes`, `cursor`, `pi`, `opencode`, `antigravity` and `grok`; every shipped row is `removable: true`): `{"schemaVersion":1,"agents":[{"key":"claude","remove":true},{"key":"codex","remove":true},{"key":"hermes","remove":true},{"key":"cursor","remove":true},{"key":"pi","remove":true},{"key":"opencode","remove":true},{"key":"antigravity","remove":true},{"key":"grok","remove":true}]}`. `muse` needs no tombstone: it stays an embedded row with support disabled and the read gate drops it on every read path, so the persisted base never publishes it. Launch and open Settings > Coding Agents. Confirm `settings.catalog.empty` reads `No catalog agents available`, `settings.catalog.reload` is offered, no preset cards are selectable, and the manual Custom Agent row is still usable.
2. Unavailable: close the GUI, move `agents.10.default.json` aside (do not delete it), and create an empty directory named `agents.10.default.json` in its place. Launch and inspect both Settings > Coding Agents and the New Agent picker. The base path is now not a readable regular file, so startup leaves it in place and publishes nothing. Confirm `settings.catalog.error` and `onboarding.catalog.error` read `Catalog unavailable`, show the path and reason, offer `Reload catalog`, and show no selectable bundled defaults.
3. Local warning with usable base: close the GUI, restore a valid base and add an `agents.50.personal.no-git.json` whose row carries an unknown field, launch, and inspect both surfaces. Confirm a warning renders its path and reason while base rows remain selectable and manual Custom Agent still works.
4. Reload recovery: fix `agents.50.personal.no-git.json`, click `Reload catalog` on each surface, and confirm the warning clears and rows become selectable again without an app restart.
5. Primary source switch: with two disposable projects, switch the primary project and confirm both surfaces clear any previously selected catalog preset, refetch, and show the new project's catalog; a stale preset cannot be confirmed after the switch.

Expected Result:

Both registration surfaces show the persisted catalog's empty, unavailable and warning states with path and reason, never offer embedded defaults, keep manual Custom Agent usable, recover through `Reload catalog`, and clear stale selections when the primary project changes.

Evidence Required:

- Screenshots and semantic query results for `settings.catalog.empty`, `settings.catalog.error`, `settings.catalog.warning.<index>`, `settings.catalog.reload`, `onboarding.catalog.error`, `onboarding.catalog.warning.<index>`, and `onboarding.catalog.reload` in each state.
- Settings snapshot or log lines showing each warning path and reason.
- Before/after byte copies or hashes of the fixture files proving the read-only surfaces did not change them.
- Proof the original fixtures were restored after the case.

Pass/Fail Criteria:

PASS if every state renders as described on both surfaces, Reload recovers without a restart, selection never survives a source change, and no read writes fixture bytes. FAIL if a state is silently replaced by embedded presets, a path or reason is missing, Reload cannot recover, or a stale preset can be confirmed. BLOCKED if the disposable fixture cannot be prepared or restored safely.

## Native installer acceptance

**Status: UNEXECUTED / BLOCKED for native runtime qualification on all three platforms.** This documentation phase ran no installers, product tests, GUI, screenshots or input automation. The user owns future Windows, macOS and Linux testing after opting in; this procedure does not authorize execution. Use disposable native OS environments and user profiles when execution is separately authorized. Never use a live profile for replacement/rerun scenarios.

[PR #2856](https://github.com/mblua/AgentsCommander/pull/2856) merged the default-off routes and closed #2800. It records catalog **6/6**, settings **4/4** and focused UI **92/92**; these are PR-recorded results, not reruns in this documentation phase. The accepted default-off scope changed the earlier landing prerequisites; merge and generic checks did not satisfy native acceptance. The PR explicitly reports no real installer/native/GUI/transport qualification. Documentation completion supplies no runtime PASS, cannot retroactively qualify unperformed P1 tests, and does not close #2787.

### Existing automated evidence and its limits

At source `d88299216ee22eb8aefef0802ec05eceb89eb8af`, [coding_agents_catalog.rs](../../src-tauri/src/config/coding_agents_catalog.rs) contains these six `scope_b_catalog_2800_` tests:

- `scope_b_catalog_2800_exact_32_cells`
- `scope_b_catalog_2800_non_install_fields_unchanged`
- `scope_b_catalog_2800_unknown_os_and_missing_override_use_default`
- `scope_b_catalog_2800_host_selector_matches_supported_cfg`
- `scope_b_catalog_2800_project_and_personal_composition_preserved`
- `scope_b_catalog_2800_malformed_project_and_muse_policy_preserved`

They check catalog selection/composition, including eight agents × three OS overrides plus eight defaults; they reuse existing 2736 regressions. They do not execute installers. Any future report using this family must select and pass all six, with zero failed/ignored, and retain the source SHA, exact command, selected/passed/failed/ignored counts and full logs.

The `agent_install_2736_` tests in [agent_update.rs](../../src-tauri/src/agent_update.rs) cover the generic runner using synthetic success/exit-3 commands, timeout, concurrency, events, containment, cleanup and payloads. They do not execute the eight shipped platform wrappers. Neither `agent_install_2787_catalog_` nor `agent_install_2787_wrapper_` exists at this source; do not invent a selector or passing count. Dedicated shipped-wrapper/native qualification remains missing. Any required product test or runner change needs separate authorized scope.

### Record 24 independent native cells

Every cell below is unexecuted: setup, executed command and runtime logs are missing. Record each as PASS, FAIL or BLOCKED with the specific missing prerequisite/evidence; an unavailable OS remains BLOCKED. Do not hide skips or substitute one OS, architecture, agent, generic test or catalog assertion for another cell.

| Catalog key / binary | Windows native | macOS native | Linux native |
|---|---|---|---|
| `claude` / `claude` | BLOCKED — unexecuted | BLOCKED — unexecuted | BLOCKED — unexecuted |
| `codex` / `codex` | BLOCKED — unexecuted | BLOCKED — unexecuted | BLOCKED — unexecuted |
| `hermes` / `hermes` | BLOCKED — unexecuted | BLOCKED — unexecuted | BLOCKED — unexecuted |
| `cursor` / `agent` | BLOCKED — unexecuted | BLOCKED — unexecuted | BLOCKED — unexecuted |
| `pi` / `pi` | BLOCKED — unexecuted | BLOCKED — unexecuted | BLOCKED — unexecuted |
| `opencode` / `opencode` | BLOCKED — unexecuted | BLOCKED — unexecuted | BLOCKED — unexecuted |
| `antigravity` / `agy` | BLOCKED — unexecuted | BLOCKED — unexecuted | BLOCKED — unexecuted |
| `grok` / `grok` | BLOCKED — unexecuted | BLOCKED — unexecuted | BLOCKED — unexecuted |

For each cell, retain source SHA, effective persisted command and catalog/override hashes; OS version/architecture; shell, PowerShell, curl, Node/npm and archive/checksum tool versions where applicable; prerequisite checks; profile and CWD (including a path with spaces); inherited/effective PATH; null stdin; start/end times, exit status and complete stdout/stderr. Record the chosen native runner invocation rather than inventing a shipped qualification harness. Require nonzero selected counts with zero hidden ignores for any automated claim.

1. Capture executable presence and AC presence before installation. Execute the exact selected persisted command through the shipped runner with null stdin and its 300-second bound. Retain command/output/status even on failure or timeout.
2. Locate the installed executable and independently run its **full path** with `--version`; retain exit/output and the actual path. Exit 0 from the installer or Welcome's Installed indicator alone cannot pass the cell.
3. Record AC presence in the same process, then restart AC from a fresh OS environment and record presence again. On Windows, a User PATH change does not refresh running AC. On Unix, account for existing user-bin directories and the once-cached login PATH. A terminal-only restart is insufficient evidence of a fresh AC environment.
4. Repeat with an existing install in the disposable profile and with a spaced CWD. Record replacement, profile/tool-store changes and recovery. Capture any prerequisite, PATH, timeout or rerun failure instead of reducing it to a successful exit.

### Missing shipped-wrapper and catalog scenarios

The following runtime/fixture scenarios are also **UNEXECUTED / BLOCKED**: no independently reviewed native artifact supplies their setup, exact invocation and logs. Future execution must preserve the actual shipped wrapper/runner; synthetic generic-runner commands cannot substitute for it.

| Scenario | Required evidence |
|---|---|
| Positive script and exact exit | Shipped-wrapper fixture executes a marker exactly once; capture positive exit 0 and explicit downloaded-script `exit 7`. Independently capture a native child exiting 7 and whether the upstream script propagates it. |
| Failed/empty fetch | Initial download failure and empty response return failure, with no execution marker; additionally check Windows whitespace-only rejection. Record fetch deadline and error/output; do not treat pipeline success as download success. |
| PowerShell propagation | Capture terminating and nonterminating script errors, ignored versus propagated native exits, fetch/write/cleanup failures, child status and raw stderr/CLIXML. Verify UTF-16LE/no-BOM outer payload and UTF-8/BOM temporary script separately. Do not assume every script error produces failure. |
| Hermes noninteractive flags | Prove the selected Windows `-NonInteractive` script argument or Unix `bash -s -- --non-interactive` reaches upstream, with null stdin and no setup prompt; record managed-tool and optional-component outcomes. |
| OpenCode Windows npm route | Record npm's `--allow-scripts` support, resolved package/Node engines, optional platform binary presence and independent version. Audit that only `opencode-ai` lifecycle is permitted; retain evidence of no global wildcard/config change or project approval write. |
| Pi prerequisites | Record existing compatible Node/npm; separately prove behavior when either is missing under null stdin. AC's npm route does not bootstrap them. |
| PATH, CWD and rerun | Retain the before/same-process/fresh-process presence and independent full-path version results, spaced-CWD run, existing-install rerun and upstream changes; account for Cursor Windows replacement. |
| Managed refresh and override preservation | Start with a verified old managed revision in disposable fixtures, preserve project 40/personal 50 hashes, then startup/register with the new revision. Verify local default/OS strings stay verbatim, omitted keys inherit, whole-object null clears, OS null falls back to composed default, new definitions validate, and Muse stays suppressed. |
| Edited/unmanaged and read-only reload | Compare bytes/hashes before and after startup/registration/read-only Reload: semantic edits/unmanaged bases retain bytes and warnings/old installers; Reload writes nothing. A formatting-only managed edit may refresh when its semantic hash matches and the shipped revision differs. |

For a PASS, require complete per-cell prerequisites, command/exit/timing/logs, independent version and AC-presence evidence plus applicable wrapper/refresh scenarios. FAIL records an observed mismatch; BLOCKED records missing setup or proof. CI is runtime evidence only when those exact tests actually execute with nonzero selection, no ignores and retained logs. Catalog/generic-runner checks alone never qualify these native cells.
