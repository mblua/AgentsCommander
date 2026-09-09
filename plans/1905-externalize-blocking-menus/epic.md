# Epic Plan #1905: externalize blocking-menu patterns into `settings-blocking-menus.json` with a `.local.json` overlay

Author: ac-architect-v4, room-5, 2026-09-09 UTC. Full `code-implementation-workflow` path, Round 1 candidate.
Status: READY_FOR_IMPLEMENTATION

Issue: [mblua/AgentsCommander#1905](https://github.com/mblua/AgentsCommander/issues/1905)
Delivery path: Full (score 42, raised by the schema/persistence/migration criterion).
PARTITION: 3 phases (cut by owner, Rust then docs; the Rust work is cut again at the green-tree
boundary between the store and the migration, so each phase file stays under the 400-line cap).

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
4. The runtime, the `list-peers` fields, `enabled: false`, `[]`, `menuGuardEnabled` and the
   keep-invalid-verbatim rule all keep their meaning.

## 2. Pinned base and delivery authority

- Repository: `D:\0_repos\AgentsCommander_iac\.ac\room-5-ac-dev-team-v4\repo-AgentsCommander`
- Target branch: `feature/1905-externalize-blocking-menus`, created from `main` at
  `80aeb85` (full SHA: `git rev-parse 80aeb85` on the branch; pin it in the PR body).
- Clean-tree precondition: `git status --porcelain` is empty before each phase starts.
- Version at base: `src-tauri/Cargo.toml:3` is `0.31.0`. No version bump belongs to this plan.

## 3. Task class and accepted threat model

Routine application-code change with a persisted-shape migration that moves user data between two
files in the same directory. No release, signing, packaging, untrusted host or security-boundary
change. Baseline gates apply; enhanced provenance controls (anchored executable hashes, DLL closure
inventories, poisoned-PATH tests) are not applicable and are not required.

The "destructive or irreversible migration" trigger is examined and answered with one proportionate
control rather than a harness: the migration writes the `.local` file BEFORE it strips
`settings.json`, is state-keyed (it runs only while a `blockingMenus` key is still on disk), never
overwrites a `byAgent` entry that already exists in the `.local` file, and refuses to run when the
`.local` file is present but unparseable. Phase 2 pins each of those four properties with a test
that drives the real loader over a tempdir. Reversal is by hand and is documented in phase 3: copy
the `byAgent` array back under the agent in `settings.json` and delete the two files.

## 4. Evidence (verified at the base SHA)

E1. `src-tauri/src/config/settings.rs:88-93` `AgentConfig.blocking_menus: Option<Vec<BlockingMenuEntry>>`,
`#[serde(default, skip_serializing_if = "Option::is_none")]`, wire key `blockingMenus`. `:104`
`BlockingMenuEntry { Valid(BlockingMenuConfig), Invalid(Value) }`, untagged. `:117`
`BlockingMenuConfig { pattern, notification, enabled (default true), captured_against }`.

E2. `settings.rs:1034` `CODEX_HOOKS_REVIEW_PATTERN`; `:1036` `codex_hooks_review_menu()`; `:1047`
`default_blocking_menus_for_command` (stems `pi` and `codex`, everything else `[]`); `:1074`
`materialize_blocking_menus` (fills `None` only); `:1105` `apply_issue_1757_migration` (content-keyed,
skips empty arrays, returns early when the overlay owns `agents`).

E3. Load-time call sites. GUI loader `load_settings_from_path` (`:2100`): `materialize` at `:2185`,
`1757` at `:2189`, `repair_coding_agent_profiles_config` at `:2193`, then `needs_save` routes through
`save_settings_to_path_preserving_project_paths_typed` (`:5075`) and adopts the fresh decode
(`settings = written`). CLI loaders: `load_settings_for_cli` (`:2272`) calls `repair` at
`:2340`, `materialize` at `:2341`, `1757` at `:2342`; `load_settings_for_cli_strict` (`:2359`) calls
them at `:2412-2414`. The CLI loaders never write (`:2337`, `:2345`).

E4. Overlay. `LocalSettingsOverlay::load_and_merge` (`src-tauri/src/config/local_overlay.rs:287`)
derives the overlay path as `settings_path.with_file_name("settings.local.json")` (`:294`).
`OVERLAY_KEY_AGENTS` (`settings.rs:2593`) and the #1737 D7c rule: an overlay that owns `agents`
replaces the whole array and `restore_base` writes the base array back on every save.

E5. Runtime read. `src-tauri/src/pty/menu_guard/mod.rs:265-270` finds the agent by
`session.agent_id` in `settings.agents` and clones `agent.blocking_menus`. `MenuGuard` (`:47`) holds
`sessions`, `compiled_patterns`, `next_episode_id`; `MenuGuard::new()` (`:63`) is `Default`.
`evaluate_logical_rows` (`:88`) takes `entries: &[BlockingMenuEntry]` and is unchanged by this plan.
`lib.rs:2969` constructs it: `Arc::new(crate::pty::menu_guard::MenuGuard::new())`, manages it at
`:2981`. `lib.rs:2648` loads settings first: `let loaded_settings = config::settings::load_settings();`.

E6. Test callers of `default_blocking_menus_for_command`: `menu_guard/mod.rs:389,399,400,436,536` and
`commands/session.rs:11119`. Every other `blocking_menus` mention outside `settings.rs` is a struct
constructor writing `None` (`agent_update.rs:5622`, `cli/coding_agent.rs:529,550`,
`cli/create_agent.rs:283`, `cli/self_switch.rs:504`, `commands/config.rs:2958`,
`config/agent_command.rs:1176`, `config/coding_agent_mutations.rs:585`,
`config/coding_agent_profiles.rs:806`, `lib.rs:4601`, `phone/mailbox.rs:13492`, `web/commands.rs:1373`,
`tests/pty_powershell_managed_native.rs:383`) or a hand-built `BlockingMenuEntry` passed to the
evaluator (`phone/mailbox.rs:26940`, `pty/inject.rs:1009`). None of them changes.

E7. Frontend: `src/shared/ipc.ts:579` `resolveBlockingMenu` and `src/sidebar/App.tsx:25,390` consume
the blocked event only. No TypeScript type carries `blockingMenus`, so a Settings save never
re-materializes the key.

E8. Shipped-JSON precedent: `src-tauri/src/config/coding_agents_catalog.rs:58`
`include_str!("../../resources/coding-agents/agents.default.json")`. `.gitattributes:4` pins
`*.json text eol=lf`, so the embedded bytes are LF on every checkout; the CRLF trap of #914 applies
to `.md`, not to `.json`. The on-disk copy is nevertheless written from a re-serialization, never
from the embedded bytes, so the "is it stale" comparison is against bytes AC itself produced.

E9. Atomic writer: `src-tauri/src/config/local_config_io.rs:80` `pub fn write_file_atomic(path, bytes)`
writes `.<name>.<pid>.tmp` beside the target and publishes by rename. The module has zero outgoing
arcs in the detector graph and is a size-1 SCC (measured, section 7).

E10. Instance artifact registry: `src-tauri/src/config/instance_artifacts.rs:153`
`SETTINGS_LOCAL_OVERRIDE_FILE_NAME`; rows at `:421-444` (`settings.json`, lock, local overlay,
migration backups), byte-ordered by `name` and pinned by `ignore_rows_are_unique_and_byte_sorted_by_name`
(`:556`). `instance_gitignore.rs:1026-1031` is the fixture list that every `Ignore` row must reach;
`:615` and `:1239` pin `rules.len() == 2 + ignore_rows().len()`, which adapts by itself.

E11. Test harnesses that exist and are reused: `settings.rs:9612`
`issue_1757_reaches_settings_json_through_the_real_load_chain` (tempdir + real loader + raw JSON
read-back); `:9703-9733` `base_fixture`, `seed(dir, base, local)`, `disk_object`; `:10845`
`an_overlay_owned_agents_array_suppresses_the_1757_migration`. `menu_guard/mod.rs:534` T11 replays a
vt100 frame against the shipped codex entries.

E12. Docs: `docs/features/menu-guard.md` (whole page written around `settings.json` hand-edits),
`docs/reference/settings.md:97` (`blockingMenus` row of `AgentConfig`) and `:460-481` (Menu guard
section), `docs/reference/directory-layout.md:78-95` (instance-dir file table).

E13. Default strings history: `git log -S` shows the pi and codex folder-trust literals unchanged since
`cb6e10c` (#1647) and the hooks-review literal unchanged since `81499f5` (#1757). No shipped default
ever changed text, so "equal to the current shipped set" is the only pristine shape that exists on
disk once the #1757 back-fill has run.

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

`schemaVersion` defaults to 1 and must be 1; any other value rejects the file (D6). `note` is an
optional string AC never reads. Both maps default to empty and are `BTreeMap` in Rust so the shipped
file serializes deterministically. Array elements are the existing `BlockingMenuEntry`, so an
entry AC cannot read is kept verbatim and skipped, exactly as today. Unknown top-level keys are
tolerated on read and preserved on the one merge write (D7).

D2. **Keying.** The shipped file uses `byCommand` only; a test pins that its `byAgent` is empty and
the resolver ignores a shipped `byAgent`. The `.local` file may use either map. Keys of `byCommand`
are the lowercase executable stem that `command_executable_basename` produces
(`coding_agents_catalog.rs:626`); the lookup is an exact string match and the docs say so.

D3. **Precedence, replace-whole at every level.** For a session whose agent has id `id` and command
stem `stem`, the effective array is the first present of:

1. `local.byAgent[id]`
2. `local.byCommand[stem]`
3. `shipped.byCommand[stem]`
4. `[]`

A present array replaces the layers below it entirely, so `"byAgent": {"codex": []}` in the `.local`
file switches the guard off for that agent, and `enabled: false` inside a `.local` copy of a shipped
array disables one pattern durably. This is the same "non-object replaces whole" rule the
`settings.local.json` overlay already documents, and it is not a regression: today every array is
frozen; after this plan only arrays the user chose to own are.

D4. **Shipped content lives in the source tree** at
`src-tauri/resources/blocking-menus/settings-blocking-menus.json`, embedded with `include_str!` from
`settings.rs`. It carries exactly the three entries `default_blocking_menus_for_command` returns at
base (E2), under `byCommand.pi` and `byCommand.codex`, in that order, plus a `note` telling the
reader that AC rewrites the file and where user entries go. `default_blocking_menus_for_command`
stays, with its name and signature, and becomes a read of the embedded content, so every test caller
in E6 keeps compiling and keeps meaning "the shipped set for this command".

D5. **Refresh rule.** At every GUI startup, AC parses the embedded content, serializes it with
`serde_json::to_vec_pretty` plus one trailing `\n`, and compares those bytes with the bytes on disk
at `settings-blocking-menus.json`. If the file is absent or the bytes differ, AC writes the
canonical bytes through `write_file_atomic` (E9). If they are equal, nothing is written. There is
no version stamp: the content is its own identity, and a release whose patterns did not change
writes nothing. A hand edit inside the shipped file is therefore undone at the next start, and the
file's `note` and both doc pages say so. A write failure is logged once at error level and the
embedded content serves from memory; the in-memory truth is always the embedded content, never the
disk copy.

D6. **Reading the `.local` file.** Absent is normal and yields an empty layer. Unreadable, not JSON,
not an object, or `schemaVersion != 1` is logged once at error level, yields an empty layer for the
session, and (D7) blocks the migration. AC never rewrites this file except in D7.

D7. **One-shot migration, state-keyed and no-clobber.** `export_blocking_menus_to_local_file`
runs in the GUI loader only, in the `[settings-migration]` block where `materialize` and `1757` run
today (E3), and only when at least one agent has `blocking_menus == Some(_)`. Steps:

1. If the overlay owns `agents` (E4), return `false` without touching anything: the in-memory
   arrays came from `settings.local.json`, which AC never writes, so they cannot be stripped. The
   resolver ignores `AgentConfig.blocking_menus` entirely (D9), so those arrays are inert from now
   on; one info line per such agent names the id and the `.local` file to move them to.
2. Apply the existing #1757 back-fill in memory (`apply_issue_1757_migration`, unchanged), so a
   pre-#1757 pristine codex array compares equal to the shipped set in the next step.
3. For each agent with `Some(array)`: if `array == default_blocking_menus_for_command(&agent.command)`
   the array is pristine and produces no `.local` entry; otherwise it is a candidate
   `byAgent[agent.id] = array` (Invalid entries included, verbatim).
4. Read the `.local` file as a raw `serde_json::Value` object (absent = empty object). If it exists
   but is not a JSON object, or its `schemaVersion` is present and not 1, log an error and return
   `false` without stripping anything; the migration retries at the next start.
5. Insert every candidate whose id is not already a key of `byAgent`; an existing key wins and is
   logged at info level. If at least one candidate was inserted, set `schemaVersion: 1` if absent
   and write the object with `serde_json::to_vec_pretty` plus `\n` through `write_file_atomic`.
   If that write fails, log an error and return `false` without stripping anything.
6. Only now set `blocking_menus = None` on every agent and return `true`, which sets `needs_save`
   so the existing PRESERVE-mode save removes every `blockingMenus` key from `settings.json`.

Idempotency follows from the state key: after step 6 lands, no agent has `Some`, so the function is
a no-op forever. If the settings save fails after step 5, the next start recomputes the same
candidates, finds them present, inserts nothing, and strips again.

D8. **CLI loaders stop materializing.** `load_settings_for_cli` and `load_settings_for_cli_strict`
drop their `materialize_blocking_menus` and `apply_issue_1757_migration` calls and touch
`blocking_menus` nowhere. They round-trip whatever is on disk unchanged. `materialize_blocking_menus`
is deleted; `apply_issue_1757_migration` survives as a private step of D7.

D9. **`AgentConfig.blocking_menus` stays as a legacy field.** It is read by D7 only, is `None` on
every install after D7, and no production writer ever sets `Some`. The serde attributes are
unchanged, so the key is absent from disk after the migration and every constructor in E6 keeps
compiling. Removing the field would touch 14 files for no behavior and would strip the key from an
overlay-owned array on the next save (E4), so it stays.

D10. **The store lives inside `MenuGuard`.** `BlockingMenusStore { shipped: &'static BlockingMenusFile,
local: BlockingMenusFile }` is defined in `settings.rs`. `MenuGuard` gains a `store` field;
`MenuGuard::new()` keeps its signature and uses `BlockingMenusStore::shipped_only()`, so every
existing test still evaluates the shipped set; `MenuGuard::with_store(store)` is what `lib.rs:2969`
calls, after `load_settings()` has run the migration at `lib.rs:2648`. The scan loop resolves
`store.resolve(agent_id, &agent.command)` in place of `agent.blocking_menus.clone()`. No file
watcher is added; both files are read once per process, exactly like `settings.json`.

D11. **Registry rows.** Two `Ignore` rows join `INSTANCE_ARTIFACTS`, with constants
`BLOCKING_MENUS_SHIPPED_FILE_NAME = "settings-blocking-menus.json"` and
`BLOCKING_MENUS_LOCAL_FILE_NAME = "settings-blocking-menus.local.json"`, placed immediately before
the `settings.json` row because `-` (0x2D) sorts before `.` (0x2E). The fixture list in
`instance_gitignore.rs` gains both names.

## 6. Scope, files, and compatibility

Phase 1 (Rust store, 6 files): `src-tauri/resources/blocking-menus/settings-blocking-menus.json`
(new), `src-tauri/src/config/settings.rs`, `src-tauri/src/pty/menu_guard/mod.rs`,
`src-tauri/src/lib.rs`, `src-tauri/src/config/instance_artifacts.rs`,
`src-tauri/src/config/instance_gitignore.rs`, plus the regenerated `src-tauri/module-arcs.txt`
(section 7). After phase 1 the old arrays are still materialized in `settings.json` but no longer
read; phases 1 and 2 ship in the same PR, so that state never ships alone.
Phase 2 (Rust migration, 1 file): `src-tauri/src/config/settings.rs`.
Phase 3 (docs, 3 files): `docs/features/menu-guard.md`, `docs/reference/settings.md`,
`docs/reference/directory-layout.md`.

Compatibility:

- Old binary on a migrated install: `AgentConfig.blocking_menus` is absent, so the old
  `materialize_blocking_menus` fills the defaults again and the old guard works. The two new files
  are unknown to it and untouched. Downgrade is safe; a later upgrade re-runs D7 and finds only
  pristine arrays (or the user's edits, which win by D7 step 5).
- Settings saved from the GUI, CLI or API never carry `blockingMenus` (E7, D8, D9).
- `menuGuardEnabled` stays in `settings.json` and stays overlay-able.
- `list-peers` reads `blockedMenu` from session state and is untouched.

## 7. Cycle and layering statement

Measured on the clean base tree with
`node "<VAULT>/rust/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph pre.json --quiet`:
199 modules, 3926 edges, `moduleCycles: 1`, one cyclic SCC of 85 members that already contains
`agentscommander_lib`, `config::settings`, `pty::menu_guard` and `config::coding_agents_catalog`.
`config::local_config_io`, `config::instance_artifacts`, `config::local_overlay` and `path_identity`
are size-1 SCCs; `local_config_io` has zero outgoing arcs.

Arcs this plan adds, enumerated:

| From | To | Site | Verdict |
|---|---|---|---|
| `config::settings` | `config::local_config_io` | `write_file_atomic` calls in D5 and D7 | Into a zero-out-degree leaf: cannot create or grow an SCC (same argument as `instance_artifacts.rs:14-23`). |

Every other reference the plan adds (`lib.rs` to `settings`, `menu_guard` to `settings`,
`settings` to `instance_artifacts` and `coding_agents_catalog`, `instance_gitignore` to
`instance_artifacts`) is an arc that already exists in `src-tauri/module-arcs.txt`
(`:16`, `:889`, `:676-677`). No new module is created, so the SCC member set cannot change.
Layering: no lower layer gains a UI transport; `settings.rs` gains no `tauri` or `AppHandle` use.

Acceptance (phase 1 AC8, re-checked by phase 2 AC8): re-run the detector on the phase head, `moduleCycles` stays 1,
the single cyclic SCC has the same 85 members, and `node scripts/02-module-arc-record.mjs --graph
post.json --out src-tauri/module-arcs.txt` changes the committed record by exactly one added line,
`agentscommander_lib::config::settings -> agentscommander_lib::config::local_config_io`, which phase 1
commits.

## 8. Delivery invariants (delivery-nonfunctional-invariants, baseline gates)

| Gate | Source of truth | Evidence and owner | Failure behavior |
|---|---|---|---|
| CI parity | `.github/workflows/pr-regression-gates.yml`: `rust-regression` (Windows: `cargo check --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --lib --bins --tests`), `rust-regression-linux`, `rust-regression-macos`, `rust-fmt` (`cargo fmt --all -- --check`), `test-debt`, typecheck job (`npm run typecheck`) | Implementer runs the phase's local commands; CI on the exact PR-head SHA is authoritative for the three OS legs | Any red check on the PR head blocks delivery; no rerun on another SHA counts |
| Deterministic build | `Cargo.lock` as committed; `cargo` from the repo toolchain | Local commands in each phase file, run from `src-tauri` | Version drift is reported, not worked around |
| Authorized Git | Issue #1905, branch `feature/1905-externalize-blocking-menus`, base `80aeb85` | One commit per phase, PR to `main`; never push to `main` | Dirty tree or wrong branch stops the phase |
| Scope | The `Files` list of each phase file | `git diff --name-only $(git merge-base main HEAD)..HEAD` equals the phase's list (plus earlier phases' lists and this plan directory) | Any extra path is removed or explained before review |
| Mutation and recovery | Per-file edits only; recovery by `git checkout -- <path>` on paths this phase touched and nothing else | `git status --porcelain` after each phase | Never a repo-wide reset |
| Bounded execution | `cargo test` commands carry `--lib` and a filter where given; no interactive stdin | Implementer captures stdout and stderr of each verification command | A timed-out or failed command is reported as such |
| Evidence | Each phase's acceptance criteria are commands with expected output | The reviewer re-runs them on the phase head | A criterion that cannot be run is a `NEEDS_ANOTHER_ROUND` |

Enhanced controls: none applicable (section 3).

## 9. Phase table

| Phase | Child issue | Class | Owner | Files | Depends on | Parallel with | Phase-SHA256 |
|---|---|---|---|---|---|---|---|
| 1 | #1905 | design-bearing | Rust | 6 (+ `module-arcs.txt`) | none | none | see report |
| 2 | #1905 | design-bearing | Rust | 1 | 1 | none | see report |
| 3 | #1905 | patterned | docs | 3 | 1, 2 | none | see report |

Phase files: `plans/1905-externalize-blocking-menus/phase-1-rust-store.md`,
`plans/1905-externalize-blocking-menus/phase-2-rust-migration.md`,
`plans/1905-externalize-blocking-menus/phase-3-docs.md`. Each is self-contained; an implementer
reads only their phase file. The `Phase-SHA256` column reads "see report" on purpose: the digests
live in the round message, so the epic's own digest does not depend on the phase files.
