use super::activity_stamp::{T1, T2};
use super::*;

// ── #2807 (C2): the config pair entry point ─────────────────────────────

pub(super) const CLEANUP_STATE: &str = "after_cleanup_state_publish";
pub(super) const CLEANUP_TRACKED: &str = "after_cleanup_tracked_write";
pub(super) const CALLER_STATE: &str = "after_caller_state_publish";
pub(super) const CALLER_DECISIONS: &str = "after_caller_decisions_write";
/// Fired by no production build: only E1f's scratch mutant opens this gap.
pub(super) const MUTANT_GAP: &str = "mutant_gap_open";

const PAIR_CHILD_FQN: &str = "config::agent_config::tests::pair_fixture::config_pair_child_entry";
const PAIR_CHILD_DECISIONS_ENV: &str = "AC_2807_CHILD_DECISIONS";
const PAIR_CHILD_JOURNAL_ENV: &str = "AC_2807_CHILD_JOURNAL";
const PAIR_CHILD_RENDEZVOUS_ENV: &str = "AC_2807_CHILD_RENDEZVOUS";
const PAIR_CHILD_STOPS_ENV: &str = "AC_2807_CHILD_STOPS";
const PAIR_CHILD_MUTATE_ENV: &str = "AC_2807_CHILD_MUTATE";
const PAIR_CHILD_SEQUENCE_ENV: &str = "AC_2807_CHILD_SEQUENCE";
const READY_FILE: &str = "paused.ready";
const RELEASE_FILE: &str = "paused.release";
const STAGES_LOG: &str = "stages.log";
pub(super) const HOOK_BOUND: Duration = Duration::from_secs(60);

/// One agent directory under an `.ac` root, plus an instance journal
/// directory outside it.
pub(super) struct Pair {
    pub(super) tmp: tempfile::TempDir,
    pub(super) dir: PathBuf,
    pub(super) journal: PathBuf,
}

pub(super) fn pair() -> Pair {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join(".ac").join("_agent_a");
    let journal = tmp.path().join("journal");
    std::fs::create_dir_all(&dir).expect("create agent dir");
    std::fs::create_dir_all(&journal).expect("create journal dir");
    Pair { tmp, dir, journal }
}

pub(super) fn put_json(path: &Path, value: &Value) {
    let mut text = serde_json::to_string_pretty(value).expect("serialize");
    text.push('\n');
    std::fs::write(path, text).expect("seed file");
}

pub(super) fn get_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("read file")).expect("JSON")
}

pub(super) fn bytes_of(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

pub(super) fn split_marker() -> Value {
    json!({ "v": 1, "keys": STATE_KEYS })
}

/// A state file that carries the split marker.
pub(super) fn marked(tooling: Value) -> Value {
    json!({ "tooling": tooling, "split": split_marker() })
}

pub(super) fn state_keys_in(file: &Value) -> Vec<&'static str> {
    STATE_KEYS
        .into_iter()
        .filter(|key| file["tooling"].get(key).is_some())
        .collect()
}

pub(super) fn no_change(_: &mut JsonMap, _: &mut JsonMap) -> Result<(), String> {
    Ok(())
}

pub(super) fn set_tooling(map: &mut JsonMap, key: &str, value: Value) {
    map.entry("tooling")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("tooling object")
        .insert(key.to_string(), value);
}

/// Every file under `root`, with its bytes; a directory is `None`.
pub(super) fn tree_of(root: &Path) -> std::collections::BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(dir: &Path, out: &mut std::collections::BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in std::fs::read_dir(dir).expect("read dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                out.insert(path.clone(), None);
                visit(&path, out);
            } else {
                out.insert(path.clone(), Some(std::fs::read(&path).expect("read")));
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    visit(root, &mut out);
    out
}

pub(super) fn append_line(path: &Path, line: &str) {
    use std::io::Write as _;
    // One write in append mode: atomic for a short line, from any process.
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(format!("{line}\n").as_bytes()))
        .expect("append line");
}

pub(super) fn lines_of(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

impl Pair {
    pub(super) fn decisions(&self) -> PathBuf {
        self.dir.join("config.json")
    }

    pub(super) fn state(&self) -> PathBuf {
        state_file_path(&self.dir)
    }

    pub(super) fn seed_decisions(&self, value: Value) {
        put_json(&self.decisions(), &value);
    }

    pub(super) fn seed_state(&self, value: Value) {
        put_json(&self.state(), &value);
    }

    pub(super) fn tracked(&self) -> Value {
        get_json(&self.decisions())
    }

    pub(super) fn stored_state(&self) -> Value {
        get_json(&self.state())
    }

    /// The value a reader gets, through the loader.
    pub(super) fn read(&self, key: &str) -> Value {
        read_agent_local_config_json(&self.dir)
            .expect("loader")
            .expect("a config")["tooling"][key]
            .clone()
    }

    pub(super) fn write<F>(&self, mutate: F) -> Result<(), String>
    where
        F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
    {
        update_agent_config_in(&self.decisions(), Some(&self.journal), mutate)
    }

    pub(super) fn write_with_publish<F>(&self, mutate: F) -> Result<(), String>
    where
        F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
    {
        update_agent_config_with_activity_and_publish(
            &self.decisions(),
            Some(&self.journal),
            &|_: &str| {},
            mutate,
            None,
            Some(&|tmp, target| self.publish(tmp, target)),
        )
    }

    pub(super) fn publish(&self, tmp: &Path, target: &Path) -> Result<(), String> {
        if (target == self.decisions() && self.decisions_temp().is_dir())
            || (target == self.state() && self.state_temp().is_dir())
        {
            assert!(
                tmp.is_file(),
                "fresh staging acquired before injected refusal"
            );
            return Err("fixture temp config publish failure".into());
        }
        crate::config::local_config_io::publish_temp_config(tmp, target)
    }

    /// A writer outside this binary's protocol: it puts a state key back
    /// into the tracked file with a plain write.
    pub(super) fn foreign_write(&self, key: &str, value: Value) {
        let mut tracked = self.tracked();
        set_tooling(tracked.as_object_mut().expect("object"), key, value);
        put_json(&self.decisions(), &tracked);
    }

    pub(super) fn publish_temp(&self, file_name: &str) -> PathBuf {
        self.dir
            .join(format!(".{file_name}.{}.tmp", std::process::id()))
    }

    pub(super) fn decisions_temp(&self) -> PathBuf {
        self.publish_temp("config.json")
    }

    pub(super) fn state_temp(&self) -> PathBuf {
        self.publish_temp(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
    }

    pub(super) fn journal_bytes(&self) -> Option<Vec<u8>> {
        bytes_of(
            &self
                .journal
                .join(crate::config::instance_artifacts::NAMING_MIGRATION_STATE_NAME),
        )
    }
}

/// A child process driving `update_agent_config_with_stage`, stopped at
/// named stages through one rendezvous subdirectory per stage.
pub(super) struct PairChild {
    pub(super) child: std::process::Child,
    pub(super) rendezvous: PathBuf,
}

pub(super) fn spawn_pair_child(
    pair: &Pair,
    stops: &[&str],
    mutate: &Value,
    sequence: Option<&Path>,
) -> PairChild {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let rendezvous = pair.tmp.path().join(format!(
        "rendezvous-{}",
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    for stage in stops {
        std::fs::create_dir_all(rendezvous.join(stage)).expect("create rendezvous");
    }
    let mut command =
        std::process::Command::new(std::env::current_exe().expect("current test exe"));
    command
        .args(["--exact", PAIR_CHILD_FQN, "--nocapture", "--test-threads=1"])
        .env(PAIR_CHILD_DECISIONS_ENV, pair.decisions())
        .env(PAIR_CHILD_JOURNAL_ENV, &pair.journal)
        .env(PAIR_CHILD_RENDEZVOUS_ENV, &rendezvous)
        .env(PAIR_CHILD_STOPS_ENV, stops.join(","))
        .env(PAIR_CHILD_MUTATE_ENV, mutate.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    if let Some(sequence) = sequence {
        command.env(PAIR_CHILD_SEQUENCE_ENV, sequence);
    }
    PairChild {
        child: command.spawn().expect("spawn the pair child"),
        rendezvous,
    }
}

impl PairChild {
    pub(super) fn ready(&self, stage: &str) -> bool {
        self.rendezvous.join(stage).join(READY_FILE).exists()
    }

    /// The first of `stages` the child stops at, or `None` when the bound
    /// passes or the child exits first.
    pub(super) fn first_ready<'a>(
        &mut self,
        stages: &[&'a str],
        bound: Duration,
    ) -> Option<&'a str> {
        let started = Instant::now();
        loop {
            if let Some(stage) = stages.iter().copied().find(|stage| self.ready(stage)) {
                return Some(stage);
            }
            let exited = !matches!(self.child.try_wait(), Ok(None));
            if exited || started.elapsed() >= bound {
                return stages.iter().copied().find(|stage| self.ready(stage));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub(super) fn release(&self, stage: &str) {
        std::fs::write(self.rendezvous.join(stage).join(RELEASE_FILE), b"release")
            .expect("release the child");
    }

    pub(super) fn kill(&mut self) -> String {
        let killed = self.child.kill();
        format!("kill {killed:?}, exit {:?}", self.child.wait())
    }

    /// Waits for the child to end by itself.
    pub(super) fn exit(&mut self) -> std::process::ExitStatus {
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status;
            }
            assert!(
                started.elapsed() < HOOK_BOUND,
                "the pair child did not end within 60 s"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Every stage the child fired, in order.
    pub(super) fn stages(&self) -> Vec<String> {
        lines_of(&self.rendezvous.join(STAGES_LOG))
    }

    /// Stops the child at `stage`, kills it there, and returns the stages
    /// it fired.
    pub(super) fn kill_at(&mut self, stage: &str) -> Vec<String> {
        let reached = self.first_ready(&[stage], HOOK_BOUND);
        let stages = self.stages();
        let status = self.kill();
        assert_eq!(
            reached,
            Some(stage),
            "the child never stopped at {stage}: fired {stages:?}, {status}"
        );
        assert_eq!(
            stages.last().map(String::as_str),
            Some(stage),
            "the kill must happen at {stage}"
        );
        stages
    }
}

impl Drop for PairChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Not a test of its own: the body a spawned child runs, inert otherwise.
#[test]
fn config_pair_child_entry() {
    let Some(decisions) = std::env::var_os(PAIR_CHILD_DECISIONS_ENV) else {
        return;
    };
    let decisions = PathBuf::from(decisions);
    let journal = std::env::var_os(PAIR_CHILD_JOURNAL_ENV).map(PathBuf::from);
    let rendezvous = PathBuf::from(std::env::var_os(PAIR_CHILD_RENDEZVOUS_ENV).expect("dir"));
    let stops = std::env::var(PAIR_CHILD_STOPS_ENV).expect("stops");
    let stops: Vec<&str> = stops.split(',').filter(|s| !s.is_empty()).collect();
    let sequence = std::env::var_os(PAIR_CHILD_SEQUENCE_ENV).map(PathBuf::from);
    let mutate: Value =
        serde_json::from_str(&std::env::var(PAIR_CHILD_MUTATE_ENV).expect("mutate"))
            .expect("mutate JSON");

    let on_stage = |stage: &str| {
        append_line(&rendezvous.join(STAGES_LOG), stage);
        if !stops.contains(&stage) {
            return;
        }
        // The line first, the ready signal second, so a kill loses no line.
        if let Some(sequence) = &sequence {
            let line = match stage {
                CLEANUP_TRACKED => "child:paused",
                MUTANT_GAP => "child:at-gap",
                CALLER_STATE => "child:published v3",
                other => panic!("no sequence line for {other}"),
            };
            append_line(sequence, line);
        }
        let dir = rendezvous.join(stage);
        std::fs::write(dir.join(READY_FILE), b"ready").expect("announce pause");
        let started = Instant::now();
        while !dir.join(RELEASE_FILE).exists() {
            assert!(started.elapsed() < HOOK_BOUND, "pause exceeded 60 s");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    update_agent_config_with_stage(&decisions, journal.as_deref(), &on_stage, |d, s| {
        for (side, map) in [("decisions", d), ("state", s)] {
            let Some(entries) = mutate.get(side).and_then(Value::as_object) else {
                continue;
            };
            for (key, value) in entries {
                set_tooling(map, key, value.clone());
            }
        }
        Ok(())
    })
    .expect("the child's write");
}

pub(super) fn sample_state_values() -> [(&'static str, Value); 4] {
    [
        ("lastCodingAgent", json!("claude")),
        (
            "codingAgents",
            json!({ "claude": { "app": "Claude Code", "lastUsed": T1 } }),
        ),
        ("lastAgentMessageAt", json!(T2)),
        ("profileContentHash", json!("9f2c41")),
    ]
}

/// The JSON text of a value, so "byte for byte" is compared as bytes.
pub(super) fn text(value: &Value) -> String {
    serde_json::to_string(value).expect("serialize")
}

// -- E1 ------------------------------------------------------------------
