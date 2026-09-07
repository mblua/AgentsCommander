# P3: give the blocked menu its own glyph and colour, and the toast button a CSS rule

Class: patterned. Owner: frontend. Depends on: P1.
Status: READY_FOR_IMPLEMENTATION

## Objective

A blocked menu and a raised hand are two different states with two different urgencies, and today a
user cannot tell them apart: both render `RaiseHandIcon` inside the same amber chip. The class that
was written to distinguish them has no CSS rule anywhere. Separate them.

Also give `.toast-item__action` a CSS rule. It has none in the whole repository, so both toast buttons
are raw `<button>` elements at the browser default 13.333px Arial with the global reset stripping
their padding. They wrap to two lines, which is why a blocked-menu toast measures 52px against an
error's 32px, and 52px is what makes a full toast stack tall.

## Exact files

1. `src/sidebar/components/BlockedMenuIcon.tsx` (new)
2. `src/sidebar/components/ProjectPanel.tsx`
3. `src/sidebar/styles/variables.css`
4. `src/sidebar/styles/sidebar.css`
5. `src/shared/styles/toast.css`
6. `src/sidebar/components/ProjectPanel.menu-guard.test.tsx`

## Change 1, `src/sidebar/components/BlockedMenuIcon.tsx` (new)

Copy the shape of `src/sidebar/components/RaiseHandIcon.tsx` exactly: a `Component<{ class?: string }>`
returning a single `<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">` with one
`<path>`, default-exported, with a doc comment naming the issue and the source. Use the Heroicons
`exclamation-triangle` solid path.

The glyph, not the colour, is the discriminator. A 15px chip and a colour difference are not enough on
their own for a colour-blind user or a small screen, so the shapes must differ obviously: a hand and a
warning triangle read apart at 11px.

## Change 2, `src/sidebar/components/ProjectPanel.tsx`

- Import `BlockedMenuIcon` beside the existing `import RaiseHandIcon from "./RaiseHandIcon";` on line 23.
- At line 2465, inside the `showBlockedMenu()` branch, replace
  `<RaiseHandIcon class="coord-communication-icon" />` with
  `<BlockedMenuIcon class="coord-communication-icon" />`.
- Leave line 2451, the raise-hand branch, untouched.
- Leave the `data-kind`, `data-ac-testid`, `title` and `aria-label` attributes on lines 2459-2463
  exactly as they are. They already distinguish the two slots for automation and screen readers; this
  phase fixes only what a sighted user sees.

## Change 3, `src/sidebar/styles/variables.css`

Add `--status-blocked: #f97316;` to the dark block beside `--status-pending` on line 18, and
`--status-blocked: #c2410c;` to the light block beside line 79.

No sidebar style variant overrides any `--status-*` token, so one definition per theme block reaches
all of them. Do not reuse `--status-waiting`: it is green, and it is already the subject of an open
report about two greens colliding on one row. Do not reuse `--status-pending` either: it is `#eab308`
in the dark block (`variables.css:18`) and `#ca8a04` in the light one (`:79`), and the dark value is
the exact amber the raise-hand chip already uses, which would defeat the whole phase.

## Change 4, `src/sidebar/styles/sidebar.css`

Add a rule for `.coord-communication-slot--blocked-menu` immediately after the base
`.coord-communication-slot` rule at lines 6018-6029, overriding only `background` and `color`:

```
.coord-communication-slot--blocked-menu {
  background: color-mix(in srgb, var(--status-blocked) 18%, transparent);
  color: var(--status-blocked);
}
```

If `color-mix` is not acceptable in this codebase's browser target, use an explicit `rgba()` per theme
instead; either is fine, but the two chips must not end up sharing a background.

One precision about the argument in change 3: `--status-pending` is `#eab308` in the dark block
(`variables.css:18`) and `#ca8a04` in the light block (`:79`). The reason not to reuse it stands
unchanged either way, because the raise-hand chip does not read the token at all: it hardcodes
`#eab308` at `src/sidebar/styles/sidebar.css:6027`, in both themes.

## Change 5, `src/shared/styles/toast.css`

Add a `.toast-item__action` rule beside the existing `.toast-item__dismiss` rule at lines 55-65. It
must set at least `flex: 0 0 auto`, `font: inherit`, `white-space: nowrap`, a small padding, a border,
`border-radius: var(--radius-md)`, `color: inherit`, a transparent or subtle background, and
`cursor: pointer`. `white-space: nowrap` is the one that matters: it is what stops the two-line wrap.

## Required behaviour

- A row with a visible blocked menu shows the warning-triangle glyph in an orange chip.
- A row with a visible raised hand still shows the hand glyph in the amber chip, unchanged.
- The two chips never share a background colour or a glyph in any theme.
- Both toast action buttons render on one line.

## Failure behaviour

- A missing `--status-blocked` definition must not fall back to the raise-hand amber. If the token is
  ever dropped, the chip loses its colour rather than impersonating a raised hand, so do not add
  `#eab308` as a fallback value anywhere in change 4.

## Tests

Extend `src/sidebar/components/ProjectPanel.menu-guard.test.tsx`.

1. **Different glyphs.** Render one replica row with a visible `blockedMenu` and one with a visible
   `raiseHand`. Query both communication slots and assert their inner `<svg>` `path` `d` attributes
   are different strings. Comparing the rendered paths, not the component names, is what makes this
   test fail if someone reverts change 2.
2. **Different chip classes.** Assert the blocked slot carries
   `coord-communication-slot--blocked-menu` and the raise-hand slot does not.
3. **`data-kind` is unchanged.** Assert the two slots still expose `data-kind="blockedMenu"` and
   `data-kind="raiseHand"`. This is the regression net for change 2 touching more than the glyph.
4. **The CSS rule exists and is not the amber.** Read `src/sidebar/styles/sidebar.css` and
   `src/sidebar/styles/variables.css` as text, assert a `.coord-communication-slot--blocked-menu`
   rule is present, and assert `--status-blocked` is defined in both theme blocks and that neither
   value is `#eab308`. A DOM test cannot see an unattached stylesheet, so this one asserts on the
   file, and it must say so in a comment.
5. **The toast action rule exists.** Read `src/shared/styles/toast.css` as text and assert a
   `.toast-item__action` rule is present and contains `white-space` and `nowrap`. Same reasoning as
   test 4.

## Verification command

```
npm run typecheck
npx vitest run src/sidebar/components/ProjectPanel.menu-guard.test.tsx
npm test
npm run check:frontend-dependencies
git status --porcelain
```

## Acceptance criteria

1. `npm run typecheck` exits 0 with no output.
2. The targeted run reports the 5 tests above passing, 0 failures, alongside the file's existing tests
   still passing.
3. `npm test` reports every suite passing; exit 1 with an all-passed summary is the accepted issue-480
   signature and nothing else is.
4. `npm run check:frontend-dependencies` exits 0. This phase adds one arc,
   `src/sidebar/components/ProjectPanel.tsx` to `src/sidebar/components/BlockedMenuIcon.tsx`. The new
   file must import nothing but `solid-js`; if it imports anything else, stop and cut it.
5. `git status --porcelain` lists exactly the six files above and nothing else.
6. Two reverts, each checked once and undone: restoring `RaiseHandIcon` at line 2465 makes test 1
   fail; setting `--status-blocked` to `#eab308` makes test 4 fail.
7. `grep -n "eab308" src/sidebar/styles/sidebar.css` still shows the raise-hand chip's own amber and
   no new occurrence.

## Preserve list

- Do not touch line 2451 or anything else in the raise-hand branch at
  `src/sidebar/components/ProjectPanel.tsx:2443-2453`.
- Do not change `data-kind`, `data-ac-testid`, `title` or `aria-label` on either slot. Automation and
  screen readers already depend on them.
- Do not change the base `.coord-communication-slot` rule at `src/sidebar/styles/sidebar.css:6018-6029`
  or `.coord-communication-icon` at `:6034-6039`. Add a sibling rule; do not edit the shared one.
- Do not change any other `--status-*` token value. One open report already tracks that palette.
- Do not add `overflow` or `max-height` to `.toast-host`. Out of scope for this epic.
- Do not touch `src/sidebar/components/RaiseHandIcon.tsx`.
