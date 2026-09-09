# #1905 Phase 4: documentation follows the new layout

Status: READY_FOR_IMPLEMENTATION
Class: patterned (mirrors the existing `menu-guard.md` and `settings.md` sections; no new page).
Owner: docs. Depends on: phases 1, 2 and 3 (the behavior described here must be on the branch).
Parallel with: nothing.
Branch: `feature/1905-externalize-blocking-menus`. Base: `main` at `80aeb85`.
Line numbers are pinned to that base. Round 4 changed only this file; phases 1-3 are
byte-identical to round 3.

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
  holds the user's patterns; AC reads it at start and never rewrites it, except when the migration
  writes to it (that write inserts `byAgent` rows and never overwrites one; see the migration
  facts below).
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
  the shipped file. For every other stem the shipped defaults detect nothing; a legacy array still
  on the agent or a `.local` entry can still apply.
- Turning off, three forms. Two live in `.local`, and each of those two is effective only when no
  higher layer supplies an array for that agent (a legacy array still on the agent wins over
  `.local`): `enabled: false` on an entry inside a `.local` array (`byAgent.<id>` or
  `byCommand.<stem>`; to disable one shipped entry, copy the stem's shipped entries into that
  `.local` array first, because the `.local` array replaces the shipped array whole); and
  `"byAgent": {"<id>": []}` or `"byCommand": {"<stem>": []}` in `.local` for one agent or one
  command. The third form, `"menuGuardEnabled": false` in `settings.json` (overlay-able), stops
  everything regardless of layers. An entry can now be removed durably by owning the array in
  `.local` without it; the hooks-review entry is no longer special.
- Replace-whole has a cost the page must state: a `byAgent.<id>` row in `.local` (written by hand
  or by the migration) freezes that agent against every future shipped pattern for its stem,
  because the row replaces the shipped array instead of adding to it. A user who wants shipped
  updates plus one extra pattern keeps the shipped entries in the row and revisits it after upgrades.
- The durable off form for a stem that ships nothing (claude, gemini, ...) is now
  `"byAgent": {"<id>": []}` in `.local`; a `[]` left in `settings.json` for such a stem is treated
  as AC's own materialized default and is dropped by the migration.
- Migration: on the first settings load after upgrade, every `blockingMenus` array still in
  `settings.json` is compared with the shipped set for its stem. Equal arrays are dropped, and so
  is `[]` on a stem that ships nothing (that is what AC itself wrote there). Every other array is
  copied into `.local` under `byAgent.<id>`: `[]` on `pi` or `codex`, and any array holding a
  disabled entry, a custom entry, or an entry AC cannot read (readable entries in AC's own form,
  with `enabled` written out; unreadable entries verbatim). Then the key leaves `settings.json`.
  An id that already exists in `.local` is kept, not overwritten. The `.local` file is written
  before `settings.json` is touched.
- Migration exceptions, each leaving the arrays in place and applying as before: `.local` exists
  but does not parse or has the wrong shape; `.local` cannot be written; two agents share an id
  with different arrays or different commands; the `agents` array is owned by
  `settings.local.json`. The log says which. AC retries on every settings load, which is every
  GUI start, every settings reload the running GUI performs, and every CLI verb, and logs one
  error line per attempt until the cause is fixed; for an overlay-owned array, the fix is to move
  the entries into `.local` by hand and delete those agents' `blockingMenus` keys from the overlay.
- Undo, which needs an older binary: with AC closed, copy each `byAgent.<id>` array from `.local`
  back under that agent as `blockingMenus` in `settings.json`, delete both new files, and run the
  older version; starting the new version instead migrates again at once. Command-wide entries
  the user added under `byCommand` have no place in `settings.json` and are lost by this undo.
- The hand-edit rule survives for the `.local` file: close AC first, because the guard reads it
  once, when AC starts, and a Settings save never rewrites it; there is still no Settings screen
  and no CLI verb.
- `capturedAgainst` and `note` are free text AC never parses.

## Page edits

### `docs/features/menu-guard.md`

- "What ships by default" (`:25-41`): keep `:27-35` (stem rule and the three-row table), say the
  patterns live in `settings-blocking-menus.json` under `byCommand`; rewrite `:37` ("starts with
  an empty array and detects nothing ... you write the pattern by hand") to the fact that every
  other stem ships nothing, a legacy array still on the agent or a `.local` entry can still apply,
  and the walkthrough below shows how; delete the paragraph about `[]` materialization (`:39`) and
  the paragraph about the #1757 back-fill (`:41`); replace them with one paragraph on the refresh
  rule ("rewritten at start when the content differs").
- `:62`, inside "How a pattern is matched" (`:43-62`): replace `has no \`blockingMenus\` array`
  with `has no patterns`. That sentence is stale (patterns no longer live on the agent) and it is
  the only edit inside `:43-62`; Preserve names the exception. After it, `:62` no longer holds the
  literal, so AC2 needs no exemption.
- `:74`, last paragraph of "Episodes and re-arming": keep unchanged. The sentence is about the root
  switch, `menuGuardEnabled`, which stays in `settings.json` after this plan, so "the one that reads
  your edited `settings.json`" is still true. Not stale; Preserve holds.
- Replace "Adding a pattern by hand" (`:76-127`) with a `.local`-based walkthrough: same capture and
  pattern steps 1 and 2, then step 3 writes a `settings-blocking-menus.local.json` example with
  `byAgent.claude` holding the entry object at `:113-118` (pattern, notification, enabled,
  capturedAgainst; same bytes, indentation may differ, see AC5), step 4 unchanged.
- Replace "Why `settings.local.json` does not help here" (`:129-135`) with "The two files and their
  precedence": the precedence list including the still-on-the-agent layer, the replace-whole rule,
  the `byCommand` versus `byAgent` choice, and a one-line note that `menuGuardEnabled` still belongs
  to `settings.json` and its overlay.
- "Turning the guard off" (`:137-149`): rewrite the table to the three `.local` forms plus
  `menuGuardEnabled`; delete the paragraph that says the hooks-review entry cannot be removed (`:149`).
- Add "## Upgrading from a `blockingMenus` array" with the migration facts, the exceptions, and the
  undo recipe. Insertion point: immediately after the end of "Turning the guard off" (its last
  paragraph is `:149` at base) and before `## Settings` (`:151`), so that AC2's awk range
  `/^## Upgrading from a/,/^## Settings/` is bounded. This section is the one place on the page
  that may instruct writing `blockingMenus`.
- "Settings" table (`:151-158`): `blockingMenus` row becomes "legacy; moved to
  `settings-blocking-menus.local.json` on the first start after upgrade; while still present it
  applies as before"; add rows for the two files pointing at the reference page.
- Troubleshooting (`:160-180`):
  - "My agent stalls" (`:162`): the `"blockingMenus": []` materialization sentence goes; say only
    the `pi` and `codex` stems ship patterns and point at the `.local` walkthrough.
  - "I added a pattern and it does nothing" (`:164-168`): cause 1 (`:166`, "a Settings save
    overwrote the file") is false for `.local`, which AC never rewrites; it becomes "you edited
    `.local` while AC was running; it is read once at start, so restart AC". Causes 2 and 3 stay.
    Add cause 4, "you edited the shipped file, which AC rewrites at start", and cause 5, "a higher
    layer holds an array for that agent: a `byAgent.<id>` row in `.local` (the migration writes one
    for every agent whose array was not the shipped set) replaces `byCommand`, and a legacy array
    still on the agent replaces both".
  - "My edit disappeared" (`:170`): name three causes: (a) you edited `settings.json` while AC was
    running (the base text); (b) you edited `settings-blocking-menus.json`, which AC rewrites at
    start; (c) the migration moved your legacy array from `settings.json` into `.local` under
    `byAgent.<id>`, so look there.
  - "One bad entry broke my settings file" (`:180`): keep, and add one sentence: a `.local` or
    shipped file with the wrong shape is ignored whole with one error line, and the layers below it
    still apply.

### `docs/reference/settings.md`

- `:97` `blockingMenus` row of `AgentConfig`: "Legacy. Moved to `settings-blocking-menus.local.json`
  on the first start after upgrade and then absent, unless the migration could not run (see
  [Menu guard](#menu-guard)); while present it applies as before."
- `:460-481` Menu guard section: `:462` now says patterns are hand-edited in
  `settings-blocking-menus.local.json` with AC closed; keep the `menuGuardEnabled` row; replace the
  `blockingMenus` paragraph (`:468`) with the two-file description, a `BlockingMenusFile` table
  (`schemaVersion`, `note`, `byCommand`, `byAgent`), the precedence list, the refresh rule and the
  migration summary with its exceptions; keep the `BlockingMenuConfig` table and the keep-invalid
  paragraph (`:479`), extending the latter to whole-file rejection.
- `:530` See also line: keep, wording may mention the two files.

### `docs/reference/directory-layout.md`

Insert the two rows below immediately after the `settings.pre-384-v1.json` row (`:81`) of the
existing table. The header lines are shown for column reference only; do not paste them.

| Entry | What it is | Source |
|---|---|---|
| `settings-blocking-menus.json` | Blocking-menu patterns AC ships; rewritten at start when the content differs from the binary's | `config/settings.rs` |
| `settings-blocking-menus.local.json` | User-owned blocking-menu patterns; read at start, written by AC only when the #1905 migration succeeds (once on a normal upgrade; again after a failed or retried migration, never overwriting an existing entry) | `config/settings.rs` |

## Verification (repository root, Git Bash)

```
rg -n "blockingMenus" docs/features/menu-guard.md docs/reference/settings.md
awk '/^## Upgrading from a/,/^## Settings/' docs/features/menu-guard.md | rg -c "blockingMenus"
rg -c "settings-blocking-menus" docs/features/menu-guard.md docs/reference/settings.md docs/reference/directory-layout.md
N=$(rg -n '"capturedAgainst": "claude 2.1 / Windows"' docs/features/menu-guard.md | cut -d: -f1); echo "$N"
diff <(git show 80aeb85:docs/features/menu-guard.md | sed -n '113,118p' | tr -d '\r' | sed 's/^[[:space:]]*//') <(sed -n "$((N-4)),$((N+1))p" docs/features/menu-guard.md | tr -d '\r' | sed 's/^[[:space:]]*//'); echo "exit $?"
npm run typecheck
```

## Acceptance criteria

- AC1 `git diff --name-only HEAD~1..HEAD` lists exactly the three docs files.
- AC2 In `menu-guard.md`, every `blockingMenus` line printed by the first command that is NOT
  inside the "Upgrading from a `blockingMenus` array" section (the second command counts those)
  contains "legacy" or "still" (case-insensitive), and there are at most 4 such lines. Budget:
  the precedence list 1, the Settings row 1, Troubleshooting at most 2 (`:62` no longer holds the
  literal after its edit). Outside that section the page no longer instructs adding
  `blockingMenus` to `agents[]`. In `settings.md` at most 4 lines, each containing "legacy",
  "while present", "still" or "See also" (case-insensitive); `:97` and `:530` account for two, so
  the rewritten Menu guard section may hold the literal on at most two lines. Control: at base the
  first command prints 10 lines for `menu-guard.md` and 3 for `settings.md`.
- AC3 The third command prints at least 6 for `menu-guard.md`, at least 4 for `settings.md`,
  exactly 2 for `directory-layout.md` (at base all three are 0).
- AC4 Every fact in "Facts to state" appears on `menu-guard.md`; the reviewer ticks the list.
- AC5 The claude example entry in the new walkthrough is identical to the object at base
  `docs/features/menu-guard.md:113-118` (pattern, notification, enabled, capturedAgainst) ignoring
  indentation: the fourth command prints exactly one line number (the string occurs once on the
  page at base), and the fifth prints nothing and `exit 0`. Leading whitespace is stripped on both
  sides because the object sits at 4 spaces inside `blockingMenus: [` at base and normally at 6
  inside `"byAgent": { "claude": [`.
- AC6 `npm run typecheck` exits 0 (docs never break it; this is the parity check the
  `frontend-regression` job runs).

## Preserve

Every section not named above; the `list-peers` table; the regex-matching rules section (`:43-62`)
except the one-phrase edit at `:62` named above; the episodes section including `:74`; the See also
list of `menu-guard.md` (add nothing, remove nothing).
