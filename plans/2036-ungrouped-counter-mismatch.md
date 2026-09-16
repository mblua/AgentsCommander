# Plan #2036: One Ungrouped membership rule for the rail counter and the panel list

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/2036 (OPEN)
- Repo: `repo-AgentsCommander`; branch `fix/2036-ungrouped-counter-mismatch`
- Base (frozen at authoring, 2026-09-15 UTC): `main` = branch HEAD = remote branch head =
  `329ab94ed011a427b70c760093d5b542b4987d94` (`git rev-parse HEAD`; `git ls-remote origin
  refs/heads/fix/2036-ungrouped-counter-mismatch`). Tracked tree clean. Every line number below
  refers to that SHA; if a quoted line no longer matches, re-anchor on the quoted text, never on
  the number.
- Class: Lite (band 1-25), one phase, no partition. Owner `ac-dev-webpage-ui-v4`; reviewer Grinch;
  coordinator `ac-tech-lead-v4`.
- Canonical plan: this file. Root `.gitignore` ignores `/plans/`, so commit with
  `git add -f plans/2036-ungrouped-counter-mismatch.md`.
- Frontend only. No backend, IPC, `src/shared/types.ts`, dependency, workflow, release, or
  migration change. 3 source files, 2 amended test files, 1 new test file.

## 1. Problem and verified cause

Reported: with Ungrouped selected, the rail button counts e.g. `4/19` while the project panel
lists 10 rooms.

The two sides derive "ungrouped" from different rules today:

- Rail, `src/sidebar/components/WorkgroupGroupRail.tsx:406-410` (`ungroupedWorkgroups`): a room is
  ungrouped when it matches no `config().groups` entry via `groupMatches` (`:78-83`). It never
  reads `config().nonStop`.
- Panel, `src/sidebar/components/ProjectPanel.tsx:1117-1124` (`workgroupMatchesAnyGroup`, consumed
  by `groupPredicate` `:1125-1134`, branch `:1128`): ungrouped requires no compiled group match
  **and** no `nonStopMatchesWorkgroup` match.

Cause: every room matched only by the Alert me! (NonStop) regex is counted by the rail and hidden
by the panel. The reported 19 vs 10 gap is consistent with 9 NonStop-only rooms. The user's project
directory is not available in this repo, so this plan proves the mechanism with a fixture, not the
user's exact numbers; no claim is made about the exact 9.

Secondary checks the coordinator asked for (all at `329ab94e`):

- Invalid or oversized group regex: rail `groupMatches` and panel `groupMatchesWorkgroup` both go
  through `compileGroupRegex` (`workgroup-groups.ts:304-311`), which returns `null` for invalid
  syntax and for `> MAX_GROUP_REGEX_LENGTH` (1024); both treat `null` as non-match. Identical.
- Id length cap: rail `:80` and panel `:1110-1111` both use `id.length` (UTF-16 code units);
  `nonStopMatchesWorkgroup` (`workgroup-groups.ts:313-318`) and every store guard
  (`addWorkgroupToGroup` `:586`, `removeWorkgroupFromGroup` `:611`, `createGroupForWorkgroup`
  `:636`, `addWorkgroupToNonStop` `:658`, `removeWorkgroupFromNonStop` `:684`) use `charLength`
  (Unicode code points). Consequence on `main`: a legal 160-code-point astral room name (up to 320
  UTF-16 units) can be added to a group by the store and its exact regex matches, yet rail and
  panel refuse to test the id, so the room can never leave Ungrouped. The panel's current
  `workgroupMatchesAnyGroup` is already internally split across both readings. This is the second
  latent divergence in the same rule family; D2 closes it.
- The panel's other consumers of the same predicate (`naturalCoordinatorItems` `:2111`, selected
  room `:1152`) read `groupVisibleWorkgroups`, so they follow the panel rule automatically. The
  rail is the only second implementation of the ungrouped rule.

No test on `main` pins the buggy rail value: `WorkgroupGroupRail.test.tsx` has no NonStop-plus-
Ungrouped counter assertion, and the favorites NonStop cases use the default `(?!)` regex.

## 2. Decision (single rule, single home)

### D1 — the canonical rule lives in `src/sidebar/stores/workgroup-groups.ts`

```
a room is UNGROUPED  ⇔  id over MAX_GROUP_MATCH_ID_LENGTH
                        OR (no compiled group regex matches AND the NonStop regex does not match)
```

The NonStop term applies regardless of `nonStop.show`, which is the panel behavior already locked
by `ProjectPanel.groups-filter.test.tsx:240` (`hidden_alert_me_match_stays_excluded_from_ungrouped`).

`workgroup-groups.ts` owns group config semantics, regex compilation, NonStop matching and every
cap; both components already import from it. New exports used by both sides:

- `compileWorkgroupGroups(groups)` — compiles the configured group regexes once per config.
- `groupMatchesWorkgroup(compiled, groupId, wg)` — regular-group membership (group buttons and
  panel group selection), cap + compiled regex.
- `isUngroupedWorkgroup(compiled, nonStop, wg)` — **the** Ungrouped rule; the only exported
  ungrouped predicate. Neither component may re-derive it.

Alternatives closed:

- (a) Panel includes NonStop-matched rooms in Ungrouped: rejected. It contradicts the locked tests
  `alert_me_only_visible_is_excluded_from_ungrouped` (`:174`) and
  `hidden_alert_me_match_stays_excluded_from_ungrouped` (`:240`), and lists the same room both
  under Alert me! and Ungrouped.
- (b) Copy the NonStop term into the rail's local filter: rejected. That is exactly the duplicate
  token that produced #2036.
- (c) New matcher module: rejected. No third consumer, YAGNI; the store is the semantic owner.

### D2 — one cap literal: `charLength` (code points)

The two component copies of the cap move into the store helper and switch from `.length`
(UTF-16 units) to `charLength`, the convention already used by `nonStopMatchesWorkgroup` and every
store guard. Behavior changes only for room ids longer than 160 UTF-16 units, reachable only by
non-BMP (astral) names of at most 160 code points; those rooms can already be added to a group
through the store, so this makes "added" and "matches" agree. It is deliberately in scope because
leaving two cap readings inside one rule is the same class of drift this plan removes. It is
covered by a dedicated test and must be called out in the PR description.

## 3. In scope / out of scope

In scope:

- `src/sidebar/stores/workgroup-groups.ts`: canonical helpers (D1, D2).
- `src/sidebar/components/WorkgroupGroupRail.tsx`: delete the local rule, call the shared one.
- `src/sidebar/components/ProjectPanel.tsx`: delete the local rule, call the shared one.
- Tests: `src/sidebar/stores/workgroup-groups.test.ts`,
  `src/sidebar/components/WorkgroupGroupRail.test.tsx`, new
  `src/sidebar/App.ungrouped-counter.test.tsx`.

Out of scope (binding):

- The counter format `working/total` (`buttonContent`, `WorkgroupGroupRail.tsx:102-114`) and the
  working-dot semantics.
- NonStop visibility/favorite/toggle semantics, `WorkgroupGroupsModal`, the groups store's save
  and validation behavior, `non-stop-watchdog-client`, rail selection normalization.
- The panel search filter: the panel's Rooms count is `filteredWorkgroups().length`
  (`ProjectPanel.tsx:3005`), which shrinks with an active search text; the rail counter never
  reflected search and still must not. The new App regression test runs with no search text.
- `src-tauri/`, `src/shared/types.ts`, `src/shared/ipc.ts`.

## 4. Exact changes

### 4.1 `src/sidebar/stores/workgroup-groups.ts` (insert after `nonStopMatchesWorkgroup`, `:318`)

```ts
export interface CompiledWorkgroupGroup {
  group: WorkgroupGroup;
  regex: RegExp | null;
}

/** Compiles each configured group once, so a caller can test many rooms cheaply. */
export function compileWorkgroupGroups(
  groups: readonly WorkgroupGroup[]
): CompiledWorkgroupGroup[] {
  return groups.map((group) => ({ group, regex: compileGroupRegex(group) }));
}

function canTestGroupMatchId(wg: AcWorkgroup): boolean {
  return charLength(groupMatchId(wg)) <= MAX_GROUP_MATCH_ID_LENGTH;
}

/** Regular-group membership: cap + compiled group regex. */
export function groupMatchesWorkgroup(
  compiled: readonly CompiledWorkgroupGroup[],
  groupId: string,
  wg: AcWorkgroup
): boolean {
  if (!canTestGroupMatchId(wg)) return false;
  const entry = compiled.find((candidate) => candidate.group.id === groupId);
  return !!entry?.regex?.test(groupMatchId(wg));
}

/**
 * #2036 — THE Ungrouped membership rule. A room is ungrouped when its id is over
 * MAX_GROUP_MATCH_ID_LENGTH, or when no compiled group regex and no Alert me!
 * (NonStop) regex matches it (regardless of `nonStop.show`). The rail counter and
 * the panel list MUST both call this; never re-derive either side locally.
 */
export function isUngroupedWorkgroup(
  compiled: readonly CompiledWorkgroupGroup[],
  nonStop: NonStopGroupConfig | null | undefined,
  wg: AcWorkgroup
): boolean {
  if (!canTestGroupMatchId(wg)) return true;
  const id = groupMatchId(wg);
  if (compiled.some((entry) => !!entry.regex?.test(id))) return false;
  return !nonStop || !nonStopMatchesWorkgroup(nonStop, wg);
}
```

`WorkgroupGroup` and `NonStopGroupConfig` are already imported at the top of the file.

### 4.2 `src/sidebar/components/WorkgroupGroupRail.tsx`

- Imports (`:9-18`): remove `MAX_GROUP_MATCH_ID_LENGTH`, `compileGroupRegex`, `groupMatchId`; add
  `compileWorkgroupGroups`, `groupMatchesWorkgroup`, `isUngroupedWorkgroup` and
  `type CompiledWorkgroupGroup`. Keep `nonStopDisplayName`, `nonStopMatchesWorkgroup`,
  `workgroupGroupsStore`, `type WorkgroupGroupSelection`.
- Delete the local `groupMatches` (`:78-83`).
- `groupButtonFor` (`:156`): add a last parameter `compiled: readonly CompiledWorkgroupGroup[]`
  and replace the filter with
  `project.workgroups.filter((wg) => groupMatchesWorkgroup(compiled, group.id, wg))`.
- Favorites memo (`:322-345`): after `const config = workgroupGroupsStore.config(project.path);`
  add `const compiled = compileWorkgroupGroups(config.groups);` and pass `compiled` at the
  `groupButtonFor` call (`:340`).
- `ProjectRailSection`: new memo
  `const compiledGroups = createMemo(() => compileWorkgroupGroups(config().groups));`.
  `ungroupedWorkgroups` (`:406-410`) becomes

  ```ts
  const ungroupedWorkgroups = createMemo(() =>
    props.project.workgroups.filter((wg) =>
      isUngroupedWorkgroup(compiledGroups(), config().nonStop, wg)
    )
  );
  ```

  and the project-rail `groupButtonFor` call (`:445`) passes `compiledGroups()` as the last
  argument.

### 4.3 `src/sidebar/components/ProjectPanel.tsx`

- Imports (`:88-95`): remove `MAX_GROUP_MATCH_ID_LENGTH` and `groupMatchId`; add
  `compileWorkgroupGroups`, `groupMatchesWorkgroup`, `isUngroupedWorkgroup`. Keep
  `compileGroupRegex` (used at `:1669`), `nonStopMatchesWorkgroup` (used at `:1129-1131` and
  `:1598`).
- Replace `compiledGroups` (`:1107-1109`) with
  `const compiledGroups = createMemo(() => compileWorkgroupGroups(groupsConfig().groups));`.
- Delete `canTestGroupMatchId` (`:1110-1111`), the local `groupMatchesWorkgroup` (`:1112-1116`)
  and `workgroupMatchesAnyGroup` (`:1117-1124`).
- `groupPredicate` (`:1125-1134`):

  ```ts
  const groupPredicate = (wg: AcWorkgroup) => {
    const selected = selectedGroup();
    if (selected.kind === "all") return true;
    if (selected.kind === "ungrouped")
      return isUngroupedWorkgroup(compiledGroups(), groupsConfig().nonStop, wg);
    if (selected.kind === "nonstop") {
      const ns = groupsConfig().nonStop;
      return !!ns && nonStopMatchesWorkgroup(ns, wg);
    }
    return groupMatchesWorkgroup(compiledGroups(), selected.id, wg);
  };
  ```

- `groupAlreadyMatches` (`:1578-1579`) becomes
  `groupMatchesWorkgroup(compiledGroups(), groupId, wg)`.

Expected diff: 3 source files, no other module moved, no new dependency, no state or IPC shape.

## 5. Edge cases (all verified by code path in section 4)

| Case | Result | Both sides equal? |
|---|---|---|
| Room matches only NonStop, `show: true` | not ungrouped | yes (the #2036 fix) |
| Room matches only NonStop, `show: false` | not ungrouped (panel test `:240` is the authority; new rail test locks it) | yes |
| Room matches a group and NonStop | not ungrouped | yes |
| NonStop regex invalid or `> 1024` chars | NonStop term false → room ungrouped | yes |
| Group regex invalid or `> 1024` chars | treated as non-match → room ungrouped | yes |
| Room id over 160 code points | ungrouped even if a regex matches the text | yes |
| 160-code-point astral name matching a group | grouped (D2; previously ungrouped on both sides) | yes |
| No groups and no NonStop | every room ungrouped (`no_explicit_match_remains_ungrouped`, `:220`) | yes |
| Config not loaded yet (defaults: `groups: []`, `nonStop: null`) | every room ungrouped, as on `main` | yes |
| Search text active in the panel | panel list shrinks; rail counter unchanged | unchanged behavior, out of scope |

## 6. Tests

### T1 — required positive control: rail hides NonStop-only rooms (`WorkgroupGroupRail.test.tsx`)

Fixture: `groupsConfig({ groups: [{ id: "ui", name: "UI", regex: exactGroupRegexForWorkgroup("wg-1-dev-team") }], nonStop: { ...defaultNonStop(), show: true, regex: exactGroupRegexForWorkgroup("wg-2-rust-team") } })`
on the existing `project()` fixture (wg-1, wg-2, wg-3). No sessions.

```ts
it("#2036 excludes Alert me!-only rooms from the Ungrouped counter", async () => {
  ...
  await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "nonstop", "ui"]));
  expect(target("workgroupGroups.button.ungrouped").textContent).toContain("0/1"); // wg-3 only
  expect(target("workgroupGroups.button.nonstop").textContent).toContain("0/1");   // wg-2
});
```

On `main` this fails at the first counter assertion: rail renders `0/2` (wg-2 + wg-3) because
`ungroupedWorkgroups` ignores NonStop. Exact expected/received must be captured raw (section 7).

### T2 — hidden NonStop still excludes (`WorkgroupGroupRail.test.tsx`)

Same fixture but `nonStop: { ...defaultNonStop(), show: false, regex: exactGroupRegexForWorkgroup("wg-2-rust-team") }`;
expected button order `["all", "ungrouped", "ui"]` and `ungrouped` counter `0/1`. On `main` it
renders `0/2`. This mirrors the panel's locked `hidden_alert_me_match_stays_excluded_from_ungrouped`
test and keeps the two sides pinned to the same rule.

### T3 — canonical rule unit matrix (`workgroup-groups.test.ts`, next to the `nonStopMatchesWorkgroup` test `:332`)

`isUngroupedWorkgroup` over a compiled list: NonStop-only → `false`; `show:false` NonStop-only →
`false`; regular-group match → `false`; both match → `false`; invalid group regex → `true`;
invalid NonStop regex → `true`; no config match → `true`; `nonStop` `null`/`undefined` → `true`;
id of 200 code points → `true`; `"𝕨".repeat(160)` (160 code points, 320 UTF-16 units) with an
exact group regex → `false`, and the same name over 160 code points → `true`. Plus
`compileWorkgroupGroups` mapping invalid/oversized regexes to `regex: null`.

T3 cannot be red on `main`: the helper does not exist there, so its "red" state is a compile
error. Do not present it as a pre-fix failure; T1/T2/T4 are the positive controls.

### T4 — rail/panel agreement at App level (new `src/sidebar/App.ungrouped-counter.test.tsx`)

Renders `SidebarApp` with one project (`C:\Project`) and three rooms, using the
`App.order-lock.test.tsx` `setupTransport` shape (`get_settings` with `projectPaths: [projectPath]`,
`open_project`, `discover_project`, `get_project_groups` with the T1 config, `search_repos: []`,
`list_sessions: []`, `list_detached_sessions: []`, `telegram_list_bridges: []`).

Steps and assertions:

1. `await waitFor(() => expect(target("workgroupGroups.button.ungrouped")).not.toBeNull())` and
   click it (or `workgroupGroupsStore.select(projectPath, { kind: "ungrouped" })` once the config
   is loaded).
2. Read the Rooms header count with the locator already used by
   `ProjectPanel.regex-filter.test.tsx:387-392`:
   the `.ac-wg-header` whose `.ac-wg-name` text is `Rooms`, then its `.ac-team-count`.
3. Assert `roomsCount() === "1"` (only wg-3) and the rail button text contains `0/1`.
4. Assert the same value is compared directly, e.g.
   `expect(target("workgroupGroups.button.ungrouped").textContent).toContain(`/${roomsCount()}`)`,
   so the test states agreement, not only two constants.

On `main` this is red: the panel count is already `1` while the rail renders `0/2`; the agreement
assertion fails (or the `0/1` containment fails).

### Existing tests that must stay green (they are the panel's authority)

`ProjectPanel.groups-filter.test.tsx:174`, `:194`, `:220`, `:240`, and the rail `#777` NonStop
counter test `WorkgroupGroupRail.test.tsx:527`. No expectation in them changes.

## 7. Proof protocol (for the reviewer)

1. On the branch at `329ab94e`, add T1, T2 and T4 only (no source change) and run
   `npx vitest run src/sidebar/components/WorkgroupGroupRail.test.tsx src/sidebar/App.ungrouped-counter.test.tsx`.
   Capture the raw red output: test names plus expected/received (`0/1` vs `0/2`, rail `0/2` vs
   Rooms `1`).
2. Implement section 4, add T3. Re-run the same files plus
   `src/sidebar/stores/workgroup-groups.test.ts` and `src/sidebar/components/ProjectPanel.groups-filter.test.tsx`
   → green. Run `npm run typecheck` → clean.
3. Reply to the coordinator with both raw outputs (pre-fix red at base, post-fix green) and the
   commit SHAs. Do not claim T3 as a pre-fix failure.

Commands:

```
npm run typecheck
npx vitest run src/sidebar/stores/workgroup-groups.test.ts \
  src/sidebar/components/WorkgroupGroupRail.test.tsx \
  src/sidebar/components/ProjectPanel.groups-filter.test.tsx \
  src/sidebar/App.ungrouped-counter.test.tsx
```

## 8. Risks and compatibility

- Behavior change surface: only "which rooms the Ungrouped selection shows and counts" and, for
  non-BMP names over the UTF-16 cap, regular-group membership (D2). No persisted data, no IPC, no
  backend, no migration; a revert is a single `git revert` of the source commit.
- Performance: no regression expected. The panel keeps one compile per config (now through
  `compileWorkgroupGroups`); the rail compiles once per project config instead of once per group
  per room, so it gets slightly cheaper.
- Rollout: none. Feature remains frontend-only and applies on reload.

## 9. Implementation order

1. T1 + T2 + T4 committed alone; capture the pre-fix red.
2. Store helper + T3 (D1, D2).
3. Rail and panel switch to the shared rule (D1), D2 cap applied everywhere.
4. Typecheck + focused suites + full `npm test` if time allows; then one implementation commit
   referencing #2036, plus the plan file with `git add -f`.

## Plan Contract

No TBD, no open decision, no competing alternative (D1 alternatives are closed; D2 is decided).
Every touched symbol is named with its file and line at the frozen base. Sole authority for the
rule is `isUngroupedWorkgroup` in `src/sidebar/stores/workgroup-groups.ts`.
