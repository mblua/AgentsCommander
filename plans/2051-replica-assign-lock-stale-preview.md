# Plan #2051: "This replica + lock" sends the replica preview's fingerprint

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/2051 (OPEN, label `bug`)
- Repo: `repo-AgentsCommander`; branch `fix/2051-replica-assign-lock-stale-preview`
- Base (frozen at authoring, 2026-09-15 UTC): branch HEAD = remote branch head =
  `15545bd59051445819f06270d2a5f8755380147e` (`git rev-parse HEAD`; `git ls-remote origin
  refs/heads/fix/2051-replica-assign-lock-stale-preview`). Tracked tree clean. Every line number
  below refers to that SHA; if a quoted line no longer matches, re-anchor on the quoted text,
  never on the number.
- Class: Lite (band 1-25), one phase, no partition. Owner `ac-dev-webpage-ui-v4`; reviewer Grinch;
  coordinator `ac-tech-lead-v4`. No architect, no digest.
- Canonical plan: this file. Root `.gitignore` ignores `/plans/`, so commit with
  `git add -f plans/2051-replica-assign-lock-stale-preview.md`.
- 3 modified files, 0 added, 0 removed: `src/sidebar/components/AgentPickerModal.tsx`,
  `src/sidebar/components/AgentPickerModal.test.tsx`, `src-tauri/src/commands/config.rs`
  (validator + its in-file tests). No wire shape, IPC, `src/shared/types.ts`, CSS, dependency,
  workflow, release or migration change.

## 1. Problem and verified cause

Reported: in the coding-agent picker, "This replica + lock" -> "Assign + lock this replica" always
fails with

```
stalePreview: Target selection changed. Rerun preview before applying profile selection.
```

even when nothing changed between the preview and the apply, and rerunning the preview does not
help (reproduced 2026-09-15 on replica `room-16-ac-healer-team/ac-healer`).

Verified at the frozen base:

- The picker's apply is `src/sidebar/components/AgentPickerModal.tsx:956` (`const apply = async
  () => {`). It always has a real replica-scope preview in hand: `runScopePreview` stores the
  response at `:498`, and the backend always fills `targetFingerprint` with a non-optional `String`
  (`src-tauri/src/commands/config.rs:1175`, filled at `:1383`).
- `AgentPickerModal.tsx:985-988` is the defect: for `scope === "replica"` the confirmation is
  hard-coded to `null`, so that fingerprint is dropped before the request is sent.
- `validate_profile_assignment_confirmation` (`config.rs:1786-1805`) accepts exactly two shapes:
  a provided value equal to the freshly recomputed fingerprint (`:1797`), or `None` for
  `Replica + Ordinary` (`:1800-1802`). Everything else returns the stale error (`:1792-1794`,
  `:1803`).
- `profile_assignment_requires_explicit_confirmation` (`config.rs:1760-1765`) makes
  `Replica + AssignAndLock` a confirmation-required operation, and the preview advertises it
  (`:1384`).
- Apply reaches that validator at `config.rs:1484-1490`, before `enumeration` writes, restart
  prevalidation and publication. A `Replica + AssignAndLock` request that carries `None` can
  therefore only be answered with `stalePreview`; the browser promise rejects, the modal prints it
  (`AgentPickerModal.tsx:1030-1033`, rendered `:1660-1661`) and toasts it (`:2021`).

Result: replication + lock from the GUI is broken 100% of the time by construction, and the text
blames a stale preview that never existed.

- Echoing the preview value is *correct*, not merely accepted: for replica scope `bulk == false`
  (`config.rs:2513`), so `conflict_count == 0` (`:2514-2521`), `decisions == None` and
  `top_fingerprint == direct_fingerprint` (`:2583-2589`) - the value returned as
  `targetFingerprint` (`:1383`) is exactly what the apply recomputes through
  `assignment_expected_fingerprint(None)` (`:2623-2625`).
- Introduced by the #1941/#1943 selection-lock work: the API plan states "Legacy no-fingerprint
  ordinary replica apply remains permitted, but all bulk, new assignAndLock and removal operations
  require current preview confirmation"
  (`plans/issue_1937_implementation/1941-selection-lock-api.md:79`), and the picker plan added the
  `+ lock` UI (`1943-selection-lock-picker.md`) without changing the client branch that sends
  `None` for replica scope.

Second, latent cause of the same failure, fixed in the same change: the replica-scope preview is
fetched with `restartSessions: restartSessions()` (`AgentPickerModal.tsx:493`), while a replica
apply always sends `restartSessions: false` (`:979`, because the post-assign "Restart now?" prompt
owns replica restarts, #537). `restart_sessions` is part of the hashed tuple
(`config.rs:2268`, `:2428-2444`). The toggle is hidden on replica scope (`:1870`) but its signal
survives a switch from a bulk scope, so once the client starts echoing the fingerprint, a checked
toggle left over from `kind`/`workgroup` would still produce a genuine mismatch - and the same
misleading message.

Also verified: the guard itself was never broken. At the Rust API level the combination works today
when the fingerprint is provided (`issue_1937_selection_api_replica_assign_and_lock_is_atomic`,
`config.rs:7931-7976`, asserts `updatedCount == 1` and the lock written). The bug is client-only;
the only server-side gap is the error vocabulary for a *missing* fingerprint (issue item 5).

## 2. Decision

D1 - The client sends the replica-scope preview's own `targetFingerprint` for
`Replica + AssignAndLock`. The replica scope's preview and the replica scope's apply hash the same
input set (operation, anchor, scope, pair, restart, candidate state), so matching fingerprint =
no change, mismatch = the target really changed. No new wire field, no server-side trust change.

D2 - `Replica + Ordinary` keeps sending `null`, exactly as today, and the backend keeps accepting it
(`config.rs:1800-1802`). Requirement 3 ("'This replica' without lock keeps working as today") is
literal: the deliberate single-replica ordinary assign keeps its legacy no-confirmation semantics,
including the ability to apply over a target that changed after the preview. The change does not
widen or narrow that path.

D3 - The replica-scope preview hashes the *effective* restart choice: `false` for replica scope,
`restartSessions()` for bulk scopes. The same helper drives the apply, so preview and apply can no
longer disagree on a value they must agree on. Behavior-preserving for bulk scope (identical
expression) and for replica (the apply already sent `false`).

D4 - The client never sends a lock apply without a fingerprint: `applyEnabled()` requires the
replica preview to be loaded and not busy when `scope === "replica" && mode === "assignAndLock"`,
the same rule the bulk scopes already apply (`AgentPickerModal.tsx:650`). While the preview is in
flight the button is disabled and the existing "Loading targets..." / preview-error surfaces
(`:1782-1790`) stay the only feedback. `Replica + Ordinary` keeps today's enabling rule.

D5 - A rejected replica + lock refreshes the replica-scope preview, mirroring the bulk recovery
(`:1013-1016`, `:1035-1039`): the old fingerprint is cleared, the fresh preview is fetched, and the
button re-enables with the new value. Without this, a *legitimate* stale rejection leaves a
dead Retry that replays the same stale fingerprint.

D6 - The backend distinguishes "no fingerprint sent" from "fingerprint did not match" (issue item
5): `None` for an operation that requires confirmation returns a new
`confirmationMissing:` error; only a provided, non-matching value returns `stalePreview: Target
selection changed...`. The new message states the fact and does not tell the user to rerun the
preview, because a missing fingerprint is a client contract failure, not a target change. This is
a Rust change; it is required, because the message is minted in Rust and the wire contract must be
honest for every client (desktop, browser/WS, an older cached bundle), not only for the picker that
this plan also fixes. The legacy `None` + `Replica + Ordinary` path stays `Ok`.

Alternatives closed:

- (a) Frontend-only, "never send null for replica": rejected as the *sole* fix. It leaves the
  server minting a false "Target selection changed" for any other client, which is exactly what
  issue item 5 forbids; and it cannot express the difference between missing and mismatched.
- (b) Make `confirmedTargetFingerprint` a required `String` for confirmation-required operations:
  rejected. It is an `Option<String>` shared by the legacy replica-ordinary path (frontend
  `confirmedTargetFingerprint: null`, `web/commands.rs:1625`, `:1625` pinned by the web dispatch
  test at `:1594`) and by removal/default commands that reuse the same vocabulary; a serde-level
  rejection would replace a business error with a deserialization error and break the legacy path.
- (c) Re-fetch the preview inside `apply()` and use that fingerprint: rejected. It would defeat the
  stale-state guard (the approval would always match the state at write time - requirement 2 would
  silently stop protecting).
- (d) Drop `restart_sessions` from the fingerprint for replica scope: rejected. The fingerprint is
  one typed tuple (`config.rs:2255-2270`) shared by every scope; changing its shape for one scope
  weakens a guard the #1941 design made uniform, for a mismatch that is a client bug.
- (e) Clear `restartSessions()` when entering replica scope: rejected. It discards a user choice
  made for bulk scope, and the toggle is hidden there, so the state loss would be invisible.

## 3. In scope / out of scope

In scope:

- The replica-scope fingerprint echo, the replica-scope restart parity, the replica+lock preview
  gate and the post-rejection re-preview in `AgentPickerModal.tsx`.
- The missing-vs-stale error split in `validate_profile_assignment_confirmation` and its tests.
- Frontend and backend tests for both the positive and the negative control.

Out of scope (untouched, must stay green):

- Bulk scopes (`kind`, `workgroup`) with and without lock, the conflict review, the decision
  projections, removal, and the Matrix default: their request shapes, fingerprints and messages.
- `Replica + Ordinary` semantics (requirement 3), and the `requiresExplicitConfirmation` preview
  hint.
- The fingerprint scheme, the guarded per-file CAS, the operation-turn ownership, any IPC type,
  `types.ts`, CSS, dependencies, docs and workflows.

## 4. Exact changes

### 4.1 `src/sidebar/components/AgentPickerModal.tsx`

(a) Fingerprint echo - `apply()`, replace `:985-988`:

```ts
    // #2051 - "This replica + lock" is confirmed by the replica-scope preview's
    // own fingerprint; the legacy replica ordinary apply deliberately sends none
    // (the backend still accepts a missing fingerprint for replica + ordinary).
    const previewFingerprint = scopePreview()?.targetFingerprint ?? null;
    const confirmedFingerprint =
      scope === "replica"
        ? mode === "assignAndLock"
          ? previewFingerprint
          : null
        : reviewedFingerprint ?? previewFingerprint;
```

(`mode` is the local const from `assignmentMode()` at `:960`.) The bulk expression is
behavior-identical to the current `reviewedFingerprint ?? scopePreview()?.targetFingerprint ?? null`.

(b) Restart parity - new helper above `runScopePreview` (`:482`), used by the preview request and
by the apply:

```ts
  /** #2051 - a replica apply never restarts through this toggle (#537, the
   *  post-assign prompt owns it), so the replica preview must hash the same
   *  `false` the apply sends. Bulk scopes keep the toggle. */
  const selectionRestart = (scope: ProfileAssignmentScope) =>
    scope === "replica" ? false : restartSessions();
```

Replace `:493` `restartSessions: restartSessions(),` with `restartSessions: selectionRestart(scope),`
and `:979` `const restart = scope === "replica" ? false : restartSessions();` with
`const restart = selectionRestart(scope);`. The effect still reads `restartSessions()` at `:517`, so
toggling it still re-previews every scope.

(c) Preview gate - `applyEnabled()`, replace `:648`:

```ts
    if (scope === "replica") {
      // #2051 - the fingerprint IS that confirmation, so the lock button waits
      // for its own preview exactly like the bulk scopes below.
      if (assignmentMode() === "assignAndLock" && (scopePreviewBusy() || !scopePreview())) {
        return false;
      }
      return !isRedundantReplicaSelection();
    }
```

The lock radios only render under `showBroadScope()` (`:1670`), which requires a WG replica path
(`:250-252`), so this gate cannot be reached by a non-replica path.

(d) Post-rejection recovery - `apply()` catch, replace `:1035`:

```ts
      // #2051 - a rejected replica + lock needs the same fresh review the bulk
      // scopes get; the old fingerprint must never be replayed. Replica ordinary
      // keeps today's behavior.
      const needsFreshReview = scope !== "replica" || mode === "assignAndLock";
      if (needsFreshReview && target && isWgReplica()) {
```

(The body of the block, `:1036-1038`, is unchanged.)

### 4.2 `src-tauri/src/commands/config.rs` - `validate_profile_assignment_confirmation`

Replace the body of `config.rs:1786-1805` so that only a *provided, non-matching* value is stale:

```rust
fn validate_profile_assignment_confirmation(
    scope: &ProfileAssignmentScope,
    mode: AssignmentMode,
    provided: Option<&str>,
    fingerprint: &str,
) -> Result<(), String> {
    match provided {
        Some(value) if value == fingerprint => Ok(()),
        // #1941 - the legacy deliberate replica assignment keeps working without
        // a fingerprint; every bulk, assignAndLock and removal path needs one.
        None if *scope == ProfileAssignmentScope::Replica && mode == AssignmentMode::Ordinary => {
            Ok(())
        }
        // #2051 - no fingerprint at all is a client contract failure, not a
        // target change: never report it as a stale preview.
        None => Err(
            "confirmationMissing: This operation requires the preview confirmation, and none was sent. Nothing was written."
                .to_string(),
        ),
        Some(_) => Err(
            "stalePreview: Target selection changed. Rerun preview before applying profile selection."
                .to_string(),
        ),
    }
}
```

Notes:

- The stale message and its `stalePreview:` code are byte-identical to today for mismatches.
- The new message deliberately contains neither `Target selection changed` nor an instruction to
  rerun the preview.
- `profile_assignment_requires_explicit_confirmation` (`:1760-1765`) is **not** changed: replica +
  lock still needs a fingerprint, the preview still reports `requiresExplicitConfirmation: true`.
- No other call site of this validator exists (`grep` over `src-tauri/src`), so the split cannot
  leak into removal/default messages; their own `stalePreview` strings (`:2817`, `:2980`) are
  untouched.

### 4.3 Tests

Modified files only; two existing assertions change (both pin the old missing-fingerprint message)
and are named in section 6.

## 5. Behavior and edge cases

| # | Case | Result |
|---|---|---|
| 1 | Replica + lock, nothing changed (the report) | The preview's `targetFingerprint` is echoed; the backend matches it; pair + lock written in one operation; `newlyProtectedPaths` = the replica; no restart; modal commits |
| 2 | Replica + lock, target changed after the preview (pair, lock flag, membership, live sessions, anchor identity) | Fingerprint mismatch -> `stalePreview`; the validator runs before any write; no config write, no sidecar, no restart; the modal clears the replica preview and re-previews (D5) |
| 3 | Replica + lock, preview in flight or not loaded | Apply disabled; "Loading targets..." shown; no request can be sent without a fingerprint (D4) |
| 4 | Replica + lock, preview failed | Apply stays disabled; the preview error is rendered (`agentPicker.previewError`, `:1787-1790`); the user can retry by changing a selection input, exactly like bulk scope today |
| 5 | Replica + lock, matching fingerprint sent | Accepted; the existing API test `config.rs:7931` proves the server side |
| 6 | Replica + lock, no fingerprint sent at all (non-GUI/legacy client) | `confirmationMissing`; no write; never "Target selection changed" (item 5) |
| 7 | Replica ordinary, with or without a fingerprint | Unchanged: the client sends `null`, the backend accepts `None` for replica + ordinary; a changed target still applies, as today |
| 8 | Bulk scopes with and without lock | Unchanged: reviewed-decision fingerprint when a conflict was reviewed, otherwise the direct/top fingerprint |
| 9 | Bulk scope with "Restart sessions after apply" checked, then switch to replica + lock | The replica preview and the replica apply both hash `restartSessions: false`; bulk previews keep the toggled value (D3) |
| 10 | Ordinary <-> lock toggle inside replica scope | `createEffect` (`:510-535`) resets all three previews to `null` and re-fetches them for the new `(scope, mode)`; the lock mode's fingerprint carries `operation: "assignAndLock"`, so the gate (D4) prevents reusing the ordinary preview's fingerprint |
| 11 | Another window updates the selection while the modal is open | `handleExternalSelectionUpdate` re-fetches all three previews and clears them; replica + lock apply is disabled until the fresh one lands; the next apply uses the fresh fingerprint |
| 12 | Lock state unknown/invalid for the replica | Unchanged: lock radios are disabled (`lockStateUsable()`, `:639-650` region) and lock apply stays blocked |
| 13 | Same pair + lock, ordinary assign disabled by #551 | Unchanged: `isRedundantReplicaSelection()` returns `false` for `assignAndLock` (`:611-613`), so the lock path stays enabled |
| 14 | Non-replica path (repo dir, origin agent) or missing `scopeContext` | Unchanged: no radiogroup, no preview calls, no fingerprint, local commit only |

## 6. Tests

### 6.1 Frontend - `src/sidebar/components/AgentPickerModal.test.tsx` (modified)

All new cases go inside the existing `describe("selection lock picker (#1943)")` block, whose
`renderLockPicker` fixture already renders the six radios on an unlocked WG replica and whose
`scopeAwarePreview()` mock (`:289-329`) already answers the replica scope with
`targetFingerprint: "fp-replica"`. Harness helpers reused as-is: `renderLockPicker`, `settle`,
`clickRadio`, `target`, `text`, `maybe`.

- **T-F1 (positive, red on base)** - "sends the replica preview fingerprint for This replica +
  lock (#2051)": render; `clickRadio("agentPicker.scope.lock.replica")`; settle; click
  `agentPicker.apply`; settle; assert the call

  ```ts
  expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
    expect.objectContaining({
      scope: "replica",
      assignmentMode: "assignAndLock",
      restartSessions: false,
      confirmedTargetFingerprint: "fp-replica",
    }),
  );
  ```

  and `onSelect` called. On base this fails with `confirmedTargetFingerprint: null` - the raw
  reproduction of the report.
- **T-F2 (the D4 gate)** - "blocks This replica + lock until the replica preview is loaded
  (#2051)": install a local `previewCodingAgentProfileSelection` mock that leaves `scope ===
  "replica"` pending and resolves the other two scopes; after `clickRadio` assert
  `agentPicker.apply` is `disabled` and `agentPicker.previewBusy` text is shown; resolve the
  replica preview with `targetFingerprint: "fp-replica-fresh"`; assert the button is enabled.
- **T-F3 (D5 recovery, red on base)** - "re-previews replica scope after a rejected replica + lock
  apply (#2051)": `applyCodingAgentProfileSelection` rejects with the exact stale message; click
  lock.replica; `mockClear()` the preview mock; click apply; settle; assert `onSelect` was never
  called, the toast contains the message, and the preview mock was called exactly once with
  `{ scope: "replica", assignmentMode: "assignAndLock" }`. On base the count is 0.
- **T-F4 (D3 restart parity, red on base)** - "hashes the replica preview with the restart value
  the apply sends (#2051)": local preview mock that records `{scope, restartSessions}` and answers
  the replica scope with `fp-replica-restart` when `restartSessions === true` (mimicking the
  backend's restart-sensitive hash) and `fp-replica` otherwise; click `agentPicker.scope.kind`;
  check `agentPicker.restartToggle`; then click `agentPicker.scope.lock.replica`; assert every
  recorded replica request used `restartSessions: false`, and that the apply carries
  `{ restartSessions: false, confirmedTargetFingerprint: "fp-replica" }`.
- **T-F5 (requirement 3 pin, must stay green, no edit)** - the existing
  "applies a replica-scope selection through the backend and then commits" (`:1070-1085`) already
  asserts `confirmedTargetFingerprint: null` for replica ordinary.
- **T-F6 (requirement 4 pin, must stay green, no edit)** - the existing bulk fingerprint tests:
  kind ordinary (`:817-870`), workgroup ordinary (`:877-910`), reviewed `unlockedOnly` and
  `forceReviewed` (`:1540-1575`), and "keeps same-pair assignAndLock enabled..." (`:1472`).

The frontend tests pin the *wire payload* and the client's reaction. They cannot prove "writes
nothing" (the transport is mocked); that proof is the Rust test T-R3.

### 6.2 Backend - `src-tauri/src/commands/config.rs` (modified, in-file `#[cfg(test)] mod tests`)

- **T-R1 (updated existing, red on base)** -
  `broad_assignment_rejects_missing_or_stale_fingerprint` (`:5974-6000`): for `Kind` and
  `Workgroup`, the missing-fingerprint case now asserts `confirmationMissing` **and** the absence
  of `Target selection changed`; the stale case keeps asserting `stalePreview` +
  `Target selection changed`.
- **T-R2 (new unit)** - `replica_assign_and_lock_confirmation_shapes`: direct calls to
  `validate_profile_assignment_confirmation` asserting (a) replica + lock + matching -> `Ok`, (b)
  replica + lock + `None` -> `confirmationMissing`, never stale, (c) replica + lock + mismatched
  -> `stalePreview`, (d) replica + ordinary + `None` -> `Ok` (legacy, D2), (e) replica + ordinary +
  mismatched -> `stalePreview` (a provided value that does not match is still stale).
- **T-R3 (new integration - the required negative control)** -
  `issue_2051_selection_api_replica_lock_changed_target_is_stale_without_writes`, using
  `selection_api_fixture` / `selection_api_replica` / `unlocked_tooling` / `api_preview` /
  `api_apply` / `config_bytes` / `sidecar_path` (`:7278-7360`, `:7437`, `:7536`):
  1. create an unlocked replica; `preview = api_preview(settings, &replica, Replica,
     AssignAndLock)`;
  2. change the stored pair after the preview, exactly like
     `issue_1937_selection_api_stale_approvals_are_rejected_without_writes` does (`:8284-8296`):
     read `config.json`, set `tooling.profile = "C"`, rewrite, capture `changed_bytes`;
  3. `api_apply(..., Replica, AssignAndLock, None, Some(&preview.target_fingerprint))` ->
     `Err` containing `stalePreview`; assert `config_bytes(&replica) == changed_bytes`, no
     `sidecar_path`, and `selectionLocked` is not `true`;
  4. control inside the same test: take a **fresh** preview after the change and apply with that
     fingerprint -> `Ok`, `updatedCount == 1`, `newlyProtectedPaths == vec![updatedReplicaPaths[0]]`.
     This proves step 3 rejected *because of the change*, not because the combination is broken.
- **T-R4 (new integration, item 5, red on base)** -
  `issue_2051_selection_api_replica_lock_missing_fingerprint_is_not_stale`: unlocked replica,
  `api_apply(..., Replica, AssignAndLock, None, None)` -> `Err` containing `confirmationMissing`
  and **not** `Target selection changed`; config bytes unchanged, no sidecar, no lock written. On
  base the same call returns the stale message, so this is the red proof for item 5.
- **T-R5 (updated existing, red on base)** -
  `issue_1937_selection_api_bulk_assign_and_lock_requires_decision` (`:8124`, assert at `:8171`):
  the force-decision-without-fingerprint case changes from `stalePreview` to
  `confirmationMissing` (+ absence of `Target selection changed`). The no-write assertions
  (`:8172-8175`) stay.

Existing backend suites that must stay green (the authority for what this plan does not touch):

- `issue_1937_selection_api_replica_assign_and_lock_is_atomic` (`:7931`) - the API positive control
  for replica + lock with a fingerprint.
- `issue_1937_selection_api_absent_assignment_mode_is_ordinary_legacy` (`:7828`) - replica
  ordinary, no fingerprint.
- `issue_2010_selection_api_replica_locked_same_pair_keeps_lock` (`:7978`) and
  `issue_2010_selection_api_replica_locked_different_pair_still_errors` (`:8026`).
- `issue_1937_selection_api_stale_approvals_are_rejected_without_writes` (`:8257`),
  `issue_1937_selection_api_decision_snapshots_are_per_decision` (`:8179`),
  `issue_1937_selection_api_rejects_decision_outside_bulk_assign_and_lock` (`:8071`) - bulk
  staleness and decision rejection, message unchanged.
- `issue_1937_selection_api_per_file_cas_rejects_changed_target` (`:8922`) - the low-level CAS.
- `src-tauri/src/web/commands.rs:1594`
  `apply_coding_agent_profile_selection_web_dispatch_broadcasts` - browser transport sends replica
  ordinary with `confirmedTargetFingerprint: null` (the D2 legacy allowance over WS).

## 7. Acceptance criteria

Issue's required behavior, one by one:

1. Replica + lock, nothing changed -> applies in one step, no error: T-F1 (payload), T-F4
   (restart parity), T-R3 step 4 and the existing `config.rs:7931` (server accepts).
2. Replica + lock, target changed after the preview -> rejected and nothing written: T-R3 steps
   1-3 (the negative control the tech lead requires) plus T-F3 (the client's reaction).
3. Replica + ordinary unchanged: T-F5 and the existing Rust legacy test / web test.
4. Bulk scopes with and without lock unchanged: T-F6, and no diff touches that branch.
5. "Target selection changed" only on a real change; a missing fingerprint is never reported that
   way: T-R1, T-R2(b)/(e), T-R4, T-R5.

Issue's three acceptance tests: "replica + lock no change applies" (1), "replica + lock change is
rejected, nothing written" (2), "replica without lock unchanged" (3).

Gates:

- `npm run typecheck` clean.
- `npx vitest run src/sidebar/components/AgentPickerModal.test.tsx` green (68 existing + 4 new).
- Full `npm test` green (any pre-existing failure must be named and shown unrelated).
- `cargo test --lib issue_2051 -- --test-threads=1 --nocapture` green, plus the named
  `issue_1937`/`issue_2010` selection tests green.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (CI gate).
- No dependency, workflow, docs, CSS, IPC or type change.

## 8. Proof protocol (for the reviewer)

1. On the base `15545bd`, add **only** the frontend cases T-F1/T-F3/T-F4 and run

   ```
   npx vitest run src/sidebar/components/AgentPickerModal.test.tsx
   ```

   Capture the raw red: T-F1 fails with `confirmedTargetFingerprint: null` received, T-F3 fails
   with 0 preview calls after the rejection, T-F4 fails on the replica preview's
   `restartSessions: true` / the echoed fingerprint.
2. Still on base, add only T-R4 and run

   ```
   cargo test --lib issue_2051 -- --test-threads=1 --nocapture
   ```

   Capture the raw red: the missing fingerprint returns `stalePreview: Target selection changed...`
   instead of `confirmationMissing`. T-R3 is green on base by construction (base rejects every
   replica + lock apply without a matching fingerprint) and stays green after the fix - it is the
   control that the fix does not weaken the guard.
3. Implement section 4, then re-run both commands plus

   ```
   npm run typecheck
   cargo test --lib issue_1937_selection_api -- --test-threads=1
   cargo test --lib issue_2010 -- --test-threads=1
   cargo clippy --workspace --all-targets -- -D warnings
   ```

   -> green. Then the full suites (`npm test`, `cargo test --lib --bins --tests`).
4. Reply to the coordinator with the raw pre-fix red and post-fix green outputs, the exact commit
   SHAs, and the changed-file list.

Environment note for steps 2-3: this room replica has no `src-tauri/target` yet, so the first Rust
build is a cold full compile (the `~/.cargo` registry cache is populated). Budget a long first run;
do not interpret it as a hang.

## 9. Risks and compatibility

| Risk | Assessment |
|---|---|
| Weakening the stale guard (blast radius 8) | The change only *narrows* staleness: the mismatch branch is untouched and a missing fingerprint still fails, just with an honest message. T-R3 is the explicit control that a changed target is still refused with zero writes |
| Wire compatibility | No type change: `confirmedTargetFingerprint` stays `Option<String>`; the replica-ordinary `null` contract is unchanged in both directions (frontend T-F5, `web/commands.rs:1594`) |
| Error-vocabulary change | Additive: a new `confirmationMissing:` code; `stalePreview:` keeps its exact text for real mismatches. No production code matches on the string (only tests do), so no client breaks |
| Behavior change for other clients | A bulk apply arriving with no fingerprint now reports `confirmationMissing` instead of `stalePreview`. Same rejection, honest cause. No client in this repo sends that combination (the picker gates bulk on a loaded preview, `:650`) |
| Hidden fingerprint coupling for lock mode | `operation` is part of the hash, so the replica-scope fingerprint for `ordinary` differs from `assignAndLock`; the effect resets previews on a mode change (`:510-535`) and D4's gate prevents cross-mode reuse |
| Test-noise from the restart hash | Only the local mock in T-F4 makes the replica answer restart-sensitive; the shared `scopeAwarePreview()` fixture is untouched, so other tests keep their fingerprints |
| Cold Rust build (tooling) | No `target/` in this replica; the first `cargo test`/`clippy` is slow. Not a correctness risk |
| `plans/` is gitignored | Commit the plan with `git add -f` or it is silently missing from the push |

## 10. Implementation order

1. Add T-F1/T-F3/T-F4 (frontend, red) and T-R4 (Rust, red); capture both raw reds.
2. Frontend: 4.1(a) fingerprint echo, then 4.1(b) restart helper, then 4.1(c) gate, then 4.1(d)
   recovery. Re-run the frontend file until green.
3. Backend: 4.2 validator split; update T-R1/T-R5; add T-R2/T-R3/T-R4. Run the focused Rust tests.
4. Gates: `npm run typecheck`, full `npm test`, focused `cargo test`, `cargo clippy`.
5. Commit the 3 files plus this plan (`git add -f plans/2051-replica-assign-lock-stale-preview.md`),
   push, verify the remote head moved.
6. Reply to `ac-tech-lead-v4` with the plan path, the commit SHA, the changed-file list and the
   environment risk (cold Rust build).

## Plan Contract

Scope: the 3 files in section 4 plus this plan. The implementer runs the gates in section 7, keeps
the existing suites in section 6.2 green, and reports raw outputs. Any change beyond these files,
any weakening of the stale guard (requirement 2), or any reinterpretation of requirement 3
(replica ordinary keeps its no-fingerprint path) stops the work and goes back to the coordinator
before implementation.
