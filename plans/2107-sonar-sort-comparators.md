# Plan #2107: Explicit code-unit comparator for five bare string sorts

Author: ac-dev-webpage-ui-v4, room-5. Lite, 2026-09-16 UTC.

Status: READY_FOR_IMPLEMENTATION

Issue: [mblua/AgentsCommander#2107](https://github.com/mblua/AgentsCommander/issues/2107)

## Objective

Clear five SonarCloud `typescript:S2871` findings without changing any resulting order.

## Cause

`Array.prototype.sort()` with no comparator sorts by UTF-16 code units. The order is correct, but Sonar flags the missing comparator. No existing helper gives code-unit order: every current comparator in `src/` uses `localeCompare`, which changes order (case folding, `_` placement, numeric collation) and breaks parity with Rust `BTreeMap<String, _>` byte order for `sortedWatcherIds`.

## Scope

1. New file `src/shared/string-order.ts` exporting one function:

   ```ts
   /** UTF-16 code-unit order: exactly what a comparator-less `sort()` does. */
   export function compareCodeUnits(a: string, b: string): number {
     if (a < b) return -1;
     if (a > b) return 1;
     return 0;
   }
   ```

   `if` statements, not a nested ternary (avoids Sonar S3358).

2. Replace each bare `.sort()` with `.sort(compareCodeUnits)` and import from the shared module:
   - `src/sidebar/stores/rail-collapse.ts` `snapshot()` (line 19).
   - `src/sidebar/components/settings-watchers.ts` `distinctCommandStems` (line 204).
   - `src/sidebar/components/settings-watchers.ts` `sortedWatcherIds` (line 246). Keep the doc comment about `BTreeMap` order.
   - `src/watchers/App.tsx` `watcherOptions` memo (line 329) and `workgroupOptions` memo (line 342).

3. New test `src/shared/string-order.test.ts`.

## Out of scope

- Any existing `localeCompare` call site (including `src/watchers/activity.ts:317`).
- Other Sonar rules, `src-tauri/`, UI behavior or styling.

## Behavior and edge cases

- Order is identical to today for every input: `sort()` without a comparator compares with `<`/`>` on strings, which is code-unit order.
- Uppercase before lowercase (`"B" < "a"`), `_` after uppercase and before lowercase, digits lexicographic (`"10" < "9"`), empty string first, equal strings return 0.
- Non-BMP characters: code-unit order, same as today (can differ from Rust byte order only for surrogate vs U+E000–U+FFFF; unchanged by this fix).
- All inputs are `string[]`; `undefined` never reaches the comparator (default sort would place it last; not applicable).

## Tests and acceptance criteria

`src/shared/string-order.test.ts`:
- Returns -1 / 1 / 0 for `("a","b")`, `("b","a")`, `("a","a")`.
- Parity: for a fixed array `["b", "B", "_x", "a10", "a9", "", "Z", "é", "e", "😀", "￿"]`, `[...arr].sort(compareCodeUnits)` deep-equals `[...arr].sort()`.

Existing suites must pass unchanged: `rail-collapse.test.ts`, `settings-watchers.test.ts`, `src/watchers/App.test.tsx`.

Command:

```
npx vitest run src/shared/string-order.test.ts src/sidebar/stores/rail-collapse.test.ts src/sidebar/components/settings-watchers.test.ts src/watchers/App.test.tsx
```

Also `npx tsc --noEmit` clean.

Acceptance: all five lines use `compareCodeUnits`; no bare `.sort()` remains at those sites; tests above pass; SonarCloud no longer reports S2871 on these files.
