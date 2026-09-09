# #1905 Phase 3: documentation follows the new layout

Class: patterned (mirrors the existing `menu-guard.md` and `settings.md` sections; no new page).
Owner: docs. Depends on: phases 1 and 2 (the behavior described here must be on the branch).
Branch: `feature/1905-externalize-blocking-menus`. Base: `main` at `80aeb85`.
Line numbers are pinned to that base.

## Objective

Three pages stop telling the user to hand-edit `blockingMenus` in `settings.json` and instead
describe the two new files, their precedence, the refresh rule, the migration, and how to undo it.

## Files

1. `docs/features/menu-guard.md`
2. `docs/reference/settings.md`
3. `docs/reference/directory-layout.md`

## Facts to state (binding; wording is the writer's)

- Two files next to `settings.json`: `settings-blocking-menus.json` is owned by AC and rewritten
  from the running version's embedded content at every start whenever it differs, so edits there
  are lost; `settings-blocking-menus.local.json` is owned by the user, read at start, never
  rewritten by AC except the one-time append during the migration.
- Both files have the same shape: `schemaVersion` (must be `1`), optional `note`, `byCommand`
  (keys are the lowercase executable stem, exact match), `byAgent` (keys are agent ids).
- Precedence: `.local` `byAgent[id]`, else `.local` `byCommand[stem]`, else shipped
  `byCommand[stem]`, else nothing. A present array replaces the layers below it whole.
- What ships: the same three patterns as today (`pi` one, `codex` two), now under `byCommand` in
  the shipped file. Every other stem detects nothing until the user adds a pattern.
- Turning off: `enabled: false` on an entry in a `.local` copy of the array; `"byAgent": {"<id>": []}`
  or `"byCommand": {"<stem>": []}` in `.local` for one agent or one command; `menuGuardEnabled`
  in `settings.json` for everything. An entry can now be removed durably by owning the array in
  `.local` without it.
- Migration: on the first start after upgrade, every `blockingMenus` array still in
  `settings.json` is compared with the shipped set for its stem; equal arrays are dropped, every
  other array (including `[]` on `pi` or `codex`, a disabled entry, a custom entry, an entry AC
  cannot read) is copied verbatim into `.local` under `byAgent.<id>`, and the key leaves
  `settings.json`. An id that already exists in `.local` is kept, not overwritten. If `.local`
  exists but does not parse, nothing moves and the log says why. An `agents` array owned by
  `settings.local.json` is left alone and its `blockingMenus` are ignored with an info line.
- Undo by hand, with AC closed: copy the `byAgent.<id>` array back under that agent as
  `blockingMenus` in `settings.json`, delete both new files. An older binary that sees no key
  fills its own defaults again.
- The hand-edit rule survives for the `.local` file: close AC first, because it is read once at
  start; there is still no Settings screen and no CLI verb.
- Invalid entries are kept verbatim and skipped; a whole `.local` file that is not a JSON object
  or has another `schemaVersion` is ignored with one error line.
- `capturedAgainst` and `note` are free text AC never parses.

## Page edits

### `docs/features/menu-guard.md`

- "What ships by default": say the patterns live in `settings-blocking-menus.json` under
  `byCommand`, keep the three-row table, delete the paragraph about `[]` materialization and the
  paragraph about the #1757 back-fill; replace with one paragraph on the refresh rule.
- Replace "Adding a pattern by hand" with a `.local`-based walkthrough: same capture and pattern
  steps 1 and 2, then step 3 writes a `settings-blocking-menus.local.json` example with
  `byAgent.claude` (keep the claude example entry byte-for-byte from `:120-135`), step 4 unchanged.
- Replace "Why `settings.local.json` does not help here" with "The two files and their precedence":
  the precedence list, the replace-whole rule, the `byCommand` versus `byAgent` choice, and a
  one-line note that `menuGuardEnabled` still belongs to `settings.json` and its overlay.
- "Turning the guard off": rewrite the table to the three `.local` forms plus `menuGuardEnabled`;
  delete the paragraph that says the hooks-review entry cannot be removed.
- Add "Upgrading from a `blockingMenus` array" with the migration facts and the undo recipe.
- "Settings" table: `blockingMenus` row becomes "legacy, moved on first start"; add rows for the
  two files pointing at the reference page.
- Troubleshooting: "My agent stalls" points at `.local`; "I added a pattern and it does nothing"
  adds cause 4, "you edited the shipped file, which AC rewrites at start"; "My edit disappeared"
  names both possibilities.

### `docs/reference/settings.md`

- `:97` `blockingMenus` row of `AgentConfig`: "Legacy. Moved to `settings-blocking-menus.local.json`
  on the first start after upgrade and then absent. See [Menu guard](#menu-guard)."
- `:460-481` Menu guard section: keep the `menuGuardEnabled` row; replace the `blockingMenus`
  paragraph with the two-file description, a `BlockingMenusFile` table (`schemaVersion`, `note`,
  `byCommand`, `byAgent`), the precedence list, and the refresh rule; keep the `BlockingMenuConfig`
  table and the keep-invalid paragraph, extending the latter to whole-file rejection.

### `docs/reference/directory-layout.md`

After the `settings.pre-384-v1.json` row (`:81`) add:

| Entry | What it is | Source |
|---|---|---|
| `settings-blocking-menus.json` | Shipped blocking-menu patterns; rewritten from the binary at every start | `config/settings.rs` |
| `settings-blocking-menus.local.json` | User-owned blocking-menu patterns; read at start, written once by the #1905 migration | `config/settings.rs` |

## Verification

```
rg -n "blockingMenus" docs/features/menu-guard.md docs/reference/settings.md
rg -n "settings-blocking-menus" docs/features/menu-guard.md docs/reference/settings.md docs/reference/directory-layout.md
npm run typecheck
```

## Acceptance criteria

- AC1 `git diff --name-only HEAD~1..HEAD` lists exactly the three docs files.
- AC2 The first `rg` prints at most 3 lines per file, each of which contains "legacy" or "moved"
  or "upgrad" (case-insensitive); the page no longer instructs adding `blockingMenus` to
  `agents[]`. Control: at base the same command prints 10 or more lines.
- AC3 The second `rg` prints at least 6 lines in `menu-guard.md`, at least 4 in `settings.md`,
  exactly 2 in `directory-layout.md`.
- AC4 Every fact in "Facts to state" appears on `menu-guard.md`; the reviewer ticks the list.
- AC5 The claude example entry in the new walkthrough is byte-identical to the one at base
  `:125-130` (pattern, notification, enabled, capturedAgainst).
- AC6 `npm run typecheck` exits 0 (docs never break it; this is the parity check the CI job runs).

## Preserve

Every section not named above; the `list-peers` table; the regex-matching rules section; the
episodes section; the See also list (add nothing, remove nothing).
