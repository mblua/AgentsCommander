# #1905 Phase 1: the blocking-menus store, the shipped file and the `.local` overlay

Status: READY_FOR_IMPLEMENTATION
Class: design-bearing. Owner: Rust. Depends on: nothing. Parallel with: nothing.
Branch: `feature/1905-externalize-blocking-menus`. Base: `main` at `80aeb85`.
Every line number below is pinned to that base and describes the tree BEFORE this phase's edits.

## Objective

Add the store the menu guard will read in phase 2: `settings-blocking-menus.json`, which AC owns
and rewrites from embedded content whenever it differs, and `settings-blocking-menus.local.json`,
which the user owns and AC only reads. Nothing in production calls the store yet and the settings
loader is untouched, so guard evaluation is base; the one visible change is two more lines in
every instance `.gitignore` at startup (the registry rows of Edit 3).

## Files (no other file changes)

1. `src-tauri/resources/blocking-menus/settings-blocking-menus.json` (new)
2. `src-tauri/src/config/settings.rs`
3. `src-tauri/src/config/instance_artifacts.rs`
4. `src-tauri/src/config/instance_gitignore.rs`
5. `src-tauri/module-arcs.txt` (regenerated, AC8)

## Decisions (inlined, binding)

- **One schema, two files.** `BlockingMenusFile { schemaVersion (default 1, must be 1), note
  (optional string, ignored), byCommand: {stem: [BlockingMenuEntry]}, byAgent: {agentId: [BlockingMenuEntry]} }`.
  Maps are `BTreeMap`. Unknown top-level keys are tolerated on read. A wrong type for `note`,
  `byCommand` or `byAgent`, a non-object, or `schemaVersion != 1` rejects the whole file.
- **One parser.** `parse_blocking_menus_file(&str)` is the only path from text to typed file:
  parse to `serde_json::Value`, require `Value::Object` (a serde struct whose fields all have
  defaults also decodes from a JSON array, so without this check `[1]` and `[]` would pass as an
  empty file), then `from_value`, then the `schemaVersion` check. The loader and, in phase 3,
  the migration both use it, so they accept and reject the same bytes.
- **Precedence, replace-whole:** layer 0 `agent.blocking_menus == Some(array)` (legacy, still in
  memory), else `local.byAgent[id]`, else `local.byCommand[stem]`, else `shipped.byCommand[stem]`,
  else `[]`. The shipped `byAgent` is ignored and pinned empty. `stem` is
  `command_executable_basename(command)` (`coding_agents_catalog.rs:626`), exact match.
- **Shipped file refresh:** when the store is built, canonical bytes = `serde_json::to_vec_pretty(embedded)`
  plus one `\n`. Absent or different on disk means write through `write_file_atomic`
  (`local_config_io.rs:80`); equal means no write. Embedded content is the in-memory truth always.
  The refresh lives in the store constructor, not in the settings loader.
- **`.local` read:** absent is empty. Unreadable or rejected by the parser logs one `error!` line
  and yields an empty layer. AC never writes it in this phase.
- `default_blocking_menus_for_command` keeps its name and signature and reads the embedded
  content, so every test caller keeps meaning "the shipped set for this command".

## Edit 1: the shipped resource (new file)

Exact bytes of `src-tauri/resources/blocking-menus/settings-blocking-menus.json` (LF, one trailing
newline; `.gitattributes:4` pins `*.json` to LF). Regex backslashes are doubled in JSON; test T1
binds these bytes to the Rust literals, so a lost backslash fails the tests:

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

## Edit 2: `src-tauri/src/config/settings.rs`

2a. Line 10 becomes
`use crate::config::instance_artifacts::{BLOCKING_MENUS_LOCAL_FILE_NAME, BLOCKING_MENUS_SHIPPED_FILE_NAME, SETTINGS_LOCK_FILE_NAME};`
and line 7 becomes `use std::sync::{Arc, OnceLock};`.

2b. Replace `default_blocking_menus_for_command` (`:1046-1070`) with the block below. Leave
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

fn default_blocking_menus_schema_version() -> u32 { BLOCKING_MENUS_SCHEMA_VERSION }

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

/// #1905 (D6) - the one parser both files go through; the migration validates against it too.
/// The `Value` step rejects a JSON array, which serde would otherwise decode as an empty file.
pub(crate) fn parse_blocking_menus_file(contents: &str) -> Result<BlockingMenusFile, String> {
    let value =
        serde_json::from_str::<Value>(contents).map_err(|e| format!("does not parse: {e}"))?;
    if !value.is_object() {
        return Err("is not a JSON object".to_string());
    }
    let file = serde_json::from_value::<BlockingMenusFile>(value)
        .map_err(|e| format!("does not parse: {e}"))?;
    if file.schema_version != BLOCKING_MENUS_SCHEMA_VERSION {
        return Err(format!(
            "has schemaVersion {}; only {} is supported",
            file.schema_version, BLOCKING_MENUS_SCHEMA_VERSION
        ));
    }
    Ok(file)
}

/// A parse failure is a build defect T1 catches; production logs once and serves an empty file.
pub fn shipped_blocking_menus() -> &'static BlockingMenusFile {
    static SHIPPED: OnceLock<BlockingMenusFile> = OnceLock::new();
    SHIPPED.get_or_init(|| {
        parse_blocking_menus_file(EMBEDDED_BLOCKING_MENUS_JSON).unwrap_or_else(|e| {
            log::error!("[blocking-menus] embedded settings-blocking-menus.json {e}");
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
    match parse_blocking_menus_file(&contents) {
        Ok(file) => file,
        Err(e) => {
            log::error!("[blocking-menus] {} {e}; ignoring the file", path.display());
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

    /// D3 layers 1 to 4: the two files only.
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

    /// D3 layer 0 first: an array still on the agent (settings.json not yet migrated, or an
    /// overlay-owned agents array) is what applied before #1905 and keeps applying unchanged.
    pub fn resolve_for(&self, agent: &AgentConfig) -> Vec<BlockingMenuEntry> {
        match &agent.blocking_menus {
            Some(entries) => entries.clone(),
            None => self.resolve(&agent.id, &agent.command),
        }
    }
}
```

`cargo fmt` rewraps the long lines; the log texts are binding.

2c. Tests: new module `blocking_menus_1905` as the LAST item of `mod tests` (after the closing
brace of `mod local_overlay_1737`), opening with `use super::super::*;` and `use serde_json::json;`
like `local_overlay_1737` (`:9695-9696`). Each test uses `tempfile::tempdir()` and a `settings.json`
path inside it; no settings file needs to exist. `super::agent_1757(id, command, blocking_menus)`
(`:9425`) builds every `AgentConfig` below.

- T1 `embedded_content_matches_the_rust_literals`: `shipped_blocking_menus()` has
  `schema_version == 1`, `note.is_some()`, empty `by_agent`, `by_command` keys exactly
  `["codex", "pi"]`; `by_command["pi"]` is one `Valid` entry with pattern
  `r"^\s*Trust project folder\?"`, `enabled`, `captured_against == Some("pi 0.52 / Windows")`;
  `by_command["codex"][0]` has pattern `r"^\s*Do you trust the contents of this directory\?"` and
  `captured_against == Some("codex 0.x / Linux")`;
  `by_command["codex"][1] == BlockingMenuEntry::Valid(codex_hooks_review_menu())`.
- T2 `resolve_precedence_is_legacy_then_local_agent_then_local_command_then_shipped`: `X`, `Y`,
  `Z` are three `Valid` entries with distinct patterns; local `by_agent = {"codex-b": [X], "pi-off": []}`,
  `by_command = {"codex": [Y]}`; assert `resolve("codex-b","codex") == [X]`, `resolve("codex-a","codex") == [Y]`,
  `resolve("codex-a", "Codex.exe --search") == [Y]` (directory-free on purpose: `file_stem` of a
  backslash path differs on Unix), `resolve("pi-off","pi")` empty, `resolve("claude-1","claude")` empty,
  `resolve("pi-1","pi") == default_blocking_menus_for_command("pi")`. Layer 0:
  `resolve_for(&agent_1757("codex-b", "codex", Some(vec![Z])))` is `[Z]` although `by_agent["codex-b"]`
  is `[X]`; `resolve_for(&agent_1757("pi-1", "pi", Some(vec![])))` is empty; `resolve_for(&agent_1757("codex-b", "codex", None)) == [X]`.
- T3 `shipped_file_is_written_when_absent_or_stale_and_left_alone_when_equal`: first `refresh`
  returns true and the bytes equal `pretty_json_bytes(shipped_blocking_menus())`; second returns
  false; append `x`; third returns true and the bytes equal canonical again.
- T4 `local_file_rejections_yield_an_empty_layer_and_match_the_parser`: for each text in
  `{ not json`, `[1]`, `[]`, `null`, `{"schemaVersion": 2}`, `{"schemaVersion":1,"byCommand":42,"byAgent":{}}`,
  `{"byAgent":{"codex":42}}`, `{"note":42,"byAgent":{}}`: `parse_blocking_menus_file` is `Err` and,
  written to the `.local` path, `load_local_blocking_menus_file` returns
  `BlockingMenusFile::default()`. Absent also returns the default. A valid file with one
  `byAgent` entry and an `"extra": true` key parses `Ok` and loads that entry.
  `{"byAgent":{"codex":[42]}}` parses `Ok` with one `Invalid(42)` entry (kept verbatim, as today).

Existing tests `:9230`, `:9331`, `:9371`, `:9505-9612` and `:10845` are unchanged in this phase.

## Edit 3: `src-tauri/src/config/instance_artifacts.rs`

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
`# AgentsCommander: operator-owned blocking-menu overlay; machine-local by design`. Shape: the
`settings.local.json` row at `:433-438`; `ignore_rows_are_unique_and_byte_sorted_by_name` (`:556`)
proves the position (`-` sorts before `.`).

## Edit 4: `src-tauri/src/config/instance_gitignore.rs`

In the fixture list, after `"settings.json.lock",` (`:1027`) add
`"settings-blocking-menus.json",` and `"settings-blocking-menus.local.json",`.
`HISTORICAL_FIXED_RULES` (`:511`) is untouched.

## Required behavior and failure behavior

- Guard evaluation is identical to base: nothing constructs the store outside tests, the loader
  still materializes, the guard still reads `agent.blocking_menus`. Green on `main` alone.
- Store, when built: absent or stale shipped file is written with one `info!`; unwritable
  directory or broken `.local` logs one `error!` and the embedded content serves.

## Verification (from `src-tauri`)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --lib -- blocking_menus_1905 instance_artifacts instance_gitignore
cargo test --lib --bins --tests
```

## Acceptance criteria

- AC1 `git diff --name-only HEAD~1..HEAD` lists exactly the five files above.
- AC2 `rg -n "parse_blocking_menus_file" src-tauri/src/config/settings.rs` prints 3 or more lines.
- AC3 `rg -n "include_str" src-tauri/src/config/settings.rs` prints exactly 1 line naming
  `settings-blocking-menus.json`.
- AC4 `node -e "const f=require('./src-tauri/resources/blocking-menus/settings-blocking-menus.json'); console.log(f.byCommand.pi[0].pattern, f.byCommand.codex[1].pattern)"`
  (repository root) prints `^\s*Trust project folder\? ^[^A-Za-z0-9]*Hooks need review\b`.
- AC5 The four verification commands exit 0; the filtered run reports the 4 new
  `blocking_menus_1905` tests passed and every pre-existing `instance_artifacts` and
  `instance_gitignore` test still passes.
- AC6 `rg -n "blocking_menus" src-tauri/src/pty/menu_guard/mod.rs src-tauri/src/lib.rs` prints
  the same lines as at base (`menu_guard/mod.rs:269,389,399,400,436,536`, `lib.rs:4601`): this
  phase does not touch the consumer.
- AC7 CI on the exact PR head: `rust-regression`, `rust-regression-linux`, `rust-regression-macos`,
  `rust-fmt`, `test-debt` and `frontend-regression` all green.
- AC8 Cycle gate. Working directory: the repository root (the directory holding `src-tauri/` and
  `scripts/`), clean tree at the phase head. `SCRATCH` is any directory outside the repository.

```
VAULT="../repo-personal/ObsidianVault/Coding Agents/IA-Programming/rust"
mkdir -p "$SCRATCH/base" && git archive 80aeb85 src-tauri | tar -x -C "$SCRATCH/base"
node "$VAULT/01-rust_module-dependency-cycles.mjs" "$SCRATCH/base/src-tauri" --write-baseline "$SCRATCH/pre-cycles.json" --quiet
node "$VAULT/01-rust_module-dependency-cycles.mjs" src-tauri --write-baseline "$SCRATCH/post-cycles.json" --quiet
node -e "const p=require('path');const [a,b]=process.argv.slice(1).map(f=>require(p.resolve(f)).moduleCycles.map(c=>[...c.members].sort().join('|')).sort());const same=JSON.stringify(a)===JSON.stringify(b);console.log(same?'SCC member sets identical':'SCC member sets differ');process.exit(same?0:1)" "$SCRATCH/pre-cycles.json" "$SCRATCH/post-cycles.json"
node "$VAULT/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph "$SCRATCH/post.json" --quiet
npm run record:arcs -- --graph "$SCRATCH/post.json"
git diff --stat src-tauri/module-arcs.txt
```

  Expected: the two `--write-baseline` runs exit 0 and each records exactly 1 module cycle with
  86 members (id `40d22fc71179d71f` at base); the comparison prints `SCC member sets identical`
  and exits 0; `--emit-graph` exits 1 (cycles exist; only exit 3 means no graph); `record:arcs`
  exits 0; the diff shows exactly one insertion, `agentscommander_lib::config::settings -> agentscommander_lib::config::local_config_io`,
  which this phase commits. Any other diff line or a `differ` verdict fails the gate.

## Preserve

`evaluate_logical_rows` signature and behavior; `BlockingMenuEntry` and `BlockingMenuConfig` serde
shapes; `CODEX_HOOKS_REVIEW_PATTERN` and `codex_hooks_review_menu`; `materialize_blocking_menus`
and `apply_issue_1757_migration` and their call sites (phase 3 owns them); the name and signature
of `default_blocking_menus_for_command`; `AgentConfig.blocking_menus`; `menu_guard_enabled`;
`ERR_MENU_GUARD_DEFERRED`; `menu_guard/mod.rs` and `lib.rs` in full (phase 2 owns them); every
`blocking_menus: None` constructor listed in the epic; `commands/session.rs:11119`.
