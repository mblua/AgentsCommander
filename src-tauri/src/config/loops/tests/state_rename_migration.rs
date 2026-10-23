use super::*;

// #2718 (B4b) - rows E5 to E15 of the phase plan. Names are literal oracles.

const B4B_OLD: &str = "state.json";

const B4B_NEW: &str = "loop.state.no-git.json";

const B4B_SET_ASIDE: &str = "state.json.deprecated-1.no-git";

const B4B_STATE_A: &str = "{\n  \"lastCheckedAt\": \"2026-09-01T10:00:00Z\",\n  \"lastDueAt\": \
    null,\n  \"lastDeliveredAt\": null,\n  \"lastResult\": null,\n  \"pendingDueAt\": null,\n  \
    \"pendingRunId\": null,\n  \"lastMissedClosedAt\": null,\n  \"nextDueAt\": \
    \"2026-09-02T10:00:00Z\"\n}\n";

const B4B_STATE_B: &str = "{\"lastCheckedAt\": \"2026-08-01T08:00:00Z\"}";

const B4B_BUDGET: Duration = crate::config::naming_migration::MIGRATION_LOCK_BUDGET;

struct B4bFixture {
    _tmp: tempfile::TempDir,
    ac: PathBuf,
    cfg: PathBuf,
}

fn b4b_fixture() -> B4bFixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac = tmp.path().join("project").join(".ac");
    let cfg = tmp.path().join("cfg");
    std::fs::create_dir_all(&ac).unwrap();
    std::fs::create_dir_all(&cfg).unwrap();
    B4bFixture { _tmp: tmp, ac, cfg }
}

/// `_loop_<id>` with a config file and, when given, the old state file.
fn b4b_loop(ac: &Path, id: &str, old_state: Option<&str>) -> PathBuf {
    let dir = ac.join(format!("_loop_{id}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), "# fixture\n").unwrap();
    if let Some(bytes) = old_state {
        std::fs::write(dir.join(B4B_OLD), bytes).unwrap();
    }
    dir
}

/// The directory the scope sees: under the canonical AC root.
fn b4b_canonical(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir.parent().unwrap())
        .unwrap()
        .join(dir.file_name().unwrap())
}

fn b4b_lock(dir: &Path, cfg: Option<&Path>) -> LoopDirLock {
    acquire_loop_lock_for_dir_in(dir, LOOP_LOCK_TIMEOUT, cfg).expect("loop lock")
}

fn b4b_record(cfg: &Path, dir: &Path) -> Option<naming_migration::ScopeRecord> {
    naming_migration::read_journal(Some(cfg))
        .expect("readable journal")
        .and_then(|j| j.scope(&loop_state_scope_key(&b4b_canonical(dir))).cloned())
}

fn b4b_state_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n != "config.toml")
        .collect();
    names.sort();
    names
}

fn b4b_complete(cfg: &Path, dir: &Path) {
    assert_eq!(
        b4b_record(cfg, dir).expect("scope recorded").status,
        naming_migration::ScopeStatus::Complete,
        "{}",
        dir.display()
    );
}

/// E5: the live name is the registry's, and the three ignore rows are
/// composed from the registry constants. Deliberately absent from
/// `live_names_are_the_target_names` (`config/instance_gitignore.rs`):
/// naming `config::loops`, a cycle member, from that module would widen the
/// frozen table of `tests/instance_gitignore_layering.rs`.
#[test]
fn loop_state_file_is_the_registry_name_and_its_rows_derive_from_it() {
    use crate::config::instance_artifacts::{
        LOOP_STATE_TARGET_NAME, LOOP_STATE_TMP_TARGET_GLOB, SET_ASIDE_GLOB,
    };
    assert_eq!(LOOP_STATE_FILE, LOOP_STATE_TARGET_NAME);
    assert_eq!(LOOP_STATE_FILE, B4B_NEW);
    let [state, tmp, set_aside] = naming_migration::loop_state_ignore_rows();
    assert_eq!(state.0, format!("_loop_*/{LOOP_STATE_TARGET_NAME}"));
    assert_eq!(tmp.0, format!("_loop_*/{LOOP_STATE_TMP_TARGET_GLOB}"));
    assert_eq!(set_aside.0, format!("_loop_*/{SET_ASIDE_GLOB}"));
}

#[test]
fn the_loop_lock_migrates_the_state_file_and_only_a_real_loop() {
    let f = b4b_fixture();
    let a = b4b_loop(&f.ac, "a", Some(B4B_STATE_A));
    let b = b4b_loop(&f.ac, "b", Some(B4B_STATE_B));
    let other = f.ac.join("not_a_loop");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join(B4B_OLD), b"not a loop").unwrap();

    drop(b4b_lock(&a, Some(&f.cfg)));
    drop(b4b_lock(&b, Some(&f.cfg)));
    assert!(acquire_loop_lock_for_dir_in(&other, LOOP_LOCK_TIMEOUT, Some(&f.cfg)).is_err());

    for (dir, bytes) in [(&a, B4B_STATE_A), (&b, B4B_STATE_B)] {
        assert_eq!(std::fs::read_to_string(dir.join(B4B_NEW)).unwrap(), bytes);
        assert_eq!(b4b_state_files(dir), [B4B_NEW]);
        b4b_complete(&f.cfg, dir);
    }
    assert_eq!(std::fs::read(other.join(B4B_OLD)).unwrap(), b"not a loop");
}

#[test]
fn the_migration_does_not_deadlock_against_its_own_section() {
    let f = b4b_fixture();
    let a = b4b_loop(&f.ac, "a", Some(B4B_STATE_A));
    let started = std::time::Instant::now();
    let guard = b4b_lock(&a, Some(&f.cfg));
    write_loop_state_atomic(&a, &LoopState::default()).expect("write under the lock");
    drop(guard);
    assert!(
        started.elapsed() < B4B_BUDGET,
        "the section took {:?}",
        started.elapsed()
    );
    assert_eq!(b4b_state_files(&a), [B4B_NEW]);
}

#[test]
fn the_scheduler_writer_cannot_race_the_rename() {
    let f = b4b_fixture();
    let a = b4b_loop(&f.ac, "a", Some(B4B_STATE_A));
    let armed = naming_migration::pause::arm("before_rename", &b4b_canonical(&a));
    let (first_dir, first_cfg) = (a.clone(), f.cfg.clone());
    let first = std::thread::spawn(move || {
        let guard = b4b_lock(&first_dir, Some(&first_cfg));
        std::thread::sleep(Duration::from_millis(300));
        drop(guard);
    });
    armed
        .reached
        .recv_timeout(Duration::from_secs(60))
        .expect("the first leg reached the rename");
    assert_eq!(b4b_state_files(&a), [B4B_OLD], "two names at the pause");
    let returned = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (second_dir, second_cfg, flag) = (a.clone(), f.cfg.clone(), returned.clone());
    let second = std::thread::spawn(move || {
        let guard = b4b_lock(&second_dir, Some(&second_cfg));
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        drop(guard);
    });
    std::thread::sleep(Duration::from_millis(700));
    assert!(
        !returned.load(std::sync::atomic::Ordering::SeqCst),
        "the second leg took the Loop lock while the rename was pending"
    );
    armed.release.send(()).unwrap();
    first.join().unwrap();
    second.join().unwrap();
    assert!(returned.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(b4b_state_files(&a), [B4B_NEW]);
}

#[test]
fn an_unmigrated_loop_still_reports_its_real_state_unlocked() {
    let f = b4b_fixture();
    let a = b4b_loop(&f.ac, "a", Some(B4B_STATE_A));
    let state = read_loop_state(&a).expect("read");
    let (with_raw, raw) = read_loop_state_with_raw(&a).expect("read raw");
    let expected: DateTime<Utc> = "2026-09-01T10:00:00Z".parse().unwrap();
    assert_eq!(state.last_checked_at, Some(expected));
    assert_eq!(with_raw.last_checked_at, Some(expected));
    assert_eq!(raw.as_deref(), Some(B4B_STATE_A));
}

#[test]
fn the_first_write_after_the_rename_is_not_stale() {
    let f = b4b_fixture();
    let a = b4b_loop(&f.ac, "a", Some(B4B_STATE_A));
    let (_, expected) = read_loop_state_with_raw(&a).expect("unlocked read");
    let guard = b4b_lock(&a, Some(&f.cfg));
    let written =
        write_loop_state_if_unchanged(&a, &LoopState::default(), expected.as_deref()).expect("cas");
    drop(guard);
    assert_eq!(written, LoopStateWrite::Written, "the first tick was Stale");
    assert_eq!(b4b_state_files(&a), [B4B_NEW]);
}

#[test]
fn only_a_locked_section_ever_writes_the_state_file() {
    let f = b4b_fixture();
    let a = b4b_loop(&f.ac, "a", Some(B4B_STATE_A));
    let _ = read_loop_state(&a).unwrap();
    let _ = read_loop_state_with_raw(&a).unwrap();
    assert_eq!(
        b4b_state_files(&a),
        [B4B_OLD],
        "an unlocked read wrote a file"
    );
    let guard = b4b_lock(&a, Some(&f.cfg));
    write_loop_state_atomic(&a, &LoopState::default()).unwrap();
    drop(guard);
    assert_eq!(b4b_state_files(&a), [B4B_NEW]);
}

#[test]
fn no_journal_dir_still_migrates_before_the_first_write() {
    let f = b4b_fixture();
    let a = b4b_loop(&f.ac, "a", Some(B4B_STATE_A));
    let lock_sidecar = f.ac.join("._loop_a.lock");
    let guard = b4b_lock(&a, None);
    assert_eq!(
        std::fs::read_to_string(a.join(B4B_NEW)).unwrap(),
        B4B_STATE_A
    );
    assert_eq!(b4b_state_files(&a), [B4B_NEW]);
    write_loop_state_atomic(&a, &LoopState::default()).unwrap();
    drop(guard);
    assert_eq!(
        b4b_state_files(&a),
        [B4B_NEW],
        "a second file or a temporary"
    );
    assert!(
        lock_sidecar.is_file(),
        "the Loop sidecar is created once and stays"
    );
    let mut cfg_entries = std::fs::read_dir(&f.cfg).unwrap();
    assert!(
        cfg_entries.next().is_none(),
        "a journal-less run wrote a journal"
    );
}

#[test]
fn three_scope_records_coexist() {
    let f = b4b_fixture();
    let a = b4b_loop(&f.ac, "a", Some(B4B_STATE_A));
    let canonical_ac = std::fs::canonicalize(&f.ac).unwrap();
    let catalog = canonical_ac.join("coding-agents");
    std::fs::create_dir_all(&catalog).unwrap();
    // Shapes duplicated from `catalog_scope_key` (config/coding_agents_catalog.rs)
    // and `project_settings_scope_key` (config/project_settings.rs); those
    // phases' own behaviour stays their evidence.
    let catalog_key = format!("catalog:{}", catalog.display());
    let settings_key = format!("project-settings:{}", f.ac.display());
    naming_migration::update_journal(Some(&f.cfg), |j| {
        j.note(
            &catalog_key,
            "deferred: an interrupted #1968 migration is blocked",
        );
        j.set_status(&catalog_key, naming_migration::ScopeStatus::Deferred);
        j.set_status(&settings_key, naming_migration::ScopeStatus::Complete);
    })
    .unwrap();
    let before = naming_migration::read_journal(Some(&f.cfg))
        .unwrap()
        .unwrap();
    let foreign_before: Vec<serde_json::Value> = [&catalog_key, &settings_key]
        .iter()
        .map(|k| serde_json::to_value(before.scope(k).unwrap()).unwrap())
        .collect();

    drop(b4b_lock(&a, Some(&f.cfg)));

    let loop_key = loop_state_scope_key(&b4b_canonical(&a));
    assert_ne!(loop_key, catalog_key);
    assert_ne!(loop_key, settings_key);
    assert_ne!(catalog_key, settings_key);
    let after = naming_migration::read_journal(Some(&f.cfg))
        .expect("the journal still parses")
        .unwrap();
    for (key, was) in [&catalog_key, &settings_key].iter().zip(&foreign_before) {
        assert_eq!(
            &serde_json::to_value(after.scope(key).expect("foreign kept")).unwrap(),
            was,
            "{key} changed"
        );
    }
    assert_eq!(
        after.scope(&loop_key).expect("loop record").status,
        naming_migration::ScopeStatus::Complete
    );
}

fn b4b_stale_complete(cfg: &Path, dir: &Path) {
    let canonical = b4b_canonical(dir);
    let key = loop_state_scope_key(&canonical);
    naming_migration::update_journal(Some(cfg), |j| {
        j.record_step(
            &key,
            naming_migration::StepRecord::new(
                &canonical,
                B4B_OLD,
                B4B_NEW,
                naming_migration::StepState::Renamed,
            ),
        );
        j.set_status(&key, naming_migration::ScopeStatus::Complete);
    })
    .unwrap();
}

#[test]
fn a_complete_scope_whose_old_name_is_back_is_re_run() {
    let f = b4b_fixture();
    let a = b4b_loop(&f.ac, "a", Some(B4B_STATE_A));
    b4b_stale_complete(&f.cfg, &a);
    drop(b4b_lock(&a, Some(&f.cfg)));
    assert_eq!(
        b4b_state_files(&a),
        [B4B_NEW],
        "the stale Complete was trusted"
    );
    assert_eq!(
        std::fs::read_to_string(a.join(B4B_NEW)).unwrap(),
        B4B_STATE_A
    );

    let g = b4b_fixture();
    let b = b4b_loop(&g.ac, "b", Some(B4B_STATE_A));
    std::fs::write(b.join(B4B_NEW), B4B_STATE_B).unwrap();
    b4b_stale_complete(&g.cfg, &b);
    let guard = b4b_lock(&b, Some(&g.cfg));
    write_loop_state_atomic(&b, &LoopState::default()).expect("the section still runs");
    drop(guard);
    assert_eq!(
        std::fs::read_to_string(b.join(B4B_SET_ASIDE)).unwrap(),
        B4B_STATE_A
    );
    assert_eq!(b4b_state_files(&b), [B4B_NEW, B4B_SET_ASIDE]);
    b4b_complete(&g.cfg, &b);
}

#[test]
fn the_loop_scope_does_not_re_acquire_its_own_lock() {
    let f = b4b_fixture();
    let a = b4b_loop(&f.ac, "a", Some(B4B_STATE_A));
    let started = std::time::Instant::now();
    drop(b4b_lock(&a, Some(&f.cfg)));
    assert!(
        started.elapsed() < B4B_BUDGET,
        "took {:?}",
        started.elapsed()
    );
}

#[test]
fn a_failed_sweep_leaves_the_loop_wholly_on_its_old_name() {
    let f = b4b_fixture();
    let a = b4b_loop(&f.ac, "a", Some(B4B_STATE_A));
    std::fs::create_dir(f.ac.join(".gitignore")).unwrap();
    let guard = b4b_lock(&a, Some(&f.cfg));
    assert_eq!(
        b4b_state_files(&a),
        [B4B_OLD],
        "renamed past a failed sweep"
    );
    write_loop_state_atomic(&a, &LoopState::default()).expect("schedule write");
    drop(guard);
    assert_eq!(
        b4b_state_files(&a),
        [B4B_OLD],
        "a write after a failed sweep created the new name"
    );

    std::fs::remove_dir(f.ac.join(".gitignore")).unwrap();
    let guard = b4b_lock(&a, Some(&f.cfg));
    write_loop_state_atomic(&a, &LoopState::default()).unwrap();
    drop(guard);
    assert_eq!(b4b_state_files(&a), [B4B_NEW]);
}

// -- E14b, the sweep of 4.5 ------------------------------------------------

const B4B_ROWS: [(&str, &str); 3] = [
    (
        "_loop_*/loop.state.no-git.json",
        "# AgentsCommander: exclude Loop scheduler runtime state.",
    ),
    (
        "_loop_*/loop.state.no-git.json.*.tmp",
        "# AgentsCommander: exclude Loop state write temporaries.",
    ),
    (
        "_loop_*/*.deprecated-*.no-git",
        "# AgentsCommander: exclude Loop files the naming migration set aside.",
    ),
];

fn b4b_assert_rows_once(content: &str, label: &str) {
    let lines: Vec<&str> = content.lines().collect();
    for (pattern, comment) in B4B_ROWS {
        let at: Vec<usize> = (0..lines.len()).filter(|&i| lines[i] == pattern).collect();
        assert_eq!(at.len(), 1, "{label}: {pattern} is not there exactly once");
        assert!(
            at[0] > 0 && lines[at[0] - 1] == comment,
            "{label}: {pattern} lacks its comment"
        );
    }
}

fn b4b_git(cwd: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git must execute")
}

#[test]
fn the_loop_scope_ignores_the_new_names_before_it_renames_anything() {
    let f = b4b_fixture();
    let project = f.ac.parent().unwrap().to_path_buf();
    std::fs::write(
        f.ac.join(".gitignore"),
        "# AgentsCommander: exclude Loop scheduler runtime state.\n_loop_*/state.json\n",
    )
    .unwrap();
    let a = b4b_loop(&f.ac, "a", Some(B4B_STATE_A));
    drop(b4b_lock(&a, Some(&f.cfg)));
    let content = std::fs::read_to_string(f.ac.join(".gitignore")).unwrap();
    b4b_assert_rows_once(&content, "base");
    assert!(
        content.lines().any(|l| l == "_loop_*/state.json"),
        "append-only"
    );
    assert!(b4b_git(&project, &["init", "--quiet"]).status.success());
    for (name, rule) in [
        (B4B_NEW, B4B_ROWS[0].0),
        ("loop.state.no-git.json.1.tmp", B4B_ROWS[1].0),
        (B4B_SET_ASIDE, B4B_ROWS[2].0),
    ] {
        std::fs::write(a.join(name), b"").unwrap();
        let relative = format!(".ac/_loop_a/{name}");
        let out = b4b_git(
            &project,
            &["check-ignore", "-v", "--no-index", "--", &relative],
        );
        assert!(out.status.success(), "{relative} is not ignored");
        let stdout = String::from_utf8(out.stdout).unwrap();
        let (source, _) = stdout.trim_end().split_once('\t').expect("verbose output");
        assert!(
            source.ends_with(&format!(":{rule}")),
            "{relative}: {source}"
        );
    }

    // Both orders with the registration writer: every AC row once.
    for sweep_first in [true, false] {
        let k = b4b_fixture();
        std::fs::write(k.ac.join(".gitignore"), "user-line-2718\n").unwrap();
        let dir = b4b_loop(&k.ac, "a", Some(B4B_STATE_A));
        let writer = || {
            crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(&k.ac, &[])
                .expect("writer")
        };
        if sweep_first {
            drop(b4b_lock(&dir, Some(&k.cfg)));
            writer();
        } else {
            writer();
            drop(b4b_lock(&dir, Some(&k.cfg)));
        }
        let content = std::fs::read_to_string(k.ac.join(".gitignore")).unwrap();
        let label = if sweep_first {
            "sweep, writer"
        } else {
            "writer, sweep"
        };
        b4b_assert_rows_once(&content, label);
        assert!(content.lines().any(|l| l == "user-line-2718"), "{label}");
        let rules: Vec<&str> = content
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .collect();
        let unique: std::collections::BTreeSet<&str> = rules.iter().copied().collect();
        assert_eq!(unique.len(), rules.len(), "{label}: a row appears twice");
    }

    // Interleaved: the writer and a user edit between the sweep's read and
    // its append. Nothing lost; a 4.3 row at most twice; nothing else twice.
    let expected: Vec<String> = {
        let fresh = b4b_fixture();
        crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(&fresh.ac, &[])
            .expect("writer");
        std::fs::read_to_string(fresh.ac.join(".gitignore"))
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(str::to_string)
            .collect()
    };
    let m = b4b_fixture();
    std::fs::write(m.ac.join(".gitignore"), "user-line-2718\n").unwrap();
    let dir = b4b_loop(&m.ac, "a", Some(B4B_STATE_A));
    let canonical_ac = std::fs::canonicalize(&m.ac).unwrap();
    let armed = naming_migration::pause::arm("before_gitignore_append", &canonical_ac);
    let (lock_dir, cfg) = (dir.clone(), m.cfg.clone());
    let sweep = std::thread::spawn(move || drop(b4b_lock(&lock_dir, Some(&cfg))));
    armed
        .reached
        .recv_timeout(Duration::from_secs(60))
        .expect("the sweep reached its append");
    crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(&m.ac, &[])
        .expect("writer in the window");
    {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(m.ac.join(".gitignore"))
            .unwrap();
        file.write_all(b"user-line-late\n").unwrap();
    }
    armed.release.send(()).unwrap();
    sweep.join().unwrap();
    let content = std::fs::read_to_string(m.ac.join(".gitignore")).unwrap();
    let count = |rule: &str| content.lines().filter(|l| *l == rule).count();
    for user in ["user-line-2718", "user-line-late"] {
        assert_eq!(count(user), 1, "interleaved: the user line {user} was lost");
    }
    for rule in &expected {
        let n = count(rule);
        let own = B4B_ROWS.iter().any(|(pattern, _)| pattern == rule);
        assert!(n >= 1, "interleaved: the writer's rule {rule} was lost");
        assert!(
            n <= if own { 2 } else { 1 },
            "interleaved: {rule} appears {n} times"
        );
    }
}

// -- E13, two processes ----------------------------------------------------

const B4B_CHILD_FQN: &str = "config::loops::tests::state_rename_migration::b4b_child_process_entry";

const B4B_CHILD_DIR: &str = "AC_2718_CHILD_LOOP_DIR";

const B4B_CHILD_CFG: &str = "AC_2718_CHILD_CFG";

fn b4b_spawn(dir: &Path, cfg: &Path, pause_dir: Option<&Path>) -> std::process::Child {
    let mut command =
        std::process::Command::new(std::env::current_exe().expect("current test exe"));
    command
        .args(["--exact", B4B_CHILD_FQN, "--nocapture", "--test-threads=1"])
        .env(B4B_CHILD_DIR, dir)
        .env(B4B_CHILD_CFG, cfg)
        .env_remove(naming_migration::pause::PAUSE_DIR_ENV)
        .env_remove(naming_migration::pause::PAUSE_STAGE_ENV)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(rendezvous) = pause_dir {
        command
            .env(naming_migration::pause::PAUSE_DIR_ENV, rendezvous)
            .env(naming_migration::pause::PAUSE_STAGE_ENV, "after_rename");
    }
    command.spawn().expect("spawn child")
}

fn b4b_wait_child(child: std::process::Child) {
    let output = child.wait_with_output().expect("child output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "child failed or ran nothing: {}\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Not a test of its own: the body a spawned child runs, inert otherwise.
#[test]
fn b4b_child_process_entry() {
    let Some(dir) = std::env::var_os(B4B_CHILD_DIR) else {
        return;
    };
    let cfg = PathBuf::from(std::env::var_os(B4B_CHILD_CFG).expect("cfg"));
    drop(b4b_lock(Path::new(&dir), Some(&cfg)));
}

#[test]
fn two_loop_scopes_keep_both_record_sets() {
    let f = b4b_fixture();
    let a = b4b_loop(&f.ac, "a", Some(B4B_STATE_A));
    let b = b4b_loop(&f.ac, "b", Some(B4B_STATE_B));
    let rendezvous = f.cfg.join("rv");
    std::fs::create_dir_all(&rendezvous).unwrap();
    let first = b4b_spawn(&a, &f.cfg, Some(&rendezvous));
    let ready = rendezvous.join(naming_migration::pause::READY_FILE);
    let started = std::time::Instant::now();
    while !ready.exists() {
        assert!(started.elapsed() < Duration::from_secs(60), "no pause");
        std::thread::sleep(Duration::from_millis(20));
    }
    // While the first holds loop a's lock mid-scope, the second commits.
    b4b_wait_child(b4b_spawn(&b, &f.cfg, None));
    std::fs::write(
        rendezvous.join(naming_migration::pause::RELEASE_FILE),
        b"go",
    )
    .unwrap();
    b4b_wait_child(first);

    for (dir, bytes) in [(&a, B4B_STATE_A), (&b, B4B_STATE_B)] {
        let record = b4b_record(&f.cfg, dir).expect("both scopes recorded");
        assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
        assert!(record
            .steps
            .iter()
            .any(|s| s.state == naming_migration::StepState::Renamed));
        assert_eq!(std::fs::read_to_string(dir.join(B4B_NEW)).unwrap(), bytes);
    }
}
