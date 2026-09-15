# #2038 Independent column scroll in the Coding Agent profile modal

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/2038
- Branch: `fix/2038-independent-column-scroll` (created from `f83189a6`)
- Planning base (frozen): `f83189a6` (`git rev-parse HEAD` at planning time)
- Band: 1-25 (Lite). One phase, one file changed, no partition.
- Owner: `ac-dev-webpage-ui-v4`. Reviewer: `ac-dev-rust-grinch-v4`. Coordinator: `ac-tech-lead-v4`.
- Planning evidence: `.visual-specs/2038/` (gitignored, per-machine): `s-before.html` (the real post-#2014 modal DOM), `patch-*.css` candidates, `measure-2038.cjs` (Playwright/Chromium, real CSS in iframes of exact size), `geometry-2038-v6.json`, `geometry-2038-v6-warn.json`, `run-*.log`.

## 0. Task class and threat model

Routine frontend layout change. CSS only: no TSX, no IPC, no persisted shape, no dependency, no workflow, no release change. No enhanced controls apply (no signing, packaging provenance, untrusted host, security boundary or migration).

## 1. Requirement

In the Coding Agent profile modal, column 2 (Profile cards) and column 3 (Same Profile In Other Agents) must scroll independently: scrolling column 2 must never move column 3 and vice versa. Column 1 (Coding Agents) already scrolls on its own and must stay that way. The narrow single-column layout must keep working.

## 2. Root cause (measured, not inferred)

`.agent-profile-assignment-scroll` (`src/sidebar/styles/sidebar.css:4581-4591`) is the only scroll container for both right-hand columns: it is a 2-column grid with `overflow-y: auto`, one auto-height row, and no scroll container inside either column. `.agent-profile-card-list` (`:4572-4579`) declares `overflow-y: auto` but no bounded height, so it never scrolls; `.agent-projection-panel` (`:5551-5556`) is content-sized and lets the shared wrapper absorb the overflow.

Chromium measurement of the real DOM at 1280x600 (`.visual-specs/2038/geometry-2038-v6.json`, variant `pristine@1280x600`, which is the pre-patch stylesheet):

| | value |
|---|---|
| `.agent-profile-assignment-scroll` scroll range | 107 px |
| `.agent-profile-card-list` scroll range | 0 px (box = its content, 320 px) |
| setting the wrapper `scrollTop = 200` | moves BOTH panels: `panel2.top` 63 -> -44, `panel3.top` 63 -> -44 |

The same holds at 1280x500 (207 px), 1000x560 (257 px) and 1600x520 (185 px): one shared scrollbar moves both columns, which is exactly the reported defect.

## 3. Decided solution

Above the existing 900 px stacking breakpoint: (a) the wrapper's single row is pinned to the frame height, (b) each of the two right-hand column panels becomes its own scroll area, (c) column 3's comparison-table row keeps the 220 px floor that the table's own clamp already defines, so a short frame scrolls column 3 instead of squeezing the comparison rows out of view. Nothing below 900 px is touched.

### D1. The wrapper stops being the shared scroller

`grid-template-rows: minmax(0, 1fr)` inside the wide block pins the wrapper's one row to the wrapper's own (definite) height. This is stated as an explicit track rather than left to Chrome's auto-row fitting, which happens to produce the same numbers today (measured: dropping the declaration changes no measured value in this fixture) but is engine-dependent; the explicit row makes the fix's core invariant - the wrapper has nothing to scroll - independent of that behaviour. Its `overflow-y: auto` at `:4588` is deliberately NOT changed: with the row pinned and both panels being scroll containers, the wrapper's scroll range measures 0 at every wide size (see section 6), and the safety valve stays available if a future change makes a panel overflow.

### D2. The scrolling unit of each column

- Column 2: the panel is a scroll area (`overflow-y: auto`); its head row stays pinned and the card list (`overflow-y: auto` already declared at `:4578`) takes the remaining height and scrolls. The panel itself scrolls only if the frame is shorter than its own head (measured only at 901x500, section 6.5).
- Column 3: the panel is a scroll area and its rows are `auto auto minmax(220px, 1fr) auto` (the 4th row is the fallback warning strip, present only when a fallback applies).
- Column 1 keeps its existing, separate mechanism (`.agent-profile-provider-panel`: `max-height: 100%` + `overflow: hidden` at `:4550-4556`, `grid-template-rows: auto auto minmax(0, 1fr)` at `:4255-4258`): untouched, and its list's scroll range is byte-identical before/after (section 6.1).

The child rule is written as `.agent-profile-assignment-scroll > .agent-profile-panel`, which selects the two columns inside the wrapper and cannot reach column 1 (an `<aside>` that is a child of `.agent-profile-assignment-body`, not of the wrapper).

### D3. The 220 px floor is not a new number

`.agent-comparison-table` (`:5611-5620`) already declares `max-height: clamp(220px, 42vh, 420px)`: 220 px is the design's own minimum table height. The floor reuses it, so the table can never be squeezed below the height its clamp already promises, and its inner scroll range is unchanged.

### D4. Scope boundary

`@media (min-width: 901px)`, the exact complement of the existing `@media (max-width: 900px)` at `:5764`. The narrow block is not edited: below 901 px the wide rules cannot apply and the stacked layout is byte-identical (section 6.3). Costs and bounds are stated: a viewport in the open interval (900 px, 901 px) - only reachable with a fractional device pixel ratio - keeps today's behaviour (no fix, no regression), and a new `min-width` media query is the first in this sheet (the sheet's other breakpoints are `max-width`).

### D5. No DOM change

The selector, the row templates and the floor are all reachable from the existing markup and classes, so `AgentPickerModal.tsx` is not touched, no class/`data-*` is added, and no existing test name or assertion changes.

## 4. Exact scope

Only these tracked paths change:

1. `src/sidebar/styles/sidebar.css` (one added `@media` block)
2. `src/sidebar/styles/agent-picker-column-scroll-css.test.ts` (new byte-level test)

Plus this plan (already tracked). No change to `src/sidebar/components/**`, `src/shared/**`, `src-tauri/**`, `package*.json`, workflows.

## 5. Implementation steps

### 5.1 `src/sidebar/styles/sidebar.css` - one added block

Insert verbatim between the end of the `@media (max-width: 900px)` block (`:5782`) and the `/* Footer hints */` comment (`:5784`). Byte order matters: `.agent-projection-panel`'s base rule at `:5551` must be overridden, and a rule inside a media block does not gain specificity from it.

The exact anchor (the last lines of the narrow block, then a blank line, then the existing comment) is:

```css
  .agent-picker-actions .modal-btn {
    flex: 1 1 220px;
  }
}

/* Footer hints */
```

and the inserted block is:

```css
/* ── #2038: the two right-hand columns of the Coding Agent profile modal scroll
   independently. Above the 900px stacking breakpoint the wrapper only lays the
   columns out: its single row is the frame height (scroll range 0), each column
   panel is its own scroll area, and the comparison-table row keeps the 220px floor
   of the table's own clamp(220px, 42vh, 420px) so a short frame scrolls column 3
   instead of squeezing the rows out of view. The @media (max-width: 900px)
   single-column block above is deliberately untouched. */
@media (min-width: 901px) {
  .agent-profile-assignment-scroll {
    grid-template-rows: minmax(0, 1fr);
  }

  .agent-profile-assignment-scroll > .agent-profile-panel {
    overflow-y: auto;
  }

  .agent-projection-panel {
    grid-template-rows: auto auto minmax(220px, 1fr) auto;
  }
}
```

### 5.2 Cascade check (no other rule has to move)

| Declaration | Competitor | Outcome |
|---|---|---|
| `.agent-profile-assignment-scroll { grid-template-rows }` | none (initial `none`) | applies |
| `... > .agent-profile-panel { overflow-y: auto }` (0,2,0) | `.agent-profile-panel` (0,1,0) `:4539` (declares no `overflow`) | applies |
| `.agent-projection-panel { grid-template-rows }` (0,1,0) | same-selector rule `:5551` | later byte order wins; the block is inserted after it |
| any of the three vs `@media (max-width: 900px)` `:5764-5782` | - | the two media conditions are mutually exclusive |

## 6. Behaviour and edge cases (all numbers measured in Chromium)

`wrapper` = `.agent-profile-assignment-scroll` scroll range; `list2 box/range` = `.agent-profile-card-list`; `tableBody box/range` = `.agent-comparison-table-body`; `p3scroll` = `.agent-projection-panel` scroll range. `before` = current bytes (variant `pristine` in the JSON); `after` = the block of 5.1 applied (variant `v6`).

### 6.1 Wide layout, no fallback warning strip

| size | | wrapper | list2 box/range | tableBody box/range | p3scroll | table height |
|---|---|---|---|---|---|---|
| 1280x800 | before | 0 | 320/0 | 306/604 | 0 | 336.0 |
| 1280x800 | after | 0 | **320/0** | **306/604** | 0 | **336.0** |
| 1280x600 | before | 107 | 320/0 | 216/694 | 0 | 245.5 |
| 1280x600 | after | **0** | 213/107 | 190/720 | 81 | 220.0 |
| 1280x500 | before | 207 | 320/0 | 190/720 | 0 | 220.0 |
| 1280x500 | after | **0** | 113/207 | 190/720 | 181 | 220.0 |
| 1000x560 | before | 257 | 346/0 | 205/908 | 0 | 235.2 |
| 1000x560 | after | **0** | 89/257 | 190/923 | 205 | 220.0 |
| 1600x520 | before | 185 | 320/0 | 190/720 | 0 | 220.0 |
| 1600x520 | after | **0** | 135/185 | 190/720 | 159 | 220.0 |
| 1920x1080 | before | 0 | 320/0 | 390/520 | 0 | 420.0 |
| 1920x1080 | after | 0 | **320/0** | **390/520** | 0 | **420.0** |

Bold cells are byte-identical to `before`: where nothing overflowed (1280x800, 1920x1080) the rendered geometry is unchanged. Each profile card's rect is in fact unchanged at every measured size: the top/height triples are `(115, 133.5), (256.5, 73), (337.5, 97)` in both variants at both large sizes and no card rect differs at any of the ten sizes (the card list's box grows or shrinks around them, it never re-lays them out). Where the frame is too short, the wrapper's scroll range becomes 0 and the overflow moves into the columns.

Independence, measured directly:

| probe | before | after |
|---|---|---|
| wrapper `scrollTop = 200` moves both panels (1280x600) | yes (`63 -> -44` each) | n/a (wrapper range 0) |
| scroll column 2 -> column 3 panel top changed | - | no |
| scroll column 2 -> column 3 table `scrollTop` changed | - | no |
| scroll column 3 table / column 3 panel -> column 2 `scrollTop` changed | - | no |
| last profile card / last comparison row / last Coding Agent card reachable at all target sizes | yes | yes |
| column 1 list scroll range at 1280x600 / 1280x800 | 691 / 491 | 691 / 491 (unchanged) |

### 6.2 Fallback warning strip present (the 4th column-3 row)

Re-measured with `s-before-warn.html` (the real 4th row injected: `agent-profile-warning-strip agent-projection-status`, 32-46 px tall). At 1280x800 and 1920x1080 the after-geometry is identical to before (table 307.0/420.0, `tableBody` 277/633 and 390/520). At 1280x600 / 1280x500 / 1000x560 / 1600x520 the wrapper range is 0, the warning strip is reachable by scrolling column 3 (its full 32/46 px visible after `p3scroll`), and every comparison row stays reachable. `.agent-projection-status` visibility logic is untouched (CSS only), so the row's 0-height case (no fallback) is the table of 6.1.

### 6.3 Narrow width (<= 900 px): untouched, byte-identical

`@media (min-width: 901px)` cannot apply. The whole measurement (every panel/list/table box, scroll range, card and provider rect, mutex probe and body-scroll value) of the after variant equals the before variant at 900x800 and at 800x500 (the app's minimum window width, `src-tauri/src/lib.rs:3526`): `narrowIdenticalToBefore = true`. In that layout the body stays the single scroller (body scroll range 367 / 541 as today), and `.agent-profile-assignment-scroll` keeps `grid-template-columns: 1fr; overflow: visible`.

### 6.4 Comparison-table inner scroll

Unchanged and preserved. The table keeps `max-height: clamp(220px, 42vh, 420px)` and `.agent-comparison-table-body` keeps its own `overflow-y: auto` + `scrollbar-gutter: stable` (`:5623-5638`); its scroll range stays non-zero at every measured size (520-1126 px). At short heights column 3 gains one outer scrollbar (the panel); the wheel over the table scrolls the table body first and chains to the column at its end. This is the same nesting the modal already has today between the shared wrapper and the table body.

### 6.5 Measured boundaries (stated, not hidden)

- 1280x500 / 1000x560 / 1280x600 / 1600x520: everything reachable, column 2 shows at least one card (89-213 px of list) and column 3 shows 190 px of table body. These are the acceptance sizes.
- 1280x420 (below the window minimum height 500): the shared wrapper stays at 0 and both columns still scroll, but column 1's list collapses to a sliver (its last card is unreachable) - a pre-existing condition of the same size in `before` (wrapper range 287 px, same unreachable card). Not a #2038 regression, not fixed here.
- 901x500 with the heavy modal state (lock bar + Matrix default + scope stack, the `s1` fixture): the body itself is only 24 px tall and the pre-existing `body` overflow (26 px, present in `before` too) clips it; column 1's cards are already unreachable in `before` at that size. After the change column 2 matches column 1's pre-existing behaviour there (panel head 29 px > 24 px box, measured list box 0 px; the pristine layout gave the whole column only a 24 px band). Tracked as #2041 (https://github.com/mblua/AgentsCommander/issues/2041); the lower-blocks height budget of that degenerate state is out of scope for #2038.
- The `(900 px, 901 px)` open interval keeps today's behaviour (section D4).

## 7. Tests and acceptance criteria

### 7.1 New byte-level test (automated, the reviewable contract)

`src/sidebar/styles/agent-picker-column-scroll-css.test.ts`, following the throw-on-miss extractor contract of `src/sidebar/styles/working-tint-css.test.ts` (no `?? ""`, every regex CRLF-safe with `[^}]*` and the `m` flag; never a multi-line literal). Run: `npx vitest run src/sidebar/styles/agent-picker-column-scroll-css.test.ts`.

| # | assertion |
|---|---|
| C1 | the `@media (min-width: 901px)` block exists and contains exactly three rules, with exactly these declarations: `.agent-profile-assignment-scroll { grid-template-rows: minmax(0, 1fr) }`, `.agent-profile-assignment-scroll > .agent-profile-panel { overflow-y: auto }`, `.agent-projection-panel { grid-template-rows: auto auto minmax(220px, 1fr) auto }` |
| C2 | cascade pin: the block's byte index is greater than that of the `.agent-projection-panel` base rule and greater than that of the `@media (max-width: 900px)` block |
| C3 | the narrow block is byte-unchanged: `.agent-profile-assignment-scroll` there still declares `grid-template-columns: 1fr` and `overflow: visible`, and no `grid-template-rows` |
| C4 | the wide block contains no `!important`, no nested `@media`, and no selector naming `.agent-profile-provider-panel` or `.agent-profile-provider-list` (column 1 cannot be matched by it) |
| C5 | additivity: the base `.agent-profile-assignment-scroll` still declares `overflow-y: auto` and `grid-template-columns: minmax(300px, 0.92fr) minmax(320px, 0.9fr)`; the base `.agent-projection-panel` still declares `grid-template-rows: auto auto minmax(0, 1fr) auto` and `min-height: 0` |
| C6 | the floor's provenance: the px lower bound parsed out of `.agent-comparison-table`'s `max-height: clamp(...)` equals the floor used in `.agent-projection-panel`'s track list |
| C7 | census: exactly one rule in the sheet declares `minmax(220px, 1fr)` |

### 7.2 Real-engine geometry gate (dev, before handoff)

jsdom has no layout, so the column behaviour is proven in a real engine. The harness and the fixture snapshot already exist in this working copy. The frozen pre-edit stylesheet pair (`sidebar-pristine.css` + a copy of `variables.css`, which `sidebar.css` imports, next to it) is already saved; `s-pristine.html` links it and `s-before.html` links the live `/src/sidebar/styles/sidebar.css`. The variant named by `BASE_VARIANT` is the narrow-identity baseline.

```
cd repo-AgentsCommander
# 1. BEFORE editing anything (baseline) - the pristine pair is already saved:
BASE_VARIANT=pristine node .visual-specs/2038/measure-2038.cjs pristine
#    -> keep geometry-2038.json as geometry-2038-baseline.json
# 2. apply 5.1, then measure the live stylesheet against the pristine copy:
BASE_VARIANT=pristine node .visual-specs/2038/measure-2038.cjs pristine before
#    -> `before@...` rows are now the patched bytes (s-before.html links the live CSS),
#       `pristine@...` rows are the baseline, and narrow rows report narrowIdenticalToBefore
```

(`playwright` is installed globally here: prefix with `NODE_PATH=C:/Users/maria/AppData/Roaming/npm/node_modules`. If `.visual-specs/2038/` is missing on another machine, rebuild the snapshot with the #2014 probe recipe, section G1 of `plans/2014-coding-agent-modal-v4.md`, and copy both `sidebar.css` and `variables.css` for the pristine variant.)

Acceptance at 1280x800, 1280x600, 1280x500, 1000x560 and 1600x520, with and without the warning strip, plus narrow at 900x800 and 800x500:

| id | criterion |
|---|---|
| A1 | `wrapper.maxScrollTop === 0` at every wide size and `body.maxScrollTop === 0` |
| A2 | scrolling column 2 changes neither column 3's panel top nor its table `scrollTop`; scrolling column 3's table or panel changes neither column 2's panel top nor its `scrollTop` |
| A3 | the last profile card, the last Coding Agent card, the last comparison row and (when present) the warning strip are reachable at the sizes of 6.1/6.2 |
| A4 | at 1280x800 and 1920x1080 the after measurement equals the baseline for: profile card tops/heights, the list box, the table height, the table-body box/range, the column-1 list range |
| A5 | narrow sizes: the whole after measurement equals the baseline (byte-identical stacked layout) |
| A6 | no panel shows content taller than its box without `overflow-y: auto`/`scroll` (i.e. nothing is clipped away) |

Recorded baseline and result: `geometry-2038-v6.json` (no warning strip) and `geometry-2038-v6-warn.json` (with it), plus `run-v6.log`, `run-v6-warn.log`. The numbers in section 6 are read out of those files.

### 7.3 Existing suites and gates

| gate | evidence |
|---|---|
| targeted UI tests | `npx vitest run src/sidebar/components/AgentPickerModal.test.tsx src/sidebar/styles/agent-picker-column-scroll-css.test.ts` green; no existing test name, assertion or snapshot changed (the DOM is unchanged) |
| full frontend | `npm test` (only a known unrelated failure signature, if any, tolerated exactly as CI does) |
| types | `npm run typecheck` exit 0 |
| build | `npm run build` exit 0 |
| dependency rules | `npm run check:frontend-dependencies` green (no import added anywhere) |
| scope | `git diff --name-only "$(git log -1 --format=%H -- plans/2038-independent-column-scroll.md)"...HEAD` = exactly the two paths of section 9. Base at the plan's FINAL revision commit, stated explicitly: the plan commit itself is excluded by construction (with `f83189a6...HEAD` it would be counted as a change) |

## 8. Positive controls (mutants the reviewer runs)

Materialise each mutant, run the geometry gate (7.2) and, where listed, the byte test (7.1); the named check must fail, then restore the file and verify `git diff` is empty for that hunk.

| # | mutant | must fail |
|---|---|---|
| M1 | `minmax(220px, 1fr)` -> `minmax(0, 1fr)` in the wide block (i.e. only the panel scroller, no table floor) | A3: comparison rows unreachable at short frames. Measured by Grinch through a runtime override: at 1000x560 the table-body box is 0 px and the row box 0 px; at 1280x500 the box is 9 px, caught by the `>= 60` px guard of the verdict |
| M2 | drop `.agent-profile-assignment-scroll > .agent-profile-panel { overflow-y: auto }` | A1 at 1280x600+ (wrapper range 107/207/257 px returns, both columns move); measured as patch `m-noscroll` |
| M3 | `overflow-y: auto` -> `hidden` on those panels | A3/A6 (`panel3 overflows 81/181 px without its own scroll`); measured as patch `m-hidden` |
| M4 | breakpoint `min-width: 901px` -> `900px` | A5 (at exactly 900x800 the wide rules apply: wrapper range 0, list range 306, narrow geometry changes) plus C1/C2 |
| M5 | move the wide block above the `.agent-projection-panel` base rule | C2; measured effect: the floor is overridden and A3 fails as in M1 |
| M6 | drop `grid-template-rows: minmax(0, 1fr)` from the wrapper | C1; A4/A6 (Chrome still measures 0 by auto-row fitting, so this one is caught by the byte test, not by geometry - stated so the mutant is not mistaken for a behaviour test) |

## 9. File impact

- **ADDED**: `src/sidebar/styles/agent-picker-column-scroll-css.test.ts`
- **MODIFIED**: `src/sidebar/styles/sidebar.css` (one `@media (min-width: 901px)` block inserted after `:5782`; no existing declaration edited)
- **REMOVED**: none
- **UNCHANGED (asserted)**: `src/sidebar/components/AgentPickerModal.tsx`, every other test file, `src/shared/**`, `src-tauri/**`

## 10. Out of scope

Fixing the narrow-layout defects of this modal (at <= 900 px the column-3 panel is 26 px tall in this snapshot and its comparison table is 2 px: pre-existing, present in `before`, unchanged); the body/lower-blocks height budget at 901x500 with the heavy state; column 1's behaviour at 1280x420; any DOM, class, token, IPC, backend or test-name change; any scrollbar restyling.

## 11. Handoff content (dev to coordinator)

Branch and head SHA, the two-file diff, the byte test output, the geometry gate summary (the two JSON files + the A1-A6 verdict table with the numbers of section 6), mutant results, `npm run typecheck` / `npm test` / `npm run build` results.
