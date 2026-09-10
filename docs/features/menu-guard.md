# Menu guard

For developers whose coding agent is parked on a dialog nobody noticed. After this page you can tell whether AC watches for that dialog, add a pattern for one it does not know, and turn the whole thing off.

The menu guard watches every agent terminal for a **blocking menu**: a prompt the coding agent puts on screen and will not move past until a human answers it. A folder-trust question is the usual one. AC has no way to answer it for you, so instead it tells you the session is stuck, points you at the terminal, and stops writing to that session until the menu is gone.

## What the guard does when it matches

Four things happen:

1. **The session is marked blocked**, and the state is written to `sessions.json` so a process reading disk can see it too.
2. **A sticky toast appears**, carrying the pattern's own `notification` text. It has two buttons: `See terminal` raises the blocked terminal and leaves the toast up, and `Resolved by user` clears the notice.
3. **A chip appears on the replica row** in the project panel, with the accessible label `Interactive menu requires user input`.
4. **Injected writes into that session are refused.** They return an error starting with `menu_guard_deferred`, so an inter-agent message aimed at that session is held rather than typed into a dialog. The mailbox treats a deferral as "not yet" rather than a failed attempt, so nothing is rejected while you are away. **What you type yourself is not affected**, which is how you answer the menu.

The block also shows up outside the app. [`list-peers`](../reference/cli.md#list-peers) emits two extra fields for a blocked peer, and omits both when it is not blocked:

| Field | Meaning |
|---|---|
| `blockedMenu` | `true` when the matched session is parked on a blocking terminal menu. |
| `blockedMenuMessage` | The pattern's `notification` text, for example `codex is waiting for you to answer the folder-trust menu in this terminal`. |

`working`, `sessionStatus` and `waitingForInput` keep their normal values while a session is blocked.

## What ships by default

Defaults are chosen by the agent's **command executable stem**: the file stem of the first token of `command`, lowercased. `C:\tools\Codex.exe --search` has the stem `codex`. This is the same rule [Watchers](watchers.md#commands-a-watcher-can-run) uses for its selector.

Three patterns ship, across exactly two stems:

| Stem | Pattern | Notification |
|---|---|---|
| `pi` | `^\s*Trust project folder\?` | `pi is waiting for you to answer the folder-trust menu in this terminal` |
| `codex` | `^\s*Do you trust the contents of this directory\?` | `codex is waiting for you to answer the folder-trust menu in this terminal` |
| `codex` | `^[^A-Za-z0-9]*Hooks need review\b` | `codex is waiting for you to answer the hooks-review menu in this terminal` |

Those patterns live in `settings-blocking-menus.json`, under `byCommand`.

**Every other stem ships nothing.** That includes Claude Code, Antigravity, and anything you added yourself. A legacy array still on the agent, or an entry in `settings-blocking-menus.local.json`, can still apply; the walkthrough below shows how.

AC owns `settings-blocking-menus.json` and rewrites it at start whenever its content differs from the running version's embedded content, so edits there are lost.

## How a pattern is matched

`pattern` is a [Rust `regex` crate](https://docs.rs/regex) expression, version `1.12.3` in this build. The rules that decide whether it matches:

- **It is matched against the parsed screen, not the raw byte stream.** AC reads rows out of its terminal model, so color codes, cursor moves and other escape sequences are already gone. Never write a pattern that expects an escape sequence; it will never match.
- **One logical row at a time.** A line longer than the terminal is stored as two or more physical rows, and AC joins them back together, with no separator, before matching. So a pattern can span a wrap, but it can never span two different lines.
- **A wrapped line touching the top of the screen is skipped entirely.** Its beginning may already have scrolled away, and AC would rather miss a match than match half a line. This corrects itself on the next scroll.
- **The match is unanchored.** A pattern with no `^` matches anywhere in the row.
- **`^` means the start of the logical row, including its leading spaces.** The terminal keeps the row's left inset, so `^Do you trust` does not match `| Do you trust the files in this folder?`. Both shipped idioms exist to handle that: `^\s*` skips whitespace, and `^[^A-Za-z0-9]*` also skips box-drawing characters, bullets and arrows.
- **Rust regex has no lookaround and no backreferences.** `^(?=.*trust)` fails to compile with `look-around, including look-ahead and look-behind, is not supported`, and `(a)\1` fails with `backreferences are not supported`. A pattern that fails to compile is logged once and skipped; it never stops the app or the other patterns.

  ```text
  [menu_guard] Invalid regex pattern '<pattern>': <detail>
  ```

- **Entries are tried in array order, and the first match wins.** A disabled entry, and an entry AC could not read as a pattern object, are both skipped.

The scan runs on its own loop, **one tick every 250 ms**, across every live session. A tick that finds the screen unchanged since the last one does nothing at all: no match, no save, no event. If a tick runs long, the missed ticks are skipped rather than queued.

Sessions that are not agent sessions are never matched. A plain shell has no coding agent behind it, so it has no patterns and nothing to match against.

## Episodes and re-arming

An **episode** is one appearance of one menu. Episodes exist so that `Resolved by user` can silence a notice without silencing the feature.

- The first tick that matches a pattern opens an episode and raises the notice.
- Every later tick that matches the **same** pattern belongs to the same episode. AC publishes only on a real change of state, so you get one notice rather than four a second.
- `Resolved by user` suppresses the current episode. The session stops being blocked and the toast goes, even though the menu may still be on screen, and writes into the session are allowed again.
- When the menu **disappears**, AC clears the notice and forgets the suppression. The next appearance is a fresh episode and raises a fresh notice. That is the re-arm.
- A **different** pattern matching also opens a new episode, so a suppressed folder-trust prompt does not silence a hooks-review prompt that follows it.

Turning the guard off at the root also ends every episode it is currently holding: the next tick clears each blocked session, drops its toast and its chip, and lets writes through again. No session restart is needed for that; the app restart you need is the one that reads your edited `settings.json` in the first place.

## Adding a pattern by hand

There is no Settings screen and no CLI verb for the blocking-menu patterns. You edit `settings-blocking-menus.local.json`.

Apart from the one-time upgrade migration, AC never rewrites that file, so editing it while AC is running loses nothing. The guard reads it once, when AC starts, so restart AC after editing.

### 1. Capture the row

Open the agent, reproduce the dialog, and copy the line the agent prints, exactly as it appears. Wording changes between coding-agent releases, so take it from your own terminal rather than from a blog post.

Say the row is:

```text
| Do you trust the files in this folder?
```

### 2. Write the pattern

The leading `| ` is part of the row, so an anchored `^Do you trust` matches nothing. Skip the decoration instead:

```text
^[^A-Za-z0-9]*Do you trust the files in this folder\?
```

Escape the `?`. In JSON, every backslash doubles.

### 3. Add it to your file

Create `settings-blocking-menus.local.json` next to `settings.json` and add the agent under `byAgent`. Adding it to the `claude` agent looks like this:

```json
{
  "schemaVersion": 1,
  "note": "my own patterns; AC never parses this",
  "byAgent": {
    "claude": [
      {
        "pattern": "^[^A-Za-z0-9]*Do you trust the files in this folder\\?",
        "notification": "claude is waiting for you to answer the folder-trust menu in this terminal",
        "enabled": true,
        "capturedAgainst": "claude 2.1 / Windows"
      }
    ]
  }
}
```

`notification` is what you will read on the toast and in `blockedMenuMessage`, so name the agent and say what to do. `capturedAgainst` and the file's optional `note` are free text that AC never parses; `capturedAgainst` is there so that in a year you know which version the pattern was written against.

### 4. Restart AC and reproduce

Start AgentsCommander, launch the agent, and trigger the dialog again. Within about a quarter of a second you get the toast and the row chip. If nothing happens, see [Troubleshooting](#troubleshooting).

## The two files and their precedence

The patterns live in two files next to `settings.json`:

| File | Who writes it | When |
|---|---|---|
| `settings-blocking-menus.json` | AC | Rewritten at start whenever its content differs from the running version's embedded content, so edits there are lost. |
| `settings-blocking-menus.local.json` | You | Read at start. AC writes it only when the upgrade migration runs, and that write adds `byAgent` rows without overwriting an existing one. |

Both files share one shape: `schemaVersion` (must be `1`), an optional `note`, `byCommand` (keys are the lowercase executable stem, exact match) and `byAgent` (keys are agent ids). A `.local` file that is not an object, carries another `schemaVersion`, or gives the wrong type for `note`, `byCommand` or `byAgent` is ignored whole, with one error line in the log. The shipped file is never parsed at runtime: AC rewrites it from the binary's embedded copy at start and evaluates that embedded copy. An entry inside an array that AC cannot read is kept verbatim and skipped, as before.

The guard picks one array per agent. The first layer that supplies one wins, replacing the layers below it whole:

1. an array still on the agent: a legacy `blockingMenus` in `settings.json` that the migration could not move, or one inside an `agents` array owned by `settings.local.json`
2. `byAgent[id]` in `.local`
3. `byCommand[stem]` in `.local`
4. `byCommand[stem]` in the shipped file
5. otherwise nothing: the agent detects nothing

Use `byCommand` when the pattern should follow the command and reach every agent that runs it; use `byAgent` when it should follow one agent id. A `byAgent` row always beats a `byCommand` row.

**Replace-whole has a cost.** A `byAgent.<id>` row in `.local` - written by hand or by the migration - freezes that agent against every future shipped pattern for its stem, because the row replaces the shipped array instead of adding to it. To keep shipped updates plus one extra pattern, keep the shipped entries in that row and revisit it after upgrades.

`menuGuardEnabled` is not part of either file. It stays a key in `settings.json`, and `settings.local.json` can still override it.

## Turning the guard off

The scopes, smallest first:

| What you want | What to write |
|---|---|
| Stop one pattern, keep the rest | `"enabled": false` on that entry, inside a `.local` array |
| Stop every pattern for one agent | `"byAgent": {"<id>": []}` in `settings-blocking-menus.local.json` |
| Stop every pattern for one command | `"byCommand": {"<stem>": []}` in `settings-blocking-menus.local.json` |
| Stop the feature everywhere | `"menuGuardEnabled": false` at the root, or in the `settings.local.json` overlay |

The first three forms live in `.local` and each is effective only when no higher layer supplies an array for that agent: a legacy array still on the agent wins over `.local`. To switch off one shipped entry with `"enabled": false`, copy that stem's shipped entries into your `.local` array first, because the `.local` array replaces the shipped array whole.

For a stem that ships nothing (Claude Code, Antigravity, ...), the durable off form is now `"byAgent": {"<id>": []}` in `.local`. A `[]` left in `settings.json` for such a stem is AC's own materialized default and is dropped by the migration.

An entry can be removed durably by owning the array in `.local` without it. Once the array lives there, the Codex hooks-review entry is no longer special - the migration still back-fills it once, on the way in, as the next section describes.

## Upgrading from a `blockingMenus` array

The first settings load after the upgrade moves every `blockingMenus` array out of `settings.json`. Each array still there is compared with the shipped set for its command stem:

- An array equal to the shipped set is dropped.
- `[]` on a stem that ships nothing (Claude Code, Antigravity, ...) is also dropped, because that is what AC itself wrote there.
- Every other array is copied into `settings-blocking-menus.local.json` under `byAgent.<id>`: `[]` on `pi` or `codex`, and any array holding a disabled entry, a custom entry, or an entry AC cannot read. Readable entries are written in AC's own form, with `enabled` written out; unreadable entries are copied verbatim.

Then the `blockingMenus` key leaves `settings.json`. An id that already exists in `.local` is kept, not overwritten, and the copy in `settings.json` is discarded rather than merged; the `.local` file is written before `settings.json` is touched.

**The back-fill runs first.** Before the compare, a non-empty array on a `codex`-stem agent that lacks the hooks-review pattern gets that entry appended (the #1757 back-fill, run once more inside the migration; a disabled copy counts as present). Two consequences:

- A codex array holding only the folder-trust entry, or one whose hooks-review entry was deleted by hand, becomes the shipped set. It is pristine, so it is dropped: that deletion is lost and the agent gets the shipped set back.
- A customised codex array such as `[folder-trust, custom]` is exported as `[folder-trust, custom, hooks-review]`, gaining an entry you never wrote. Remove it from the `.local` row afterwards if you do not want it.

**When the migration cannot run**, the arrays stay in place and apply exactly as before. The causes, each logged:

- `.local` exists but cannot be read.
- `.local` exists but does not parse or has the wrong shape.
- `.local` cannot be written.
- Two agents share an id with different arrays or different commands.
- The `agents` array is owned by `settings.local.json`.

AC retries on every settings load: every GUI start, every settings reload the running GUI performs, and every CLI verb that validates a session token (send, list-peers, close-session and the others). Verbs such as open-project, new-project and create-agent-matrix load settings another way and never retry. Each attempt logs one line until the cause is fixed: an error line, or for an overlay-owned `agents` array one info line per agent. For that overlay case, move the entries into `.local` by hand and delete those agents' `blockingMenus` keys from the overlay.

**Undoing the move** needs an older binary. With AC closed, copy each `byAgent.<id>` array from `.local` back under that agent as `blockingMenus` in `settings.json`, delete both new files, and run the older version. Starting the new version instead migrates again at once. Command-wide entries you added under `byCommand` have no place in `settings.json` and are lost by this undo.

## Settings

| Key | What it controls |
|---|---|
| `menuGuardEnabled` | Root switch for the whole feature. `true` by default. With `false`, each tick clears any session the guard was holding and evaluates nothing. |
| `blockingMenus` | Legacy. Moved to `settings-blocking-menus.local.json` on the first start after upgrade; while still present it applies as before. |

Two files next to `settings.json`, not keys in it, also control this feature: `settings-blocking-menus.json` (AC-owned, rewritten at start when it differs from the running version) and `settings-blocking-menus.local.json` (yours, read at start); see [Settings reference](../reference/settings.md#menu-guard) for their shape and precedence.

See [Settings reference](../reference/settings.md#menu-guard) for the full `BlockingMenuConfig` shape, field by field.

## Troubleshooting

**"My agent stalls on a dialog and AC says nothing."** Check whether your agent has any patterns at all. Only the `pi` and `codex` stems ship patterns; every other agent, Claude Code included, ships nothing. Add one, as in [Adding a pattern by hand](#adding-a-pattern-by-hand).

**"I added a pattern and it does nothing."** Five usual causes, in the order worth checking:

1. You edited `.local` while AC was running. It is read once at start, so restart AC.
2. The pattern does not compile. Look for `[menu_guard] Invalid regex pattern` in the log. Lookahead and backreferences are the common ones, and neither is supported.
3. The pattern is anchored past the row's real start. The row keeps its leading spaces and any box-drawing prefix, so `^Do you trust` fails on `| Do you trust ...`. Drop the `^`, or use `^[^A-Za-z0-9]*`.
4. You edited `settings-blocking-menus.json`. AC rewrites that file at start, so those edits are lost.
5. A higher layer holds an array for that agent. A `byAgent.<id>` row in `.local` - the migration writes one for every agent whose array was not the shipped set - replaces `byCommand`, and a legacy `blockingMenus` array still on the agent replaces both.

**"My edit disappeared."** One of three things happened. (a) You edited `settings.json` while AC was running: it loads that file into memory at startup, never refreshes that copy from disk, and writes it back on save, so the running app is authoritative until you close it. (b) You edited `settings-blocking-menus.json`: AC rewrites it at start, so those edits are lost. (c) The migration moved your legacy `blockingMenus` array from `settings.json` into `.local` under `byAgent.<id>`, so look for it there.

**"The pattern matches text I can see on one line, but nothing fires."** The line is wrapping across the top edge of the screen. A wrapped logical row that starts at physical row 0 is skipped, because its beginning may have scrolled away. Make the terminal wider, or scroll, and it evaluates on the next tick.

**"I get the row chip but no toast."** At most four toasts are visible at once, and the menu-guard toast is an info toast. Four unread sticky **error** toasts fill the cap, and the eviction rule protects errors, so the menu-guard toast is the one dropped. Dismiss the errors. The chip and `list-peers` still report the block either way. See [Notifications and dialogs](notifications-and-dialogs.md#toasts).

**"I clicked `Resolved by user` and it came back."** That is the re-arm working. Suppression lasts for the current episode only; once the menu leaves the screen and returns, it is a new episode and a new notice.

**"A message I sent to a blocked agent never arrived."** Expected while it is blocked. Writes into a blocked session are refused with `menu_guard_deferred` and the message is held, not rejected. Answer the menu, or click `Resolved by user`, and delivery resumes.

**"One bad entry broke my settings file."** It does not. An entry AC cannot read is kept verbatim, skipped at evaluation, and left in place. A whole file with the wrong shape - not an object, another `schemaVersion`, or the wrong type for `note`, `byCommand` or `byAgent` - is ignored whole with one error line, and the layers below it still apply. Every other entry and every other setting keeps working.

## See also

- [Notifications and dialogs](notifications-and-dialogs.md) - the toast this feature raises and how long toasts live
- [Watchers](watchers.md) - root-level patterns over the same terminal rows, for matches that are not blocking menus
- [Context tracking](context-tracking.md) - the other per-agent pattern on `AgentConfig`
- [Settings reference](../reference/settings.md#menu-guard) - `menuGuardEnabled` and the `BlockingMenuConfig` schema
- [CLI reference](../reference/cli.md#list-peers) - the verb that reports `blockedMenu`
