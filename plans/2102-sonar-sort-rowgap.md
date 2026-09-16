# #2102: Sonar css:S4657 row-gap override and S2871 bare `.sort()`

Status: READY_FOR_IMPLEMENTATION
Round: 1.
Issue: [#2102](https://github.com/mblua/AgentsCommander/issues/2102). Branch: `fix/2102-sonar-sort-rowgap`. Base: `b90bfb95` (`main`).
Class: lint cleanup. No behavior change.
Owner: frontend (`src/`) plus one Node script.
Files (4): `src/sidebar/styles/sidebar.css`, `src/shared/profile-utils.ts`, `src/resource-monitor/App.tsx`, `scripts/reclaim-build-artifacts.mjs`.

## Facts at base

- F1. `sidebar.css` 9459-9465, `.tpl-picker-item-head`: `row-gap: 4px;` (9463) then `gap: var(--spacing-xs);` (9464). `gap` is the shorthand for `row-gap` + `column-gap`, so it overrides 9463. `--spacing-xs` is `4px` (`src/sidebar/styles/variables.css:35`). Computed row-gap is 4px with or without 9463.
- F2. Bare `.sort()` on arrays holding only strings:
  - `src/shared/profile-utils.ts:49` `sortedProfileLetters`: `[...letters].sort()`, `letters: Set<string>`.
  - `src/resource-monitor/App.tsx:122` `distinct`: `[...new Set(values.filter((v): v is string => !!v))].sort()`.
  - `scripts/reclaim-build-artifacts.mjs:253` `for (const rr of [...repoRoots].sort())` and `:294` `repoRoots: [...repoRoots].sort()`; `repoRoots` is a `Set` of path strings.
- F3. Default sort converts elements with `String()` and orders by UTF-16 code units. For strings, `String(x) === x`, and no array holds `undefined`.

## Decision D1: comparator

Use an explicit code-unit comparator `(a, b) => (a < b ? -1 : a > b ? 1 : 0)`. JS `<` on two strings compares UTF-16 code units, the same order as default sort, and sort is stable in both cases, so output is identical for every input.

Rejected: `localeCompare` / `Intl.Collator`. They are locale-dependent and reorder case and accents (e.g. `"B" < "a"` in code units, but `"a"` before `"B"` in most locales). That changes the resource-monitor filter order and the script's path order/JSON output.

No shared helper: three files in different runtimes (sidebar/shared TS, resource-monitor TS, Node `.mjs`); the inline comparator is one line.

## Exact edits

1. `src/sidebar/styles/sidebar.css`: delete line 9463 `  row-gap: 4px;`. Keep `gap: var(--spacing-xs);`.
2. `src/shared/profile-utils.ts:49`: `return [...letters].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));`
3. `src/resource-monitor/App.tsx:122`: `[...new Set(values.filter((v): v is string => !!v))].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));`
4. `scripts/reclaim-build-artifacts.mjs`: add near the top-level helpers `const byCodeUnit = (a, b) => (a < b ? -1 : a > b ? 1 : 0);` and use `[...repoRoots].sort(byCodeUnit)` at 253 and 294.

Nothing else changes.

## Checks

- `npm run typecheck`
- `npm test`
- `node --check scripts/reclaim-build-artifacts.mjs`
- `git grep -nE '\.sort\(\)' -- src/shared/profile-utils.ts src/resource-monitor/App.tsx scripts/reclaim-build-artifacts.mjs` → no match.
- `sed -n '/^\.tpl-picker-item-head {/,/^}/p' src/sidebar/styles/sidebar.css` → no `row-gap`.
- Equivalence spot check: `node -e 'const c=(a,b)=>(a<b?-1:a>b?1:0);const x=["b","B","a","é","e","Z","_","10","9"];console.log(JSON.stringify([...x].sort())===JSON.stringify([...x].sort(c)))'` → `true`.

## Acceptance criteria

- AC1. The 5 Sonar findings of #2102 no longer apply to these lines.
- AC2. Diff touches only the 4 files, limited to the edits above.
- AC3. All checks pass.
- AC4. Sort order and `.tpl-picker-item-head` layout are unchanged.
