# #1905 Phase 2: the menu guard reads the store

Status: READY_FOR_IMPLEMENTATION
Class: patterned (mirrors how `MenuGuard` already receives its collaborators and how the #1757
T11 test replays a frame; no new abstraction). Owner: Rust. Depends on: phase 1
(`BlockingMenusStore`, `resolve_for`, `load_from_config_dir` must be on the branch).
Parallel with: nothing.
Branch: `feature/1905-externalize-blocking-menus`. Base: `main` at `80aeb85`.
Line numbers are pinned to that base; phase 1 does not touch either file of this phase.

## Objective

`MenuGuard` owns a `BlockingMenusStore` and resolves each session's entries through
`store.resolve_for(agent)` instead of cloning `agent.blocking_menus`. Because layer 0 of the
store is the agent's own array and every agent still carries a materialized array until phase 3,
this phase changes no behavior; it wires the consumer so phase 3 can strip the arrays.

## Files

1. `src-tauri/src/pty/menu_guard/mod.rs`
2. `src-tauri/src/lib.rs`

## Decisions (inlined, binding)

- `MenuGuard::new()` keeps its signature and evaluates the shipped set only
  (`BlockingMenusStore::shipped_only()`), so the three harnesses that call it
  (`commands/session.rs:6264`, `phone/mailbox.rs:26939`, `pty/inject.rs:1008`) and every test in
  `menu_guard/mod.rs` keep compiling and keep their meaning.
- `MenuGuard::with_store(store)` is the production constructor; `lib.rs` builds the store with
  `BlockingMenusStore::load_from_config_dir()` after `load_settings()` (`lib.rs:2648`) has run.
- `entries_for(&self, agent: &AgentConfig)` is the one seam between the scan loop and the store;
  the scan loop's only change is the call.

## Edit 1: `src-tauri/src/pty/menu_guard/mod.rs`

- Line 14 becomes
  `use crate::config::settings::{AgentConfig, BlockingMenuConfig, BlockingMenuEntry, BlockingMenusStore, SettingsState};`.
- `MenuGuard` (`:46-50`) gains a fourth field `store: BlockingMenusStore`; `impl Default`
  (`:52-60`) sets `store: BlockingMenusStore::shipped_only()`. Add to `impl MenuGuard`, after
  `new` (`:63-65`):

```rust
    /// #1905 - production constructor: the store built from the config dir at startup.
    pub fn with_store(store: BlockingMenusStore) -> Self {
        Self {
            store,
            ..Self::default()
        }
    }

    /// #1905 - the entries evaluated for one agent's sessions (D3, layer 0 first).
    pub(crate) fn entries_for(&self, agent: &AgentConfig) -> Vec<BlockingMenuEntry> {
        self.store.resolve_for(agent)
    }
```

- Scan loop `:265-270` becomes:

```rust
            let entries: Vec<BlockingMenuEntry> = session
                .agent_id
                .as_deref()
                .and_then(|aid| settings.agents.iter().find(|a| a.id == aid))
                .map(|agent| self.entries_for(agent))
                .unwrap_or_default();
```

## Edit 2: tests in the existing `mod tests` of `menu_guard/mod.rs`

Add `use crate::config::settings::BlockingMenusStore;` next to the existing test import at
`:389`. Fixtures: `agent(id, command, blocking_menus)` is a local helper building an
`AgentConfig` with the eleven fields exactly as `settings.rs:9430-9442` does (`label = id`,
`color = "#000000"`, empty `envs`, `isolated_home: false`, every `Option` `None`,
`backend: Default::default()`). `CUSTOM` is the entry
`{"pattern": "^\\s*Do you trust the files in this folder\\?", "notification": "claude is waiting for you to answer the folder-trust menu in this terminal", "enabled": true, "capturedAgainst": "claude 2.1 / Windows"}`
(the page's own example, `docs/features/menu-guard.md:113-118`), and `CLAUDE_ROW` is a
`LogicalRow` with text `  Do you trust the files in this folder? (y/n)`.

- T5a `a_custom_pattern_on_disk_reaches_the_evaluator_through_the_production_store`: tempdir;
  write `settings-blocking-menus.local.json` with `byAgent = {"claude-1": [CUSTOM],
  "claude-off": [CUSTOM with "enabled": false], "pi-off": []}`; build
  `BlockingMenusStore::load_from_settings_path(&dir.join("settings.json"))` (the production
  loader: it also writes the shipped file, assert it exists) and `MenuGuard::with_store(store)`.
  Then, each on a fresh `Uuid::new_v4()`:
  1. `entries_for(&agent("claude-1", "claude", None))` has length 1, and
     `evaluate_logical_rows(id, &[CLAUDE_ROW], &entries)` gives `is_blocked` with
     `matched_notification` equal to the CUSTOM notification. Control: `MenuGuard::new()` gives an
     empty `entries_for` for the same agent and `!is_blocked` on the same row, so a store that
     always returns `[]` fails this test.
  2. `entries_for(&agent("claude-off", "claude", None))` has length 1 and the evaluation is
     `!is_blocked` (disabled entry).
  3. `entries_for(&agent("pi-off", "pi", None))` is empty and the evaluation of the row
     `Trust project folder?` is `!is_blocked`, while `MenuGuard::new()` blocks that row for the
     same agent (explicit empty array in `.local` switches the shipped set off).
- T5b `a_legacy_array_on_the_agent_wins_over_both_files`: same store; `agent("claude-1",
  "claude", Some(vec![]))` gives empty entries and `!is_blocked` on `CLAUDE_ROW` even though
  `byAgent["claude-1"]` is `[CUSTOM]`; `agent("pi-off", "pi", Some(vec![CUSTOM]))` gives one
  entry and `is_blocked` on `CLAUDE_ROW` although `byAgent["pi-off"]` is `[]`; `agent("pi-2",
  "pi", Some(default_blocking_menus_for_command("pi")))` blocks the row `Trust project folder?`
  (the materialized state every agent is in until phase 3).

Tests at `:395`, `:433`, `:483`, `:534` are unchanged.

## Edit 3: `src-tauri/src/lib.rs`

Line 2969 becomes

```rust
            let menu_guard = Arc::new(crate::pty::menu_guard::MenuGuard::with_store(
                config::settings::BlockingMenusStore::load_from_config_dir(),
            ));
```

(`cargo fmt` decides the wrapping). It runs inside `setup`, after `load_settings()` at `:2648`.
`lib.rs:4601` is untouched.

## Required behavior and failure behavior

- Every agent still has `Some(array)` (materialized by the untouched loader), so the guard
  evaluates exactly what it evaluated at base; this phase is green on `main` by itself.
- At startup the store refresh writes or leaves the shipped file (phase 1 D5) and reads the
  `.local` file once; a broken `.local` logs once and the shipped set serves for agents with no
  array.
- `menuGuardEnabled: false` still clears every held session on the next tick (untouched code).

## Verification (from `src-tauri`)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --lib -- menu_guard
cargo test --lib --bins --tests
```

## Acceptance criteria

- AC1 `git diff --name-only HEAD~1..HEAD` lists exactly the two files above.
- AC2 `rg -n "\.blocking_menus" src-tauri/src/pty/menu_guard/mod.rs` prints nothing and exits 1;
  control: `rg -n "resolve_for|entries_for" src-tauri/src/pty/menu_guard/mod.rs` prints 4 or more
  lines.
- AC3 `rg -n "MenuGuard::new\(\)" src-tauri/src` prints the same 8 lines as at base
  (`commands/session.rs:6264`, `phone/mailbox.rs:26939`, `pty/inject.rs:1008`,
  `menu_guard/mod.rs:396,434,484,535` shifted by this phase's insertions) minus `lib.rs:2969`,
  which now reads `with_store`.
- AC4 The four verification commands exit 0; the filtered run reports the 2 new tests (T5a, T5b)
  passed and every pre-existing `menu_guard` test still passes.
- AC5 Manual, once: launch the phase head with an empty config dir; `settings-blocking-menus.json`
  appears with the phase 1 Edit 1 content re-serialized (LF, trailing newline); launch again and
  its modification time does not change. Quote both listings in the phase reply.
- AC6 CI on the exact PR head: `rust-regression`, `rust-regression-linux`, `rust-regression-macos`,
  `rust-fmt`, `test-debt` and `frontend-regression` all green.
- AC7 Cycle gate, clean tree at the phase head, working directory the repository root, `SCRATCH`
  outside the repository:

```
VAULT="../repo-personal/ObsidianVault/Coding Agents/IA-Programming/rust"
mkdir -p "$SCRATCH/base" && git archive 80aeb85 src-tauri | tar -x -C "$SCRATCH/base"
node "$VAULT/01-rust_module-dependency-cycles.mjs" "$SCRATCH/base/src-tauri" --write-baseline "$SCRATCH/pre-cycles.json" --quiet
node "$VAULT/01-rust_module-dependency-cycles.mjs" src-tauri --write-baseline "$SCRATCH/post-cycles.json" --quiet
node -e "const p=require('path');const [a,b]=process.argv.slice(1).map(f=>require(p.resolve(f)).moduleCycles.map(c=>[...c.members].sort().join('|')).sort());const same=JSON.stringify(a)===JSON.stringify(b);console.log(same?'SCC member sets identical':'SCC member sets differ');process.exit(same?0:1)" "$SCRATCH/pre-cycles.json" "$SCRATCH/post-cycles.json"
node "$VAULT/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph "$SCRATCH/post.json" --quiet
npm run record:arcs -- --graph "$SCRATCH/post.json"
git status --porcelain src-tauri/module-arcs.txt
```

  Expected: both baselines exit 0 with 1 cycle of 86 members; the comparison prints
  `SCC member sets identical` and exits 0; `--emit-graph` exits 1; `record:arcs` exits 0; `git
  status` prints nothing (this phase adds no arc: `menu_guard -> settings` and `lib -> settings`
  are `module-arcs.txt:889` and `:16` at base).

## Preserve

`evaluate_logical_rows`, `is_blocked`, `resolve_current_episode`, `scan_tick` apart from the six
lines of Edit 1, `start`; `ERR_MENU_GUARD_DEFERRED`; the T11 replay test at `:534`; the three
`MenuGuard::new()` harness sites; `lib.rs:2648` and `:4601`; everything phase 1 preserved.
