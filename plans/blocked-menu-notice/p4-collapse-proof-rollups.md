# P4: roll the blocked-menu state up through every level that can hide a row

Class: patterned. Owner: frontend. Depends on: P3 (both edit `src/sidebar/components/ProjectPanel.tsx`).
Status: READY_FOR_IMPLEMENTATION

## Objective

The per-row chip is the durable record, but three independent collapses and one regex filter can hide
the row that carries it. Every level that can hide a row must summarise what it hides.

The helper for this is already written and has no caller at all, tests included:
`workgroupHasBlockedMenu` in
`src/sidebar/components/workgroup-session.ts:62-64`, over `replicaHasBlockedMenu` at `:57-60`. Its
raise-hand twin is already wired at `src/sidebar/components/WorkgroupGroupRail.tsx:108`. This phase
mirrors that wiring, one level at a time.

The badge shows whenever the group contains a blocked menu, collapsed or not. That is redundant while
the group is expanded, and it is deliberate: it matches the raise-hand rollup on the rail, which does
not check collapse either, and it covers the regex filter for free, since a filtered-out row is
hidden while the section is still expanded.

## Exact files

1. `src/sidebar/components/ProjectPanel.tsx`
2. `src/sidebar/components/WorkgroupGroupRail.tsx`
3. `src/sidebar/styles/sidebar.css`
4. `src/sidebar/components/ProjectPanel.menu-guard.test.tsx`
5. `src/sidebar/components/WorkgroupGroupRail.blocked-menu.test.tsx` (new)

## Change 1, `src/sidebar/components/WorkgroupGroupRail.tsx`

- Import `workgroupHasBlockedMenu` from `./workgroup-session`, beside the existing
  `workgroupHasRaisedHand` import. This adds a symbol to an import that already exists, so it adds no
  module arc.
- Import `BlockedMenuIcon` from `./BlockedMenuIcon`, the file P3 created, beside the existing
  `import RaiseHandIcon from "./RaiseHandIcon";` on line 24. This one IS a new module arc; it is the
  only one this phase adds.
- Add `blockedMenu: boolean;` to the `GroupButton` type beside `raiseHand: boolean;` on line 37.
- Widen the `Pick<GroupButton, ...>` on line 102 to include `"blockedMenu"`, and add
  `blockedMenu: workgroups.some(workgroupHasBlockedMenu),` beside line 108.
- Add a `blockedMenu` test id to each of the three test-id builders beside lines 117, 126 and 139,
  using the same key shape as the `raiseHand` entry next to it.
- Add `blockedMenu: false,` beside the literal at line 389.
- In the title line, render a second `<Show>` beside the raise-hand one at lines 249-258, before the
  title span, using `BlockedMenuIcon` from P3 with `class="workgroup-group-rail-blocked-menu-icon"`,
  wrapped in a `<span class="workgroup-group-rail-blocked-menu">` with
  `title` and `aria-label` both `"A session is waiting on an interactive menu"`.

Order the two indicators consistently: blocked menu first, then raised hand, in every render site.

## Change 2, `src/sidebar/components/ProjectPanel.tsx`

`workgroupHasBlockedMenu` AND `replicaHasBlockedMenu` both come from the existing
`./workgroup-session` import block ending at line 85. Name both: the quick-group header below uses
`replicaHasBlockedMenu` (`workgroup-session.ts:57-60`) directly, not only the workgroup-level helper.

Add the same badge in three places, each a `<Show>` around a
`<span class="ac-wg-header-blocked-menu">` containing `<BlockedMenuIcon>`:

- the workgroup subgroup header, inside `.ac-wg-header-text` at lines 2606-2611, shown when
  `workgroupHasBlockedMenu(wg)`;
- the orchestrators quick-group header, inside `.ac-wg-header-text` at lines 2779-2781, shown when
  `filteredCoordinatorItems().some((item) => replicaHasBlockedMenu(item.wg, item.replica))`;
- the project header, as a SIBLING of the `.project-header-main` button, immediately after its
  closing `</button>` on line 2641 and before the `.project-filter-row` div that opens on line 2642,
  shown when `proj.workgroups.some(workgroupHasBlockedMenu)`.

  Not inside the button. `.project-header-main` is the `<button>` that opens on line 2630 and closes
  on line 2641, and the title span on line 2640 is inside it. Putting the badge next to that span
  would fold it into the collapse toggle's clickable area and into its accessible name, so a screen
  reader would announce the badge as part of "toggle this project". As a sibling it stays a status
  indicator.

Give each a distinct `data-ac-testid`: `workgroup.header.blockedMenu.<wg name part>`,
`coordinators.header.blockedMenu` and `project.header.blockedMenu`, using `automationIdPart` for the
variable part exactly as the neighbouring test ids do.

This is a presence badge, not a count. A project-level count is out of scope for this epic.

## Change 3, `src/sidebar/styles/sidebar.css`

Add rules for `.ac-wg-header-blocked-menu` and `.workgroup-group-rail-blocked-menu`. Mirror
`.workgroup-group-rail-raise-hand` at lines 4015-4029 for layout and `pointer-events: none`, and take
the colour from `var(--status-blocked)`, the token P3 added. Size the header glyph to sit on the
header text line; the rail glyph keeps the existing 8px of its raise-hand neighbour.

## Required behaviour

- One replica blocked, everything expanded: the row chip and all three enclosing badges are present.
- The same, with the workgroup subgroup collapsed: the row is gone, the subgroup header badge, the
  project header badge and the rail tab indicator remain.
- The same, with the project panel collapsed: only the project header badge and the rail indicator
  remain, and at least one of them is always visible.
- The same, with a regex filter that excludes the blocked replica: the row is gone and every badge
  remains.
- No replica blocked: no badge anywhere. Absence is asserted, not assumed.
- A raised hand alone never lights a blocked-menu badge, and a blocked menu alone never lights a
  raise-hand badge.

## Failure behaviour

- `replicaHasBlockedMenu` returns false for a replica with no session, so a group whose sessions are
  all gone shows no badge. That is correct and must stay: a dead session is not waiting on a menu.
- The rail render must not throw when `props.button.blockedMenu` is undefined on a stale button
  object. The literal at line 389 exists for that reason; do not delete it.

## Tests

### `src/sidebar/components/ProjectPanel.menu-guard.test.tsx` (extend)

1. **Expanded.** One replica with a visible blocked menu. Assert the row slot and all three header
   badges are present by their test ids.
2. **Subgroup collapsed.** Collapse the workgroup subgroup, assert the row slot is absent and the
   subgroup, coordinators and project badges are present.
3. **Project collapsed.** Collapse the project panel, assert the project header badge is present and
   the row slot is absent. This is the test that proves the notice cannot be hidden completely.
4. **Filtered out.** Set a regex filter that excludes the blocked replica, assert the row slot is
   absent and all three badges are present.
5. **Absence.** No replica blocked: assert every one of the three badge test ids is absent. Assert on
   absence explicitly; a passing render is not evidence.
6. **Raised hand does not light it.** A replica with a visible `raiseHand` and no blocked menu: the
   blocked-menu badges are all absent and the raise-hand indicator is present.

### `src/sidebar/components/WorkgroupGroupRail.blocked-menu.test.tsx` (new)

Mirror `src/sidebar/components/WorkgroupGroupRail.raise-hand.test.tsx` in structure and fixtures.

7. A group with one blocked replica shows the rail blocked-menu indicator.
8. A group with none does not.
9. A group with a raised hand but no blocked menu shows the raise-hand indicator and not the
   blocked-menu one, and the reverse case shows the reverse. Both directions, one test each, because
   a single direction cannot catch a swapped predicate.
10. A group with both shows both, blocked menu first in DOM order.

## Verification command

```
npm run typecheck
npx vitest run src/sidebar/components/ProjectPanel.menu-guard.test.tsx src/sidebar/components/WorkgroupGroupRail.blocked-menu.test.tsx src/sidebar/components/WorkgroupGroupRail.raise-hand.test.tsx
npm test
npm run check:frontend-dependencies
git status --porcelain
```

## Acceptance criteria

1. `npm run typecheck` exits 0 with no output.
2. The targeted run reports the 10 tests above passing plus the existing raise-hand rail tests still
   passing, 0 failures. The raise-hand file is in the command because this phase edits the type and
   the render site it depends on; a change there is a regression, not a rename.
3. `npm test` reports every suite passing; exit 1 with an all-passed summary is the accepted issue-480
   signature and nothing else is.
4. `npm run check:frontend-dependencies` exits 0. This phase adds exactly one module arc,
   `src/sidebar/components/WorkgroupGroupRail.tsx` to `src/sidebar/components/BlockedMenuIcon.tsx`,
   a leaf importing only `solid-js`, mirroring that file's existing `RaiseHandIcon` import on line
   24. It adds no arc to `./workgroup-session`: both edited components already import it, so
   `workgroupHasBlockedMenu` adds a symbol, not an arc. A cycle report here means an unplanned
   import was added.
5. `git status --porcelain` lists exactly the five files above and nothing else.
6. `grep -rn "workgroupHasBlockedMenu" src/ --include=*.tsx | grep -v test.tsx` now returns at least
   two non-test call sites. Before this phase it returned none anywhere, tests included. The second
   `grep` is deliberate: every test file in these directories is also a `.tsx`, so without it the
   count includes the fixtures and has to be eyeballed.
7. One revert, checked once and undone: swapping `workgroupHasBlockedMenu` for
   `workgroupHasRaisedHand` in change 1 makes test 9 fail in both directions.

## Preserve list

- Do not change `workgroupHasRaisedHand`, `replicaHasRaisedHand`, `workgroupHasBlockedMenu` or
  `replicaHasBlockedMenu` in `src/sidebar/components/workgroup-session.ts`. They are already correct;
  this phase only calls them.
- Do not remove or reorder the existing raise-hand indicator at
  `src/sidebar/components/WorkgroupGroupRail.tsx:249-258`.
- Do not change any collapse key, any collapse default, or `projectCollapseStore`.
- Do not change `filteredReplicasForWorkgroup` or the regex filter. The badges make the filter safe;
  they do not change what it filters.
- Do not add a count anywhere. Presence only.
- Do not touch any Rust file.
