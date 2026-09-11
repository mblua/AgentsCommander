# #1925 Phase 2: the `remoteBlockingMenusEnabled` settings flag

Status: READY_FOR_IMPLEMENTATION

- Parent issue: https://github.com/mblua/AgentsCommander/issues/1925. Child issue: created by `ac-tech-lead-v4`; its number arrives in your Step 8 handoff.
- Branch: `feature/<child-issue>-rust-settings-flag`, exactly as named in the Step 8 handoff, cut from `origin/main` at this phase's Step 8.
- Drift baseline: `main` = `b4f7c9de7f15b75f8f12c034c8356f73babd0a8d`, where this plan was verified. Every line number below is pinned to it; phase 1 has since moved lines in `settings.rs`, so find each anchor by its quoted text. It is not the branch point.
- Class: `patterned`. It mirrors `npm_update_notifications_enabled` (`settings.rs:621-624`, default `:1018`). Owner: `ac-dev-rust-v4`.
- Depends on: phase 1 (the test module `remote_blocking_menus_1925` in the same file). Parallel with: phase 3. Phases 4, 5 and 6 depend on this phase.
- Contract changed: settings schema (one new `AppSettings` field). Its UI consumer is phase 4; its only reader is phase 6.
- Delivery: commit on the phase branch and report the head SHA to `ac-tech-lead-v4`. Push only as the Step 8 handoff says. `ac-tech-lead-v4` opens and lands the PR and collects the exact-head CI evidence.

## 1. Objective

Add the flag that the Settings checkbox (phase 4) and the startup download (phase 6) both need. The repo owner decided it defaults to `true`.

Landing this phase alone ships no request, no new file and no UI: nothing reads the flag yet. The only visible change is one more key, `"remoteBlockingMenusEnabled": true`, when AC next writes `settings.json`.

## 2. Before any write (mandatory, from the repo root)

1. State the environment risk in writing to `ac-tech-lead-v4` before touching code: Windows host; cargo target dir is `<repo>/target`; shallow clone; any other local risk you see.
2. Branch and base:
   ```
   git fetch origin main
   git rev-parse --abbrev-ref HEAD
   git rev-parse HEAD origin/main
   git status --porcelain
   ```
   The branch equals the Step 8 name and matches `^feature/[1-9][0-9]*-rust-settings-flag$`; both SHAs are equal; status prints nothing.
3. Landed dependency (phase 1), each must print as stated:
   ```
   git ls-files remote-resources/blocking-menus/v1/settings-blocking-menus.json   # that path
   grep -c 'fn validate_remote_blocking_menus_file' src-tauri/src/config/settings.rs   # 1
   grep -c 'mod remote_blocking_menus_1925' src-tauri/src/config/settings.rs   # 1
   ```
   Otherwise stop: phase 1 has not landed.
4. Drift:
   ```
   git cat-file -e "b4f7c9de7f15b75f8f12c034c8356f73babd0a8d^{commit}" || git fetch --deepen=200 origin main
   git diff --name-only b4f7c9de7f15b75f8f12c034c8356f73babd0a8d "$(git merge-base HEAD origin/main)"
   ```
   Expected entries: phase 1's five files (`src-tauri/src/config/instance_artifacts.rs`, `src-tauri/src/config/instance_gitignore.rs`, `src-tauri/src/config/settings.rs`, `src-tauri/src/pty/menu_guard/mod.rs`, `remote-resources/blocking-menus/v1/settings-blocking-menus.json`) and, if phase 3 landed, its five (`remote-resources/SERVED-PATHS.md`, `.github/CODEOWNERS`, `scripts/check-served-paths.mjs`, `package.json`, `.github/workflows/pr-regression-gates.yml`). If the list names any other path under `src-tauri/`, or `Cargo.lock`, `rustfmt.toml`, `.cargo/`, or a `.github/workflows/` change that phase 3 did not make, stop and report it. Otherwise record the list and continue.
5. Run the "pre" half of the dependency-cycle gate (section 7) on this clean tree.

## 3. Exact files and symbols

| File | Change |
|---|---|
| `src-tauri/src/config/settings.rs` | field and doc comment; `Default` value; one line in the S6 golden fixture; one test |

No other file changes. Verified at the drift baseline: every other `AppSettings` struct literal uses functional update (`..`), so no other file needs the field. If the compiler disagrees, stop and report the path.

## 4. Decisions (binding for this phase)

- D1. Field `remote_blocking_menus_enabled: bool`, `#[serde(default = "default_true")]`, serialized `remoteBlockingMenusEnabled` (`AppSettings` is `rename_all = "camelCase"`), default `true`. Separate from `npm_update_notifications_enabled`: different endpoint, different data.
- D2. Nothing reads the flag in this phase. Do not add a reader, a request, a registry row or a `settings_path` visibility change: phase 6 does that.
- D3. When phase 6 lands, the flag gates only the download, read once at startup; a file already downloaded keeps applying. The doc comment says so.
- D4. No module reference is added.

## 5. Required behavior

1. After `npm_update_notifications_enabled` (`:622-624`) add:
   ```rust
       /// #1925 When true, download the remote blocking-menu patterns on startup
       /// (<=1x/24h); a downloaded file applies at the next start. Default true.
       #[serde(default = "default_true")]
       pub remote_blocking_menus_enabled: bool,
   ```
2. In `impl Default for AppSettings`, directly after `npm_update_notifications_enabled: true,` (`:1018`), add `remote_blocking_menus_enabled: true,`.
3. The S6 golden fixture `EXPECTED_NON_PROJECT_SETTINGS_JSON` (`:10244-10392`) is compared by `a_no_overlay_save_writes_the_control_captured_on_the_pinned_base` (`:10421`), whose normalizer sorts keys by bytes (`s6_normalized_non_project_settings`, `:10394-10417`). Insert the line `  "remoteBlockingMenusEnabled": true,` directly after `  "raiseTerminalOnClick": true,` (`:10341`) and before `  "resourceBackoffPolling": true,` (`:10342`), with the same two-space indent. Change nothing else in the fixture or its doc comment.

## 6. Tests

In the phase 1 module `remote_blocking_menus_1925` in `settings.rs`:

1. `remote_flag_defaults_true_and_round_trips`:
   - `AppSettings::default().remote_blocking_menus_enabled` is `true`.
   - `serde_json::to_value(AppSettings::default())` holds `"remoteBlockingMenusEnabled": true`.
   - That value with the key removed deserializes to `true`; with the key set to `false` it deserializes to `false`.
   - Control: that value with `npmUpdateNotificationsEnabled` set to `false` and the new key removed still deserializes `remote_blocking_menus_enabled` to `true`, so the two flags are independent.

## 7. Verification (report each exit code and result line)

From `src-tauri`:

```
cargo test --locked --lib 1925 -- --test-threads=1
cargo test --locked --lib a_no_overlay_save_writes_the_control_captured_on_the_pinned_base
cargo test --locked --lib settings
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

- `1925`: `test result: ok.` with at least 8 passed (7 from phase 1 plus this one) and 0 failed; the new name appears.
- S6 negative control: run the S6 test once after steps 1 and 2 of section 5 and before step 3. It must fail with a diff that names `remoteBlockingMenusEnabled`. After step 3 it passes with 1 passed. Report both runs.
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

Green only if every `cmp` prints nothing and exits 0. If the pre `cmp` fails, the committed record on `main` is stale: stop and report it before writing. Any post failure is a failed gate: stop and report it.

## 8. Acceptance criteria

1. Everything in section 7 holds.
2. `git diff --name-only "$(git merge-base HEAD origin/main)" HEAD` lists exactly `src-tauri/src/config/settings.rs` (ignore `plans/`).
3. `git diff "$(git merge-base HEAD origin/main)" HEAD -- src-tauri/src/config/settings.rs | grep '^-' | grep -v '^---'` prints nothing: this phase only adds lines.
4. `src-tauri/module-arcs.txt` is unchanged.

## 9. Preserve

- `npm_update_notifications_enabled`, its default and every reader of it.
- `update_check.rs`, `lib.rs`, `instance_artifacts.rs` and `instance_gitignore.rs` are untouched.
- `fn settings_path()` stays private in this phase.

## 10. Recovery

Restore only paths this phase changed that still hold this run's output (`git restore --source=HEAD -- <path>`). No `git reset`, no `git clean`, no repository-wide restore. Report external changes to `ac-tech-lead-v4`.
