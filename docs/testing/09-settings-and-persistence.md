# 09 Settings And Persistence

These cases validate settings surfaces and durable state across app restart, including harmless setting changes, project/room registration, window geometry, test reset boundaries, and recovery from invalid or missing disposable settings.

Use clearly disposable test projects, rooms, settings values, sessions, and app identity state. Prefer creating test data in the tester's allowed scratch/evidence area. If no safe in-app cleanup exists, record residual state rather than deleting user data manually.

Visual preconditions from `README.md#visual-test-environment` apply to every case in this file. SET-001 captures target-window identity explicitly; later cases inherit it.

Current deterministic mode: use `agentscommander_testeable.exe` with explicit placement and `window-info` verification. Run `agentscommander_testeable.exe test-reset --confirm-testeable` before cases that require clean disposable state, and only when the testable GUI is not active.

Use only a fresh disposable/release `agentscommander_testeable.exe` identity. Do not manually delete or hand-edit files outside `.agentscommander_testeable` and `agentscommander_testeable`. The GUI must be closed before `test-reset --confirm-testeable`. If there is no documented safe method to create invalid settings, `SET-006` must be marked `BLOCKED` during execution.

Required evidence categories for this suite: before and after screenshots, settings JSON snapshots if available, before and after directory listings for reset, `window-info` or process evidence proving the GUI is closed before reset, and restart evidence for persistence checks.

## Execution Log

Date: TBD

Tester: TBD

App under test: TBD

Target window: TBD

Evidence root: TBD

Test data: TBD

| Case | Result | Evidence | Notes |
| --- | --- | --- | --- |
| SET-001 | NOT RUN | No evidence because NOT RUN. | Settings target readiness not executed in this run. |
| SET-002 | NOT RUN | No evidence because NOT RUN. | Settings save checks not executed in this run. |
| SET-003 | NOT RUN | No evidence because NOT RUN. | Project/room persistence checks not executed in this run. |
| SET-004 | NOT RUN | No evidence because NOT RUN. | Window geometry persistence checks not executed in this run. |
| SET-005 | NOT RUN | No evidence because NOT RUN. | Test reset boundary checks not executed in this run. |
| SET-006 | NOT RUN | No evidence because NOT RUN. | Invalid/missing settings recovery not executed in this run. |
| SET-007 | NOT RUN | No evidence because NOT RUN. | Dual-path six-field raw inspection not executed in this run. |
| SET-008 | NOT RUN | No evidence because NOT RUN. | Legacy/new/mixed schema loading not executed in this run. |
| SET-009 | NOT RUN | No evidence because NOT RUN. | Malformed companion retention not executed in this run. |
| SET-010 | NOT RUN | No evidence because NOT RUN. | Failed-write byte preservation not executed in this run. |
| SET-011 | NOT RUN | No evidence because NOT RUN. | CLI/GUI interleaving pair preservation not executed in this run. |

Residual test data:

- None recorded; suite not executed yet.

Automation gaps observed:

- No automation gaps recorded; suite not executed yet.

### SET-001: Settings surface opens from the target app

Purpose:

Verify that the settings UI opens from the deterministic testable app and can be identified as belonging to that target.

Preconditions:

- `agentscommander_testeable.exe` is launched with deterministic placement.
- `window-info` confirms the target process path and title.
- The tester has an evidence root for screenshots and settings snapshots.

Steps:

1. Run `agentscommander_testeable.exe window-info` and save the JSON output.
2. Capture the target app before opening settings.
3. Open the settings entry point from the target app.
4. Capture the settings surface and visible active tab.
5. Record whether the settings surface is modal, embedded, or separate window.
6. Close settings without changing values.

Expected Result:

The settings surface opens from `AC [TESTEABLE]`, is visually readable, and can be closed without mutating settings.

Evidence Required:

- `SET-001-window-info.json`.
- `SET-001-before-settings.png`.
- `SET-001-settings-open.png`.
- Optional `SET-001-settings-closed.png`.

Pass/Fail Criteria:

PASS if settings opens and closes cleanly on the testable target. PARTIAL if settings opens but one capture is incomplete. FAIL if the settings surface belongs to a different app identity or cannot be closed. BLOCKED if the target app cannot be verified.

### SET-002: Harmless settings change saves and reloads

Purpose:

Verify that a low-risk setting change saves and persists across app restart without affecting live user state.

Preconditions:

- Depends on SET-001.
- A harmless setting is chosen, such as a visual preference or disposable local path, that does not start services, send messages, or invoke account-backed providers.
- The tester can capture before and after settings state.

Steps:

1. Capture the settings surface and current value before change.
2. If available, save a settings JSON snapshot for the testable identity.
3. Change one harmless setting to a known alternate value.
4. Save or apply the setting through the GUI.
5. Capture the post-save settings surface and any visible feedback.
6. Close and relaunch `agentscommander_testeable.exe --app`.
7. Open settings again and capture the persisted value.
8. Record any residual changed value for later cleanup/reset.

Expected Result:

The harmless setting persists after restart and no unrelated settings, projects, or live identity state are changed.

Evidence Required:

- `SET-002-before-change.png`.
- `SET-002-after-save.png`.
- `SET-002-post-restart.png`.
- Optional before/after settings JSON snapshots.

Pass/Fail Criteria:

PASS if only the chosen harmless setting changes and persists after restart. PARTIAL if persistence is visible but JSON snapshots are unavailable. FAIL if the setting does not persist, unrelated settings change, or a live identity is affected. BLOCKED if no safe harmless setting can be changed.

### SET-003: Project and room registrations persist across restart

Purpose:

Verify that disposable project/room registrations remain coherent after a normal testable app restart.

Preconditions:

- Depends on SET-001.
- A disposable project and optional disposable room have been created or opened in the testable identity.
- No live project registration is being modified for this case.

Steps:

1. Capture the project/room list before restart.
2. Save any available settings or project registration snapshot for the testable identity.
3. Close `AC [TESTEABLE]` normally.
4. Relaunch `agentscommander_testeable.exe --app` with deterministic placement.
5. Run `agentscommander_testeable.exe window-info` and save the output.
6. Capture the project/room list after restart.
7. Compare registration count, visible names, selected project/room, and duplicate state.

Expected Result:

The disposable project/room registration persists or restores according to documented behavior without duplicates or live-state bleed.

Evidence Required:

- `SET-003-pre-restart-projects.png`.
- Optional `SET-003-pre-restart-settings.json`.
- `SET-003-post-window-info.json`.
- `SET-003-post-restart-projects.png`.
- Optional `SET-003-post-restart-settings.json`.

Pass/Fail Criteria:

PASS if registration state after restart is coherent and duplicate-free. PARTIAL if UI state is correct but one optional JSON snapshot is unavailable. FAIL if registrations disappear unexpectedly, duplicate, or include unintended live projects. BLOCKED if no disposable project/room registration exists.

### SET-004: Window geometry persistence is observable

Purpose:

Verify that window placement or geometry behavior is observable and documented using `window-info`.

Preconditions:

- Depends on SET-001.
- The tester can launch with explicit placement flags or `AC_TEST_WINDOW_PLACEMENT`.
- Geometry changes are limited to the testable app window.

Steps:

1. Launch or relaunch `agentscommander_testeable.exe --app` with explicit placement flags or `AC_TEST_WINDOW_PLACEMENT`.
2. Run `agentscommander_testeable.exe window-info` and save baseline geometry.
3. Capture the target window at baseline geometry.
4. Move or resize the testable app only if the case is checking manual geometry persistence.
5. Run `window-info` again and save changed geometry.
6. Restart the testable app.
7. Run `window-info` after restart and capture the target window.
8. Compare observed geometry against documented placement or persistence behavior.

Expected Result:

Window geometry is measurable before and after restart, and the observed placement follows the documented testable-app placement or persistence rules.

Evidence Required:

- `SET-004-baseline-window-info.json`.
- `SET-004-baseline-window.png`.
- `SET-004-changed-window-info.json` if manual movement/resizing is used.
- `SET-004-post-restart-window-info.json`.
- `SET-004-post-restart-window.png`.

Pass/Fail Criteria:

PASS if geometry behavior matches the documented rule and is supported by `window-info`. PARTIAL if the app is usable but one geometry artifact is incomplete. FAIL if geometry is unpredictable, reported for the wrong process, or contradicts placement flags. BLOCKED if geometry cannot be changed or measured safely.

### SET-005: Test reset removes only disposable testable identity state

Purpose:

Verify that `test-reset --confirm-testeable` is used only while the GUI is closed and removes only documented disposable testable identity paths.

Preconditions:

- The testable GUI is closed.
- The tester can prove no `AC [TESTEABLE]` window is active.
- Disposable testable identity directories exist or their absence can be recorded.
- The tester will not manually delete or edit settings outside `.agentscommander_testeable` and `agentscommander_testeable`.

Steps:

1. Capture process/window evidence showing the testable GUI is not active.
2. Capture a before directory listing for the executable directory, including `.agentscommander_testeable` and `agentscommander_testeable` if present.
3. Run `agentscommander_testeable.exe test-reset --confirm-testeable`.
4. Save stdout/stderr, including planned-delete and final-result JSON lines.
5. Capture an after directory listing for the same executable directory.
6. Confirm no paths outside `.agentscommander_testeable` and `agentscommander_testeable` were deleted.
7. Relaunch the testable app and capture fresh identity startup if needed.

Expected Result:

The reset command refuses unsafe active-GUI conditions and, when allowed, deletes only the documented disposable sibling paths.

Evidence Required:

- `SET-005-gui-closed-evidence.txt` or screenshot/window enumeration.
- `SET-005-before-directory-listing.txt`.
- `SET-005-test-reset.log`.
- `SET-005-after-directory-listing.txt`.
- Optional `SET-005-post-reset-launch.png`.

Pass/Fail Criteria:

PASS if reset runs only with GUI closed and only documented disposable paths are affected. PARTIAL if deletion behavior is correct but optional relaunch evidence is missing. FAIL if reset runs against an active GUI, deletes undocumented paths, or lacks structured output. BLOCKED if the tester cannot prove the GUI is closed or cannot inspect the executable directory safely.

### SET-006: Missing or invalid disposable settings recover safely

Purpose:

Document safe recovery behavior for missing or invalid settings only when a documented safe method exists for the disposable testable identity.

Preconditions:

- The testable GUI is closed.
- The test is limited to `.agentscommander_testeable` or `agentscommander_testeable` under the testable executable directory.
- A documented safe method exists to create missing or invalid disposable settings without hand-editing live user state.
- If no documented safe method exists, this case must be marked `BLOCKED` during execution.

Steps:

1. Capture process/window evidence showing the testable GUI is not active.
2. Capture a baseline directory listing and settings snapshot for the disposable testable identity.
3. Apply only the documented safe method for missing or invalid disposable settings.
4. Save command stdout/stderr or file-state evidence from that documented method.
5. Launch `agentscommander_testeable.exe --app`.
6. Capture startup behavior, recovery prompts, settings defaults, or structured errors.
7. Close the app and restore clean disposable state using `test-reset --confirm-testeable` if required by the documented method.

Expected Result:

When a documented safe invalid-settings method exists, the testable app recovers safely or reports a clear error without touching live state. When no safe method exists, the correct result is `BLOCKED` with evidence of the missing method.

Evidence Required:

- `SET-006-safe-method-reference.md` or notes identifying the documented safe method.
- `SET-006-before-state.txt` or settings snapshot.
- `SET-006-invalid-settings-command.log` if a safe command exists.
- `SET-006-startup-recovery.png` or structured error output.
- `SET-006-reset-after-test.log` if cleanup is needed.

Pass/Fail Criteria:

PASS if a documented safe method is used and recovery/error behavior is clear and limited to disposable identity. PARTIAL if recovery is safe but one optional cleanup artifact is missing. FAIL if the app corrupts state, touches live settings, or fails without clear error. BLOCKED if no documented safe invalid-settings method exists.

### SET-007: Dual-path project fields are written and aligned

Purpose:

Verify that registering active and archived projects writes all six project fields in `settings.json` with correctly aligned companion arrays, portable companions in `/`-separated form, and a `null` companion for a cross-drive/share project.

Preconditions:

- Depends on SET-001.
- A disposable testable identity with at least one disposable AC project on the same drive/share as the binary, and, if available, one disposable project on a different drive or UNC share.
- Read-only inspection of the disposable `settings.json` is available.

Steps:

1. Register one disposable project on the binary's drive/share through the UI or CLI.
2. If a second drive/share is available, register a disposable project located there.
3. Archive one registered project so the archived arrays are populated.
4. Capture a `settings.json` snapshot and inspect the six fields: `projectPath`, `projectPathRelativeToInstance`, `projectPaths`, `projectPathsRelativeToInstance`, `archivedProjectPaths`, `archivedProjectPathsRelativeToInstance`.
5. Confirm each companion array has the same length and order as its absolute array, companion strings use `/` separators, and the cross-drive/share project (if present) has a `null` companion while keeping its absolute path.

Expected Result:

All six fields are present, companion arrays are index-aligned with their absolute arrays, portable companions are `/`-separated, and a cross-drive/share entry stores `null` and remains absolute-only.

Evidence Required:

- `SET-007-settings-snapshot.json` showing the six fields.
- Notes mapping each companion slot to its absolute entry, including the `null` cross-drive/share slot when tested.

Pass/Fail Criteria:

PASS if all six fields are present and aligned and the `null` cross-drive/share case is correct (or noted as untested when no second drive/share exists). PARTIAL if fields are correct but the cross-drive/share case could not be exercised. FAIL if arrays are misaligned, a companion is absolute or backslash-separated, or a same-drive project stores `null`. BLOCKED if `settings.json` cannot be inspected.

### SET-008: Legacy, new, and mixed schemas all load

Purpose:

Verify that a legacy absolute-only `settings.json`, a fully paired new-schema file, and a mixed file (some entries paired, some companion-absent) all load their projects, and that a legacy entry gains a companion only after a validating operation.

Preconditions:

- The testable GUI is closed.
- The test edits only the disposable testable identity's `settings.json` under `.agentscommander_testeable`.
- Two or more disposable AC projects exist on disk for registration in each variant.

Steps:

1. Prepare variant A (legacy): `settings.json` with `projectPaths` populated and no companion fields at all.
2. Launch the testable app, confirm the projects load, then close it. Confirm the file still has no companion fields (loading alone does not migrate).
3. Trigger a validating operation (register or archive a project through the UI/CLI) and confirm a companion is then added for the reconciled entries.
4. Prepare variant B (new): `settings.json` with fully aligned companion arrays. Launch, confirm the projects load, close.
5. Prepare variant C (mixed): `projectPaths` with an aligned companion array where one slot is `null` and one legacy singular-only carrier is present. Launch, confirm every valid entry loads and no entry is dropped.

Expected Result:

Each schema variant loads its valid projects; a legacy file gains companions only after a validating operation, not from a plain load.

Evidence Required:

- The three input `settings.json` variants (or diffs) captured before launch.
- Post-launch sidebar screenshots for each variant.
- Post-operation `settings.json` snapshot for variant A showing companions added only after the validating operation.

Pass/Fail Criteria:

PASS if all three variants load correctly and legacy migration happens only after a validating operation. PARTIAL if loading is correct but one migration snapshot is missing. FAIL if any valid project fails to load, a plain load rewrites a legacy file, or a mixed entry is dropped. BLOCKED if the disposable `settings.json` cannot be prepared safely.

### SET-009: Misaligned companion is retained, not normalized

Purpose:

Verify that a structurally malformed project pair (a companion array whose length does not match its absolute array) is preserved on disk rather than silently normalized, is reported instead of loaded, and blocks project mutation while unrelated settings saves still succeed.

Preconditions:

- The testable GUI is closed.
- The test edits only the disposable testable identity's `settings.json` under `.agentscommander_testeable`.
- A documented safe method exists to create the malformed disposable file. If not, mark this case `BLOCKED`.

Steps:

1. Capture a baseline `settings.json` snapshot and its byte size/mtime.
2. Edit the disposable file so `projectPathsRelativeToInstance` has a different length than `projectPaths` (a misaligned companion).
3. Launch the testable app.
4. Confirm the malformed list produces no loaded projects from that list and the app reports the malformed condition rather than crashing.
5. Without triggering a project mutation, capture the `settings.json` bytes/mtime and confirm they are unchanged (no auto-normalization write).
6. Change one unrelated harmless setting and save; confirm the save succeeds and the malformed project fields are still present verbatim.
7. Attempt a project mutation (open, archive, or remove); confirm it is refused while the malformed field is present.

Expected Result:

The malformed companion is retained byte-for-byte on load, reported rather than loaded, and blocks project mutation, while an unrelated settings save still succeeds and leaves the malformed fields intact.

Evidence Required:

- `SET-009-before.json` and `SET-009-after-load.json` (or a byte/mtime comparison) proving no normalization write.
- Structured error or log evidence of the malformed report.
- `SET-009-after-unrelated-save.json` showing the malformed fields preserved after a harmless save.
- Evidence that a project mutation was refused.

Pass/Fail Criteria:

PASS if the malformed pair is preserved, reported, and blocks mutation while unrelated saves succeed. PARTIAL if behavior is correct but one artifact is missing. FAIL if the file is normalized/overwritten, the malformed entry loads, a project mutation proceeds, or an unrelated save is blocked. BLOCKED if no safe method exists to create the malformed disposable file.

### SET-010: Failed settings write leaves the file intact

Purpose:

Verify that when a settings write cannot complete, the on-disk `settings.json` is left unchanged, the app still uses the validated selected project paths for the current run, and the failure is reported as an actionable diagnostic.

Preconditions:

- The testable GUI is closed for setup.
- The test operates only on the disposable testable identity directory.
- A documented safe method exists to make the disposable settings directory or file temporarily unwritable. If not, mark this case `BLOCKED`.

Steps:

1. With a disposable project registered, capture a baseline `settings.json` snapshot and its bytes/mtime.
2. Make the disposable settings directory or file unwritable using the documented safe method.
3. Launch the app (or trigger an operation that would reconcile/write settings).
4. Confirm the registered project still loads and is usable for this run from the validated selected path.
5. Capture the `settings.json` bytes/mtime and confirm they are unchanged.
6. Capture the diagnostic warning/error and confirm it is actionable and does not claim a successful write.
7. Restore writability and confirm a later save succeeds normally.

Expected Result:

A failed write leaves the file byte-identical, the project remains usable for the run via the in-memory selected path, and the failure surfaces as an actionable diagnostic without corrupting or partially writing the file.

Evidence Required:

- `SET-010-before.json` and `SET-010-after.json` (or byte/mtime comparison) showing no change.
- Screenshot or log of the actionable failure diagnostic.
- Evidence the project loaded despite the failed write.

Pass/Fail Criteria:

PASS if the file is unchanged, the project stays usable, and the diagnostic is actionable. PARTIAL if behavior is correct but one artifact is missing. FAIL if the file is truncated, partially written, or overwritten, or the failure is silent or misreported as success. BLOCKED if no safe method exists to make the disposable settings unwritable.

### SET-011: CLI and GUI interleaving preserves both pair forms

Purpose:

Verify that a CLI registration performed while the GUI is open, followed by a GUI settings save and a GUI project mutation, preserves every active and archived pair with aligned absolute and companion arrays and loses no registration.

Preconditions:

- Depends on SET-001.
- The testable GUI is running for the interleaving steps.
- Two or more disposable AC projects exist for registration.
- Read-only inspection of the disposable `settings.json` is available.

Steps:

1. With the GUI open and at least one project already registered, capture the baseline project list and `settings.json`.
2. Run a CLI `open-project` for a second disposable project against the same testable identity.
3. In the GUI, perform an unrelated settings save (a harmless setting change).
4. In the GUI, perform a project mutation (archive or unarchive an existing project).
5. Capture the final `settings.json` and project list.
6. Confirm both projects remain registered, the CLI-registered entry survives, and every active and archived entry has an aligned absolute path and companion.

Expected Result:

After the interleaving, no registration or companion is lost, the CLI-registered project persists, and all pairs remain aligned and ordered.

Evidence Required:

- Baseline and final `settings.json` snapshots.
- Baseline and final project-list screenshots.
- Notes confirming pair alignment for every active and archived entry.

Pass/Fail Criteria:

PASS if all registrations and companions survive the interleaving with aligned pairs. PARTIAL if state is correct but one snapshot is missing. FAIL if the CLI-registered entry is clobbered, a companion is dropped or misaligned, or a project registration is lost. BLOCKED if the interleaving cannot be exercised on the testable identity.
