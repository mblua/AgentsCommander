# Epic Plan #1905: externalize blocking-menu patterns into `settings-blocking-menus.json` with a `.local.json` overlay

Author: ac-architect-v4, room-5, 2026-09-09 UTC. Full `code-implementation-workflow` path, Round 3 candidate.
Status: READY_FOR_IMPLEMENTATION

Issue: [mblua/AgentsCommander#1905](https://github.com/mblua/AgentsCommander/issues/1905)
Delivery path: Full (score 42, raised by the schema/persistence/migration criterion).
PARTITION: 4 phases (cut by owner, Rust then docs; the Rust work is cut twice more: the store is
the contract, the guard is its consumer, the migration is the persistence change, so each phase
file stays under the 400-line cap and each phase leaves `main` behaviorally green on its own).

Every line number in this epic and in the phase files is pinned to the base SHA below and describes
the tree BEFORE this plan's edits. Anchors are symbol names; line numbers are navigation aids only.

## 1. Objective

Today every agent in `settings.json` carries its own `blockingMenus` array. AC fills it once from
the command stem and then never touches it again, so a new shipped pattern needs a hand-written Rust
back-fill (`apply_issue_1757_migration`). Shipped patterns and user patterns sit in the same array,
and AC cannot refresh its own defaults without risking the user's entries.

After this plan:

1. Shipped patterns live in `settings-blocking-menus.json`, next to `settings.json`. AC owns that
   file and rewrites it from the content embedded in the binary whenever the two differ, so every
   release ships its patterns to every install with no per-pattern migration.
2. User patterns live in `settings-blocking-menus.local.json`, next to it. AC reads it at startup
   and never rewrites it, except for one append-only merge during the one-shot migration.
3. On the first load after upgrade, every `blockingMenus` array still in `settings.json` is
   carried over: arrays equal to the shipped set for their stem are dropped (the shipped file now
   serves them), every other array is copied into the `.local` file keyed by agent id, and the key
   disappears from `settings.json`. Nothing the user has today is lost.
4. Any array the migration cannot move (overlay-owned, or a migration that aborted) keeps applying
   exactly as it does today, because the guard reads a still-present array first (D3 layer 0).
5. The runtime, the `list-peers` fields, `enabled: false`, `[]`, `menuGuardEnabled` and the
   keep-invalid-verbatim rule all keep their meaning.

## 2. Pinned base and delivery authority

- Repository: `D:\0_repos\AgentsCommander_iac\.ac\room-5-ac-dev-team-v4\repo-AgentsCommander`
- Target branch: `feature/1905-externalize-blocking-menus`, created from `main` at
  `80aeb85b12a172f55f7ca82544500a43ecace6b8` (pin the full SHA in the PR body).
- Clean-tree precondition: `git status --porcelain` is empty before each phase starts.
- Version at base: `src-tauri/Cargo.toml:3` is `0.31.0`. No version bump belongs to this plan.

## 3. Task class and accepted threat model

Routine application-code change with a persisted-shape migration that moves user data between two
files in the same directory. No release, signing, packaging, untrusted host or security-boundary
change. Baseline gates apply; enhanced provenance controls (anchored executable hashes, DLL closure
inventories, poisoned-PATH tests) are not applicable and are not required.

The "destructive or irreversible migration" trigger applies, and the enhanced control chosen is
the smallest one that answers it, not a harness. Hazard: the migration strips `blockingMenus` from
`settings.json`, and a strip whose counterpart write did not land loses user patterns. Control,
each part pinned by a test that drives the real loader over a tempdir (phase 3):

1. Schema parity before mutation: the complete `.local` file is validated with the same parser
   the runtime uses (`parse_blocking_menus_file`) before anything is merged, and the merged bytes
   are validated with it again before they are written (D7 steps 4 and 7).
2. Write before strip: the `.local` file is published atomically before any array is set to
   `None`; every failure before that point returns with nothing stripped (D7).
3. No clobber and no loss: an existing `byAgent[id]` wins; two agents sharing an id with
   different arrays or different commands abort the export; explicit `[]` on a stem with
   shipped patterns is exported (D7 steps 3 and 5).
4. State-keyed retry: the export runs while any `Some` array is on disk, so a save that fails
   after the `.local` write is completed at the next start, and a restart between the two writes
   finishes the job (D7, tests T11 and T12).
5. Compatibility for what was not moved: the guard evaluates a still-present array first (D3
   layer 0, D12), so an aborted export or an overlay-owned array changes no behavior.
6. Concurrency: stated in D13. The loader that runs D7 also runs inside CLI processes (E13),
   so no process-local lock could serialize it; no new lock is added because every
   interleaving converges through the per-pid atomic writers and the settings file lock.

Reversal is by hand and needs an older binary; phase 4 documents it.

## 4. Evidence (verified at the base SHA)

E1. `src-tauri/src/config/settings.rs:85-93` `AgentConfig.blocking_menus: Option<Vec<BlockingMenuEntry>>`,
`#[serde(default, skip_serializing_if = "Option::is_none")]`, wire key `blockingMenus`. `:102-108`
`BlockingMenuEntry { Valid(BlockingMenuConfig), Invalid(Value) }`, untagged, derives `PartialEq, Eq`.
`:120-127` `BlockingMenuConfig { pattern, notification, enabled (default true), captured_against }`.
`AgentConfig` has eleven fields (`id, label, command, color, envs, isolated_home,
instructions_filename, config_seed, context_regex, blocking_menus, backend`; constructor shape at
`:9232-9244`).

E2. `settings.rs:1034` `CODEX_HOOKS_REVIEW_PATTERN`; `:1036` `codex_hooks_review_menu()` (private);
`:1046-1070` `default_blocking_menus_for_command` (stems `pi` and `codex`, everything else `[]`);
`:1073-1083` `materialize_blocking_menus` fills every `None` with that default, so every agent on
every install carries a materialized array today, `[]` included for stems without patterns;
`:1105-1137` `apply_issue_1757_migration` (content-keyed, skips empty arrays, returns early when the
overlay owns `agents`). `docs/features/menu-guard.md:39` documents that a materialized `[]` "is
indistinguishable on disk from 'I turned this off deliberately'" and calls that intentional.

E3. Load-time call sites. `load_settings()` (`:2084`) resolves the config path and calls
`load_settings_from_path` (`:2100`, `pub(crate)`); the two are one loader, and E13 lists every
process and thread that calls it. In that loader: `materialize` at `:2185`,
`1757` at `:2189`, `repair_coding_agent_profiles_config` at `:2193`, then `needs_save` routes through
`save_settings_to_path_preserving_project_paths_typed` (`:5075`, PRESERVE mode) and adopts the fresh
decode (`settings = written`, `:2227`); a failed save is logged by `report_settings_save_error`
(`:3682-3689`, log only, no panic) and the in-memory settings are returned as they are. CLI loaders:
`load_settings_for_cli` (`:2272`) calls `materialize` at `:2341` and `1757` at `:2342`;
`load_settings_for_cli_strict` (`:2359`) at `:2413-2414`. The CLI loaders never write.

E4. Overlay. `LocalSettingsOverlay::load_and_merge` (`src-tauri/src/config/local_overlay.rs:287`)
derives the overlay path as `settings_path.with_file_name("settings.local.json")` (`:294`).
`OVERLAY_KEY_AGENTS` (`settings.rs:2584`) and the #1737 D7c rule: an overlay that owns `agents`
replaces the whole array and `restore_base` writes the base array back on every save.

E5. Runtime read. `src-tauri/src/pty/menu_guard/mod.rs:265-270` finds the agent by
`session.agent_id` in `settings.agents` and clones `agent.blocking_menus`; it is the only
`blocking_menus` read outside `settings.rs`. `MenuGuard` (`:46-50`) holds `sessions`,
`compiled_patterns`, `next_episode_id`; `MenuGuard::new()` (`:63`) is `Default` (`:52-60`).
`evaluate_logical_rows` (`:88`) takes `entries: &[BlockingMenuEntry]` and is unchanged by this plan.
`lib.rs:2969` constructs it: `Arc::new(crate::pty::menu_guard::MenuGuard::new())`, manages it at
`:2981`. `MenuGuard::new()` is also used by three test harnesses (`commands/session.rs:6264`,
`phone/mailbox.rs:26939`, `pty/inject.rs:1008`), which must keep compiling.

E6. Test callers of `default_blocking_menus_for_command`: `menu_guard/mod.rs:389,399,400,436,536` and
`commands/session.rs:11119`. Every other `blocking_menus` mention outside `settings.rs` is a struct
constructor writing `None` (`agent_update.rs:5622`, `cli/coding_agent.rs:529,550`,
`cli/create_agent.rs:283`, `cli/self_switch.rs:504`, `commands/config.rs:2958`,
`commands/session.rs:5284,5297,5362,5418,9348,9638,9718`, `config/agent_command.rs:1176`,
`config/coding_agent_mutations.rs:585`, `config/coding_agent_profiles.rs:806`, `lib.rs:4601`,
`phone/mailbox.rs:13492`, `web/commands.rs:1373`, `tests/pty_powershell_managed_native.rs:383`) or a
hand-built `BlockingMenuEntry` passed to the evaluator (`phone/mailbox.rs:26940`,
`pty/inject.rs:1009`). None of them changes.

E7. Frontend: `src/shared/ipc.ts:579` `resolveBlockingMenu` and `src/sidebar/App.tsx:25,390` consume
the blocked event only. No TypeScript type carries `blockingMenus`, so a Settings save never
re-materializes the key.

E8. Shipped-JSON precedent: `src-tauri/src/config/coding_agents_catalog.rs:58`
`include_str!("../../resources/coding-agents/agents.default.json")`. `.gitattributes:4` pins
`*.json text eol=lf`, so the embedded bytes are LF on every checkout. The on-disk copy is written
from a re-serialization, never from the embedded bytes, so the "is it stale" comparison is against
bytes AC itself produced.

E9. Atomic writer: `src-tauri/src/config/local_config_io.rs:80` `pub fn write_file_atomic(path, bytes)`
writes `.<name>.<pid>.tmp` beside the target (`temp_config_path`, `:122-128`, `std::process::id()`)
under a process-local mutex (`:88`) and publishes by rename. `File::create` on the temp path
(`:100`) fails when a directory already occupies that name, which is how test T10 forces a write
failure on every OS. The module has zero outgoing arcs in the detector graph (section 7).

E10. Settings write lock: `SettingsFileLock::acquire(settings_path, timeout)` (`settings.rs:3805-3813`)
opens `settings.json.lock` with shared read/write access (`:3841-3859`) and polls an exclusive OS
lock (`flock` on Unix, `LockFileEx` on Windows, `:3977-4028`) every 10 ms until the timeout.
`save_settings_value` (`:4050-4062`) acquires it with a 2-second timeout, so a test that holds a
`SettingsFileLock` on the same path makes the loader's save fail after 2 seconds while the
`.local` write, which uses the E9 mutex and no file lock, still succeeds (test T11).

E11. Instance artifact registry: `src-tauri/src/config/instance_artifacts.rs:153`
`SETTINGS_LOCAL_OVERRIDE_FILE_NAME`; rows at `:415-444` (`sessions.json`, `settings.json`, lock,
local overlay, migration backups), byte-ordered by `name` and pinned by
`ignore_rows_are_unique_and_byte_sorted_by_name` (`:556`). `instance_gitignore.rs:1025-1035` is the
fixture list every `Ignore` row must reach; `:615` and `:1239` pin `rules.len() == 2 + ignore_rows().len()`.

E12. Test harnesses that exist and are reused: `settings.rs:9425-9450` `agent_1757(id, command,
blocking_menus)` and `settings_1757(agents)` build an `AgentConfig` and an `AppSettings` in memory
(private items of `mod tests`, visible to every child module through `super::`); `:9612`
`issue_1757_reaches_settings_json_through_the_real_load_chain` (tempdir + real loader + raw JSON
read-back); `:9694` `mod local_overlay_1737` with private helpers `base_fixture` (`:9703`),
`seed(dir, base, local)` (`:9712`), `disk_object` (`:9727`), `folder_trust_entry` (`:9738`),
`codex_agent_with` (`:9747`); `:10845` `an_overlay_owned_agents_array_suppresses_the_1757_migration`
with four numeric assertions (`:10887-10897`). `menu_guard/mod.rs:534` T11 replays a vt100 frame
against the shipped codex entries. `tempfile` is a dev-dependency (`Cargo.toml:77`).

E13. Every caller of the loader that runs the migration block (`load_settings()`, E3), at base.
GUI process, main thread: `lib.rs:2648` before the Tauri builder, then `:2989`, `:3028`, `:3117`,
`:3163` inside the synchronous `setup` body. GUI process, other threads after setup (async
commands and handlers): `commands/config.rs:1811`, `:1827`, `:1923`; `commands/window.rs:618`,
`:933`; `phone/mailbox.rs:6857`, `:9362`; `config/teams.rs:1689`; `pty/container_backend.rs:3360`;
`testability/ui_automation.rs:1131`. CLI processes (one process per verb, spawned by agents at any
time): `cli/mod.rs:388` inside `validate_cli_token`, which every token-bearing verb runs (`send`,
`list-peers-lean`, ...); `cli/send.rs:864`; `cli/list_peers.rs:865`, `:994`, `:1091`;
`cli/close_session.rs:178`. Read-only loaders that never save: `lib.rs:2496` calls
`load_settings_for_cli()` before `:2648`, and the CLI verbs also use `load_settings_for_cli` and
`load_settings_for_cli_strict` elsewhere. So the GUI's first load has no in-process peer, but a
CLI process can run the same loader at the same time; D13 is written for that premise. The
settings save inside the loader (`save_settings_value`, `:4050`) takes `SettingsFileLock` (E10),
writes `settings.json.<pid>.<op_id>.tmp` (`:4832`) and publishes by a retried rename
(`replace_settings_file_atomic`, `:4897`, `:5027`).

E14. Docs: `docs/features/menu-guard.md` (sections at `:25`, `:76`, `:102-121` claude example with
its entry at `:113-118`, `:129`, `:137`, `:151`, `:160`, `:182`; ten `blockingMenus` mentions),
`docs/reference/settings.md:97` (`blockingMenus` row), `:460-481` (Menu guard section), `:530` (See
also; three mentions), `docs/reference/directory-layout.md:77-96` (instance-dir file table).

E15. CI legs (`.github/workflows/pr-regression-gates.yml`): `rust-regression` (Windows) runs
`cargo check --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test
--lib --bins --tests` (`:82-92`); `rust-regression-linux` runs check, clippy and one filtered test
(`:133-161`); `rust-regression-macos` runs check and clippy only (`:242-248`); `rust-fmt` (`:263-265`);
`frontend-regression` runs `npm run typecheck` (`:365`); `test-debt` (`:22`). The full Rust suite
runs on Windows only, and every test in this plan is compiled on all three legs by `--all-targets`.

E16. Default strings history: `git log -S` shows the pi and codex folder-trust literals unchanged since
`cb6e10c` (#1647) and the hooks-review literal unchanged since `81499f5` (#1757). No shipped default
ever changed text, so "equal to the current shipped set" is the only pristine shape on disk.

## 5. Binding decisions

D1. **Two files, one schema.** Both `settings-blocking-menus.json` and `settings-blocking-menus.local.json`
are a `BlockingMenusFile` object:

```json
{
  "schemaVersion": 1,
  "note": "free text, ignored by AC",
  "byCommand": { "<lowercase command stem>": [ BlockingMenuEntry, ... ] },
  "byAgent":   { "<agent id>":              [ BlockingMenuEntry, ... ] }
}
```

`schemaVersion` defaults to 1 and must be 1. `note` is an optional string AC never reads. Both maps
default to empty and are `BTreeMap` in Rust so the shipped file serializes deterministically. Array
elements are the existing `BlockingMenuEntry`, so an entry AC cannot read is kept verbatim and
skipped, exactly as today. Unknown top-level keys are tolerated on read and preserved on the one
merge write (D7). A file that is not an object, or whose `note`, `byCommand` or `byAgent` has the
wrong type, or whose `schemaVersion` is not 1, is rejected whole (D6).

D2. **Keying.** The shipped file uses `byCommand` only; a test pins that its `byAgent` is empty and
the resolver ignores a shipped `byAgent`. The `.local` file may use either map. Keys of `byCommand`
are the lowercase executable stem that `command_executable_basename` produces
(`coding_agents_catalog.rs:626`); the lookup is an exact string match and the docs say so.

D3. **Precedence, replace-whole at every level.** For an agent `a` with id `id` and command stem
`stem`, the effective array is the first present of:

0. `a.blocking_menus == Some(array)`: an array still in `settings.json` or supplied by an
   overlay-owned `agents` array (the legacy layer, D12)
1. `local.byAgent[id]`
2. `local.byCommand[stem]`
3. `shipped.byCommand[stem]`
4. `[]`

A present array replaces the layers below it entirely, so `"byAgent": {"codex": []}` in the `.local`
file switches the guard off for that agent, and `enabled: false` inside a `.local` copy of a shipped
array disables one pattern durably. Layer 0 is what the guard evaluates today, so an install whose
arrays were never moved behaves exactly as before this plan.

D4. **Shipped content lives in the source tree** at
`src-tauri/resources/blocking-menus/settings-blocking-menus.json`, embedded with `include_str!` from
`settings.rs`. It carries exactly the three entries `default_blocking_menus_for_command` returns at
base (E2), under `byCommand.codex` and `byCommand.pi`, plus a `note`. `default_blocking_menus_for_command`
stays, with its name and signature, and becomes a read of the embedded content, so every test caller
in E6 keeps compiling and keeps meaning "the shipped set for this command".

D5. **Refresh rule.** When the store is built at GUI startup, AC parses the embedded content,
serializes it with `serde_json::to_vec_pretty` plus one trailing `\n`, and compares those bytes with
the bytes on disk at `settings-blocking-menus.json`. If the file is absent or the bytes differ, AC
writes the canonical bytes through `write_file_atomic` (E9). If they are equal, nothing is written.
There is no version stamp: the content is its own identity. A hand edit inside the shipped file is
undone at the next start, and the file's `note` and both doc pages say so. A write failure is logged
once at error level and the embedded content serves from memory; the in-memory truth is always the
embedded content, never the disk copy. The refresh lives in `BlockingMenusStore::load_from_settings_path`,
not in the settings loader, so a test that wants the file on disk builds the store explicitly.

D6. **One parser for both files.** `parse_blocking_menus_file(&str) -> Result<BlockingMenusFile, String>`
is the only way any `.local` text becomes a typed file: JSON parse into a `serde_json::Value`,
reject anything that is not `Value::Object` (serde-derived structs also accept a JSON array,
because every field has a default, so `[1]` and `[]` would otherwise decode as an empty file),
decode the object into the typed struct, then the `schemaVersion == 1` check. The loader uses it
(absent is normal and yields an empty layer; any
`Err`, or an unreadable file, logs once at error level and yields an empty layer for the session).
The migration uses the same function on the raw text before merging and on the merged bytes before
writing, so what the export writes is by construction what the runtime accepts (blocker A). AC never
rewrites the `.local` file except in D7.

D7. **One-shot migration, state-keyed and no-clobber.** `export_blocking_menus_to_local_file`
runs in `load_settings_from_path`, the loader behind `load_settings()`, which the GUI and the CLI
verbs both call (E13), in the `[settings-migration]` block where `materialize` and `1757` run today
(E3), and only when at least one agent has `blocking_menus == Some(_)`. The two `_for_cli` loaders
never run it (D8). Steps, in order:

1. If the overlay owns `agents` (E4), return `false` without touching anything. Those arrays keep
   applying through layer 0 (D12); one info line per such agent names the id and the `.local` file.
2. Apply the existing #1757 back-fill in memory (`apply_issue_1757_migration`, unchanged), so a
   pre-#1757 pristine codex array compares equal to the shipped set, and so the in-memory arrays are
   exactly what the current binary would have served if a later step aborts.
3. Collision check: over every agent with `Some`, group by id. Two agents sharing an id with
   different arrays, or with equal arrays but different commands (the pristine test of step 5
   depends on the command, so one `byAgent[id]` row cannot serve both), abort the export (one
   error line naming the id); nothing is stripped and layer 0 keeps serving both. Equal arrays
   under one id and one command are one candidate.
4. Read the `.local` file. Absent means an empty object. Present: run `parse_blocking_menus_file`
   on the complete text; any `Err` (not JSON, not an object, wrong `schemaVersion`, wrong type for
   `note`, `byCommand` or `byAgent`, or an unreadable file) aborts with nothing stripped.
5. Candidates: for each id from step 3, if `array == default_blocking_menus_for_command(command)`
   the array is pristine and produces no entry; otherwise `byAgent[id] = array` through the typed
   round-trip (`serde_json::to_value(&entries)`): a `Valid` entry lands in AC's canonical form
   (`enabled` written explicitly, unknown keys inside the entry dropped, exactly as every base
   save of `settings.json` already drops them), an `Invalid` entry lands verbatim. `[]` on `pi`
   or `codex` differs from the shipped set and is exported. `[]` on a
   stem whose shipped set is `[]` (claude, gemini, every other stem) is pristine and is dropped: it
   is the byte shape `materialize_blocking_menus` wrote on every such agent on every install (E2),
   the page documents it as indistinguishable from a deliberate off, and exporting it would freeze
   one `byAgent` row per agent against every future shipped pattern for that stem, which defeats
   objective 1. After this plan, "off for a stem that ships nothing" is written as
   `"byAgent": {"<id>": []}` in the `.local` file, which is durable.
6. Merge into the raw object: set `schemaVersion: 1` if absent; insert every candidate whose id is
   not already a key of `byAgent` (an existing key wins, one info line); unknown keys survive.
7. If at least one candidate was inserted: serialize with `serde_json::to_vec_pretty` plus `\n`,
   run `parse_blocking_menus_file` on those bytes (abort on `Err`), write them through
   `write_file_atomic`. A write failure aborts with nothing stripped.
8. Only now set `blocking_menus = None` on every agent and return `true`, which sets `needs_save`
   so the existing PRESERVE-mode save removes every `blockingMenus` key from `settings.json`.

Idempotency follows from the state key: after step 8 lands, no agent has `Some`, so the function is
a no-op forever. If the settings save fails after step 7, the next start recomputes the same
candidates, finds them present, inserts nothing, and strips again (T11, T12).

D8. **CLI loaders stop materializing.** `load_settings_for_cli` and `load_settings_for_cli_strict`
drop their `materialize_blocking_menus` and `apply_issue_1757_migration` calls and touch
`blocking_menus` nowhere. They round-trip whatever is on disk unchanged. `materialize_blocking_menus`
is deleted; `apply_issue_1757_migration` survives as a private step of D7.

D9. **`AgentConfig.blocking_menus` stays as a legacy field.** It is read by D7 and by D3 layer 0,
is `None` on every migrated install, and no production writer ever sets `Some`. The serde attributes
are unchanged, so the key is absent from disk after the migration and every constructor in E6 keeps
compiling. Removing the field would touch 14 files for no behavior and would strip the key from an
overlay-owned array on the next save (E4), so it stays.

D10. **The store lives inside `MenuGuard`.** `BlockingMenusStore { shipped: &'static BlockingMenusFile,
local: BlockingMenusFile }` is defined in `settings.rs`. `MenuGuard` gains a `store` field;
`MenuGuard::new()` keeps its signature and uses `BlockingMenusStore::shipped_only()`, so every
existing test and harness still evaluates the shipped set; `MenuGuard::with_store(store)` is what
`lib.rs:2969` calls, after `load_settings()` has run the migration at `lib.rs:2648`. The scan loop
resolves `store.resolve_for(agent)` (D3, layer 0 included) in place of `agent.blocking_menus.clone()`.
No file watcher is added; both files are read once per process, exactly like `settings.json`.

D11. **Registry rows.** Two `Ignore` rows join `INSTANCE_ARTIFACTS`, with constants
`BLOCKING_MENUS_SHIPPED_FILE_NAME = "settings-blocking-menus.json"` and
`BLOCKING_MENUS_LOCAL_FILE_NAME = "settings-blocking-menus.local.json"`, placed immediately before
the `settings.json` row because `-` (0x2D) sorts before `.` (0x2E). The fixture list in
`instance_gitignore.rs` gains both names.

D12. **Compatibility for arrays that were not moved (blocker B).** Layer 0 of D3 is the whole
mechanism: an array still present in memory, whether from `settings.json` after an aborted export
or from an overlay-owned `agents` array, is what the guard evaluates, with no consultation of either
file. `settings.local.json` is never modified. An overlay `pi: []` therefore stays off, a custom
claude pattern in an overlay keeps detecting, and a broken `.local` file leaves every legacy array
in force. Phase 3 pins the overlay case and both abort cases with real-loader tests that assert the
resolved entries, not a log line.

D13. **Concurrency of the migration (blocker D).** The transaction (read `settings.json`, merge and
publish `.local`, strip, save) is not held under one lock, and no lock is added. Premise: the
loader that runs D7 also runs inside every CLI process (E13), so on the upgrade start an agent's
`send` in a second process can run D7 at the same time as the GUI's first load, and no
process-local lock can serialize that. Within one process the E9 mutex serializes `.local`
writers; across processes the mechanism is different and is what makes every interleaving
converge:

1. Both processes compute the same candidates from the same `settings.json` bytes, so the
   `.local` content they would write is identical.
2. Each `.local` write goes through `write_file_atomic`: a per-pid temp file
   (`.<name>.<pid>.tmp`, E9) and publication by rename, so two writers never share a temp file
   and the published file is always one writer's complete bytes. Whichever lands second writes
   the same bytes.
3. A process that reads `.local` after the other's publish finds every id present, inserts
   nothing, and goes on to strip (D7 step 6 and 8).
4. The strip is the loader's settings save under `SettingsFileLock` (E10, E13): a save that
   cannot take the lock within 2 seconds fails, is logged, and leaves the arrays on disk; on
   Windows a rename over a `settings.json` that another process holds open fails after the
   bounded #537-style retry (`replace_settings_file_atomic`) with the same result. In both cases
   the `.local` write has already landed, so the next load inserts nothing and strips again.
5. A CLI save that read the keys before another process's strip and wrote after it puts the keys
   back; the next load re-exports (existing `byAgent` entries win, nothing is lost) and strips
   again.

Every interleaving converges to the stripped `settings.json` with the `.local` content of the
first successful export. Test T11 pins step 4 and test T12 pins the intermediate state.

## 6. Scope, files, and compatibility

Phase 1 (Rust store, 4 files plus the arc record): `src-tauri/resources/blocking-menus/settings-blocking-menus.json`
(new), `src-tauri/src/config/settings.rs`, `src-tauri/src/config/instance_artifacts.rs`,
`src-tauri/src/config/instance_gitignore.rs`, plus the regenerated `src-tauri/module-arcs.txt`
(section 7). Nothing in production calls the store yet and `materialize` still runs, so guard
evaluation is base; the only visible change is that every instance `.gitignore` gains the two
registry rows' lines at startup (D11).
Phase 2 (Rust guard, 2 files): `src-tauri/src/pty/menu_guard/mod.rs`, `src-tauri/src/lib.rs`. The
guard reads layer 0 first, and every agent still carries a materialized array, so guard evaluation
is base; the visible change is that `settings-blocking-menus.json` is written at GUI startup (D5).
Phase 3 (Rust migration, 1 file): `src-tauri/src/config/settings.rs`. The only behavior change.
Phase 4 (docs, 3 files): `docs/features/menu-guard.md`, `docs/reference/settings.md`,
`docs/reference/directory-layout.md`.

The four phases ship in one PR; each is a separate commit that leaves `main` green on its own.

Phase 2 is carried over byte-identical from round 2 (both Rust reviewers voted it executable).
Its test T5a uses a test-only `CUSTOM` entry whose pattern is `^\s*Do you trust the files in this
folder\?`; phase 4 AC5 refers to the page bytes at `docs/features/menu-guard.md:113-118`, whose
pattern is `^[^A-Za-z0-9]*Do you trust the files in this folder\?`. The two are independent: the
test's pattern only has to match its own `CLAUDE_ROW`, and no test binds it to the page.

Compatibility:

- Old binary on a migrated install: `AgentConfig.blocking_menus` is absent, so the old
  `materialize_blocking_menus` fills the defaults again and the old guard works. The two new files
  are unknown to it and untouched. Downgrade is safe; a later upgrade re-runs D7 and finds only
  pristine arrays (or the user's edits, which win by D7 step 6).
- Settings saved from the GUI, CLI or API never carry `blockingMenus` (E7, D8, D9).
- `menuGuardEnabled` stays in `settings.json` and stays overlay-able.
- `list-peers` reads `blockedMenu` from session state and is untouched.

## 7. Cycle and layering statement

Measured on the clean base tree (`git status --porcelain` empty) with the instrument at
`../repo-personal/ObsidianVault/Coding Agents/IA-Programming/rust/01-rust_module-dependency-cycles.mjs`
(v1.1.0, present in this room), working directory the repository root:
`--write-baseline` exits 0 and records one module cycle, id `40d22fc71179d71f`, **86 members**
(the round-1 text said 85; 86 is the measured figure), containing `agentscommander_lib`,
`config::settings`, `pty::menu_guard` and `config::coding_agents_catalog`.
`config::local_config_io` and `config::instance_artifacts` are outside it (size-1 SCCs);
`local_config_io` has zero outgoing arcs (`grep -c "config::local_config_io ->" module-arcs.txt`
is 0). `--emit-graph` reports 199 modules, 3926 edges, `moduleCycles: 1`, exit 1; the recorder
reproduces the committed `module-arcs.txt` (1090 lines) byte for byte.

Arcs this plan adds, enumerated:

| From | To | Site | Verdict |
|---|---|---|---|
| `config::settings` | `config::local_config_io` | `write_file_atomic` calls in D5 (phase 1) and D7 (phase 3) | Into a zero-out-degree leaf outside the SCC: cannot create or grow an SCC. |

Every other reference the plan adds (`lib.rs` to `settings`, `menu_guard` to `settings`,
`settings` to `instance_artifacts` and `coding_agents_catalog`, `instance_gitignore` to
`instance_artifacts`) is an arc already in `src-tauri/module-arcs.txt` (`:16`, `:889`, `:677`,
`:676`, `:611`). No new module is created, so the SCC member set cannot change. Layering: no lower
layer gains a UI transport; `settings.rs` gains no `tauri` or `AppHandle` use.

Executable gate (phase 1 AC8 carries the full recipe; phases 2 and 3 inline it): capture the base
baseline from a `git archive 80aeb85` twin, capture the head baseline, compare the sorted member
sets of every cycle with one `node -e` command that exits 1 on any difference, then regenerate the
arc record. Expected: identical member sets; the record changes by exactly one added line in phase
1 and is byte-identical in phases 2 and 3.

## 8. Delivery invariants (delivery-nonfunctional-invariants, baseline gates)

| Gate | Source of truth | Evidence and owner | Failure behavior |
|---|---|---|---|
| CI parity | E15: `rust-regression` (Windows, full suite), `rust-regression-linux` (check, clippy, one filtered test), `rust-regression-macos` (check, clippy), `rust-fmt`, `frontend-regression` (typecheck), `test-debt` | Implementer runs the phase's local commands on Windows; CI on the exact PR-head SHA is authoritative for the three OS legs; test fixtures use no OS-specific path so the Windows-only suite proves nothing the other legs would contradict | Any red check on the PR head blocks delivery; no rerun on another SHA counts |
| Deterministic build | `Cargo.lock` as committed; `cargo` from the repo toolchain (stable, `Option::is_none_or` not needed since round 2) | Local commands in each phase file, run from `src-tauri` | Version drift is reported, not worked around |
| Authorized Git | Issue #1905, branch `feature/1905-externalize-blocking-menus`, base `80aeb85` | One commit per phase, PR to `main`; never push to `main` | Dirty tree or wrong branch stops the phase |
| Scope | The `Files` list of each phase file | `git diff --name-only HEAD~1..HEAD` equals the phase's list | Any extra path is removed or explained before review |
| Mutation and recovery | Per-file edits only; recovery by `git checkout -- <path>` on paths this phase touched and nothing else | `git status --porcelain` after each phase | Never a repo-wide reset |
| Bounded execution | `cargo test` commands carry `--lib` and a filter where given; no interactive stdin; T11 waits the 2-second lock timeout by design | Implementer captures stdout and stderr of each verification command | A timed-out or failed command is reported as such |
| Evidence | Each phase's acceptance criteria are commands with expected output and exit status | The reviewer re-runs them on the phase head | A criterion that cannot be run is a `NEEDS_ANOTHER_ROUND` |

Enhanced controls: the migration control of section 3 (schema parity, write-before-strip,
no-clobber, state-keyed retry, layer-0 compatibility), each with a named test in phase 3. No other
enhanced control applies.

## 9. Phase table

| Phase | Child issue | Class | Owner | Files | Depends on | Parallel with | Phase-SHA256 |
|---|---|---|---|---|---|---|---|
| 1 | #1905 | design-bearing | Rust | 4 (+ `module-arcs.txt`) | none | none | see report |
| 2 | #1905 | patterned | Rust | 2 | 1 | none | see report |
| 3 | #1905 | design-bearing | Rust | 1 | 1, 2 | none | see report |
| 4 | #1905 | patterned | docs | 3 | 1, 2, 3 | none | see report |

Phase files: `plans/1905-externalize-blocking-menus/phase-1-rust-store.md`,
`plans/1905-externalize-blocking-menus/phase-2-rust-guard.md`,
`plans/1905-externalize-blocking-menus/phase-3-rust-migration.md`,
`plans/1905-externalize-blocking-menus/phase-4-docs.md`. Each is self-contained and carries its own
`Status:` line; an implementer reads only their phase file. The `Phase-SHA256` column reads "see
report" on purpose: the digests live in the round message, so the epic's own digest does not depend
on the phase files.
