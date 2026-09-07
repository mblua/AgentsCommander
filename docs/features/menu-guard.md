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

**Every other coding agent starts with an empty array and detects nothing.** That includes Claude Code, Antigravity, and anything you added yourself. If you run one of those and want the guard to work, you write the pattern by hand; the section below shows how.

A stem with no defaults materializes to `"blockingMenus": []`, which is indistinguishable on disk from "I turned this off deliberately". That is intentional: see [Turning the guard off](#turning-the-guard-off).

One default arrived after the feature shipped, so AC back-fills it. On every load, an agent whose stem is `codex` and whose array is non-empty gets the hooks-review entry appended if no entry already carries that exact pattern. It is the only back-fill that touches `blockingMenus` today, it never runs on an empty array, and it never runs on an agent whose `agents` array comes from a `.local` overlay.

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

Sessions that are not agent sessions are never matched. A plain shell has no coding agent behind it, so it has no `blockingMenus` array and nothing to match against.

## Episodes and re-arming

An **episode** is one appearance of one menu. Episodes exist so that `Resolved by user` can silence a notice without silencing the feature.

- The first tick that matches a pattern opens an episode and raises the notice.
- Every later tick that matches the **same** pattern belongs to the same episode. AC publishes only on a real change of state, so you get one notice rather than four a second.
- `Resolved by user` suppresses the current episode. The session stops being blocked and the toast goes, even though the menu may still be on screen, and writes into the session are allowed again.
- When the menu **disappears**, AC clears the notice and forgets the suppression. The next appearance is a fresh episode and raises a fresh notice. That is the re-arm.
- A **different** pattern matching also opens a new episode, so a suppressed folder-trust prompt does not silence a hooks-review prompt that follows it.

Turning the guard off at the root also ends every episode it is currently holding: the next tick clears each blocked session, drops its toast and its chip, and lets writes through again. No session restart is needed for that; the app restart you need is the one that reads your edited `settings.json` in the first place.

## Adding a pattern by hand

There is no Settings screen and no CLI verb for `blockingMenus`. You edit `settings.json`.

**Close AgentsCommander first.** AC loads `settings.json` into memory once at startup and never refreshes that copy from disk - there is no settings file watcher - and any save from the running app writes its in-memory copy over the whole file. Edit while it is running and your pattern is gone at the next save.

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

### 3. Add it to the agent

Find the agent in `agents[]` and add the array. Adding it to the `claude` agent looks like this:

```json
{
  "id": "claude",
  "label": "Claude Code",
  "command": "claude",
  "color": "#E87B35",
  "blockingMenus": [
    {
      "pattern": "^[^A-Za-z0-9]*Do you trust the files in this folder\\?",
      "notification": "claude is waiting for you to answer the folder-trust menu in this terminal",
      "enabled": true,
      "capturedAgainst": "claude 2.1 / Windows"
    }
  ]
}
```

`notification` is what you will read on the toast and in `blockedMenuMessage`, so name the agent and say what to do. `capturedAgainst` is free text that AC never parses; it is there so that in a year you know which version the pattern was written against.

### 4. Restart AC and reproduce

Start AgentsCommander, launch the agent, and trigger the dialog again. Within about a quarter of a second you get the toast and the row chip. If nothing happens, see [Troubleshooting](#troubleshooting).

## Why `settings.local.json` does not help here

The `.local` overlay merges two objects key by key, but **anything that is not an object replaces the base value whole**. `agents` is an array, so an overlay that mentions `agents` at all replaces your entire agent catalogue with whatever the overlay lists. You cannot use it to add one pattern to one agent while leaving the other agents alone.

It is worse than merely useless: an overlay that owns `agents` also suppresses the codex hooks-review back-fill, and the base array is written back on every save. Put `blockingMenus` in `settings.json` itself.

The root switch is a different story. `menuGuardEnabled` is a plain boolean, so overriding it in `settings.local.json` behaves exactly as you would expect and touches nothing else.

## Turning the guard off

Three scopes, smallest first:

| What you want | What to write |
|---|---|
| Stop one pattern, keep the rest | `"enabled": false` on that entry |
| Stop every pattern for one agent | `"blockingMenus": []` on that agent |
| Stop the feature everywhere | `"menuGuardEnabled": false` at the root |

`[]` is durable. AC only fills in defaults for an agent whose `blockingMenus` is **absent**, so an empty array is never repopulated, and the codex back-fill skips it too. Deleting the key instead of emptying it gets you the defaults back on the next start.

One entry cannot be removed by deleting it: the Codex hooks-review pattern. The back-fill recognizes it by its pattern text and ignores `enabled`, so a deleted copy returns on the next load while `"enabled": false` survives. If you want that one off, disable it rather than delete it.

## Settings

| Key | What it controls |
|---|---|
| `menuGuardEnabled` | Root switch for the whole feature. `true` by default. With `false`, each tick clears any session the guard was holding and evaluates nothing. |
| `blockingMenus` | Per-agent array of patterns on each entry of `agents[]`. Absent means "fill in the defaults for my command"; `[]` means "off for this agent". |

See [Settings reference](../reference/settings.md#menu-guard) for the full `BlockingMenuConfig` shape, field by field.

## Troubleshooting

**"My agent stalls on a dialog and AC says nothing."** Check whether your agent has any patterns at all. Only the `pi` and `codex` stems ship defaults; every other agent, Claude Code included, materializes to `"blockingMenus": []`. Add a pattern, as in [Adding a pattern by hand](#adding-a-pattern-by-hand).

**"I added a pattern and it does nothing."** Three usual causes, in the order worth checking:

1. You edited while AC was running and a Settings save overwrote the file. Reopen `settings.json` and look for your entry.
2. The pattern does not compile. Look for `[menu_guard] Invalid regex pattern` in the log. Lookahead and backreferences are the common ones, and neither is supported.
3. The pattern is anchored past the row's real start. The row keeps its leading spaces and any box-drawing prefix, so `^Do you trust` fails on `| Do you trust ...`. Drop the `^`, or use `^[^A-Za-z0-9]*`.

**"My edit disappeared."** You edited while AC was running. AC loads the settings it runs on into memory at startup, never refreshes that copy from disk, and writes it back on save, so the running app is authoritative until you close it.

**"The pattern matches text I can see on one line, but nothing fires."** The line is wrapping across the top edge of the screen. A wrapped logical row that starts at physical row 0 is skipped, because its beginning may have scrolled away. Make the terminal wider, or scroll, and it evaluates on the next tick.

**"I get the row chip but no toast."** At most four toasts are visible at once, and the menu-guard toast is an info toast. Four unread sticky **error** toasts fill the cap, and the eviction rule protects errors, so the menu-guard toast is the one dropped. Dismiss the errors. The chip and `list-peers` still report the block either way. See [Notifications and dialogs](notifications-and-dialogs.md#toasts).

**"I clicked `Resolved by user` and it came back."** That is the re-arm working. Suppression lasts for the current episode only; once the menu leaves the screen and returns, it is a new episode and a new notice.

**"A message I sent to a blocked agent never arrived."** Expected while it is blocked. Writes into a blocked session are refused with `menu_guard_deferred` and the message is held, not rejected. Answer the menu, or click `Resolved by user`, and delivery resumes.

**"One bad entry broke my settings file."** It does not. An entry AC cannot read is kept verbatim, skipped at evaluation, and written back untouched on the next save. Every other entry and every other setting keeps working.

## See also

- [Notifications and dialogs](notifications-and-dialogs.md) - the toast this feature raises and how long toasts live
- [Watchers](watchers.md) - root-level patterns over the same terminal rows, for matches that are not blocking menus
- [Context tracking](context-tracking.md) - the other per-agent pattern on `AgentConfig`
- [Settings reference](../reference/settings.md#menu-guard) - `menuGuardEnabled` and the `BlockingMenuConfig` schema
- [CLI reference](../reference/cli.md#list-peers) - the verb that reports `blockedMenu`
