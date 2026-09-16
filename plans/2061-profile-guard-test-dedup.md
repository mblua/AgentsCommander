# Plan #2061: force-enabled Delete Profile regression test and #2059 fixture dedup

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/2061 (OPEN)
- Repo: `repo-AgentsCommander`; branch `test/2061-profile-guard-test-dedup`
- Base (frozen at authoring, 2026-09-16 UTC): `main` = branch HEAD = remote branch head =
  `5203c4e3b0b5909fdc12337194c886b1514966c3` (`git rev-parse HEAD`; `git ls-remote origin
  refs/heads/test/2061-profile-guard-test-dedup`). Tracked tree clean. Every line number below
  refers to that SHA; if a quoted line no longer matches, re-anchor on the quoted text, never on
  the number.
- Class: Lite (band 1-25), one phase, no partition. Owner `ac-dev-webpage-ui-v4`; reviewer
  `ac-dev-rust-grinch-v4`; coordinator `ac-tech-lead-v4`.
- Canonical plan: this file. Root `.gitignore` ignores `/plans/`, so commit with
  `git add -f plans/2061-profile-guard-test-dedup.md`.
- Exactly one file changes: `src/sidebar/components/SettingsModal.automation.test.ts`. Production
  code is not modified; the mutation probes in section 6 edit `SettingsModal.tsx` temporarily and
  restore it, and `git diff` at the end must contain no production file.

## 1. Problem and verified cause

### Item 1 - the runtime guard has no committed regression test

E1. `removeProfileLetter` starts at `SettingsModal.tsx:1409`. Its early return is
    `if (slotHolderIds(letter).length > 0) return;` at `:1413`, before `setDraftDirty(true)`
    (`:1414`) and before every store write. `slotHolderIds` (`:1383-1390`) reads only
    `settings.data.codingAgentProfiles`, so a live holder makes the function a no-op.

E2. The Delete button renders at `SettingsModal.tsx:3802-3816`:
    `onClick={() => removeProfileLetter(letter)}` at `:3804`,
    `disabled={slotHolderIds(letter).length > 0}` at `:3805`,
    `data-ac-state="blocked"` at `:3806`, `data-ac-testid={`${cardId}.deleteProfile`}` at
    `:3812`, and the notice node `<Show>` at `:3817-3825` (`:3820` testid). The Clear button
    (`onClick={() => clearProfileCell(agent.id, letter)}` at `:3793`,
    `data-ac-testid={`${cardId}.clearCell`}` at `:3796`) renders before it at `:3791-3801`.

E3. The committed guard test `blocks Delete Profile while the slot is configured in another
    agent` (`SettingsModal.automation.test.ts:1462-1537`, T0) asserts `deleteBtn.disabled === true`
    at `:1513` and then calls `deleteBtn.click()` at `:1519` on the still-disabled button. jsdom
    does not run a click's activation behavior on a disabled form control, so the handler is never
    invoked and T0 pins the attribute, not the function. The guard line at `:1413` is therefore
    dead in the committed suite: removing it keeps every committed test green.

E4. The issue records the reviewer's scratch probe: force-enable the button, assert `disabled`
    is still `false` immediately before the click, fire `.click()` and a bubbling `MouseEvent`,
    observe both agent cards intact and the saved payload still holding `profileSlots.B` and both
    agents' B cells, with a passing Clear-button control proving the click path is live. The
    probe was deleted instead of committed.

### Item 2 - duplicated fixture setup

E5. SonarCloud failed PR #2059 on one condition: `new_duplicated_lines_density` 51.5% against a
    3% threshold. All of it is in this test file: 301 of 393 new lines flagged (76.6%). The
    production files are at 0%.

E6. The six new #2059 cases are T1-T6 at `:1540`, `:1607`, `:1661`, `:1732`, `:1797`, `:1866`,
    plus the modified T0 at `:1462`. Every one repeats the same fixture skeleton: the two-agent
    literal (`id`/`label`/`command`/`color`/`envs`/`isolatedHome`), the `codingAgentProfiles`
    skeleton (`schemaVersion: 2`, the same `profileSlots A/B`, `defaultProfileByAgent: {}`,
    `profileLabelsByAgent`, `profilesByAgent`) and the six-line mount boilerplate
    (`createElement` / `append` / `render` / `settle` / `enterTwoRails`). T6 (`:1866`) is the only
    one that does not call `enterTwoRails()`.

E7. Baseline measured at the frozen base: `npx vitest run
    src/sidebar/components/SettingsModal.automation.test.ts` -> `Test Files 1 passed (1)`,
    `Tests 64 passed (64)`, exit 0.

## 2. Decisions

**D1 - Item 1 is one new test, not a rewrite of T0.** T0 stays as it is (it still pins the
disabled attribute). A new `it` is added next to it, named
`#2061: a force-enabled Delete Profile click cannot delete a held slot`, with this exact shape:

1. Fixture: both live agents hold B (same shape as T0), two rails.
2. Assert the button is blocked (`disabled`, `data-ac-state="blocked"`, notice present).
3. `deleteBtn.disabled = false;` then `expect(deleteBtn.disabled).toBe(false);` **immediately
   before the click** - if the force-enable does not stick, the test fails here instead of
   passing vacuously.
4. `deleteBtn.click(); await settle();` - the same programmatic click jsdom dispatches for a
   live button.
5. Assert no mutation: both B cards still exist and the notice still renders. Do **not** assert
   the button re-locked itself: the early return writes nothing, so Solid re-runs no reactive
   expression in that window and `disabled` is still `false`.
6. Save and assert the payload is intact: `profileSlots.B`, `profilesByAgent.codex.B` and
   `profilesByAgent.claude.B` present, `profileSlots.A` present.
7. Positive control with the same `.click()` mechanism: clear codex's B -> the notice now names
   only `Claude Code`; clear claude's B -> the same Delete button is enabled and the notice is
   gone; click Delete -> both B cards are gone. This proves the click path reaches
   `removeProfileLetter` and deletes once the guard condition is false, so step 5's no-op can
   only be the guard.

**D2 - Two mutation probes are part of the deliverable evidence** (section 6): P1 removes the
early return at `SettingsModal.tsx:1413`; P2 removes only the `deleteBtn.disabled = false;`
assignment while keeping the `expect(deleteBtn.disabled).toBe(false);` assertion, so the test
must fail instead of silently degrading to a disabled-button no-op. Neither is a permanent edit.

**D3 - One shared fixture builder for all eight guard cases.** `profileGuardSettings` plus
`profileCell` plus the two agent constants (section 4.1) build every fixture: T0, T1-T6 and the
new test. The builder's default is "two live agents, A configured, B empty"; a case overrides
only the cells/labels/agents that differ. T0 is included as the seventh copy of the identical
skeleton; leaving one copy in place would recreate the duplication being removed. No other test
in the file is touched.

**D4 - Mount and save boilerplate are helpers too.** `mountProfilesSection(twoRails = true)`
replaces the six-line mount sequence and `saveAndReadDraft()` replaces the three-line
click-save-read sequence in the eight guard tests only.

**D5 - Item 3 of the issue (`flex: 1 0 100%`) is out of scope.** It is a note for the next
reader. Nothing in this plan changes CSS or records it beyond this sentence.

## 3. In scope / out of scope

In scope (one file):

- `src/sidebar/components/SettingsModal.automation.test.ts`: the new #2061 test, the three
  helpers of section 4.1, and the fixture/mount/save rewrite of T0 and T1-T6.

Out of scope (binding):

- `SettingsModal.tsx`, `profile-utils.ts`, `sidebar.css`, every other source file, `src-tauri/`.
  The guard behaviour is frozen by #2057 and is not under test design here.
- Every other test in `SettingsModal.automation.test.ts`: its fixture, name, assertions and
  helpers stay byte-identical. Other sections' tests keep their own `settings({...})` fixtures.
- Item 3 of the issue (CSS/plan-text note).
- SonarCloud configuration, quality-gate settings, workflows, dependencies.

## 4. Exact changes

### 4.1 Helpers (insert after `enterTwoRails`, `:262-267`, before `describe`, `:269`)

Extend the existing type import at `:4`:

```ts
import type { AgentConfig, AppSettings, ProfileCellConfig, SettingsSnapshot } from "../../shared/types";
```

Insert:

```ts
// #2061 - shared fixture for the eight #2057/#2061 guard cases. Every one renders
// the same two rails and the same A/B slot skeleton; only which agents hold B
// (cells and/or labels) varies.
const CODEX_RAIL_AGENT: AgentConfig = {
  id: "codex",
  label: "Codex",
  command: "codex",
  color: "#10b981",
  envs: [],
  isolatedHome: false,
};

const CLAUDE_RAIL_AGENT: AgentConfig = {
  id: "claude",
  label: "Claude Code",
  command: "claude",
  color: "#d97706",
  envs: [],
  isolatedHome: false,
};

function profileCell(command: string, enabled = true): ProfileCellConfig {
  return { enabled, command, env: {}, notes: "" };
}

function profileGuardSettings({
  agents = [CODEX_RAIL_AGENT, CLAUDE_RAIL_AGENT],
  profilesByAgent = {
    codex: { A: profileCell("codex") },
    claude: { A: profileCell("claude") },
  },
  profileLabelsByAgent = {},
}: {
  agents?: AgentConfig[];
  profilesByAgent?: Record<string, Record<string, ProfileCellConfig>>;
  profileLabelsByAgent?: Record<string, Record<string, string>>;
} = {}): SettingsSnapshot {
  return settings({
    agents,
    codingAgentProfiles: {
      schemaVersion: 2,
      profileSlots: { A: { label: "" }, B: { label: "fast" } },
      defaultProfileByAgent: {},
      profilesByAgent,
      profileLabelsByAgent,
    },
  });
}

// #2061 - the guard cases mount the same profiles section; two rails except the
// one-agent dead-id case. Returns the disposer.
async function mountProfilesSection(twoRails = true): Promise<() => void> {
  const root = document.createElement("div");
  document.body.append(root);
  const dispose = render(
    () => SettingsModal({ onClose: () => {}, section: "profiles" }),
    root,
  );
  await settle();
  if (twoRails) await enterTwoRails();
  return dispose;
}

// #2061 - save and read the first persisted draft.
async function saveAndReadDraft(): Promise<AppSettings | undefined> {
  byTestId<HTMLButtonElement>("settings.save").click();
  await settle();
  return vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0];
}
```

`vi.mocked(SettingsAPI.saveDraft).mock.calls[0]?.[0]` is `AppSettings | undefined` because the
mock's parameter is `AppSettings` (`src/shared/ipc.ts:373`), so the return type is exact.

### 4.2 Per-test fixture calls (replace each test's inline fixture and mount/save lines)

| Test (anchor on its exact name) | `vi.mocked(SettingsAPI.get).mockResolvedValueOnce(...)` argument |
|---|---|
| `blocks Delete Profile while the slot is configured in another agent` (T0) | `profileGuardSettings({ profilesByAgent: { codex: { A: profileCell("codex"), B: profileCell("codex --profile fast") }, claude: { A: profileCell("claude"), B: profileCell("claude --model opus") } } })` |
| `deletes the slot when it is empty in every live coding agent` (T1) | `profileGuardSettings()` |
| `blocks the slot delete when only the selected agent holds it` (T2) | `profileGuardSettings({ profilesByAgent: { codex: { A: profileCell("codex"), B: profileCell("codex --profile fast") }, claude: { A: profileCell("claude") } }, profileLabelsByAgent: { codex: { B: "fast" } } })` |
| `clears the one holder and then deletes the slot` (T3) | identical to T2 |
| `Clear in this agent leaves the other agents and the slot untouched` (T4) | identical to T0 |
| `does not treat a disabled empty residue cell as a holder` (T5) | `profileGuardSettings({ profilesByAgent: { codex: { A: profileCell("codex") }, claude: { A: profileCell("claude"), B: profileCell("", false) } } })` |
| `a configured dead agent id does not block the slot delete` (T6) | `profileGuardSettings({ agents: [CODEX_RAIL_AGENT], profilesByAgent: { codex: { A: profileCell("codex") }, ghost: { A: profileCell("ghost"), B: profileCell("ghost --profile fast") } } })` |

Mount and save wiring per test:

- T0, T1, T2, T3, T4, T5: `const dispose = await mountProfilesSection();`.
- T6: `const dispose = await mountProfilesSection(false);` (it never calls `enterTwoRails()`).
- Save, where the test already saves: T0, T1, T3, T4, T5, T6 and the new test use
  `const saved = await saveAndReadDraft();`; T2 does not save and does not change.

Nothing else in these tests changes: every existing name, comment, assertion, click, settle and
`dispose()` call stays exactly as committed. The only deleted lines are the inline `agents` array,
the inline `codingAgentProfiles` object, the mount boilerplate and the three save lines.

### 4.3 New test (insert after T0's closing `});` at `:1537`, before the `// #2057 - T1` comment at `:1539`)

```ts
  // #2061 - the guard is a runtime invariant, not just the disabled attribute.
  // A force-enabled click reaches removeProfileLetter, and the early return must
  // still refuse to delete. The Clear/Delete control afterwards proves the click
  // path is live in this harness.
  it("#2061: a force-enabled Delete Profile click cannot delete a held slot", async () => {
    vi.mocked(SettingsAPI.get).mockResolvedValueOnce(profileGuardSettings({
      profilesByAgent: {
        codex: {
          A: profileCell("codex"),
          B: profileCell("codex --profile fast"),
        },
        claude: {
          A: profileCell("claude"),
          B: profileCell("claude --model opus"),
        },
      },
    }));
    const dispose = await mountProfilesSection();

    const deleteBtn = byTestId<HTMLButtonElement>("settings.profileCard.0.B.deleteProfile");
    expect(deleteBtn.disabled).toBe(true);
    expect(deleteBtn.getAttribute("data-ac-state")).toBe("blocked");

    // Bypass the disabled attribute, then prove at the instant of the click that
    // the button is really clickable, so the no-op below can only be the guard.
    deleteBtn.disabled = false;
    expect(deleteBtn.disabled).toBe(false);
    deleteBtn.click();
    await settle();

    // The early return fired before any store write: cards, notice and draft intact.
    expect(byTestId("settings.profileCard.0.B")).toBeTruthy();
    expect(byTestId("settings.profileCard.1.B")).toBeTruthy();
    expect(byTestId("settings.profileCard.0.B.deleteProfile.blocked")).toBeTruthy();

    const saved = await saveAndReadDraft();
    expect(saved?.codingAgentProfiles.profileSlots.B).toBeTruthy();
    expect(saved?.codingAgentProfiles.profilesByAgent.codex?.B).toBeTruthy();
    expect(saved?.codingAgentProfiles.profilesByAgent.claude?.B).toBeTruthy();
    // The A baseline is untouched.
    expect(saved?.codingAgentProfiles.profileSlots.A).toBeTruthy();

    // Positive control - same harness, same .click(): emptying both holders
    // enables the same button, and its click then deletes the slot.
    byTestId<HTMLButtonElement>("settings.profileCard.0.B.clearCell").click();
    await settle();
    expect(byTestId("settings.profileCard.0.B.deleteProfile.blocked").textContent).toBe(
      "Still configured in: Claude Code. Empty this profile in every coding agent before deleting the slot.",
    );

    byTestId<HTMLButtonElement>("settings.profileCard.1.B.clearCell").click();
    await settle();
    const unblockedBtn = byTestId<HTMLButtonElement>("settings.profileCard.0.B.deleteProfile");
    expect(unblockedBtn.disabled).toBe(false);
    unblockedBtn.click();
    await settle();

    expect(document.querySelector('[data-ac-testid="settings.profileCard.0.B"]')).toBeNull();
    expect(document.querySelector('[data-ac-testid="settings.profileCard.1.B"]')).toBeNull();

    dispose();
  });
```

## 5. Test behaviour and edge cases

| Case | Expected |
|---|---|
| Blocked slot, guard present, force-enabled click | no state change: both B cards, notice, draft payload intact |
| Blocked slot, guard removed (P1) | click deletes B; the first `settings.profileCard.0.B` query throws and the test fails |
| Force-enable assignment removed (P2) | `expect(deleteBtn.disabled).toBe(false)` fails; no vacuous pass |
| Clear codex's B with claude still holding | notice text becomes `Still configured in: Claude Code. ...`; Delete stays blocked |
| Clear the last holder | notice unmounts, Delete enables, click removes B from both rails |
| `deleteBtn.disabled` after the no-op click | left `false` on purpose; no store write means no re-render, and D1.5 forbids asserting a re-lock |
| Test counts | file goes from 64 to 65 tests; no other count changes |
| Other tests in the file | untouched; T0 keeps its disabled-button click and all its assertions |

## 6. Verification

Run from the repo root
`D:\0_repos\AgentsCommander_iac\.ac\room-21-ac-dev-team-v4\repo-AgentsCommander`.
`BASE=5203c4e3b0b5909fdc12337194c886b1514966c3`.

| Step | Command | Expected | On failure |
|------|---------|----------|-----------|
| V1 | `npm run typecheck` | exit 0 | fix types; do not widen scope |
| V2 | `npx vitest run src/sidebar/components/SettingsModal.automation.test.ts` | exit 0, `Test Files 1 passed (1)`, `Tests 65 passed (65)` | fix the test |
| V2b | `npm test` (the `frontend-regression` CI mirror, whole suite) | exit 0; no count claimed here | re-run the failing file at `$BASE`; a failure that does not reproduce there blocks delivery |
| V3 | `npx vitest run src/sidebar/components/SettingsModal.automation.test.ts -t "force-enabled"` | exit 0, `Tests 1 passed \| 64 skipped (65)` | fix the test |
| V4 | P1: delete `if (slotHolderIds(letter).length > 0) return;` at `SettingsModal.tsx:1413`, run V3, then `git restore src/sidebar/components/SettingsModal.tsx` | before restore: exit 1, `Tests 1 failed \| 64 skipped (65)`, failing the new test at the missing `settings.profileCard.0.B`; after restore V2 is green | the test is not pinning the guard; report, do not weaken the probe |
| V5 | P2: delete only the `deleteBtn.disabled = false;` line (keep its assertion), run V3, then `git restore src/sidebar/components/SettingsModal.tsx` | before restore: exit 1, `Tests 1 failed \| 64 skipped (65)` at `expect(deleteBtn.disabled).toBe(false)`; after restore V2 is green | the test can pass vacuously; report it |
| V6 | Dedup proxy on the final diff: `git diff $BASE...HEAD -- src/sidebar/components/SettingsModal.automation.test.ts \| grep '^+' \| grep -c 'schemaVersion: 2'`; same with `profileSlots: { A: { label: "" }, B: { label: "fast" } }` and `defaultProfileByAgent: {}` | each count is exactly `1` (the builder) | a second copy exists; finish the refactor |
| V7 | `git status --porcelain` after V4/V5 | only `M src/sidebar/components/SettingsModal.automation.test.ts` (plus the plan commit already on the branch) | investigate before committing |
| V8 | SonarCloud on the PR head | `new_duplicated_lines_density` < 3% (was 51.5% on #2059) | locally reproducible proxies are V6; if the gate still fails, report the analyzer's file/block and stop |

Owner of V1-V7: the implementer, before opening the PR. Owner of V8: SonarCloud on the exact PR
head; it is not runnable locally and is not replaced by V6.

## 7. Acceptance criteria

1. `SettingsModal.automation.test.ts` contains exactly one new test, named
   `#2061: a force-enabled Delete Profile click cannot delete a held slot`, and no other test is
   added or renamed.
2. The new test force-enables the Delete button, asserts `disabled === false` immediately before
   the click, clicks it, asserts both B cards and the notice survived, and saves to assert
   `profileSlots.B`, `profilesByAgent.codex.B` and `profilesByAgent.claude.B` intact.
3. The new test's control clears both holders, asserts the notice shrinks to `Claude Code` and
   then disappears, then clicks the same Delete button and asserts both B cards are gone.
4. P1 (guard line removed) fails the new test; P2 (force-enable removed) fails it at the
   `disabled` assertion; both restores leave the tree clean (V4, V5, V7).
5. V2 reports exactly `Tests 65 passed (65)`; the other 64 tests are unmodified behaviourally.
6. T0 and T1-T6 build their fixtures through `profileGuardSettings`, mount through
   `mountProfilesSection`, and save through `saveAndReadDraft`; no inline `agents` array or
   `codingAgentProfiles` literal remains in those seven tests.
7. V6 shows exactly one added line for each skeleton literal in the test-file diff.
8. SonarCloud on the PR reports `new_duplicated_lines_density` < 3%.
9. The final diff contains no production file.

## 8. Implementation order

1. Apply 4.1, 4.2 and 4.3 in one pass (fixture refactor and new test together, per the issue).
2. Run V1, V2, V2b, V3; then V4 and V5 probes with restores; then V6 and V7.
3. Commit the single test file with `test(2061): guard regression via force-enabled click and
   dedup #2059 fixtures`; the plan is already committed on the branch.
4. Report to the coordinator: V2/V3 output, both probe outputs with their restores, V6 counts,
   and the PR link for V8. The reviewer owns the proof audit.

## Plan Contract

No TBD, no open decision, no competing alternative. D1 fixes the test shape and its exact
assertions; D3/D4 fix the helper names, signatures and call sites; section 4 fixes every changed
line; section 6 fixes every command and expected count. Production files are out of scope, the
guard behaviour is frozen by #2057, and the only authoritative sources of the two residuals are
the issue and the frozen base `5203c4e3b0b5909fdc12337194c886b1514966c3`.
