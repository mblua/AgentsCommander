# #1925 Phase 1: the `remote` precedence layer (read side)

Status: READY_FOR_IMPLEMENTATION

- Parent issue: https://github.com/mblua/AgentsCommander/issues/1925. Child issue: created by `ac-tech-lead-v4`; its number arrives in your Step 8 handoff.
- Branch: `feature/<child-issue>-rust-remote-layer`, exactly as named in the Step 8 handoff, cut from `origin/main` at this phase's Step 8.
- Drift baseline: `main` = `b4f7c9de7f15b75f8f12c034c8356f73babd0a8d`, where this plan was verified. Every line number below is pinned to it. It is not the branch point.
- Class: `design-bearing`. Owner: `ac-dev-rust-v4`.
- Depends on: nothing. Parallel with: nothing. Phases 2, 3 and 6 depend on this phase.
- Contract changed: persistence (a new read-only instance file, `settings-blocking-menus.remote.json`). No IPC, CLI or settings-schema change.
- Delivery: commit on the phase branch and report the head SHA to `ac-tech-lead-v4`. Push only as the Step 8 handoff says. `ac-tech-lead-v4` opens and lands the PR and collects the exact-head CI evidence.

## 1. Objective

Menu-guard patterns can come from a downloaded file. This phase adds the part that reads it: a whole-file validator, a cache reader that re-validates on every read, a new `remote` arm in `BlockingMenusStore::resolve`, the first published copy of the file, and the registry row for the new instance file. Nothing downloads yet (phase 6 does).

Landing this phase alone ships nothing visible: no install has the cache file, no request exists, and the only fetch literal in source is still the Home one.

## 2. Before any write (mandatory, from the repo root)

1. State the environment risk in writing to `ac-tech-lead-v4` before touching code: Windows host; cargo target dir is `<repo>/target`; the clone is shallow (`git rev-parse --is-shallow-repository` = `true`), so nothing below may depend on history older than the drift baseline; any other local risk you see.
2. Branch and base:
   ```
   git fetch origin main
   git rev-parse --abbrev-ref HEAD
   git rev-parse HEAD origin/main
   git status --porcelain
   ```
   The branch equals the Step 8 name and matches `^feature/[1-9][0-9]*-rust-remote-layer$`; both SHAs are equal (no commit yet); status prints nothing. From here on, `BASE` means `$(git merge-base HEAD origin/main)`.
3. Landed dependencies: none.
4. Drift:
   ```
   git cat-file -e "b4f7c9de7f15b75f8f12c034c8356f73babd0a8d^{commit}" || git fetch --deepen=200 origin main
   git diff --name-only b4f7c9de7f15b75f8f12c034c8356f73babd0a8d "$(git merge-base HEAD origin/main)"
   ```
   If that list names a file in section 3, `src-tauri/Cargo.toml`, `Cargo.lock`, `rustfmt.toml`, `.cargo/` or `.github/workflows/`, stop and report it to `ac-tech-lead-v4`. Otherwise record the list and continue. If a section 3 file moved, find each anchor by its quoted symbol or text.
5. Run the "pre" half of the dependency-cycle gate (section 7) on this clean tree.

## 3. Exact files and symbols

| File | Change |
|---|---|
| `src-tauri/src/config/instance_artifacts.rs` | new const + one registry row |
| `src-tauri/src/config/instance_gitignore.rs` | one fixture path in a test |
| `src-tauri/src/config/settings.rs` | consts, validator, path helper, cache reader, store arm, tests |
| `src-tauri/src/pty/menu_guard/mod.rs` | one test only |
| `remote-resources/blocking-menus/v1/settings-blocking-menus.json` | new file |

No other file changes. Do not add a module or a `.rs` file (D8).

## 4. Decisions (binding for this phase)

- D1. Layer order, first hit wins, the hit replaces every layer below it whole, no merging: (0) the agent's legacy `blockingMenus` array (`resolve_for`), (1) `local.byAgent[<agent id>]`, (2) `local.byCommand[<stem>]`, (3) `remote.byCommand[<stem>]` (new), (4) `shipped.byCommand[<stem>]`. A remote `[]` for a stem therefore retracts the shipped patterns for that stem. Do not enforce "remote is a superset of shipped".
- D2. `remote.byAgent` is never consulted: the validator rejects any file whose `byAgent` is not empty.
- D3. `default_blocking_menus_for_command` (`settings.rs:1113`) keeps reading `shipped_blocking_menus()` only. So do the #1905 export migration and `refresh_shipped_blocking_menus_file`.
- D4. Any failed check rejects the whole file, never one entry. On read, a rejected or unreadable cache yields an empty remote layer (the shipped layer then applies) and one `log::warn!` line. A missing cache is silent. The file on disk is left in place.
- D5. The cache is re-validated at every read, not trusted because a writer validated it: it sits in a directory the local user can edit.
- D6. Limits: at most 200 entries summed over every `byCommand` array (disabled and unreadable entries count); `pattern` at most 512 bytes; `notification` at most 200 bytes and no character for which `char::is_control` is true (this covers `\n`, `\r`, `\t`); every pattern must build with `regex::RegexBuilder::new(pattern).size_limit(256 * 1024).build()`. Every entry is checked, `enabled: false` included. A `BlockingMenuEntry::Invalid` entry rejects the file. Memory ceiling this implies: the menu guard keeps one compiled copy per pattern, so an accepted file adds at most 200 x 256 KiB = 50 MiB of compiled programs, plus each regex's lazy-DFA cache at the `regex` crate default. Reaching it needs repo-write access; the real file compiles to a few KiB.
- D7. Catch-all check: a pattern that matches `""` rejects the file, and so does a pattern that matches any row of this corpus, embedded in the binary exactly as listed:

```rust
const REMOTE_BLOCKING_MENUS_BENIGN_ROWS: &[&str] = &[
    "",
    "   ",
    "$ ",
    "> ",
    r"PS C:\Users\dev\project> ",
    "dev@host:~/project$ ",
    "The quick brown fox jumps over the lazy dog.",
    "    at main (src/index.ts:10:5)",
    "thread 'main' panicked at src/main.rs:2:5:",
    "Traceback (most recent call last):",
    "error: could not compile `app` (bin \"app\") due to 1 previous error",
    "\u{256d}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{256e}",
    "\u{2502}                \u{2502}",
];
```

  Matching is `Regex::is_match` on each row, unanchored, the way the guard matches. Do not add menu-like rows such as `1. Yes`: a legitimate trust-menu pattern may match them.
- D8. Placement is forced by the dependency-cycle gate: every symbol goes into `config::settings`, which is already in the crate's one cyclic SCC and already references `config::instance_artifacts`. `regex` is an external crate, not a module arc. A new module would join the SCC and fail the gate.

## 5. Required behavior

### 5.1 `instance_artifacts.rs`

- After `BLOCKING_MENUS_LOCAL_FILE_NAME` (`:157`) add:
  `/// #1925 - blocking-menu patterns downloaded from GitHub; written only by the startup download.`
  `pub(crate) const BLOCKING_MENUS_REMOTE_FILE_NAME: &str = "settings-blocking-menus.remote.json";`
- In `INSTANCE_ARTIFACTS`, directly after the `BLOCKING_MENUS_LOCAL_FILE_NAME` row (`:431-436`) and before the `"settings.json"` row, add a row: `name: BLOCKING_MENUS_REMOTE_FILE_NAME`, `kind: ArtifactKind::File`, `disposition: Disposition::Ignore`, `comment: "# AgentsCommander: downloaded blocking-menu patterns; replaced by the next accepted download"`. `ignore_rows_are_unique_and_byte_sorted_by_name` decides the position; follow it if it disagrees.

### 5.2 `instance_gitignore.rs`

In `git_fixture_ignores_exactly_required_paths_without_untracking`, add `"settings-blocking-menus.remote.json",` to `required_paths` right after `"settings-blocking-menus.local.json",` (`:1029`).

### 5.3 `settings.rs`

1. Add `BLOCKING_MENUS_REMOTE_FILE_NAME` to the `use crate::config::instance_artifacts::{...}` list (`:10-12`).
2. After `BLOCKING_MENUS_SCHEMA_VERSION` (`:1051`) add private consts `REMOTE_BLOCKING_MENUS_MAX_ENTRIES: usize = 200`, `REMOTE_PATTERN_MAX_BYTES: usize = 512`, `REMOTE_NOTIFICATION_MAX_BYTES: usize = 200`, `REMOTE_PATTERN_REGEX_SIZE_LIMIT: usize = 256 * 1024`, and the corpus from D7. Doc-comment the size limit with the D6 memory ceiling.
3. After `parse_blocking_menus_file` (`:1084-1099`) add `pub(crate) fn validate_remote_blocking_menus_file(contents: &str) -> Result<BlockingMenusFile, String>`. Check in this order and return the first failure. The quoted text must appear in the `Err` string, because the tests assert it:
   1. `parse_blocking_menus_file(contents)`, returning its own error unchanged (`does not parse`, `is not a JSON object`, `schemaVersion`).
   2. `byAgent must be empty`.
   3. `more than 200 entries`.
   4. For each `(stem, index)` in `by_command` order: `is not a valid blocking-menu entry`; `pattern longer than 512 bytes`; `notification longer than 200 bytes`; `notification contains a control character`; `pattern does not compile: <regex error>`; `pattern matches the empty string`; `pattern matches a benign row`. Name the stem and index in every entry message, for example `entry codex[1]: pattern matches the empty string`.
4. After `blocking_menus_local_path` (`:1129-1131`) add `pub(crate) fn blocking_menus_remote_path(settings_path: &Path) -> PathBuf`, using `settings_path.with_file_name(BLOCKING_MENUS_REMOTE_FILE_NAME)`.
5. After `load_local_blocking_menus_file` (`:1165-1182`) add `pub(crate) fn load_remote_blocking_menus_file(settings_path: &Path) -> BlockingMenusFile`, shaped like the local loader: `NotFound` returns the default silently; any other read error logs `log::warn!("[blocking-menus] could not read {}: {e}", ...)` and returns the default; a validation error logs `log::warn!("[blocking-menus] {} {e}; ignoring the downloaded patterns", ...)` and returns the default.
6. `BlockingMenusStore` (`:1184-1243`):
   - Add the field `remote: BlockingMenusFile` between `shipped` and `local`.
   - Add `pub(crate) fn with_layers(remote: BlockingMenusFile, local: BlockingMenusFile) -> Self`. Make `with_local(local)` call `Self::with_layers(BlockingMenusFile::default(), local)`. `shipped_only()` keeps its body, so its remote layer is empty.
   - `load_from_settings_path` keeps `refresh_shipped_blocking_menus_file(settings_path)` first, then returns `Self::with_layers(load_remote_blocking_menus_file(settings_path), load_local_blocking_menus_file(settings_path))`.
   - `resolve`: after the `self.local.by_command` hit, add `if let Some(entries) = self.remote.by_command.get(&stem) { return entries.clone(); }` before the shipped fallback. Update the doc comments: "D3 layers 1 to 4" becomes layers 1 to 5 with remote named, and the store doc keeps "Built once per process; no file watcher".

### 5.4 `remote-resources/blocking-menus/v1/settings-blocking-menus.json`

New file, LF line endings, one trailing newline, two-space indent. Copy the shipped file `src-tauri/resources/blocking-menus/settings-blocking-menus.json` exactly, changing only the `note` value to:

`Published from main to every installed AgentsCommander that downloads blocking-menu patterns. Generate it from src-tauri/resources/blocking-menus/settings-blocking-menus.json plus additions: for every stem named here, this array replaces the shipped array whole. Installed copies replace this note with their download record.`

It keeps all 3 shipped entries (`codex` x2, `pi` x1) and `"byAgent": {}`. It adds no new pattern.

## 6. Tests

In `settings.rs`, add a module `remote_blocking_menus_1925` inside the tests module, as a sibling directly after `mod blocking_menus_1905` (`:11461`). Use `tempfile::TempDir` and write fixture files next to a `settings.json` path, as `blocking_menus_1905` does. Phases 2 and 6 add tests to this module later.

1. `the_published_remote_file_passes_the_validator`: `include_str!("../../../remote-resources/blocking-menus/v1/settings-blocking-menus.json")` validates `Ok`, and its `by_agent` is empty. This is the CI check over the published bytes: `rust-regression` runs it on every pull request.
2. `the_shipped_patterns_pass_the_validator`: `EMBEDDED_BLOCKING_MENUS_JSON` validates `Ok`.
3. `every_rejection_names_its_check`: a table of `(label, json, expected substring)`, each `Err` containing its substring. Cases: `{`; `[]`; `schemaVersion` 2; non-empty `byAgent`; 201 entries; an entry `{"pattern": 5}`; a 513-byte pattern; a 201-byte notification; a pattern of 257 `é` (U+00E9, 2 bytes each: 257 characters, 514 bytes) expecting `pattern longer than 512 bytes`; a notification of 101 `é` (101 characters, 202 bytes) expecting `notification longer than 200 bytes` (a check that counts characters accepts both, so these two fail it); notification `"a\nb"`; notification containing `\u{7}`; pattern `(`; a pattern of at most 512 bytes whose error text contains `size limit` (for example `\w{1000}`; if it compiles, pick another and assert the substring); patterns `^`, `.*`, `\s*` (empty string); patterns `.`, `\w+`, `^\s*>`, `Traceback` (benign row). Also one accepted case, `^\s*Grok test menu\?`, with a notification of exactly 200 bytes and a pattern of exactly 512 bytes, to pin both boundaries as accepted.
4. `the_cache_is_revalidated_on_every_read`: write a valid cache with `grok` = [G]; a store from `load_from_settings_path` resolves `("grok-1", "grok")` to [G]. Overwrite the cache with a non-empty `byAgent`; a new store resolves `grok` to `[]` and `codex` to the shipped 2 entries. Delete the file; a new store resolves `grok` to `[]`.
5. `precedence_is_local_then_remote_then_shipped`, built with `with_layers`: remote `codex` = [R] gives `codex` -> [R]; remote `grok` = [G] gives `grok` -> [G], while the control `shipped_only()` gives `[]`; local `byCommand.grok` = [L] gives [L]; local `byAgent["grok-1"]` = [A] gives [A]; remote `codex` = `[]` gives `[]`; a remote file without `pi` gives the shipped `pi` entry; `resolve_for` with `blocking_menus: Some(vec![X])` gives [X].
6. `default_blocking_menus_for_command_ignores_the_remote_cache`: with a cache holding `codex` = [R] loaded into a store, `default_blocking_menus_for_command("codex")` equals `shipped_blocking_menus().by_command["codex"]` and has 2 entries.

In `pty/menu_guard/mod.rs` tests, next to `a_custom_pattern_on_disk_reaches_the_evaluator_through_the_production_store`:

7. `a_remote_pattern_in_the_cache_reaches_the_evaluator_1925`: write `settings-blocking-menus.remote.json` with `grok` = one entry, pattern `^\s*Grok test menu\?` and notification `grok is waiting for you to answer the test menu in this terminal`. Build `MenuGuard::with_store(BlockingMenusStore::load_from_settings_path(&dir.join("settings.json")))`. `entries_for(&agent("grok-1", "grok", None))` has 1 entry; `evaluate_logical_rows` on a row `Grok test menu?` has `is_blocked` true and `matched_notification` equal to that text. Control: `MenuGuard::new()` gives no entries and is not blocked.

## 7. Verification (report each exit code and result line)

From `src-tauri`:

```
cargo test --locked --lib 1925 -- --test-threads=1
cargo test --locked --lib blocking_menus
cargo test --locked --lib instance_
cargo test --locked --test instance_gitignore_layering
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

- The first command prints `test result: ok. 7 passed; 0 failed` or a larger passed count, and each of the 7 test names appears in its output.
- Optional locally, authoritative in CI: `cargo test --locked --lib --bins --tests`.

Dependency-cycle gate, from the repo root, with `VAULT` = `D:/0_repos/AgentsCommander_iac/.ac/room-5-ac-dev-team-v4/repo-personal/ObsidianVault/Coding Agents/IA-Programming/rust` and `SCRATCH` = any directory outside the repo. "Pre" runs on the clean branch point (section 2 step 5); "post" runs on the finished, committed tree.

```
# pre
node "$VAULT/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph "$SCRATCH/pre.json" --quiet   # exit 1 is normal; exit 3 means no graph
node scripts/02-module-arc-record.mjs --graph "$SCRATCH/pre.json" --out "$SCRATCH/pre-arcs.txt"
cmp "$SCRATCH/pre-arcs.txt" src-tauri/module-arcs.txt
node "$SCRATCH/sccdigest.cjs" "$SCRATCH/pre.json" > "$SCRATCH/pre-digest.txt"
# post
node "$VAULT/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph "$SCRATCH/post.json" --quiet
node scripts/02-module-arc-record.mjs --graph "$SCRATCH/post.json" --out "$SCRATCH/post-arcs.txt"
cmp "$SCRATCH/post-arcs.txt" src-tauri/module-arcs.txt
node "$SCRATCH/sccdigest.cjs" "$SCRATCH/post.json" > "$SCRATCH/post-digest.txt"
cmp "$SCRATCH/pre-digest.txt" "$SCRATCH/post-digest.txt"
```

`sccdigest.cjs`:

```js
const fs=require("fs"),crypto=require("crypto");
const g=JSON.parse(fs.readFileSync(process.argv[2],"utf8"));
const ids=g.modules.map(m=>m.id),adj=new Map(ids.map(i=>[i,new Set()]));
for(const e of g.edges) if(e.from!==e.to&&adj.has(e.from)&&adj.has(e.to)) adj.get(e.from).add(e.to);
let n=0;const st=[],on=new Set(),I=new Map(),L=new Map(),out=[];
function f(v){I.set(v,n);L.set(v,n);n++;st.push(v);on.add(v);for(const w of adj.get(v)){if(!I.has(w)){f(w);L.set(v,Math.min(L.get(v),L.get(w)));}else if(on.has(w))L.set(v,Math.min(L.get(v),I.get(w)));}
 if(L.get(v)===I.get(v)){const c=[];let w;do{w=st.pop();on.delete(w);c.push(w);}while(w!==v);if(c.length>1)out.push(c.sort().join("\n")+"\n");}}
for(const v of ids) if(!I.has(v)) f(v);
out.sort();
console.log(`modules=${ids.length} cyclicSccs=${out.length} sizes=${out.map(s=>s.split("\n").length-1).join(",")}`);
out.forEach((s,i)=>console.log(`scc${i}-sha256=${crypto.createHash("sha256").update(s).digest("hex").toUpperCase()}`));
```

Green only if every `cmp` prints nothing and exits 0. At the drift baseline the digest is `modules=199 cyclicSccs=1 sizes=86` and `scc0-sha256=5572EC179B7E7DAA97E5572FB2CD7D9337F7CA1BA645DB0C3CAD24CC2259FAB5`; report your pre digest either way. If the pre `cmp` fails, the committed record on `main` is stale: stop and report it before writing. Any post failure is a failed gate: stop and report it.

## 8. Acceptance criteria

1. Every command in section 7 passes as stated.
2. `git diff --name-only "$(git merge-base HEAD origin/main)" HEAD` lists exactly the 5 paths of section 3 (ignore `plans/`). `git diff --name-status "$(git merge-base HEAD origin/main)" HEAD -- src-tauri/src` shows no `A` line.
3. `git diff "$(git merge-base HEAD origin/main)" HEAD -- src-tauri/src/config/settings.rs` shows no change inside `default_blocking_menus_for_command`, `refresh_shipped_blocking_menus_file`, `load_local_blocking_menus_file`, `export_blocking_menus_to_local_file` or `apply_issue_1757_migration`.
4. `src-tauri/module-arcs.txt` is unchanged.
5. `git grep -n 'raw.githubusercontent.com/' -- src-tauri/src src` lists the same lines as on the branch point: this phase adds no fetch literal.

## 9. Preserve

- `parse_blocking_menus_file` stays the only parser; the validator calls it and adds checks.
- `.local` rejection behavior and log lines are unchanged.
- `MenuGuard::default()` still uses `BlockingMenusStore::shipped_only()`.
- No file watcher and no hot swap: the store is still built once per process.

## 10. Recovery

On failure, restore only paths this phase changed, and only while they still hold this run's own output: `git restore --source=HEAD -- <path>` for modified files, and deletion of the new JSON file if this run created it. Never use `git reset`, `git clean` or a repository-wide restore. Report to `ac-tech-lead-v4` any path that changed outside this run.
