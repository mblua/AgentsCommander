# #1925 Phase 6: the startup download

Status: READY_FOR_IMPLEMENTATION

- Parent issue: https://github.com/mblua/AgentsCommander/issues/1925. Child issue: created by `ac-tech-lead-v4`; its number arrives in your Step 8 handoff.
- Branch: `feature/<child-issue>-rust-startup-download`, exactly as named in the Step 8 handoff, cut from `origin/main` at this phase's Step 8.
- Drift baseline: `main` = `b4f7c9de7f15b75f8f12c034c8356f73babd0a8d`, where this plan was verified. Every line number below is pinned to it; phases 1 and 2 have since moved lines in `settings.rs`, `instance_artifacts.rs` and `instance_gitignore.rs`, so find each anchor by its quoted text. It is not the branch point.
- Class: `design-bearing`. Owner: `ac-dev-rust-v4`.
- Depends on: phase 1 (validator, `blocking_menus_remote_path`, `load_remote_blocking_menus_file`, the store's remote arm, `BLOCKING_MENUS_REMOTE_FILE_NAME`), phase 2 (the flag), phase 3 (the served-path row and its CI check), phase 4 (the checkbox) and phase 5 (the `PRIVACY.md` entry). Parallel with: nothing.
- Why last: this phase starts a default-on startup request. Landing it before phase 4, 5 or 3 would ship a request the Settings UI cannot turn off, that `PRIVACY.md` does not disclose, or whose fetch literal the served-path check does not know.
- Contract changed: persistence, write side (the phase 1 cache file, unchanged in shape, and a new stamp file `blocking-menus-remote-check.json`). No IPC, CLI or settings-schema change.
- Delivery: commit on the phase branch and report the head SHA to `ac-tech-lead-v4`. Push only as the Step 8 handoff says. `ac-tech-lead-v4` opens and lands the PR and collects the exact-head CI evidence.

## 1. Objective

A detached startup task downloads the published blocking-menu file at most once per 24 hours, when `remoteBlockingMenusEnabled` is on. It validates the whole file on arrival and, only when every check passes, replaces the cache atomically. The download never blocks startup, never notifies, and takes effect at the next start.

## 2. Before any write (mandatory, from the repo root)

1. State the environment risk in writing to `ac-tech-lead-v4` before touching code. At minimum: the new tests open loopback TCP sockets on `127.0.0.1`. An `HTTP_PROXY`, `HTTPS_PROXY` or `ALL_PROXY` variable in your shell can route them through a proxy, so unset those variables or cover `127.0.0.1` in `NO_PROXY`; GitHub runners set none. A refused loopback connect takes about 2 s on Windows. The clone is shallow. The served-path check needs Node.
2. Branch and base:
   ```
   git fetch origin main
   git rev-parse --abbrev-ref HEAD
   git rev-parse HEAD origin/main
   git status --porcelain
   ```
   The branch equals the Step 8 name and matches `^feature/[1-9][0-9]*-rust-startup-download$`; both SHAs are equal; status prints nothing.
3. Landed dependencies, each must print as stated; otherwise stop, because this phase must not land before all five:
   ```
   grep -c 'fn load_remote_blocking_menus_file' src-tauri/src/config/settings.rs   # 1 (phase 1)
   grep -c 'pub remote_blocking_menus_enabled: bool' src-tauri/src/config/settings.rs   # 1 (phase 2)
   git ls-files scripts/check-served-paths.mjs remote-resources/SERVED-PATHS.md   # both paths (phase 3)
   grep -c 'settings.general.remoteBlockingMenusEnabled' src/sidebar/components/SettingsModal.tsx   # 1 (phase 4)
   grep -c 'remote-resources/blocking-menus/v1/settings-blocking-menus.json' PRIVACY.md   # at least 1 (phase 5)
   npm run check:served-paths   # served-paths: 2 rows, 1 literals, consistent
   ```
4. Drift:
   ```
   git cat-file -e "b4f7c9de7f15b75f8f12c034c8356f73babd0a8d^{commit}" || git fetch --deepen=200 origin main
   git diff --name-only b4f7c9de7f15b75f8f12c034c8356f73babd0a8d "$(git merge-base HEAD origin/main)"
   ```
   Expected entries: the files of phases 1 to 5 of #1925, which include `settings.rs`, `instance_artifacts.rs`, `instance_gitignore.rs`, `package.json` and `.github/workflows/pr-regression-gates.yml`. If the list names `src-tauri/src/update_check.rs`, `src-tauri/src/lib.rs`, `src-tauri/Cargo.toml`, `Cargo.lock`, `rustfmt.toml`, `.cargo/`, or any other workflow change, stop and report it.
5. Run the "pre" half of the dependency-cycle gate (section 7) on this clean tree.

## 3. Exact files and symbols

| File | Change |
|---|---|
| `src-tauri/src/config/settings.rs` | URL consts; response acceptance; cache writer; stamp path, reader and writer; `settings_path` visibility; tests |
| `src-tauri/src/config/instance_artifacts.rs` | stamp file-name const + one registry row |
| `src-tauri/src/config/instance_gitignore.rs` | one fixture path in a test |
| `src-tauri/src/update_check.rs` | throttle helper, download check, flag-reading shell, startup wrapper, tests |
| `src-tauri/src/lib.rs` | one spawn block |

No new module or `.rs` file, and no dependency change.

## 4. Decisions (binding for this phase)

- D1. Placement is forced by the dependency-cycle gate. The async shell lives in `update_check.rs`, which is already in the crate's one cyclic SCC and already references `config::settings` (`:189`) and `network` (`:262`). Every synchronous helper lives in `config::settings`. A new module called from `lib.rs` that calls into `config::settings` would join that SCC and fail the gate. `config::settings` gets no `tauri` or `AppHandle` use.
- D2. Source: `https://raw.githubusercontent.com/mblua/AgentsCommander/main/remote-resources/blocking-menus/v1/settings-blocking-menus.json`, as one single Rust string literal, with no `concat!` and no `format!`. The byte run `raw.githubusercontent.com/` appears exactly once in this phase's added lines: in that literal. No test, doc comment, log line or format string may contain it. Reason: CI's served-path scan (`npm run check:served-paths`, job `test-debt`) counts every such run under `src-tauri/`, test code included, and compares its ref and path exactly. A templated copy such as `.../{REMOTE_BLOCKING_MENUS_SOURCE_REF}/.../v{BLOCKING_MENUS_SCHEMA_VERSION}/...` is counted with no matching row and turns `test-debt` red; a regex run over that literal counted it with ref `{REMOTE_BLOCKING_MENUS_SOURCE_REF}`. The `v1` segment must equal `BLOCKING_MENUS_SCHEMA_VERSION`; test 1 pins that by comparing split segments. The parser already rejects a `schemaVersion` other than 1, which rejects a mismatch on arrival.
- D3. The flag `remote_blocking_menus_enabled` (phase 2) is read once, from `SettingsState`, in exactly one function, `run_remote_blocking_menus_for_app`. Production reaches it through `run_remote_blocking_menus_startup`, and test 9 calls it directly with a mock app. It gates only the download: a cache already on disk keeps applying when the flag is off.
- D4. Throttle: an on-disk stamp `blocking-menus-remote-check.json` (`{"lastCheckedAt": "<RFC 3339>"}`) next to `settings.json`. A download is due when the stamp is missing, unreadable, at least 24 h old, or in the future. The stamp is written after every attempt that passed the due check: accepted, rejected or unreachable. That makes "at most once per 24 h" literal and rules out a retry storm. A disabled or not-due run writes nothing.
- D5. The request goes through `OutboundNetwork::acquire` with label `update_check.remote_blocking_menus` and `general()`, with `User-Agent: agentscommander/<CARGO_PKG_VERSION>` (as `fetch_latest`, `update_check.rs:261-273`). One `tokio::time::timeout` of 10 s covers send and body read together. Accept only HTTP 200. Read the body with `Response::chunk()` and stop, rejecting, as soon as the total exceeds 64 KB (`64 * 1024` bytes).
- D6. Fail-silent: every non-accepted outcome logs at `debug` only. No toast, no event, no emit, no `UpdateCheckState` use. An accepted download logs one `info` line.
- D7. The cache is written with `crate::config::local_config_io::write_file_atomic`, a temp sibling then an atomic rename, only after validation passes. It is written with `pretty_json_bytes`, and its `note` is replaced by: `Downloaded by AgentsCommander from <url> (source ref main) at <RFC 3339 seconds, Z>. Replaced by the next accepted download and ignored at start if it fails validation; put your own patterns in settings-blocking-menus.local.json.` `<url>` is the runtime URL argument, never a second literal. A rejected or unreachable attempt never touches the cache.
- D8. Applied at the next start. The spawn goes after the existing update-check block (`lib.rs:3064-3072`). The store is built earlier (`lib.rs:2969-2971`) and never rebuilt, so this run never sees the new file.

## 5. Required behavior

### 5.1 `instance_artifacts.rs` and `instance_gitignore.rs`

- Add `/// #1925 - time of the last remote blocking-menu download attempt; throttles it to once per 24 h.` and `pub(crate) const BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME: &str = "blocking-menus-remote-check.json";` next to `BLOCKING_MENUS_REMOTE_FILE_NAME`.
- Add an `Ignore` `File` row for it with `comment: "# AgentsCommander: remote blocking-menu download throttle stamp"`. By byte order it goes after the `"app.log.*"` row and before the `CODEX_HOME_DIR_NAME` (`"codex-home"`) row; `ignore_rows_are_unique_and_byte_sorted_by_name` decides if it disagrees.
- In `git_fixture_ignores_exactly_required_paths_without_untracking`, add `"blocking-menus-remote-check.json",` to `required_paths` after `"app.log.5",` (`:999`) and before `"codex-home/agent-1/config.toml",` (`:1000`).

### 5.2 `settings.rs`

1. `fn settings_path()` (`:2393`) becomes `pub(crate) fn settings_path()`. The body is unchanged.
2. Add `BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME` to the `instance_artifacts` import.
3. Near the phase 1 consts add `pub(crate) const REMOTE_BLOCKING_MENUS_URL: &str =` followed by the D2 literal on the next line, `pub(crate) const REMOTE_BLOCKING_MENUS_SOURCE_REF: &str = "main";` and `pub(crate) const REMOTE_BLOCKING_MENUS_MAX_BYTES: usize = 64 * 1024;`.
4. `pub(crate) fn accept_remote_blocking_menus_response(status: u16, body: &[u8]) -> Result<BlockingMenusFile, String>`: a status other than 200 gives `HTTP status <n>`; a body over the max gives `body larger than 65536 bytes`; non-UTF-8 gives `body is not UTF-8`; otherwise it returns `validate_remote_blocking_menus_file(text)`.
5. `pub(crate) fn write_remote_blocking_menus_cache(settings_path: &Path, file: BlockingMenusFile, source_url: &str, fetched_at: chrono::DateTime<chrono::Utc>) -> Result<(), String>`: sets the D7 note (time via `to_rfc3339_opts(chrono::SecondsFormat::Secs, true)`), then `pretty_json_bytes`, then `write_file_atomic(&blocking_menus_remote_path(settings_path), ...)`.
6. `pub(crate) fn blocking_menus_remote_check_path(settings_path: &Path) -> PathBuf`; a private `#[derive(Serialize, Deserialize)] #[serde(rename_all = "camelCase")] struct RemoteBlockingMenusCheckStamp { last_checked_at: chrono::DateTime<chrono::Utc> }`; `pub(crate) fn read_remote_blocking_menus_check_stamp(settings_path: &Path) -> Option<chrono::DateTime<chrono::Utc>>`, where any read or parse failure gives `None`; and `pub(crate) fn write_remote_blocking_menus_check_stamp(settings_path: &Path, at: chrono::DateTime<chrono::Utc>) -> Result<(), String>`, using `pretty_json_bytes` and `write_file_atomic`.

### 5.3 `update_check.rs`

1. Module doc: append one paragraph, with no URL in it: `#1925 - the same detached, fail-silent startup shape also downloads the remote blocking-menu patterns (run_remote_blocking_menus_startup); that download never notifies and applies at the next start.`
2. `use std::path::PathBuf;` becomes `use std::path::{Path, PathBuf};`.
3. Extract `fn interval_elapsed(last: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool`, holding today's `should_check` logic (`:84-92`). `should_check` becomes `interval_elapsed(cache.as_ref().map(|c| c.last_checked_at), now)`. Put `interval_elapsed` directly after `should_check`, and keep `should_check`'s doc comment (`:82-83`) and signature line (`:84`) byte-identical: section 8 criterion 3 allows removed lines only at `:10` and inside `:84-92`. Existing tests stay unchanged and green.
4. `#[derive(Debug, Clone, PartialEq, Eq)] pub(crate) enum RemoteMenusCheck { Disabled, NotDue, Accepted, Rejected(String), Unreachable(String), WriteFailed(String) }`.
5. `pub(crate) async fn run_remote_blocking_menus_check(network: &crate::network::OutboundNetwork, settings_path: &Path, enabled: bool, url: &str, now: DateTime<Utc>) -> RemoteMenusCheck`, in this order:
   1. `!enabled` returns `Disabled`, before any disk or network access.
   2. `!interval_elapsed(read_remote_blocking_menus_check_stamp(settings_path), now)` returns `NotDue`.
   3. Acquire the D5 permit; an error becomes `Unreachable`. Inside the 10 s timeout: send; if the status is not 200, keep the status and an empty body; otherwise read chunks, stopping as soon as the total passes the max. Elapsed time becomes `Unreachable("timed out")`; a transport error becomes `Unreachable(<error>)`; an oversized body becomes `Rejected("body larger than 65536 bytes")`; anything else goes to `accept_remote_blocking_menus_response`, where `Err` becomes `Rejected(reason)`.
   4. If accepted, `write_remote_blocking_menus_cache(settings_path, file, url, now)`: `Ok` gives `Accepted`, `Err(e)` gives `WriteFailed(e)`.
   5. Always, once step 3 has started, `write_remote_blocking_menus_check_stamp(settings_path, now)`. A failure logs at debug and does not change the outcome.
6. `pub(crate) async fn run_remote_blocking_menus_for_app<R: tauri::Runtime>(app: &AppHandle<R>, settings_path: &Path, url: &str, now: DateTime<Utc>) -> RemoteMenusCheck`: read `remote_blocking_menus_enabled` once from `app.state::<crate::config::settings::SettingsState>()`, exactly as `run_startup_check` reads its flag (`:188-192`); take `app.state::<crate::network::OutboundNetwork>()`; return `run_remote_blocking_menus_check(&network, settings_path, enabled, url, now).await`. It reads no other setting.
7. `pub async fn run_remote_blocking_menus_startup(app: AppHandle)`: `crate::config::settings::settings_path()` returning `None` logs at debug and returns. Otherwise call `run_remote_blocking_menus_for_app(&app, &path, REMOTE_BLOCKING_MENUS_URL, Utc::now())`. `Accepted` logs `log::info!("[remote-menus] downloaded blocking-menu patterns; they apply at the next start")`; every other outcome logs `log::debug!("[remote-menus] {outcome:?}")`. Nothing else.

### 5.4 `lib.rs`

Directly after the update-check block (`:3064-3072`) add a block of the same shape: a comment `// #1925 - detached remote blocking-menu patterns download. Fail-silent; applies at the next start.`, then clone `app.handle()`, then `tauri::async_runtime::spawn(async move { crate::update_check::run_remote_blocking_menus_startup(handle).await; });`.

## 6. Tests

In the phase 1 module `remote_blocking_menus_1925` in `settings.rs`:

1. `remote_url_pins_the_schema_version_and_source_ref`. Compare split segments; never write the joined host and path (D2):
   ```rust
   let segments: Vec<&str> = REMOTE_BLOCKING_MENUS_URL.split('/').collect();
   assert_eq!(segments.len(), 10);
   assert_eq!(segments[..5], ["https:", "", "raw.githubusercontent.com", "mblua", "AgentsCommander"]);
   assert_eq!(segments[5], REMOTE_BLOCKING_MENUS_SOURCE_REF);
   assert_eq!(REMOTE_BLOCKING_MENUS_SOURCE_REF, "main");
   assert_eq!(segments[6..8], ["remote-resources", "blocking-menus"]);
   assert_eq!(segments[8], format!("v{BLOCKING_MENUS_SCHEMA_VERSION}"));
   assert_eq!(segments[9], "settings-blocking-menus.json");
   ```
   An equivalent form that clippy prefers is fine if it keeps the D2 rule.
2. `accept_response_checks_status_size_and_encoding`: 404 gives `HTTP status 404`; 500 gives `HTTP status 500`; 200 with 65537 bytes gives `larger than`; 200 with bytes `[0xff, 0xfe]` gives `not UTF-8`; 200 with the published file (`include_str!("../../../remote-resources/blocking-menus/v1/settings-blocking-menus.json")`) is `Ok`; 200 with a valid file padded with spaces to exactly 65536 bytes is `Ok`.
3. `cache_writer_records_source_time_and_ref`: write with `REMOTE_BLOCKING_MENUS_URL` as `source_url`, read the bytes; the note contains `REMOTE_BLOCKING_MENUS_URL` (the const, not a literal), `(source ref main)` and the RFC 3339 time; `load_remote_blocking_menus_file` returns the same `by_command`.
4. `check_stamp_round_trips_and_ignores_garbage`: a written stamp reads back equal; a garbage file and a missing file both read `None`.

In `update_check.rs`, a module `remote_blocking_menus_1925` as the last item inside `mod tests`, directly before its closing `}` (`:490`), using `#[tokio::test]`, `tempfile::TempDir` and these helpers:

- `test_app(settings: AppSettings) -> tauri::App<tauri::test::MockRuntime>`: the shape of `settings_app` in `src-tauri/src/commands/pty.rs:1091-1097` (`Arc::new(tokio::sync::RwLock::new(settings))` managed as `SettingsState`, `tauri::test::mock_builder()`, `mock_context(noop_assets())`), plus `.manage(crate::network::OutboundNetwork::new_for_tests(4))`. `pty.rs:1255-1257` already builds such an app inside `#[tokio::test]`.
- `serve_once(status: u16, body: Vec<u8>) -> String`: binds `tokio::net::TcpListener` on `127.0.0.1:0`, spawns a task that accepts one connection, reads until `\r\n\r\n` or EOF, writes `HTTP/1.1 <status> Status\r\nContent-Length: <n>\r\nConnection: close\r\n\r\n` followed by the body, then shuts the socket down. Every accept, read, write and shutdown error is ignored (`let _ =` or an early return) and the task never panics: in the 65537-byte case the client stops reading at the 64 KB cap and drops the socket, so the write can fail with a reset on Windows, and that must not fail or flake the test. It returns `http://127.0.0.1:<port>/remote-resources/blocking-menus/v1/settings-blocking-menus.json`.
- `offline_url()`: bind a listener, take its port, drop it, and return the same path on that port.
- The fixture file has `grok` = [G], pattern `^\s*Grok test menu\?`.

5. `disabled_makes_no_request_and_writes_nothing`: `run_remote_blocking_menus_check` with `enabled: false` gives `Disabled`; `acquired_labels_for_tests()` is empty; neither the stamp nor the cache exists.
6. `a_recent_stamp_makes_no_request`: stamp at now minus 1 h gives `NotDue` and no label. Stamp at now plus 1 h against `offline_url()` gives `Unreachable`, the label is recorded, and the stamp now equals `now`.
7. `an_accepted_download_is_cached_and_applies_at_the_next_start` (acceptance 1): first build store A with `load_from_settings_path`. Serve 200 with the fixture; the outcome is `Accepted`; the labels are exactly `["update_check.remote_blocking_menus"]`; the cache note carries URL, ref and time; the stamp equals `now`. Store A still resolves `grok` to `[]`. A store B loaded afterwards resolves `("grok-1", "grok")` to [G].
8. `every_failure_keeps_the_previous_cache` (acceptance 3), table-driven, a fresh tempdir per case, each seeded with a valid cache holding `grok` = [G1]: offline gives `Unreachable`; 404 gives `Rejected` containing `HTTP status 404`; `{` gives `does not parse`; a 65537-byte body gives `larger than`; an entry with pattern `.*` gives `matches the empty string`; a non-empty `byAgent` gives `byAgent must be empty`; `schemaVersion` 2 gives `schemaVersion`. For every case: the cache bytes are byte-equal to the seed, a loaded store resolves `grok` to [G1], and the stamp equals `now`.
9. `the_startup_path_reads_the_remote_flag_not_the_npm_flag` (acceptance 2, through the production flag read). Each case uses its own `test_app`, tempdir and `settings.json` path inside it, and calls `run_remote_blocking_menus_for_app(app.handle(), &path, &url, now)`:
   - Case A: `AppSettings { remote_blocking_menus_enabled: false, npm_update_notifications_enabled: true, ..AppSettings::default() }` with `url = offline_url()`. The outcome is `Disabled`; `app.state::<crate::network::OutboundNetwork>().acquired_labels_for_tests()` is empty; neither the stamp nor the cache exists.
   - Case B, the control: `remote_blocking_menus_enabled: true, npm_update_notifications_enabled: false` with `url = serve_once(200, fixture)`. The outcome is `Accepted`, and the labels are exactly `["update_check.remote_blocking_menus"]`.
   - Under any mutation the test still touches only the loopback port and the tempdir, never the real endpoint or the real config directory.

## 7. Verification (report each exit code and result line)

From `src-tauri`:

```
cargo test --locked --lib 1925 -- --test-threads=1
cargo test --locked --lib update_check
cargo test --locked --lib instance_
cargo test --locked --test instance_gitignore_layering
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo test --locked --lib --bins --tests
```

From the repo root:

```
npm run check:served-paths
git diff "$(git merge-base HEAD origin/main)" HEAD -- src-tauri/src | grep '^+' | grep -c 'raw.githubusercontent.com/'
git diff "$(git merge-base HEAD origin/main)" HEAD -- src-tauri/src/update_check.rs src-tauri/src/lib.rs | grep '^+' | grep -E 'emit|Emitter|toast|UpdateCheckState'
git diff --name-status "$(git merge-base HEAD origin/main)" HEAD -- src-tauri/src | grep '^A'
grep -c 'run_remote_blocking_menus_startup' src-tauri/src/lib.rs
```

- `1925`: `test result: ok.` with at least 17 passed (7 from phase 1, 1 from phase 2, 9 here) and 0 failed; every test name from this phase appears.
- `check:served-paths`: exit 0 and exactly `served-paths: 2 rows, 2 literals, consistent`. This is the B1 proof: the pin test adds no counted literal.
- The `grep -c 'raw.githubusercontent.com/'` line prints `1`.
- The `emit` line and the `^A` line print nothing. The `lib.rs` count prints `1`.
- Mutation proof for test 9, run once locally and reverted: (a) in `run_remote_blocking_menus_for_app`, replace the flag read with `true`: case A fails; (b) read `npm_update_notifications_enabled` instead: test 9 fails; (c) replace the flag read with `false`: case B fails. Report the failing assertion line of each run, then show `git diff -- src-tauri/src/update_check.rs` holds none of the three mutations.

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

Green only if every `cmp` prints nothing and exits 0. If the pre `cmp` fails, the committed record on `main` is stale: stop and report it before writing. Any post failure is a failed gate: stop and report it.

## 8. Acceptance criteria

1. Everything in section 7 passes as stated.
2. `git diff --name-only "$(git merge-base HEAD origin/main)" HEAD` lists exactly the 5 paths of section 3 (ignore `plans/`). `src-tauri/module-arcs.txt`, `src-tauri/Cargo.toml` and `Cargo.lock` are unchanged.
3. The existing `update_check` tests are byte-identical, and no line of `update_check.rs` is removed except those the section 5.3 edit replaces. Section 2 step 4 stops when `update_check.rs` drifted, so the drift-baseline line numbers hold at the branch point. From the repo root, with `SCRATCH` as in section 7:
   ```
   BASE="$(git merge-base HEAD origin/main)"
   git show "$BASE:src-tauri/src/update_check.rs" | sed -n '/^mod tests {$/,$p' | sed '$d' > "$SCRATCH/tests-base.txt"
   wc -l < "$SCRATCH/tests-base.txt"
   git show "HEAD:src-tauri/src/update_check.rs" | sed -n '/^mod tests {$/,$p' | head -n "$(wc -l < "$SCRATCH/tests-base.txt")" | cmp - "$SCRATCH/tests-base.txt"
   git diff -U0 "$BASE" HEAD -- src-tauri/src/update_check.rs | awk '/^@@ /{split(substr($2,2),a,","); s=a[1]+0; n=(a[2]=="")?1:a[2]+0; if(n>0 && !(s==10 && n==1) && !(s>=84 && s+n-1<=92)) print}'
   ```
   - `wc -l` prints `134`: base `mod tests {` (`:356`) through its last test, without the closing `}` (`:490`).
   - `cmp` prints nothing and exits 0: every existing test line is unchanged and the new submodule comes after all of them. Editing an existing test, or putting the submodule anywhere else, fails it.
   - `awk` prints nothing: every removed line is `use std::path::PathBuf;` (`:10`) or inside `should_check` (`:84-92`). Deleting or editing any other existing line prints its hunk header.

## 9. Preserve

- `run_startup_check`, `fetch_latest`, `cli_notice` and `read_cached_notice` behave exactly as before; `should_check` keeps its results.
- `fetch_home_markdown` and `HOME_MARKDOWN_URL` (`commands/config.rs:36-37`) are untouched.
- `default_blocking_menus_for_command` still reads shipped content only.
- Startup never waits on the download: the spawn is detached and nothing awaits it.

## 10. Recovery

Restore only paths this phase changed that still hold this run's output (`git restore --source=HEAD -- <path>`). No `git reset`, no `git clean`, no repository-wide restore. Revert every mutation-proof edit before committing. Report external changes to `ac-tech-lead-v4`.
