# Plan 2057 - Guard profile slot delete against configured cells

Status: READY_FOR_IMPLEMENTATION
Issue: https://github.com/mblua/AgentsCommander/issues/2057
Repo: `repo-AgentsCommander`
Branch: `fix/2057-guard-profile-slot-delete`
Planning base (frozen): `23ddedbe0a2e3efd24b51b8797d4a2beefc1c0b6` (`main`, 2026-09-15 21:27:59 -0300)
Owner: frontend
Class: design-bearing
PARTITION: 1 phase (5 files, one owner, no IPC/CLI/persistence/schema contract change; the partition trigger does not apply)

## 1. Task class and accepted threat model

Routine application-code change. Frontend TypeScript and CSS only. No Rust, no IPC
command, no persisted schema change, no build/packaging/release step, no security
boundary. Baseline controls apply; no enhanced control is applicable. Concrete
reasons for each non-applicable enhanced control are in section 9.

## 2. Problem

`removeProfileLetter` deletes a profile letter from **every** coding agent, while the
UI shows only the rail of the agent whose button was pressed. Pressing "Delete
Profile" on a slot that looks empty in the visible rail silently destroys fully
configured cells belonging to agents that are not on screen. The settings writer is
an atomic tmp+rename with no backup or rotation, and `repair_coding_agent_profiles`
only recreates the mandatory `A` slot, so the loss is unrecoverable from inside the
app.

User requirement (verbatim, Spanish): "No deberia permitirse borrar un profile que
esta efectivamente configurado en otro coding agent. Deberia aparecer un cartel
diciendo que primero debe dejar vacio el profile en todos los coding agent."

## 3. Evidence (verified on the frozen base)

E1. `src/sidebar/components/SettingsModal.tsx:1381-1407` - `removeProfileLetter(letter)`
    deletes `profileSlots[letter]`, then loops every entry of `profilesByAgent` and
    `profileLabelsByAgent` deleting that letter. It takes no agent id.

E2. `src/sidebar/components/SettingsModal.tsx:3731` - the click handler is
    `onClick={() => removeProfileLetter(letter)}`. Every sibling control in the same
    card passes `agent.id` (`updateProfileCellCommand(agent.id, letter, ...)` at
    `:3611`, `removeCellEnvRow(agent.id, ...)` at `:3685`).

E3. `src/sidebar/components/SettingsModal.tsx:3726-3728` - the global scope is a
    deliberate #538 decision, and the button `title` at `:3732` advertises it
    ("from all coding agents"). The same comment records that #538 **removed** the
    per-cell delete affordance: there is today no control anywhere in the profile
    card that empties one agent's cell.

E4. `src/sidebar/components/SettingsModal.automation.test.ts:1462-1531` - the test
    "deletes a whole profile slot from every agent via Delete Profile" pins the
    current behaviour and asserts at `:1516` that `profilesByAgent.claude.B` is gone
    after deleting B from the codex rail. Its fixture has B configured on both
    agents, so under this plan that click is blocked; the test must be inverted.

E5. Profile identity is keyed by unique agent `id`, frontend
    (`profilesByAgent[agent.id]`, `SettingsModal.tsx:1330-1356`) and backend
    (`profiles_by_agent: BTreeMap<String, BTreeMap<String, ProfileCellConfig>>`,
    `src-tauri/src/config/settings.rs:162`; repair keys on `agent.id` at `:1969-1978`).
    Identity is correct and is not the defect.

E6. `src/shared/profile-utils.ts:176-183` already exports `profileConfiguredElsewhere`,
    which tests `Boolean(cells[letter]?.enabled)` only. `cellForLetter`
    (`profile-utils.ts:80-88`) applies the same `enabled`-only rule, and it is the
    rule the launch fallback chain in `resolveProfilePreview` (`:164-171`) uses. So
    an `enabled` cell changes launch behaviour even when its command is empty.

E7. `src/sidebar/components/SettingsModal.tsx:1320-1325` - `emptyProfileCell()` returns
    `{ enabled: true, command: "", env: {}, notes: "" }`, and `updateProfileCellCommand`
    (`:1406-1414`) materialises it on the first keystroke. Typing a command and then
    clearing the text leaves a real `{ enabled: true, command: "" }` cell behind. An
    existence-only rule would therefore treat keystroke residue as configured.

E8. `src/sidebar/components/SettingsModal.tsx:1512-1522` - `removeAgent` does **not**
    clean `profilesByAgent` or `profileLabelsByAgent`. Cells keyed by an agent id
    that is no longer in `settings.data.agents` are reachable in the data,
    unreachable from the UI, and would be unclearable.

E9. `src/sidebar/components/SettingsModal.tsx:1427-1444` - `cellEnvRows`/`syncCellEnv`
    write env rows straight back into the stored cell, and `updateProfileCellCommand`
    (`:1406-1414`) writes the command text straight into the stored cell. The local
    `profileCellText`/`profileCellEnvRows` stores only shadow the cell before the
    first edit. `settings.data.codingAgentProfiles` is therefore the authoritative
    read for any guard.

E10. `src/sidebar/components/SettingsModal.tsx:743` - saving persists the whole trimmed
     object via `SettingsAPI.saveDraft(nextSettings)`; `save_settings_draft`
     (`src-tauri/src/commands/config.rs:617-622`) writes into the same `settings.json`.
     There is no separate draft file and no second chance.

E11. `src/sidebar/styles/sidebar.css:2296-2340` holds `.settings-profile-card-footer`,
     `.settings-profile-delete-profile` (with a `:hover:not(:disabled)` rule already
     present at `:2310`) and `.settings-profile-cell-error`.

E12. `defaultProfileByAgent` is keyed by target name, not agent id
     (`AgentPickerModal.tsx:263,435,610`), is not touched by `removeProfileLetter`,
     and a dangling letter degrades through the existing fallback chain.

## 4. Decisions (binding; nothing here is left to the implementer)

**D1 - What "effectively configured" means.** A coding agent *holds* slot `L` when
either of these is true:

- its cell `profilesByAgent[id][L]` exists and (`enabled` is true **or** the trimmed
  `command` is non-empty **or** `env` has at least one key **or** the trimmed `notes`
  is non-empty); or
- its own slot label `profileLabelsByAgent[id][L]` is non-empty after trimming.

Rationale: the first disjunct of the cell clause is exactly the rule that changes
launch behaviour (E6); the remaining disjuncts catch user-entered data that a
disabled cell would still lose. The union is a superset of both, so no data can be
destroyed past the guard. Residue cells that are disabled and carry nothing
(`{enabled: false, command: "", env: {}, notes: ""}`) and empty-string labels are
**not** held, which keeps an empty-everywhere slot deletable. The existing
`profileConfiguredElsewhere` is **not** reused for this: its `enabled`-only rule is
correct for the badge but would miss a disabled cell that still holds a command (E6).
It stays as it is and keeps its current callers.

**D2 - Holders are computed over live agents only.** The holder set is restricted to
ids present in `settings.data.agents`. Cells or labels keyed by an agent id that no
longer exists (E8) are not holders; they are dead data with no UI that could clear
them, and treating them as holders would make the slot permanently undeletable. They
are removed with the slot, as today.

**D3 - Slot-level (global) delete semantics stay.** The #538 decision is preserved:
when the guard passes, "Delete Profile" still removes `profileSlots[L]` and the
letter from every agent's cells and labels, exactly as `removeProfileLetter` does
today. The guard is a precondition, not a change of scope. `A` remains immutable and
keeps no delete button (`Show when={letter !== "A"}` at `:3723`).

**D4 - Blocked state: disabled button plus an inline notice naming the agents.** No
new modal and no confirm dialog. When the slot has at least one holder:

- the "Delete Profile" button renders `disabled`, with `data-ac-state="blocked"` and
  `title="Empty this profile in every coding agent before deleting the slot"`;
- a sibling notice renders directly below it inside `.settings-profile-card-footer`,
  with `data-ac-testid={`${cardId}.deleteProfile.blocked`}`, `data-ac-role="status"`
  and class `settings-profile-delete-blocked`, whose text is exactly:
  `Still configured in: <names>. Empty this profile in every coding agent before deleting the slot.`

`<names>` is the holder agents' `label` (falling back to the agent `id` when `label`
is empty), in `settings.data.agents` order, joined with `", "`. The currently
selected agent is included when it holds the slot, so the notice always tells the
whole truth. When there are no holders the button renders enabled with
`data-ac-state="enabled"`, its existing title, and no notice node is rendered at all.

**D5 - "Clear in this agent" is added, and this reverses the other half of #538.**
Blocking deletion without an affordance that empties one agent's cell would deadlock
the slot forever (E3). A second footer button is added, in the same
`Show when={letter !== "A"}` block, **before** the Delete Profile button:

- label `Clear in this agent`, class `settings-profile-cell-btn settings-profile-clear-cell`,
  `data-ac-testid={`${cardId}.clearCell`}`, `data-ac-role="button"`,
  `title={`Empty the ${letter} profile for this coding agent only`}`;
- `disabled` with `data-ac-state="empty"` when this agent does not hold the slot
  under D1, otherwise `data-ac-state="enabled"`;
- `onClick={() => clearProfileCell(agent.id, letter)}`.

`clearProfileCell(agentId, letter)` sets the draft dirty and deletes, in one
`produce` over `codingAgentProfiles`, `profilesByAgent[agentId][letter]` and
`profileLabelsByAgent[agentId][letter]`; it then deletes the exact key
`agentId + ":" + letter` from `profileCellText`, `profileCellErrors` and
`profileCellEnvRows`. It must delete that exact key, not the `endsWith(":" + letter)`
sweep `removeProfileLetter` uses, because only one agent is affected. It does not
touch `profileSlots`, other agents, or `defaultProfileByAgent`.

**D6 - `profileSlots[L].label` and `defaultProfileByAgent` are not holders.**
`profileSlots[L]` is the slot record the delete exists to remove, and its legacy
label is not agent-scoped, so it cannot express "configured in another coding agent".
`defaultProfileByAgent` is keyed by target name and degrades through the existing
fallback chain (E12); it is left untouched, exactly as today.

**D7 - Empty-everywhere and configured-only-here.** With no holders the delete
proceeds unchanged. When the only holder is the currently selected agent the delete
is still **blocked**, and the notice names that agent; the user clears it with the
D5 button, the button then enables, and the delete proceeds. This is deliberate: the
requirement is that the slot be empty in *every* coding agent, and a same-agent
exception would re-open a one-click path to destroying the visible agent's own
configured cell.

**D8 - The guard is both a render-time predicate and a runtime re-check.**
`removeProfileLetter` gains an early return when the holder set is non-empty, so the
invariant does not depend on the button's `disabled` attribute alone.

## 5. Scope - exact files and symbols

| # | File | Change |
|---|------|--------|
| 1 | `src/shared/profile-utils.ts` | add exported `profileCellHoldsData` and `profileSlotHolders` |
| 2 | `src/shared/profile-utils.test.ts` | add unit tests for both new functions |
| 3 | `src/sidebar/components/SettingsModal.tsx` | import the two helpers; add `slotHolderIds`, `slotHolderNames`, `agentHoldsSlot`, `clearProfileCell`; guard `removeProfileLetter`; render the Clear button, the disabled Delete state and the notice |
| 4 | `src/sidebar/components/SettingsModal.automation.test.ts` | invert the `:1462-1531` test; add the new cases in section 7 |
| 5 | `src/sidebar/styles/sidebar.css` | add `.settings-profile-clear-cell` and `.settings-profile-delete-blocked` |

Nothing else changes. No file is created or deleted. `src-tauri/**` is untouched.

### 5.1 `src/shared/profile-utils.ts`

Add after `profileConfiguredElsewhere` (which is left unchanged):

```ts
export function profileCellHoldsData(cell: ProfileCellConfig | null | undefined): boolean {
  if (!cell) return false;
  if (cell.enabled) return true;
  if ((cell.command ?? "").trim() !== "") return true;
  if (Object.keys(cell.env ?? {}).length > 0) return true;
  return (cell.notes ?? "").trim() !== "";
}

export function profileSlotHolders(
  profiles: CodingAgentProfilesConfig,
  letter: string,
  liveAgentIds: readonly string[],
): string[] {
  return liveAgentIds.filter(
    (id) =>
      profileCellHoldsData(profiles.profilesByAgent[id]?.[letter]) ||
      (profiles.profileLabelsByAgent[id]?.[letter] ?? "").trim() !== "",
  );
}
```

The `?? ""` / `?? {}` tolerances match the file's existing defensive style for a
mixed-version payload and are not optional. `profileSlotHolders` returns ids in
`liveAgentIds` order, which is `settings.data.agents` order, so D4's naming order is
a property of the helper and needs no second sort. `ProfileCellConfig` and
`CodingAgentProfilesConfig` are already imported at the top of the file, so the
module's import set stays byte-identical and no new arc is created.

### 5.2 `src/sidebar/components/SettingsModal.tsx`

Add `profileCellHoldsData` and `profileSlotHolders` to the existing
`from "../../shared/profile-utils"` import block at `:64-71`, in its alphabetical
position. No new module is imported.

Near `removeProfileLetter`, add:

```ts
const slotHolderIds = (letter: string): string[] =>
  settings.data
    ? profileSlotHolders(
        settings.data.codingAgentProfiles,
        letter,
        settings.data.agents.map((a) => a.id),
      )
    : [];

const slotHolderNames = (letter: string): string => {
  const held = new Set(slotHolderIds(letter));
  return (settings.data?.agents ?? [])
    .filter((a) => held.has(a.id))
    .map((a) => a.label || a.id)
    .join(", ");
};

const agentHoldsSlot = (agentId: string, letter: string): boolean =>
  settings.data
    ? profileCellHoldsData(
        settings.data.codingAgentProfiles.profilesByAgent[agentId]?.[letter],
      ) ||
      (settings.data.codingAgentProfiles.profileLabelsByAgent[agentId]?.[letter] ?? "")
        .trim() !== ""
    : false;
```

Guard `removeProfileLetter` by extending its existing first line (D8):

```ts
if (!settings.data || letter === "A") return;
if (slotHolderIds(letter).length > 0) return;
```

Add `clearProfileCell` exactly as specified in D5.

In the footer block at `:3723-3738`, keep the #538 comment and extend it to record
that #2057 reinstates a per-agent clear and guards the slot delete. Render, in order:
the Clear button (D5), the Delete button with its guarded `disabled`/`data-ac-state`/
`title` (D4), then the notice wrapped in
`<Show when={slotHolderIds(letter).length > 0}>` (D4).

### 5.3 `src/sidebar/styles/sidebar.css`

Add beside the existing rules at `:2296-2340`: `.settings-profile-clear-cell`
following `.settings-profile-cell-btn`'s visual weight, and
`.settings-profile-delete-blocked` matching `.settings-profile-cell-error`'s
typography in a neutral, non-error colour. No existing selector is edited; the
existing `.settings-profile-delete-profile:hover:not(:disabled)` rule at `:2310`
already handles the disabled state correctly and must not be touched.

## 6. Required behaviour and failure behaviour

- Delete Profile with at least one holder: the click cannot fire (button disabled)
  and, if fired programmatically, `removeProfileLetter` returns without mutating.
  `draftDirty` is not set. The notice names every holder.
- Delete Profile with no holder: unchanged behaviour. `profileSlots[L]` and the
  letter under every agent's cells and labels are removed, and residue entries for
  dead agent ids go with them.
- Clear in this agent: removes only that agent's cell and label for `L` and the
  three local per-cell stores for that one key. The slot and every other agent are
  untouched. The button then reports `data-ac-state="empty"` and disables.
- `A`: no Clear button, no Delete button, no notice.
- `settings.data` not loaded: every helper returns the empty result; nothing renders
  and nothing mutates.
- The guard reads `settings.data.codingAgentProfiles` only (E9); it never reads
  `profileCellText` or `profileCellEnvRows`.

## 7. Tests

Modified - `src/sidebar/components/SettingsModal.automation.test.ts:1462-1531`:
rename to "blocks Delete Profile while the slot is configured in another agent" and
invert it against the same fixture. Assert: the `settings.profileCard.0.B.deleteProfile`
button has `disabled` and `data-ac-state="blocked"`; `settings.profileCard.0.B.deleteProfile.blocked`
exists and its `textContent` is exactly
`Still configured in: Codex, Claude Code. Empty this profile in every coding agent before deleting the slot.`;
after clicking it, both `settings.profileCard.0.B` and `settings.profileCard.1.B`
still exist; after Save, `profileSlots.B`, `profilesByAgent.codex.B` and
`profilesByAgent.claude.B` are all still present. Keep the existing `enterTwoRails()`
setup and the A-baseline assertion.

New, same file:

T1. Deletes the slot when it is empty everywhere: fixture with `profileSlots.B`
    present and no `B` cell or label under any agent. The button is enabled with
    `data-ac-state="enabled"`, no `.blocked` node exists, the click removes the card
    from both rails, and after Save `profileSlots.B` is undefined.

T2. Blocked when only the selected agent holds it (D7): fixture with `B` configured
    on `codex` only. The button is blocked and the notice reads
    `Still configured in: Codex. Empty this profile in every coding agent before deleting the slot.`

T3. Clear-then-delete round trip: from T2's fixture, click
    `settings.profileCard.0.B.clearCell`, assert the Delete button has become enabled
    and the notice node is gone, then click Delete and Save, and assert
    `profileSlots.B`, `profilesByAgent.codex.B` and `profileLabelsByAgent.codex.B`
    are all undefined while `profilesByAgent.codex.A` survives.

T4. Clear is scoped to one agent: fixture with `B` configured on `codex` and
    `claude`. Click `settings.profileCard.0.B.clearCell`, Save, and assert
    `profilesByAgent.codex.B` is undefined while `profilesByAgent.claude.B` is
    unchanged and `profileSlots.B` still exists.

T5. Residue is not a holder: fixture with `B` on `claude` as
    `{ enabled: false, command: "", env: {}, notes: "" }` and no `B` label anywhere.
    The Delete button on the codex rail is enabled and the click succeeds.

T6. A dead agent id does not block (D2): fixture whose `agents` list has `codex` only
    while `profilesByAgent` also carries a fully configured `ghost.B`. The Delete
    button is enabled, the click succeeds, and after Save `profilesByAgent.ghost` has
    no `B`.

New, `src/shared/profile-utils.test.ts`:

T7. `profileCellHoldsData`: true for `{enabled: true, command: "", env: {}, notes: ""}`;
    true for each of a disabled cell with a non-blank command, with one env key, and
    with non-blank notes; false for `null`, for `undefined`, and for
    `{enabled: false, command: "   ", env: {}, notes: "  "}`.

T8. `profileSlotHolders`: returns `[]` for an empty slot; returns ids in
    `liveAgentIds` order, not in `profilesByAgent` key order; includes an agent held
    only by a non-blank `profileLabelsByAgent` entry; excludes an agent whose label is
    `"   "`; excludes an id absent from `liveAgentIds` even when its cell is fully
    configured.

## 8. Verification

Run from the repo root
`D:\0_repos\AgentsCommander_iac\.ac\room-21-ac-dev-team-v4\repo-AgentsCommander`.

| Step | Command | Expected | On failure |
|------|---------|----------|-----------|
| V1 | `npm run typecheck` | exit 0 | fix types; do not widen scope |
| V2 | `npx vitest run src/shared/profile-utils.test.ts` | exit 0, T7-T8 pass | fix the helper |
| V3 | `npx vitest run src/sidebar/components/SettingsModal.automation.test.ts` | exit 0, inverted test plus T1-T6 pass | fix the component |
| V4 | `npm test` | exit 0, or only the known #480 unhandled-WebSocket signature that the CI guard at `.github/workflows/pr-regression-gates.yml:2442-2505` tolerates, and nothing else | any other failure blocks |
| V5 | `git status --porcelain` | exactly the five paths in section 5, no untracked residue | investigate before committing |
| V6 | `git diff --stat <recorded phase base>..HEAD` | the same five paths | investigate |

Owner of V1-V6: the implementer, before opening the PR. Owner of the host-dependent
evidence: GitHub, on the exact PR head.

## 9. Delivery nonfunctional gates

**G1 CI-to-plan parity.** Triggered jobs for a `src/**` diff in
`.github/workflows/pr-regression-gates.yml`: `frontend-regression` (checkout, Node 22,
npm 11.6.2, `npm ci`, `npm run typecheck`, `npm test` under the #480 classifier guard)
is the one that exercises this change, and its steps are mirrored locally by V1 and
V4. `test-debt`, `rust-fmt`, `rust-regression`, `rust-regression-linux`,
`rust-regression-macos`, `rust-linux-release-parity`, `terminal-snapshot-portable`,
`windows-release-cli-smoke` and `issue-1850-windows-profile` also run and must pass;
they are unaffected by a TypeScript/CSS diff and are remote-owned. Delivery requires
every triggered and configured-required check green on the exact PR-head SHA.
Re-derive this table if the workflows or the diff shape drift.

**G2 Deterministic toolchain.** Node 22 and npm 11.6.2 as pinned by the workflow;
`package-lock.json` is unchanged and `npm ci` must not modify it. No Rust toolchain is
involved. No new dependency is added.

**G3 Authorized, traceable Git.** Issue #2057 is open; branch
`fix/2057-guard-profile-slot-delete` already exists off `main` at the frozen base.
All state-changing Git runs inside `repo-AgentsCommander`. Delivery is one PR into
`main` closing #2057. No direct push to `main`. Before the first product write,
re-fetch `main`, record the actual phase base SHA, and classify drift: only movement
touching `src/shared/profile-utils.ts`, `src/sidebar/components/SettingsModal.tsx`,
either test file, `src/sidebar/styles/sidebar.css`, `package-lock.json` or
`.github/workflows/pr-regression-gates.yml` requires refreshing the affected evidence.
Unrelated movement is recorded and synchronized at the PR gate, and does not reopen
this design.

**G4 Process state and working directory.** Every command in section 8 runs with an
explicit cwd at the repo root. Reproducing the CI classifier locally would create
`npm-test-results.json`, `npm-test.log` and `npm-test.normalized.log`; those are CI
artifacts and must not be committed. V5 catches them.

**G5 Validation and scope.** The intended path set is the five files in section 5,
frozen before mutation; V5 and V6 are the postcondition. No generated payload, no
EOL-sensitive artifact, no lockfile change, so ordinary `git diff`/`status` evidence
is sufficient and no canonical byte domain is needed.

**G6 Mutation ownership and recovery.** Five hand-edited files on a dedicated branch.
Immediately before writing, re-verify branch, base and a clean index. Recovery is
`git restore --source=<recorded phase base> -- <the exact path>` for a path this run
actually changed and whose current bytes are still this run's output; on any conflict,
stop and report rather than restoring. No `git reset --hard`, no `git clean`, no
repository-wide restore.

**G7 Bounded execution and diagnostics.** `npm test` is the only long command; it is
non-interactive and bounded by the runner. Retain its stdout and exit code. A
timed-out or failed run is reported as failed.

**G8 Evidence discipline.** Zero is a valid state throughout: an empty holder set
enables the button, an empty `agents` list yields no holders, and an absent notice
node is asserted as absent (T1, T3), not merely left unqueried.

Enhanced controls - all explicitly **non-applicable**, with reason: independently
anchored executable hashes, DLL/helper closure inventories and SDK manifests (no
binary is produced or shipped); poisoned-`PATH` and hostile-host tests (routine
change on a trusted developer host, no supply-chain or signing requirement);
exclusive locks, compare-and-swap writers and mutation ledgers (single-author branch,
no concurrent mutation, no destructive migration); a custom process-group or
descendant-leak harness (no execution infrastructure is created); a full tracked-tree
byte image (no clean/smudge filter or generated payload in scope).

## 10. Dependency-cycle and layering statement

The diff adds exactly two module-to-module references, both from
`src/sidebar/components/SettingsModal.tsx` to `src/shared/profile-utils.ts`
(`profileCellHoldsData`, `profileSlotHolders`), added to the import block already
present at `SettingsModal.tsx:64-71`. That arc already exists on the frozen base, so
the change adds **zero new arcs** and removes none. `profile-utils.ts` gains no import
at all (section 5.1). Direction is unchanged and correct: a sidebar component depends
on a shared pure module, never the reverse. No new SCC, no SCC member-set change, no
cross-boundary arc, no role inversion.

The repository's arc instrument `scripts/02-module-arc-record.mjs` projects a **Rust**
dependency graph into `src-tauri/module-arcs.txt` (its header comment, and
`RECORD_RELATIVE` at `:62`). This diff touches no `.rs` file, so that record cannot
move and the `rust-levelization-run` criterion is satisfied vacuously; the TypeScript
arcs above were verified by reading the import sites directly. `npm run record:arcs`
is not required for this change.

## 11. Out of scope (file follow-up issues; do not widen this PR)

- Profile identity and keying (E5) - correct as-is.
- The `agentAutoUpdateByCommand` command collision at `SettingsModal.tsx:1280-1286`.
- Settings backup and rotation (E10) - the real reason the loss was unrecoverable,
  and the highest-value follow-up.
- `removeAgent` leaving orphan profile cells and labels behind (E8) - this plan makes
  them harmless, it does not clean them up.
- `defaultProfileByAgent` entries pointing at a deleted letter (E12).

## 12. Acceptance criteria

1. Deleting a profile slot is impossible, by button state and by function guard,
   while any live coding agent holds it under D1.
2. The blocked state shows the exact notice text of D4, naming every holder in
   `settings.data.agents` order.
3. A slot empty in every live coding agent still deletes globally, exactly as before.
4. "Clear in this agent" empties one agent's cell and label for that letter and
   nothing else, giving the user the path the notice instructs.
5. The `:1462-1531` test is inverted and T1-T8 exist and pass.
6. V1-V6 pass locally and every triggered and configured-required CI check is green
   on the exact PR-head SHA.
7. The diff touches exactly the five files in section 5.
