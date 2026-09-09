# #1905 Phase 1: the blocking-menus store, the shipped file and the `.local` overlay

Class: design-bearing. Owner: Rust. Depends on: nothing. Parallel with: nothing.
Branch: `feature/1905-externalize-blocking-menus`. Base: `main` at `80aeb85`.
Every line number below is pinned to that base and describes the tree BEFORE this phase's edits.

## Objective

The menu guard stops reading `AgentConfig.blocking_menus` and resolves its entries through a
store built from two files next to `settings.json`: `settings-blocking-menus.json`, which AC owns
and rewrites from embedded content whenever it differs, and `settings-blocking-menus.local.json`,
which the user owns and AC only reads. After this phase, `settings.json` still carries the old
arrays (phase 2 moves them); they are materialized but no longer read. Phases 1 and 2 land in the
same PR, so that intermediate state never ships alone.

## Files

1. `src-tauri/resources/blocking-menus/settings-blocking-menus.json` (new)
2. `src-tauri/src/config/settings.rs`
3. `src-tauri/src/pty/menu_guard/mod.rs`
4. `src-tauri/src/lib.rs`
5. `src-tauri/src/config/instance_artifacts.rs`
6. `src-tauri/src/config/instance_gitignore.rs`
7. `src-tauri/module-arcs.txt` (regenerated, AC8)

No other file changes. Every `blocking_menus: None` constructor elsewhere and both hand-built
entry lists (`phone/mailbox.rs:26940`, `pty/inject.rs:1009`) compile unchanged.

## Decisions (inlined, binding)

- **One schema, two files.** `BlockingMenusFile { schemaVersion (default 1, must be 1), note
  (optional, ignored), byCommand: {stem: [BlockingMenuEntry]}, byAgent: {agentId: [BlockingMenuEntry]} }`.
  Maps are `BTreeMap`. Unknown top-level keys are tolerated on read.
- **Precedence, replace-whole:** `local.byAgent[id]`, else `local.byCommand[stem]`, else
  `shipped.byCommand[stem]`, else `[]`. The shipped `byAgent` is ignored and pinned empty. `stem`
  is `command_executable_basename(command)` (`coding_agents_catalog.rs:626`), exact match.
- **Shipped file refresh:** at GUI startup, canonical bytes = `serde_json::to_vec_pretty(embedded)`
  plus one `\n`. Absent or different on disk means write through `write_file_atomic`
  (`local_config_io.rs:80`); equal means no write. Embedded content is the in-memory truth always.
- **`.local` read:** absent is empty. Unreadable, invalid JSON, not an object, or
  `schemaVersion != 1` logs one `error!` line and yields an empty layer. AC never writes it here.
- **Store ownership:** `MenuGuard` owns a `BlockingMenusStore`. `MenuGuard::new()` is shipped-only;
  `MenuGuard::with_store(store)` is what `lib.rs` uses.
- `default_blocking_menus_for_command` keeps its name and signature and reads the embedded
  content, so every test caller keeps meaning "the shipped set for this command".

## Edit 1: the shipped resource (new file)

Exact bytes of `src-tauri/resources/blocking-menus/settings-blocking-menus.json` (LF, one trailing
newline; `.gitattributes:4` pins `*.json` to LF). Regex backslashes are doubled in JSON:

```json
{
  "schemaVersion": 1,
  "note": "Owned by AgentsCommander. Rewritten from the running version's embedded content at every start, so edits here are lost. Put your own patterns in settings-blocking-menus.local.json next to this file.",
  "byCommand": {
    "codex": [
      {
        "pattern": "^\\s*Do you trust the contents of this directory\\?",
        "notification": "codex is waiting for you to answer the folder-trust menu in this terminal",
        "enabled": true,
        "capturedAgainst": "codex 0.x / Linux"
      },
      {
        "pattern": "^[^A-Za-z0-9]*Hooks need review\\b",
        "notification": "codex is waiting for you to answer the hooks-review menu in this terminal",
        "enabled": true,
        "capturedAgainst": "codex 0.153.2 / Windows"
      }
    ],
    "pi": [
      {
        "pattern": "^\\s*Trust project folder\\?",
        "notification": "pi is waiting for you to answer the folder-trust menu in this terminal",
        "enabled": true,
        "capturedAgainst": "pi 0.52 / Windows"
      }
    ]
  },
  "byAgent": {}
}
```

Test T1 binds these bytes to the Rust literals, so a lost backslash fails the tests.

## Edit 2: `src-tauri/src/config/settings.rs`

2a. Line 10 becomes
`use crate::config::instance_artifacts::{BLOCKING_MENUS_LOCAL_FILE_NAME, BLOCKING_MENUS_SHIPPED_FILE_NAME, SETTINGS_LOCK_FILE_NAME};`
and line 7 becomes `use std::sync::{Arc, OnceLock};`.

2b. Replace `default_blocking_menus_for_command` (`:1046-1071`) with the block below. Leave
`CODEX_HOOKS_REVIEW_PATTERN` (`:1034`), `codex_hooks_review_menu` (`:1036`),
`materialize_blocking_menus` (`:1073`) and `apply_issue_1757_migration` (`:1105`) untouched;
rewrite the doc comment at `:1030-1033` to name the two consumers now: `apply_issue_1757_migration`
and test T1, which pins the embedded JSON to the constant.

```rust
/// #1905 - the patterns AC ships, embedded at build time. The config-dir copy is a
/// materialization of this content; the runtime evaluates this constant, never the disk file.
const EMBEDDED_BLOCKING_MENUS_JSON: &str =
    include_str!("../../resources/blocking-menus/settings-blocking-menus.json");

pub const BLOCKING_MENUS_SCHEMA_VERSION: u32 = 1;

fn default_blocking_menus_schema_version() -> u32 {
    BLOCKING_MENUS_SCHEMA_VERSION
}

/// #1905 - one blocking-menus file; the shipped file and the `.local` file share this shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BlockingMenusFile {
    #[serde(default = "default_blocking_menus_schema_version")]
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default)]
    pub by_command: BTreeMap<String, Vec<BlockingMenuEntry>>,
    #[serde(default)]
    pub by_agent: BTreeMap<String, Vec<BlockingMenuEntry>>,
}

impl Default for BlockingMenusFile {
    fn default() -> Self {
        Self {
            schema_version: BLOCKING_MENUS_SCHEMA_VERSION,
            note: None,
            by_command: BTreeMap::new(),
            by_agent: BTreeMap::new(),
        }
    }
}

/// A parse failure is a build defect T1 catches; production logs once and serves an empty file.
pub fn shipped_blocking_menus() -> &'static BlockingMenusFile {
    static SHIPPED: OnceLock<BlockingMenusFile> = OnceLock::new();
    SHIPPED.get_or_init(|| {
        serde_json::from_str::<BlockingMenusFile>(EMBEDDED_BLOCKING_MENUS_JSON).unwrap_or_else(|e| {
            log::error!("[blocking-menus] embedded settings-blocking-menus.json does not parse: {e}");
            BlockingMenusFile::default()
        })
    })
}

/// #1646 / #1647 / #1905 - the shipped patterns for a command, by executable stem.
pub fn default_blocking_menus_for_command(command: &str) -> Vec<BlockingMenuEntry> {
    let Some(stem) = crate::config::coding_agents_catalog::command_executable_basename(command)
    else {
        return Vec::new();
    };
    shipped_blocking_menus().by_command.get(&stem).cloned().unwrap_or_default()
}

pub(crate) fn blocking_menus_shipped_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(BLOCKING_MENUS_SHIPPED_FILE_NAME)
}

pub(crate) fn blocking_menus_local_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(BLOCKING_MENUS_LOCAL_FILE_NAME)
}

/// Pretty JSON plus one trailing newline: the only byte shape AC writes for both files.
pub(crate) fn pretty_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// #1905 (D5) - make the on-disk shipped file equal to the embedded content. True when written.
pub(crate) fn refresh_shipped_blocking_menus_file(settings_path: &Path) -> bool {
    let path = blocking_menus_shipped_path(settings_path);
    let Ok(canonical) = pretty_json_bytes(shipped_blocking_menus()) else {
        return false;
    };
    if matches!(std::fs::read(&path), Ok(existing) if existing == canonical) {
        return false;
    }
    match crate::config::local_config_io::write_file_atomic(&path, &canonical) {
        Ok(()) => {
            log::info!("[blocking-menus] wrote the shipped patterns to {}", path.display());
            true
        }
        Err(e) => {
            log::error!("[blocking-menus] could not write {}: {e}", path.display());
            false
        }
    }
}

/// #1905 (D6) - the user layer. Every rejection logs once and yields an empty layer.
pub(crate) fn load_local_blocking_menus_file(settings_path: &Path) -> BlockingMenusFile {
    let path = blocking_menus_local_path(settings_path);
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return BlockingMenusFile::default(),
        Err(e) => {
            log::error!("[blocking-menus] could not read {}: {e}", path.display());
            return BlockingMenusFile::default();
        }
    };
    match serde_json::from_str::<BlockingMenusFile>(&contents) {
        Ok(file) if file.schema_version == BLOCKING_MENUS_SCHEMA_VERSION => file,
        Ok(file) => {
            log::error!("[blocking-menus] {} has schemaVersion {}; only {} is supported, ignoring the file", path.display(), file.schema_version, BLOCKING_MENUS_SCHEMA_VERSION);
            BlockingMenusFile::default()
        }
        Err(e) => {
            log::error!("[blocking-menus] {} does not parse, ignoring the file: {e}", path.display());
            BlockingMenusFile::default()
        }
    }
}

/// #1905 (D3, D10) - what the menu guard evaluates. Built once per process; no file watcher.
#[derive(Debug, Clone)]
pub struct BlockingMenusStore {
    shipped: &'static BlockingMenusFile,
    local: BlockingMenusFile,
}

impl BlockingMenusStore {
    pub fn shipped_only() -> Self {
        Self::with_local(BlockingMenusFile::default())
    }

    pub fn with_local(local: BlockingMenusFile) -> Self {
        Self { shipped: shipped_blocking_menus(), local }
    }

    /// GUI startup entry point: refresh the shipped file, then read the user layer.
    pub fn load_from_config_dir() -> Self {
        match settings_path() {
            Some(path) => Self::load_from_settings_path(&path),
            None => Self::shipped_only(),
        }
    }

    pub(crate) fn load_from_settings_path(settings_path: &Path) -> Self {
        refresh_shipped_blocking_menus_file(settings_path);
        Self::with_local(load_local_blocking_menus_file(settings_path))
    }

    pub fn resolve(&self, agent_id: &str, command: &str) -> Vec<BlockingMenuEntry> {
        if let Some(entries) = self.local.by_agent.get(agent_id) {
            return entries.clone();
        }
        let Some(stem) = crate::config::coding_agents_catalog::command_executable_basename(command)
        else {
            return Vec::new();
        };
        if let Some(entries) = self.local.by_command.get(&stem) {
            return entries.clone();
        }
        self.shipped.by_command.get(&stem).cloned().unwrap_or_default()
    }
}
```

`cargo fmt` rewraps the long `log::error!` lines; the texts are binding.

2c. Tests: new module `blocking_menus_1905` inside `mod tests`, using `tempfile::tempdir()` and
a `settings.json` path inside it (no settings file needs to exist for T1 to T4).

- T1 `embedded_content_matches_the_rust_literals`: `shipped_blocking_menus()` has
  `schema_version == 1`, `note.is_some()`, empty `by_agent`, `by_command` keys exactly
  `["codex", "pi"]`; `by_command["pi"]` is one `Valid` entry with pattern
  `r"^\s*Trust project folder\?"`, `enabled`, `captured_against == Some("pi 0.52 / Windows")`;
  `by_command["codex"][0]` has pattern `r"^\s*Do you trust the contents of this directory\?"` and
  `captured_against == Some("codex 0.x / Linux")`;
  `by_command["codex"][1] == BlockingMenuEntry::Valid(codex_hooks_review_menu())`.
- T2 `resolve_precedence_is_local_agent_then_local_command_then_shipped`: local
  `by_agent = {"codex-b": [X], "pi-off": []}`, `by_command = {"codex": [Y]}`; assert
  `resolve("codex-b","codex") == [X]`, `resolve("codex-a","codex") == [Y]`,
  `resolve("codex-a", r"C:\tools\Codex.exe --search") == [Y]`, `resolve("pi-off","pi")` empty,
  `resolve("pi-1","pi") == default_blocking_menus_for_command("pi")`, `resolve("claude-1","claude")` empty.
- T3 `shipped_file_is_written_when_absent_or_stale_and_left_alone_when_equal`: first `refresh`
  returns true and the bytes equal `pretty_json_bytes(shipped_blocking_menus())`; second returns
  false; append `x`; third returns true and the bytes equal canonical again.
- T4 `local_file_rejections_yield_an_empty_layer`: absent, `{ not json`, `[1]`,
  `{"schemaVersion": 2}` each load as `BlockingMenusFile::default()`; a valid file with one
  `byAgent` entry loads it.

Existing tests `:9230`, `:9331`, `:9371`, `:9505-9612` and `:10845` are unchanged in this phase.

## Edit 3: `src-tauri/src/pty/menu_guard/mod.rs`

- Line 15 becomes
  `use crate::config::settings::{BlockingMenuConfig, BlockingMenuEntry, BlockingMenusStore, SettingsState};`.
- `MenuGuard` (`:47-51`) gains `store: BlockingMenusStore`; `impl Default` (`:53-61`) sets
  `store: BlockingMenusStore::shipped_only()`. Add to `impl MenuGuard`:

```rust
    pub fn with_store(store: BlockingMenusStore) -> Self {
        Self {
            store,
            ..Self::default()
        }
    }

    pub(crate) fn entries_for(&self, agent_id: &str, command: &str) -> Vec<BlockingMenuEntry> {
        self.store.resolve(agent_id, command)
    }
```

- Scan loop `:265-270`: replace `.and_then(|agent| agent.blocking_menus.clone())` with
  `.map(|agent| self.entries_for(&agent.id, &agent.command))`; the rest of the chain stays.
- T5 `with_store_overrides_the_shipped_set_for_one_agent` in the existing `mod tests`: a store
  with local `by_agent = {"pi-1": []}`; `MenuGuard::with_store(store).entries_for("pi-1","pi")` is
  empty while `MenuGuard::new().entries_for("pi-1","pi").len() == 1`. Tests at `:395`, `:433`,
  `:483`, `:534` are unchanged.

## Edit 4: `src-tauri/src/lib.rs`

Line 2969 becomes
`let menu_guard = Arc::new(crate::pty::menu_guard::MenuGuard::with_store(config::settings::BlockingMenusStore::load_from_config_dir()));`
(wrapped by `cargo fmt`). It runs after `load_settings()` at `:2648`. `lib.rs:4601` is untouched.

## Edit 5: `src-tauri/src/config/instance_artifacts.rs`

After `:153` add:

```rust
/// #1905 - AC-owned shipped blocking-menu patterns; rewritten from embedded content at startup.
pub(crate) const BLOCKING_MENUS_SHIPPED_FILE_NAME: &str = "settings-blocking-menus.json";
/// #1905 - the operator-owned blocking-menu overlay, read at load; written once by the export.
pub(crate) const BLOCKING_MENUS_LOCAL_FILE_NAME: &str = "settings-blocking-menus.local.json";
```

Insert two `Ignore` rows of `kind: ArtifactKind::File` immediately BEFORE the `settings.json` row
(`:421`), first `BLOCKING_MENUS_SHIPPED_FILE_NAME` with comment
`# AgentsCommander: shipped blocking-menu patterns; rewritten from the binary at every start`, then
`BLOCKING_MENUS_LOCAL_FILE_NAME` with comment
`# AgentsCommander: operator-owned blocking-menu overlay; machine-local by design`. The shape is
the `settings.local.json` row at `:433-438`; `ignore_rows_are_unique_and_byte_sorted_by_name`
(`:556`) proves the position (`-` sorts before `.`).

## Edit 6: `src-tauri/src/config/instance_gitignore.rs`

In the fixture list, after `"settings.json.lock",` (`:1027`) add
`"settings-blocking-menus.json",` and `"settings-blocking-menus.local.json",`.
`HISTORICAL_FIXED_RULES` (`:511`) is untouched.

## Required behavior and failure behavior

- No files yet: shipped file written once; `.local` absent; pi and codex detect what they detect today.
- Stale shipped file: rewritten with one `info!`. Unwritable dir: one `error!`, embedded content serves.
- `.local` broken: one `error!`, shipped content serves.
- An agent whose `settings.json` array was customized is served by the shipped set until phase 2
  moves the array; this is the documented same-PR intermediate state.
- `menuGuardEnabled: false` still clears every held session on the next tick (untouched code).

## Verification (from `src-tauri`)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --lib -- blocking_menus_1905 menu_guard instance_artifacts instance_gitignore
cargo test --lib --bins --tests
```

## Acceptance criteria

- AC1 `git diff --name-only $(git merge-base main HEAD)..HEAD` lists exactly the seven files above
  plus `plans/1905-externalize-blocking-menus/*`.
- AC2 `rg -n "\.blocking_menus" src-tauri/src/pty/menu_guard/mod.rs` prints nothing and exits 1;
  control: the same over `src-tauri/src/config/settings.rs` prints 3 or more lines.
- AC3 `rg -n "include_str" src-tauri/src/config/settings.rs` prints exactly 1 line naming
  `settings-blocking-menus.json`.
- AC4 `node -e "const f=require('./src-tauri/resources/blocking-menus/settings-blocking-menus.json'); console.log(f.byCommand.pi[0].pattern, f.byCommand.codex[1].pattern)"`
  prints `^\s*Trust project folder\? ^[^A-Za-z0-9]*Hooks need review\b` (single backslashes).
- AC5 The four verification commands exit 0; the filtered run reports 5 or more newly added tests
  passed (T1 to T5) and every pre-existing `menu_guard` test still passes.
- AC6 Manual, once: launch the phase head with an empty config dir; `settings-blocking-menus.json`
  appears with the Edit 1 content re-serialized (LF, trailing newline); launch again and its
  modification time does not change. Quote both listings in the phase reply.
- AC7 CI on the exact PR head: `rust-regression`, `rust-regression-linux`, `rust-regression-macos`,
  `rust-fmt`, `test-debt` and the typecheck job all green.
- AC8 Cycle gate, clean tree at the phase head:
  `node "<VAULT>\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph post.json --quiet`
  (exit 1 is the normal outcome; only 3 means no graph), then
  `node scripts/02-module-arc-record.mjs --graph post.json --out src-tauri/module-arcs.txt`.
  `git diff --stat src-tauri/module-arcs.txt` shows exactly one insertion, the line
  `agentscommander_lib::config::settings -> agentscommander_lib::config::local_config_io`, committed
  in this phase; `summary.moduleCycles` in `post.json` is `1` and the cyclic SCC has the same 85
  members as at base (Tarjan over `edges` with `cfgGated` excluded, or `--baseline`).

## Preserve

`evaluate_logical_rows` signature and behavior; `BlockingMenuEntry` and `BlockingMenuConfig` serde
shapes; `CODEX_HOOKS_REVIEW_PATTERN` and `codex_hooks_review_menu`; `materialize_blocking_menus`
and `apply_issue_1757_migration` and their call sites (phase 2 owns them); the name and signature
of `default_blocking_menus_for_command`; `AgentConfig.blocking_menus`; `menu_guard_enabled`;
`ERR_MENU_GUARD_DEFERRED`; the T11 replay test at `menu_guard/mod.rs:534`; every
`blocking_menus: None` constructor listed in the epic; `commands/session.rs:11119`.
