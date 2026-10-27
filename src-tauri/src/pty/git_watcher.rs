use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures::stream::StreamExt;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::config::settings::AppSettings;
use crate::session::manager::SessionManager;
use crate::session::session::SessionRepo;

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const DETECT_TIMEOUT: Duration = Duration::from_secs(2);

/// #1298 - read-site clamp bounds for the two sweeper dials. See plan D1.
/// `SWEEP_MIN_INTERVAL_SECS` is also the floor an EMPTY round applies, which is
/// what makes the cold start pick up work about a second after it appears.
const SWEEP_MIN_CONCURRENCY: u8 = 1;
const SWEEP_MAX_CONCURRENCY: u8 = 4;
const SWEEP_MIN_INTERVAL_SECS: u64 = 1;
const SWEEP_MAX_INTERVAL_SECS: u64 = 3600;

/// #280 §3.3 - per-path counter for `detect_git_status` timeouts (#1028 renamed
/// the detection; the counter and its policy are unchanged). Used to dampen
/// the WARN storm by logging only the first occurrence per path plus every
/// 50th thereafter. Process-local: counts reset on restart. Bounded by the
/// number of distinct repo paths (~150 in production observations), so no
/// GC is needed.
///
/// #1298 - now the sweeper's SINGLE counter rather than one of two: the
/// per-watcher twin in `commands/ac_discovery.rs` went with its subject when
/// detection was merged into one producer.
fn note_timeout(path: &str) -> u64 {
    static TIMEOUT_COUNTS: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    let map = TIMEOUT_COUNTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = map.lock().unwrap_or_else(|e| e.into_inner());
    let count = g.entry(path.to_string()).or_insert(0);
    *count += 1;
    *count
}

/// #1028/#1078 - one local repository status detection: the branch from the
/// `## ` header plus whether local work is confirmed by cached origin tracking.
///
/// `dirty` is true when Git emits a non-ignored worktree/index entry, or when the
/// checked-out `HEAD` is not confirmed reachable from the current branch's configured,
/// locally cached `origin/*` upstream. This detector never performs a remote operation.
/// It remains deliberately separate from the blocking deletion-safety checks in
/// `commands/entity_creation.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitStatus {
    /// `None` for a detached HEAD or a mid-rebase, matching what
    /// `rev-parse --abbrev-ref` reported before this replaced it.
    pub(crate) branch: Option<String>,
    pub(crate) dirty: bool,
}

/// #1028 - last SUCCESSFULLY detected worktree-dirty state per repo path, so a
/// transient detection failure holds the previous answer instead of asserting a
/// confident "clean". The errors are asymmetric: a false clean means shipping
/// uncommitted work (silent, costly), a false red self-corrects on the next tick
/// (cheap). `branch` deliberately keeps its old no-hold behaviour.
///
/// Only successful detections are ever written, so no failure can poison the map
/// and there is nothing to reset. Keyed by repo path, not by session or replica:
/// dirty is a property of the worktree, and several replicas legitimately share one
/// repo dir. Process-local, so it resets on restart, which is correct: an unknown
/// repo is `None` (violet) until its first answer. Bounded by the number of distinct
/// repo paths (~150 in production observations), so no GC is needed.
///
/// `pub(crate)`, and that is FORCED, not preferred: on a timeout `tokio::time::timeout`
/// DROPS the `detect_git_status` future, so it never returns and cannot remember
/// anything. The hold must therefore be applied where the failure is observed, which is
/// each watcher's own timeout wrapper, and one of those lives in
/// `commands/ac_discovery.rs`. No arrangement keeps this map module-private and still
/// holds on the failure path, which is the only path it exists for.
///
/// Deliberately SHARED by both watchers, diverging from `note_timeout` above, which is
/// deliberately per-watcher. That split is right for a throttle counter and irrelevant
/// for a fact about a worktree. Sharing is REQUIRED here: with separate maps, a timeout
/// cluster hitting both watchers leaves their holds sourced from successes up to 5s and
/// up to 15s old, so if the repo changed in between the two watchers hold DIFFERENT
/// values, both write divergent `dirty` into `git_repos`, and both emit (each compares
/// against its own cache). That is a red/violet badge flicker. The CAS is not a backstop:
/// it only suppresses discovery when GitWatcher's write lands inside discovery's
/// detection window. Sharing makes the two watchers agree by construction.
///
/// `.unwrap_or_else(|e| e.into_inner())` is required, not stylistic: `.unwrap()` would
/// take BOTH watchers down permanently on a single poisoning.
///
/// INVARIANT, and it is NOT enforced here: this function remembers whatever it is
/// handed. "Never remembers an ancestor's dirt" is enforced by the caller,
/// `detect_git_status`, through its three gates (`.git` metadata check, ceiling,
/// `status.success()`). Only ever feed this from `detect_git_status`. A caller shaped
/// like `detect_repo_branch` (`commands/ac_discovery.rs`), which has no `.git` guard
/// and no ceiling, would silently violate it. Nothing does today.
pub(crate) fn remember_dirty(path: &str, detected: Option<bool>) -> Option<bool> {
    let mut g = last_dirty_map().lock().unwrap_or_else(|e| e.into_inner());
    match detected {
        Some(d) => {
            g.insert(path.to_string(), d);
            Some(d)
        }
        None => g.get(path).copied(),
    }
}

/// #1298 - `LAST_DIRTY` is hoisted out of `remember_dirty`'s body so
/// `retain_swept_paths` can evict from it too. See INV-8: this map and the
/// published snapshot must gain and lose the same paths, or a dropped path reads
/// `None` from the snapshot and its last successful `bool` from here, which is a
/// confident colour sourced from a detection that will never be repeated.
static LAST_DIRTY: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();

fn last_dirty_map() -> &'static Mutex<HashMap<String, bool>> {
    LAST_DIRTY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// #1298 - the snapshot the single global sweeper publishes and both poll loops
/// read instead of spawning `git` themselves. Keyed by the EXACT repo-path string
/// its two sources handed the sweeper (INV-3): any normalization here silently
/// turns a consumer read into a miss, which renders as a permanent violet badge
/// with no error anywhere.
///
/// Storing `Option<GitStatus>` rather than removing the key on failure is
/// deliberate: "never detected" and "last detection failed" stay indistinguishable
/// to the reader, which is exactly the semantics both consumers already implement
/// (a `None` status becomes `branch: None` plus `remember_dirty(path, None)`).
///
/// Same lock discipline as `remember_dirty`, including
/// `.unwrap_or_else(|e| e.into_inner())`, which is required for the reason
/// documented there. GC is `retain_swept_paths`, not absence of one.
static GIT_STATUS_SNAPSHOT: OnceLock<Mutex<HashMap<String, Option<GitStatus>>>> = OnceLock::new();

fn git_status_snapshot() -> &'static Mutex<HashMap<String, Option<GitStatus>>> {
    GIT_STATUS_SNAPSHOT.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn publish_git_status(path: &str, status: Option<GitStatus>) {
    let mut g = git_status_snapshot()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    g.insert(path.to_string(), status);
}

/// `pub(crate)` is FORCED, exactly as it is for `remember_dirty`: one of the two
/// readers lives in `commands/ac_discovery.rs`.
pub(crate) fn read_git_status(path: &str) -> Option<GitStatus> {
    let g = git_status_snapshot()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    g.get(path).cloned().flatten()
}

/// #1298 - the discovery half of the sweeper's work list, PUSHED here by
/// `DiscoveryBranchWatcher` whenever it mutates its `replicas` map. Push, not
/// pull: pulling would make `pty::git_watcher` import a `commands::` type, which
/// is a layering inversion. Duplicates are expected (several replicas name the
/// same repo) and `build_work_list` dedupes them.
static DISCOVERY_REPO_PATHS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn discovery_repo_paths_cell() -> &'static Mutex<Vec<String>> {
    DISCOVERY_REPO_PATHS.get_or_init(|| Mutex::new(Vec::new()))
}

pub(crate) fn set_discovery_repo_paths(paths: Vec<String>) {
    *discovery_repo_paths_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = paths;
}

pub(crate) fn discovery_repo_paths() -> Vec<String> {
    discovery_repo_paths_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// #2064 - the room half of the map `remote_watcher` needs: `(repo_path,
/// room_dir)`, pushed by the same `with_replicas_mut` call that pushes
/// `DISCOVERY_REPO_PATHS`. `room_dir` is the parent of the replica path, so no
/// new filesystem discovery is involved. Same push-not-pull reason as above, and
/// `GitSweeper` never reads this map.
static DISCOVERY_REPO_ROOMS: OnceLock<Mutex<Vec<(String, String)>>> = OnceLock::new();

fn discovery_repo_rooms_cell() -> &'static Mutex<Vec<(String, String)>> {
    DISCOVERY_REPO_ROOMS.get_or_init(|| Mutex::new(Vec::new()))
}

pub(crate) fn set_discovery_repo_rooms(pairs: Vec<(String, String)>) {
    *discovery_repo_rooms_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = pairs;
}

/// `pub(crate)` rather than `pub(super)` (both this getter and
/// `discovery_repo_paths` above): the ac_discovery tests assert that this map
/// mirrors the pushed path vector, and a `pub(super)` accessor would be
/// invisible from `commands::`. Production readers of both still live in `pty`.
pub(crate) fn discovery_repo_rooms() -> Vec<(String, String)> {
    discovery_repo_rooms_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// #1298 - drop every path this sweep no longer covers, from BOTH process-global
/// maps. See INV-8: evicting only the snapshot leaves `LAST_DIRTY` serving the
/// last successful bool forever, which is a confident lie rather than an
/// "unknown".
///
/// Takes the two guards SEQUENTIALLY, never nested: nesting them would introduce
/// the tree's only two-lock order between these maps for no benefit.
fn retain_swept_paths(live: &HashSet<String>) {
    {
        let mut snapshot = git_status_snapshot()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        snapshot.retain(|path, _| live.contains(path));
    }
    let mut dirty = last_dirty_map().lock().unwrap_or_else(|e| e.into_inner());
    dirty.retain(|path, _| live.contains(path));
}

/// Return whether a successful porcelain header confirms that `HEAD` is reachable
/// from the current branch's configured, locally cached `origin/*` upstream.
///
/// Only a synchronized header or an exact positive behind-only marker confirms that
/// relation. Every missing, detached, unborn, non-origin, gone, ahead, diverged, or
/// malformed shape fails closed.
fn cached_origin_contains_head(header: &str) -> bool {
    if header == "HEAD (no branch)" || header.starts_with("No commits yet on ") {
        return false;
    }

    let Some((branch, upstream_and_tracking)) = header.split_once("...") else {
        return false;
    };
    let branch = branch.trim();
    if branch.is_empty() || branch == "HEAD" {
        return false;
    }

    // Split only at the tracking marker boundary. Spaces and `[` are forbidden in
    // refnames, so this cannot truncate a valid upstream name.
    let (upstream, tracking) = match upstream_and_tracking.split_once(" [") {
        Some((upstream, tracking)) => (upstream, Some(tracking)),
        None => (upstream_and_tracking, None),
    };
    let Some(target) = upstream.strip_prefix("origin/") else {
        return false;
    };
    if target.is_empty() {
        return false;
    }

    match tracking {
        None => true,
        Some(tracking) => {
            let Some(count) = tracking
                .strip_prefix("behind ")
                .and_then(|value| value.strip_suffix(']'))
            else {
                return false;
            };
            !count.is_empty()
                && count.bytes().all(|byte| byte.is_ascii_digit())
                && count.parse::<u64>().is_ok_and(|value| value > 0)
        }
    }
}

/// #1028/#1078 - parse
/// `git status --porcelain --branch --untracked-files=normal --ahead-behind` output.
///
/// Git builds the header as `## ` + (`No commits yet on ` if unborn) + branch +
/// (`...` + upstream + optional ` [...]` if an upstream is configured). Splitting on
/// `...` and ` [` is safe because Git's refname rules reject those sequences inside a
/// real branch name. Single dots and slashes (`feat/a.b-c`) survive branch extraction.
pub(crate) fn parse_status_porcelain_branch(stdout: &str) -> Option<GitStatus> {
    let mut lines = stdout.lines();
    // Output we do not understand: do not guess.
    let header = lines.next()?.strip_prefix("## ")?;

    let has_porcelain_entries = lines.any(|line| !line.trim().is_empty());
    let dirty = has_porcelain_entries || !cached_origin_contains_head(header);

    // (a) detached / mid-rebase. Must short-circuit before (b).
    let branch = if header == "HEAD (no branch)" {
        None
    } else {
        // (b) unborn: strip the prefix and CONTINUE, which is what makes the
        // unborn-with-upstream shape parse.
        let rest = header.strip_prefix("No commits yet on ").unwrap_or(header);
        // (c) drop the upstream.
        let rest = rest.split_once("...").map_or(rest, |(b, _)| b);
        // (d) defensively drop a tracking suffix. Unreachable after (c) with today's
        // git (a marker implies an upstream implies `...`), and safe: `[` is forbidden
        // in refnames, so this cannot eat a real name.
        let rest = rest.split_once(" [").map_or(rest, |(b, _)| b);
        // (e) unreachable per the refname rule (git rejects a branch named `HEAD`);
        // kept for parity with the `rev-parse` implementation this replaced.
        let rest = rest.trim();
        if rest.is_empty() || rest == "HEAD" {
            None
        } else {
            Some(rest.to_string())
        }
    };

    Some(GitStatus { branch, dirty })
}

/// #1028/#1078 - detect branch and local work not confirmed by cached origin tracking
/// for `working_dir` in one local Git call. Shared by both badge writers (`GitWatcher`
/// and `DiscoveryBranchWatcher`), which previously
/// kept near-identical `detect_branch` copies that had already diverged: one set the
/// ceiling env, the other did not. Both writers must compute `dirty` identically or the
/// badge flickers, and duplicating the parse is how that guarantee gets lost.
///
/// #1298 - one caller now: the global `GitSweeper`, through the single
/// `detect_status_with_timeout` free function below, logging under `[GitSweeper]`.
/// INV-1: that wrapper's `DETECT_TIMEOUT` is the ONLY bound on a sweeper round,
/// whose worst case is `N * DETECT_TIMEOUT` at concurrency 1. Removing it because
/// "the loop is sequential now" would let one hung repo (dead share, stale
/// `index.lock`) freeze freshness for every other repo forever, with no recovery
/// short of restarting the app. `kill_on_drop(true)` below is part of that
/// invariant: the timeout is what drops this future, and the drop is what kills
/// `git.exe`.
///
/// `--no-optional-locks` is LOAD-BEARING, not hygiene. Do not drop it in a cleanup. A
/// plain `status` takes `.git/index.lock` (a ~13ms window) and rewrites `.git/index`;
/// with the caller's 2s timeout dropping this future, `kill_on_drop` TerminateProcess-es
/// git, and a kill inside that window orphaned the lock 8/8 in measurement. An orphaned
/// `index.lock` leaves `git status` at exit 0 and silent while the user's own `git add`
/// and `git commit` hard-fail: the badge keeps painting while their commits break. The
/// flag also stops the poll writing `.git/index` every tick. Its cost is that a
/// stat-dirty index can never be refreshed by us, so such a repo stays on the slow
/// status path; that is measured and accepted.
///
/// Two gates keep git from answering with an UNRELATED ancestor repository's status. The
/// replica tree sits inside a parent repo that AC's own operation keeps permanently
/// dirty, so a false red from up there would never self-clear:
/// 1. `.git` metadata check: rejects a path with no `.git` at all. `metadata`, not
///    `is_dir`, because linked worktrees and submodules use a `.git` FILE. `tokio::fs`,
///    not `std::fs`, because a sync stat inside an async fn cannot be cancelled by
///    `tokio::time::timeout`: on a dead share it would wedge a runtime worker and make
///    the caller's 2s cap a lie.
/// 2. `GIT_CEILING_DIRECTORIES` = the repo path's parent: rejects a path whose `.git`
///    EXISTS but is corrupt or empty (an interrupted clone), which the metadata check
///    passes and which otherwise walks up and reports the ancestor's dirt. Measured:
///    corrupt `.git` + this ceiling gives `fatal: not a git repository`, exit 128, while
///    a healthy repo, a linked worktree whose main repo lives above the ceiling, and a
///    dirty submodule superproject are all unharmed. The ceiling constrains the
///    discovery ascent only, not gitdir resolution through a `.git` file.
///
/// This deliberately does NOT call `git_ceiling_directories_for_session_root`: that
/// helper returns `None` for every path a watcher passes it (its `is_agent_dir` gate
/// rejects `repo-*` paths), i.e. it is a no-op that reads like a guard. Replacing a
/// guard that cannot fire with one that does is not re-adding it.
///
/// Returns `None` on every failure path, which the callers turn into "hold the last
/// known dirty" via `remember_dirty` and "branch unknown" for `branch`.
pub(crate) async fn detect_git_status(working_dir: &str) -> Option<GitStatus> {
    // Gate 1: require a work tree ROOT; do not let git discover a repo by walking up.
    if crate::filesystem::repositories::git_root_metadata(&Path::new(working_dir).join(".git"))
        .await
        .is_err()
    {
        return None;
    }

    match crate::process::git::watcher::watcher_status_output(working_dir).await {
        // Gate 3: retained from the two `detect_branch` copies. A failed git writes
        // `fatal:` to stderr and leaves stdout empty, so the parser would return `None`
        // anyway; dropping the gate would be a silent behaviour change, not a fix.
        Ok(out) if out.status.success() => {
            parse_status_porcelain_branch(&String::from_utf8_lossy(&out.stdout))
        }
        _ => None,
    }
}

pub struct GitWatcher {
    session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    /// `None` only for the #2683 headless test constructor; production always
    /// stores `Some` through `new`.
    app_handle: Option<AppHandle>,
    /// Last-emitted per-repo state keyed by session id. Equality gate for `session_git_repos`.
    /// `Vec` equality is order-sensitive; callers preserve replica config.json `repos` order.
    cache: Mutex<HashMap<Uuid, Vec<SessionRepo>>>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GitReposPayload {
    session_id: String,
    repos: Vec<SessionRepo>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CoordinatorChangedPayload {
    pub session_id: String,
    pub is_coordinator: bool,
}

impl GitWatcher {
    pub fn new(
        session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
        app_handle: AppHandle,
    ) -> Arc<Self> {
        Arc::new(Self {
            session_manager,
            app_handle: Some(app_handle),
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// #2683 - a watcher with no `AppHandle`, for the unix `spawn_sync` tests
    /// that cannot build a `tauri::App` on a headless runner. It never emits.
    #[cfg(test)]
    pub(crate) fn new_headless_for_tests(
        session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            session_manager,
            app_handle: None,
            cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn start(self: &Arc<Self>, shutdown: crate::shutdown::ShutdownSignal) {
        let watcher = Arc::clone(self);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime for GitWatcher");
            rt.block_on(async move {
                loop {
                    tokio::select! {
                        biased;
                        _ = shutdown.token().cancelled() => {
                            log::info!("[GitWatcher] Shutdown signal received, stopping");
                            break;
                        }
                        _ = tokio::time::sleep(POLL_INTERVAL) => {
                            watcher.poll().await;
                        }
                    }
                }
            });
        });
    }

    pub fn remove_session(&self, id: Uuid) {
        self.cache.lock().unwrap().remove(&id);
    }

    /// Force a re-emit on the next tick for `id`. Called by
    /// `refresh_git_repos_for_sessions` callers so their newly-written `git_repos`
    /// isn't silently skipped by a stale cache hit.
    pub fn invalidate_session_cache(&self, id: Uuid) {
        self.cache.lock().unwrap().remove(&id);
    }

    async fn poll(&self) {
        let sessions: Vec<(Uuid, Vec<SessionRepo>, u64)> = {
            let mgr = self.session_manager.read().await;
            mgr.get_sessions_repos().await
        };

        for (id, repos, gen_snapshot) in sessions {
            if repos.is_empty() {
                // Nothing to watch. If cache still has this id, clear it so a later
                // "repos appeared" transition re-emits.
                let mut cache = self.cache.lock().unwrap();
                cache.remove(&id);
                continue;
            }

            // #1298 - pure map reads. This loop no longer spawns `git`: the global
            // `GitSweeper` is the only polling producer, and this poll consumes its
            // published snapshot. An unpublished path reads `None`, which is the
            // same "unknown" a failed detection produced before.
            let statuses: Vec<Option<GitStatus>> = repos
                .iter()
                .map(|r| read_git_status(&r.source_path))
                .collect();

            let refreshed: Vec<SessionRepo> = repos
                .iter()
                .zip(statuses)
                .map(|(r, status)| SessionRepo {
                    label: r.label.clone(),
                    source_path: r.source_path.clone(),
                    // #1028 - `branch` keeps its old behaviour exactly: a failed
                    // detection reports `None`, with no hold.
                    branch: status.as_ref().and_then(|s| s.branch.clone()),
                    // #1028 - `dirty` DOES hold across a failure, because asserting a
                    // confident "clean" while the worktree has uncommitted work is the
                    // one error this feature must not make. Same shape in
                    // DiscoveryBranchWatcher::poll; the map behind it is shared.
                    dirty: remember_dirty(&r.source_path, status.as_ref().map(|s| s.dirty)),
                })
                .collect();

            let changed = {
                let cache = self.cache.lock().unwrap();
                cache.get(&id) != Some(&refreshed)
            };

            if changed {
                // CAS write — if a refresh bumped the gen between our snapshot and now,
                // the write + emit are skipped. Prevents the stale-overwrite race (Grinch #14).
                let wrote = {
                    let mgr = self.session_manager.read().await;
                    mgr.set_git_repos_if_gen(id, refreshed.clone(), gen_snapshot)
                        .await
                };

                if wrote {
                    if let Some(app_handle) = &self.app_handle {
                        let _ = app_handle.emit(
                            "session_git_repos",
                            GitReposPayload {
                                session_id: id.to_string(),
                                repos: refreshed.clone(),
                            },
                        );
                    }
                    self.cache.lock().unwrap().insert(id, refreshed);
                } else {
                    log::debug!(
                        "[GitWatcher] gen mismatch on session {} — refresh landed during poll; \
                        skipping stale emit",
                        id
                    );
                    // Invalidate our cache so the next tick re-evaluates against the
                    // refreshed list.
                    self.cache.lock().unwrap().remove(&id);
                }
            }
        }
    }
}

/// Detect branch + worktree-dirty via the shared `detect_git_status`, bounded by a
/// 2s timeout. On timeout the pending future is dropped; `.kill_on_drop(true)`
/// ensures the child `git.exe` is terminated so repeated polls can't leak processes.
///
/// #1298 - was one of two per-watcher methods. It is now the sweeper's single
/// wrapper, and its `DETECT_TIMEOUT` is INV-1: see `detect_git_status` above for
/// why it must survive any later cleanup.
///
/// Because `timeout` DROPS the future here, `detect_git_status` never returns on this
/// path and cannot remember anything itself: the caller applies the dirty hold.
async fn detect_status_with_timeout(working_dir: &str) -> Option<GitStatus> {
    match tokio::time::timeout(DETECT_TIMEOUT, detect_git_status(working_dir)).await {
        Ok(result) => result,
        Err(_) => {
            let n = note_timeout(working_dir);
            // #280 §3.3 — throttle the per-path WARN. Observed rate is
            // ~120 timeouts/day/path; logging every one floods app.log.
            // Log the first sighting (heartbeat) and every 50th
            // thereafter (still ~2-3 WARN/day/path) — enough signal to
            // notice persistent slowness without spamming.
            if n == 1 || n.is_multiple_of(50) {
                log::warn!(
                    "[GitSweeper] detect_git_status timed out for {} (>{}s); occurrence={} \
                    (logging 1st + every 50th)",
                    working_dir,
                    DETECT_TIMEOUT.as_secs(),
                    n
                );
            }
            None
        }
    }
}

/// #1298 - read-site clamp for the two sweeper dials. See plan D1: clamping here
/// rather than in `validate_and_repair_settings` is what stops the app from
/// rewriting the user's hand-edited `settings.json`. It is NOT a hot-reload
/// mechanism: the sweeper reads the in-memory settings state, which is loaded once
/// at startup and never refreshed from disk, so changing either dial takes a
/// RESTART.
fn sweep_dials(cfg: &AppSettings) -> (usize, Duration) {
    let concurrency = cfg
        .git_sweep_concurrency
        .clamp(SWEEP_MIN_CONCURRENCY, SWEEP_MAX_CONCURRENCY);
    let interval_secs = cfg
        .git_sweep_min_interval_secs
        .clamp(SWEEP_MIN_INTERVAL_SECS, SWEEP_MAX_INTERVAL_SECS);

    if concurrency != cfg.git_sweep_concurrency {
        log::debug!(
            "[GitSweeper] gitSweepConcurrency {} clamped to {}",
            cfg.git_sweep_concurrency,
            concurrency
        );
    }
    if interval_secs != cfg.git_sweep_min_interval_secs {
        log::debug!(
            "[GitSweeper] gitSweepMinIntervalSecs {} clamped to {}",
            cfg.git_sweep_min_interval_secs,
            interval_secs
        );
    }

    (concurrency as usize, Duration::from_secs(interval_secs))
}

/// #1298 - the sweeper's work list: the UNION of the discovery replicas and the
/// live sessions, deduplicated by EXACT path string (INV-3, no normalization),
/// with archived projects dropped (D2), sorted for a deterministic round order.
/// `archived_roots` are already normalized.
///
/// INV-7: neither half may be dropped. Discovery alone misses sessions in
/// workgroups whose project is not in `settings.projectPaths`; sessions alone
/// lose dormant replicas, which is `DiscoveryBranchWatcher`'s reason to exist.
///
/// NOT pure and NOT cheap: with a non-empty `archived_roots` this performs one
/// `std::fs::canonicalize` per distinct candidate, through
/// `is_under_normalized_archived_roots`. It must therefore be called on the
/// blocking pool whenever `archived_roots` is non-empty (INV-1). With an empty
/// `archived_roots` the predicate short-circuits with zero syscalls and the call
/// is a `HashSet` plus a `sort`.
///
/// Dedupe FIRST, filter SECOND. The result is identical either way, because the
/// predicate is deterministic per path string, so this pays one canonicalize per
/// DISTINCT path instead of one per occurrence: about 24 instead of about 200 on
/// this layout, which is the same factor applied to the uncancellable blocking
/// tail at shutdown. Do not fold it back into one chained `filter`.
///
/// Sorting is not cosmetic: at concurrency 1 the order decides who waits behind a
/// hung repo, so a deterministic order makes that reproducible. Consequence worth
/// knowing before reading a log: the alphabetically first path sits permanently at
/// the head of every round. That pattern is expected, not a bug.
pub(super) fn build_work_list(
    discovery: Vec<String>,
    sessions: Vec<String>,
    archived_roots: &[String],
) -> Vec<String> {
    let mut paths: Vec<String> = discovery
        .into_iter()
        .chain(sessions)
        .collect::<HashSet<String>>()
        .into_iter()
        .filter(|p| {
            !crate::config::sessions_persistence::is_under_normalized_archived_roots(
                p,
                archived_roots,
            )
        })
        .collect();
    paths.sort();
    paths
}

/// #1298 - the single global POLLING producer of per-repo git state. Both watchers
/// read the snapshot it publishes instead of spawning `git` themselves.
///
/// Takes `SettingsState` rather than an `AppHandle` on purpose: the sweeper emits
/// nothing, so it must not acquire a Tauri transport dependency it does not need.
/// It unifies DETECTION, not EMISSION (INV-4): the two watchers keep their own
/// events, keys, cadences and emission gates.
pub struct GitSweeper {
    session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    settings: crate::config::settings::SettingsState,
}

impl GitSweeper {
    pub fn new(
        session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
        settings: crate::config::settings::SettingsState,
    ) -> Arc<Self> {
        Arc::new(Self {
            session_manager,
            settings,
        })
    }

    /// A dedicated thread owning its own runtime, like `GitWatcher::start`, so the
    /// sweep never competes with the Tauri runtime that serves the UI. NOT the same
    /// runtime FLAVOUR: `Runtime::new()` builds one worker per core (24 on the
    /// machine in #1298) and production already runs four such runtimes, so a fifth
    /// to drive a loop whose design point is doing one thing at a time is
    /// indefensible. One worker is sufficient by construction: `buffer_unordered`
    /// needs concurrency, not parallelism; the work is process spawning and I/O, and
    /// `enable_all()` installs the I/O and time drivers a current-thread runtime
    /// needs; and `spawn_blocking` runs on the separate blocking pool regardless.
    ///
    /// The round itself sits inside the shutdown `select!`. Without it, worst-case
    /// shutdown latency would be `N * DETECT_TIMEOUT`. Cancelling mid-round drops
    /// the in-flight detection, and `kill_on_drop` terminates its `git.exe`.
    pub fn start(self: &Arc<Self>, shutdown: crate::shutdown::ShutdownSignal) {
        let sweeper = Arc::clone(self);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime for GitSweeper");
            rt.block_on(async move {
                loop {
                    // `started` is taken BEFORE the round, which is what makes the
                    // period max(floor, round duration) and what keeps the first
                    // round from being delayed (plan D3).
                    let started = std::time::Instant::now();
                    let floor = tokio::select! {
                        biased;
                        _ = shutdown.token().cancelled() => break,
                        floor = sweeper.round() => floor,
                    };
                    let remaining = floor.saturating_sub(started.elapsed());
                    tokio::select! {
                        biased;
                        _ = shutdown.token().cancelled() => break,
                        _ = tokio::time::sleep(remaining) => {}
                    }
                }
                log::info!("[GitSweeper] Shutdown signal received, stopping");
            });
        });
    }

    /// One sweep over the whole work list, returning the floor to apply afterwards
    /// so both dials are read exactly once per round.
    ///
    /// An EMPTY round floors at the clamped minimum instead of the configured floor
    /// (INV-2): a round with no work takes about 0ms, so an unfloored loop would
    /// spin a core, and a full-length floor would leave the cold start blind for a
    /// whole interval. Neither floor may be 0.
    async fn round(&self) -> Duration {
        let round_started = std::time::Instant::now();

        // The settings read guard must NOT outlive this block: it is a
        // `tokio::sync::RwLock`, so holding it across an await compiles fine and
        // would block every `save_settings` write for the whole round.
        let (concurrency, floor, archived) = {
            let cfg = match self.settings.current_read().await {
                Ok(cfg) => cfg,
                Err(_) => {
                    log::debug!("[GitSweeper] settings unavailable; skipping round");
                    return Duration::from_secs(SWEEP_MIN_INTERVAL_SECS);
                }
            };
            let (concurrency, floor) = sweep_dials(&cfg);
            (concurrency, floor, cfg.archived_project_paths.clone())
        };

        let sessions: Vec<String> = {
            let mgr = self.session_manager.read().await;
            mgr.get_sessions_repos().await
        }
        .into_iter()
        .flat_map(|(_, repos, _)| repos.into_iter().map(|r| r.source_path))
        .collect();

        let discovery = discovery_repo_paths();

        // The WHOLE build, normalization included, runs on the blocking pool
        // whenever the archived list is non-empty: `build_work_list` canonicalizes
        // once per distinct candidate, which on this single-worker runtime would
        // otherwise be unbounded blocking I/O with no await point for the shutdown
        // `select!` to preempt (INV-1). The empty case short-circuits with zero
        // syscalls, so it belongs on the async body and saves a task hop.
        let paths = if archived.is_empty() {
            build_work_list(discovery, sessions, &[])
        } else {
            // The closure is `move` and consumes both halves, so the join-failure
            // arm needs its own copies.
            let (discovery_fb, sessions_fb) = (discovery.clone(), sessions.clone());
            match tokio::task::spawn_blocking(move || {
                let roots = crate::config::sessions_persistence::normalize_project_roots(&archived);
                build_work_list(discovery, sessions, &roots)
            })
            .await
            {
                Ok(paths) => paths,
                Err(e) => {
                    // A superset sweep is a freshness cost, never a correctness one.
                    log::warn!(
                        "[GitSweeper] work-list build failed ({e}); sweeping unfiltered this round"
                    );
                    build_work_list(discovery_fb, sessions_fb, &[])
                }
            }
        };

        // `detect_status_with_timeout(p).await` is evaluated as an ARGUMENT, before
        // `publish_git_status` takes its std guard. Writing it the other way round
        // would hold a `std::sync::Mutex` guard across an await.
        futures::stream::iter(paths.iter().map(|p| async move {
            publish_git_status(p, detect_status_with_timeout(p).await);
        }))
        .buffer_unordered(concurrency)
        .for_each(|_| async {})
        .await;

        // Defence in depth, and the distinction matters: the tempting justification
        // (round 1 sweeps an empty list and would wipe everything) is FALSE, since on
        // round 1 the maps are empty too. Keep the guard anyway, it is free and it
        // protects a future consumer, but do not defend it with that false mechanism.
        if !paths.is_empty() {
            retain_swept_paths(&paths.iter().cloned().collect());
        }

        let applied = if paths.is_empty() {
            Duration::from_secs(SWEEP_MIN_INTERVAL_SECS)
        } else {
            floor
        };

        // The applied floor is in the line because it is the only way both clamps
        // are observable at the DEFAULT log level: `sweep_dials`'s clamp lines are
        // `debug!`. The level split matters too: an empty round floors at 1s, so an
        // unconditional `info!` would emit 86,400 lines/day in exactly the state
        // INV-2 exists to protect.
        if paths.is_empty() {
            log::debug!(
                "[GitSweeper] round: {} path(s), concurrency={}, floor={}ms, {}ms",
                paths.len(),
                concurrency,
                applied.as_millis(),
                round_started.elapsed().as_millis()
            );
        } else {
            log::info!(
                "[GitSweeper] round: {} path(s), concurrency={}, floor={}ms, {}ms",
                paths.len(),
                concurrency,
                applied.as_millis(),
                round_started.elapsed().as_millis()
            );
        }

        applied
    }
}

/// #2064 - the discovery maps are process-global and are written by two test
/// modules (`pty::remote_watcher` rounds and `commands::ac_discovery`'s push
/// test), so those tests must serialize against each other, not merely within
/// one module. Lives beside the maps it guards. Async-aware because the round
/// tests hold it across their awaits; the sync ac_discovery test uses
/// `blocking_lock`.
#[cfg(test)]
pub(crate) static DISCOVERY_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
mod tests;
