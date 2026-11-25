use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

mod events;

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

/// §3.3 — `note_timeout` is a monotonic per-path counter so the
/// caller can throttle WARN emissions to "first + every Nth". Two
/// independent paths get independent counters.
#[test]
fn note_timeout_counts_per_path() {
    let owner = GitState::new();
    // Process-global state: use unique-per-test path strings to avoid
    // collisions with other tests that may exercise this helper.
    let p1 = "/test-280-3-3/git-watcher/path-a";
    let p2 = "/test-280-3-3/git-watcher/path-b";
    assert_eq!(note_timeout(p1, &owner), 1);
    assert_eq!(note_timeout(p1, &owner), 2);
    assert_eq!(note_timeout(p1, &owner), 3);
    // Independent counter for a different path.
    assert_eq!(note_timeout(p2, &owner), 1);
    assert_eq!(note_timeout(p1, &owner), 4);
}

/// T1 - the read-site clamp, and the executable form of INV-2: no floor
/// may be 0, so a configured 0 becomes the 1s minimum rather than a spin.
#[test]
fn sweep_dials_clamps_both_dials() {
    let mut cfg = AppSettings::default();
    assert_eq!(sweep_dials(&cfg), (1, Duration::from_secs(10)));

    cfg.git_sweep_concurrency = 0;
    cfg.git_sweep_min_interval_secs = 0;
    assert_eq!(sweep_dials(&cfg), (1, Duration::from_secs(1)));

    cfg.git_sweep_concurrency = 9;
    cfg.git_sweep_min_interval_secs = 100_000;
    assert_eq!(sweep_dials(&cfg), (4, Duration::from_secs(3600)));
}

/// T2 - INV-7 (both halves survive), INV-3 (dedupe by exact string) and
/// D2 (archived dropped from BOTH halves, not just the discovery one).
#[test]
fn build_work_list_unions_dedupes_and_drops_archived() {
    let archived = crate::config::sessions_persistence::normalize_project_roots(&[
        "D:/test-1298/archived-project".to_string(),
    ]);
    let discovery = vec![
        "D:/test-1298/live/repo-shared".to_string(),
        "D:/test-1298/live/repo-dormant".to_string(),
        "D:/test-1298/archived-project/repo-gone".to_string(),
    ];
    let sessions = vec![
        "D:/test-1298/live/repo-shared".to_string(),
        "D:/test-1298/unloaded/repo-session-only".to_string(),
        "D:/test-1298/archived-project/repo-gone".to_string(),
    ];

    assert_eq!(
        build_work_list(discovery, sessions, &archived),
        vec![
            "D:/test-1298/live/repo-dormant".to_string(),
            "D:/test-1298/live/repo-shared".to_string(),
            "D:/test-1298/unloaded/repo-session-only".to_string(),
        ]
    );
}

/// T3 - the snapshot contract, including INV-6: a published `None`
/// overwrites a previous `Some`, so `branch` does not hold across a failure.
#[test]
fn published_git_status_round_trips_and_unknown_paths_read_none() {
    let owner = GitState::new();
    // Process-global state: unique-per-test path strings, as the
    // `remember_dirty` and `note_timeout` tests do for the same reason.
    let path = "/test-1298/snapshot/path-a";
    assert!(read_git_status("/test-1298/snapshot/never-published", &owner).is_none());

    let status = GitStatus {
        branch: Some("main".to_string()),
        dirty: true,
    };
    publish_git_status(path, Some(status.clone()), &owner);
    assert_eq!(read_git_status(path, &owner), Some(status));

    publish_git_status(path, None, &owner);
    assert!(read_git_status(path, &owner).is_none());
}

/// T4 - the executable form of INV-8. The second half of the assertion is
/// the whole point: without the `LAST_DIRTY` eviction a dropped path reads
/// `None` from the snapshot and its last successful bool from `remember_dirty`,
/// i.e. a confident colour sourced from a detection that will never repeat.
///
/// `live` spares every key already in the two process-global maps, because the
/// `remember_dirty_*` tests share them and run concurrently with this one; the
/// retain under test is the one that drops `path-dropped`.
#[test]
fn retain_swept_paths_evicts_dropped_paths_from_both_maps() {
    let owner = GitState::new();
    let kept = "/test-1298/retain/path-kept";
    let dropped = "/test-1298/retain/path-dropped";
    let status = GitStatus {
        branch: Some("main".to_string()),
        dirty: true,
    };

    publish_git_status(kept, Some(status.clone()), &owner);
    publish_git_status(dropped, Some(status), &owner);
    assert_eq!(remember_dirty(kept, Some(true), &owner), Some(true));
    assert_eq!(remember_dirty(dropped, Some(true), &owner), Some(true));

    let mut live: HashSet<String> = git_status_snapshot(&owner)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .keys()
        .cloned()
        .collect();
    live.extend(
        last_dirty_map(&owner)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned(),
    );
    live.remove(dropped);

    retain_swept_paths(&live, &owner);

    assert!(read_git_status(kept, &owner).is_some());
    assert_eq!(remember_dirty(kept, None, &owner), Some(true));
    assert!(read_git_status(dropped, &owner).is_none());
    assert_eq!(remember_dirty(dropped, None, &owner), None);
}

// --- Cached-origin parser semantics ---

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

// --- Real temporary repositories ---

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

// --- Remember_dirty ---
// Process-global state: use unique-per-test path strings to avoid collisions,
// exactly as `note_timeout_counts_per_path` above documents.

/// A failure with nothing remembered must not invent a value. `None` = "never got a
/// first answer", which renders violet, and that is honest.
#[test]
fn remember_dirty_never_detected_stays_none() {
    let owner = GitState::new();
    let p = "/test-1028/remember/never-detected";
    assert_eq!(remember_dirty(p, None, &owner), None);
    assert_eq!(remember_dirty(p, None, &owner), None);
}

/// G1's regression guard: the hold must survive a CLUSTER, not just a single blip.
/// Timeouts cluster (I/O spikes, AV scans) and `git status` is heaviest exactly when
/// the worktree is being written to, i.e. exactly when the repo is dirty.
#[test]
fn remember_dirty_survives_failure_cluster() {
    let owner = GitState::new();
    let p = "/test-1028/remember/cluster";
    assert_eq!(remember_dirty(p, Some(true), &owner), Some(true));
    for tick in 0..5 {
        assert_eq!(
            remember_dirty(p, None, &owner),
            Some(true),
            "tick {tick} of a 5-tick cluster must still hold red"
        );
    }
}

/// A later success overwrites the memory; the hold is last-known, not sticky-red.
#[test]
fn remember_dirty_success_overwrites_then_holds() {
    let owner = GitState::new();
    let p = "/test-1028/remember/overwrite";
    assert_eq!(remember_dirty(p, Some(true), &owner), Some(true));
    assert_eq!(remember_dirty(p, Some(false), &owner), Some(false));
    assert_eq!(
        remember_dirty(p, None, &owner),
        Some(false),
        "holds the newer answer"
    );
}

/// Keyed by path: one repo's memory never leaks onto another.
#[test]
fn remember_dirty_keeps_paths_independent() {
    let owner = GitState::new();
    let p1 = "/test-1028/remember/path-a";
    let p2 = "/test-1028/remember/path-b";
    assert_eq!(remember_dirty(p1, Some(true), &owner), Some(true));
    assert_eq!(remember_dirty(p2, None, &owner), None, "p2 never detected");
    assert_eq!(remember_dirty(p2, Some(false), &owner), Some(false));
    assert_eq!(
        remember_dirty(p1, None, &owner),
        Some(true),
        "p1 unaffected by p2"
    );
}

/// The map is SHARED by both watchers on purpose: during a timeout cluster separate
/// maps would let the two watchers hold different-age values, write divergent
/// `dirty`, and both emit, flickering the badge. Sharing makes them agree by
/// construction. This asserts the shape that guarantees it: a read from "the other
/// watcher" sees the freshest successful observation of that path, whoever made it.
#[test]
fn remember_dirty_is_shared_across_watchers() {
    let owner = GitState::new();
    let p = "/test-1028/remember/shared";
    // GitWatcher (5s) observes dirty.
    assert_eq!(remember_dirty(p, Some(true), &owner), Some(true));
    // DiscoveryBranchWatcher (15s) fails on the same path and holds GitWatcher's
    // answer rather than its own older one or a confident clean.
    assert_eq!(remember_dirty(p, None, &owner), Some(true));
    // Discovery then observes clean; GitWatcher's next failure holds THAT.
    assert_eq!(remember_dirty(p, Some(false), &owner), Some(false));
    assert_eq!(remember_dirty(p, None, &owner), Some(false));
}

fn poison_git_mutex<T>(mutex: &Mutex<T>) {
    let poisoned = std::panic::catch_unwind(|| {
        let _guard = mutex.lock().expect("fresh mutex");
        panic!("poison this owner mutex");
    });
    assert!(poisoned.is_err());
    assert!(mutex.is_poisoned());
}

#[test]
fn git_owner_shared_producers_preserve_unknown_and_prune() {
    let owner = Arc::new(GitState::new());
    let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let settings = crate::config::settings::new_settings_state(AppSettings::default());
    let first = GitSweeper::with_state(sessions.clone(), settings.clone(), owner.clone());
    let second = GitSweeper::with_state(sessions, settings, owner.clone());
    assert!(Arc::ptr_eq(&first.git_state, &second.git_state));
    assert_git_owner_lazy(&owner);

    let path = "shared-repo";
    let status = GitStatus {
        branch: Some("main".into()),
        dirty: true,
    };
    assert_git_shared_publication(&first, &second, path, status);

    assert_git_poisoned_maps(&owner, &first, &second, path);
    assert_git_discovery(&owner, &first, &second, path);
}

fn assert_git_owner_lazy(owner: &GitState) {
    assert!(owner.timeout_counts.get().is_none());
    assert!(owner.last_dirty.get().is_none());
    assert!(owner.snapshot.get().is_none());
    assert!(owner.discovery_paths.get().is_none());
    assert!(owner.discovery_rooms.get().is_none());
}

fn assert_git_shared_publication(
    first: &GitSweeper,
    second: &GitSweeper,
    path: &str,
    status: GitStatus,
) {
    publish_git_status(path, Some(status.clone()), &first.git_state);
    assert_eq!(read_git_status(path, &second.git_state), Some(status));
    assert_eq!(
        remember_dirty(path, Some(true), &first.git_state),
        Some(true)
    );
    publish_git_status(path, None, &second.git_state);
    assert!(read_git_status(path, &first.git_state).is_none());
    assert_eq!(remember_dirty(path, None, &second.git_state), Some(true));
    let fresh = GitState::new();
    assert_eq!(remember_dirty(path, None, &fresh), None);
    assert!(read_git_status(path, &fresh).is_none());
    retain_swept_paths(&HashSet::new(), &second.git_state);
    assert!(read_git_status(path, &first.git_state).is_none());
    assert_eq!(remember_dirty(path, None, &first.git_state), None);
}

fn assert_git_poisoned_maps(owner: &GitState, first: &GitSweeper, second: &GitSweeper, path: &str) {
    assert_eq!(note_timeout(path, &first.git_state), 1);
    poison_git_mutex(owner.timeout_counts.get().unwrap());
    assert_eq!(note_timeout(path, &second.git_state), 2);
    poison_git_mutex(last_dirty_map(owner));
    assert_eq!(
        remember_dirty(path, Some(false), &first.git_state),
        Some(false)
    );
    assert_eq!(remember_dirty(path, None, &second.git_state), Some(false));
    poison_git_mutex(git_status_snapshot(owner));
    publish_git_status(
        path,
        Some(GitStatus {
            branch: None,
            dirty: false,
        }),
        &first.git_state,
    );
    assert!(!read_git_status(path, &second.git_state).unwrap().dirty);
}

fn assert_git_discovery(owner: &GitState, first: &GitSweeper, second: &GitSweeper, path: &str) {
    set_discovery_repo_paths(vec![path.into(), path.into()], &first.git_state);
    poison_git_mutex(discovery_repo_paths_cell(owner));
    assert_eq!(discovery_repo_paths(&second.git_state), vec![path, path]);
    set_discovery_repo_paths(vec![path.into()], &second.git_state);
    assert_eq!(discovery_repo_paths(&first.git_state), vec![path]);
    let rooms = vec![(path.into(), "room".into())];
    set_discovery_repo_rooms(rooms.clone(), &first.git_state);
    poison_git_mutex(discovery_repo_rooms_cell(owner));
    assert_eq!(discovery_repo_rooms(&second.git_state), rooms);
    set_discovery_repo_rooms(Vec::new(), &second.git_state);
    assert!(discovery_repo_rooms(&first.git_state).is_empty());
    retain_swept_paths(&HashSet::new(), &first.git_state);
    assert!(read_git_status(path, &second.git_state).is_none());
    assert_eq!(remember_dirty(path, None, &second.git_state), None);
}

// ----- #5206 P03: observer runtime start -----------------------------------------------

const P03_CHILD: &str = "AC5206_P03_GIT_CHILD";

static P03_ERRORS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct P03Log;

impl log::Log for P03Log {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() == log::Level::Error
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            P03_ERRORS.lock().unwrap().push(record.args().to_string());
        }
    }
    fn flush(&self) {}
}

/// Runs `name` again in a child of this test binary, so its process-wide logger is its own.
/// Returns true inside the child; in the parent it waits for and checks the child.
fn p03_in_child(name: &str) -> bool {
    if std::env::var_os(P03_CHILD).is_some() {
        log::set_logger(&P03Log).expect("child installs the error logger");
        log::set_max_level(log::LevelFilter::Error);
        return true;
    }
    let path = module_path!().split_once("::").expect("crate path").1;
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([format!("{path}::{name}").as_str(), "--exact", "--nocapture"])
        .args(["--test-threads", "1"])
        .env(P03_CHILD, "1")
        .stdin(std::process::Stdio::null())
        .spawn()
        .expect("spawn the child test");
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "child {name} failed: {status}");
            return false;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child {name} timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A failed build logs once per identity, with cause and effect, and never runs the work.
#[test]
fn a_failed_git_runtime_build_logs_once_and_skips_the_work() {
    if !p03_in_child("a_failed_git_runtime_build_logs_once_and_skips_the_work") {
        return;
    }
    for (name, effect) in [
        ("GitWatcher", WATCHER_STOPPED),
        ("GitSweeper", SWEEP_STOPPED),
    ] {
        let (made, worked) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let before = P03_ERRORS.lock().unwrap().len();
        git_observer_runtime(
            name,
            effect,
            || {
                made.fetch_add(1, Ordering::SeqCst);
                Err(std::io::Error::other("E_FACTORY"))
            },
            |_| {
                worked.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert_eq!((made.into_inner(), worked.into_inner()), (1, 0), "{name}");
        let logs = P03_ERRORS.lock().unwrap()[before..].to_vec();
        assert_eq!(logs.len(), 1, "{name}: {logs:?}");
        assert!(logs[0].starts_with(&format!("[{name}] ")), "{logs:?}");
        assert!(
            logs[0].contains("E_FACTORY") && logs[0].ends_with(effect),
            "{logs:?}"
        );
    }
}

/// The shared builder makes an independent CurrentThread runtime per call, and the
/// helper hands it to the work exactly once.
#[test]
fn build_git_runtime_makes_one_current_thread_runtime_per_observer() {
    use tokio::runtime::RuntimeFlavor;
    let calls = AtomicUsize::new(0);
    let mut handles = Vec::new();
    for _ in 0..2 {
        git_observer_runtime("GitWatcher", WATCHER_STOPPED, build_git_runtime, |rt| {
            calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(rt.handle().runtime_flavor(), RuntimeFlavor::CurrentThread);
            assert_eq!(rt.block_on(async { 7 }), 7);
            handles.push(rt.handle().id());
        });
    }
    assert_eq!(calls.into_inner(), 2);
    assert_ne!(handles[0], handles[1], "each observer owns its own runtime");
}

fn repo_session_manager() -> Arc<tokio::sync::RwLock<SessionManager>> {
    Arc::new(tokio::sync::RwLock::new(SessionManager::new()))
}

async fn add_repo_session(manager: &Arc<tokio::sync::RwLock<SessionManager>>) -> Uuid {
    let repo = SessionRepo {
        label: "A".into(),
        source_path: "/nonexistent/p03".into(),
        branch: None,
        dirty: None,
    };
    let mgr = manager.read().await;
    let session = mgr
        .create_session(
            "cmd".into(),
            vec![],
            "/tmp".into(),
            None,
            None,
            vec![repo],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session");
    session.id
}

/// The watcher waits a full `POLL_INTERVAL` before its first poll, then polls.
#[tokio::test(start_paused = true)]
async fn the_git_watcher_first_polls_after_poll_interval() {
    let manager = repo_session_manager();
    let id = add_repo_session(&manager).await;
    let watcher = GitWatcher::new_headless_for_tests(Arc::clone(&manager));
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let task = tokio::spawn({
        let (watcher, shutdown) = (Arc::clone(&watcher), shutdown.clone());
        async move { watcher.watch(shutdown).await }
    });
    let polled = |w: &GitWatcher| w.cache.lock().unwrap().contains_key(&id);
    tokio::time::sleep(POLL_INTERVAL - Duration::from_millis(1)).await;
    assert!(!polled(&watcher), "no poll before POLL_INTERVAL");
    tokio::time::sleep(Duration::from_millis(2)).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert!(polled(&watcher), "one poll at POLL_INTERVAL");
    shutdown.token().cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
}

/// Shutdown ends the watcher during its wait.
#[tokio::test(start_paused = true)]
async fn the_git_watcher_stops_during_its_wait() {
    let watcher = GitWatcher::new_headless_for_tests(repo_session_manager());
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { watcher.watch(shutdown).await }
    });
    tokio::time::sleep(Duration::from_secs(1)).await;
    shutdown.token().cancel();
    tokio::time::timeout(Duration::from_millis(10), task)
        .await
        .unwrap()
        .unwrap();
}

fn sweeper(manager: Arc<tokio::sync::RwLock<SessionManager>>) -> Arc<GitSweeper> {
    let settings = crate::config::settings::new_settings_state(Default::default());
    GitSweeper::with_state(manager, settings, Arc::new(GitState::new()))
}

/// The first round starts at once (it reaches the session read immediately), and
/// shutdown ends the sweep while that round is blocked there.
#[tokio::test(start_paused = true)]
async fn the_git_sweeper_starts_at_once_and_stops_during_a_round() {
    let manager = repo_session_manager();
    let held = manager.write().await;
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let sweeper = sweeper(Arc::clone(&manager));
    let sweep = sweeper.sweep(shutdown.clone());
    tokio::pin!(sweep);
    let pending = tokio::time::timeout(Duration::from_secs(30), &mut sweep).await;
    assert!(
        pending.is_err(),
        "the round stays blocked on the session read"
    );
    shutdown.token().cancel();
    tokio::time::timeout(Duration::from_millis(10), &mut sweep)
        .await
        .unwrap();
    drop(held);
}

/// The period is max(floor, round duration): a shorter round waits out the floor, a longer
/// one is followed at once, and shutdown ends the floor wait. The fixture round reports
/// each start on a channel and takes a scripted time on the paused Tokio clock.
#[tokio::test(start_paused = true)]
async fn the_git_sweep_period_is_the_floor_or_the_round_whichever_is_longer() {
    let floor = Duration::from_secs(1);
    let script = [
        Duration::from_millis(300),
        Duration::from_millis(1500),
        Duration::ZERO,
    ];
    let t0 = tokio::time::Instant::now();
    let (starts, mut started) = tokio::sync::mpsc::unbounded_channel();
    let rounds = std::cell::Cell::new(0usize);
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let round = || {
        let n = rounds.replace(rounds.get() + 1);
        let starts = starts.clone();
        async move {
            starts.send(t0.elapsed()).unwrap();
            tokio::time::sleep(script.get(n).copied().unwrap_or_default()).await;
            floor
        }
    };
    let sweep = sweep_rounds(round, shutdown.clone());
    tokio::pin!(sweep);
    let mut seen = Vec::new();
    while seen.len() < 3 {
        tokio::select! {
            () = &mut sweep => panic!("the sweep ended on its own"),
            Some(at) = started.recv() => seen.push(at),
        }
    }
    // 0: first round at once; +1.0 s: 300 ms round, floor wins; +1.5 s: 1500 ms round wins.
    let ms = |m| Duration::from_millis(m);
    assert_eq!(seen, vec![ms(0), ms(1000), ms(2500)]);
    // The zero-length third round now waits its floor; shutdown ends the sweep there.
    assert!(tokio::time::timeout(ms(500), &mut sweep).await.is_err());
    assert!(
        started.try_recv().is_err(),
        "no round during the floor wait"
    );
    shutdown.token().cancel();
    tokio::time::timeout(ms(10), &mut sweep).await.unwrap();
}
