use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::join_all;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::session::manager::SessionManager;
use crate::session::session::SessionRepo;

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const DETECT_TIMEOUT: Duration = Duration::from_secs(2);

/// #280 §3.3 - per-path counter for `detect_git_status` timeouts (#1028 renamed
/// the detection; the counter and its policy are unchanged). Used to dampen
/// the WARN storm by logging only the first occurrence per path plus every
/// 50th thereafter. Process-local: counts reset on restart. Bounded by the
/// number of distinct repo paths (~150 in production observations), so no
/// GC is needed.
fn note_timeout(path: &str) -> u64 {
    use std::sync::OnceLock;
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
#[derive(Debug, PartialEq, Eq)]
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
/// like `detect_git_branch_sync` (`commands/ac_discovery.rs`), which has no `.git` guard
/// and no ceiling, would silently violate it. Nothing does today.
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
/// Each caller keeps its OWN timeout wrapper. That is required, not stylistic: each owns
/// its per-path counter and log tag (`note_timeout` / `note_discovery_timeout`,
/// `[GitWatcher]` / `[DiscoveryBranchWatcher]`), and merging them would collapse the two
/// tags that keep the watchers distinguishable in app.log.
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
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    // Gate 1: require a work tree ROOT; do not let git discover a repo by walking up.
    if tokio::fs::metadata(Path::new(working_dir).join(".git"))
        .await
        .is_err()
    {
        return None;
    }

    let mut cmd = tokio::process::Command::new("git");
    crate::pty::credentials::scrub_credentials_from_tokio_command(&mut cmd);
    cmd.args([
        "--no-optional-locks",
        "status",
        "--porcelain",
        "--branch",
        "--untracked-files=normal",
        "--ahead-behind",
    ])
    .current_dir(working_dir)
    .kill_on_drop(true);

    // Gate 2: no ancestor ascent. `join_paths` emits the platform separator, matching
    // the existing ceiling code. With no parent (a filesystem root), skip the env
    // entirely rather than set an empty value.
    if let Some(parent) = Path::new(working_dir).parent() {
        match std::env::join_paths(std::iter::once(parent)) {
            Ok(ceiling) => {
                cmd.env("GIT_CEILING_DIRECTORIES", ceiling);
            }
            Err(e) => {
                // Only reachable if the path contains the path separator itself.
                // Gate 1 still stands; log rather than abort the detection.
                log::debug!(
                    "[git] Could not build GIT_CEILING_DIRECTORIES for {}: {}",
                    working_dir,
                    e
                );
            }
        }
    }

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    match cmd.output().await {
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
    app_handle: AppHandle,
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
            app_handle,
            cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn start(self: &Arc<Self>, shutdown: crate::shutdown::ShutdownSignal) {
        let watcher = Arc::clone(self);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new()
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

            // Parallelize per-repo detection (Grinch #16). Each call bounded by 2s
            // (detect_status_with_timeout). Without join_all, worst-case per poll is
            // M*N*2s under simultaneous stalls.
            let statuses: Vec<Option<GitStatus>> = join_all(
                repos
                    .iter()
                    .map(|r| Self::detect_status_with_timeout(&r.source_path)),
            )
            .await;

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
                    let _ = self.app_handle.emit(
                        "session_git_repos",
                        GitReposPayload {
                            session_id: id.to_string(),
                            repos: refreshed.clone(),
                        },
                    );
                    self.cache.lock().unwrap().insert(id, refreshed);
                } else {
                    log::debug!(
                        "[GitWatcher] gen mismatch on session {} — refresh landed during poll; skipping stale emit",
                        id
                    );
                    // Invalidate our cache so the next tick re-evaluates against the refreshed list.
                    self.cache.lock().unwrap().remove(&id);
                }
            }
        }
    }

    /// Detect branch + worktree-dirty via the shared `detect_git_status`, bounded by a
    /// 2s timeout. On timeout the pending future is dropped; `.kill_on_drop(true)`
    /// ensures the child `git.exe` is terminated so repeated polls can't leak processes.
    ///
    /// #1028 - this wrapper stays private to GitWatcher rather than merging with
    /// DiscoveryBranchWatcher's: each owns its per-path counter and log tag, and merging
    /// them would collapse `note_timeout`/`note_discovery_timeout` and the two tags that
    /// keep the watchers distinguishable in app.log.
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
                        "[GitWatcher] detect_git_status timed out for {} (>{}s); occurrence={} (logging 1st + every 50th)",
                        working_dir,
                        DETECT_TIMEOUT.as_secs(),
                        n
                    );
                }
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CAS semantic guard (validation #18): a stale `expected_gen` must fail and leave state untouched.
    #[tokio::test]
    async fn set_git_repos_if_gen_rejects_stale_gen() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "cmd".into(),
                vec![],
                "C:/tmp".into(),
                None,
                None,
                vec![SessionRepo {
                    label: "A".into(),
                    source_path: "C:/a".into(),
                    branch: None,
                    dirty: None,
                }],
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session");

        let id = session.id;
        let gen0 = mgr.get_git_repos_gen(id).await.unwrap();

        // Simulate a refresh landing: bump gen by writing new repos.
        mgr.set_git_repos(
            id,
            vec![SessionRepo {
                label: "A2".into(),
                source_path: "C:/a2".into(),
                branch: None,
                dirty: None,
            }],
        )
        .await;

        // Stale write with pre-refresh gen must fail.
        let wrote = mgr
            .set_git_repos_if_gen(
                id,
                vec![SessionRepo {
                    label: "A-stale".into(),
                    source_path: "C:/a-stale".into(),
                    branch: Some("main".into()),
                    dirty: None,
                }],
                gen0,
            )
            .await;
        assert!(!wrote, "stale gen must be rejected");

        // State is still the refreshed list (label "A2"), not the stale one.
        let gen_now = mgr.get_git_repos_gen(id).await.unwrap();
        assert_ne!(gen_now, gen0);

        // Write with current gen succeeds.
        let wrote2 = mgr
            .set_git_repos_if_gen(
                id,
                vec![SessionRepo {
                    label: "A2".into(),
                    source_path: "C:/a2".into(),
                    branch: Some("feature".into()),
                    dirty: None,
                }],
                gen_now,
            )
            .await;
        assert!(wrote2, "current gen must succeed");
    }

    /// #280 §3.3 — `note_timeout` is a monotonic per-path counter so the
    /// caller can throttle WARN emissions to "first + every Nth". Two
    /// independent paths get independent counters.
    #[test]
    fn note_timeout_counts_per_path() {
        // Process-global state: use unique-per-test path strings to avoid
        // collisions with other tests that may exercise this helper.
        let p1 = "/test-280-3-3/git-watcher/path-a";
        let p2 = "/test-280-3-3/git-watcher/path-b";
        assert_eq!(note_timeout(p1), 1);
        assert_eq!(note_timeout(p1), 2);
        assert_eq!(note_timeout(p1), 3);
        // Independent counter for a different path.
        assert_eq!(note_timeout(p2), 1);
        assert_eq!(note_timeout(p1), 4);
    }

    // --- #1078: cached-origin parser semantics ---

    fn parse(stdout: &str) -> GitStatus {
        parse_status_porcelain_branch(stdout).expect("header should parse")
    }

    #[test]
    fn parse_synced_and_positive_behind_only_origin_headers_are_clean() {
        for stdout in [
            "## main...origin/main\n",
            "## main...origin/main",
            "## main...origin/main [behind 1]\n",
            "## main...origin/main [behind 42]\n",
        ] {
            let status = parse(stdout);
            assert_eq!(status.branch.as_deref(), Some("main"), "{stdout:?}");
            assert!(!status.dirty, "{stdout:?}");
        }
    }

    #[test]
    fn parse_ahead_diverged_gone_and_malformed_tracking_are_red() {
        for header in [
            "main...origin/main [ahead 1]",
            "main...origin/main [ahead 1, behind 2]",
            "main...origin/main [gone]",
            "main...origin/main [unknown 1]",
            "main...origin/main [behind 0]",
            "main...origin/main [behind -1]",
            "main...origin/main [behind 1, ahead 2]",
            "main...origin/main [behind 18446744073709551616]",
            "main...origin/main [behind 1",
        ] {
            let stdout = format!("## {header}\n");
            assert!(parse(&stdout).dirty, "{header:?} must fail closed");
        }
    }

    #[test]
    fn parse_missing_and_non_origin_upstreams_are_red() {
        for header in [
            "main",
            "main...backup/main",
            "main...backup/main [behind 1]",
            "main...origin/",
        ] {
            let stdout = format!("## {header}\n");
            let status = parse(&stdout);
            assert_eq!(status.branch.as_deref(), Some("main"), "{header:?}");
            assert!(status.dirty, "{header:?}");
        }
    }

    #[test]
    fn parse_detached_empty_head_and_unborn_headers_are_red() {
        let detached = parse("## HEAD (no branch)\n");
        assert_eq!(detached.branch, None);
        assert!(detached.dirty);

        let empty = parse("## \n");
        assert_eq!(empty.branch, None);
        assert!(empty.dirty);

        let head = parse("## HEAD\n");
        assert_eq!(head.branch, None);
        assert!(head.dirty);

        let unborn = parse("## No commits yet on main\n");
        assert_eq!(unborn.branch.as_deref(), Some("main"));
        assert!(unborn.dirty);

        let unborn_with_upstream = parse("## No commits yet on main...origin/main [gone]\n");
        assert_eq!(unborn_with_upstream.branch.as_deref(), Some("main"));
        assert!(unborn_with_upstream.dirty);
    }

    #[test]
    fn parse_preserves_legal_dots_and_slashes_in_branch_names() {
        let synced = parse("## feat/a.b-c...origin/feat/a.b-c\n");
        assert_eq!(synced.branch.as_deref(), Some("feat/a.b-c"));
        assert!(!synced.dirty);

        let ahead = parse("## feat/a.b-c...origin/feat/a.b-c [ahead 1]\n");
        assert_eq!(ahead.branch.as_deref(), Some("feat/a.b-c"));
        assert!(ahead.dirty);
    }

    #[test]
    fn parse_every_nonblank_porcelain_entry_as_red() {
        for (kind, row) in [
            ("unstaged", " M tracked.txt"),
            ("staged", "M  tracked.txt"),
            ("untracked", "?? untracked.txt"),
            ("deleted", " D deleted.txt"),
            ("renamed", "R  old.txt -> new.txt"),
            ("conflicted", "UU conflicted.txt"),
            ("other", "T  type-changed.txt"),
        ] {
            let stdout = format!("## main...origin/main\n{row}\n");
            assert!(parse(&stdout).dirty, "{kind}: {row:?}");
        }
    }

    #[test]
    fn parse_blank_lines_after_synced_header_are_clean() {
        assert!(!parse("## main...origin/main\n\n  \n\t\n").dirty);
    }

    #[test]
    fn parse_missing_or_garbage_first_header_returns_none() {
        assert_eq!(parse_status_porcelain_branch(""), None, "no first line");
        assert_eq!(
            parse_status_porcelain_branch("fatal: not a git repository\n"),
            None,
            "stderr text on stdout"
        );
        assert_eq!(
            parse_status_porcelain_branch("##main\n"),
            None,
            "`## ` prefix requires the space"
        );
        assert_eq!(
            parse_status_porcelain_branch(" ## main\n"),
            None,
            "header must be at the line start"
        );
    }

    // --- #1078: real temporary repositories ---

    struct TestRepository {
        repo: std::path::PathBuf,
        origin: std::path::PathBuf,
    }

    fn git_path(path: &std::path::Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    fn git(cwd: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap_or_else(|error| panic!("failed to execute git {args:?}: {error}"));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "git {args:?} failed in {} with {}\nstdout:\n{}\nstderr:\n{}",
            cwd.display(),
            output.status,
            stdout,
            stderr
        );
        stdout.trim().to_string()
    }

    fn configure_test_repo(repo: &std::path::Path, hooks: &std::path::Path) {
        let hooks = git_path(hooks);
        git(
            repo,
            &["config", "--local", "user.name", "AgentsCommander Test"],
        );
        git(
            repo,
            &[
                "config",
                "--local",
                "user.email",
                "agentscommander@example.invalid",
            ],
        );
        git(repo, &["config", "--local", "commit.gpgSign", "false"]);
        git(repo, &["config", "--local", "tag.gpgSign", "false"]);
        git(repo, &["config", "--local", "core.autocrlf", "false"]);
        git(repo, &["config", "--local", "core.hooksPath", &hooks]);
    }

    fn init_synced_test_repo(root: &std::path::Path) -> TestRepository {
        let origin = root.join("origin.git");
        let repo = root.join("watched");
        let hooks = root.join("empty-hooks");
        std::fs::create_dir_all(&origin).expect("create bare origin directory");
        std::fs::create_dir_all(&repo).expect("create watched repository directory");
        std::fs::create_dir_all(&hooks).expect("create empty hooks directory");

        let origin_arg = git_path(&origin);
        git(
            root,
            &["init", "--bare", "--initial-branch=main", &origin_arg],
        );
        git(&repo, &["init", "--initial-branch=main", "."]);
        configure_test_repo(&repo, &hooks);

        std::fs::write(repo.join(".gitignore"), "ignored/\n").expect("write .gitignore");
        std::fs::write(repo.join("tracked.txt"), "initial\n").expect("write tracked file");
        git(&repo, &["add", ".gitignore", "tracked.txt"]);
        git(&repo, &["commit", "--no-gpg-sign", "-m", "initial"]);
        git(&repo, &["remote", "add", "origin", &origin_arg]);
        git(&repo, &["push", "--set-upstream", "origin", "main"]);

        TestRepository { repo, origin }
    }

    fn status_header(repo: &std::path::Path) -> String {
        git(
            repo,
            &[
                "--no-optional-locks",
                "status",
                "--porcelain",
                "--branch",
                "--untracked-files=normal",
                "--ahead-behind",
            ],
        )
        .lines()
        .next()
        .expect("status header")
        .to_string()
    }

    async fn detect(repo: &std::path::Path) -> GitStatus {
        detect_git_status(&repo.to_string_lossy())
            .await
            .unwrap_or_else(|| panic!("status detection failed for {}", repo.display()))
    }

    #[tokio::test]
    async fn detect_git_status_forces_untracked_and_keeps_ignored_out_of_scope() {
        let temp = tempfile::tempdir().expect("tempdir");
        let test = init_synced_test_repo(temp.path());
        git(
            &test.repo,
            &["config", "--local", "status.showUntrackedFiles", "no"],
        );
        assert!(!detect(&test.repo).await.dirty, "synchronized baseline");

        let untracked = test.repo.join("visible-untracked.txt");
        std::fs::write(&untracked, "visible\n").expect("write untracked file");
        assert!(
            detect(&test.repo).await.dirty,
            "command-line untracked reporting must override repository config"
        );

        std::fs::remove_file(untracked).expect("remove untracked file");
        let ignored_dir = test.repo.join("ignored");
        std::fs::create_dir_all(&ignored_dir).expect("create ignored directory");
        std::fs::write(ignored_dir.join("only.txt"), "ignored\n").expect("write ignored file");
        assert!(
            !detect(&test.repo).await.dirty,
            "ignored-only content stays normal"
        );
    }

    #[tokio::test]
    async fn detect_git_status_stays_red_from_modify_through_commit_until_push() {
        let temp = tempfile::tempdir().expect("tempdir");
        let test = init_synced_test_repo(temp.path());
        assert!(!detect(&test.repo).await.dirty, "synchronized baseline");

        std::fs::write(test.repo.join("tracked.txt"), "initial\nmodified\n")
            .expect("modify tracked file");
        assert!(detect(&test.repo).await.dirty, "unstaged modification");

        git(&test.repo, &["add", "tracked.txt"]);
        assert!(detect(&test.repo).await.dirty, "staged modification");

        git(
            &test.repo,
            &["commit", "--no-gpg-sign", "-m", "local commit"],
        );
        assert!(
            detect(&test.repo).await.dirty,
            "clean local-ahead commit remains red"
        );

        git(&test.repo, &["push", "origin", "main"]);
        assert_eq!(
            git(&test.repo, &["rev-parse", "HEAD"]),
            git(&test.repo, &["rev-parse", "refs/remotes/origin/main"]),
            "push must update the cached origin tracking ref"
        );
        assert!(!detect(&test.repo).await.dirty, "push returns to normal");
        assert!(
            !detect(&test.repo).await.dirty,
            "normal state is stable across detections"
        );
    }

    #[tokio::test]
    async fn detect_git_status_handles_cached_history_matrix() {
        let temp = tempfile::tempdir().expect("tempdir");
        let test = init_synced_test_repo(temp.path());
        let peer = temp.path().join("peer");
        let peer_arg = git_path(&peer);
        let origin_arg = git_path(&test.origin);
        git(
            temp.path(),
            &["clone", "--branch", "main", &origin_arg, &peer_arg],
        );
        configure_test_repo(&peer, &temp.path().join("empty-hooks"));

        std::fs::write(peer.join("tracked.txt"), "initial\npeer\n").expect("advance peer file");
        git(&peer, &["add", "tracked.txt"]);
        git(&peer, &["commit", "--no-gpg-sign", "-m", "peer commit"]);
        git(&peer, &["push", "origin", "main"]);

        assert_ne!(
            git(&test.repo, &["rev-parse", "refs/remotes/origin/main"]),
            git(&test.origin, &["rev-parse", "refs/heads/main"]),
            "peer push must not mutate the watched repository's cached ref"
        );
        assert!(
            !detect(&test.repo).await.dirty,
            "without fetch, unchanged cached tracking deliberately stays normal"
        );

        git(&test.repo, &["fetch", "origin"]);
        assert!(status_header(&test.repo).contains("[behind 1]"));
        assert!(!detect(&test.repo).await.dirty, "behind-only is normal");

        std::fs::write(test.repo.join("local-only.txt"), "local\n").expect("write local-only file");
        git(&test.repo, &["add", "local-only.txt"]);
        git(
            &test.repo,
            &["commit", "--no-gpg-sign", "-m", "diverging commit"],
        );
        assert!(status_header(&test.repo).contains("[ahead 1, behind 1]"));
        assert!(detect(&test.repo).await.dirty, "diverged with ahead is red");

        git(&test.repo, &["reset", "--hard", "origin/main"]);
        assert!(!detect(&test.repo).await.dirty, "restored sync is normal");

        git(&test.repo, &["branch", "--unset-upstream"]);
        assert!(detect(&test.repo).await.dirty, "missing upstream is red");
        git(
            &test.repo,
            &["branch", "--set-upstream-to=origin/main", "main"],
        );
        assert!(
            !detect(&test.repo).await.dirty,
            "restored upstream is normal"
        );

        git(
            &test.repo,
            &["update-ref", "-d", "refs/remotes/origin/main"],
        );
        assert_eq!(
            git(&test.repo, &["config", "--get", "branch.main.remote"]),
            "origin",
            "branch configuration remains after deleting the cached target"
        );
        assert!(status_header(&test.repo).contains("[gone]"));
        assert!(detect(&test.repo).await.dirty, "gone cached target is red");

        git(&test.repo, &["fetch", "origin"]);
        assert!(!detect(&test.repo).await.dirty, "fetch restores normal");
        git(&test.repo, &["checkout", "--detach", "HEAD"]);
        let detached = detect(&test.repo).await;
        assert_eq!(detached.branch, None);
        assert!(detached.dirty, "detached HEAD is red");
    }

    // --- #1028: remember_dirty ---
    // Process-global state: use unique-per-test path strings to avoid collisions,
    // exactly as `note_timeout_counts_per_path` above documents.

    /// A failure with nothing remembered must not invent a value. `None` = "never got a
    /// first answer", which renders violet, and that is honest.
    #[test]
    fn remember_dirty_never_detected_stays_none() {
        let p = "/test-1028/remember/never-detected";
        assert_eq!(remember_dirty(p, None), None);
        assert_eq!(remember_dirty(p, None), None);
    }

    /// G1's regression guard: the hold must survive a CLUSTER, not just a single blip.
    /// Timeouts cluster (I/O spikes, AV scans) and `git status` is heaviest exactly when
    /// the worktree is being written to, i.e. exactly when the repo is dirty.
    #[test]
    fn remember_dirty_survives_failure_cluster() {
        let p = "/test-1028/remember/cluster";
        assert_eq!(remember_dirty(p, Some(true)), Some(true));
        for tick in 0..5 {
            assert_eq!(
                remember_dirty(p, None),
                Some(true),
                "tick {tick} of a 5-tick cluster must still hold red"
            );
        }
    }

    /// A later success overwrites the memory; the hold is last-known, not sticky-red.
    #[test]
    fn remember_dirty_success_overwrites_then_holds() {
        let p = "/test-1028/remember/overwrite";
        assert_eq!(remember_dirty(p, Some(true)), Some(true));
        assert_eq!(remember_dirty(p, Some(false)), Some(false));
        assert_eq!(
            remember_dirty(p, None),
            Some(false),
            "holds the newer answer"
        );
    }

    /// Keyed by path: one repo's memory never leaks onto another.
    #[test]
    fn remember_dirty_keeps_paths_independent() {
        let p1 = "/test-1028/remember/path-a";
        let p2 = "/test-1028/remember/path-b";
        assert_eq!(remember_dirty(p1, Some(true)), Some(true));
        assert_eq!(remember_dirty(p2, None), None, "p2 never detected");
        assert_eq!(remember_dirty(p2, Some(false)), Some(false));
        assert_eq!(remember_dirty(p1, None), Some(true), "p1 unaffected by p2");
    }

    /// The map is SHARED by both watchers on purpose: during a timeout cluster separate
    /// maps would let the two watchers hold different-age values, write divergent
    /// `dirty`, and both emit, flickering the badge. Sharing makes them agree by
    /// construction. This asserts the shape that guarantees it: a read from "the other
    /// watcher" sees the freshest successful observation of that path, whoever made it.
    #[test]
    fn remember_dirty_is_shared_across_watchers() {
        let p = "/test-1028/remember/shared";
        // GitWatcher (5s) observes dirty.
        assert_eq!(remember_dirty(p, Some(true)), Some(true));
        // DiscoveryBranchWatcher (15s) fails on the same path and holds GitWatcher's
        // answer rather than its own older one or a confident clean.
        assert_eq!(remember_dirty(p, None), Some(true));
        // Discovery then observes clean; GitWatcher's next failure holds THAT.
        assert_eq!(remember_dirty(p, Some(false)), Some(false));
        assert_eq!(remember_dirty(p, None), Some(false));
    }
}
