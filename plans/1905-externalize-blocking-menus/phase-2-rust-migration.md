# #1905 Phase 2: the one-shot migration out of `settings.json`

Class: design-bearing. Owner: Rust. Depends on: phase 1 (store, paths, `pretty_json_bytes`,
`BLOCKING_MENUS_SCHEMA_VERSION` and the constants must be on the branch). Parallel with: nothing.
Branch: `feature/1905-externalize-blocking-menus`. Base: `main` at `80aeb85`.
Line numbers are pinned to that base; phase 1 shifts `settings.rs` below `:1046`, so use the
symbol names as anchors there.

## Objective

The first GUI load after upgrade moves every `blockingMenus` array still in `settings.json` into
`settings-blocking-menus.local.json` (pristine arrays are dropped because the shipped file now
serves them), removes the key from `settings.json`, and never runs again. `materialize_blocking_menus`
disappears; the CLI loaders stop touching blocking menus.

## Files

1. `src-tauri/src/config/settings.rs`

## Decisions (inlined, binding)

- **Where:** `export_blocking_menus_to_local_file(&mut AppSettings, settings_path)` is called from
  the GUI loader only, in the `[settings-migration]` block, and sets `needs_save`.
- **When:** only while at least one agent has `blocking_menus == Some(_)`. After the first
  successful run no agent has `Some`, so it is a no-op forever (state-keyed idempotency).
- **Overlay-owned `agents`:** return `false` after one `info!` per affected agent naming the id and
  `settings-blocking-menus.local.json`; those arrays came from `settings.local.json`, which AC
  never writes, and the runtime ignores them since phase 1.
- **Pristine:** after applying `apply_issue_1757_migration` in memory (so a pre-#1757 codex array
  compares equal), an array equal to `default_blocking_menus_for_command(&agent.command)` produces
  no `.local` entry. Every other array, `[]` on a stem with defaults included, becomes a
  candidate `byAgent[agent.id]` copied verbatim, `Invalid` entries included.
- **Merge, no clobber:** the `.local` file is read as a raw `Value` object (absent = empty
  object); an existing `byAgent[id]` wins with an `info!`; unknown keys survive; `schemaVersion`
  is set to 1 if absent. The file is written with `pretty_json_bytes` through `write_file_atomic`
  only when at least one candidate was inserted.
- **Order and failure:** the `.local` write happens BEFORE the arrays are set to `None`. Any read,
  parse, schema, serialization or write failure logs one `error!` and returns `false` with
  nothing stripped, so the next start retries.
- **CLI loaders:** drop `materialize_blocking_menus` and `apply_issue_1757_migration` calls.
  `materialize_blocking_menus` is deleted. `apply_issue_1757_migration` survives, unchanged, as a
  step of the export and keeps its tests.
- **Field:** `AgentConfig.blocking_menus` keeps its type and serde attributes; only the export
  reads it; nothing writes `Some`.

## Edit 1: the export

Insert immediately after `apply_issue_1757_migration` (base `:1136`):

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
    if settings.local_overlay_state.owns_top_level(OVERLAY_KEY_AGENTS) {
        for agent in settings.agents.iter().filter(|a| a.blocking_menus.is_some()) {
            log::info!("[settings-migration] #1905 - agent '{}' carries blockingMenus inside an overlay-owned agents array; it is ignored, move it to {}", agent.id, BLOCKING_MENUS_LOCAL_FILE_NAME);
        }
        return false;
    }
    apply_issue_1757_migration(settings);
    let candidates: Vec<(String, Vec<BlockingMenuEntry>)> = settings
        .agents
        .iter()
        .filter_map(|agent| {
            let entries = agent.blocking_menus.as_ref()?;
            (*entries != default_blocking_menus_for_command(&agent.command))
                .then(|| (agent.id.clone(), entries.clone()))
        })
        .collect();
    let local_path = blocking_menus_local_path(settings_path);
    let mut root = match std::fs::read_to_string(&local_path) {
        Ok(contents) => match serde_json::from_str::<Value>(&contents) {
            Ok(Value::Object(map)) => map,
            Ok(_) => return abort_blocking_menus_export(&local_path, "the file is not a JSON object"),
            Err(e) => return abort_blocking_menus_export(&local_path, &format!("the file does not parse: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(e) => return abort_blocking_menus_export(&local_path, &format!("could not read the file: {e}")),
    };
    let expected = u64::from(BLOCKING_MENUS_SCHEMA_VERSION);
    if !root.get("schemaVersion").is_none_or(|v| v.as_u64() == Some(expected)) {
        return abort_blocking_menus_export(&local_path, "unsupported schemaVersion");
    }
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

`cargo fmt` rewraps the long lines; the log texts are binding.

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
- Delete `materialize_blocking_menus` and its doc comment (base `:1072-1084`).
- Doc comment of `AgentConfig.blocking_menus` (base `:83-87`) becomes: "#1905 - legacy. Read once
  by `export_blocking_menus_to_local_file`, which moves it to `settings-blocking-menus.local.json`
  and sets it to `None`; the runtime never reads it. No production writer sets `Some`."
- Doc comment of `apply_issue_1757_migration` (base `:1086-1104`): add one sentence, "Since #1905
  this runs only as the first step of `export_blocking_menus_to_local_file`."

## Edit 3: tests

Existing tests. `test_blocking_menus_defaults_materialization` (base `:9230`) becomes
`shipped_defaults_serve_pi_and_codex_only`: no `materialize` call; assert
`default_blocking_menus_for_command("pi").len() == 1`, `("codex").len() == 2` with `[1]` equal
to `BlockingMenuEntry::Valid(codex_hooks_review_menu())`, `("claude")` empty.
`test_blocking_menus_explicit_empty_array` (base `:9371`): drop the `materialize` half, keep the
parse assertion, add that `serde_json::to_string(&agent)` contains `"blockingMenus":[]`.
`issue_1757_reaches_settings_json_through_the_real_load_chain` (base `:9612`) becomes
`a_pre_1757_pristine_codex_array_is_dropped_by_the_export`: same fixture; after
`load_settings_from_path`, `value["agents"][0].get("blockingMenus").is_none()`, the `.local` path
does not exist, and the shipped path exists with bytes equal to
`pretty_json_bytes(shipped_blocking_menus())`. In
`an_overlay_owned_agents_array_suppresses_the_1757_migration` (base `:10845`) add, after the first
load, `assert!(!blocking_menus_local_path(&path).exists())`; its two numeric assertions stay true.

New tests in the phase-1 module `blocking_menus_1905`, reusing `seed`, `base_fixture` and
`disk_object` (base `:9703-9733`). Fixture entries: `ft` = codex folder-trust, `hooks` =
`codex_hooks_review_menu()`, `custom` = any valid entry with a distinct pattern.

- T6 `export_moves_customized_arrays_and_drops_pristine_ones_then_is_idempotent`: agents `pi` with
  `[]`, `codex` with `[ft, hooks]`, `codex-old` (command `codex`) with `[ft]`, `claude` with `[]`,
  `mine` (command `claude`) with `[custom, 12345]`. After load: no agent on disk has `blockingMenus`;
  `.local` parses with `schemaVersion == 1`, `byAgent` keys exactly `["mine", "pi"]`,
  `byAgent.pi == []`, `byAgent.mine` equal to the two elements verbatim; the shipped file exists.
  Load again: `.local`, shipped and `settings.json` bytes all unchanged.
- T7 `export_keeps_an_existing_local_entry_and_preserves_unknown_keys`: pre-seed `.local` with
  `{"extra": true, "byAgent": {"codex": [Z]}}`; agents `codex` with `[ft(enabled:false)]` and
  `mine` (command `claude`) with `[custom]`. After load: `byAgent.codex == [Z]`,
  `byAgent.mine == [custom]`, `extra == true`, `schemaVersion == 1`, no `blockingMenus` on disk.
  Also: `apply_issue_1757_migration` on a copy of the fixture settings yields
  `[ft(enabled:false), hooks]` for codex, proving the step runs before the comparison.
- T8 `export_refuses_when_the_local_file_is_unparseable`: `.local` is `{ not json`; after load the
  codex agent on disk still has its `blockingMenus` key and the `.local` bytes are unchanged.
- T9 `export_is_state_keyed`: a fixture with no `blockingMenus` key anywhere loads without creating
  the `.local` file and without a `[settings-migration] #1905` save (compare `settings.json`
  bytes before and after, allowing only the `rootToken` line the loader adds when absent; give
  the fixture a `rootToken` so the bytes are identical).

## Required behavior and failure behavior

- Upgrade with pristine arrays: `settings.json` loses every `blockingMenus` key, `.local` is not
  created, detection is unchanged.
- Upgrade with customized arrays: those arrays appear under `byAgent` in `.local`, verbatim.
- `.local` unreadable or invalid: nothing moves, one `error!`, retry next start.
- Settings save fails after the `.local` write: next start finds the ids present, inserts nothing,
  strips again.
- Overlay owns `agents`: nothing moves, one `info!` per agent with an array.

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
  prints `0`, and likewise for `load_settings_for_cli_strict\(`; control: the same pipeline over
  `load_settings_from_path\(` prints `1` or more.
- AC4 `rg -c "apply_issue_1757_migration\(" src-tauri/src/config/settings.rs` at the phase head
  equals the base count minus 3 (the deleted GUI and two CLI calls) plus 1 (the export step) plus
  the count added by T7; state all four numbers in the phase reply.
- AC5 The four verification commands exit 0; the filtered run reports 9 or more `blocking_menus_1905`
  tests passed (T1 to T4 from phase 1, T6 to T9 here) and the five renamed or amended tests pass.
- AC6 Manual, once, on a copy of a real `settings.json` carrying a `blockingMenus` array: after one
  launch the key is gone, the shipped file exists, `.local` holds only the non-pristine arrays; a
  second launch changes none of the three files. Quote the listings in the phase reply.
- AC7 CI on the exact PR head: `rust-regression`, `rust-regression-linux`, `rust-regression-macos`,
  `rust-fmt`, `test-debt` and the typecheck job all green.
- AC8 Cycle gate: `module-arcs.txt` regenerated as in phase 1 AC8 is byte-identical to the
  committed file (this phase adds no arc: `write_file_atomic` is already referenced from
  `settings.rs` since phase 1).

## Preserve

`apply_issue_1757_migration` body and its tests at base `:9505-9581`; `CODEX_HOOKS_REVIEW_PATTERN`;
`AgentConfig.blocking_menus` type and serde attributes; the PRESERVE-mode save path and the
`needs_save` backup logic around base `:2205-2235`; every other `[settings-migration]` step;
`commands/session.rs:11119`; everything phase 1 preserved.
