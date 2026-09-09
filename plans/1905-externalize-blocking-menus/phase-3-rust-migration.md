# #1905 Phase 3: the one-shot migration out of `settings.json`

Status: READY_FOR_IMPLEMENTATION
Class: design-bearing. Owner: Rust. Depends on: phase 1 (store, paths, `pretty_json_bytes`,
`parse_blocking_menus_file`, `BLOCKING_MENUS_SCHEMA_VERSION`, the constants) and phase 2 (the
guard resolves through `resolve_for`, so stripped arrays are served by the files).
Parallel with: nothing.
Branch: `feature/1905-externalize-blocking-menus`. Base: `main` at `80aeb85`.
Line numbers are pinned to that base; phase 1 shifts `settings.rs` below `:1046`, so use the
symbol names as anchors there.

## Objective

The first GUI load after upgrade moves every `blockingMenus` array still in `settings.json` into
`settings-blocking-menus.local.json` (pristine arrays are dropped because the shipped file now
serves them), removes the key from `settings.json`, and never runs again. `materialize_blocking_menus`
disappears; the CLI loaders stop touching blocking menus. Anything the export cannot move keeps
applying through the guard's layer 0.

## Files

1. `src-tauri/src/config/settings.rs`

## Decisions (inlined, binding)

- **Where:** `export_blocking_menus_to_local_file(&mut AppSettings, settings_path)` is called from
  the GUI loader only, in the `[settings-migration]` block, and sets `needs_save`.
- **When:** only while at least one agent has `blocking_menus == Some(_)`. After the first
  successful run no agent has `Some`, so it is a no-op forever (state-keyed idempotency).
- **Overlay-owned `agents`:** return `false` after one `info!` per affected agent; those arrays
  came from `settings.local.json`, which AC never writes, and they keep applying through layer 0.
- **Schema parity:** the complete `.local` text is validated with `parse_blocking_menus_file`
  before any mutation, and the merged bytes are validated with it again before the write. What
  the runtime rejects, the export refuses to touch.
- **Collision:** two agents with `Some` sharing an id and holding different arrays abort the
  export (nothing stripped, layer 0 keeps serving both); equal arrays are one candidate.
- **Pristine:** after `apply_issue_1757_migration` in memory, an array equal to
  `default_blocking_menus_for_command(&agent.command)` produces no `.local` entry. `[]` on `pi`
  or `codex` differs from the shipped set and is exported. `[]` on a stem whose shipped set is
  `[]` equals the shipped set and is dropped: it is exactly what `materialize_blocking_menus`
  wrote on every such agent (`settings.rs:1073-1083`, `docs/features/menu-guard.md:39`), and
  exporting it would freeze one `byAgent` row per agent against every future shipped pattern.
- **Merge, no clobber:** the `.local` file is read as a raw `Value` object (absent = empty
  object); an existing `byAgent[id]` wins with an `info!`; unknown keys survive; `schemaVersion`
  is set to 1 if absent. The file is written with `pretty_json_bytes` through `write_file_atomic`
  only when at least one candidate was inserted.
- **Order and failure:** the `.local` write happens BEFORE the arrays are set to `None`. Any read,
  parse, schema, collision, serialization or write failure logs one `error!` and returns `false`
  with nothing stripped, so the next start retries and layer 0 serves meanwhile.
- **CLI loaders:** drop `materialize_blocking_menus` and `apply_issue_1757_migration` calls.
  `materialize_blocking_menus` is deleted. `apply_issue_1757_migration` survives, unchanged, as a
  step of the export and keeps its tests.
- **Field:** `AgentConfig.blocking_menus` keeps its type and serde attributes; the export and
  layer 0 read it; nothing writes `Some`.

## Edit 1: the export

Insert immediately after `apply_issue_1757_migration` (base `:1137`):

```rust
/// Every failure path of the export: log, and leave settings.json untouched.
fn abort_blocking_menus_export(local_path: &Path, why: &str) -> bool {
    log::error!("[settings-migration] #1905 - {why} ({}); leaving settings.json untouched", local_path.display());
    false
}

/// #1905 (D7) - one-shot, state-keyed export of every `blockingMenus` array still in
/// `settings.json`. Writes the `.local` file before it strips anything; true means "save".
pub(crate) fn export_blocking_menus_to_local_file(settings: &mut AppSettings, settings_path: &Path) -> bool {
    if settings.agents.iter().all(|a| a.blocking_menus.is_none()) {
        return false;
    }
    let local_path = blocking_menus_local_path(settings_path);
    if settings.local_overlay_state.owns_top_level(OVERLAY_KEY_AGENTS) {
        for agent in settings.agents.iter().filter(|a| a.blocking_menus.is_some()) {
            log::info!("[settings-migration] #1905 - agent '{}' carries blockingMenus inside an overlay-owned agents array; it keeps applying from there and is not exported; move it to {} to use the new file", agent.id, BLOCKING_MENUS_LOCAL_FILE_NAME);
        }
        return false;
    }
    apply_issue_1757_migration(settings);
    // One id, one array: two agents sharing an id with different arrays cannot both live under
    // byAgent[id], so the export aborts and the guard's layer 0 keeps serving both.
    let mut by_id: BTreeMap<&str, (&str, &Vec<BlockingMenuEntry>)> = BTreeMap::new();
    for agent in &settings.agents {
        let Some(entries) = agent.blocking_menus.as_ref() else {
            continue;
        };
        if let Some((_, previous)) = by_id.insert(agent.id.as_str(), (agent.command.as_str(), entries)) {
            if previous != entries {
                return abort_blocking_menus_export(&local_path, &format!("two agents share the id '{}' with different blockingMenus arrays", agent.id));
            }
        }
    }
    let candidates: Vec<(String, Vec<BlockingMenuEntry>)> = by_id
        .iter()
        .filter(|(_, (command, entries))| **entries != default_blocking_menus_for_command(command))
        .map(|(id, (_, entries))| ((*id).to_string(), (*entries).clone()))
        .collect();
    let raw = match std::fs::read_to_string(&local_path) {
        Ok(contents) => Some(contents),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return abort_blocking_menus_export(&local_path, &format!("could not read the file: {e}")),
    };
    let mut root = match raw.as_deref() {
        None => Map::new(),
        Some(contents) => {
            // Schema parity: the runtime's own parser, on the complete file, before any mutation.
            if let Err(e) = parse_blocking_menus_file(contents) {
                return abort_blocking_menus_export(&local_path, &format!("the file {e}"));
            }
            match serde_json::from_str::<Value>(contents) {
                Ok(Value::Object(map)) => map,
                _ => return abort_blocking_menus_export(&local_path, "the file is not a JSON object"),
            }
        }
    };
    root.entry("schemaVersion").or_insert_with(|| Value::from(BLOCKING_MENUS_SCHEMA_VERSION));
    let Some(by_agent) = root.entry("byAgent").or_insert_with(|| Value::Object(Map::new())).as_object_mut() else {
        return abort_blocking_menus_export(&local_path, "byAgent is not an object");
    };
    let mut inserted = 0usize;
    for (id, entries) in candidates {
        if by_agent.contains_key(&id) {
            log::info!("[settings-migration] #1905 - {} already has byAgent['{id}']; keeping it", local_path.display());
            continue;
        }
        match serde_json::to_value(&entries) {
            Ok(value) => {
                by_agent.insert(id, value);
                inserted += 1;
            }
            Err(e) => return abort_blocking_menus_export(&local_path, &format!("could not serialize blockingMenus of '{id}': {e}")),
        }
    }
    if inserted > 0 {
        let bytes = match pretty_json_bytes(&Value::Object(root)) {
            Ok(bytes) => bytes,
            Err(e) => return abort_blocking_menus_export(&local_path, &format!("could not serialize the file: {e}")),
        };
        // Schema parity again, on the exact bytes about to be published.
        if let Err(e) = std::str::from_utf8(&bytes).map_err(|e| e.to_string()).and_then(parse_blocking_menus_file) {
            return abort_blocking_menus_export(&local_path, &format!("the merged file {e}"));
        }
        if let Err(e) = crate::config::local_config_io::write_file_atomic(&local_path, &bytes) {
            return abort_blocking_menus_export(&local_path, &format!("could not write the file: {e}"));
        }
        log::info!("[settings-migration] #1905 - exported {inserted} blockingMenus array(s) to {}", local_path.display());
    }
    for agent in &mut settings.agents {
        agent.blocking_menus = None;
    }
    true
}
```

`cargo fmt` rewraps the long lines; the log texts are binding. `by_id` borrows `settings.agents`
immutably and is last used building `candidates`, so the final `&mut` loop compiles.

## Edit 2: call sites and deletions

- GUI loader `load_settings_from_path`: replace the two blocks at base `:2185-2192` (the
  `materialize_blocking_menus` and `apply_issue_1757_migration` `if`s) with

```rust
    if export_blocking_menus_to_local_file(&mut settings, path) {
        log::info!("[settings-migration] #1905 - moved blockingMenus out of settings.json");
        needs_save = true;
    }
```

- `load_settings_for_cli`: delete base `:2341-2342`. `load_settings_for_cli_strict`: delete base
  `:2413-2414`. Nothing replaces them; the lockstep comment at `:2266-2270` stays true.
- Delete `materialize_blocking_menus` and its doc comment (base `:1072-1083`).
- Doc comment of `AgentConfig.blocking_menus` (base `:87-91`) becomes: "#1905 - legacy. Read by
  `export_blocking_menus_to_local_file`, which moves it to `settings-blocking-menus.local.json`
  and sets it to `None`, and by the menu guard as layer 0 while it is still present. No
  production writer sets `Some`."
- Doc comment of `apply_issue_1757_migration` (base `:1086-1104`): add one sentence, "Since #1905
  this runs only as the second step of `export_blocking_menus_to_local_file`."

## Edit 3: tests

Visibility (both edits inside `settings.rs`): in `mod local_overlay_1737`, change `fn base_fixture`
(`:9703`), `fn seed` (`:9712`), `fn disk_object` (`:9727`), `fn folder_trust_entry` (`:9738`) and
`fn codex_agent_with` (`:9747`) to `pub(super) fn`. In `mod blocking_menus_1905` (phase 1) add
`use super::local_overlay_1737::{base_fixture, codex_agent_with, disk_object, folder_trust_entry, seed};`
and `use std::time::Duration;`.

Existing tests. `test_blocking_menus_defaults_materialization` (base `:9230`) becomes
`shipped_defaults_serve_pi_and_codex_only`: both `materialize` uses (`:9273`, `:9327`) go; assert
`default_blocking_menus_for_command("pi").len() == 1`, `("codex").len() == 2` with `[1]` equal
to `BlockingMenuEntry::Valid(codex_hooks_review_menu())`, `("claude")` empty, keeping the
per-field assertions at `:9299-9324` over those returned vectors.
`test_blocking_menus_explicit_empty_array` (base `:9371`): drop the `materialize` half, keep the
parse assertion, add that `serde_json::to_string(&agent)` contains `"blockingMenus":[]`.
`issue_1757_reaches_settings_json_through_the_real_load_chain` (base `:9612`) becomes
`a_pre_1757_pristine_codex_array_is_dropped_by_the_export`: same fixture; after
`load_settings_from_path`, `value["agents"][0].get("blockingMenus").is_none()` and the `.local`
path does not exist; then `BlockingMenusStore::load_from_settings_path(&path).resolve_for(&settings.agents[0])`
equals `default_blocking_menus_for_command("codex")` (two entries) and the shipped path exists.
In `an_overlay_owned_agents_array_suppresses_the_1757_migration` (base `:10845`) add, after the
first load, `assert!(!blocking_menus_local_path(&path).exists())` and
`assert_eq!(BlockingMenusStore::load_from_settings_path(&path).resolve_for(&settings.agents[0]).len(), 1)`
(layer 0: the overlay's one-entry array, not the shipped two); its four numeric assertions stay true.

New tests in `blocking_menus_1905`. Fixture entries as JSON values: `ft` = `folder_trust_entry()`,
`hooks` = `serde_json::to_value(codex_hooks_review_menu())`, `custom` = a valid entry with pattern
`^custom-1905`, `custom2` = the same with pattern `^custom-1905-b`. `agent_json(id, command,
menus)` builds `{"id","label":id,"command","color":"#000000","blockingMenus": menus}` (the
`codex_agent_with` shape with id and command as parameters). Every test seeds `base_fixture()`
plus an `agents` array through `seed`, loads with `load_settings_from_path`, reads the disk with
`disk_object`, and checks resolution with `BlockingMenusStore::load_from_settings_path(&path).resolve_for(agent)`
over the agents of the returned settings. "Keys present" means every seeded agent still has its
`blockingMenus` array on disk, byte-equal to the seed after `1757` is applied where the seed was a
pristine one-entry codex array.

- T6 `export_moves_customized_arrays_and_drops_pristine_ones_then_is_idempotent`: agents `pi`
  with `[]`, `codex` with `[ft, hooks]`, `codex-old` (command `codex`) with `[ft]`, `codex-off`
  (command `codex`) with `[]`, `claude` with `[]`, `mine` (command `claude`) with `[custom, 12345]`.
  After load: no agent on disk has `blockingMenus`; `.local` parses with `schemaVersion == 1`,
  `byAgent` keys exactly `["codex-off", "mine", "pi"]`, `byAgent.pi == []`, `byAgent["codex-off"] == []`,
  `byAgent.mine` equal to the two elements verbatim; resolution: `pi` and `codex-off` empty,
  `codex` and `codex-old` equal to the shipped codex set, `claude` empty, `mine` two entries.
  Load again: `.local` and `settings.json` bytes unchanged.
- T7 `export_keeps_an_existing_local_entry_and_preserves_unknown_keys`: pre-seed `.local` with
  `{"extra": true, "byAgent": {"codex": [custom]}}`; agents `codex` with `[ft(enabled:false)]`
  and `mine` (command `claude`) with `[custom2]`. After load: `byAgent.codex == [custom]`,
  `byAgent.mine == [custom2]`, `extra == true`, `schemaVersion == 1`, no `blockingMenus` on disk;
  `resolve_for(codex) == [custom]`. Also: `apply_issue_1757_migration` on a copy of the fixture
  settings yields `[ft(enabled:false), hooks]` for codex, proving the step runs before the comparison.
- T8 `export_refuses_when_the_local_file_is_unparseable`: `.local` is `{ not json`; agents
  `codex` with `[ft]` and `mine` (command `claude`) with `[custom]`. After load: keys present,
  `.local` bytes unchanged, `resolve_for(codex) == [ft, hooks]` (layer 0 with the in-memory
  1757 step, what the current binary serves) and `resolve_for(mine) == [custom]`.
- T9 `export_is_state_keyed`: a fixture with no `blockingMenus` key anywhere loads without creating
  the `.local` file and with `settings.json` bytes identical before and after (`base_fixture`
  carries a `rootToken`, so no other migration writes).
- T10 `export_aborts_when_the_local_file_cannot_be_written`: create a DIRECTORY at
  `dir.join(format!(".settings-blocking-menus.local.json.{}.tmp", std::process::id()))` (the
  name `write_file_atomic` will try to create, `local_config_io.rs:122-128`); agents `mine`
  (command `claude`) with `[custom]`. After load: keys present, `.local` absent,
  `resolve_for(mine) == [custom]`. Remove the directory and load again: `.local` exists with
  `byAgent.mine == [custom]` and the key is gone (the retry).
- T11 `a_failed_settings_save_after_a_successful_export_is_completed_at_the_next_start`: agents
  `codex` with `[ft, hooks]` and `mine` (command `claude`) with `[custom]`. Hold
  `let held = SettingsFileLock::acquire(&path, Duration::from_millis(50)).unwrap();` then load
  (the loader's own save waits its 2-second timeout and fails; this test takes about 2 seconds
  by design). After load: `.local` exists with `byAgent.mine == [custom]`; keys present on disk;
  `resolve_for(mine) == [custom]` (served by the file, the in-memory array is `None`). Drop
  `held`, record the `.local` bytes, load again: keys gone, `.local` bytes identical,
  `resolve_for(mine) == [custom]`, `resolve_for(codex)` equal to the shipped codex set.
- T12 `a_restart_between_the_two_writes_finishes_the_export`: seed `.local` as
  `{"schemaVersion": 1, "byAgent": {"mine": [custom]}}` and agents `codex` with `[ft, hooks]`,
  `mine` (command `claude`) with `[custom]` (the exact on-disk state T11 leaves after its first
  load). Load: keys gone, `.local` bytes identical to the seed, `resolve_for(mine) == [custom]`.
- T13 `schema_parity_rejects_what_the_runtime_rejects`: for each `.local` text in
  `{"schemaVersion":1,"byCommand":42,"byAgent":{}}`, `{"byAgent":{"codex":42}}`,
  `{"note":42,"byAgent":{}}`, `{"schemaVersion":2}`, `[1]`: fresh tempdir, agents `codex` with
  `[ft]` and `mine` (command `claude`) with `[custom]`; after load: keys present, `.local` bytes
  unchanged, `resolve_for(mine) == [custom]`, and `parse_blocking_menus_file(text).is_err()`.
- T14 `duplicate_agent_ids_abort_unless_their_arrays_are_equal`: agents `dup` (command `claude`)
  with `[custom]` and a second `dup` (command `claude`) with `[custom2]`: after load keys present,
  `.local` absent, `resolve_for` of each loaded agent equals its own array. Second tempdir, both
  `dup` with `[custom]`: after load keys gone, `byAgent` keys exactly `["dup"]`, `byAgent.dup == [custom]`.

## Required behavior and failure behavior

- Upgrade with pristine arrays: `settings.json` loses every `blockingMenus` key, `.local` is not
  created, detection is unchanged.
- Upgrade with customized arrays (a custom entry, a disabled entry, an invalid entry, `[]` on
  `pi` or `codex`): those arrays appear under `byAgent` in `.local`, verbatim.
- `.local` unreadable, invalid, or unwritable; duplicate ids with different arrays: nothing moves,
  one `error!`, retry next start, layer 0 serves the arrays meanwhile.
- Settings save fails after the `.local` write: the files serve the same entries; next start finds
  the ids present, inserts nothing, strips again.
- Overlay owns `agents`: nothing moves, one `info!` per agent with an array, layer 0 serves them.

## Verification (from `src-tauri`)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --lib -- blocking_menus_1905 shipped_defaults_serve a_pre_1757 explicit_empty issue_1757 an_overlay_owned_agents
cargo test --lib --bins --tests
```

## Acceptance criteria

- AC1 `git diff --name-only HEAD~1..HEAD` lists exactly `src-tauri/src/config/settings.rs`.
- AC2 `rg -n "materialize_blocking_menus" src-tauri/src` prints nothing and exits 1; control:
  `rg -n "export_blocking_menus_to_local_file" src-tauri/src/config/settings.rs` prints 2 or more lines.
- AC3 `awk '/^pub fn load_settings_for_cli\(/,/^}/' src-tauri/src/config/settings.rs | rg -c "blocking_menus|1757"`
  prints nothing and exits 1, and likewise for `load_settings_for_cli_strict\(`; control: the same
  pipeline over `load_settings_from_path\(` prints `1` or more and exits 0.
- AC4 `rg -c "apply_issue_1757_migration\(" src-tauri/src/config/settings.rs` at the phase head
  equals the base count minus 3 (the deleted GUI and two CLI calls) plus 1 (the export step) plus
  the count added by T7; state all four numbers in the phase reply.
- AC5 The four verification commands exit 0; the filtered run reports 13 or more
  `blocking_menus_1905` tests passed (T1 to T4 from phase 1, T6 to T14 here) and the five renamed
  or amended tests pass; T11 is allowed to take about 2 seconds.
- AC6 Manual, once, on a copy of a real `settings.json` carrying a `blockingMenus` array: after one
  launch the key is gone, the shipped file exists, `.local` holds only the non-pristine arrays; a
  second launch changes none of the three files. Quote the listings in the phase reply.
- AC7 CI on the exact PR head: `rust-regression`, `rust-regression-linux`, `rust-regression-macos`,
  `rust-fmt`, `test-debt` and `frontend-regression` all green.
- AC8 Cycle gate, clean tree at the phase head, working directory the repository root, `SCRATCH`
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

  Expected: both baselines exit 0 with 1 cycle of 86 members; `SCC member sets identical`, exit 0;
  `--emit-graph` exits 1; `record:arcs` exits 0; `git status` prints nothing (this phase adds no
  arc: `write_file_atomic` is already referenced from `settings.rs` since phase 1).

## Preserve

`apply_issue_1757_migration` body and its tests at base `:9505-9581`; `CODEX_HOOKS_REVIEW_PATTERN`;
`AgentConfig.blocking_menus` type and serde attributes; `SettingsFileLock` and the PRESERVE-mode
save path with the `needs_save` backup logic around base `:2205-2235`; every other
`[settings-migration]` step; `commands/session.rs:11119`; everything phases 1 and 2 preserved.
