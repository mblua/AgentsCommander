# Plan #1912: code-level support switch for built-in coding agents (read gate + seed gate)

Status: READY_FOR_IMPLEMENTATION

Base: `main` = `origin/main` = `4ba6c99e30f2f3ba3d47a5b37e846f38c91d015d`. Branch `feature/1912-coding-agent-support-switch`. Every line number below was measured at that commit, BEFORE any edit; anchor by symbol when implementing, because the new code above line 198 shifts everything after it. Band Lite 1-25, score 25 (re-scored after the 2026-09-09 seed-gating scope change). Verification-difficulty veto in force: the reviewer checks the proof itself (section 8, positive controls). Plan author: ac-dev-rust-v4. Plan reviewer: ac-dev-rust-grinch-v4.

## 1. Issue and objective

Issue #1912: "Code-level enable/disable switch for built-in coding agents in the catalog".

Add ONE explicit, reviewable control in code that turns each built-in coding agent on or off. A `false` row must make that agent disappear from every consumer (Welcome pick list, Settings preset buttons, Settings Auto-update table, startup update prompt, WebSocket route, CLI `coding-agent catalog` / `add --from-catalog`) on every install, including installs whose `agents.json` was seeded long ago, without rewriting any user-owned file. Since the 2026-09-09 scope change, a `false` row must ALSO be absent from what AC seeds on a new install or registration: the catalog manifest and the `_seed/<dest>` config-folder master, and its "Re-seed default configuration" button must not be offered.

This issue lands the mechanism with EVERY row `true`. Which keys go `false` is a later one-line follow-up (section 11 is the inventory that follow-up needs).

## 2. Cause, with evidence

Nothing is broken; the control does not exist. Verified at `4ba6c99e`:

| Fact | Where |
|---|---|
| Embedded default: 8 agents in this order: claude, codex, hermes, cursor, pi, opencode, antigravity, muse; `schemaVersion` 1; muse has no `instructionsFilename` and no `updateCommands` | `src-tauri/resources/coding-agents/agents.default.json`; `coding_agents_catalog.rs:56-57` (`include_str!`) |
| No `enabled`/`hidden`/platform field, no `cfg(feature)`, no `[features]` table | `coding_agents_catalog.rs:80-123` (`CodingAgentDefinition`); `src-tauri/Cargo.toml` |
| The single read funnel: `validate_and_filter(agents, source)` (validate, then first-wins dedup on `key`) | `coding_agents_catalog.rs:198-219` |
| Its only two callers: `validated_embedded_default` (224) and the parsed arm of `load_catalog` (291) | `:223-229`, `:271-305` |
| `load_catalog` returns `validated_embedded_default()` on missing (280), unreadable (287) and corrupt (302) manifests | `:271-305` |
| Backfill source is `validated_embedded_default()` (240), matched by `command` | `:237-259` |
| `load_catalog_for_settings`: primary project `.ac` (363) else legacy config dir (365) else embedded (366) | `:361-368` |
| Every external reader calls `load_catalog_for_settings` and nothing else | `commands/config.rs:521` (IPC + WS), `agent_update.rs:1665` (Settings overview) and `:3225` (startup pass), `cli/coding_agent.rs:247` and `:270` |
| Ungated raw door: `pub fn embedded_default_catalog()`; production callers outside the module: none (grep of `src-tauri/src`, `src-tauri/tests`) | `:146` |
| Seed-once manifest write: any existing entry means user-owned; embedded bytes written verbatim in three arms | `ensure_seeded`, `:383-472`; raw bytes at `:425`, `:430`, `:432` |
| Legacy regular file is copied VERBATIM, even when corrupt | `:411-423`, `:436-446` |
| Masters are command-basename-keyed, not key-keyed: `EmbeddedSeedMaster { command_basename, dest, files }`, three masters claude/codex/opencode | `:541-580` |
| Masters consumers: `embedded_master_for_command_basename` (600, used by `reseed_master_for_command` 903-906), `reseedable_command_basenames` (610, IPC `list_reseedable_agent_commands` at `commands/config.rs:531`), `ensure_seeded_masters` loop (671) | `:600-616`, `:670-757` |
| Steady-state pre-check requires EVERY master dir | `ensure_seeded_for_project_with_token`, `:798-804` |
| Every master maps to an embedded def by basename (so a key-to-basename mapping exists) | test `every_embedded_master_maps_to_a_catalog_def_with_matching_configseed`, `:1362-1388` |
| Settings Auto-update rows = one per catalog entry with non-empty `update_commands` | `agent_update.rs:1516-1535` (`build_update_overview_rows`), fed at `:1665` |
| Startup prompt rule 0: registered AND in catalog | `agent_update.rs:1459-1463`, `:1472-1485` |
| Frontend fallback has 7 entries (no muse), served only when the IPC call rejects; its test says "7 built-ins" | `src/shared/agent-presets.ts:3-97`; `src/sidebar/stores/coding-agents.ts:6,50`; `src/shared/agent-presets.test.ts:7,26-35` |
| Test-only override precedent (task-local + `with_..._for_test`) | `src-tauri/src/agent_version.rs:35-62` |

Baselines, run at `4ba6c99e` on this box: `cargo test --lib config::coding_agents_catalog` 38 passed; `cargo test --lib get_coding_agent_catalog_route_returns_backfilled_catalog` 1 passed; `cargo test --test cli_project_registration -- new_project_seeds_catalog_into_ac cli_catalog_serves_legacy_fallback_then_embedded_without_projects` 2 passed; `npx vitest run src/shared/agent-presets.test.ts src/sidebar/stores/coding-agents.test.ts src/sidebar/components/SettingsModal.catalog.test.tsx` 19 passed.

## 3. Scope

In scope (Reading A + seed gating, user-confirmed):

- The `BUILTIN_AGENT_SUPPORT` table, the read gate in `validate_and_filter`, the test override, the seed gate on the embedded manifest bytes and on the masters, the pre-check predicate, `pub(crate)` on `embedded_default_catalog`, doc comments.
- Frontend mirror: `FALLBACK_CODING_AGENTS` gains `muse` so the rule "fallback = enabled rows of the table" holds at landing; its drift test moves to 8.
- New Rust tests with positive controls through the override; the `EmbeddedSeedMaster.key` pin.

Out of scope, decided: deleting entries from `agents.default.json`; touching `settings.agents`; platform gating; trimming `agent_version.rs:64` probe stems; docs under `docs/` and `CHANGELOG.md` (doc comments only); flipping any row to `false`; the stale "six built-ins" comments at `SettingsModal.catalog.test.tsx:114` and `cli_project_registration.rs:738`.

## 4. Decided solution

### 4.1 The table (`coding_agents_catalog.rs`, directly after `EMBEDDED_DEFAULT_CATALOG_JSON`, line 57)

```rust
/// #1912 - the ONLY place a built-in coding agent is turned on or off. One row
/// per key in `agents.default.json`, same order (a test pins both). `false` =
/// de-supported: dropped by `validate_and_filter` on EVERY read path (embedded
/// default, project manifest, legacy manifest, backfill source), omitted from
/// the embedded bytes `ensure_seeded` writes, and its config-folder master is
/// neither seeded nor re-seedable. Already-seeded files are never rewritten or
/// trimmed; the read gate covers them. A key absent from this table (a
/// user-authored entry) is always kept.
pub(crate) const BUILTIN_AGENT_SUPPORT: &[(&str, bool)] = &[
    ("claude", true),
    ("codex", true),
    ("hermes", true),
    ("cursor", true),
    ("pi", true),
    ("opencode", true),
    ("antigravity", true),
    ("muse", true),
];
```

Identity is the catalog `key` (documented as the stable identity at `:80-82`), never the command: a user-authored entry with another key but the same command is user data and is kept.

### 4.2 The read gate (`validate_and_filter`, line 198)

Predicate and active-table helpers, placed directly above `validate_and_filter`:

```rust
/// `false` only for a key present in `table` with a `false` row.
fn is_supported_builtin(key: &str, table: &[(&str, bool)]) -> bool {
    !table.iter().any(|(k, on)| *k == key && !*on)
}

/// The table in force: the shipped const, or the test override on this thread.
fn active_builtin_agent_support() -> &'static [(&'static str, bool)] {
    #[cfg(test)]
    if let Some(table) = BUILTIN_AGENT_SUPPORT_OVERRIDE.with(|cell| cell.get()) {
        return table;
    }
    BUILTIN_AGENT_SUPPORT
}
```

Loop body order in `validate_and_filter` (decided, the dedup-order correction from the feasibility read): validate (existing) -> `seen_keys.insert` (existing) -> support gate (new) -> push. The gate runs AFTER the key is recorded in `seen_keys`, so a second entry with the de-supported key in a user file is still a duplicate and cannot resurrect it:

```rust
    let table = active_builtin_agent_support();
    for def in agents {
        if let Err(e) = validate_definition(&def) { /* unchanged */ }
        if !seen_keys.insert(def.key.clone()) { /* unchanged */ }
        if !is_supported_builtin(&def.key, table) {
            log::info!(
                "[coding-agents] skipping de-supported built-in '{}' in {source}",
                def.key
            );
            continue;
        }
        out.push(def);
    }
```

`info`, not `warn`: a de-supported row is an expected state, not a defect. Signature of `validate_and_filter` unchanged; its two callers unchanged.

### 4.3 Test mechanism: ONE `#[cfg(test)]` thread-local override (decided; twins rejected)

```rust
#[cfg(test)]
thread_local! {
    static BUILTIN_AGENT_SUPPORT_OVERRIDE:
        std::cell::Cell<Option<&'static [(&'static str, bool)]>> =
        const { std::cell::Cell::new(None) };
}

/// #1912 test-only: run `f` with `table` in force instead of
/// `BUILTIN_AGENT_SUPPORT` on the CURRENT THREAD; the previous value is
/// restored when `f` returns or panics. Sync closures only: never use it from
/// a multi-thread `#[tokio::test]`.
#[cfg(test)]
pub(crate) fn with_builtin_agent_support_for_test<R>(
    table: &'static [(&'static str, bool)],
    f: impl FnOnce() -> R,
) -> R {
    struct Restore(Option<&'static [(&'static str, bool)]>);
    impl Drop for Restore {
        fn drop(&mut self) {
            BUILTIN_AGENT_SUPPORT_OVERRIDE.with(|cell| cell.set(self.0));
        }
    }
    let _restore =
        Restore(BUILTIN_AGENT_SUPPORT_OVERRIDE.with(|cell| cell.replace(Some(table))));
    f()
}
```

Why this and not five `_with(table)` twins: the override reaches the PRODUCTION wrappers (`load_catalog`, `load_catalog_for_settings`, the backfill, `ensure_seeded`, `ensure_seeded_masters`, `reseedable_command_basenames`, `reseed_master_for_command`, the pre-check) with zero signature changes, so every positive control in section 8 exercises the real call chain; twins would test copies and leave the wrapper-to-const link unproven. Precedent: `agent_version.rs:35-62`. The catalog path is synchronous, so `thread_local!` replaces `tokio::task_local!`. All new tests that use it are plain `#[test]`.

### 4.4 Seed gate

**4.4.1 Manifest bytes.** New helper, used in the THREE arms of `ensure_seeded` that today write `EMBEDDED_DEFAULT_CATALOG_JSON.as_bytes().to_vec()` (lines 425, 430, 432):

```rust
/// #1912 - the bytes `ensure_seeded` writes when seeding from the embedded
/// default. Every row enabled (the shipped state): the raw resource, byte-
/// identical to today. Otherwise the enabled rows, re-serialized (pretty, one
/// trailing newline). The unreachable serialization error (the struct round-
/// trips in `embedded_default_matches_current_presets_exactly`) logs `error`
/// and falls back to the raw bytes: a seeded-but-hidden key is recoverable
/// through the read gate, an unseeded project is not better.
fn embedded_seed_bytes() -> Vec<u8> {
    let table = active_builtin_agent_support();
    if table.iter().all(|(_, on)| *on) {
        return EMBEDDED_DEFAULT_CATALOG_JSON.as_bytes().to_vec();
    }
    let mut catalog = embedded_default_catalog();
    catalog.agents.retain(|def| is_supported_builtin(&def.key, table));
    match serde_json::to_vec_pretty(&catalog) {
        Ok(mut bytes) => { bytes.push(b'\n'); bytes }
        Err(e) => {
            log::error!("[coding-agents] failed to serialize the filtered embedded catalog ({e}); seeding the raw resource");
            EMBEDDED_DEFAULT_CATALOG_JSON.as_bytes().to_vec()
        }
    }
}
```

Why two branches: the raw resource is the hand-formatted, pinned artifact (`cli_project_registration.rs:646-676` pins its parsed shape, `ensure_seeded_for_project_steady_state_precheck_skips_gate_and_writes` pins byte stability); re-serializing only when a row is actually `false` keeps today's seeded bytes unchanged by construction, so the follow-up flip is the first and only thing that changes them.

**4.4.2 Legacy manifest copy: VERBATIM, unchanged (decided).** The legacy `<config_dir>/coding-agents/agents.json` is user data that may not even parse (`:436-446` copies a corrupt file on purpose); one file cannot lose one entry without rewriting user bytes, and the read gate hides the key from every consumer anyway. `ensure_seeded` lines 411-423 stay as they are.

**4.4.3 Masters: key-to-basename mapping = a `key` field on the master (decided).** Add `key: &'static str` to `EmbeddedSeedMaster` (`:541-550`) and set `"claude"`, `"codex"`, `"opencode"` in `EMBEDDED_SEED_MASTERS` (`:555-580`). The existing test `every_embedded_master_maps_to_a_catalog_def_with_matching_configseed` gains `assert_eq!(def.key, m.key)`, so the field cannot drift from the def its basename resolves to; the table-vs-JSON pin (R1) then guarantees every master key has a row. No per-call JSON parse, no second lookup rule.

```rust
/// #1912 - the masters whose key is enabled in the table in force.
fn supported_embedded_masters() -> impl Iterator<Item = &'static EmbeddedSeedMaster> {
    let table = active_builtin_agent_support();
    EMBEDDED_SEED_MASTERS
        .iter()
        .filter(move |m| is_supported_builtin(m.key, table))
}
```

Consumers, all switched from `EMBEDDED_SEED_MASTERS.iter()` to `supported_embedded_masters()`:

- `embedded_master_for_command_basename` (600): a de-supported master resolves to `None`, so `reseed_master_for_command` (903-906) returns its existing error "'<command>' is not a recognized built-in with a shipped default config folder". Server-side re-check, no new error text.
- `reseedable_command_basenames` (610): the button is not offered. `web/commands.rs:1952-1959` compares against this function, so it self-adjusts.
- `ensure_seeded_masters` loop (671): a de-supported master is skipped ENTIRELY, embedded staging AND legacy `_seed/<dest>` tree copy alike (decided: the master tree is per-agent, so skipping it is a clean per-agent decision, unlike the one-file legacy manifest). An already-present master dir is never touched (the `Ok(_) => continue` arm is reached only for supported masters and the loop never deletes).
- Steady-state pre-check (798-804): extracted into a predicate so it can be unit-tested, and switched to the supported set:

```rust
/// #1912 - existence-only steady-state check: the manifest and every SUPPORTED
/// master dir exist. A de-supported master is never required, or every boot
/// would take the project gate for nothing.
fn all_seeds_present(ac_dir: &Path) -> bool {
    std::fs::symlink_metadata(manifest_path(ac_dir)).is_ok()
        && supported_embedded_masters()
            .all(|m| std::fs::symlink_metadata(master_dir_for_dest(ac_dir, m.dest)).is_ok())
}
```

and `ensure_seeded_for_project_with_token` reads `if all_seeds_present(&ac_dir) { return; }`.

### 4.5 `embedded_default_catalog` becomes `pub(crate)` (line 146)

Its doc comment gains: "RAW and UNGATED by design: the seed and the tests read it; every consumer-facing read goes through `validate_and_filter`." No caller outside the crate exists.

### 4.6 Frontend mirror

Rule, written as a comment above `FALLBACK_CODING_AGENTS` (`src/shared/agent-presets.ts:3`): "Mirror of the ENABLED rows of `BUILTIN_AGENT_SUPPORT` (`src-tauri/src/config/coding_agents_catalog.rs`), same order and fields as `agents.default.json`. Served only when the IPC catalog call rejects, so it must never resurrect a de-supported built-in." Append the `muse` entry after `antigravity`, byte-faithful to the JSON: key `muse`, label `Muse Code`, description `Meta terminal coding agent (beta; macOS/Linux host only)`, color `#0668E1`, command `muse`, NO `instructionsFilename`, `envs: []`, `isolatedHome: false`, `removable: true`, `updateCommands: []`, `autoUpdate: false`. No other TypeScript changes: `types.ts`, `ipc.ts`, the store and the three components render whatever the IPC returns.

### 4.7 Doc comments (one sentence each, no code)

Module header (`:1-28`): the switch, the read gate, the seed gate, the never-rewrite invariant. `load_catalog` (`:260-270`), `validated_embedded_default` (`:221-222`), `backfill_update_commands_from_embedded_default` (`:230-236`): "a `false` row in `BUILTIN_AGENT_SUPPORT` is dropped here" / "a de-supported built-in no longer donates its sequence". `ensure_seeded` (`:370-382`): embedded bytes are the enabled rows; the legacy copy stays verbatim. `ensure_seeded_masters` (`:657-669`): de-supported masters are skipped from every source. `commands/config.rs:496-509` (`get_coding_agent_catalog`): one sentence pointing at the table.

## 5. Affected files and symbols

| File | Change |
|---|---|
| `src-tauri/src/config/coding_agents_catalog.rs` | add `BUILTIN_AGENT_SUPPORT`, `is_supported_builtin`, `active_builtin_agent_support`, `BUILTIN_AGENT_SUPPORT_OVERRIDE` + `with_builtin_agent_support_for_test` (cfg(test)), `embedded_seed_bytes`, `supported_embedded_masters`, `all_seeds_present`; gate in `validate_and_filter`; `EmbeddedSeedMaster.key`; `ensure_seeded` three arms; `embedded_master_for_command_basename`, `reseedable_command_basenames`, `ensure_seeded_masters`, `ensure_seeded_for_project_with_token` use the supported set; `embedded_default_catalog` -> `pub(crate)`; doc comments; 14 new tests + 1 extended (section 8) |
| `src-tauri/src/commands/config.rs` | doc comment on `get_coding_agent_catalog` only |
| `src/shared/agent-presets.ts` | mirror rule comment + `muse` entry |
| `src/shared/agent-presets.test.ts` | header comment, `EXPECTED_BUILTINS` gains the muse row (no `instructionsFilename`), key list gains `"muse"`, test title "8 built-ins" |
| `plans/1912-coding-agent-support-switch.md` | this file |

Unchanged on purpose: `agents.default.json`, `schemaVersion`, `CodingAgentDefinition` (no new field, no wire change, no invoke-allowlist change), `agent_update.rs`, `cli/coding_agent.rs`, `web/commands.rs`, `agent_version.rs`, `src/shared/types.ts`, `src/shared/ipc.ts`, `src/sidebar/stores/coding-agents.ts`, the three pick-list components. Module graph: everything new lives inside `coding_agents_catalog.rs`, zero new cross-module arcs.

## 6. Required behavior and edge cases

Read side (all reached through `validate_and_filter`):

| Path | With a `false` row for key K |
|---|---|
| Embedded self-heal: manifest missing, unreadable, corrupt (`:280`, `:287`, `:302`) | K absent; corrupt file preserved byte-for-byte (G3 unchanged) |
| Parsed project manifest (`:291`) | K absent even if the file carries it; a second K entry is a duplicate and also dropped; a user entry with another key and K's command is kept |
| Legacy config-dir manifest (`:365`) | same as parsed (same function) |
| Backfill (`:240`) | K's `updateCommands` are no longer donated to a same-command entry with an empty sequence |
| `load_catalog_for_settings` (`:361`) and therefore IPC, WS, CLI, Settings overview, startup pass | K absent |
| Valid empty user catalog `[]` | still `[]` (honored verbatim, unchanged) |
| Key not in the table | always kept |
| Invalid entry with key K | skipped by validation before the gate; no difference observable |
| Failure behavior | none new: the read path still never errors; one `info` log line per dropped entry per read |

Seed side:

| Path | With a `false` row for key K |
|---|---|
| `ensure_seeded`, embedded source (absent legacy, non-file legacy, unreadable legacy) | writes the enabled rows only; file parses; K absent from the bytes |
| `ensure_seeded`, legacy regular file | verbatim copy, K included if the legacy file has it; reads hide it |
| `ensure_seeded`, manifest already present (any form) | returns `None`, byte untouched: NEVER rewritten, NEVER trimmed |
| `ensure_seeded_masters`, K's master absent | not created, from embedded nor from legacy `_seed/<dest>` |
| `ensure_seeded_masters`, K's master present | untouched (never deleted) |
| `reseedable_command_basenames` | K's basename absent |
| `reseed_master_for_command(K's command)` | `Err("... is not a recognized built-in with a shipped default config folder")` |
| `all_seeds_present` | ignores K's master; other masters still self-heal |
| Every row `true` (the shipped state) | every seeded byte identical to today; every existing test green untouched |

## 7. Settings surfaces in the user's screenshot: covered by the read gate, no extra code

- "+ <agent>" preset buttons and the Settings catalog list: `codingAgentsStore.catalog()` <- IPC `get_coding_agent_catalog` <- `coding_agent_catalog_inner` (`commands/config.rs:521`) <- `load_catalog_for_settings`. Gated. Only the IPC-reject fallback bypasses the backend, and section 4.6 keeps it mirrored.
- Auto-update table, including "(not registered)" and "Will ask at startup" rows: `update_overview_with` loads the catalog at `agent_update.rs:1665` and `build_update_overview_rows` (`:1516-1535`) emits one row per catalog entry with a non-empty sequence; the two labels are frontend decorations of an existing row (`AgentAutoUpdateStatusList.tsx:128`, `agent-update-status.ts:70`). A `false` row has no row to decorate. Gated.
- Startup prompt / update run: `PassSupervisor::run_body` (`:3225`) -> `build_update_plan` rule 0. A registered but de-supported command is neither prompted nor updated (the Reading A consequence the user accepted). Gated.
- Re-seed button: `list_reseedable_agent_commands` (`commands/config.rs:531`) -> `reseedable_command_basenames`, section 4.4.3. Gated by the seed side.

## 8. Tests

All new Rust tests go in `coding_agents_catalog.rs`'s `mod tests`, plain `#[test]`, using `with_builtin_agent_support_for_test`. Test tables are module-level `#[cfg(test)]` consts, all 8 rows spelled out, exactly one `false`: `TABLE_MUSE_OFF` (`("muse", false)`) and `TABLE_CLAUDE_OFF` (`("claude", false)`). Objective acceptance = every assertion listed passes; the "control" assertions are the positive controls the reviewer verifies.

| # | Test name | Asserts |
|---|---|---|
| R1 | `builtin_agent_support_table_matches_embedded_default_keys_in_order` | `BUILTIN_AGENT_SUPPORT.iter().map(k)` collected == `embedded_default_catalog().agents.iter().map(key)` collected, as `Vec<&str>` (exact order, hence set equality both ways and no duplicates) |
| R2 | `builtin_agent_support_ships_every_row_enabled` | `BUILTIN_AGENT_SUPPORT.iter().all(on)`; flip-time inventory item, edited by the follow-up |
| R3 | `support_override_scopes_to_closure_and_restores_shipped_table` | on an absent manifest: `load_catalog` len 8 with a muse key BEFORE; inside `with_builtin_agent_support_for_test(TABLE_MUSE_OFF, ..)` len 7 and no muse key; AFTER the closure len 8 with muse again |
| R4 | `desupported_row_dropped_from_embedded_self_heal_paths` | under `TABLE_MUSE_OFF`: missing manifest -> 7, no muse, `[0].key == "claude"`; corrupt manifest -> 7, no muse, corrupt bytes preserved |
| R5 | `desupported_row_dropped_from_parsed_manifest_and_dedup_cannot_resurrect_it` | manifest `[muse "First", muse "Second", {key:"mine", command:"muse"}]` under `TABLE_MUSE_OFF` -> exactly `["mine"]`, `[0].command == "muse"` (control: same file with no override -> `["muse","mine"]`, label "First") |
| R6 | `desupported_row_dropped_from_load_catalog_for_settings_primary` | settings with a primary project, file absent, under `TABLE_MUSE_OFF` -> 7, no muse; write a user file `[muse, custom]` -> exactly `["custom"]` |
| R7 | `desupported_builtin_no_longer_donates_update_commands_in_backfill` | manifest `[{key:"my-claude", command:"claude", updateCommands:[]}]`: control without override -> `["claude --update"]`; under `TABLE_CLAUDE_OFF` -> `[]` |
| R8 | `desupported_row_absent_from_seeded_manifest_bytes` | control: `ensure_seeded(dir, None)` without override writes bytes == `EMBEDDED_DEFAULT_CATALOG_JSON`; fresh dir under `TABLE_MUSE_OFF`: `ensure_seeded` returns `Some`, file parses as `CodingAgentCatalog` with `schema_version 1` and 7 agents, the raw bytes do NOT contain `"muse"`, `load_catalog` -> 7 |
| R9 | `legacy_catalog_copied_verbatim_even_when_it_carries_a_desupported_key` | legacy file `[muse, custom]` (hand-authored bytes) under `TABLE_MUSE_OFF`: project file bytes == legacy bytes (contains `"muse"`); `load_catalog` -> exactly `["custom"]` |
| R10 | `already_seeded_manifest_never_trimmed_by_a_false_row` | seed without override (8-row bytes); under `TABLE_MUSE_OFF`: `ensure_seeded` -> `None`, bytes identical, `load_catalog` -> 7 without muse |
| R11 | `desupported_master_not_seeded_not_reseedable_and_reseed_refused` | control without override: `reseedable_command_basenames()` sorted == `[claude, codex, opencode]`; under `TABLE_CLAUDE_OFF`: `ensure_seeded_masters(dir, None)` leaves `.claude` absent and `.codex`, `.opencode` non-empty; `reseedable_command_basenames()` sorted == `[codex, opencode]`; `reseed_master_for_command(dir, "claude")` is `Err` containing "not a recognized built-in"; `reseed_master_for_command(dir, "codex")` is `Ok` |
| R12 | `desupported_master_skips_legacy_tree_copy_too` | legacy `_seed/.claude/settings.json` = `LEGACY CLAUDE`, `_seed/.codex/config.toml` = `LEGACY CODEX`; under `TABLE_CLAUDE_OFF`: project `.claude` absent, project `.codex/config.toml` == `LEGACY CODEX`, legacy tree untouched |
| R13 | `all_seeds_present_ignores_desupported_master` | seed manifest + masters under `TABLE_CLAUDE_OFF`: `all_seeds_present` true inside the override, false outside it (`.claude` missing); delete `.codex` -> false inside the override too; `ensure_seeded_for_project` on that root inside the override recreates `.codex` and still not `.claude` |
| R14 | `already_seeded_master_never_removed_by_a_false_row` | seed masters without override; under `TABLE_CLAUDE_OFF`: `ensure_seeded_masters` again -> `.claude/settings.json` bytes unchanged; `reseedable_command_basenames()` excludes claude |
| ext | `every_embedded_master_maps_to_a_catalog_def_with_matching_configseed` | add `assert_eq!(def.key, m.key)` |

Frontend: `src/shared/agent-presets.test.ts` "matches the backend embedded default: 8 built-ins, exact order and fields": key list `[claude, codex, hermes, cursor, pi, opencode, antigravity, muse]`; `EXPECTED_BUILTINS` gains `{ key: "muse", label: "Muse Code", description: "Meta terminal coding agent (beta; macOS/Linux host only)", color: "#0668E1", command: "muse" }` (no `instructionsFilename`, matched with `toMatchObject`); the update-commands test already puts muse in its `else` branch expecting `[]`. Other files that iterate the fallback (`SettingsModal.catalog.test.tsx:116,146`, `SettingsModal.automation.test.ts:19-21,86`, `coding-agents.test.ts:44,68,101`) select by key or by identity and stay green; `agent-update-status.test.ts:57-65` uses its own fixture.

Stay green untouched, by design: `embedded_default_parses_with_eight_agents_in_order`, `embedded_default_matches_current_presets_exactly`, `every_embedded_entry_validates`, `embedded_default_ships_update_commands_for_all_but_cursor_and_muse` (raw JSON, unchanged); every `== 8` pin listed in section 11 (every row `true`); `web/commands.rs` `get_coding_agent_catalog_route_returns_backfilled_catalog`; `tests/cli_project_registration.rs` `new_project_seeds_catalog_into_ac` and `cli_catalog_serves_legacy_fallback_then_embedded_without_projects`.

## 9. Verification commands

From `src-tauri/` (the workspace root; never from the replica root):

```
cargo test --lib config::coding_agents_catalog
```
Expected: 52 passed (38 baseline + 14 new), 0 failed.

```
cargo test --lib get_coding_agent_catalog_route_returns_backfilled_catalog
cargo test --test cli_project_registration -- new_project_seeds_catalog_into_ac cli_catalog_serves_legacy_fallback_then_embedded_without_projects
```
Expected: 1 passed, then 2 passed (the second builds the `agentscommander-new` binary).

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no diff, no warnings (clippy compiles the cfg(test) override too).

From the repo root:

```
npm run test -- src/shared/agent-presets.test.ts src/sidebar/stores/coding-agents.test.ts src/sidebar/components/SettingsModal.catalog.test.tsx
npm run typecheck
```
Expected: 19 passed (unchanged count, the drift test now pins 8), typecheck clean.

## 10. Rejected alternatives (decided, not reopened)

- Five `_with(table)` twins: tests copies, leaves the wrapper-to-const link unproven, five signatures (section 4.3).
- Always re-serializing the seeded manifest: changes today's seeded bytes and the pinned e2e shape for no gain (4.4.1).
- Filtering the legacy manifest copy: rewrites user bytes, impossible for a corrupt file (4.4.2).
- Parsing the embedded JSON per masters call to map key to basename: a runtime lookup where a pinned static field is exact and free (4.4.3).
- Copying a legacy master tree for a de-supported key: AC would still create a `_seed/<dest>` for an agent it no longer supports (4.4.3).
- Cargo feature, JSON `enabled` field, settings toggle, frontend-only filter, per-consumer gating, denylist const: the architect's list, unchanged.

## 11. Flip-time inventory (for the one-line follow-up that sets a row to `false`)

Nothing here moves at landing. When key K goes `false`, these move, all measured at `4ba6c99e`:

- `coding_agents_catalog.rs` gated `== 8` / last-is-muse pins: `1233-1234`, `1247-1252`, `1312`, `1541`, `1547`, `1552`, `1685`, `1699`, `1759`, `1763`; test R2 (`builtin_agent_support_ships_every_row_enabled`); if K is claude, codex or opencode: `reseedable_commands_are_claude_codex_opencode` (1355), `ensure_seeded_masters_creates_nonempty_masters_and_preserves_edits` (1390), the reseed tests (1414-1479), `ensure_seeded_for_project_steady_state_precheck_skips_gate_and_writes` (1717).
- `web/commands.rs:1935-1944` (`rows.len() == 8`, last `muse`, six with update commands).
- `tests/cli_project_registration.rs:646-676` (seeded file: 8 agents, muse last), `:697-709` (`_seed` names), `:766-785` (CLI catalog: 8, muse object).
- `src/shared/agent-presets.ts` (drop K's entry) and `agent-presets.test.ts`; frontend tests that name K through the fallback: `SettingsModal.catalog.test.tsx`, `SettingsModal.automation.test.ts` if K is one of its six.
- Not touched by the table, product decision if wanted: `agent_version.rs:64` probe stems; `docs/` and `CHANGELOG.md` mentions.
