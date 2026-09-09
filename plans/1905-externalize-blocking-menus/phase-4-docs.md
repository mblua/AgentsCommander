# #1905 Phase 4: documentation follows the new layout

Status: READY_FOR_IMPLEMENTATION
Class: patterned (mirrors the existing `menu-guard.md` and `settings.md` sections; no new page).
Owner: docs. Depends on: phases 1, 2 and 3 (the behavior described here must be on the branch).
Parallel with: nothing.
Branch: `feature/1905-externalize-blocking-menus`. Base: `main` at `80aeb85`.
Line numbers are pinned to that base.

## Objective

Three pages stop telling the user to hand-edit `blockingMenus` in `settings.json` and instead
describe the two new files, their precedence, the refresh rule, the migration, what still applies
from the old place, and how to undo it.

## Files

1. `docs/features/menu-guard.md`
2. `docs/reference/settings.md`
3. `docs/reference/directory-layout.md`

## Facts to state (binding; wording is the writer's)

- Two files next to `settings.json`: `settings-blocking-menus.json` holds the blocking-menu
  patterns AC ships; AC owns it and rewrites it at start whenever its content differs from the
  running version's embedded content, so edits there are lost. `settings-blocking-menus.local.json`
  holds the user's patterns; AC reads it at start and never rewrites it, except for one append
  during the migration.
- Both files have the same shape: `schemaVersion` (must be `1`), optional `note`, `byCommand`
  (keys are the lowercase executable stem, exact match), `byAgent` (keys are agent ids). A file
  that is not an object, has another `schemaVersion`, or has the wrong type for `note`,
  `byCommand` or `byAgent` is ignored whole with one error line; an entry inside an array that AC
  cannot read is kept verbatim and skipped, as before.
- Precedence, first present wins and replaces the layers below it whole: an array still on the
  agent (`blockingMenus` in `settings.json` not yet migrated, or inside an `agents` array owned by
  `settings.local.json`), then `.local` `byAgent[id]`, then `.local` `byCommand[stem]`, then the
  shipped `byCommand[stem]`, then nothing.
- What ships: the same three patterns as today (`pi` one, `codex` two), now under `byCommand` in
  the shipped file. Every other stem detects nothing until the user adds a pattern.
- Turning off: `enabled: false` on an entry in a `.local` copy of the array; `"byAgent": {"<id>": []}`
  or `"byCommand": {"<stem>": []}` in `.local` for one agent or one command; `menuGuardEnabled`
  in `settings.json` (overlay-able) for everything. An entry can now be removed durably by owning
  the array in `.local` without it; the hooks-review entry is no longer special.
- Migration: on the first start after upgrade, every `blockingMenus` array still in
  `settings.json` is compared with the shipped set for its stem. Equal arrays are dropped, and so
  is `[]` on a stem that ships nothing (that is what AC itself wrote there). Every other array,
  including `[]` on `pi` or `codex`, a disabled entry, a custom entry, an entry AC cannot read, is
  copied verbatim into `.local` under `byAgent.<id>`, and the key leaves `settings.json`. An id
  that already exists in `.local` is kept, not overwritten. The `.local` file is written before
  `settings.json` is touched.
- Migration exceptions, each leaving the arrays in place and applying as before: `.local` exists
  but does not parse or has the wrong shape; `.local` cannot be written; two agents share an id
  with different arrays; the `agents` array is owned by `settings.local.json`. The log says which.
  The migration retries at every start until the cause is fixed; for an overlay-owned array, the
  fix is to move the entries into `.local` by hand and delete them from the overlay.
- Undo, which needs an older binary: with AC closed, copy each `byAgent.<id>` array from `.local`
  back under that agent as `blockingMenus` in `settings.json`, delete both new files, and run the
  older version; starting the new version instead migrates again at once. Command-wide entries
  the user added under `byCommand` have no place in `settings.json` and are lost by this undo.
- The hand-edit rule survives for the `.local` file: close AC first, because it is read once at
  start; there is still no Settings screen and no CLI verb.
- `capturedAgainst` and `note` are free text AC never parses.

## Page edits

### `docs/features/menu-guard.md`

- "What ships by default" (`:25-41`): say the patterns live in `settings-blocking-menus.json`
  under `byCommand`, keep the three-row table, delete the paragraph about `[]` materialization
  (`:39`) and the paragraph about the #1757 back-fill (`:41`); replace with one paragraph on the
  refresh rule ("rewritten at start when the content differs").
- Replace "Adding a pattern by hand" (`:76-127`) with a `.local`-based walkthrough: same capture and
  pattern steps 1 and 2, then step 3 writes a `settings-blocking-menus.local.json` example with
  `byAgent.claude` (the entry object at `:113-118` byte-for-byte: pattern, notification, enabled,
  capturedAgainst), step 4 unchanged.
- Replace "Why `settings.local.json` does not help here" (`:129-135`) with "The two files and their
  precedence": the precedence list including the still-on-the-agent layer, the replace-whole rule,
  the `byCommand` versus `byAgent` choice, and a one-line note that `menuGuardEnabled` still belongs
  to `settings.json` and its overlay.
- "Turning the guard off" (`:137-149`): rewrite the table to the three `.local` forms plus
  `menuGuardEnabled`; delete the paragraph that says the hooks-review entry cannot be removed (`:149`).
- Add "## Upgrading from a `blockingMenus` array" with the migration facts, the exceptions, and the
  undo recipe. This section is the one place on the page that may instruct writing `blockingMenus`.
- "Settings" table (`:151-158`): `blockingMenus` row becomes "legacy; moved to
  `settings-blocking-menus.local.json` on the first start after upgrade; while still present it
  applies as before"; add rows for the two files pointing at the reference page.
- Troubleshooting (`:160-180`): "My agent stalls" points at `.local`; "I added a pattern and it does
  nothing" adds cause 4, "you edited the shipped file, which AC rewrites at start"; "My edit
  disappeared" names both possibilities.

### `docs/reference/settings.md`

- `:97` `blockingMenus` row of `AgentConfig`: "Legacy. Moved to `settings-blocking-menus.local.json`
  on the first start after upgrade and then absent, unless the migration could not run (see
  [Menu guard](#menu-guard)); while present it applies as before."
- `:460-481` Menu guard section: keep the `menuGuardEnabled` row; replace the `blockingMenus`
  paragraph (`:468`) with the two-file description, a `BlockingMenusFile` table (`schemaVersion`,
  `note`, `byCommand`, `byAgent`), the precedence list, the refresh rule and the migration
  summary with its exceptions; keep the `BlockingMenuConfig` table and the keep-invalid paragraph
  (`:479`), extending the latter to whole-file rejection.
- `:530` See also line: keep, wording may mention the two files.

### `docs/reference/directory-layout.md`

After the `settings.pre-384-v1.json` row (`:81`) add:

| Entry | What it is | Source |
|---|---|---|
| `settings-blocking-menus.json` | Blocking-menu patterns AC ships; rewritten at start when the content differs from the binary's | `config/settings.rs` |
| `settings-blocking-menus.local.json` | User-owned blocking-menu patterns; read at start, written once by the #1905 migration | `config/settings.rs` |

## Verification (repository root)

```
rg -n "blockingMenus" docs/features/menu-guard.md docs/reference/settings.md
awk '/^## Upgrading from a/,/^## Settings/' docs/features/menu-guard.md | rg -c "blockingMenus"
rg -c "settings-blocking-menus" docs/features/menu-guard.md docs/reference/settings.md docs/reference/directory-layout.md
npm run typecheck
```

## Acceptance criteria

- AC1 `git diff --name-only HEAD~1..HEAD` lists exactly the three docs files.
- AC2 In `menu-guard.md`, every `blockingMenus` line printed by the first command that is NOT
  inside the "Upgrading from a `blockingMenus` array" section (the second command counts those)
  contains "legacy" or "still" (case-insensitive), and there are at most 3 such lines; outside
  that section the page no longer instructs adding `blockingMenus` to `agents[]`. In
  `settings.md` at most 3 lines, each containing "legacy" or "while present" or "See also".
  Control: at base the first command prints 10 lines for `menu-guard.md` and 3 for `settings.md`.
- AC3 The third command prints at least 6 for `menu-guard.md`, at least 4 for `settings.md`,
  exactly 2 for `directory-layout.md` (at base all three are 0).
- AC4 Every fact in "Facts to state" appears on `menu-guard.md`; the reviewer ticks the list.
- AC5 The claude example entry in the new walkthrough is byte-identical to the object at base
  `docs/features/menu-guard.md:113-118` (pattern, notification, enabled, capturedAgainst).
- AC6 `npm run typecheck` exits 0 (docs never break it; this is the parity check the
  `frontend-regression` job runs).

## Preserve

Every section not named above; the `list-peers` table; the regex-matching rules section; the
episodes section; the See also list of `menu-guard.md` (add nothing, remove nothing).
