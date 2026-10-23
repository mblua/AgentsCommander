use super::*;
use std::collections::BTreeMap as Map;
use std::ffi::OsString;
use std::process::{Child, Command, Stdio};

mod concurrency;
mod scenarios;

// Synthetic names only: a product name here would make the engine agree
// with a later phase by construction instead of by observation.
const FROM: &str = "legacy.json";
const TO: &str = "target.json";
const NEW_LOCK: &str = ".target.json.lock";
const OLD_LOCK: &str = ".legacy.json.lock";
const SCOPE: &str = "synthetic";
const STEP: Rename = Rename { from: FROM, to: TO };
const ONE_STEP: [Rename; 1] = [STEP];
const MANY: [Rename; 6] = [
    Rename {
        from: "legacy-0.json",
        to: "target-0.json",
    },
    Rename {
        from: "legacy-1.json",
        to: "target-1.json",
    },
    Rename {
        from: "legacy-2.json",
        to: "target-2.json",
    },
    Rename {
        from: "legacy-3.json",
        to: "target-3.json",
    },
    Rename {
        from: "legacy-4.json",
        to: "target-4.json",
    },
    Rename {
        from: "legacy-5.json",
        to: "target-5.json",
    },
];

const CHILD_FQN: &str = "config::naming_migration::tests::child_process_entry";
const CHILD_ROLE_ENV: &str = "AC_2713_CHILD_ROLE";
const CHILD_DATA_ENV: &str = "AC_2713_CHILD_DATA_DIR";
const CHILD_JOURNAL_ENV: &str = "AC_2713_CHILD_JOURNAL_DIR";
const CHILD_SCOPE_ENV: &str = "AC_2713_CHILD_SCOPE";
const CHILD_RENDEZVOUS_ENV: &str = "AC_2713_CHILD_RENDEZVOUS";
const WAIT_BOUND: Duration = Duration::from_secs(60);

fn aside(n: u32) -> String {
    format!("{FROM}.deprecated-{n}.no-git")
}

struct Fixture {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    cfg: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data = tmp.path().join("data");
    let cfg = tmp.path().join("cfg");
    std::fs::create_dir_all(&data).expect("data dir");
    std::fs::create_dir_all(&cfg).expect("cfg dir");
    Fixture {
        _tmp: tmp,
        data,
        cfg,
    }
}

fn put(dir: &Path, name: &str, bytes: &[u8]) {
    std::fs::write(dir.join(name), bytes).expect("seed file");
}

fn get(dir: &Path, name: &str) -> Option<Vec<u8>> {
    match std::fs::read(dir.join(name)) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => panic!("read {name}: {e}"),
    }
}

/// Every file in `dir` with its bytes; lock sidecars by name only.
fn snapshot(dir: &Path) -> Map<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .expect("list dir")
        .map(|entry| {
            let entry = entry.expect("dir entry");
            let name = entry.file_name().into_string().expect("utf-8 name");
            // A held sidecar cannot be read on Windows; its bytes are empty.
            let bytes = if name.ends_with(".lock") {
                Vec::new()
            } else {
                std::fs::read(entry.path()).expect("read entry")
            };
            (name, bytes)
        })
        .collect()
}

fn journal(cfg: &Path) -> Journal {
    read_journal(Some(cfg))
        .expect("readable journal")
        .expect("journal exists")
}

fn steps_of(cfg: &Path, scope: &str) -> Vec<StepRecord> {
    journal(cfg)
        .scope(scope)
        .expect("scope recorded")
        .steps
        .clone()
}

fn lock(dir: &Path) -> ScopeLock {
    lock_scope(dir, NEW_LOCK, Some(OLD_LOCK), MIGRATION_LOCK_BUDGET).expect("scope lock")
}

/// Creates the two lock sidecars, so an entry count taken afterwards sees
/// only what the migration itself adds.
fn prime_locks(dir: &Path) {
    drop(lock(dir));
}

/// The caller shape every phase follows: a pre-check, the scope lock, the
/// re-check under it through `update_journal`, the steps, then `Complete`.
fn run_scope_with(
    dir: &Path,
    steps: &[Rename],
    family: Option<(&str, &str, &str)>,
    scope: &str,
    journal_dir: Option<&Path>,
    timeout: Duration,
) -> Result<Vec<Outcome>, Refusal> {
    if scope_is_settled(read_journal(journal_dir)?.as_ref(), scope) {
        return Ok(Vec::new());
    }
    let held = lock_scope(dir, NEW_LOCK, Some(OLD_LOCK), timeout)?;
    if update_journal(journal_dir, |j| scope_is_settled(Some(j), scope))?.unwrap_or(false) {
        return Ok(Vec::new());
    }
    let mut outcomes: Vec<Outcome> = steps
        .iter()
        .map(|r| rename_step(dir, r, scope, journal_dir, &held))
        .collect();
    if let Some((old_prefix, new_prefix, suffix)) = family {
        outcomes.extend(
            rename_prefix_family(
                dir,
                old_prefix,
                new_prefix,
                suffix,
                scope,
                journal_dir,
                &held,
            )
            .into_iter()
            .map(|(_, outcome)| outcome),
        );
    }
    if outcomes.iter().any(|o| matches!(o, Outcome::Refused(_))) {
        return Ok(outcomes);
    }
    migration_pause_hook("before_scope_complete", dir);
    update_journal(journal_dir, |j| j.set_status(scope, ScopeStatus::Complete))?;
    Ok(outcomes)
}

fn run_scope(
    dir: &Path,
    steps: &[Rename],
    scope: &str,
    journal_dir: Option<&Path>,
) -> Vec<Outcome> {
    run_scope_with(dir, steps, None, scope, journal_dir, MIGRATION_LOCK_BUDGET).expect("scope ran")
}

fn assert_complete(cfg: &Path, scope: &str) {
    assert_eq!(status(&journal(cfg), scope), Some(ScopeStatus::Complete));
}

// -- cross-process plumbing ----------------------------------------------

fn spawn_child(role: &str, envs: &[(&str, OsString)]) -> Child {
    let mut command = Command::new(std::env::current_exe().expect("current test exe"));
    command
        .args(["--exact", CHILD_FQN, "--nocapture", "--test-threads=1"])
        .env(CHILD_ROLE_ENV, role)
        .env_remove(pause::PAUSE_DIR_ENV)
        .env_remove(pause::PAUSE_STAGE_ENV)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in envs {
        command.env(key, value);
    }
    command.spawn().expect("spawn child")
}

fn wait_for_file(path: &Path) {
    let started = Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < WAIT_BOUND,
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_child(mut child: Child) {
    let started = Instant::now();
    while child.try_wait().expect("poll child").is_none() {
        if started.elapsed() >= WAIT_BOUND {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child exceeded {WAIT_BOUND:?} and was killed");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().expect("child output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "child failed or ran nothing: {}\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn seed_many(dir: &Path, tag: &str) {
    for r in &MANY {
        put(dir, r.from, format!("{tag}:{}", r.from).as_bytes());
    }
}

fn assert_many_complete(cfg: &Path, data: &Path, scope: &str, tag: &str) {
    let steps = steps_of(cfg, scope);
    assert_eq!(
        steps.len(),
        MANY.len(),
        "{scope} lost step records: {steps:?}"
    );
    for r in &MANY {
        let record = steps
            .iter()
            .find(|s| s.from == r.from && s.to == r.to)
            .unwrap_or_else(|| panic!("{scope} lost the record of {}", r.from));
        assert_eq!(record.state, StepState::Renamed);
        assert_eq!(
            get(data, r.to),
            Some(format!("{tag}:{}", r.from).into_bytes())
        );
    }
    assert_complete(cfg, scope);
}

/// Not a test of its own: the body a spawned child runs, inert otherwise.
#[test]
fn child_process_entry() {
    let Some(role) = std::env::var_os(CHILD_ROLE_ENV) else {
        return;
    };
    let data = PathBuf::from(std::env::var_os(CHILD_DATA_ENV).expect("data dir"));
    match role.to_str() {
        Some("scope") => {
            let cfg = PathBuf::from(std::env::var_os(CHILD_JOURNAL_ENV).expect("journal dir"));
            let scope = std::env::var(CHILD_SCOPE_ENV).expect("scope");
            let outcomes = run_scope(&data, &MANY, &scope, Some(&cfg));
            assert!(
                outcomes.iter().all(|o| *o == Outcome::Renamed),
                "{outcomes:?}"
            );
        }
        Some("hold") => {
            let rendezvous =
                PathBuf::from(std::env::var_os(CHILD_RENDEZVOUS_ENV).expect("rendezvous"));
            let _held = lock(&data);
            std::fs::write(rendezvous.join(pause::READY_FILE), b"ready").expect("ready");
            wait_for_file(&rendezvous.join(pause::RELEASE_FILE));
        }
        other => panic!("unknown child role {other:?}"),
    }
}

#[test]
fn pause_hook_is_disarmed_by_default() {
    assert!(std::env::var_os(pause::PAUSE_DIR_ENV).is_none());
    assert!(std::env::var_os(pause::PAUSE_STAGE_ENV).is_none());
    let tmp = tempfile::tempdir().expect("tempdir");
    let started = Instant::now();
    for stage in ["before_rename", "after_rename", "before_set_aside_move"] {
        migration_pause_hook(stage, tmp.path());
    }
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[derive(Clone, Copy, Debug)]
enum Disk {
    FromOnly,
    ToOnly,
    Neither,
    Both,
    BothWithAside,
}

fn seed(dir: &Path, disk: Disk) {
    match disk {
        Disk::FromOnly => put(dir, FROM, b"old"),
        Disk::ToOnly => put(dir, TO, b"new"),
        Disk::Neither => {}
        Disk::Both => {
            put(dir, FROM, b"old");
            put(dir, TO, b"new");
        }
        Disk::BothWithAside => {
            put(dir, FROM, b"old");
            put(dir, TO, b"new");
            put(dir, &aside(1), b"earlier");
        }
    }
}
