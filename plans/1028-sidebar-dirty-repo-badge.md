# Plan #1028: Sidebar repo badge letters go red when the worktree is dirty

Author: architect (wg-25). Base: `main` @ `4acadfe5b22e67dff40cd20eda87b23eca4a7cbe`. Branch: `feature/1028-sidebar-dirty-repo-badge`. Issue: mblua/AgentsCommander#1028.

Status: READY_FOR_IMPLEMENTATION

Certified by architect after consensus round 2. This plan is the sole implementation specification; it carries no TBD, no competing alternative, and no choice left to the implementer. Verify the `Plan-SHA256` in the architect's round-2 report before touching code.

Consensus applied 2026-07-16. Round 1: dev-rust (Step 5), dev-webpage-ui (Step 5), grinch G1-G8 (Step 6). Round 2: dev-rust and grinch cleared §4.4's mechanism (grinch PASS on all five attack axes) and contributed R1-R7. Resolution map in §10.

---

## 1. Issue and objective

Colour the Sidebar repo badge **letters only** red (background unchanged) when that repo's working tree is dirty: untracked files, unstaged modifications, or staged-but-uncommitted changes. Clean stays violet.

The badge is `<label>/<branch>`, rendered at `ProjectPanel.tsx:2659-2671` inside `renderReplicaItem`. **It renders on coordinator rows only**: the render is gated `<Show when={isCoord() && repoBadges().length > 0}>` with `isCoord = () => replica.isCoordinator` (`:2392`), and `replicaSearchText` (`:1149`) documents the same gate as "Repo/branch badges render only for coordinators" (G8). Every statement in this plan about "rows" means coordinator rows.

The backend already spawns `git` against every one of these repo paths on a timer and discards everything except the branch name. This change stops discarding the dirty bit.

Dormant (no running session) coordinator rows are **in scope**: the user decided this, and `DiscoveryBranchWatcher` already polls every replica every 15s regardless of any session, so a dormant row's dirtiness is known and thrown away, not unknown.

---

## 2. Evidence and current-state gap

### 2.1 Three emitters write this struct to the UI (G4)

`SessionRepo` (`src-tauri/src/session/session.rs:36-44`) is the badge. Three code paths put it in front of the user:

| Emitter | Location | Cadence | Emits |
|---|---|---|---|
| `GitWatcher::poll` | `pty/git_watcher.rs:97-165` | **5s** | `session_git_repos` (`:148`) |
| `DiscoveryBranchWatcher::poll` | `commands/ac_discovery.rs:607-730` | **15s** | **Gate A** `ac_discovery_branch_updated` (`:685`), **Gate B** `session_git_repos` (`:711`) |
| `sync_workgroup_repos_inner` | `commands/entity_creation.rs:2336-2346` | **one-shot, on user action** | `session_git_repos` (`:2340`), hand-rolled `serde_json::json!` |

The first two are the badge writers this change modifies. The third is a hand-rolled emit of `build_session_repo` output (`entity_creation.rs:2128-2144`), i.e. `branch: None` and, after this change, `dirty: None`, straight to the frontend. Its conclusion is benign and bounded: it is one-shot on a user-initiated sync, and `git_watcher.invalidate_session_cache(session_id)` at `:2338` forces `GitWatcher` to re-emit within 5s. So it is a <=5s violet flash on a rare user action, exactly matching what `branch` already does there. It is listed because §2.1 is the section the feature rests on and a two-emitter census is wrong.

Both badge writers bound each detection with a 2s `DETECT_TIMEOUT` + `kill_on_drop(true)`, parallelise per repo with `join_all`, and commit through the `set_git_repos_if_gen` CAS (`session/manager.rs:701-716`), which exists to arbitrate these racing writers.

Gate A covers **every replica, session or not** (the cold feed): registration walks `wg.agents` with no session lookup (`ac_discovery.rs:499-524`), a missing session is explicitly tolerated (`:622-631`), and the git spawn is unconditional (`:635-641`). Gate B is gated on `if let Some(session_id)` (`:702`), so it is live-only.

**Consequence, non-negotiable:** if only one writer computes `dirty`, the other writes the `Option` default every 15s. Both gates compare with `PartialEq` over the whole value (`git_watcher.rs:132-135`; `ac_discovery.rs:675-683`, `:691-700`), so `Some(true)` -> `None` -> `Some(true)` is a *genuine* change every tick: the machinery that normally suppresses no-op emits would instead **guarantee** a violet flash twice per 15s cycle, on both events. Both writers must compute `dirty`.

### 2.2 The cold path is already plumbed end to end

`DiscoveryBranchPayload` (`ac_discovery.rs:361-387`) already carries `repo_branches: Vec<Option<String>>` and `repo_paths: Vec<String>`, populated at `:669-670`. Frontend: `DiscoveryBranchUpdate` (`ipc.ts:685-690`), listener `onDiscoveryBranchUpdated` (`:692-699`), wired at `ProjectPanel.tsx:415-424`, zipped into `repoBranchByPath` at `replica-volatile.ts:88-98`. A third parallel array rides all of it. No new event, no new spawn, no new cadence, no new thread.

### 2.3 `clearForPaths` is the highest "quietly does not work" risk

`replica-volatile.ts:139-161` deliberately preserves `repoBranchByPath` across a reload, for the reason documented at `:122-136`: it has no counterpart on `AcAgentReplica`, and Gate A only emits when the payload **changed**, so a wiped map is never re-sent. `repoDirtyByPath` inherits that exactly. Miss it and dirty silently vanishes on every reload (every loop tick, every CLI refresh, every entity creation).

This is not hypothetical. The test at `replica-volatile.test.ts:157-166` **used to assert the opposite**, and its own comment records that this made it "the alibi for a HIGH bug": `Browse Branch` died on the first reload and stayed dead "not for 15s, forever", because Gate A never re-emits an identical payload. `repoDirtyByPath` inherits that failure mode exactly.

### 2.4 The ancestor-repo trap is real, self-perpetuating, and reproduced

The replica tree sits inside a parent git repo. Measured in a synthetic dirty parent with a child at `parent/.ac/wg-25/repo-X`:

```
no .git                          -> ## main /  M f.txt    metadata guard CATCHES   (CASE A)
empty .git dir                   -> ## main /  M f.txt    metadata guard MISSES    (CASE B)
.git dir, config, no HEAD        -> ## main /  M f.txt    metadata guard MISSES    (interrupted clone)
.git dir, HEAD only, no objects  -> ## main /  M f.txt    metadata guard MISSES
.git FILE, dangling gitdir       -> fatal: not a git repository    SAFE, does not walk up
```

**And the parent is permanently dirty by AC's own operation.** Verified in `AgentsCommander_ac`: `git check-ignore -v .ac` exits 1 (**`.ac/` is NOT gitignored**), `git ls-files .ac` returns **312 tracked files**, and `git status` reports **12 dirty `.ac/` entries right now**. So a false red from the ancestor there would never self-clear.

**The existing ceiling guard cannot fire.** `git_watcher.rs:202-206` sets `GIT_CEILING_DIRECTORIES` from `git_ceiling_directories_for_session_root`, which returns `None` unless `is_agent_dir(cwd)` (`config/session_context.rs:1233-1236`; `is_agent_dir` = replica agent dir | canonical matrix dir | root agent dir, `:1224-1228`). A `repo-*` path is none of those, so **it is a proven no-op for every path the watcher passes it**, as `commands/repos.rs:206-213` documents ("a no-op that reads like a guard"). `DiscoveryBranchWatcher::detect_branch` (`ac_discovery.rs:931-956`) sets no ceiling at all. Today this only mis-reports a *branch*; with dirty it becomes a false red driven by an unrelated repository, on a timer, on a passively-read surface.

### 2.5 `check_workgroup_repos_dirty` is not reusable

`commands/entity_creation.rs:2422-2509` already runs `git status --porcelain` (`:2462`), but it is sync/blocking with no timeout, iterates workgroup dirs rather than a given path, and uses a **broader** definition that also counts `unpushed commits` (`:2482`) and `no remote upstream` (`:2501`). A clean-but-unpushed repo is "dirty" under that function and must **not** be under ours. Reuse the pattern, not the function.

### 2.6 Corrections to the Step 1 reports

1. **`skip_serializing_if = "Option::is_none"` does not solve the stale-red problem it was proposed for.** It omits the key only when the value is *already* `None`; a stale `Some(true)` still serializes and still restores. Proven by compiling both variants (§4.1).
2. **There is a fifth header shape**, not four: `## No commits yet on master...origin/master [gone]`. Issue AC 7 says "(4)"; it is 5. §4.3.
3. **The `.git` metadata guard is necessary but not sufficient** (§2.4, CASE B). Closed by the ceiling in §4.2, not accepted as a residual (G3).
4. **Hold-last-known does not "kill the blink at the source."** With the single call a timeout loses branch *and* dirty. `branch` keeps today's no-hold behaviour, so it still flips to `None`, the gate still sees a genuine change, and the array is still re-emitted. Hold-last-known protects the `dirty` **field**, not the emit. The user-facing benefit survives: the re-emitted array carries the held `dirty`, so the recreated span recomputes `dirty === true` and **the colour does not move**.
5. **`dirty: detected.unwrap_or(r.dirty)` works for `GitWatcher` only.** `DiscoveryBranchWatcher::poll` iterates `entry.repos`, a `Vec<(String, String)>` (`ac_discovery.rs:352-359`, `:643-652`), with no previous `dirty` in hand. Superseded by §4.4, which removes the need for a per-watcher previous value entirely.

---

## 3. Scope

### 3.1 In scope

- `dirty` on `SessionRepo`, computed by **both** badge writers from a **single** shared git call returning branch and dirty together.
- Hold last-known `dirty` across a failed detection (§4.4).
- Ancestor-repo guard: `.git` metadata check **and** a ceiling that actually fires (§4.2).
- `repo_dirty` on the Gate A payload plus the frontend `repoDirtyByPath` volatile layer, so dormant coordinator rows are covered.
- Red letters via an additive CSS variant class; badge `title` carries the third state.

### 3.2 Out of scope

- **Dedupe by `source_path`.** Filed as #1029. Measured and refuted as a timeout mitigation (8 concurrent same-path calls finish in 424ms, ~2x *faster* than sequential, sharing the page cache). Efficiency only. Do not fold it in.
- **`SessionItem`'s `.session-item-branch` chips** (`SessionItem.tsx:393-405`). Dim gray, not violet. User decision: out.
- **Light-theme `.branch` contrast.** No light-theme override, so it keeps its dark-theme violet and reads 1.94:1 there today, a hard WCAG fail. Pre-existing; needs its own issue.
- **Timeout decay** (N consecutive failures -> `None`). **Not justified as "speculative" (G2).** §7.3 establishes a *measured* path to a permanent timeout on a bulk-stat-dirty repo, where `branch` (which has no hold) would go `None` on every tick indefinitely. The omission is kept for two reasons that are not "we have not seen it": the trigger is a rare compound condition (bulk stat-dirty *and* a repo large enough to exceed 2s, measured at 6000 tracked files but not at 654), and decay is a distinct feature with its own state and policy that would need its own issue. **This is the known, measured cost of the decision, recorded as a decision.**
- **Cross-watcher emit-cache coherence** (§6.6). Pre-existing, ships today for `branch`, not introduced here.
- **`branch` hold-last-known.** `branch` keeps today's exact behaviour.

---

## 4. Decided solution

### 4.1 Persisted `dirty`: `#[serde(default, skip_deserializing)]`

**Decision.** It satisfies both devs' stated goals at once; there is no trade.

The mechanism the reports debated (`skip_serializing_if`) is the wrong tool for both. Proven by compiling both candidates against serde 1.0.228 / serde_json 1.0.149:

```
--- OPTION B: skip_serializing_if = Option::is_none ---     (what the reports debated)
serialize Some(true) -> {"label":"A","dirty":true}          << stale value IS persisted
restore from that    -> OptB { dirty: Some(true) }          << STALE RED SURVIVES
serialize None       -> {"label":"A"}                       << key omitted from the WIRE too

--- OPTION C: skip_deserializing ---                        (decided)
serialize Some(true) -> {"label":"A","dirty":true}          << wire ALWAYS self-describing
restore from that    -> OptC { dirty: None }                << restores as None: no stale red
serialize None       -> {"label":"A","dirty":null}          << key present as null
old JSON (no key)    -> OptC { dirty: None }                << back-compat OK
input claims dirty   -> OptC { dirty: None }                << backend-authoritative
```

Option B fails **both** goals: the stale `true` gets through, and the key is stripped from the wire on `None`. Option C gives an always-present key and a guaranteed `None` on restore.

Safe because nothing needs to read `dirty` back. **Five** deserialize entry points, all audited:

- `sessions_persistence.rs:188` (`PersistedSession.git_repos`) - the intended target. Restore yields `None`.
- `commands/session.rs:1910` - `create_session` is a `#[tauri::command]` and genuinely deserializes `Vec<SessionRepo>` from frontend input. The frontend sends `SessionRepoInput` = `{label, sourcePath}` (`ipc.ts:77-80`) and never sends `dirty`, so nothing is lost. Making it un-settable is correct: `dirty` is backend-authoritative and now un-spoofable from a command payload.
- `SessionInfo` (`session/session.rs:238`) derives `Deserialize`, but no Rust-side `from_str`/`from_value`/`from_slice`/`from_reader` on it exists. Confirmed by grep.
- `load_sessions_raw` (`sessions_persistence.rs:598-604`) - production, used by the CLI (`cli/list_peers.rs:7`, `cli/list_sessions.rs:4`). Read-only listing; never reads `dirty`; never writes `sessions.json` back. Harmless.
- `commands/config.rs:1930` `read_sessions_file` - inside `#[cfg(test)]`, paired with `write_sessions_file` at `:1925`. Harmless today. **Note for the implementer: a future test that round-trips `dirty` through this helper will silently get `None` back and will look like a serde bug rather than the intended design.**

Old `sessions.json` has no `dirty` key; `skip_deserializing` uses `Default::default()` = `None`. `#[serde(default)]` is redundant next to `skip_deserializing` (serde already falls back to `Default::default()`) but is kept: the issue text specifies it, it is harmless, and it states intent at the field. The `PartialEq`/`Eq` derives are unaffected: `manager.rs:766`'s `if &s.git_repos != repos` gate is already dominated by `branch: None` vs a detected branch, so it reports changed today and after.

Cost: the on-disk file still writes a `dirty` key that is ignored on read. Dead data, useful in a bug report. Accepted.

### 4.2 One shared detection, replacing two diverged copies

`GitWatcher::detect_branch` (`git_watcher.rs:193-224`) and `DiscoveryBranchWatcher::detect_branch` (`ac_discovery.rs:931-956`) are near-identical copies that have **already diverged**: one sets the (no-op) ceiling env, the other does not. The plan's own non-negotiable is that both writers compute `dirty` identically. Duplicating a new 5-shape parse into both is how that guarantee gets lost.

**Decision:** one shared `pub(crate)` guard + spawn + parse in `pty/git_watcher.rs`. Each watcher keeps its **own timeout wrapper**, which is **required, not stylistic**: each owns its per-path counter and log tag (`note_timeout` / `note_discovery_timeout`, `[GitWatcher]` / `[DiscoveryBranchWatcher]`), and merging them would collapse the two tags that keep the watchers distinguishable in `app.log`.

Placement in `pty/git_watcher.rs` rather than a new module: no module restructuring, and `commands/ac_discovery.rs` already depends on `crate::pty::*` (`crate::pty::credentials::scrub_credentials_from_tokio_command`, `ac_discovery.rs:937`, inside the very function being deleted). Tests live next to the parser.

The call:

```
git --no-optional-locks status --porcelain --branch
```

**`--no-optional-locks` is load-bearing, not hygiene (G7).** It is what keeps the 2s `kill_on_drop` from orphaning `.git/index.lock` in the user's repo. Measured: plain `status` takes the lock (a ~13ms window at ~99% of the run) and rewrites `.git/index`; `--no-optional-locks` does neither; a hard kill (`TerminateProcess`, which is what `kill_on_drop` does) inside that window orphaned the lock **8/8**. An orphaned `index.lock` leaves `git status` at exit 0 and silent while `git add` and `git commit` **hard-fail**, so the agent's commits break while the badge keeps painting normally. The bound is narrow (the kill must land within ~13ms of the 2s deadline; killing at 30-170ms orphaned 0/12), but the blast radius is severe and the mechanism is proven. It also stops the poll writing `.git/index` every tick. **This flag has a cost too, in §7.3; the plan owns both halves.** Do not remove it in a later cleanup.

The prior note that "`git status` exits 0 with `index.lock` held either way" is true and about the **wrong direction**: it establishes the watcher tolerates someone else's lock, not that it will not orphan one on everyone else.

**Ancestor guard: both halves (G3).**

1. **`.git` metadata check.** `tokio::fs::metadata(Path::new(working_dir).join(".git"))`, return `None` on error, before spawning. Use `metadata` not `is_dir` (linked worktrees and submodules use a `.git` **file**) and `tokio::fs` not `std` (a sync stat inside an async fn cannot be cancelled by `tokio::time::timeout`; on a dead share it would wedge a runtime worker and make the 2s cap a lie). Precedent: `commands/repos.rs:218-224`.
2. **`GIT_CEILING_DIRECTORIES` set to the repo path's parent**, via one `cmd.env()` call. This closes CASE B (§2.4), which the metadata check misses.

The earlier draft kept CASE B open citing `commands/repos.rs` precedent. **That argument is withdrawn.** `repos.rs`'s comment enumerates "a failed clone, deleted `.git`, a plain folder the user configured" - all CASE A. Corrupt-but-present `.git` appears nowhere in it, so the "identical residual" was an absence of consideration, not a considered acceptance. And `repos.rs`'s "do not add it back" forbids `git_ceiling_directories_for_session_root`, "a no-op that reads like a guard" - a *different mechanism*. Computing the ceiling from the repo path's parent is not re-adding the no-op helper, and only the helper is forbidden. Blast radius differs too: `repos.rs` is menu-driven and one-shot, and its failure is a wrong GitHub page on a click; the badge's is a persistent false red on a 5s/15s timer on a passively-read surface.

Measured. The first line closes CASE B; the rest are the "does the ceiling break a legitimate repo" cases, and nothing was harmed:

```
CASE B (corrupt .git) + GIT_CEILING_DIRECTORIES=<parent-of-repo>
                                          -> fatal: not a git repository, exit 128   (closes it)
HEALTHY repo + same ceiling               -> ## No commits yet on feat               (unharmed)
LINKED WORKTREE (.git is a FILE)          -> ## wt / ?? w.txt                        (unharmed)
  ... whose MAIN repo lives ABOVE the ceiling -> ## wt2 / ?? w.txt                   (unharmed)
SUBMODULE superproject, clean             -> ## main                                 (unharmed)
SUBMODULE dirty, WITHOUT ceiling          -> ## main /  M sub
SUBMODULE dirty, WITH ceiling             -> ## main /  M sub                        (IDENTICAL)
```

The ceiling constrains only the **discovery ascent**; it does not constrain **gitdir resolution** through a `.git` file. That is why a linked worktree still resolves correctly even when its main repo lives above the ceiling, and why the issue's submodule decision (§6.2: default behaviour, a moved or dirty submodule turns the superproject red) is preserved byte-for-byte: ` M sub` appears identically with and without the ceiling. Implementation notes: build the value from the repo path's parent with `std::env::join_paths` (which emits the platform separator, `;` on Windows, matching what the existing ceiling code does); if the path has no parent, skip the env entirely rather than setting an empty value.

**This removes the `git_ceiling_directories_for_session_root` call from `GitWatcher::detect_branch`.** Deliberate and behaviour-neutral: it returns `None` for every path the watcher passes it (§2.4). Replacing a guard that cannot fire with one that does is strictly better.

Keep, from the current copies: `scrub_credentials_from_tokio_command`, `kill_on_drop(true)`, `CREATE_NO_WINDOW`, and **the `out.status.success()` gate** (`git_watcher.rs:214`, `ac_discovery.rs:946`). Dropping the success gate would not open a correctness hole (a failed git writes `fatal:` to stderr, leaving stdout empty, so the parser's "no `## ` first line" rule returns `None` anyway) but it would be an unintended silent behaviour change.

Name the function for worktree dirty and comment the distinction from `check_workgroup_repos_dirty` (§2.5).

### 4.3 Parse contract: 5 shapes, enumeration provably closed

| # | State | Header |
|---|---|---|
| 1 | born, no upstream | `## master` |
| 2 | born, upstream in sync | `## master...origin/master` |
| 3 | born, upstream diverged | `## master...origin/master [ahead 1, behind 2]` |
| 4 | detached / mid-rebase | `## HEAD (no branch)` |
| 5 | unborn, no upstream | `## No commits yet on master` |
| 5b | **unborn with upstream** | `## No commits yet on master...origin/master [gone]` |

Tracking markers observed: `[ahead 1]`, `[behind 2]`, `[ahead 1, behind 2]`, `[gone]`.

**The enumeration is closed, not merely "what we found".** Git builds the header as `## ` + (`No commits yet on ` if unborn) + branch + (`...` + upstream + optional ` [...]` if an upstream is configured), so unborn and upstream are orthogonal and the matrix is enumerable. The closing measurement: with a **real, existing** `origin/master` fetched, an unborn branch **still** prints `[gone]`, because an unborn branch has no commit so `stat_tracking_info` cannot compute ahead/behind and falls through to the gone marker. **There is therefore no "unborn + upstream + no bracket" shape**, and both variants of 5b produce the identical string form.

**Splitting on `...` is unambiguous**, and step (d) is safe, because git's refname rules forbid both sequences:

```
git branch "feat..bad" -> fatal: 'feat..bad' is not a valid branch name    (so `...` split is safe)
git branch "feat[x]"   -> fatal: 'feat[x]' is not a valid branch name      (so step (d) cannot eat a real name)
git branch HEAD        -> fatal: 'HEAD' is not a valid branch name         (so step (e) is unreachable)
## feat/a.b-c                                                              (single dots and slashes survive)
```

Required parse order:

1. If the first line does not start with `## `, return `None` (output we do not understand; do not guess).
2. `dirty` = **any** non-empty line after the first. Filter empty lines; output ends with a trailing newline.
3. Branch, from the first line with `## ` stripped:
   a. If it equals `HEAD (no branch)`, branch = `None`. **Must short-circuit before (b).**
   b. Strip a leading `No commits yet on ` if present. **Then continue to (c); do not return here.** This is what makes 5b work.
   c. Take everything before the first `...` if present, else the whole remainder.
   d. Defensively trim a trailing ` [...]` tracking suffix. Unreachable after (c) with today's git (a marker implies an upstream implies `...`), and safe because `[` is forbidden in refnames.
   e. Trim. If empty or equal to `HEAD`, branch = `None`. Unreachable per the refname rule above; kept for parity with today's `git_watcher.rs:216`.

Dirty lines are any porcelain v1 entry: ` M tracked.txt` (unstaged), `A  staged.txt` (staged-but-uncommitted), `?? untracked.txt` (untracked). All three verified together in one run, and all three are exactly the definition the issue names.

### 4.4 Hold last-known: a path-keyed memory of successful detections only

**This section replaces the round-1 mechanism entirely. It is the fix for G1, and it cleared consensus round 2: grinch PASSed it on write-reordering, two runtimes, poisoning, the bound, and sharing, and withdrew its own competing fix.**

Convert transient false-cleans into transient false-reds. The errors are asymmetric: a false-clean means shipping uncommitted work (silent, costly); a false-red self-corrects on the next tick (cheap). `branch` keeps today's behaviour exactly: `None` on failure, no hold.

**Why the round-1 mechanism failed.** It sourced GitWatcher's previous value from the manager (`r.dirty`, via `get_sessions_repos` -> `s.git_repos`, `manager.rs:781`) and discovery's from `repos_cache`. Both are stores whose lifecycle is governed by *emit gating* and *race arbitration*, reused for a *value memory* purpose. Their resets have nothing to do with whether we still know a repo's dirty state, and they fire for reasons correlated with the very failures the hold exists to absorb:

| Source | Reset / poisoning | Correlated with a timeout cluster? |
|---|---|---|
| Manager `r.dirty` | Gate B writes `s.git_repos` (`ac_discovery.rs:704-708`), so a discovery failure writes `None` into GitWatcher's hold source | **Yes** (G1's 6-step chain) |
| Manager `r.dirty` | `refresh_git_repos_for_sessions` (`manager.rs:756-770`) wholesale-replaces `s.git_repos` with `build_session_repo` output (`dirty: None`) | No (user sync) |
| `repos_cache` | CAS gen mismatch -> `remove` (`ac_discovery.rs:723`) | **Yes** (a GitWatcher timeout bumps the gen, failing discovery's CAS) |
| `repos_cache` | `invalidate_replicas` (`ac_discovery.rs:557-576`) | No (user sync) |
| GitWatcher's own cache | `invalidate_session_cache` (`entity_creation.rs:2338`) | No (user sync) |
| GitWatcher's own cache | CAS gen mismatch -> `remove` (`git_watcher.rs:161`) | **Yes** |

Grinch's proposed fix (GitWatcher holds from `self.cache`) removes the *correlated* poisoning of row 1, which is a real improvement, but rows 5 and 6 show its source has resets of its own, and grinch states the remaining residual himself: "a cluster that outlives a `repos_cache` reset still degrades **discovery** to `None`." Since the feature's whole promise is never to assert a confident clean, a fix that knowingly leaves a false-clean path is not the right resolution when a complete one is smaller.

**The decided mechanism.** A process-local map keyed by repo `source_path`, written **only on a successful detection**, read on failure. This is a structural twin of `note_timeout` (`git_watcher.rs:15-28`) which sits ~100 lines above it in the same file: same `OnceLock<Mutex<HashMap<String, _>>>` shape, same lock-poisoning idiom, same "process-local, resets on restart" semantics, and the same documented bound ("bounded by the number of distinct repo paths (~150 in production observations), so no GC is needed").

```rust
/// #1028 - last SUCCESSFULLY detected worktree-dirty state per repo path, so a
/// transient detection failure holds the previous answer instead of asserting a
/// confident "clean". Only successful detections are ever written, so a failure
/// can never poison the memory. Keyed by repo path, not by session or replica:
/// dirty is a property of the worktree, and several replicas legitimately share
/// one repo dir. Process-local: resets on restart, which is correct (an unknown
/// repo is `None` = violet until its first answer). Bounded by the number of
/// distinct repo paths (~150 in production observations), so no GC is needed.
/// NOTE: unlike `note_timeout`, this map is deliberately SHARED by both watchers.
/// See the sharing rationale below: separate maps would re-admit the flicker
/// that §2.1 calls broken by design.
///
/// `pub(crate)` because the hold must be applied where a FAILURE is observed,
/// which is each watcher's timeout wrapper, and one of those lives in
/// `commands/ac_discovery.rs`. See §5.1.
///
/// INVARIANT, and it is NOT enforced here: this function remembers whatever it
/// is handed. "Never remembers an ancestor's dirt" is enforced by the caller,
/// `detect_git_status`, via its three gates (`.git` metadata check, ceiling,
/// `out.status.success()`). Only feed this from `detect_git_status`. A caller
/// like `detect_git_branch_sync` (`ac_discovery.rs:305-331`), which has no
/// `.git` guard and no ceiling, would silently violate it. Nothing does today.
pub(crate) fn remember_dirty(path: &str, detected: Option<bool>) -> Option<bool> {
    use std::sync::OnceLock;
    static LAST_DIRTY: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let map = LAST_DIRTY.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = map.lock().unwrap_or_else(|e| e.into_inner());
    match detected {
        Some(d) => {
            g.insert(path.to_string(), d);
            Some(d)
        }
        None => g.get(path).copied(),
    }
}
```

Call site, **identical in shape in both watchers**, at each `SessionRepo` construction. The path argument must come from the **source binding**, not the field being initialised: inside a `SessionRepo { .. }` literal, `source_path` is a field name and not a variable in scope.

```rust
// GitWatcher::poll (git_watcher.rs:122-130), iterating `repos.iter().zip(statuses)` as `(r, status)`:
branch: status.as_ref().and_then(|s| s.branch.clone()),
dirty: remember_dirty(&r.source_path, status.as_ref().map(|s| s.dirty)),

// DiscoveryBranchWatcher::poll (ac_discovery.rs:643-652), whose closure destructures `((label, path), status)`:
branch: status.as_ref().and_then(|s| s.branch.clone()),
dirty: crate::pty::git_watcher::remember_dirty(path, status.as_ref().map(|s| s.dirty)),
```

Compiled and behaviour-tested standalone, including the case that defeats both reviewed alternatives:

```
cold start + failure        -> None         (unknown, violet - never detected)
detect true                 -> Some(true)   (red)
5-tick timeout CLUSTER      -> Some(true)   (still red, cluster survived)
detect false                -> Some(false)  (violet)
independent path            -> None         (unaffected by the other path)
```

**Why this resolves G1 completely, and why it is smaller than what it replaces.** The invariant is one line: *only successful detections are ever written*, so no failure of either watcher can poison it, and there is nothing to reset. Concretely it closes every row of the table above, and it deletes from the round-1 plan: the `repos_cache` snapshot, dev-rust's owned-keys compile fix, the "snapshot before `:643`" timing constraint, the four reset points, and the asymmetry between the two watchers. A held value is always "the last successful detection of this path by anyone", and the 8 replicas sharing 2 repo dirs measured in wg-25 all get the same answer.

Two properties fall out of the shape and are worth naming, because both were attacked in round 2 and neither is a discipline the implementer must maintain. `remember_dirty` is a **sync fn**, so the guard cannot outlive it and no `.await` can appear inside: that is a guarantee from the signature, and it is what *deletes* round 1's owned-keys borrow problem rather than fixing it. And `.unwrap_or_else(|e| e.into_inner())` is required, not stylistic: `.unwrap()` would be the bug, since a single panic would take both watchers down permanently. Poisoning needs a panic while the guard is held, and the section is a `HashMap` insert/get over `String`/`bool` with no user code and no unwinding panic point.

**Sharing is REQUIRED by §2.1, not merely the best estimate. Do not "fix" it back to the `note_timeout` precedent.** With **separate** per-watcher maps, a cluster hitting both watchers leaves their holds sourced from successes up to 5s and up to 15s old respectively; if the repo changed in between, **the two watchers hold different values**, both write divergent `dirty` into `s.git_repos`, and **both emit**, because each compares against its own differing cache. The CAS is not a backstop: it only suppresses discovery when GitWatcher's write lands inside discovery's detection window. The result is the red/violet flicker §2.1 calls "broken by design", gated behind a cluster instead of arriving every tick. Sharing makes the two watchers agree by construction. Verified by dev-rust: `probe_shared_keeps_watchers_in_agreement ... ok`.

The scan sets also cut *for* sharing: on the paths only one watcher touches, shared behaves exactly like private; on the overlap, both get the freshest observation. Neutral where they differ, better where they meet.

**Deliberate divergence from the cited precedent, stated so it is a decision:** `note_timeout` and `note_discovery_timeout` are deliberately **separate** statics so "the two watchers track their own paths independently (different scan sets)" (`ac_discovery.rs:338-340`). That is the right call for a **throttle counter** (sharing would make "1st + every 50th" fire at wrong times for both watchers) and irrelevant for **a fact about a worktree**. The `~150 distinct repo paths, no GC` bound transfers **exactly and not by analogy**, because `note_timeout`'s comment bounds by *distinct repo paths*, not by timeouts, and that is the same key space (~20 KB). The one real difference is fill *rate*: `note_timeout` gains an entry only on a timeout, `remember_dirty` on nearly every path, so the map goes from a handful to ~150, with *smaller* entries (`bool` vs `u64`). The leak-on-churn case (`invalidate_replicas` re-registering with new paths, `ac_discovery.rs:554-556`) needs ~85,000 distinct repo paths in one process lifetime to reach 10 MB. Accepted.

**A remembered value cannot be an ancestor's via repo discovery, which is the property the mechanism rests on.** `remember_dirty` writes only when `detect_git_status` returns `Some`, so the question is whether a *success* can carry a wrong `dirty`. Not through discovery: three independent gates each turn the ancestor-walk cases into `None` before a value exists. The `.git` metadata check rejects CASE A; the ceiling makes CASE B exit 128 (measured, §4.2); the retained `out.status.success()` gate rejects any non-zero exit. Measured end to end: corrupt `.git` + ceiling gives `exit=128`, so the success gate rejects it, so `parse_status_porcelain_branch` is never reached, so nothing is remembered. The pre-§4.2 ancestor cases that *did* return `Some` with the ancestor's dirt (§2.4) are exactly what §4.2 closes, which is why §4.2 and §4.4 are one change and not two.

**The claim is scoped to discovery on purpose.** All three gates govern how git *discovers* a repo by walking directories. **`GIT_DIR`/`GIT_WORK_TREE` bypass discovery entirely**, and `detect_git_status` inherits the app process environment (`scrub_credentials_from_tokio_command` touches credential helpers only; the `env_remove_keys` machinery in `config/agent_command.rs` is for agent PTY sessions, not this spawn). Measured, valid `.git`, ceiling set exactly as §4.2 specifies:

```
repo-X alone, ceiling set                              -> ## feat                 exit=0   (correct)
repo-X + GIT_DIR/GIT_WORK_TREE -> dirty parent,
        ceiling STILL set                              -> ## main /  M file.txt   exit=0   (ancestor's dirt)
today's rev-parse, same GIT_DIR                        -> main                             (pre-existing)
```

**This is a sentence, not a code change, and deliberately so.** There is no realistic vector: a desktop app would need the user to have exported `GIT_DIR` in the launching shell. It is **pre-existing** and identical for today's `rev-parse` spawn (third line above), so it is not introduced here. It belongs to §4.2's spawn rather than §4.4's memory. And **§4.4 does not amplify it**: an env var set for the process lifetime means every detection is wrong and the map is continuously overwritten with the same wrong value, so the memory adds no persistence the process did not already have; a restart clears both. `env_remove` is explicitly **not** wanted here: it would be code for a threat nobody faces.

**Residual: a stale success can overwrite a fresher one, bounded to ~21ms.** The invariant "only successful detections are ever written" closes everything about *failures*, and is silent about *stale successes*, which are a different axis from the ancestor question above. The map write **bypasses the CAS**: `remember_dirty` fires at construction (`git_watcher.rs:122`, `ac_discovery.rs:643`), ~60 lines before the CAS (`:142`, `:706`), so a value the CAS rejects as stale **has already been committed to the map**. The map is last-writer-wins, so an earlier observer completing last can overwrite a fresher value with no failure involved. Verified by dev-rust: `probe_stale_success_overwrites_fresher ... ok`.

**It is inherent, not fixable, and the plan accepts it.** The call cannot move after the CAS: the construction site *needs* the held value to build `refreshed`, which is what the CAS receives, so the dependency is circular. And discovery's CAS runs only inside `if let Some(session_id)` (`ac_discovery.rs:702`), so **dormant replicas never reach it**; moving the call there would mean dormant paths are never remembered, destroying the cold-path hold this section exists to buy.

**Bounded by measurement.** An inversion needs the start stagger between the two watchers to be smaller than the difference in their detections' durations, and both watchers run the *identical command on the identical path*, so durations match and start order is completion order. Grinch measured: 6000-file repo at 1s stagger, **0/4 inversions**; 654-file repo, **max staleness injectable 21 ms**, self-correcting on the next success. #1029's "8 concurrent same-path calls are ~2x faster, sharing the page cache" **does not transfer** as a counter-argument, because `status` is lstat-and-rehash bound rather than page-cache bound. These figures are local SSD, warm cache, synthetic repos, and they **bound a race window, not a production rate**. The residual surfaces only if a *later* detection fails, and it errs to a **false-red**, which is the direction §4.4 deliberately chooses and the direction every reviewed alternative failed to hold.

**Residuals, accepted and stated:**
- A repo whose `.git` is deleted mid-session holds its last red until restart, because `detect_git_status` cannot distinguish "not a work tree" from "timed out". This is the same "hold indefinitely" edge already accepted under timeout decay (§3.2). For a path that was *never* detected, the value is `None` and stays `None`, which is correct.
- Entries for repos removed from config persist until restart. Same property, same bound, and the same explicit acceptance as `note_timeout`'s counters.
- Tests must use unique-per-test path strings, exactly as `note_timeout_counts_per_path` already documents ("Process-global state: use unique-per-test path strings to avoid collisions").

### 4.5 Frontend

- **Type contract:** Rust `dirty: Option<bool>`; TS **`dirty: boolean | null` (required, no `?`)** (G6). Badge keyed strictly off **`dirty === true`**, collapsing `{false, null}` to violet. Pin `=== true` rather than a truthy check: behaviourally identical today, correct if the field widens, and it reads as deliberate.

  **Why required, reversing the round-1 draft.** §5.1's safety argument is that `SessionRepo` has no `Default`, so every Rust construction site is compile-forced; a `?` throws that away on the TS side for test convenience. The precedent cuts the same way: `types.ts:1-5` already has `branch: string | null` **required** against a Rust `#[serde(default)] Option<String>`. The site that matters is `replica-repo-badges.ts:55-59`, the one production builder of `SessionRepo[]` on the cold path: with `?`, an implementer who adds `repoDirtyByPath` to the signature and forgets the `dirty:` line gets **no `tsc` error and a permanently violet cold feed** - the exact silent failure the type is there to prevent. dev-webpage-ui's own correction concedes that literal will always set `dirty`, removing the only production reason for the `?`. Cost: the ~6 test literals §7.1 already counts.
- **Additive variant class, not an edit to the existing rule.** `.ac-discovery-badge.branch` is also used by `AcDiscoveryPanel.tsx:294`, so editing its `color` repaints that too. `.ac-discovery-badge.branch.dirty` is (0,3,0) and beats the (0,2,0) rule at `sidebar.css:5511`. This is the repo's established idiom, with the specificity rationale already documented at `sidebar.css:5534-5538` for `.coord-idle.red` (`:5553`). Setting `color` only leaves `background` untouched by construction, satisfying "letters only" literally.
- **Red = `var(--status-exited)`** (`variables.css:16` `#ff3b5c` dark, `:67` `#dc2626` light). No new hex. Contrast, measured by compositing the badge's `rgba(139,92,246,0.15)` over the real row backgrounds: **4.97:1** base, 4.57 hover, 4.31 active. Within 0.01 of `.coord-idle.red` (4.89 / 4.51 / 4.32), a red badge already shipping on the same row.
- **Wire naming.** `SessionRepo` carries `#[serde(rename_all = "camelCase")]` and `dirty` is one lowercase word, so it round-trips unchanged. The Gate A field `repo_dirty` -> `repoDirty` does transform, but `DiscoveryBranchPayload` **already carries `rename_all = "camelCase"`** (`ac_discovery.rs:361-362`) and the identical `repo_branches` -> `repoBranches` mapping is load-bearing in production today (all of #943 B2 depends on it). **Risk: low**, not "the highest quietly-does-not-work risk" as the round-1 draft claimed. The residual is only "did someone name the two sides inconsistently", and §5.1/§5.2 pin both names so they agree by construction. The genuine highest risk is `clearForPaths` (§2.3).
- **Reactivity needs no work.** Both feeds replace the whole array; `<For>` is reference-keyed and the cold path maps into fresh objects on every evaluation (`ProjectPanel.tsx:805-812`). The `<For>` callback param `repo` is a plain value, not an accessor, so the class is computed once per item identity: the dirty signal **must** arrive as a full re-emit of the repos array, which is what all three emitters already do. No scalar "dirty changed" event, no in-place mutation. Verified there is no hole: dirty-flips-branch-same, timeout-branch-None-dirty-held, and detached-HEAD-dirty-flips all differ in the array and emit; a timeout on a repo whose branch was *already* `None` with dirty held is byte-identical and correctly stays silent, because nothing about the badge changed.
- **No entrance animation replays.** `.ac-discovery-badge` (`sidebar.css:5487-5494`) sets only `font-size`, `font-weight`, `letter-spacing`, `padding`, `border-radius`, `text-transform`; no `animation`, no `transition`, and no keyframes target it. Span recreation costs a DOM node, not a visible flash.
- **Cold cannot clobber live.** `repoBadges()` (`ProjectPanel.tsx:2405-2410`) is strictly either/or, never a merge: live wins unconditionally whenever `s.gitRepos` is non-empty. The fallback is dynamic, so if a session exits and `s.gitRepos` goes empty the row falls back to the cold feed mid-life; with the cold path dirty-aware it stays correct there, a second independent reason the cold path is in scope.

### 4.6 Tooltip

The badge already carries `title={repo.sourcePath}` (`ProjectPanel.tsx:2664`). Extend it so colour stays a binary alarm at 8px while the tooltip carries the third state:

| `dirty` | `title` |
|---|---|
| `true` | `` `${repo.sourcePath} (uncommitted changes)` `` |
| `false` | `repo.sourcePath` (unchanged) |
| `null` | `` `${repo.sourcePath} (status unknown)` `` |

**"(status unknown)" is the normal startup state, not an error state**, and the wording is chosen for that: a cold row shows it until the first Gate A tick (<=15s) and a live row until the first `GitWatcher` tick (<=5s), because `effectiveRepoDirtyByPath` returns `undefined` before any event lands and `configuredReplicaRepoBadges` maps that to `null`. It is the first thing a user sees on every launch, so it must read as "not yet known" rather than "something is broken". Deliberate.

**On a timeout the badge shows the label without its `/branch` suffix while staying red** (`AgentsCommander` in red, not `AgentsCommander/main`). That is correct on both axes independently and reads as "something is off, and there is still uncommitted work". Named here because it is a visible state and nobody should be surprised by it.

Reject a third badge colour. The codebase renders unknown as **absence**, never a distinct colour (`replica-repo-badges.ts:19` degrades to no suffix; `coordinator-badge.ts:30-40` returns `null`), there is no room at 8px/600, the user asked for one thing, and a third colour would advertise a transient internal failure the user can do nothing about.

---

## 5. Affected surfaces

Line numbers are from base `4acadfe` and will shift. Symbols are authoritative.

### 5.1 Rust

| File | Symbol | Change |
|---|---|---|
| `session/session.rs` | `SessionRepo` (`:36-44`) | Add `dirty: Option<bool>` with `#[serde(default, skip_deserializing)]` + doc. Keep `PartialEq`/`Eq`: they are what make both gates re-emit on a dirty flip. |
| `pty/git_watcher.rs` | new `pub(crate) struct GitStatus { branch: Option<String>, dirty: bool }` | Detection result. |
| `pty/git_watcher.rs` | new `pub(crate) fn parse_status_porcelain_branch(stdout: &str) -> Option<GitStatus>` | Pure parser, §4.3. |
| `pty/git_watcher.rs` | new `pub(crate) fn remember_dirty(path, Option<bool>) -> Option<bool>` | §4.4. **`pub(crate)`, not module-private, and this is forced rather than preferred:** on a timeout `tokio::time::timeout` **drops** the `detect_git_status` future, so it never returns and cannot remember anything. The hold must therefore be applied where the failure is observed, which is each watcher's own timeout wrapper or call site, and one of each lives in `commands/ac_discovery.rs`. There is no arrangement that keeps the map module-private and still holds on the failure path, which is the only path it exists for; the alternative would be merging the two timeout wrappers, collapsing `note_timeout`/`note_discovery_timeout` and the `[GitWatcher]`/`[DiscoveryBranchWatcher]` log tags that §4.2 establishes are load-bearing. A plain `fn` here is a reproduced **`error[E0603]: function 'remember_dirty' is private`**. |
| `pty/git_watcher.rs` | new `pub(crate) async fn detect_git_status(working_dir: &str) -> Option<GitStatus>` | `.git` metadata guard + ceiling env + spawn + `out.status.success()` gate + parse. Replaces both `detect_branch` copies. Keeps `scrub_credentials_from_tokio_command`, `kill_on_drop(true)`, `CREATE_NO_WINDOW`. Drops the no-op ceiling helper call (§4.2). Name it for worktree dirty; comment the distinction from `check_workgroup_repos_dirty`. |
| `pty/git_watcher.rs` | `GitWatcher::detect_branch` (`:193-224`) | Delete; superseded. |
| `pty/git_watcher.rs` | `GitWatcher::detect_branch_with_timeout` (`:170-191`) | Retarget to `detect_git_status`, return `Option<GitStatus>`. Keep `note_timeout`, the `[GitWatcher]` tag, and the 1st+every-50th dampening verbatim. |
| `pty/git_watcher.rs` | `GitWatcher::poll` (`:122-130`) | Build `SessionRepo` with the §4.4 call-site shape. |
| `commands/ac_discovery.rs` | `DiscoveryBranchWatcher::detect_branch` (`:931-956`) | Delete; superseded. |
| `commands/ac_discovery.rs` | `DiscoveryBranchWatcher::detect_branch_with_timeout` (`:910-929`) | Retarget to `crate::pty::git_watcher::detect_git_status`. Keep `note_discovery_timeout` and the `[DiscoveryBranchWatcher]` tag. |
| `commands/ac_discovery.rs` | `DiscoveryBranchWatcher::poll` (`:643-652`) | Build `SessionRepo` with the §4.4 call-site shape. **No `repos_cache` snapshot is needed**; §4.4 removed it. |
| `commands/ac_discovery.rs` | `DiscoveryBranchPayload` (`:361-387`) | Add `repo_dirty: Vec<Option<bool>>` + doc. Already derives `PartialEq`, so Gate A re-emits on a dirty flip with no gate change. |
| `commands/ac_discovery.rs` | Gate A payload construction (`:662-671`) | `repo_dirty: refreshed.iter().map(\|r\| r.dirty).collect(),` beside the existing two. |
| `commands/entity_creation.rs` | `build_session_repo` (`:2139`) | Add `dirty: None`. **Note: this literal is emitted straight to the UI at `:2340`** (§2.1), so it is a <=5s violet flash on a user-initiated sync, bounded by `invalidate_session_cache` at `:2338`. Intended. |
| `config/sessions_persistence.rs` | `:690`, `:711` | Add `dirty: None` to the two legacy-upgrade literals. |

`detect_git_branch_sync` (`ac_discovery.rs:305-331`) is **not** a badge writer and is not touched.

**Compile-forced sites: 11, not 10.** `SessionRepo` has no `Default` derive and no site uses `..Default::default()`, so every construction site must add `dirty` or the crate will not build. `grep -rn "SessionRepo {" src-tauri/src --include=*.rs` returns 12 hits minus the struct definition at `session.rs:36` = **11**:

- **Production (5):** `ac_discovery.rs:647`, `entity_creation.rs:2139`, `sessions_persistence.rs:690`, `:711`, `git_watcher.rs:125`.
- **Test (6):** `git_watcher.rs:242`, `:259`, `:271`, `:289`, `sessions_persistence.rs:3461`, `:3600` (the last two inside `#[cfg(test)]`, nearest preceding at `:1613`).

Also verified: no `SessionRepo{` without a space that the grep would miss, and `src-tauri/tests/` (~11 integration files) contains **zero** `SessionRepo` references, so there are no hidden sites in the `cargo test` build.

The report line "the 4 restore sites already default" is wrong twice: they do not default, and they are the *legacy upgrade* path, not the main restore. `PersistedSession.git_repos` restores through plain serde at `sessions_persistence.rs:188`, which is not one of them; `skip_deserializing` is what guarantees restore-as-`None`.

### 5.2 TypeScript / CSS

| File | Symbol | Change |
|---|---|---|
| `shared/types.ts` | `SessionRepo` (`:1-5`) | Add `dirty: boolean \| null;` (**required**, G6) + doc. |
| `shared/types.ts` | new `RepoDirtyByPath` | `Record<string, boolean \| null>`. Mirror the `RepoBranchByPath` doc (`:7-20`) on missing-key vs explicit-null. |
| `shared/ipc.ts` | `DiscoveryBranchUpdate` (`:685-690`) | Add `repoDirty: (boolean \| null)[];`. |
| `stores/replica-volatile.ts` | `ReplicaVolatileEntry` (`:24-32`) | Add `repoDirtyByPath?: RepoDirtyByPath;`. |
| `stores/replica-volatile.ts` | `buildRepoBranchByPath` (`:57-69`) | Generalise to `zipByPath<T>(repoPaths, values): Record<string, T \| null>`, keeping the existing length-mismatch guard **per call**, so a malformed `repoDirty` cannot drop the branch map. `T = string` -> `RepoBranchByPath`; `T = boolean` -> `RepoDirtyByPath`. **The body's `?? null` must not become `\|\|`**: with `T = string` it is a no-op that looks like dead syntax, but with `T = boolean`, `false ?? null` is `false` (correct) while `false \|\| null` is `null` (wrong), and the difference is invisible on the badge. Pinned by a test (§9.2). |
| `stores/replica-volatile.ts` | `applyDiscoveryBranchUpdate` (`:88-98`) | +1 optional param `repoDirty?: (boolean \| null)[]`, +1 `setField` inside the **existing** `batch()`. Atomicity is the point. Keeping the param optional preserves the existing 2-arg test call at `:151-156`. |
| `stores/replica-volatile.ts` | `clearForPaths` (`:139-161`) | **Preserve `repoDirtyByPath` alongside `repoBranchByPath`** (§2.3). Both captures in the **one** existing `setEntries(key, prev => ...)` callback that reads `prev` raw; restore stays inside the existing `batch`. **The restore guard must become `if (preservedBranch !== undefined \|\| preservedDirty !== undefined)`** - an `&&`, or keeping the single-map condition, silently drops the dirty map. |
| `stores/replica-volatile.ts` | new `effectiveRepoDirtyByPath(replica)` | Mirror `effectiveRepoBranchByPath` (`:190-194`). Needs no change to `ReplicaVolatileBase` (`:178-179`); it reads only `path`. |
| `stores/replica-volatile.ts` | `clearAll` (`:168-175`) | No change: it deletes whole entries. |
| `components/replica-repo-badges.ts` | `configuredReplicaRepoBadges` (`:22-62`) | Accept `repoDirtyByPath?: RepoDirtyByPath`; set `dirty: dirtyByPath?.[sourcePath] ?? null` in the literal at `:55-59`. There is no single-repo shorthand for dirty, so a path miss is `null` (violet), not a fallback. |
| `components/ProjectPanel.tsx` | `configuredReplicaRepoBadgesLive` (`:225-238`) | Pass `repoDirtyByPath: effectiveRepoDirtyByPath(replica)`. |
| `components/ProjectPanel.tsx` | `onDiscoveryBranchUpdated` listener (`:415-424`) | Pass `data.repoDirty`. |
| `components/ProjectPanel.tsx` | badge render (`:2659-2671`) | `` class={`ac-discovery-badge branch${repo.dirty === true ? " dirty" : ""}`} `` and the `title` from §4.6. |
| `styles/sidebar.css` | after `.ac-discovery-badge.branch` (`:5511-5515`) | Add `.ac-discovery-badge.branch.dirty { color: var(--status-exited); }` + a comment naming the (0,3,0) reason and the `AcDiscoveryPanel` blast-radius reason. |

**Correctly not listed**, verified: `ProjectPanel.tsx:142` and `AcDiscoveryPanel.tsx:23` build `SessionRepoInput[]` = `{label, sourcePath}`, not `SessionRepo[]`. `replicaRepoMenuEntries` (`:240`) and `replicaSearchText` (`:1143`) are further `configuredReplicaRepoBadges`/`...Live` callers that need no change: they reach only `label`/`branch` via `formatReplicaRepoBadgeLabel`. `AcDiscoveryPanel.tsx:294` is not touched.

---

## 6. Required behaviour, edge cases, failure behaviour

### 6.1 State table

| Rust `dirty` | Wire | Meaning | Badge |
|---|---|---|---|
| `Some(true)` | `true` | Backend says dirty | **red letters, background unchanged** |
| `Some(false)` | `false` | Backend says clean | violet |
| `None` | `null` | Never successfully detected for this path since process start | violet |

With §4.4, `None` means "never got a first answer", not "flaked once". That is defensible to render violet; rendering the *common* case violet would not be.

### 6.2 Definition of dirty

Exactly the three the issue names: untracked, unstaged, staged-but-uncommitted. All verified in one run.

- **`.gitignore`d files: excluded** (default). Do not pass `--ignored`, or every built repo is permanently red from `target/`.
- **ahead/behind: excluded**, free. With `-b` they appear only inside the `## ` header, and dirty is "any line after the header".
- **Submodules: default behaviour.** A submodule with a moved or dirty HEAD reports ` M <path>` and turns the superproject red. Do **not** pass `--ignore-submodules=dirty`: a moved submodule pointer genuinely is uncommitted work. User decision.

### 6.3 Both writers, or it is broken by design

§2.1. A correctness requirement, not polish.

### 6.4 Failure behaviour

| Case | Detection | `branch` | `dirty` |
|---|---|---|---|
| Success | `Some(GitStatus)` | from header | `Some(scan result)`, and remembered |
| 2s timeout | `None` | `None` (as today) | **held** (§4.4) |
| Not a work tree (`.git` absent) | `None` | `None` (as today) | held; `None` if never detected |
| Ancestor blocked by the ceiling | `None` | `None` | held; `None` if never detected |
| Not a git repo (exit 128) | `None` | `None` (as today) | held |
| Unparseable output (no `## ` first line) | `None` | `None` | held |
| Detached HEAD / mid-rebase | `Some` | `None` | answered normally |
| Unborn branch | `Some` | branch name | answered normally |
| `index.lock` held by someone else | `Some` | normal | normal. `git status` exits 0 either way. |

### 6.5 Ancestor repos

Both halves of the guard, and the measurements, are in §4.2. CASE B is **closed**, not accepted.

### 6.6 Cross-watcher emit-cache coherence (pre-existing, out of scope)

`GitWatcher`'s cache is "last **emitted** by GitWatcher", keyed by session id (`git_watcher.rs:33-35`); Gate B's `repos_cache` is keyed by replica_path (`ac_discovery.rs:438-441`). They do not share. `set_git_repos_if_gen` bumps `git_repos_gen` but touches neither (`manager.rs:701-716`). So the gates compare against **the emitter's own last emit**, while the manager and the UI are **last-writer-wins across three emitters**: a watcher's cache can certify "already sent" for a value the UI is no longer showing.

**The bound (G5).** A degraded value is corrected in <=15s (discovery) or <=5s (GitWatcher) **only if that emitter's next detection succeeds**. If detection **keeps failing**, the emitter recomputes the same degraded value, its own cache matches, `changed == false`, and it never re-emits, while the other watcher stays muted by its own cache even when it knows the truth. So the honest bound is: **<=15s for a transient degradation, unbounded for as long as detection keeps failing.** The round-1 claim of "bounded, not sticky" was false.

This ships today for `branch` and is out of scope. **§4.4 does not mitigate it** and this plan does not claim it does: §4.4 keeps `dirty` correct at the source in the common case, but a permanently-failing path degrades `branch` regardless. The round-1 draft had §6.6 and §4.4 pointing at each other as the mitigation, which was circular.

### 6.7 Performance

No new spawns, threads, dependencies, events, or cadences. `GitWatcher::start` runs a dedicated `std::thread` with its own tokio runtime (`git_watcher.rs:66-69`), so git never occupies the main Tauri runtime's workers; repos are parallel via `join_all`, sessions sequential, and `select!` sleeps then polls so ticks drift rather than pile up. Badge staleness degrades under mass stall; the app does not. See §7.3 for the cost of the `status` call itself, which is **not** uniformly small.

---

## 7. Compatibility, security, and confidence

### 7.1 Compatibility

- **Old `sessions.json`**: no `dirty` key -> `None`. Verified (§4.1).
- **Persisted stale `dirty: true`**: ignored on read -> `None`. That is the decision, verified.
- **The ~6 TS test literals** must add `dirty: null` (G6, a deliberate trade: a compile error in tests buys a compile guard on the one production cold-path builder).
- **Old frontend + new backend**: extra keys ignored by the TS cast.
- **New frontend + old backend**: `dirty` absent -> `undefined` at runtime -> `=== true` is false -> violet. Fails safe. Only reachable in dev/hot-reload, since backend and frontend ship in one bundle.

### 7.2 Security

No new attack surface. The spawn keeps `scrub_credentials_from_tokio_command`, `kill_on_drop(true)`, `CREATE_NO_WINDOW`, and the 2s bound. `--no-optional-locks` **reduces** side effects: the poll no longer writes `.git/index`, and per §4.2 it is what prevents the watcher orphaning `.git/index.lock` in the user's repo. `GIT_CEILING_DIRECTORIES` **reduces** reach: git can no longer walk up into an unrelated ancestor repository. `skip_deserializing` makes `dirty` un-settable from the frontend via `create_session`, so the badge cannot be spoofed by a command payload. No new dependency. No path is logged that is not logged today.

### 7.3 Numbers, at their real confidence

**The `+45 to +62 ms` figure holds only for a stat-refreshed index, and `--no-optional-locks` is what stops the watcher from ever refreshing one (G2).** `git status` normally persists a refreshed `.git/index` so the next run is fast; `--no-optional-locks` forbids that write. So once a repo's index goes **stat-dirty**, the watcher is stuck in the slow path **on every tick, indefinitely**, because it can never persist the refresh that would make it fast. Measured, local SSD, warm cache, git 2.52.0.windows.1:

| tracked | `rev-parse` (today) | nol status, **refreshed** | nol status, **stat-dirty** | plain status, stat-dirty |
|---|---|---|---|---|
| 654 | 78.8 ms | 78.6 ms (**+0**) | 290-323 ms, never recovers | 278 / 273 / **75** recovers on run 2 |
| 3000 | 60.7 ms | 80.1 ms (**+19**) | 1216-1509 ms, never recovers | 1364 / **141** / 115 recovers |
| 6000 | 73.3 ms | 117.8 ms (**+44**) | **2073-2704 ms, over the 2s cap, never recovers** | 2346 / **177** / 110 recovers |

The refreshed column reproduces the original `+45-62ms` measurement exactly, so that measurement is sound and its **generalisation** was not. **Do not claim "the scan is the cheap part"**: that is true only in the refreshed state.

**Bounds, which are why this is not a blocker.** Only files that are **stat-dirty but content-identical** pay this; genuinely-modified files cost the same either way (79/82/76 ms nol vs 83/104/89 ms plain, 50 modified files). At this repo's size the realistic cases are noise: at 654 tracked files, baseline 101.8 ms, 10 touched-but-identical -> 68-71 ms, 50 -> 96-119 ms, 200 -> 164-179 ms (modest but persistent), 6000 -> fatal. Bulk stat-dirty is not an everyday event (`git stash pop`, a formatter or codegen rewriting the tree identically, a OneDrive/Dropbox/backup/AV restore, archive extraction). **`repo-AgentsCommander` is 655 tracked files, so its worst case is ~4x, not a timeout.**

**The interaction with the dormant-rows decision, stated plainly.** In a **live** replica the agent's own `git` commands refresh the index, so a stat-dirty repo self-heals. In a **dormant** replica the watcher is the only git caller, so **there is no self-heal**: a dormant, bulk-stat-dirty, large repo stays in the slow path until something else runs git there. At >=6000 tracked files that means `detect_git_status` times out every tick permanently, the badge never gets a first answer (`dirty` stays `None`, violet, honest), and because `branch` has no hold **it also loses today's working branch suffix indefinitely** - a regression to a shipped feature, in that corner. §3.2 records this as the measured cost of omitting timeout decay.

The other two qualifiers still apply and are not the main variable: the measurements are local-SSD and warm-cache, and they are blind to the network/OneDrive paths that plausibly generate the production timeouts, where `status` (which lstats the whole worktree) versus `rev-parse` (which reads one ref) could be 10-100x rather than 1.4x.

**`~120 detect_branch timeouts/day/path`** (`git_watcher.rs:15-28`) is a **code comment recording someone else's production observation** (#280 §3.3). No telemetry, not reproduced; local runs produce **zero** timeouts, worst case 424ms, 4.7x under the bound. Order of magnitude, not a figure. Any arithmetic on it inherits that.

The false-clean is **correlated with dirtiness, not random**: `git status` is heaviest exactly when the worktree is being written to, which is exactly when the repo is dirty. Timeouts also cluster (I/O spikes, AV scans), so exposure is contiguous multi-tick windows. This is the real argument for §4.4, more than the raw rate - **and §4.4's mechanism must survive a cluster**, which is precisely what the round-1 mechanism failed to do (G1) and what the decided one is tested against.

---

## 8. Implementation order

Each step compiles and is independently reviewable. Steps 1-4 backend, 5-7 frontend, 8 tests.

1. **`SessionRepo.dirty`** + `#[serde(default, skip_deserializing)]` + doc. Add `dirty: None` to all **11** construction sites (§5.1) to restore the build. No behaviour change yet.
2. **Shared detection** in `pty/git_watcher.rs`: `GitStatus`, `parse_status_porcelain_branch`, `remember_dirty`, `detect_git_status` (guard + ceiling + spawn + success gate + parse). Add the parse and `remember_dirty` unit tests here, before any caller uses it.
3. **`GitWatcher`**: retarget its timeout wrapper to `detect_git_status`, delete its `detect_branch`, wire `branch` + the §4.4 call-site shape in `poll`. Live badge now goes red.
4. **`DiscoveryBranchWatcher`**: retarget its timeout wrapper, delete its `detect_branch`, wire `poll` with the same §4.4 shape. Then `DiscoveryBranchPayload.repo_dirty` + populate at Gate A. **After this step the 15s violet flash is impossible**; between steps 3 and 4 it is expected, which is why 3 and 4 must land together in one PR.
5. **Types and wire**: `types.ts` (`SessionRepo.dirty` required, `RepoDirtyByPath`), `ipc.ts` (`DiscoveryBranchUpdate.repoDirty`). This step breaks the ~6 test literals; fix them here.
6. **Volatile layer**: `zipByPath`, entry field, `applyDiscoveryBranchUpdate` param, **`clearForPaths` preserve**, `effectiveRepoDirtyByPath`.
7. **Render**: `replica-repo-badges.ts` literal, `ProjectPanel.tsx` (live wiring, listener arg, class, title), `sidebar.css` variant rule.
8. **Tests** (§9), then `npm run typecheck`, `npm test`, `cargo test`.

---

## 9. Tests and acceptance criteria

### 9.1 Rust unit tests (`src-tauri/src/pty/git_watcher.rs`)

`parse_status_porcelain_branch`, one test per header shape (§4.3):

1. `## master` -> `Some("master")`, `dirty = false`
2. `## master...origin/master` -> `Some("master")`
3. `## master...origin/master [ahead 1, behind 2]` -> `Some("master")`
4. `## HEAD (no branch)` -> `None`
5. `## No commits yet on master` -> `Some("master")`
6. `## No commits yet on master...origin/master [gone]` -> `Some("master")` **(the regression guard for the shape both round-1 reports missed)**

Plus:

7. Dirty scan: header + ` M tracked.txt` / `A  staged.txt` / `?? untracked.txt`, each and together -> `dirty = true`.
8. Clean: header only, trailing newline -> `dirty = false`.
9. Garbage: stdout with no `## ` first line -> `None`.
10. `## feat/a.b-c` -> `Some("feat/a.b-c")`.

`remember_dirty` (§4.4), using **unique-per-test path strings** per the `note_timeout_counts_per_path` precedent:

11. Never detected -> `None`; a failure does not invent a value.
12. `Some(true)` then a **5-tick failure cluster** -> `Some(true)` throughout. **This is G1's regression guard: the mechanism must survive a cluster, not just a blip.**
13. `Some(true)` -> `Some(false)` -> failure -> `Some(false)`. A later success overwrites the memory.
14. Two distinct paths keep independent memories.

Existing `set_git_repos_if_gen_rejects_stale_gen` and `note_timeout_counts_per_path` must still pass.

### 9.2 Frontend tests

- `stores/replica-volatile.test.ts`: **`clearForPaths` preserves `repoDirtyByPath`** as it does `repoBranchByPath`. This is AC 6 and, per §2.3, the highest-risk item in the change, with a HIGH bug already in its history.
- `stores/replica-volatile.test.ts`: **`repoDirty: [false]` yields `{ [REPO_A]: false }`, not `null`.** Not pedantry: `false` and `null` both render violet, so a `||`-for-`??` slip produces a byte-identical badge and only the §4.6 tooltip exposes it (it would say "(status unknown)" on every clean repo in the app). Without this test and the tooltip test, nothing catches it.
- `stores/replica-volatile.test.ts`: `applyDiscoveryBranchUpdate` zips `repoDirty` by path; a length mismatch drops **only** the dirty map and leaves the branch map intact. The two existing tests (`:135-142` drops-on-mismatch, `:151-156` two-arg call) must still pass unchanged.
- `components/replica-repo-badges.test.ts`: `dirty` resolves from `repoDirtyByPath` by path; a path miss yields `null`; reordered `repoPaths` do not transpose dirty onto the wrong repo.
- Badge class: `dirty === true` -> `dirty` class present; `false` / `null` -> absent. `coordinator-badge-class.test.ts` is the precedent for why this is not optional.
- **Badge `title` (§4.6), currently untested:** `true` -> `` `${sourcePath} (uncommitted changes)` ``; `false` -> `sourcePath` unchanged; `null` -> `` `${sourcePath} (status unknown)` ``. This is the assertion that makes the `false`-vs-`null` test observable and the only place the three states are distinguishable.

**No test claims to catch a `repoDirty` name mismatch, and none can.** A frontend test's payload is written to match the TS interface, so it is not evidence about Rust's payload: if Rust emitted `repo_dirty`, the test still passes and production goes permanently violet. **Do not add a Rust wire-shape test either**: `rename_all` is already on `DiscoveryBranchPayload` (`:361-362`), `repo_branches` -> `repoBranches` proves the mapping in production, §5.1/§5.2 pin both names, and no wire-shape test exists anywhere in `ac_discovery.rs` today. Both devs and grinch converge here. The risk does not earn a new test genre.

### 9.3 Acceptance criteria

1. A repo with untracked files, unstaged modifications, or staged-but-uncommitted changes renders its badge **letters** red; background unchanged.
2. A clean repo stays violet.
3. `AcDiscoveryPanel`'s badge is unaffected.
4. Dormant (no running session) **coordinator** rows go red on the same terms (G8: the badge is coordinator-only; a non-coordinator row has no badge at all).
5. No violet flash on the 15s discovery tick.
6. `dirty` survives a replica reload (`clearForPaths`).
7. Unit test per header shape: **6 tests covering 5 shapes plus the unborn-with-upstream variant**.
8. `npm run typecheck` (`tsc --noEmit`) clean; baseline confirmed clean before this work.
9. `npm test` (vitest) and `cargo test` green.
10. A repo path with no `.git`, **or with a corrupt `.git`**, inside a dirty parent repo does **not** render red (§4.2 CASE A and CASE B).
11. A restored session whose persisted `dirty` was `true` does not render red before the first watcher tick (§4.1).
12. A repo that is dirty, then suffers a multi-tick detection failure, **stays red** (§4.4 / §9.1 test 12).

**AC 1 and AC 2 require a real visual check, not green tests.** The class-present test asserts `class="ac-discovery-badge branch dirty"`; it does not assert the letters are red. jsdom does not evaluate the stylesheet, so a misspelled rule, wrong specificity, or wrong property leaves **every test passing and the badge violet**. Check against §4.5: letters `#ff3b5c`, background `rgba(139,92,246,0.15)` unchanged.

---

## 10. Round-1 resolution map and verdict

| # | Finding | Resolution |
|---|---|---|
| **G1** | GitWatcher holds from the shared manager; a cluster kills both holds | **Accepted, fixed differently, and the fix cleared review in round 2.** §4.4 replaced wholesale: a path-keyed memory written only on success. Closes every reset/poisoning row, including the residual grinch's own fix leaves. Grinch withdrew its own proposed fix unprompted ("I have no defence of my version") and PASSed this one on write-reordering, two runtimes, poisoning, the bound, and sharing. |
| **G2** | +45-62ms holds only for a refreshed index | **Accepted.** §7.3 rewritten with the real condition and the full measured table; "the scan is the cheap part" deleted; §3.2 no longer calls timeout decay speculative and records the measured cost instead. |
| **G3** | The `repos.rs` CASE B precedent is an oversight, not a decision | **Accepted, fix adopted.** CASE B closed via `GIT_CEILING_DIRECTORIES` in §4.2; precedent claim withdrawn; moved out of §3.2. I re-verified `.ac/` is not gitignored (312 tracked, 12 dirty now) and added a measurement nobody had: the ceiling also leaves a **linked worktree** (`.git` as a file) unharmed. |
| **G4** | Three emitters, not two | **Accepted.** §2.1 table has the third row with its cadence and bound; §5.1's `:2139` row annotated. |
| **G5** | "Bounded, not sticky" false under sustained failure | **Accepted.** §6.6 restated as "<=15s transient, unbounded while detection keeps failing"; the circular §4.4/§6.6 cross-reference removed. |
| **G6** | `dirty?:` discards the compile-forcing | **Accepted, dev-webpage-ui overruled.** `dirty: boolean \| null` required, matching the `branch` precedent. Cost is ~6 test literals (§7.1). |
| **G7** | `--no-optional-locks` is load-bearing | **Accepted.** §4.2 relabelled with the 8/8 measurement and cross-referenced to its cost in §7.3. |
| **G8** | Badge is coordinator-only | **Accepted.** §1, §3.1, AC 4 fixed. |
| dev-rust | §4.4 compile error; snapshot before `:643`; 4 reset points | **Moot.** §4.4's new mechanism has no snapshot, no borrowed keys, no timing constraint, no reset points. |
| dev-rust | 11 sites not 10; `[` refname rule; `out.status.success()`; five deserialize consumers + `read_sessions_file` note; cite `:937` | **All adopted** (§5.1, §4.3, §4.2, §4.1). |
| dev-webpage-ui | Delete the false §9.2 claim; no Rust wire test; add `false`-survives and tooltip tests; AC 1/2 visual; `clearForPaths` `\|\|` guard; downgrade the camelCase risk | **All adopted** (§9.2, §4.5, §5.2). |

### Round 2 (§4.4 only)

| # | Finding | Resolution |
|---|---|---|
| **R1** | `fn remember_dirty` is private; the cross-module call is `error[E0603]`. Reproduced by building the real module structure | **Fixed.** `pub(crate) fn`. Both reviewers flagged it independently; this is what the round bought. |
| **R2** | §5.1 left visibility as an open branch, and the plan was self-inconsistent (code block private, call site cross-module) | **Closed to `pub(crate)`, forced not preferred.** `tokio::time::timeout` **drops** the `detect_git_status` future on timeout, so it never returns and cannot remember. The hold must live where the failure is observed: each watcher's wrapper, one of which is in `ac_discovery.rs`. No arrangement keeps the map private and still holds on the failure path. |
| **R3** | `&source_path` is not in scope inside the `SessionRepo { .. }` literal | **Fixed.** `&r.source_path` in `GitWatcher::poll`, `path` in `DiscoveryBranchWatcher::poll`. |
| **R4** | `pub(crate)` widens who can violate the invariant, which `remember_dirty` does not enforce | **Adopted.** Doc comment names `detect_git_status`'s three gates as the enforcer and `detect_git_branch_sync` (no guard, no ceiling) as the shape of a caller that would break it. Nothing reachable today. |
| **R5** | The sharing rationale was weaker than the real one | **Adopted.** §2.1 *requires* sharing: separate maps let the two watchers hold different values during a cluster, both write and both emit, re-admitting the flicker §2.1 calls broken by design. The CAS is not a backstop. Also settled: `note_timeout`'s "~150, no GC" bound transfers **exactly** (its comment bounds by distinct repo paths, the same key space), and separating the counters is right for a throttle and irrelevant for a fact about a worktree. |
| **R6** | Stale-success residual: the map write bypasses the CAS, so a CAS-rejected value is already committed | **Adopted as an accepted residual.** Inherent, not fixable (the construction site needs the held value to build `refreshed`, so the dependency is circular; and discovery's CAS is inside `if let Some(session_id)`, so moving the call there would never remember dormant paths). Bounded by measurement: 0/4 inversions at 6000 files / 1s stagger, max 21ms injectable at 654 files, because both watchers run the identical command on the identical path so start order is completion order. Errs to a **false-red**, the direction §4.4 chooses. |
| **R7** | "Can never be an ancestor's" is absolute over an incomplete gate list; `GIT_DIR`/`GIT_WORK_TREE` bypass discovery | **Adopted, scoped to discovery.** I reproduced it, including that it is **pre-existing**: today's `rev-parse` with `GIT_DIR` set returns the ancestor's branch identically. A sentence, not a code change; §4.4 does not amplify it; `env_remove` explicitly not wanted. |

### Verdict: READY_FOR_IMPLEMENTATION

Every finding from both rounds is integrated. Every section is closed. The plan carries no TBD, no competing alternative, and no choice left to the implementer.

§4.4 was the section every party had got wrong, and spending round 2 on it was right: it found a reproduced compile error (R1), a self-inconsistency between the code block and the call site (R2), a scope error at both call sites (R3), and two claim-precision defects (R6, R7) that no amount of my own re-reading would have surfaced. Both reviewers cleared the mechanism itself, and grinch withdrew its own competing fix without being asked.

The three things a cold-start implementer must not "simplify", each with its reason recorded at the site: `pub(crate)` on `remember_dirty` (R2: the hold cannot work on the failure path otherwise), the **shared** map (R5: separate maps re-admit the §2.1 flicker), and `--no-optional-locks` (§4.2/G7: it is what stops the 2s `kill_on_drop` orphaning `index.lock` and breaking the agent's commits).

**Two claims in this plan are scoped rather than absolute, and the scoping is load-bearing.** §4.4's "cannot be an ancestor's" covers repo *discovery* and not `GIT_DIR`/`GIT_WORK_TREE` (R7, pre-existing, no realistic vector). §6.6's bound is "<=15s transient, unbounded while detection keeps failing" (G5), and §4.4 does **not** mitigate it. Neither was scoped to be safe; both were absolute in an earlier draft and measurement showed the absolute form was false.

**Numbers carry their conditions, per the tech-lead's constraint.** The `+45-62ms` holds only for a stat-refreshed index (§7.3). The `~120 timeouts/day/path` is an unreproduced code comment, an order of magnitude and not a figure (§7.3). Grinch's stagger and duration figures bound a **race window, not a production rate**, on local SSD with a warm cache and synthetic repos (§4.4). #1029 (dedupe) stays out.
