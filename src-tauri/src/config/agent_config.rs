use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

// ── Agent Identity ──────────────────────────────────────────────────────────
/// What the agent IS: name, role, memory location.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIdentity {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub root_path: String,
    /// Relative path to the role declaration file (e.g. "CLAUDE.md")
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role_path: String,
    /// Relative path to the memory store (e.g. ".claude/memory")
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub memory_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl AgentIdentity {
    pub fn is_empty(&self) -> bool {
        self.name.is_empty()
            && self.root_path.is_empty()
            && self.role_path.is_empty()
            && self.memory_path.is_empty()
            && self.description.is_empty()
    }
}

// ── Agent Tooling ──────────────────────────────────────────────────────────
/// Entry tracking a coding app (Claude Code, Codex, OpenCode, etc.) used in this repo.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentEntry {
    /// Human-readable app name (e.g. "Claude Code", "Codex", "OpenCode")
    #[serde(default)]
    pub app: String,
    /// AgentsCommander session ID (to check if session is still alive)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ac_session_id: Option<String>,
    /// ISO 8601 timestamp of last use
    #[serde(default)]
    pub last_used: String,
    /// #2433 - `canonical_command_text` of the agent's command, computed by the
    /// caller. Empty when an older build wrote the entry.
    #[serde(default)]
    pub command: String,
    /// #2433 - enabled profile letter -> `cell_identity` digest, computed by the
    /// caller. An agent with no enabled cells writes `{}`; an absent key means an
    /// older build wrote the entry.
    #[serde(default)]
    pub identity: BTreeMap<String, String>,
}

/// Keys of a `codingAgents` entry this build owns. `upsert_config` removes
/// them before merging so an optional one this write omits does not linger;
/// any other key (written by a newer build) survives.
const CODING_AGENT_ENTRY_KEYS: [&str; 3] = ["app", "acSessionId", "lastUsed"];

/// #2433 - the descriptor keys. Written (and replaced) only when the caller
/// supplies a descriptor; otherwise an existing descriptor is left untouched.
const CODING_AGENT_DESCRIPTOR_KEYS: [&str; 2] = ["command", "identity"];

/// Which coding apps have been used to run this agent, plus runtime config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTooling {
    /// Last agent config ID used (maps to AgentConfig.id in settings.json)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_coding_agent: Option<String>,
    /// Selection UI coding agent assignment. Does not replace lastCodingAgent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_coding_agent: Option<String>,
    /// Selection UI profile assignment. Legacy instanceProfileOverride is
    /// read separately from raw JSON during the migration window.
    #[serde(
        default,
        alias = "instanceProfileOverride",
        skip_serializing_if = "Option::is_none"
    )]
    pub profile: Option<String>,
    /// Per-agent-config-id history of coding apps used in this repo
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub coding_agents: HashMap<String, CodingAgentEntry>,
    /// #1682 - RFC3339/UTC instant of the most recent busy->idle edge on a session
    /// that a message write armed and that the stamp gates judged an agent turn. That
    /// is this plan's proxy for the coding agent having finished responding, NOT a
    /// proof of it: R7 and R8 arm with nothing submitted. Distinct from
    /// `codingAgents[<id>].lastUsed`, which is when a coding agent was last LAUNCHED.
    /// Written only by `set_last_agent_message_at`; read only by the terminal status strip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_agent_message_at: Option<String>,
    /// Telegram bot label to auto-attach when creating sessions for this agent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_bot: Option<String>,
}

impl AgentTooling {
    pub fn is_empty(&self) -> bool {
        self.last_coding_agent.is_none()
            && self.current_coding_agent.is_none()
            && self.profile.is_none()
            && self.coding_agents.is_empty()
            && self.last_agent_message_at.is_none()
            && self.telegram_bot.is_none()
    }
}

// ── Legacy Dark Factory fields (kept for backwards-compatible deserialization) ──
/// Preserved so existing config.json files with a "darkFactory" key can still be read.
/// No longer written or used for routing — teams come from discovery.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDarkFactory {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teams: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub is_coordinator_of: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supervises: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reports_to: Vec<String>,
}

impl AgentDarkFactory {
    pub fn is_empty(&self) -> bool {
        self.teams.is_empty()
            && self.is_coordinator_of.is_empty()
            && self.supervises.is_empty()
            && self.reports_to.is_empty()
    }
}

// ── Per-agent config (the root struct) ─────────────────────────────────────
/// Written to <agent-path>/.agentscommander/config.json
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLocalConfig {
    #[serde(default, skip_serializing_if = "AgentIdentity::is_empty")]
    pub agent: AgentIdentity,
    #[serde(default, skip_serializing_if = "AgentTooling::is_empty")]
    pub tooling: AgentTooling,
    /// Legacy field — kept for backwards-compatible reads of old config.json files.
    /// No longer used for routing. Teams are discovered from _team_*/config.json.
    #[serde(default, skip_serializing_if = "AgentDarkFactory::is_empty")]
    pub dark_factory: AgentDarkFactory,
}

/// Update lastCodingAgent and codingAgents in a repo's config.
/// Writes to BOTH:
///  - `<repo_path>/config.json` (root, shared across all instances — read by discovery)
///  - `<repo_path>/<agent_local_dir>/config.json` (per-instance)
///
/// Reads existing config, upserts the coding agent entry, writes back.
pub fn set_last_coding_agent(
    repo_path: &str,
    agent_id: &str,
    app_label: &str,
    ac_session_id: Option<&str>,
    descriptor: Option<(&str, &BTreeMap<String, String>)>,
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    let entry = CodingAgentEntry {
        app: app_label.to_string(),
        ac_session_id: ac_session_id.map(|s| s.to_string()),
        last_used: now,
        command: descriptor
            .map(|(command, _)| command.to_string())
            .unwrap_or_default(),
        identity: descriptor
            .map(|(_, identity)| identity.clone())
            .unwrap_or_default(),
    };
    let write_descriptor = descriptor.is_some();

    // Write to per-instance config dir
    let local_dir_name = crate::config::agent_local_dir_name();
    let instance_dir = Path::new(repo_path).join(local_dir_name.as_str());
    std::fs::create_dir_all(&instance_dir)
        .map_err(|e| format!("Failed to create {} dir: {}", local_dir_name, e))?;
    upsert_config(
        &instance_dir.join("config.json"),
        agent_id,
        &entry,
        write_descriptor,
    )?;

    // Also write to root config.json so discovery can find it regardless of instance
    upsert_config(
        &Path::new(repo_path).join("config.json"),
        agent_id,
        &entry,
        write_descriptor,
    )?;

    log::info!(
        "Updated lastCodingAgent to '{}' ({}) in {} + root config.json",
        agent_id,
        app_label,
        local_dir_name
    );
    Ok(())
}

/// #1682 - record `at_rfc3339`, the instant a busy->idle edge closed an armed
/// turn for the coding agent in `repo_path`. The caller judges that, and R7 and
/// R8 mean an armed turn is not proof the agent responded. Writes ONLY the
/// per-instance config; the root `config.json` is deliberately not touched (see
/// D2). Monotonic: a stored value that is already at or after `at_rfc3339` is
/// kept. Returns whether the file now carries `at_rfc3339`.
pub fn set_last_agent_message_at(repo_path: &str, at_rfc3339: &str) -> Result<bool, String> {
    let local_dir_name = crate::config::agent_local_dir_name();
    let instance_dir = Path::new(repo_path).join(local_dir_name.as_str());
    std::fs::create_dir_all(&instance_dir)
        .map_err(|e| format!("Failed to create {} dir: {}", local_dir_name, e))?;
    let path = instance_dir.join("config.json");

    let inserted = std::cell::Cell::new(false);
    update_agent_config(&path, |_decisions, state| {
        let tooling = ensure_object(state, "tooling", &path);
        let stored_is_not_older = tooling
            .get("lastAgentMessageAt")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .zip(chrono::DateTime::parse_from_rfc3339(at_rfc3339).ok())
            .is_some_and(|(stored, new)| stored >= new);
        if stored_is_not_older {
            return Ok(());
        }
        tooling.insert(
            "lastAgentMessageAt".to_string(),
            serde_json::json!(at_rfc3339),
        );
        inserted.set(true);
        Ok(())
    })?;

    log::debug!(
        "lastAgentMessageAt for {}: {} (inserted: {})",
        repo_path,
        at_rfc3339,
        inserted.get()
    );
    Ok(inserted.get())
}

/// #1682 - the stored stamp for `repo_path`, or `None` when the file is absent,
/// unparseable, or carries no `tooling.lastAgentMessageAt`. Never validates the
/// string: rendering owns that.
pub fn read_last_agent_message_at(repo_path: &str) -> Option<String> {
    let dir = Path::new(repo_path).join(crate::config::agent_local_dir_name().as_str());
    read_agent_local_config(&dir).and_then(|cfg| cfg.tooling.last_agent_message_at)
}

// ── State file loader (#2786, C1) ───────────────────────────────────────────
/// #2786 (C1) - the D7 split marker's top-level key in the state file.
pub(crate) const SPLIT_MARKER_KEY: &str = "split";
/// #2786 (C1) - a marker whose integer `v` is at least this reads as present.
pub(crate) const SPLIT_MARKER_VERSION: u64 = 1;

/// #2786 (C1) - the `tooling` keys whose home is the state file.
const STATE_KEYS: [&str; 4] = [
    "lastCodingAgent",
    "codingAgents",
    "lastAgentMessageAt",
    "profileContentHash",
];

/// #2786 C1 r28 candidate instrumentation, option C. Test-only. The recorder
/// is ARMED by a process-start environment flag, read once; armed off it is
/// inert (no lock, no record, no panic), so the ordinary suite is untouched.
/// Observation rows run in a dedicated process that sets the flag through
/// `Command::env`, never by mutating this process's environment.
#[cfg(test)]
pub(crate) mod load_probe {
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    pub(crate) const ARM_VAR: &str = "AC2786_LOAD_PROBE";

    static OBSERVER: Mutex<()> = Mutex::new(());
    static ENTRIES: Mutex<Option<Vec<PathBuf>>> = Mutex::new(None);

    pub(crate) fn armed() -> bool {
        static ARMED: OnceLock<bool> = OnceLock::new();
        *ARMED.get_or_init(|| std::env::var(ARM_VAR).as_deref() == Ok("1"))
    }

    pub(crate) struct Observation {
        _slot: MutexGuard<'static, ()>,
    }

    impl Observation {
        pub(crate) fn open() -> Observation {
            assert!(
                armed(),
                "#2786 C1: Observation::open in an unarmed process; run this test \
                 through the dedicated-process harness"
            );
            let slot = OBSERVER.lock().unwrap_or_else(|e| e.into_inner());
            *ENTRIES.lock().unwrap_or_else(|e| e.into_inner()) = Some(Vec::new());
            Observation { _slot: slot }
        }
        pub(crate) fn loads(&self) -> Vec<PathBuf> {
            ENTRIES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .unwrap_or_default()
        }
    }

    // #2786 C1 r29 suppression inventory. The candidate carries exactly ONE
    // `allow(dead_code)`, on `NoObservation::open` below. r28 carried two and
    // described them with a single shared sentence; that sentence was false for
    // the field one, which a reviewer measured separately.
    //
    // Measured, one allowance at a time, with `cargo clippy -p agentscommander
    // --all-targets -- -D warnings`:
    //
    //  - the FIELD allowance r28 had on the tuple field is GONE, not suppressed:
    //    the guard is now the named field `_slot`, exactly like `Observation`,
    //    and `dead_code` does not ask for an underscore-prefixed field to be
    //    read. With the r28 tuple form and that allowance removed, clippy
    //    reported `field 0 is never read` (the reviewer's measurement, and mine).
    //    Impact of the representation change: none at run time. The field is
    //    held for its RAII effect only, it is never read in either form, and a
    //    named field is dropped at exactly the same point as a tuple field.
    //
    //  - the FUNCTION allowance on `NoObservation::open` is removed too, in the
    //    commit that adds the fail-closed row
    //    (`a_load_with_no_open_observation_fails_closed`, E15's fourth
    //    selector), which constructs a `NoObservation`. That was its stated
    //    removal condition, and clippy is clean without it.
    //
    // The load-probe prototype is the cross-check: it DOES carry that row, and
    // with `_slot` it needs zero `allow(dead_code)` attributes and clippy is
    // clean, which is the same two conditions stated above, satisfied.
    pub(crate) struct NoObservation {
        _slot: MutexGuard<'static, ()>,
    }

    impl NoObservation {
        pub(crate) fn open() -> NoObservation {
            assert!(
                armed(),
                "#2786 C1: NoObservation::open in an unarmed process"
            );
            let slot = OBSERVER.lock().unwrap_or_else(|e| e.into_inner());
            *ENTRIES.lock().unwrap_or_else(|e| e.into_inner()) = None;
            NoObservation { _slot: slot }
        }
    }

    impl Drop for Observation {
        fn drop(&mut self) {
            *ENTRIES.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }

    pub(crate) fn record_load(dir: &Path) {
        if !armed() {
            return;
        }
        let mut g = ENTRIES.lock().unwrap_or_else(|e| e.into_inner());
        match g.as_mut() {
            Some(v) => v.push(dir.to_path_buf()),
            None => panic!("#2786 C1: wrapper load with no open Observation: {dir:?}"),
        }
    }
}

/// #2786 C1 r28 candidate instrumentation, option C: the dedicated-process
/// harness the E15 observation rows run through. Test-only.
///
/// r28 change vs r27: the child is owned by a `ChildGuard` from spawn to reap.
/// `try_wait` errors are handled instead of unwinding out of the row, and a
/// termination or reap that FAILS is reported as the error it returned; the
/// harness never claims a cleanup it did not achieve.
#[cfg(test)]
pub(crate) mod load_probe_harness {
    use std::io::Read;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const CHILD_TIMEOUT: Duration = Duration::from_secs(60);
    /// r29 item 5: the ceiling on the reap poll taken ONLY after a kill that
    /// errored, so a failed termination cannot block the row forever.
    const REAP_BUDGET: Duration = Duration::from_secs(5);
    static N: AtomicU64 = AtomicU64::new(0);

    /// The `#[ignore]`d inner row the harness ownership rows drive: it blocks
    /// until the harness kills it, so those rows always act on a LIVE child.
    pub(crate) const SLEEPER: &str = "config::agent_config::tests::inner_sleeps_until_killed";

    /// How far cleanup actually got for one child. Deliberately not a boolean
    /// and not an `Option`: the three outcomes are distinguishable, and
    /// `Reaped` is produced ONLY where a `wait`/`try_wait` call returned an
    /// exit status, so the harness can never claim a child it did not observe
    /// exit.
    #[derive(Debug)]
    pub(crate) enum ReapOutcome {
        /// `wait()`, or the bounded `try_wait()` loop, returned this status.
        Reaped(String),
        /// The bounded wait after a FAILED kill expired with the child never
        /// observed to exit: cleanup is PENDING and the process may be alive.
        Pending(String),
        /// `wait()`/`try_wait()` itself errored. Nothing was observed.
        Failed(String),
    }

    impl ReapOutcome {
        /// One word for the state. Used in every diagnostic.
        pub(crate) fn label(&self) -> &'static str {
            match self {
                ReapOutcome::Reaped(_) => "reaped",
                ReapOutcome::Pending(_) => "CLEANUP-PENDING",
                ReapOutcome::Failed(_) => "REAP-FAILED",
            }
        }
        /// The payload, READ here rather than suppressed: it is the exit status
        /// or the error text a human needs, so `dead_code` has no complaint and
        /// no `allow` is added for it.
        pub(crate) fn detail(&self) -> &str {
            match self {
                ReapOutcome::Reaped(s) | ReapOutcome::Pending(s) | ReapOutcome::Failed(s) => s,
            }
        }
    }

    /// What the ownership guard achieved for one child, and what it did not.
    #[derive(Debug)]
    pub(crate) struct Cleanup {
        pub(crate) pid: u32,
        pub(crate) outcome: ReapOutcome,
        pub(crate) kill_error: Option<String>,
        /// r31 F4/F5: the STILL-OWNED `Child`, on the two paths where the
        /// guard could not reap it. r30 dropped the handle here, which on a
        /// Unix host leaves the process as this process's un-reaped zombie
        /// and leaves nobody able to `wait()` it. The handle is now HANDED
        /// OVER instead of dropped: either the caller adopts it (the two
        /// direct controls do, before their first fallible assertion) or
        /// `ChildGuard::drop` kills and reaps it. `None` on the reaped path
        /// and after whoever took it has done so.
        pub(crate) unreaped: Option<Child>,
    }

    /// r31 F4/F5: kill a still-owned child and REALLY reap it, bounded.
    ///
    /// `kill()` alone is not cleanup: on a Unix host the child stays in the
    /// process table as a zombie until its parent waits, and `ps` reports it.
    /// `wait()` on the owned handle is the portable reap, needs no external
    /// command and no new dependency, and is what makes the pid disappear on
    /// both platforms. The poll is bounded so no path can wait for ever.
    pub(crate) fn bounded_kill_and_reap(child: &mut Child, pid: u32, budget: Duration) -> String {
        let killed = match child.kill() {
            Ok(()) => "kill ok".to_string(),
            Err(e) => format!("kill error: {e}"),
        };
        let deadline = Instant::now() + budget;
        loop {
            match child.try_wait() {
                Ok(Some(st)) => return format!("pid {pid}: {killed}; REAPED status {st}"),
                Ok(None) => {
                    if Instant::now() >= deadline {
                        return format!("pid {pid}: {killed}; NOT reaped within {budget:?}");
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return format!("pid {pid}: {killed}; reap error: {e}"),
            }
        }
    }

    impl Cleanup {
        /// True ONLY for an observed exit status. `Pending` and `Failed` are
        /// both false, so a row cannot pass on an unproven cleanup.
        pub(crate) fn reaped_ok(&self) -> bool {
            matches!(self.outcome, ReapOutcome::Reaped(_))
        }
        pub(crate) fn describe(&self) -> String {
            format!(
                "pid {} outcome={} ({}) kill_error={:?}",
                self.pid,
                self.outcome.label(),
                self.outcome.detail(),
                self.kill_error
            )
        }
    }

    /// Cleanups performed by `Drop`, i.e. on an unwind path, where the caller
    /// is gone and cannot be handed the report.
    static UNWIND_CLEANUPS: Mutex<Vec<Cleanup>> = Mutex::new(Vec::new());

    pub(crate) fn take_unwind_cleanups() -> Vec<Cleanup> {
        std::mem::take(&mut *UNWIND_CLEANUPS.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Owns the child from spawn to reap. While `child` is `Some`, terminating
    /// and reaping it is this guard's duty on EVERY exit path, return or
    /// unwind. Ownership is released only by `reaped_by_caller`, which the
    /// caller may call only after `try_wait` handed it an exit status.
    struct ChildGuard {
        pid: u32,
        child: Option<Child>,
        /// The injected fault of the row that created this guard, so the two
        /// r30 failure branches can be reached deterministically. `Fault::None`
        /// for every observation row, which is the whole ordinary suite.
        fault: Fault,
    }

    impl ChildGuard {
        fn new(child: Child, fault: Fault) -> ChildGuard {
            let pid = child.id();
            ChildGuard {
                pid,
                child: Some(child),
                fault,
            }
        }
        fn owned(&mut self) -> &mut Child {
            self.child
                .as_mut()
                .expect("#2786 C1: the guard no longer owns the child")
        }
        fn reaped_by_caller(&mut self) {
            self.child = None;
        }
        /// r29: bounded on the failure path. After a kill that SUCCEEDED,
        /// `wait()` is the portable reap and returns promptly. After a kill
        /// that ERRORED the child may still be running, and `wait()` would then
        /// block for as long as it lives, which on the 120 s sleeper hangs the
        /// row and the suite with no diagnostic. That path polls `try_wait`
        /// for at most `REAP_BUDGET` and, if the child is never observed to
        /// exit, reports `Pending` with the pid, so the failure is visible and
        /// owned instead of silent.
        fn terminate_and_reap(&mut self) -> Option<Cleanup> {
            // NOTE, and this is the incompleteness the plan states: `take`
            // moves the `Child` out of the guard. From here on the guard owns
            // nothing, and when `child` is dropped at the end of this function
            // the handle is gone. On the `Pending` and `Failed` paths the
            // process may still be running with nobody holding a handle to it.
            let mut child = self.child.take()?;
            let injected_kill =
                matches!(self.fault, Fault::KillFails | Fault::KillFailsAndReapErrors);
            let kill_error = if injected_kill {
                Some("#2786 C1: injected kill failure; the child was NOT terminated".to_string())
            } else {
                child.kill().err().map(|e| e.to_string())
            };
            let mut inject_reap_error = self.fault == Fault::KillFailsAndReapErrors;
            let outcome = if kill_error.is_none() {
                match child.wait() {
                    Ok(s) => ReapOutcome::Reaped(format!("{s}")),
                    Err(e) => ReapOutcome::Failed(e.to_string()),
                }
            } else {
                let deadline = Instant::now() + REAP_BUDGET;
                loop {
                    let probed = if inject_reap_error {
                        inject_reap_error = false;
                        Err(std::io::Error::other(
                            "#2786 C1: injected try_wait failure inside the reap poll",
                        ))
                    } else {
                        child.try_wait()
                    };
                    match probed {
                        Ok(Some(s)) => break ReapOutcome::Reaped(format!("{s}")),
                        Ok(None) => {
                            if Instant::now() >= deadline {
                                break ReapOutcome::Pending(format!(
                                    "terminating pid {} failed and it was not observed to exit \
                                     within {REAP_BUDGET:?}: cleanup is PENDING and the process \
                                     may still be running",
                                    self.pid
                                ));
                            }
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Err(e) => break ReapOutcome::Failed(e.to_string()),
                    }
                }
            };
            // r31 F4/F5: the handle is dropped ONLY when the child was really
            // reaped. On `Pending` and `Failed` it travels with the report, so
            // no code path drops a handle to a process that is still running.
            let unreaped = match outcome {
                ReapOutcome::Reaped(_) => None,
                ReapOutcome::Pending(_) | ReapOutcome::Failed(_) => Some(child),
            };
            Some(Cleanup {
                pid: self.pid,
                outcome,
                kill_error,
                unreaped,
            })
        }
    }

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if let Some(mut c) = self.terminate_and_reap() {
                // r31 F4: the caller is gone, so nobody can adopt the handle.
                // Kill and REAP it here, bounded, on this unwind or return
                // path, before the report is stored.
                if let Some(mut child) = c.unreaped.take() {
                    let r = bounded_kill_and_reap(&mut child, c.pid, REAP_BUDGET);
                    eprintln!("#2786 C1 r31 F4: DROP-CLEANUP {r}");
                }
                if !c.reaped_ok() {
                    // The caller is gone; this is the only place the failure
                    // can still be seen by a human reading the child log.
                    eprintln!(
                        "#2786 C1: CLEANUP NOT ACHIEVED on the unwind path: {}",
                        c.describe()
                    );
                }
                UNWIND_CLEANUPS
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(c);
            }
        }
    }

    /// The child log, removed on every exit path so an unwind does not leave a
    /// stray file in the temp directory.
    struct LogFile(std::path::PathBuf);

    impl Drop for LogFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Deterministic faults for the harness ownership rows. Never used by an
    /// observation row: `run_inner` passes `Fault::None`.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub(crate) enum Fault {
        None,
        /// The first `try_wait` is replaced by an injected I/O error.
        WaitError,
        /// The deadline is already past when the supervision loop starts.
        Timeout,
        /// Unwind while the guard still owns a live child.
        PanicWhileOwned,
        /// r30 item 5, DIRECT control: `Child::kill` is replaced by an
        /// injected error and NOT performed, so the child stays ALIVE and the
        /// bounded `try_wait` poll runs to its ceiling and reports `Pending`.
        /// This is the only way to reach that branch: a real `kill` on a live
        /// child of this process does not fail on either platform.
        KillFails,
        /// r30 item 5, DIRECT control: the kill is injected as above AND the
        /// first `try_wait` inside `terminate_and_reap` is replaced by an
        /// injected I/O error, so the branch reports `Failed` at once.
        KillFailsAndReapErrors,
    }

    /// r30 item 5: what a pid query concluded. Three outcomes, never two: an
    /// absent process and a query that could not answer are different facts,
    /// and conflating them once made a liveness check read `dead` for both.
    #[derive(Debug, PartialEq, Eq)]
    pub(crate) enum PidState {
        Alive,
        Dead,
        QueryError(String),
    }

    /// Query one pid by an explicit filter string. `pid_state` renders the
    /// filter; a test may pass a malformed one to exercise `QueryError`.
    ///
    /// It shells out to the platform's process lister through `Command`, whose
    /// output this process reads directly. It does NOT use a shell, command
    /// substitution, a job object, or the teardown of any wrapper process, so
    /// what it reports is the state of the pid at the moment of the call and
    /// not an artefact of some parent exiting.
    #[cfg(windows)]
    pub(crate) fn pid_state_by_filter(filter: &str) -> PidState {
        let out = match Command::new("tasklist")
            .args(["/FI", filter, "/NH", "/FO", "CSV"])
            .output()
        {
            Ok(o) => o,
            Err(e) => return PidState::QueryError(format!("spawning tasklist failed: {e}")),
        };
        let text = String::from_utf8_lossy(&out.stdout).to_string()
            + &String::from_utf8_lossy(&out.stderr);
        if !out.status.success() {
            return PidState::QueryError(format!(
                "tasklist exit {:?}: {}",
                out.status.code(),
                text.trim()
            ));
        }
        // An INFO line is the documented way tasklist says "no match". It is
        // NOT an error, and it is NOT a live process.
        if text.contains("INFO:") || text.contains("No tasks") {
            return PidState::Dead;
        }
        if text.trim_start().starts_with('"') {
            return PidState::Alive;
        }
        PidState::QueryError(format!("tasklist output not understood: {}", text.trim()))
    }

    /// r31 F5. `ps -p` cannot tell these two apart by exit code and stdout:
    /// a legitimately absent pid and a malformed selector BOTH exit 1 with
    /// empty stdout (measured on Debian, `evidence/f5-unix-ps-probe.txt`).
    /// stderr does not separate them reliably either: under WSL every `ps`
    /// call writes a screen-size warning, so "stderr is non-empty" reads the
    /// absent-pid case as an error too, and the only real discriminator,
    /// `ps`'s own `error:` line, is locale text.
    ///
    /// So the selector is validated HERE, before `ps` is ever spawned:
    /// `pid_state` renders a decimal pid, and any other selector is a caller
    /// mistake, reported as `QueryError` without a subprocess. A numeric
    /// selector then keeps the exit-code mapping, and anything `ps` answers
    /// that does not fit it is still `QueryError`. No new dependency.
    #[cfg(not(windows))]
    pub(crate) fn pid_state_by_filter(filter: &str) -> PidState {
        if filter.is_empty() || !filter.bytes().all(|b| b.is_ascii_digit()) {
            return PidState::QueryError(format!(
                "not a pid selector for ps: {filter:?};                  ps answers a malformed selector exactly as it answers an absent pid"
            ));
        }
        let out = match Command::new("ps")
            .args(["-p", filter, "-o", "pid="])
            .output()
        {
            Ok(o) => o,
            Err(e) => return PidState::QueryError(format!("spawning ps failed: {e}")),
        };
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        match (out.status.code(), text.is_empty()) {
            (Some(0), false) => PidState::Alive,
            (Some(1), true) => PidState::Dead,
            (c, _) => PidState::QueryError(format!("ps exit {c:?}: {text}")),
        }
    }

    /// r31 F5: a selector that is malformed ON THIS PLATFORM, so the
    /// query-error leg is exercised by the same control everywhere. r30 used
    /// the Windows filter string on both, and on a Unix host `ps` answers it
    /// exactly as it answers an absent pid, which made that leg RED there.
    #[cfg(windows)]
    pub(crate) const BAD_PID_SELECTOR: &str = "PID eq not-a-pid";
    #[cfg(not(windows))]
    pub(crate) const BAD_PID_SELECTOR: &str = "not-a-pid";

    pub(crate) fn pid_state(pid: u32) -> PidState {
        #[cfg(windows)]
        let f = format!("PID eq {pid}");
        #[cfg(not(windows))]
        let f = format!("{pid}");
        pid_state_by_filter(&f)
    }

    // r31 F5: r30's `external_kill_by_pid` is REMOVED, not kept unused.
    //
    // It shelled out to `taskkill`/`kill` on a bare pid, which is the very
    // thing the dev's F5 shows is not cleanup: on a Unix host a signal to
    // this process's own child without a `wait()` leaves a zombie that `ps`
    // still reports. The two paths that used it now hand the still-owned
    // `Child` to the caller and reap it through `bounded_kill_and_reap`,
    // which is portable and needs no external command.

    pub(crate) struct ChildRun {
        pub(crate) ok: bool,
        pub(crate) out: String,
        pub(crate) timed_out: bool,
        /// `Some` when supervision itself failed, e.g. an errored `try_wait`.
        pub(crate) wait_error: Option<String>,
        /// `Some` whenever the guard had to terminate the child, carrying what
        /// it achieved. Absent on the normal path, where the child exited by
        /// itself and `try_wait` reaped it.
        pub(crate) cleanup: Option<Cleanup>,
    }

    /// Re-execute THIS test binary for one `#[ignore]`d inner row, with the
    /// probe armed through `Command::env` only.
    pub(crate) fn run_inner(name: &str) -> ChildRun {
        run_inner_with_fault(name, Fault::None)
    }

    pub(crate) fn run_inner_with_fault(name: &str, fault: Fault) -> ChildRun {
        let exe = std::env::current_exe().expect("test binary path");
        let log = LogFile(std::env::temp_dir().join(format!(
            "ac2786-child-{}-{}.log",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        )));
        let sink = std::fs::File::create(&log.0).expect("child log");
        let sink2 = sink.try_clone().expect("child log clone");
        let child = Command::new(exe)
            .args([
                name,
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(super::load_probe::ARM_VAR, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::from(sink))
            .stderr(Stdio::from(sink2))
            .spawn()
            .expect("spawn the dedicated observation process");
        let mut guard = ChildGuard::new(child, fault);

        // The two r30 failure controls need `terminate_and_reap` to run on a
        // LIVE child, which is exactly what the past deadline produces.
        let deadline = if matches!(
            fault,
            Fault::Timeout | Fault::KillFails | Fault::KillFailsAndReapErrors
        ) {
            Instant::now()
        } else {
            Instant::now() + CHILD_TIMEOUT
        };
        let mut status = None;
        let mut timed_out = false;
        let mut wait_error = None;
        let mut cleanup = None;
        let mut inject_wait_error = fault == Fault::WaitError;
        loop {
            if fault == Fault::PanicWhileOwned {
                panic!(
                    "#2786 C1: injected unwind while the guard owns child pid {}",
                    guard.pid
                );
            }
            let probed = if inject_wait_error {
                inject_wait_error = false;
                Err(std::io::Error::other("#2786 C1: injected try_wait failure"))
            } else {
                guard.owned().try_wait()
            };
            match probed {
                Ok(Some(s)) => {
                    // `try_wait` reaped it; nothing is left for the guard.
                    status = Some(s);
                    guard.reaped_by_caller();
                    break;
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        timed_out = true;
                        cleanup = guard.terminate_and_reap();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    wait_error = Some(e.to_string());
                    cleanup = guard.terminate_and_reap();
                    break;
                }
            }
        }

        let mut out = String::new();
        if let Ok(mut f) = std::fs::File::open(&log.0) {
            let _ = f.read_to_string(&mut out);
        }
        ChildRun {
            ok: status.map(|s| s.success()).unwrap_or(false),
            out,
            timed_out,
            wait_error,
            cleanup,
        }
    }

    /// Shared assertions: supervision itself succeeded, any termination the
    /// guard had to perform really reaped the child, the child really ran ONE
    /// test, so a filter that matched nothing cannot pass as a green row, and
    /// it really reached the caller.
    pub(crate) fn assert_one_test_ran(r: &ChildRun, name: &str) {
        assert!(
            r.wait_error.is_none(),
            "supervising the child for {name} failed: {:?}",
            r.wait_error
        );
        if let Some(c) = &r.cleanup {
            assert!(
                c.reaped_ok(),
                "the child for {name} was not reaped: {}",
                c.describe()
            );
        }
        assert!(!r.timed_out, "child for {name} timed out: {}", r.out);
        assert!(
            r.out.contains("running 1 test"),
            "child for {name} did not run exactly one test: {}",
            r.out
        );
        assert!(
            r.out.contains("test result:"),
            "child for {name} printed no result line: {}",
            r.out
        );
        assert!(
            r.out.contains("REACHED-CALLER"),
            "child for {name} never reached the real caller: {}",
            r.out
        );
    }

    pub(crate) fn expect_child_pass(name: &str) -> String {
        let r = run_inner(name);
        assert_one_test_ran(&r, name);
        assert!(r.ok, "child for {name} failed: {}", r.out);
        r.out
    }

    /// The foreign id both launch-path fixtures store; no local agent has it,
    /// so it can only resolve through its descriptor.
    pub(crate) const SNAPSHOT_ID: &str = "foreign-claude";

    /// #2786 C1 E14/E15: the one fixture both callers' rows build. `config.json`
    /// holds the id under `id_key` plus a descriptor with `command: "old"`; the
    /// state file holds the descriptor for that id with `state_command` and no
    /// marker. Rewriting it on the same directory changes only the state side.
    /// Written through `write_file_atomic`, the shared writer the #1938 scan
    /// requires outside a `mod tests` block.
    pub(crate) fn write_snapshot_fixture(dir: &std::path::Path, id_key: &str, state_command: &str) {
        let tracked = serde_json::json!({"tooling": {
            id_key: SNAPSHOT_ID,
            "codingAgents": {SNAPSHOT_ID: {"command": "old"}}}});
        let state = serde_json::json!({"tooling": {
            "codingAgents": {SNAPSHOT_ID: {"command": state_command}}}});
        for (name, value) in [
            ("config.json", tracked),
            (
                crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME,
                state,
            ),
        ] {
            crate::config::local_config_io::write_file_atomic(
                &dir.join(name),
                value.to_string().as_bytes(),
            )
            .unwrap();
        }
    }

    /// One local agent per descriptor command the fixtures use, named
    /// `via-<command>`, so the resolved agent id names the descriptor read.
    pub(crate) fn snapshot_settings() -> crate::config::settings::AppSettings {
        let agent = |command: &str| crate::config::settings::AgentConfig {
            id: format!("via-{command}"),
            label: format!("label-{command}"),
            command: command.to_string(),
            color: "#000000".to_string(),
            order: None,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        };
        crate::config::settings::AppSettings {
            agents: ["old", "new", "new-a", "new-b"].map(agent).to_vec(),
            ..Default::default()
        }
    }
}

/// #2786 (C1) - the one reader of an agent's local config: `config.json` in
/// `dir`, with the state keys served key-wise from the state file beside it.
/// `None` when neither file yields a config. Each file must also pass the
/// typed parse on its own text, as the direct typed readers did before C1, so
/// a duplicate known field is still a rejection and never collapses to the
/// last value through `Value`. A state file that fails it is ignored.
pub fn read_agent_local_config(dir: &Path) -> Option<AgentLocalConfig> {
    #[cfg(test)]
    load_probe::record_load(dir);
    let decisions = read_config_text(&dir.join("config.json"));
    if decisions.as_ref().is_some_and(|text| !typed_ok(text)) {
        return None;
    }
    let state = read_config_text(&state_file_path(dir)).filter(|text| typed_ok(text));
    let value = merge_config_texts(decisions, state).ok()??;
    serde_json::from_value(value).ok()
}

/// #2786 (C1) - the raw JSON the loader serves: `config.json` in `dir` with
/// the state file's keys overlaid. `Ok(None)` when neither file exists; `Err`
/// when `config.json` exists but is not JSON. A pure read: it writes nothing.
/// The state file must pass the typed parse, exactly as in the typed wrapper,
/// so both wrappers admit the same state files; `config.json` keeps its base
/// `Value` semantics and is not filtered.
pub fn read_agent_local_config_json(dir: &Path) -> Result<Option<serde_json::Value>, String> {
    #[cfg(test)]
    load_probe::record_load(dir);
    merge_config_texts(
        read_config_text(&dir.join("config.json")),
        read_config_text(&state_file_path(dir)).filter(|text| typed_ok(text)),
    )
}

/// #2786 (C1) - overlay an already parsed state file on an already parsed
/// `config.json`, for a caller that reads both through its own guarded path.
/// A state value that is not an object is ignored.
pub fn overlay_agent_local_state(
    decisions: serde_json::Value,
    state: Option<serde_json::Value>,
) -> serde_json::Value {
    let state = match state {
        Some(serde_json::Value::Object(map)) => Some(map),
        _ => None,
    };
    overlay_state(decisions, state)
}

fn state_file_path(dir: &Path) -> std::path::PathBuf {
    dir.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
}

fn read_config_text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Merge the two texts. `config.json` that is not JSON is an error; a state
/// file that is absent, not JSON or not an object is ignored, so a reader
/// falls back to `config.json` rather than failing.
/// #2786 (C1) - the typed parse both wrappers apply to a file's own text.
fn typed_ok(text: &str) -> bool {
    serde_json::from_str::<AgentLocalConfig>(text).is_ok()
}

fn merge_config_texts(
    decisions: Option<String>,
    state: Option<String>,
) -> Result<Option<serde_json::Value>, String> {
    let decisions = decisions
        .map(|text| serde_json::from_str::<serde_json::Value>(&text).map_err(|e| e.to_string()))
        .transpose()?;
    let state = state
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| match value {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        });
    if decisions.is_none() && state.is_none() {
        return Ok(None);
    }
    let decisions = decisions.unwrap_or_else(|| serde_json::json!({}));
    Ok(Some(overlay_state(decisions, state)))
}

/// D7, "present" is exact: an object whose `v` is an integer at or above
/// [`SPLIT_MARKER_VERSION`]. A newer version reads as present on purpose.
fn split_marker_present(marker: Option<&serde_json::Value>) -> bool {
    marker
        .and_then(|marker| marker.get("v"))
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|v| v >= SPLIT_MARKER_VERSION)
}

/// Key-wise overlay. Marker absent: the state file wins for every key it
/// holds. Marker present: a key the tracked file holds keeps the tracked value.
/// `split.keys` is not consulted, and the marker is never returned.
fn overlay_state(
    mut decisions: serde_json::Value,
    state: Option<serde_json::Map<String, serde_json::Value>>,
) -> serde_json::Value {
    let Some(state) = state else {
        return decisions;
    };
    let Some(state_tooling) = state.get("tooling").and_then(|t| t.as_object()) else {
        return decisions;
    };
    let tracked_wins = split_marker_present(state.get(SPLIT_MARKER_KEY));
    let Some(root) = decisions.as_object_mut() else {
        return decisions;
    };
    let Some(tooling) = root
        .entry("tooling")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
    else {
        return decisions;
    };
    for key in STATE_KEYS {
        let Some(value) = state_tooling.get(key) else {
            continue;
        };
        if tracked_wins && tooling.contains_key(key) {
            continue;
        }
        tooling.insert(key.to_string(), value.clone());
    }
    decisions
}

// ── Config pair entry point (#2807, C2) ─────────────────────────────────────
type JsonMap = serde_json::Map<String, serde_json::Value>;

/// #2807 (C2) - the one writer of an agent's config pair: `decisions` is the
/// tracked `config.json`, and the state file is the one beside it. `mutate`
/// gets `(decisions, state)`; a decision-only writer ignores the second map.
/// Any state key still in the tracked file is moved to the state file first,
/// inside the same guard.
pub fn update_agent_config<F>(decisions: &Path, mutate: F) -> Result<(), String>
where
    F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
{
    update_agent_config_in(decisions, production_journal_dir().as_deref(), mutate)
}

/// The exact refusal of the pre-C3 `update_config_json_object(path, false, ..)`
/// callers: an absent decisions file is an error and nothing is created. As in
/// the old helper the decision is taken UNDER the file locks, so a concurrent
/// creator mid-publish is waited for, not refused.
pub fn update_existing_agent_config<F>(decisions: &Path, mutate: F) -> Result<(), String>
where
    F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
{
    update_agent_config(decisions, |d, s| {
        if !decisions.exists() {
            return Err(format!(
                "Local config {} does not exist",
                decisions.display()
            ));
        }
        mutate(d, s)
    })
}

/// The journal directory of a production call. Under `cfg(test)` the live
/// instance journal is unreachable: the default is `None`, and a test that
/// wants a note opts in to its own temporary directory. One body, one call
/// site, no second production code path.
fn production_journal_dir() -> Option<std::path::PathBuf> {
    #[cfg(test)]
    {
        journal_redirect::current()
    }
    #[cfg(not(test))]
    {
        crate::config::config_dir()
    }
}

/// #2816 (C3) - the only route from a lib test to a journal: a thread-local
/// directory, so parallel tests never see each other's.
#[cfg(test)]
pub(crate) mod journal_redirect {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    thread_local! {
        static DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    pub(crate) fn current() -> Option<PathBuf> {
        DIR.with(|dir| dir.borrow().clone())
    }

    /// Restores the previous directory on drop.
    #[must_use]
    pub(crate) struct Guard {
        previous: Option<PathBuf>,
    }

    pub(crate) fn set(dir: &Path) -> Guard {
        let previous = DIR.with(|cell| cell.replace(Some(dir.to_path_buf())));
        Guard { previous }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            DIR.with(|cell| *cell.borrow_mut() = self.previous.take());
        }
    }
}

fn update_agent_config_in<F>(
    decisions: &Path,
    journal_dir: Option<&Path>,
    mutate: F,
) -> Result<(), String>
where
    F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
{
    update_agent_config_with_stage(decisions, journal_dir, &|_: &str| {}, mutate)
}

/// The only body. `on_stage` is a no-op in production and a pause point in
/// tests, so no second code path exists.
fn update_agent_config_with_stage<F>(
    decisions: &Path,
    journal_dir: Option<&Path>,
    on_stage: &dyn Fn(&str),
    mutate: F,
) -> Result<(), String>
where
    F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
{
    // The state file must be ignored before anything can write one. A file
    // outside an `.ac` tree has no `.gitignore` of ours.
    if let Some(ac_root) = nearest_ac_root(decisions) {
        ensure_config_state_ignore_rows(ac_root)?;
    }

    let parent = decisions.parent().ok_or_else(|| {
        format!(
            "Local config {} has no parent directory",
            decisions.display()
        )
    })?;
    let state = state_file_path(parent);
    let moved = std::cell::RefCell::new(Vec::new());
    let overridden = std::cell::RefCell::new(Vec::new());
    let cleanup = |d: &mut JsonMap, s: &mut JsonMap| absorb_state_keys(d, s, &moved, &overridden);
    // The marker is stamped here and nowhere else: the cleanup has published
    // the clean tracked file before `mutate` runs.
    let wrapped = |d: &mut JsonMap, s: &mut JsonMap| {
        mutate(d, s)?;
        stamp_split_marker(s);
        Ok(())
    };
    crate::config::local_config_io::update_config_pair(
        decisions,
        &state,
        &STATE_KEYS,
        Some(&cleanup),
        on_stage,
        wrapped,
    )?;

    for (key, state_value, tracked_value) in overridden.into_inner() {
        log::warn!(
            "[config-state] '{}' in {} was {} and is now {}, the value found in {}",
            key,
            state.display(),
            state_value,
            tracked_value,
            decisions.display()
        );
    }
    let moved = moved.into_inner();
    if !moved.is_empty() {
        note_moved_state_keys(decisions, journal_dir, &moved);
    }
    Ok(())
}

fn nearest_ac_root(decisions: &Path) -> Option<&Path> {
    decisions
        .ancestors()
        .skip(1)
        .find(|dir| dir.file_name().is_some_and(|name| name == ".ac"))
}

/// #2807 (C2) 4.1 - appends to `<ac_root>/.gitignore` the agent state rows it
/// lacks. An appending write, never a rewrite, so no line another writer added
/// can be lost.
fn ensure_config_state_ignore_rows(ac_root: &Path) -> Result<(), String> {
    use std::io::Write as _;
    let path = ac_root.join(".gitignore");
    let io = |what: &str, e: std::io::Error| format!("failed to {what} {}: {e}", path.display());
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(io("read", e)),
    };
    let blocks = crate::config::naming_migration::missing_ignore_rows(
        &content,
        &crate::config::naming_migration::config_state_ignore_rows(),
    );
    if blocks.is_empty() {
        return Ok(());
    }
    let separator = if content.is_empty() || content.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(format!("{separator}{blocks}").as_bytes()))
        .map_err(|e| io("append to", e))
}

/// #2807 (C2) 3.3 - the cleanup of the pair, and the write-side twin of
/// `overlay_state`: every state key leaves the tracked map. Marker absent, the
/// state value wins where both hold the key; marker present, the tracked value
/// wins. It takes no lock, opens no file and names no path; `moved` and
/// `overridden` carry what it did back to the entry point.
fn absorb_state_keys(
    decisions: &mut JsonMap,
    state: &mut JsonMap,
    moved: &std::cell::RefCell<Vec<&'static str>>,
    overridden: &std::cell::RefCell<Vec<(&'static str, serde_json::Value, serde_json::Value)>>,
) -> Result<(), String> {
    // A malformed `tooling` is never silently replaced (#1939).
    if state.get("tooling").is_some_and(|t| !t.is_object()) {
        return Err("'tooling' must be a JSON object in the state file".to_string());
    }
    let Some(tracked) = decisions.get_mut("tooling") else {
        return Ok(());
    };
    let tracked = tracked
        .as_object_mut()
        .ok_or_else(|| "'tooling' must be a JSON object in the tracked config file".to_string())?;
    let tracked_wins = split_marker_present(state.get(SPLIT_MARKER_KEY));
    for key in STATE_KEYS {
        let Some(tracked_value) = tracked.remove(key) else {
            continue;
        };
        let state_tooling = state
            .entry("tooling")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .expect("checked above");
        match state_tooling.get(key) {
            // Never split: the tracked copy is our own interrupted copy.
            Some(_) if !tracked_wins => {}
            Some(state_value) => {
                overridden
                    .borrow_mut()
                    .push((key, state_value.clone(), tracked_value.clone()));
                state_tooling.insert(key.to_string(), tracked_value);
            }
            None => {
                state_tooling.insert(key.to_string(), tracked_value);
            }
        }
        moved.borrow_mut().push(key);
    }
    Ok(())
}

/// #2807 (C2) 3.4 - a no-op unless the state map holds a state key; otherwise
/// the D7 split marker, exactly `{ "v": 1, "keys": [the four names] }`.
fn stamp_split_marker(state: &mut JsonMap) {
    let holds_state_key = state
        .get("tooling")
        .and_then(|tooling| tooling.as_object())
        .is_some_and(|tooling| STATE_KEYS.iter().any(|key| tooling.contains_key(*key)));
    if holds_state_key {
        state.insert(
            SPLIT_MARKER_KEY.to_string(),
            serde_json::json!({ "v": SPLIT_MARKER_VERSION, "keys": STATE_KEYS }),
        );
    }
}

/// #2807 (C2) 3.5 - an audit note in the instance journal, nothing more: no
/// step record, and no decision ever reads it. A failure is logged and the
/// write still succeeded, because both files are already consistent.
fn note_moved_state_keys(decisions: &Path, journal_dir: Option<&Path>, moved: &[&str]) {
    use crate::config::naming_migration;
    let canonical = std::fs::canonicalize(decisions).unwrap_or_else(|_| decisions.to_path_buf());
    let scope = format!("config-state:{}", canonical.display());
    let note = format!("moved to the state file: {}", moved.join(", "));
    let noted = naming_migration::update_journal(journal_dir, |journal| {
        journal.set_status(&scope, naming_migration::ScopeStatus::Complete);
        journal.note(&scope, &note);
    });
    if let Err(refusal) = noted {
        log::warn!(
            "[config-state] could not record the move for {}: {:?}",
            decisions.display(),
            refusal
        );
    }
}

/// Ensure a key in a JSON map is an object, inserting `{}` if missing or resetting if corrupted.
fn ensure_object<'a>(
    map: &'a mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    context: &Path,
) -> &'a mut serde_json::Map<String, serde_json::Value> {
    let val = map.entry(key).or_insert_with(|| serde_json::json!({}));
    if !val.is_object() {
        log::warn!(
            "upsert_config: '{}' was not an object at {:?}, resetting",
            key,
            context
        );
        *val = serde_json::json!({});
    }
    val.as_object_mut().expect("just set to object")
}

/// Read-modify-write a single config.json: upsert tooling fields while preserving all others.
/// Uses serde_json::Value to avoid dropping unknown top-level fields (e.g. `identity`, `repos`)
/// that aren't part of the AgentLocalConfig struct.
fn upsert_config(
    config_path: &Path,
    agent_id: &str,
    entry: &CodingAgentEntry,
    write_descriptor: bool,
) -> Result<(), String> {
    update_agent_config(config_path, |_decisions, state| {
        // #1939 - the discriminator is the JSON shape, not the path: for both
        // `root/config.json` and `root/<agent_local_dir>/config.json`, an absent
        // top-level tooling is created, an object is preserved and updated, and
        // any present non-object (including null) is an error before
        // mutation/publication. The historical repair-by-reset is deliberately
        // gone: a malformed tooling is never silently replaced. Nested
        // `codingAgents` repair stays.
        let tooling_value = state
            .entry("tooling".to_string())
            .or_insert_with(|| serde_json::json!({}));
        let tooling = tooling_value.as_object_mut().ok_or_else(|| {
            format!(
                "'tooling' must be a JSON object at {}",
                config_path.display()
            )
        })?;
        tooling.insert("lastCodingAgent".to_string(), serde_json::json!(agent_id));

        let coding_agents = ensure_object(tooling, "codingAgents", config_path);
        let serde_json::Value::Object(mut new_fields) =
            serde_json::to_value(entry).map_err(|e| format!("Failed to serialize entry: {}", e))?
        else {
            return Err("CodingAgentEntry did not serialize to an object".to_string());
        };
        // #2433 - merge into the existing entry so an unknown key written by a
        // newer build survives; a non-object entry is replaced as before.
        let slot = coding_agents
            .entry(agent_id.to_string())
            .or_insert_with(|| serde_json::json!({}));
        if !slot.is_object() {
            *slot = serde_json::json!({});
        }
        let existing = slot.as_object_mut().expect("just set to object");
        for key in CODING_AGENT_ENTRY_KEYS {
            existing.remove(key);
        }
        for key in CODING_AGENT_DESCRIPTOR_KEYS {
            if write_descriptor {
                existing.remove(key);
            } else {
                new_fields.remove(key);
            }
        }
        existing.extend(new_fields);
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const T1: &str = "2026-09-02T01:00:00+00:00";
    const T2: &str = "2026-09-02T02:00:00+00:00";
    const T3: &str = "2026-09-02T03:00:00+00:00";

    /// Path of the per-instance config the writer under test targets.
    fn instance_config(dir: &Path) -> PathBuf {
        dir.join(crate::config::agent_local_dir_name())
            .join("config.json")
    }

    /// Seed `dir`'s per-instance config with `value`, creating the instance dir.
    fn seed_instance_config(dir: &Path, value: &serde_json::Value) {
        let path = instance_config(dir);
        std::fs::create_dir_all(path.parent().expect("instance dir")).expect("create instance dir");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(value).expect("serialize"),
        )
        .expect("seed config");
    }

    /// Raw JSON currently stored in `dir`'s per-instance config.
    fn stored(dir: &Path) -> serde_json::Value {
        let raw = std::fs::read_to_string(instance_config(dir)).expect("read stored config");
        serde_json::from_str(&raw).expect("stored config is JSON")
    }

    /// #2816 (C3) - raw JSON of the state file beside the tracked `config`.
    fn state_beside(config: &Path) -> serde_json::Value {
        let path = state_file_path(config.parent().expect("config dir"));
        let raw = std::fs::read_to_string(path).expect("read state file");
        serde_json::from_str(&raw).expect("state file is JSON")
    }

    /// #2816 (C3) - no state key may remain in the tracked `config`; an absent
    /// tracked file holds none.
    fn assert_no_state_key_tracked(config: &Path) {
        let Ok(raw) = std::fs::read_to_string(config) else {
            return;
        };
        let tracked: serde_json::Value =
            serde_json::from_str(&raw).expect("tracked config is JSON");
        for key in STATE_KEYS {
            assert!(
                tracked["tooling"].get(key).is_none(),
                "{key} is still tracked in {}",
                config.display()
            );
        }
    }

    fn set(dir: &Path, at: &str) -> Result<bool, String> {
        set_last_agent_message_at(dir.to_str().expect("utf-8 temp path"), at)
    }

    fn read(dir: &Path) -> Option<String> {
        read_last_agent_message_at(dir.to_str().expect("utf-8 temp path"))
    }

    #[test]
    fn set_last_agent_message_at_writes_only_the_instance_config() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();

        assert_eq!(set(dir, T2), Ok(true));

        assert_eq!(
            state_beside(&instance_config(dir))["tooling"]["lastAgentMessageAt"],
            serde_json::json!(T2)
        );
        assert_no_state_key_tracked(&instance_config(dir));
        // D2: the root copy is deliberately not a write target for this stamp.
        assert!(
            !dir.join("config.json").exists(),
            "the root config must not be written"
        );
    }

    #[test]
    fn set_last_agent_message_at_preserves_existing_tooling_and_unknown_keys() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        seed_instance_config(
            dir,
            &serde_json::json!({
                "tooling": {
                    "lastCodingAgent": "claude",
                    "codingAgents": {
                        "claude": { "app": "Claude Code", "lastUsed": T1 }
                    }
                },
                "repos": ["repo-AgentsCommander"]
            }),
        );

        assert_eq!(set(dir, T2), Ok(true));

        let after = state_beside(&instance_config(dir));
        assert_no_state_key_tracked(&instance_config(dir));
        assert_eq!(
            after["tooling"]["lastCodingAgent"],
            serde_json::json!("claude")
        );
        assert_eq!(
            after["tooling"]["codingAgents"]["claude"],
            serde_json::json!({ "app": "Claude Code", "lastUsed": T1 })
        );
        assert_eq!(
            stored(dir)["repos"],
            serde_json::json!(["repo-AgentsCommander"])
        );
        assert_eq!(
            after["tooling"]["lastAgentMessageAt"],
            serde_json::json!(T2)
        );
    }

    #[test]
    fn set_last_agent_message_at_is_monotonic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();

        assert_eq!(set(dir, T2), Ok(true));

        // Older instant: skipped, stored value untouched.
        assert_eq!(set(dir, T1), Ok(false));
        assert_eq!(read(dir), Some(T2.to_string()));

        // Newer instant: written.
        assert_eq!(set(dir, T3), Ok(true));
        assert_eq!(read(dir), Some(T3.to_string()));

        // An unparseable stored value is not a reason to skip.
        seed_instance_config(
            dir,
            &serde_json::json!({ "tooling": { "lastAgentMessageAt": "not-a-timestamp" } }),
        );
        assert_eq!(set(dir, T1), Ok(true));
        assert_eq!(read(dir), Some(T1.to_string()));
    }

    #[test]
    fn a_session_restart_rewrite_preserves_the_stamp() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        let repo = dir.to_str().expect("utf-8 temp path");

        assert_eq!(set(dir, T2), Ok(true));
        set_last_coding_agent(repo, "claude", "Claude Code", Some("sid"), None)
            .expect("restart rewrite");

        assert_eq!(read(dir), Some(T2.to_string()));
        assert_eq!(
            state_beside(&instance_config(dir))["tooling"]["lastCodingAgent"],
            serde_json::json!("claude")
        );
        assert_no_state_key_tracked(&instance_config(dir));
    }

    #[test]
    fn read_last_agent_message_at_binds_present_and_absent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();

        // Absent file.
        assert_eq!(read(dir), None);

        // Invalid JSON.
        let path = instance_config(dir);
        std::fs::create_dir_all(path.parent().expect("instance dir")).expect("create instance dir");
        std::fs::write(&path, "{ not json").expect("write invalid json");
        assert_eq!(read(dir), None);

        // Tooling absent.
        seed_instance_config(dir, &serde_json::json!({}));
        assert_eq!(read(dir), None);

        // Tooling present without the key.
        seed_instance_config(
            dir,
            &serde_json::json!({ "tooling": { "lastCodingAgent": "claude" } }),
        );
        assert_eq!(read(dir), None);

        // Tooling set to a non-object.
        seed_instance_config(dir, &serde_json::json!({ "tooling": 5 }));
        assert_eq!(read(dir), None);

        // #2816 (C3) consequence 5: the write on that seed is rejected by the
        // pair's cleanup and the tracked file is left byte for byte.
        let tracked = instance_config(dir);
        assert!(
            !state_file_path(tracked.parent().expect("instance dir")).exists(),
            "no state file, so the tracked check is the one measured"
        );
        let before = std::fs::read(&tracked).expect("tracked bytes before");
        let error = set(dir, T2).expect_err("a malformed tracked tooling is rejected");
        assert!(
            error.contains(
                "cleanup failed: 'tooling' must be a JSON object in the tracked config file"
            ),
            "{error}"
        );
        assert_eq!(
            std::fs::read(&tracked).expect("tracked bytes after"),
            before
        );
        assert_eq!(stored(dir)["tooling"], serde_json::json!(5));

        // Control: a well-formed value is read back.
        seed_instance_config(dir, &serde_json::json!({}));
        assert_eq!(set(dir, T2), Ok(true));
        assert_eq!(read(dir), Some(T2.to_string()));
    }

    #[test]
    fn is_empty_tracks_the_stamp() {
        assert!(AgentTooling::default().is_empty());
        assert!(!AgentTooling {
            last_agent_message_at: Some(T2.to_string()),
            ..Default::default()
        }
        .is_empty());
    }

    // ------------------------------------------------------------------
    // #1939 tooling-shape discriminator for config metadata writes.
    // ------------------------------------------------------------------

    fn codex_entry() -> CodingAgentEntry {
        CodingAgentEntry {
            app: "Codex".to_string(),
            ac_session_id: Some("sid".to_string()),
            last_used: T1.to_string(),
            command: String::new(),
            identity: BTreeMap::new(),
        }
    }

    fn root_config(dir: &Path) -> PathBuf {
        dir.join("config.json")
    }

    #[test]
    fn issue_1937_selection_state_tooling_shape_missing_and_object_succeed_on_both_targets() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        for path in [root_config(dir), instance_config(dir)] {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");

            // Missing file: the write creates the tooling object.
            upsert_config(&path, "codex", &codex_entry(), true)
                .expect("missing config must succeed");
            let saved = state_beside(&path);
            assert_eq!(saved["tooling"]["lastCodingAgent"], "codex");
            assert_no_state_key_tracked(&path);

            // Present object: preserved and updated.
            upsert_config(&path, "claude", &codex_entry(), true)
                .expect("object tooling must succeed");
            let saved = state_beside(&path);
            assert_eq!(saved["tooling"]["lastCodingAgent"], "claude");
            assert_eq!(saved["tooling"]["codingAgents"]["claude"]["app"], "Codex");
            assert_no_state_key_tracked(&path);
        }
    }

    #[test]
    fn issue_1937_selection_state_tooling_shape_non_object_fails_without_rewrite_on_both_targets() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        for path in [root_config(dir), instance_config(dir)] {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
            for literal in ["null", "5", "\"tooling\"", "[]", "false"] {
                let original = format!(r#"{{"tooling":{literal},"repos":["repo-a"]}}"#);
                std::fs::write(&path, &original).expect("seed");
                let error = upsert_config(&path, "codex", &codex_entry(), true)
                    .expect_err("non-object tooling must fail");
                assert!(
                    error.contains("'tooling' must be a JSON object"),
                    "{literal}: {error}"
                );
                assert_eq!(
                    std::fs::read_to_string(&path).expect("read"),
                    original,
                    "{literal}: bytes must be preserved"
                );
            }
        }
    }

    #[test]
    fn issue_1937_selection_state_tooling_shape_nested_coding_agents_repairs_on_both_targets() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        for path in [root_config(dir), instance_config(dir)] {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
            std::fs::write(
                &path,
                r#"{"tooling":{"lastCodingAgent":"claude","codingAgents":7},"repos":["repo-a"]}"#,
            )
            .expect("seed");
            upsert_config(&path, "codex", &codex_entry(), true)
                .expect("nested repair must succeed");
            let state = state_beside(&path);
            assert_eq!(state["tooling"]["codingAgents"]["codex"]["app"], "Codex");
            assert_eq!(state["tooling"]["lastCodingAgent"], "codex");
            assert_no_state_key_tracked(&path);
            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
            assert_eq!(saved["repos"][0], "repo-a");
        }
    }

    #[test]
    fn issue_1937_selection_state_tooling_shape_malformed_selection_locked_is_preserved() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        for path in [root_config(dir), instance_config(dir)] {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
            std::fs::write(
                &path,
                r#"{"tooling":{"selectionLocked":"yes","lastCodingAgent":"claude"},"repos":["repo-a"]}"#,
            )
            .expect("seed");
            upsert_config(&path, "codex", &codex_entry(), true)
                .expect("metadata write must succeed");
            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
            assert_eq!(saved["tooling"]["selectionLocked"], "yes");
            assert_eq!(state_beside(&path)["tooling"]["lastCodingAgent"], "codex");
            assert_no_state_key_tracked(&path);
            assert_eq!(saved["repos"][0], "repo-a");
        }
    }

    #[test]
    fn issue_1937_selection_state_tooling_shape_plain_repo_metadata_stays_compatible() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        for path in [root_config(dir), instance_config(dir)] {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
            std::fs::write(
                &path,
                r#"{"identity":"../../_agent_dev-rust","repos":["repo-a"],"tooling":{"lastCodingAgent":"claude"}}"#,
            )
            .expect("seed");
            upsert_config(&path, "codex", &codex_entry(), true)
                .expect("valid plain-repo metadata must stay compatible");
            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
            assert_eq!(saved["identity"], "../../_agent_dev-rust");
            assert_eq!(saved["repos"][0], "repo-a");
            assert_eq!(state_beside(&path)["tooling"]["lastCodingAgent"], "codex");
            assert_no_state_key_tracked(&path);
        }
    }

    // ── #2433 descriptor persistence ──

    fn fixed_entry(command: &str, identity: &[(&str, &str)]) -> CodingAgentEntry {
        CodingAgentEntry {
            app: "Claude Code".to_string(),
            ac_session_id: Some("sid".to_string()),
            last_used: T1.to_string(),
            command: command.to_string(),
            identity: identity
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn coding_agent_entry_round_trips_command_and_identity() {
        let entry = fixed_entry("claude --foo", &[("A", "9f2c1ab4de77c001"), ("B", "3e0a")]);
        let value = serde_json::to_value(&entry).expect("serialize");
        assert_eq!(value["command"], serde_json::json!("claude --foo"));
        assert_eq!(
            value["identity"],
            serde_json::json!({"A": "9f2c1ab4de77c001", "B": "3e0a"})
        );
        let back: CodingAgentEntry = serde_json::from_value(value.clone()).expect("deserialize");
        assert_eq!(serde_json::to_value(&back).expect("reserialize"), value);
    }

    #[test]
    fn entry_without_identity_deserializes() {
        let old = serde_json::json!({"app": "Codex", "acSessionId": "s", "lastUsed": T1});
        let entry: CodingAgentEntry = serde_json::from_value(old).expect("old shape loads");
        assert!(entry.identity.is_empty());
        assert!(entry.command.is_empty());
    }

    #[test]
    fn upsert_merges_into_an_existing_entry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        seed_instance_config(
            dir,
            &serde_json::json!({"tooling": {"codingAgents": {"claude": {
                "app": "Old", "acSessionId": "old-sid", "futureKey": {"x": 1}
            }}}}),
        );
        let mut entry = fixed_entry("claude", &[("A", "aa")]);
        entry.ac_session_id = None;
        upsert_config(&instance_config(dir), "claude", &entry, true).expect("upsert");

        let written = &state_beside(&instance_config(dir))["tooling"]["codingAgents"]["claude"];
        assert_no_state_key_tracked(&instance_config(dir));
        assert_eq!(written["futureKey"], serde_json::json!({"x": 1}));
        assert_eq!(written["command"], serde_json::json!("claude"));
        assert_eq!(written["identity"], serde_json::json!({"A": "aa"}));
        assert_eq!(written["app"], serde_json::json!("Claude Code"));
        // A known optional key this write omits does not linger.
        assert!(written.get("acSessionId").is_none(), "{written}");
    }

    #[test]
    fn agent_with_no_enabled_cells_writes_an_empty_identity_object() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        let repo = dir.to_str().expect("utf-8 temp path");
        set_last_coding_agent(
            repo,
            "claude",
            "Claude Code",
            None,
            Some(("claude", &BTreeMap::new())),
        )
        .expect("write");
        for config in [instance_config(dir), root_config(dir)] {
            assert_no_state_key_tracked(&config);
            let value = state_beside(&config);
            let entry = &value["tooling"]["codingAgents"]["claude"];
            assert_eq!(
                entry.get("identity"),
                Some(&serde_json::json!({})),
                "{entry}"
            );
            assert_eq!(entry["command"], serde_json::json!("claude"));
        }
    }

    #[test]
    fn writing_the_same_descriptor_twice_is_byte_identical() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        let path = instance_config(dir);
        std::fs::create_dir_all(path.parent().expect("instance dir")).expect("mkdir");
        let entry = fixed_entry("claude --x", &[("A", "aa"), ("C", "cc")]);
        let state = state_file_path(path.parent().expect("instance dir"));
        upsert_config(&path, "claude", &entry, true).expect("first");
        let first = std::fs::read(&state).expect("read first");
        upsert_config(&path, "claude", &entry, true).expect("second");
        assert_eq!(std::fs::read(&state).expect("read second"), first);
        assert_no_state_key_tracked(&path);
    }

    #[test]
    fn write_without_descriptor_leaves_the_existing_descriptor_untouched() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        let repo = dir.to_str().expect("utf-8 temp path");
        let identity = BTreeMap::from([("A".to_string(), "aa".to_string())]);
        set_last_coding_agent(repo, "gone", "Gone", None, Some(("gone --x", &identity)))
            .expect("first write");
        set_last_coding_agent(repo, "gone", "Gone", Some("sid2"), None).expect("second write");
        for config in [instance_config(dir), root_config(dir)] {
            assert_no_state_key_tracked(&config);
            let value = state_beside(&config);
            let entry = &value["tooling"]["codingAgents"]["gone"];
            assert_eq!(entry["command"], serde_json::json!("gone --x"), "{entry}");
            assert_eq!(entry["identity"], serde_json::json!({"A": "aa"}), "{entry}");
            assert_eq!(entry["acSessionId"], serde_json::json!("sid2"));
        }
    }

    #[test]
    fn write_without_descriptor_on_a_fresh_entry_writes_no_descriptor_keys() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        let repo = dir.to_str().expect("utf-8 temp path");
        set_last_coding_agent(repo, "gone", "Gone", None, None).expect("write");
        let value = state_beside(&instance_config(dir));
        assert_no_state_key_tracked(&instance_config(dir));
        let entry = &value["tooling"]["codingAgents"]["gone"];
        assert!(entry.get("command").is_none(), "{entry}");
        assert!(entry.get("identity").is_none(), "{entry}");
    }

    /// #2786 (C1) E7 - seed `config.json` and the state file in a fresh dir.
    fn loader_fixture(
        decisions: Option<serde_json::Value>,
        state: Option<serde_json::Value>,
    ) -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        if let Some(value) = decisions {
            std::fs::write(temp.path().join("config.json"), value.to_string())
                .expect("seed decisions");
        }
        if let Some(value) = state {
            std::fs::write(
                temp.path()
                    .join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
                value.to_string(),
            )
            .expect("seed state");
        }
        temp
    }

    /// Every file in `dir` with its bytes, to prove the loader wrote nothing.
    fn dir_bytes(dir: &Path) -> Vec<(std::ffi::OsString, Vec<u8>)> {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .expect("read dir")
            .map(|entry| {
                let entry = entry.expect("entry");
                (
                    entry.file_name(),
                    std::fs::read(entry.path()).expect("read"),
                )
            })
            .collect();
        files.sort();
        files
    }

    /// The four state keys, read through the loader, as one JSON object.
    fn loaded_state_keys(dir: &Path) -> serde_json::Value {
        let before = dir_bytes(dir);
        let config = read_agent_local_config(dir).expect("the loader returns a config");
        let raw = read_agent_local_config_json(dir)
            .expect("parseable")
            .expect("present");
        assert_eq!(dir_bytes(dir), before, "the loader must write nothing");
        serde_json::json!({
            "lastCodingAgent": config.tooling.last_coding_agent,
            "codingAgents": config.tooling.coding_agents.keys().collect::<Vec<_>>(),
            "lastAgentMessageAt": config.tooling.last_agent_message_at,
            "profileContentHash": raw["tooling"]["profileContentHash"],
        })
    }

    fn all_four(tag: &str) -> serde_json::Value {
        serde_json::json!({
            "lastCodingAgent": format!("{tag}-agent"),
            "codingAgents": {format!("{tag}-agent"): {"app": tag}},
            "lastAgentMessageAt": format!("{tag}-at"),
            "profileContentHash": format!("{tag}-hash"),
        })
    }

    fn expect_all_four(tag: &str) -> serde_json::Value {
        serde_json::json!({
            "lastCodingAgent": format!("{tag}-agent"),
            "codingAgents": [format!("{tag}-agent")],
            "lastAgentMessageAt": format!("{tag}-at"),
            "profileContentHash": format!("{tag}-hash"),
        })
    }

    #[test]
    fn the_loader_is_key_wise_and_marker_aware() {
        let marker = serde_json::json!({"v": 1, "keys": STATE_KEYS});

        // 1. State only.
        let temp = loader_fixture(
            None,
            Some(serde_json::json!({"tooling": all_four("state")})),
        );
        assert_eq!(loaded_state_keys(temp.path()), expect_all_four("state"));

        // 2. Decisions only, with a non-state key the loader must keep.
        let mut decisions = all_four("tracked");
        decisions["telegramBot"] = serde_json::json!("bot");
        let temp = loader_fixture(Some(serde_json::json!({"tooling": decisions})), None);
        assert_eq!(loaded_state_keys(temp.path()), expect_all_four("tracked"));
        assert_eq!(
            read_agent_local_config(temp.path())
                .expect("config")
                .tooling
                .telegram_bot
                .as_deref(),
            Some("bot")
        );

        // 3. A state file with two of the four keys hides none of the others.
        let temp = loader_fixture(
            Some(serde_json::json!({"tooling": all_four("tracked")})),
            Some(serde_json::json!({"tooling": {
                "lastCodingAgent": "state-agent",
                "profileContentHash": "state-hash",
            }})),
        );
        assert_eq!(
            loaded_state_keys(temp.path()),
            serde_json::json!({
                "lastCodingAgent": "state-agent",
                "codingAgents": ["tracked-agent"],
                "lastAgentMessageAt": "tracked-at",
                "profileContentHash": "state-hash",
            })
        );

        // 4. Both files, marker present: the tracked value wins.
        let temp = loader_fixture(
            Some(serde_json::json!({"tooling": all_four("tracked")})),
            Some(serde_json::json!({"tooling": all_four("state"), "split": marker})),
        );
        assert_eq!(loaded_state_keys(temp.path()), expect_all_four("tracked"));

        // 5. Both files, no marker: the state file wins.
        let temp = loader_fixture(
            Some(serde_json::json!({"tooling": all_four("tracked")})),
            Some(serde_json::json!({"tooling": all_four("state")})),
        );
        assert_eq!(loaded_state_keys(temp.path()), expect_all_four("state"));

        // 6. Marker shapes: only an integer `v` >= 1 is present.
        let shapes = [
            (serde_json::json!(null), "state"),
            (serde_json::json!(3), "state"),
            (serde_json::json!({"v": "1"}), "state"),
            (serde_json::json!({}), "state"),
            (serde_json::json!({"v": 2, "keys": []}), "tracked"),
        ];
        for (shape, winner) in shapes {
            let temp = loader_fixture(
                Some(serde_json::json!({"tooling": all_four("tracked")})),
                Some(serde_json::json!({"tooling": all_four("state"), "split": shape.clone()})),
            );
            assert_eq!(
                loaded_state_keys(temp.path()),
                expect_all_four(winner),
                "split = {shape}"
            );
        }
    }

    /// #2786 (C1) - a duplicate known field is still a typed rejection, as it
    /// was for the direct typed readers: parsing through `Value` must not
    /// collapse it to the last value. A state file with one is ignored.
    #[test]
    fn the_typed_loader_rejects_duplicate_known_fields() {
        let duplicate = r#"{"tooling":{"lastCodingAgent":"a","lastCodingAgent":"b"}}"#;
        assert!(serde_json::from_str::<AgentLocalConfig>(duplicate).is_err());

        let temp = loader_fixture(None, None);
        std::fs::write(temp.path().join("config.json"), duplicate).expect("seed decisions");
        assert!(read_agent_local_config(temp.path()).is_none());

        let temp = loader_fixture(
            Some(serde_json::json!({"tooling": {"lastCodingAgent": "tracked"}})),
            None,
        );
        std::fs::write(
            temp.path()
                .join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
            duplicate,
        )
        .expect("seed state");
        assert_eq!(
            read_agent_local_config(temp.path())
                .expect("config")
                .tooling
                .last_coding_agent
                .as_deref(),
            Some("tracked")
        );
    }

    /// #2786 C1 E7e: the raw wrapper filters the STATE file through the typed
    /// parse and leaves `config.json` on its base `Value` semantics.
    #[test]
    fn the_raw_loader_filters_the_state_file_but_not_the_tracked_file() {
        let state_name = crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME;
        let raw = |dir: &Path| {
            read_agent_local_config_json(dir)
                .expect("json")
                .expect("some")
        };

        // (1) a tracked duplicate is last wins, with no state file.
        let temp = loader_fixture(None, None);
        std::fs::write(
            temp.path().join("config.json"),
            r#"{"tooling":{"lastCodingAgent":"a","lastCodingAgent":"b"}}"#,
        )
        .expect("seed decisions");
        assert_eq!(raw(temp.path())["tooling"]["lastCodingAgent"], "b");

        // (2) an unrelated malformed tracked field is tolerated.
        let temp = loader_fixture(
            Some(
                serde_json::json!({"tooling": {"lastCodingAgent": "claude", "codingAgents": null}}),
            ),
            None,
        );
        assert_eq!(raw(temp.path())["tooling"]["lastCodingAgent"], "claude");

        // (3) and (4): a state file that fails the typed parse is ignored and
        // the tracked value serves the key.
        let tracked = serde_json::json!({"tooling": {
            "lastCodingAgent": "tracked",
            "codingAgents": {"tracked": {"command": "tracked"}}}});
        for (leg, state) in [
            (
                "3 codingAgents null",
                r#"{"tooling":{"codingAgents":null}}"#,
            ),
            (
                "4 duplicate known field",
                r#"{"tooling":{"lastCodingAgent":"a","lastCodingAgent":"b"}}"#,
            ),
        ] {
            let temp = loader_fixture(Some(tracked.clone()), None);
            std::fs::write(temp.path().join(state_name), state).expect("seed state");
            assert_eq!(raw(temp.path())["tooling"], tracked["tooling"], "leg {leg}");
        }

        // (5) control: a well-formed state file for the same key does overlay.
        let temp = loader_fixture(
            Some(tracked.clone()),
            Some(serde_json::json!({"tooling": {"lastCodingAgent": "state"}})),
        );
        assert_eq!(raw(temp.path())["tooling"]["lastCodingAgent"], "state");
    }

    // ---- #2786 C1 r28: harness child-ownership rows ----
    // These are NOT mutant classes of E15. They prove the harness itself never
    // leaves a child behind when supervision fails, and they drive a BLOCKING
    // inner row, so the child is certainly alive when the injected fault fires.

    /// Blocks until the harness terminates it. It is armed, but it loads
    /// nothing, so it opens no observation and records nothing.
    #[test]
    #[ignore = "#2786 C1: driven only by the harness child-ownership rows"]
    fn inner_sleeps_until_killed() {
        println!("SLEEPER-READY");
        std::thread::sleep(std::time::Duration::from_secs(120));
    }

    #[test]
    fn an_injected_wait_error_still_terminates_and_reaps_the_child() {
        use load_probe_harness::{run_inner_with_fault, Fault, SLEEPER};
        let r = run_inner_with_fault(SLEEPER, Fault::WaitError);
        let reported = r
            .wait_error
            .as_deref()
            .expect("the injected wait error must be reported, not discarded");
        assert!(reported.contains("injected try_wait failure"), "{reported}");
        let c = r
            .cleanup
            .as_ref()
            .expect("an errored wait must hand back a cleanup report");
        assert!(
            c.reaped_ok(),
            "the child was not reaped after the wait error: {}",
            c.describe()
        );
        assert!(
            c.kill_error.is_none(),
            "terminating the child failed, and the harness says so: {}",
            c.describe()
        );
        assert!(!r.ok, "a supervision failure is never a green child");
    }

    #[test]
    fn a_timed_out_child_is_terminated_and_reaped() {
        use load_probe_harness::{run_inner_with_fault, Fault, SLEEPER};
        let r = run_inner_with_fault(SLEEPER, Fault::Timeout);
        assert!(r.timed_out, "the past deadline must time the child out");
        assert!(r.wait_error.is_none(), "{:?}", r.wait_error);
        let c = r
            .cleanup
            .as_ref()
            .expect("a timeout must hand back a cleanup report");
        assert!(
            c.reaped_ok(),
            "the child was not reaped after the timeout: {}",
            c.describe()
        );
        // r29 item 3: this row checks the KILL error report too, on its own,
        // exactly as the wait-error row does. Without it, a harness that
        // reported a failed termination still passed this row in silence. The
        // check is deliberately NOT in the shared `assert_one_test_ran`: two
        // redundant guards hide each other from mutation.
        assert!(
            c.kill_error.is_none(),
            "terminating the timed-out child reported an error: {}",
            c.describe()
        );
        assert!(!r.ok, "a timed-out child is never a green child");
    }

    // ---- #2786 C1 r30 item 5: DIRECT controls on the failed-kill branch ----
    // Each one drives the blocking sleeper, injects its fault, and asserts the
    // OUTCOME, the BOUND, the row's RED verdict and the pid's state before and
    // after cleanup. The report-injection control k1 is kept as it was: it
    // proves the REPORTING, never this branch.

    /// The shared part: assert that `assert_one_test_ran`, the very assertion
    /// every observation row uses, rejects this run. That is what "the row is
    /// RED" means; nothing weaker and no new assertion.
    fn assert_the_shared_gate_rejects(r: &load_probe_harness::ChildRun, needle: &str) {
        use load_probe_harness::assert_one_test_ran;
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let caught = std::panic::catch_unwind(|| {
            assert_one_test_ran(r, load_probe_harness::SLEEPER);
        });
        std::panic::set_hook(hook);
        let err = caught.expect_err("the shared gate must reject this run");
        let msg = err.downcast_ref::<String>().cloned().unwrap_or_else(|| {
            err.downcast_ref::<&str>()
                .map(|s| s.to_string())
                .unwrap_or_default()
        });
        assert!(
            msg.contains(needle),
            "the shared gate rejected it for the wrong reason, no {needle:?} in: {msg}"
        );
        println!("ROW-RED shared gate rejected the run: {msg}");
    }

    /// r31 F4: the cleanup guard, adopted as the FIRST statement of a control
    /// row, before any fallible assertion.
    ///
    /// r30's Pending and Failed rows ran every assertion before they killed
    /// anything, so the dev's 6 s mutant panicked with the 120 s sleeper still
    /// running and nothing left to clean it up. This guard takes the still-
    /// owned `Child` out of the report at the top of the row. From then on it
    /// owns the process on EVERY exit path: the normal one, where the row
    /// calls `prove_alive_then_reap`, and the panic one, where `Drop` kills
    /// and reaps it inside a bounded poll.
    ///
    /// It owns a real `Child`, not a pid. A pid can only be killed, and on a
    /// Unix host killing this process's own child without waiting leaves a
    /// zombie that `ps` still reports. `wait()` on the handle is the portable
    /// reap, on both platforms, with no external command and no dependency.
    struct AdoptedChild {
        pid: u32,
        child: Option<std::process::Child>,
    }

    impl AdoptedChild {
        fn adopt(r: &mut load_probe_harness::ChildRun) -> AdoptedChild {
            match r.cleanup.as_mut() {
                Some(c) => AdoptedChild {
                    pid: c.pid,
                    child: c.unreaped.take(),
                },
                None => AdoptedChild {
                    pid: 0,
                    child: None,
                },
            }
        }

        /// Prove the pid is ALIVE, prove the detector can also report a query
        /// it could not answer, then reap through the owned handle and prove
        /// the pid is gone. All four legs, every time, bounded.
        ///
        /// Why this is not the shell teardown artefact the r29 evidence hit:
        /// the `Alive` reading is taken here, inside the test process that
        /// spawned the child and which is still running, before anything is
        /// torn down. No command substitution, no wrapper shell, no job object.
        fn prove_alive_then_reap(&mut self) {
            use load_probe_harness::{bounded_kill_and_reap, pid_state, pid_state_by_filter};
            use load_probe_harness::{PidState, BAD_PID_SELECTOR};
            let pid = self.pid;
            let mut child = self
                .child
                .take()
                .expect("#2786 C1 r31 F4: the guard must own the un-reaped child");
            let before = pid_state(pid);
            assert_eq!(
                before,
                PidState::Alive,
                "the injected failed kill must leave pid {pid} running; the detector said                  {before:?}"
            );
            println!("PID-BEFORE-CLEANUP pid={pid} state={before:?} (the harness did NOT reap it)");
            let bad = pid_state_by_filter(BAD_PID_SELECTOR);
            assert!(
                matches!(bad, PidState::QueryError(_)),
                "a malformed query must be a query error, not a verdict: {bad:?}"
            );
            println!("PID-QUERY-ERROR-CONTROL {bad:?}");
            let report = bounded_kill_and_reap(&mut child, pid, std::time::Duration::from_secs(10));
            println!("OWNED-REAP {report}");
            assert!(
                report.contains("REAPED status"),
                "the owned handle must really reap the child: {report}"
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut after = pid_state(pid);
            while after == PidState::Alive && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(50));
                after = pid_state(pid);
            }
            assert_eq!(
                after,
                PidState::Dead,
                "pid {pid} survived the owned reap: {after:?}"
            );
            println!("PID-AFTER-CLEANUP pid={pid} state={after:?}");
        }
    }

    impl Drop for AdoptedChild {
        fn drop(&mut self) {
            if let Some(mut c) = self.child.take() {
                let r = load_probe_harness::bounded_kill_and_reap(
                    &mut c,
                    self.pid,
                    std::time::Duration::from_secs(10),
                );
                eprintln!("#2786 C1 r31 F4: ADOPTED-CLEANUP on the unwind path: {r}");
            }
        }
    }

    #[test]
    fn a_failed_kill_with_a_live_child_is_reported_as_pending_within_the_budget() {
        use load_probe_harness::{run_inner_with_fault, Fault, SLEEPER};
        let t0 = std::time::Instant::now();
        let mut r = run_inner_with_fault(SLEEPER, Fault::KillFails);
        let elapsed = t0.elapsed();
        // r31 F4: FIRST, before a single fallible assertion. From here the
        // guard owns the surviving child on every exit path, panic included.
        let mut adopted = AdoptedChild::adopt(&mut r);
        assert!(
            adopted.child.is_some(),
            "the report must hand over the un-reaped child, not just its pid"
        );
        let c = r
            .cleanup
            .as_ref()
            .expect("a failed kill must hand back a cleanup report");
        assert_eq!(
            c.outcome.label(),
            "CLEANUP-PENDING",
            "a failed kill on a live child must be PENDING: {}",
            c.describe()
        );
        assert!(
            !c.reaped_ok(),
            "PENDING is never a reaped child: {}",
            c.describe()
        );
        assert!(
            c.kill_error
                .as_deref()
                .is_some_and(|e| e.contains("injected kill failure")),
            "the failed kill must be reported: {}",
            c.describe()
        );
        // The bound, from both sides: the poll ran its full 5 s ceiling and
        // the row did not hang. 30 s is the ceiling of the assertion, not an
        // expected duration.
        assert!(
            elapsed >= std::time::Duration::from_secs(5)
                && elapsed <= std::time::Duration::from_secs(30),
            "the bounded poll took {elapsed:?}"
        );
        println!("PENDING-CONTROL {} in {elapsed:?}", c.describe());
        assert_the_shared_gate_rejects(&r, "was not reaped");
        adopted.prove_alive_then_reap();
    }

    #[test]
    fn a_failed_kill_with_an_errored_poll_is_reported_as_failed_at_once() {
        use load_probe_harness::{run_inner_with_fault, Fault, SLEEPER};
        let t0 = std::time::Instant::now();
        let mut r = run_inner_with_fault(SLEEPER, Fault::KillFailsAndReapErrors);
        let elapsed = t0.elapsed();
        // r31 F4: FIRST, before a single fallible assertion. From here the
        // guard owns the surviving child on every exit path, panic included.
        let mut adopted = AdoptedChild::adopt(&mut r);
        assert!(
            adopted.child.is_some(),
            "the report must hand over the un-reaped child, not just its pid"
        );
        let c = r
            .cleanup
            .as_ref()
            .expect("an errored reap must hand back a cleanup report");
        assert_eq!(
            c.outcome.label(),
            "REAP-FAILED",
            "an errored poll must be REAP-FAILED: {}",
            c.describe()
        );
        assert!(
            !c.reaped_ok(),
            "FAILED is never a reaped child: {}",
            c.describe()
        );
        assert!(
            c.outcome.detail().contains("injected try_wait failure"),
            "the poll error must be carried, not discarded: {}",
            c.describe()
        );
        // It must NOT spend the 5 s budget: the error breaks out at once.
        assert!(
            elapsed <= std::time::Duration::from_secs(20),
            "the errored poll took {elapsed:?}"
        );
        println!("FAILED-CONTROL {} in {elapsed:?}", c.describe());
        assert_the_shared_gate_rejects(&r, "was not reaped");
        adopted.prove_alive_then_reap();
    }

    #[test]
    fn a_successful_kill_is_reported_as_reaped_and_leaves_no_live_pid() {
        use load_probe_harness::{pid_state, run_inner_with_fault, Fault, PidState, SLEEPER};
        let t0 = std::time::Instant::now();
        let r = run_inner_with_fault(SLEEPER, Fault::Timeout);
        let elapsed = t0.elapsed();
        let c = r
            .cleanup
            .as_ref()
            .expect("a timeout must hand back a cleanup report");
        assert_eq!(c.outcome.label(), "reaped", "{}", c.describe());
        assert!(c.reaped_ok(), "{}", c.describe());
        assert!(c.kill_error.is_none(), "{}", c.describe());
        assert!(
            elapsed <= std::time::Duration::from_secs(30),
            "the reaping path took {elapsed:?}"
        );
        let after = pid_state(c.pid);
        assert_eq!(
            after,
            PidState::Dead,
            "a reaped child must leave no live pid: {after:?}"
        );
        println!(
            "REAPED-CONTROL {} in {elapsed:?} pid state after={after:?}",
            c.describe()
        );
        // Same shared gate, same rejection: a timed-out run is never green,
        // for a different reason, so the two verdicts stay distinguishable.
        assert_the_shared_gate_rejects(&r, "timed out");
    }

    #[test]
    fn an_unwind_between_spawn_and_reap_still_reaps_the_child() {
        use load_probe_harness::{run_inner_with_fault, take_unwind_cleanups, Fault, SLEEPER};
        let _ = take_unwind_cleanups();
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let caught =
            std::panic::catch_unwind(|| run_inner_with_fault(SLEEPER, Fault::PanicWhileOwned));
        std::panic::set_hook(hook);
        assert!(caught.is_err(), "the injected unwind must reach the row");
        let done = take_unwind_cleanups();
        assert_eq!(
            done.len(),
            1,
            "the guard must reap exactly one child on the unwind path: {done:?}"
        );
        assert!(
            done[0].reaped_ok(),
            "the child was not reaped on the unwind path: {}",
            done[0].describe()
        );
        // r29 item 3: the unwind path checks the KILL error report on its own
        // too. See the comment in the timeout row for why it is not shared.
        assert!(
            done[0].kill_error.is_none(),
            "terminating the child on the unwind path reported an error: {}",
            done[0].describe()
        );
    }

    // #2786 C1 E15, third selector: an off-thread load IS counted. One load on
    // a spawned, joined thread and one on the test thread, two directories.
    #[test]
    fn the_load_probe_observes_every_thread() {
        super::load_probe_harness::expect_child_pass(
            "config::agent_config::tests::inner_the_load_probe_observes_every_thread",
        );
    }

    #[test]
    #[ignore = "#2786 C1: runs only in the dedicated armed process"]
    fn inner_the_load_probe_observes_every_thread() {
        let off = tempfile::tempdir().expect("tempdir");
        let on = tempfile::tempdir().expect("tempdir");
        let obs = super::load_probe::Observation::open();
        let off_dir = off.path().to_path_buf();
        std::thread::spawn(move || super::read_agent_local_config(&off_dir))
            .join()
            .expect("the off-thread load");
        super::read_agent_local_config_json(on.path()).expect("the on-thread load");
        println!("REACHED-CALLER");
        let loads = obs.loads();
        let want = vec![off.path().to_path_buf(), on.path().to_path_buf()];
        assert_eq!(loads, want, "recorded {loads:?}, expected exactly {want:?}");
    }

    // #2786 C1 E15, fourth selector: inside the armed process a wrapper load
    // with no open observation panics, naming the directory.
    #[test]
    fn a_load_with_no_open_observation_fails_closed() {
        super::load_probe_harness::expect_child_pass(
            "config::agent_config::tests::inner_a_load_with_no_open_observation_fails_closed",
        );
    }

    #[test]
    #[ignore = "#2786 C1: runs only in the dedicated armed process"]
    fn inner_a_load_with_no_open_observation_fails_closed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _slot = super::load_probe::NoObservation::open();
        println!("REACHED-CALLER");
        let err = std::panic::catch_unwind(|| super::read_agent_local_config(tmp.path()))
            .expect_err("a load with no open observation must panic");
        let text = err.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(
            text.contains("wrapper load with no open Observation")
                && text.contains(&format!("{:?}", tmp.path())),
            "the panic must name the directory: {text:?}"
        );
    }

    // ── #2807 (C2): the config pair entry point ─────────────────────────────

    use serde_json::{json, Value};
    use std::time::{Duration, Instant};

    const CLEANUP_STATE: &str = "after_cleanup_state_publish";
    const CLEANUP_TRACKED: &str = "after_cleanup_tracked_write";
    const CALLER_STATE: &str = "after_caller_state_publish";
    const CALLER_DECISIONS: &str = "after_caller_decisions_write";
    /// Fired by no production build: only E1f's scratch mutant opens this gap.
    const MUTANT_GAP: &str = "mutant_gap_open";

    const PAIR_CHILD_FQN: &str = "config::agent_config::tests::config_pair_child_entry";
    const PAIR_CHILD_DECISIONS_ENV: &str = "AC_2807_CHILD_DECISIONS";
    const PAIR_CHILD_JOURNAL_ENV: &str = "AC_2807_CHILD_JOURNAL";
    const PAIR_CHILD_RENDEZVOUS_ENV: &str = "AC_2807_CHILD_RENDEZVOUS";
    const PAIR_CHILD_STOPS_ENV: &str = "AC_2807_CHILD_STOPS";
    const PAIR_CHILD_MUTATE_ENV: &str = "AC_2807_CHILD_MUTATE";
    const PAIR_CHILD_SEQUENCE_ENV: &str = "AC_2807_CHILD_SEQUENCE";
    const READY_FILE: &str = "paused.ready";
    const RELEASE_FILE: &str = "paused.release";
    const STAGES_LOG: &str = "stages.log";
    const HOOK_BOUND: Duration = Duration::from_secs(60);

    /// One agent directory under an `.ac` root, plus an instance journal
    /// directory outside it.
    struct Pair {
        tmp: tempfile::TempDir,
        dir: PathBuf,
        journal: PathBuf,
    }

    fn pair() -> Pair {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join(".ac").join("_agent_a");
        let journal = tmp.path().join("journal");
        std::fs::create_dir_all(&dir).expect("create agent dir");
        std::fs::create_dir_all(&journal).expect("create journal dir");
        Pair { tmp, dir, journal }
    }

    fn put_json(path: &Path, value: &Value) {
        let mut text = serde_json::to_string_pretty(value).expect("serialize");
        text.push('\n');
        std::fs::write(path, text).expect("seed file");
    }

    fn get_json(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("read file")).expect("JSON")
    }

    fn bytes_of(path: &Path) -> Option<Vec<u8>> {
        std::fs::read(path).ok()
    }

    fn split_marker() -> Value {
        json!({ "v": 1, "keys": STATE_KEYS })
    }

    /// A state file that carries the split marker.
    fn marked(tooling: Value) -> Value {
        json!({ "tooling": tooling, "split": split_marker() })
    }

    fn state_keys_in(file: &Value) -> Vec<&'static str> {
        STATE_KEYS
            .into_iter()
            .filter(|key| file["tooling"].get(key).is_some())
            .collect()
    }

    fn no_change(_: &mut JsonMap, _: &mut JsonMap) -> Result<(), String> {
        Ok(())
    }

    fn set_tooling(map: &mut JsonMap, key: &str, value: Value) {
        map.entry("tooling")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("tooling object")
            .insert(key.to_string(), value);
    }

    /// Every file under `root`, with its bytes; a directory is `None`.
    fn tree_of(root: &Path) -> std::collections::BTreeMap<PathBuf, Option<Vec<u8>>> {
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

    fn append_line(path: &Path, line: &str) {
        use std::io::Write as _;
        // One write in append mode: atomic for a short line, from any process.
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| file.write_all(format!("{line}\n").as_bytes()))
            .expect("append line");
    }

    fn lines_of(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    impl Pair {
        fn decisions(&self) -> PathBuf {
            self.dir.join("config.json")
        }

        fn state(&self) -> PathBuf {
            state_file_path(&self.dir)
        }

        fn seed_decisions(&self, value: Value) {
            put_json(&self.decisions(), &value);
        }

        fn seed_state(&self, value: Value) {
            put_json(&self.state(), &value);
        }

        fn tracked(&self) -> Value {
            get_json(&self.decisions())
        }

        fn stored_state(&self) -> Value {
            get_json(&self.state())
        }

        /// The value a reader gets, through the loader.
        fn read(&self, key: &str) -> Value {
            read_agent_local_config_json(&self.dir)
                .expect("loader")
                .expect("a config")["tooling"][key]
                .clone()
        }

        fn write<F>(&self, mutate: F) -> Result<(), String>
        where
            F: FnOnce(&mut JsonMap, &mut JsonMap) -> Result<(), String>,
        {
            update_agent_config_in(&self.decisions(), Some(&self.journal), mutate)
        }

        /// A writer outside this binary's protocol: it puts a state key back
        /// into the tracked file with a plain write.
        fn foreign_write(&self, key: &str, value: Value) {
            let mut tracked = self.tracked();
            set_tooling(tracked.as_object_mut().expect("object"), key, value);
            put_json(&self.decisions(), &tracked);
        }

        fn publish_temp(&self, file_name: &str) -> PathBuf {
            self.dir
                .join(format!(".{file_name}.{}.tmp", std::process::id()))
        }

        fn decisions_temp(&self) -> PathBuf {
            self.publish_temp("config.json")
        }

        fn state_temp(&self) -> PathBuf {
            self.publish_temp(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
        }

        fn journal_bytes(&self) -> Option<Vec<u8>> {
            bytes_of(
                &self
                    .journal
                    .join(crate::config::instance_artifacts::NAMING_MIGRATION_STATE_NAME),
            )
        }
    }

    /// A child process driving `update_agent_config_with_stage`, stopped at
    /// named stages through one rendezvous subdirectory per stage.
    struct PairChild {
        child: std::process::Child,
        rendezvous: PathBuf,
    }

    fn spawn_pair_child(
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
        fn ready(&self, stage: &str) -> bool {
            self.rendezvous.join(stage).join(READY_FILE).exists()
        }

        /// The first of `stages` the child stops at, or `None` when the bound
        /// passes or the child exits first.
        fn first_ready<'a>(&mut self, stages: &[&'a str], bound: Duration) -> Option<&'a str> {
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

        fn release(&self, stage: &str) {
            std::fs::write(self.rendezvous.join(stage).join(RELEASE_FILE), b"release")
                .expect("release the child");
        }

        fn kill(&mut self) -> String {
            let killed = self.child.kill();
            format!("kill {killed:?}, exit {:?}", self.child.wait())
        }

        /// Waits for the child to end by itself.
        fn exit(&mut self) -> std::process::ExitStatus {
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
        fn stages(&self) -> Vec<String> {
            lines_of(&self.rendezvous.join(STAGES_LOG))
        }

        /// Stops the child at `stage`, kills it there, and returns the stages
        /// it fired.
        fn kill_at(&mut self, stage: &str) -> Vec<String> {
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

    fn sample_state_values() -> [(&'static str, Value); 4] {
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
    fn text(value: &Value) -> String {
        serde_json::to_string(value).expect("serialize")
    }

    // -- E1 ------------------------------------------------------------------

    #[test]
    fn every_state_key_moves_without_loss() {
        // (a) decisions-only, per key.
        for (key, value) in sample_state_values() {
            let p = pair();
            p.seed_decisions(json!({ "tooling": { key: value.clone(), "profile": "keep" } }));
            p.write(no_change).expect("migrate");
            let state = p.stored_state();
            assert_eq!(
                text(&state["tooling"][key]),
                text(&value),
                "(a) {key} must land in the state file byte for byte"
            );
            assert_eq!(
                p.tracked(),
                json!({ "tooling": { "profile": "keep" } }),
                "(a) {key} must leave the decisions file and nothing else may"
            );
            assert_eq!(state["split"], split_marker(), "(a) {key}: marker present");
        }

        // (b) conflict on a pair that was never split: the state value survives.
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "tracked" } }));
        p.seed_state(json!({ "tooling": { "lastCodingAgent": "state" } }));
        p.write(no_change).expect("migrate a conflict");
        assert_eq!(
            p.stored_state()["tooling"]["lastCodingAgent"],
            json!("state"),
            "(b) with no marker the state file's value must survive"
        );
        assert!(
            state_keys_in(&p.tracked()).is_empty(),
            "(b) the decisions file must lose its copy"
        );

        // (c) a second run on the migrated tree.
        let p = pair();
        let all: serde_json::Map<String, Value> = sample_state_values()
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect();
        p.seed_decisions(json!({ "tooling": all.clone() }));
        p.write(no_change).expect("first run");
        let (tracked, state) = (bytes_of(&p.decisions()), bytes_of(&p.state()));
        p.write(no_change).expect("second run");
        assert_eq!(
            p.stored_state()["tooling"],
            Value::Object(all),
            "(c) a second run must keep every state value"
        );
        assert_eq!(
            (bytes_of(&p.decisions()), bytes_of(&p.state())),
            (tracked, state),
            "(c) a second run must write neither file"
        );

        // (d) no state file, and the decisions write is forced to fail.
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude" } }));
        let tracked = bytes_of(&p.decisions());
        std::fs::create_dir(p.decisions_temp()).expect("block the decisions publish");
        p.write(no_change)
            .expect_err("(d) the decisions write must fail");
        assert_eq!(
            bytes_of(&p.state()).map(|_| p.stored_state()["tooling"]["lastCodingAgent"].clone()),
            Some(json!("claude")),
            "(d) the state file must already hold the value when the decisions write fails"
        );
        assert_eq!(
            bytes_of(&p.decisions()),
            tracked,
            "(d) decisions file unchanged"
        );
    }

    // -- E1b -----------------------------------------------------------------

    #[test]
    fn a_value_written_back_by_an_older_binary_wins() {
        /// The foreign `v2` is in the tracked file: a read returns it and
        /// writes nothing, and the next write leaves it in the state file.
        fn check(shape: &str, p: &Pair, journal: &Path, failures: &mut Vec<String>) {
            let before = tree_of(&p.dir);
            let read = p.read("lastCodingAgent");
            if read != json!("v2") {
                failures.push(format!("({shape}) a read must return v2, got {read}"));
            }
            if tree_of(&p.dir) != before {
                failures.push(format!("({shape}) a read must write nothing"));
            }
            if let Err(e) = update_agent_config_in(&p.decisions(), Some(journal), no_change) {
                failures.push(format!("({shape}) the next write failed: {e}"));
                return;
            }
            let state = p.stored_state()["tooling"]["lastCodingAgent"].clone();
            if state != json!("v2") {
                failures.push(format!(
                    "({shape}) the next write must leave v2 in the state file, got {state}"
                ));
            }
            let left = state_keys_in(&p.tracked());
            if !left.is_empty() {
                failures.push(format!(
                    "({shape}) the tracked file must hold no state key, holds {left:?}"
                ));
            }
        }
        let migrated = || {
            let p = pair();
            p.seed_decisions(json!({ "tooling": {} }));
            p.seed_state(marked(json!({ "lastCodingAgent": "v1" })));
            p
        };
        let mut failures = Vec::new();

        // (a) after a completed migration.
        let p = migrated();
        p.foreign_write("lastCodingAgent", json!("v2"));
        check("a", &p, &p.journal, &mut failures);

        // (c) a pair created after phase C, built by the entry point itself.
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "profile": "keep" } }));
        p.write(|_, state| {
            set_tooling(state, "lastCodingAgent", json!("v1"));
            Ok(())
        })
        .expect("create the pair through the entry point");
        p.foreign_write("lastCodingAgent", json!("v2"));
        check("c", &p, &p.journal, &mut failures);

        // (d) two journal directories, neither consulted.
        let p = migrated();
        let other_journal = p.tmp.path().join("journal-b");
        std::fs::create_dir_all(&other_journal).expect("second journal dir");
        p.foreign_write("lastCodingAgent", json!("v2"));
        check("d, first journal", &p, &p.journal, &mut failures);
        p.seed_state(marked(json!({ "lastCodingAgent": "v1" })));
        p.foreign_write("lastCodingAgent", json!("v2"));
        check("d, second journal", &p, &other_journal, &mut failures);

        // (e) the journal write forced to fail: its directory is a file.
        let p = migrated();
        let broken_journal = p.tmp.path().join("journal-is-a-file");
        std::fs::write(&broken_journal, b"not a directory").expect("block the journal");
        p.foreign_write("lastCodingAgent", json!("v2"));
        check("e", &p, &broken_journal, &mut failures);

        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    // -- E1c -----------------------------------------------------------------

    #[test]
    fn a_first_write_that_also_mutates_keeps_its_new_value() {
        fn leg(name: &str, p: &Pair, new: &str, failures: &mut Vec<String>) {
            let mut child = spawn_pair_child(
                p,
                &[CALLER_STATE],
                &json!({ "state": { "lastCodingAgent": new } }),
                None,
            );
            child.kill_at(CALLER_STATE);
            let read = p.read("lastCodingAgent");
            if read != json!(new) {
                failures.push(format!(
                    "({name}) after the kill a read must return {new}, got {read}"
                ));
            }
            p.write(no_change).expect("the parent's write");
            let state = p.stored_state()["tooling"]["lastCodingAgent"].clone();
            if state != json!(new) {
                failures.push(format!(
                    "({name}) the surviving value must be {new}, the state file holds {state}"
                ));
            }
        }
        let mut failures = Vec::new();

        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v_old" } }));
        leg("migrate and mutate", &p, "v_new", &mut failures);

        // (c') the tracked file is still dirty from an earlier crash.
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v0" } }));
        p.seed_state(json!({ "tooling": { "lastCodingAgent": "v0" } }));
        leg("dirty tracked file", &p, "v1", &mut failures);

        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    // -- E1d -----------------------------------------------------------------

    #[test]
    fn a_foreign_value_and_a_new_value_both_survive_their_own_sequence() {
        let foreign = || {
            let p = pair();
            p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v2" } }));
            p.seed_state(marked(json!({ "lastCodingAgent": "v1" })));
            p
        };
        let v3 = json!({ "state": { "lastCodingAgent": "v3" } });

        // Killed after our own write: the survivor is v3.
        let p = foreign();
        let stages = spawn_pair_child(&p, &[CALLER_STATE], &v3, None).kill_at(CALLER_STATE);
        assert_eq!(stages, [CLEANUP_STATE, CLEANUP_TRACKED, CALLER_STATE]);
        assert_eq!(p.read("lastCodingAgent"), json!("v3"));
        p.write(no_change).expect("re-run");
        assert_eq!(p.stored_state()["tooling"]["lastCodingAgent"], json!("v3"));

        // Killed inside the cleanup: the survivor is v2, not yet absorbed. This
        // is the preservation window: marker present beside a tracked state
        // key, and both files agree.
        let p = foreign();
        let stages = spawn_pair_child(&p, &[CLEANUP_STATE], &v3, None).kill_at(CLEANUP_STATE);
        assert_eq!(stages, [CLEANUP_STATE]);
        let state = p.stored_state();
        assert_eq!(state["tooling"]["lastCodingAgent"], json!("v2"));
        assert_eq!(
            state["split"],
            split_marker(),
            "the marker is never removed"
        );
        assert_eq!(p.tracked()["tooling"]["lastCodingAgent"], json!("v2"));
        assert_eq!(p.read("lastCodingAgent"), json!("v2"));
        p.write(no_change).expect("re-run");
        let state = p.stored_state();
        assert_eq!(state["tooling"]["lastCodingAgent"], json!("v2"));
        assert_eq!(state["split"], split_marker());
        assert!(state_keys_in(&p.tracked()).is_empty());

        // A state-only mutation: the decisions map does not change, so its
        // stage never fires and the child ends by itself.
        let p = pair();
        p.seed_decisions(json!({ "tooling": {} }));
        p.seed_state(marked(json!({ "lastCodingAgent": "v1" })));
        let mut child = spawn_pair_child(&p, &[CALLER_STATE, CALLER_DECISIONS], &v3, None);
        assert_eq!(
            child.first_ready(&[CALLER_STATE, CALLER_DECISIONS], HOOK_BOUND),
            Some(CALLER_STATE)
        );
        child.release(CALLER_STATE);
        assert!(child.exit().success(), "the state-only write must finish");
        assert_eq!(child.stages(), [CALLER_STATE]);
        assert!(!child.ready(CALLER_DECISIONS));
        assert_eq!(p.read("lastCodingAgent"), json!("v3"));
    }

    // -- E1f -----------------------------------------------------------------

    #[test]
    fn nothing_lands_between_the_cleanup_and_the_caller_sequence() {
        const LOCK_TIMEOUT: Duration = Duration::from_secs(60);
        const DISPATCH_BOUND: Duration = Duration::from_secs(2);
        const JOIN_BOUND: Duration = Duration::from_secs(5);

        fn join_within<T>(
            handle: std::thread::JoinHandle<T>,
            bound: Duration,
        ) -> Result<T, String> {
            let started = Instant::now();
            while !handle.is_finished() {
                if started.elapsed() >= bound {
                    return Err(format!("the thread did not finish within {bound:?}"));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            handle.join().map_err(|_| "the thread panicked".to_string())
        }

        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v1" } }));
        let sequence = p.tmp.path().join("sequence.log");
        let mut child = spawn_pair_child(
            &p,
            &[CLEANUP_TRACKED, MUTANT_GAP, CALLER_STATE],
            &json!({ "state": { "lastCodingAgent": "v3" } }),
            Some(&sequence),
        );

        // (Xt) abort: no more evidence is wanted, so take the one order that
        // cannot block. Both releases, kill and reap the child, then `go`, then
        // a bounded join.
        type Contender = std::thread::JoinHandle<Result<(), String>>;
        let abort = |child: &mut PairChild,
                     contender: Option<(std::sync::mpsc::Sender<()>, Contender)>,
                     why: String|
         -> ! {
            for stage in [CLEANUP_TRACKED, MUTANT_GAP, CALLER_STATE] {
                child.release(stage);
            }
            let last_stage = child.stages().last().cloned();
            let child_status = child.kill();
            let thread_status = contender.map(|(go, handle)| {
                let _ = go.send(());
                join_within(handle, JOIN_BOUND)
            });
            panic!(
                "{why}\nsequence: {:?}\nlast stage: {last_stage:?}\nchild: {child_status}\nthread: {thread_status:?}",
                lines_of(&sequence)
            );
        };

        // (1) the child pauses inside the cleanup with the guard held.
        if child.first_ready(&[CLEANUP_TRACKED], HOOK_BOUND) != Some(CLEANUP_TRACKED) {
            abort(
                &mut child,
                None,
                "the child never paused in the cleanup".into(),
            );
        }

        // (2) the competing writer, a pre-C binary: it takes the decisions
        // sidecar, then waits at `go` still holding it.
        let (go, go_gate) = std::sync::mpsc::channel::<()>();
        let lock_path = std::fs::canonicalize(&p.dir)
            .expect("canonical agent dir")
            .join(".config.json.lock");
        let (decisions_path, thread_sequence) = (p.decisions(), sequence.clone());
        let handle: Contender = std::thread::spawn(move || {
            append_line(&thread_sequence, "parent:acquiring");
            let _lock = crate::config::local_config_io::acquire_sidecar_write_lock(
                &lock_path,
                LOCK_TIMEOUT,
                "contenderLockTimeout",
                "contending config write lock",
            )?;
            append_line(&thread_sequence, "parent:acquired");
            go_gate
                .recv_timeout(HOOK_BOUND)
                .map_err(|e| format!("go was never released: {e}"))?;
            let mut tracked = get_json(&decisions_path);
            set_tooling(
                tracked.as_object_mut().expect("object"),
                "lastCodingAgent",
                json!("v2"),
            );
            put_json(&decisions_path, &tracked);
            append_line(&thread_sequence, "parent:published v2");
            Ok(())
        });
        let has = |line: &str| lines_of(&sequence).iter().any(|l| l == line);
        let started = Instant::now();
        while !has("parent:acquiring") {
            if started.elapsed() >= JOIN_BOUND {
                abort(
                    &mut child,
                    Some((go, handle)),
                    "the thread never started".into(),
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        // (3) secondary check: the lock is not taken while the cleanup pause
        // holds. It passes on both builds.
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(500) {
            if has("parent:acquired") {
                abort(
                    &mut child,
                    Some((go, handle)),
                    "the lock was taken while the child held it in the cleanup".into(),
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        // (4) the whole branch dispatch: which stage the child reports next.
        child.release(CLEANUP_TRACKED);
        match child.first_ready(&[CALLER_STATE, MUTANT_GAP], DISPATCH_BOUND) {
            // (5t) the child owns the lock: child first, thread second.
            Some(CALLER_STATE) => {
                child.kill();
                let _ = go.send(());
                match join_within(handle, HOOK_BOUND) {
                    Ok(Ok(())) => {}
                    other => panic!("the contender failed: {other:?}"),
                }
            }
            // (6t) the thread owns the lock: thread first, child second.
            Some(MUTANT_GAP) => {
                let _ = go.send(());
                match join_within(handle, HOOK_BOUND) {
                    Ok(Ok(())) => {}
                    other => abort(&mut child, None, format!("the contender failed: {other:?}")),
                }
                child.release(MUTANT_GAP);
                if child.first_ready(&[CALLER_STATE], DISPATCH_BOUND) != Some(CALLER_STATE) {
                    abort(
                        &mut child,
                        None,
                        "the child never published after the gap".into(),
                    );
                }
                child.kill();
            }
            other => abort(
                &mut child,
                Some((go, handle)),
                format!("no stage within 2 s of the cleanup release: {other:?}"),
            ),
        }

        // The blocking assertion, on the final marker file.
        let lines = lines_of(&sequence);
        let at = |line: &str| {
            lines
                .iter()
                .position(|l| l == line)
                .unwrap_or_else(|| panic!("{line} is missing from {lines:?}"))
        };
        assert!(
            at("parent:acquired") > at("child:published v3"),
            "parent:acquired must not appear before child:published v3: {lines:?}"
        );
        assert_eq!(
            lines,
            [
                "child:paused",
                "parent:acquiring",
                "child:published v3",
                "parent:acquired",
                "parent:published v2"
            ]
        );

        // End state: the tracked file holds v2, the newer write, and it wins.
        assert_eq!(p.tracked()["tooling"]["lastCodingAgent"], json!("v2"));
        p.write(no_change).expect("reconcile");
        assert_eq!(p.stored_state()["tooling"]["lastCodingAgent"], json!("v2"));
        assert!(state_keys_in(&p.tracked()).is_empty());
    }

    // -- E1e -----------------------------------------------------------------

    #[test]
    fn a_post_migration_write_survives_a_kill() {
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "profile": "old" } }));
        p.seed_state(marked(json!({ "lastCodingAgent": "v_old" })));
        let mut child = spawn_pair_child(
            &p,
            &[CALLER_DECISIONS],
            &json!({
                "state": { "lastCodingAgent": "v_new" },
                "decisions": { "profile": "new" }
            }),
            None,
        );
        child.kill_at(CALLER_DECISIONS);

        let state = p.stored_state()["tooling"]["lastCodingAgent"].clone();
        let profile = p.tracked()["tooling"]["profile"].clone();
        assert_eq!(
            state,
            json!("v_new"),
            "the state value was lost: the state file holds {state} while the decisions file \
             already holds profile {profile}"
        );
        assert_eq!(profile, json!("new"));
        assert!(state_keys_in(&p.tracked()).is_empty());
        assert_eq!(p.read("lastCodingAgent"), json!("v_new"));
    }

    // -- E2 ------------------------------------------------------------------

    #[test]
    fn the_state_file_wins_when_there_is_no_marker() {
        // First leg, in this process. The marker is stamped by the caller's
        // sequence, so it is not on disk when the cleanup has finished.
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "A" } }));
        p.seed_state(json!({ "tooling": { "lastCodingAgent": "B" } }));
        let marker_after_cleanup = std::cell::RefCell::new(None);
        let on_stage = |stage: &str| {
            if stage == CLEANUP_TRACKED {
                *marker_after_cleanup.borrow_mut() = Some(p.stored_state().get("split").cloned());
            }
        };
        update_agent_config_with_stage(&p.decisions(), Some(&p.journal), &on_stage, no_change)
            .expect("migrate a conflict");
        let state = p.stored_state();
        assert_eq!(
            state["tooling"]["lastCodingAgent"],
            json!("B"),
            "with no marker the state file's value must survive"
        );
        assert!(state_keys_in(&p.tracked()).is_empty());
        assert_eq!(
            state["split"],
            split_marker(),
            "the caller's sequence stamps it"
        );

        // Second leg, E2 crossed with E4: three kills, three runs. The second
        // state key, held only by the tracked file, makes the state side
        // change, so every named stage is reachable. Both results of a kill
        // are collected before the row fails, so a failing build still shows
        // the read and the re-run.
        let mut failures = Vec::new();
        for kill in [CLEANUP_STATE, CLEANUP_TRACKED, CALLER_STATE] {
            // The third kill is control-only: a build that has already lost
            // the value may never fire that stage, so it is not awaited.
            if kill == CALLER_STATE && !failures.is_empty() {
                break;
            }
            let p = pair();
            p.seed_decisions(json!({ "tooling": {
                "lastCodingAgent": "A",
                "codingAgents": { "claude": { "app": "Claude Code", "lastUsed": T1 } }
            } }));
            p.seed_state(json!({ "tooling": { "lastCodingAgent": "B" } }));
            spawn_pair_child(&p, &[kill], &json!({}), None).kill_at(kill);
            let read = p.read("lastCodingAgent");
            if read != json!("B") {
                failures.push(format!(
                    "killed at {kill}: a read must return B, got {read}"
                ));
            }
            p.write(no_change).expect("re-run");
            let state = p.stored_state()["tooling"]["lastCodingAgent"].clone();
            if state != json!("B") {
                failures.push(format!(
                    "killed at {kill}: the re-run must leave B in the state file, it holds {state}"
                ));
            }
            println!("E2 killed at {kill}: read {read}, state after the re-run {state}");
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));

        assert_eq!(
            marker_after_cleanup.into_inner(),
            Some(None),
            "the cleanup's publish must not carry the marker"
        );
    }

    // -- E3 ------------------------------------------------------------------

    #[test]
    fn a_second_migration_writes_nothing() {
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude", "profile": "keep" } }));
        p.write(no_change).expect("migrate");
        let journal = p.journal_bytes();
        assert!(
            String::from_utf8_lossy(journal.as_deref().expect("the audit note"))
                .contains("lastCodingAgent"),
            "the first run records the keys it moved"
        );
        let before = (bytes_of(&p.decisions()), bytes_of(&p.state()));
        p.write(no_change).expect("second run");
        assert_eq!(
            (bytes_of(&p.decisions()), bytes_of(&p.state())),
            before,
            "a second run must write neither config file"
        );
        assert_eq!(p.journal_bytes(), journal, "the journal must not grow");

        // Rule 6: a decision-only write creates no state file.
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "profile": "old" } }));
        p.write(|decisions, _state| {
            set_tooling(decisions, "profile", json!("new"));
            Ok(())
        })
        .expect("decision-only write");
        assert_eq!(p.tracked()["tooling"]["profile"], json!("new"));
        assert!(
            !p.state().exists(),
            "a decision-only write must create no state file"
        );
        assert_eq!(p.journal_bytes(), None, "nothing moved, so no note");
    }

    // -- E4 ------------------------------------------------------------------

    #[test]
    fn an_interrupted_move_leaves_the_value_in_both_files() {
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude" } }));
        // Killed at whichever cleanup stage fires first, between the two writes.
        let mut child = spawn_pair_child(&p, &[CLEANUP_STATE, CLEANUP_TRACKED], &json!({}), None);
        let stage = child.first_ready(&[CLEANUP_STATE, CLEANUP_TRACKED], HOOK_BOUND);
        let status = child.kill();
        assert!(
            stage.is_some(),
            "the child never reached the cleanup: {status}"
        );

        let tracked = p.tracked()["tooling"]["lastCodingAgent"].clone();
        let state = bytes_of(&p.state())
            .map(|_| p.stored_state()["tooling"]["lastCodingAgent"].clone())
            .unwrap_or(Value::Null);
        assert_eq!(
            (&tracked, &state),
            (&json!("claude"), &json!("claude")),
            "after the kill at {stage:?} the value must be in both files: tracked {tracked}, state {state}"
        );

        p.write(no_change).expect("re-run");
        assert_eq!(
            p.stored_state()["tooling"]["lastCodingAgent"],
            json!("claude")
        );
        assert!(
            state_keys_in(&p.tracked()).is_empty(),
            "after the re-run the value is only in the state file"
        );
    }

    // -- E5 ------------------------------------------------------------------

    #[test]
    fn a_refusal_keeps_every_value() {
        let unmigrated = || {
            let p = pair();
            p.seed_decisions(
                json!({ "tooling": { "lastCodingAgent": "v_old", "profile": "old" } }),
            );
            p
        };
        let both = |p: &Pair| (bytes_of(&p.decisions()), bytes_of(&p.state()));
        let mutate_both = |decisions: &mut JsonMap, state: &mut JsonMap| {
            set_tooling(state, "lastCodingAgent", json!("v_new"));
            set_tooling(decisions, "profile", json!("new"));
            Ok(())
        };

        // Leg 1, lock refusal: the one five-second wait of the battery.
        let p = unmigrated();
        let before = both(&p);
        let held = crate::config::local_config_io::acquire_sidecar_write_lock(
            &std::fs::canonicalize(&p.dir)
                .expect("canonical agent dir")
                .join(".config.json.lock"),
            Duration::from_secs(1),
            "testLockTimeout",
            "test lock",
        )
        .expect("hold the decisions sidecar");
        let err = p.write(no_change).expect_err("the lock refusal");
        drop(held);
        assert!(err.contains("configLockTimeout"), "{err}");
        assert_eq!(both(&p), before, "lock refusal: both files byte-identical");
        assert_eq!(p.read("lastCodingAgent"), json!("v_old"));

        // Leg 2, Io refusal inside the cleanup: the decisions publish fails
        // after the state write succeeded.
        let p = unmigrated();
        let tracked = bytes_of(&p.decisions());
        std::fs::create_dir(p.decisions_temp()).expect("block the decisions publish");
        p.write(no_change).expect_err("the Io refusal");
        assert_eq!(bytes_of(&p.decisions()), tracked, "Io: decisions unchanged");
        assert_eq!(
            p.stored_state()["tooling"]["lastCodingAgent"],
            json!("v_old"),
            "Io: the state file carries the copied key"
        );
        assert_eq!(p.read("lastCodingAgent"), json!("v_old"));
        std::fs::remove_dir(p.decisions_temp()).expect("unblock");
        p.write(no_change).expect("the next run converges");
        assert_eq!(
            p.stored_state()["tooling"]["lastCodingAgent"],
            json!("v_old")
        );
        assert_eq!(p.stored_state()["split"], split_marker());
        assert!(state_keys_in(&p.tracked()).is_empty());

        // Leg 3: a tracked `tooling` that is not an object is never repaired.
        let p = pair();
        p.seed_decisions(json!({ "tooling": 7 }));
        p.seed_state(json!({ "tooling": { "lastCodingAgent": "v_old" } }));
        let before = both(&p);
        let err = p.write(no_change).expect_err("a malformed tooling");
        assert!(err.contains("'tooling' must be a JSON object"), "{err}");
        assert_eq!(both(&p), before, "tooling: both files byte-identical");

        // Leg 4, the caller's partial publish: its state publish succeeds and
        // its decisions publish fails. The marker is present on the `Err`.
        let p = unmigrated();
        let block = |stage: &str| {
            if stage == CLEANUP_TRACKED {
                std::fs::create_dir(p.decisions_temp()).expect("block the decisions publish");
            }
        };
        update_agent_config_with_stage(&p.decisions(), Some(&p.journal), &block, mutate_both)
            .expect_err("the caller's decisions publish");
        let state = p.stored_state();
        assert_eq!(state["tooling"]["lastCodingAgent"], json!("v_new"));
        assert_eq!(
            state["split"],
            split_marker(),
            "partial: the marker is present"
        );
        assert_eq!(
            p.tracked(),
            json!({ "tooling": { "profile": "old" } }),
            "partial: no state key in the tracked file, and the decision is at its old value"
        );
        assert_eq!(p.read("lastCodingAgent"), json!("v_new"));
        std::fs::remove_dir(p.decisions_temp()).expect("unblock");

        // Leg 5, the caller's state publish fails: nothing of its sequence
        // reaches disk.
        let p = unmigrated();
        let block = |stage: &str| {
            if stage == CLEANUP_TRACKED {
                std::fs::create_dir(p.state_temp()).expect("block the state publish");
            }
        };
        update_agent_config_with_stage(&p.decisions(), Some(&p.journal), &block, mutate_both)
            .expect_err("the caller's state publish");
        assert_eq!(
            p.stored_state(),
            json!({ "tooling": { "lastCodingAgent": "v_old" } }),
            "state publish: the cleanup's value and no marker"
        );
        assert_eq!(p.tracked(), json!({ "tooling": { "profile": "old" } }));
        assert_eq!(p.read("lastCodingAgent"), json!("v_old"));
        std::fs::remove_dir(p.state_temp()).expect("unblock");
        p.write(no_change).expect("the next call");
        assert_eq!(
            p.stored_state()["split"],
            split_marker(),
            "the next call stamps it"
        );

        // Variant: `mutate` fails on a pair that already carries the marker.
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "profile": "old" } }));
        p.seed_state(marked(json!({ "lastCodingAgent": "v_old" })));
        let before = both(&p);
        let err = p
            .write(|_, _| Err("the caller refused".to_string()))
            .expect_err("a failing mutate");
        assert_eq!(err, "the caller refused");
        assert_eq!(both(&p), before, "mutate: both files byte-identical");
        assert_eq!(p.stored_state()["split"], split_marker());
        assert_eq!(p.read("lastCodingAgent"), json!("v_old"));
    }

    // -- E5b -----------------------------------------------------------------

    #[test]
    fn a_read_only_tree_is_not_modified() {
        let p = pair();
        let all: serde_json::Map<String, Value> = sample_state_values()
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect();
        p.seed_decisions(json!({ "tooling": all }));
        let before = tree_of(p.tmp.path());

        for (key, value) in sample_state_values() {
            assert_eq!(p.read(key), value, "{key} must come back");
        }
        let typed = read_agent_local_config(&p.dir).expect("typed read");
        assert_eq!(typed.tooling.last_coding_agent.as_deref(), Some("claude"));

        let after = tree_of(p.tmp.path());
        assert_eq!(
            after.keys().collect::<Vec<_>>(),
            before.keys().collect::<Vec<_>>(),
            "a read must create or remove no file"
        );
        assert!(after == before, "a read must modify no file");
    }

    // -- E5c -----------------------------------------------------------------

    #[test]
    fn a_journal_failure_does_not_change_the_outcome() {
        let migrated = |p: &Pair| {
            let state = p.stored_state();
            assert_eq!(state["tooling"]["lastCodingAgent"], json!("claude"));
            assert_eq!(state["split"], split_marker());
            assert!(state_keys_in(&p.tracked()).is_empty());
        };

        // The journal directory is a file, so the note cannot be written.
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude" } }));
        let broken_journal = p.tmp.path().join("journal-is-a-file");
        std::fs::write(&broken_journal, b"not a directory").expect("block the journal");
        let result = update_agent_config_in(&p.decisions(), Some(&broken_journal), no_change);
        assert_eq!(
            result,
            Ok(()),
            "a lost audit note must not fail a completed write"
        );
        migrated(&p);
        assert_eq!(
            std::fs::read(&broken_journal).expect("the blocking file"),
            b"not a directory"
        );

        // No journal directory: same outcome, no note attempted.
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude" } }));
        assert_eq!(
            update_agent_config_in(&p.decisions(), None, no_change),
            Ok(())
        );
        migrated(&p);
        assert_eq!(p.journal_bytes(), None);
    }

    // -- E7 ------------------------------------------------------------------

    fn ignore_rows_present(ac_root: &Path) -> bool {
        let content = std::fs::read_to_string(ac_root.join(".gitignore")).unwrap_or_default();
        crate::config::naming_migration::config_state_ignore_rows()
            .iter()
            .all(|(pattern, _)| content.lines().any(|line| line == pattern))
    }

    #[test]
    fn the_sweep_runs_before_the_first_write() {
        // A `.gitignore` the sweep cannot read or write: a directory.
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude" } }));
        let gitignore = p.tmp.path().join(".ac").join(".gitignore");
        std::fs::create_dir(&gitignore).expect("block the .gitignore");
        let tracked = bytes_of(&p.decisions());
        let err = p.write(no_change).expect_err("a failed sweep");
        assert!(err.contains(&gitignore.display().to_string()), "{err}");
        assert!(
            !p.state().exists(),
            "a failed sweep must leave no unignored state file on disk"
        );
        assert_eq!(
            bytes_of(&p.decisions()),
            tracked,
            "decisions file unchanged"
        );

        // No `.gitignore` and a decision-only write: the file is created.
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "profile": "old" } }));
        p.write(|decisions, _state| {
            set_tooling(decisions, "profile", json!("new"));
            Ok(())
        })
        .expect("decision-only write");
        assert!(ignore_rows_present(&p.tmp.path().join(".ac")));
        assert!(!p.state().exists());

        // No `.ac` ancestor: the call succeeds and creates no `.gitignore`.
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("agent");
        std::fs::create_dir_all(&dir).expect("agent dir");
        put_json(
            &dir.join("config.json"),
            &json!({ "tooling": { "lastCodingAgent": "claude" } }),
        );
        update_agent_config_in(&dir.join("config.json"), None, no_change).expect("no .ac");
        assert_eq!(
            get_json(&state_file_path(&dir))["tooling"]["lastCodingAgent"],
            json!("claude")
        );
        assert!(
            !tree_of(tmp.path())
                .keys()
                .any(|path| path.file_name().is_some_and(|name| name == ".gitignore")),
            "no .gitignore anywhere"
        );
    }

    /// 3.2 step 1 - the nearest `.ac` ancestor, for each path shape.
    #[test]
    fn the_sweep_finds_the_ac_root_for_every_path_shape() {
        let instance = crate::config::agent_local_dir_name();
        let shapes = [
            ("Matrix", PathBuf::from("_agent_a")),
            ("per-instance", Path::new("_agent_a").join(&instance)),
            ("replica", Path::new("room-1").join("__agent_b")),
        ];
        for (shape, relative) in shapes {
            let tmp = tempfile::tempdir().expect("tempdir");
            let ac_root = tmp.path().join(".ac");
            let dir = ac_root.join(relative);
            std::fs::create_dir_all(&dir).expect("agent dir");
            assert_eq!(
                nearest_ac_root(&dir.join("config.json")),
                Some(ac_root.as_path()),
                "{shape}"
            );
            update_agent_config_in(&dir.join("config.json"), None, no_change).expect(shape);
            assert!(ignore_rows_present(&ac_root), "{shape}: rows appended");
            // A second call adds nothing.
            let content = std::fs::read(ac_root.join(".gitignore")).expect("read");
            update_agent_config_in(&dir.join("config.json"), None, no_change).expect(shape);
            assert_eq!(
                std::fs::read(ac_root.join(".gitignore")).expect("read"),
                content
            );
        }
        assert_eq!(nearest_ac_root(Path::new("agent/config.json")), None);
    }

    // -- #2816 (C3): the writers reach the pair -------------------------------

    /// E1.
    #[test]
    fn the_stamp_lands_in_the_state_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        seed_instance_config(dir, &json!({ "repos": ["repo-a"] }));
        let config = instance_config(dir);

        assert_eq!(set(dir, T2), Ok(true));
        assert_eq!(
            state_beside(&config)["tooling"]["lastAgentMessageAt"],
            json!(T2)
        );
        assert_eq!(
            stored(dir),
            json!({ "repos": ["repo-a"] }),
            "the decisions file holds no stamp"
        );

        // A stored newer value is still kept.
        assert_eq!(set(dir, T1), Ok(false));
        assert_eq!(
            state_beside(&config)["tooling"]["lastAgentMessageAt"],
            json!(T2)
        );
        assert_no_state_key_tracked(&config);
    }

    /// E2.
    #[test]
    fn the_last_agent_and_its_map_land_in_the_state_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        seed_instance_config(
            dir,
            &json!({
                "repos": ["repo-a"],
                "tooling": { "codingAgents": { "claude": {
                    "app": "Old", "futureKey": { "x": 1 }
                } } }
            }),
        );
        let config = instance_config(dir);
        upsert_config(
            &config,
            "claude",
            &fixed_entry("claude", &[("A", "aa")]),
            true,
        )
        .expect("upsert");

        let state = state_beside(&config);
        assert_eq!(state["tooling"]["lastCodingAgent"], json!("claude"));
        let entry = &state["tooling"]["codingAgents"]["claude"];
        assert_eq!(entry["app"], json!("Claude Code"));
        assert_eq!(
            entry["futureKey"],
            json!({ "x": 1 }),
            "#2433: an unknown key inside the entry survives"
        );
        assert_no_state_key_tracked(&config);
        assert_eq!(stored(dir)["repos"], json!(["repo-a"]));
    }

    /// E2b.
    #[test]
    fn upsert_config_migrates_and_keeps_its_new_value() {
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v_old", "profile": "B" } }));
        assert!(bytes_of(&p.state()).is_none(), "no state file yet");

        upsert_config(&p.decisions(), "v_new", &codex_entry(), true).expect("upsert");

        let state = p.stored_state();
        assert_eq!(state["tooling"]["lastCodingAgent"], json!("v_new"));
        assert_eq!(state["split"], split_marker());
        assert!(
            state_keys_in(&p.tracked()).is_empty(),
            "no state key stays tracked"
        );
        assert_eq!(p.tracked()["tooling"]["profile"], json!("B"));
    }

    /// E2c.
    #[test]
    fn a_state_only_write_creates_no_tracked_file_and_still_reads_back() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        upsert_config(&dir.join("config.json"), "codex", &codex_entry(), true).expect("upsert");

        let state_name = crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME;
        let names: Vec<String> = dir_bytes(dir)
            .into_iter()
            .map(|(name, _)| name.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            names,
            [
                ".config.json.lock".to_string(),
                format!(".{state_name}.lock"),
                state_name.to_string(),
            ],
            "only the state file and the two lock sidecars exist"
        );
        assert!(!dir.join("config.json").exists());

        let read = read_agent_local_config_json(dir)
            .expect("parseable")
            .expect("the state file alone answers");
        assert_eq!(read["tooling"]["lastCodingAgent"], json!("codex"));
    }

    /// E3c.
    #[test]
    fn an_absent_file_waits_for_a_concurrent_creator() {
        use std::sync::mpsc::channel;
        use std::time::Duration;

        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("__agent_a");
        std::fs::create_dir_all(&dir).expect("replica dir");
        let config = dir.join("config.json");

        // The creator's position: the decisions sidecar is held and
        // `config.json` is still absent.
        let lock_path = std::fs::canonicalize(&dir)
            .expect("canonical dir")
            .join(".config.json.lock");
        let held = crate::config::local_config_io::acquire_sidecar_write_lock(
            &lock_path,
            Duration::from_secs(5),
            "configLockTimeout",
            "local config write lock",
        )
        .expect("hold the decisions sidecar");

        let (about_to_call, started) = channel();
        let (finished, done) = channel();
        let worker_config = config.clone();
        let worker = std::thread::spawn(move || {
            about_to_call.send(()).expect("signal the start");
            let result = update_existing_agent_config(&worker_config, |decisions, _| {
                decisions.insert("seen".to_string(), json!(true));
                Ok(())
            });
            let _ = finished.send(());
            result
        });
        started
            .recv_timeout(Duration::from_secs(5))
            .expect("the worker starts");

        // The bounded wait: the worker must still be blocked on the lock.
        if done.recv_timeout(Duration::from_millis(750)).is_ok() {
            let result = worker.join().expect("the worker thread panicked");
            panic!("the worker completed while the creator still held the lock: {result:?}");
        }

        put_json(&config, &json!({ "identity": "created" }));
        drop(held);

        worker
            .join()
            .expect("the worker thread panicked")
            .expect("a creator mid-publish is waited for, not refused");
        assert_eq!(
            get_json(&config),
            json!({ "identity": "created", "seen": true })
        );
    }

    /// E4: sites 8, 13, 9 and 10. Sites 5 and 11 are driven in their own
    /// modules.
    #[test]
    fn the_unchanged_writers_still_write_the_decisions_file() {
        use crate::config::coding_agent_profiles as profiles;

        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let ac_root = project.join(".ac");
        let matrix = ac_root.join("_agent_dev-rust");
        let replica = ac_root.join("wg-7-dev-team").join("__agent_dev-rust");
        std::fs::create_dir_all(&matrix).expect("matrix dir");
        std::fs::create_dir_all(&replica).expect("replica dir");
        std::fs::write(matrix.join("Role.md"), "# Role\n").expect("role");
        let no_state_file = |dir: &Path, site: &str| {
            assert!(
                !state_file_path(dir).exists(),
                "{site} created a state file"
            );
        };

        // Site 8.
        let default = profiles::ReplicaSelectionDefault {
            coding_agent_id: "codex".to_string(),
            requested_profile: "B".to_string(),
            selection_locked: false,
        };
        profiles::write_replica_selection_default(&matrix, &default, None).expect("site 8");
        assert_eq!(
            get_json(&matrix.join("config.json"))["tooling"]["replicaSelectionDefault"],
            json!({ "codingAgentId": "codex", "requestedProfile": "B", "selectionLocked": false })
        );
        no_state_file(&matrix, "site 8");

        // Site 13.
        let root = tmp.path().join("root");
        std::fs::create_dir_all(&root).expect("root dir");
        crate::config::root_agent::merge_root_agent_config(&root.join("config.json"))
            .expect("site 13");
        let merged = get_json(&root.join("config.json"));
        assert_eq!(merged["tooling"], json!({}));
        assert!(merged["context"].is_array(), "{merged}");
        no_state_file(&root, "site 13");

        // Sites 9 and 10, on an existing file.
        let replica_config = replica.join("config.json");
        put_json(
            &replica_config,
            &json!({ "identity": "../../_agent_dev-rust", "tooling": {} }),
        );
        let settings = crate::config::settings::AppSettings {
            project_paths: vec![project.to_string_lossy().to_string()],
            ..Default::default()
        };
        let expected = profiles::read_replica_selection_state(&replica)
            .expectation()
            .expect("a valid unlocked replica");
        let selection = profiles::ReplicaSelectionPair {
            coding_agent_id: "codex".to_string(),
            requested_profile: "B".to_string(),
        };
        profiles::write_replica_selection(
            &settings,
            &replica,
            &selection,
            profiles::SelectionWriteIntent::IndividualAssignLock,
            &expected,
        )
        .expect("site 9");
        let tooling = get_json(&replica_config)["tooling"].clone();
        assert_eq!(tooling["currentCodingAgent"], json!("codex"));
        assert_eq!(tooling["profile"], json!("B"));
        assert_eq!(tooling["selectionLocked"], json!(true));
        no_state_file(&replica, "site 9");

        let expected = profiles::read_replica_selection_state(&replica)
            .expectation()
            .expect("a valid locked replica");
        profiles::clear_replica_selection_lock(&settings, &replica, &expected).expect("site 10");
        assert_eq!(
            get_json(&replica_config)["tooling"]["selectionLocked"],
            json!(false)
        );
        no_state_file(&replica, "site 10");
    }

    /// E4b.
    #[test]
    fn a_decision_only_write_migrates_the_state_keys() {
        let p = pair();
        p.seed_decisions(json!({ "tooling": all_four("t"), "context": [] }));
        let _journal = journal_redirect::set(&p.journal);

        crate::config::root_agent::merge_root_agent_config(&p.decisions()).expect("merge");

        assert!(
            state_keys_in(&p.tracked()).is_empty(),
            "the four keys left the tracked file"
        );
        let state = p.stored_state();
        assert_eq!(state["tooling"], all_four("t"));
        assert_eq!(state["split"], split_marker());
        let journal = p
            .journal_bytes()
            .expect("the note is recorded in the directory the guard names");
        assert!(
            String::from_utf8_lossy(&journal).contains("moved to the state file"),
            "the journal carries the move note"
        );
    }

    /// E4c.
    #[test]
    fn a_test_without_the_guard_writes_no_journal() {
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v_old" } }));

        update_agent_config(&p.decisions(), no_change).expect("a production-form call");

        assert_eq!(
            p.stored_state()["tooling"]["lastCodingAgent"],
            json!("v_old"),
            "the call migrated the key"
        );
        let journal_name = crate::config::instance_artifacts::NAMING_MIGRATION_STATE_NAME;
        let journals: Vec<_> = tree_of(p.tmp.path())
            .into_keys()
            .filter(|path| path.file_name().is_some_and(|name| name == journal_name))
            .collect();
        assert!(journals.is_empty(), "a journal was written: {journals:?}");
        assert_eq!(
            production_journal_dir(),
            None,
            "a lib test without the guard has no journal directory"
        );
    }

    /// E3b (b): the one body all four require-existing sites call.
    #[test]
    fn the_require_existing_wrapper_refuses_an_absent_file_and_creates_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("__agent_a");
        std::fs::create_dir_all(&dir).expect("replica dir");
        let config = dir.join("config.json");

        let result = update_existing_agent_config(&config, |decisions, _| {
            decisions.insert("seen".to_string(), json!(true));
            Ok(())
        });

        assert!(!config.exists(), "the wrapper created config.json");
        assert!(
            !state_file_path(&dir).exists(),
            "the wrapper created a state file"
        );
        assert_eq!(
            result,
            Err(format!("Local config {} does not exist", config.display()))
        );
    }

    /// C4 (#2470): the seeded state file of every row that seeds one. All four
    /// state keys, plus the exact D7 marker `stamp_split_marker` writes.
    #[rustfmt::skip]
    const C4_SEEDED_STATE: &str =
        r#"{"tooling":{"lastCodingAgent":"codex","codingAgents":{},"lastAgentMessageAt":"2026-01-01T00:00:00Z","profileContentHash":"h"},"split":{"v":1,"keys":["lastCodingAgent","codingAgents","lastAgentMessageAt","profileContentHash"]}}"#;

    /// C4 (#2470): the state file beside a tracked `config.json`.
    fn c4_state_path(tracked: &std::path::Path) -> std::path::PathBuf {
        tracked.with_file_name(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
    }

    /// C4 (#2470): after one writer, the tracked file carries none of the four
    /// state keys. This says nothing at all about the state file: `c4_state_is`
    /// is the only helper that reads it, and the row calls them in that order.
    fn c4_tracked_is_clean(tracked: &std::path::Path, site: &str) {
        let text = std::fs::read_to_string(tracked).expect("tracked file");
        let value: serde_json::Value = serde_json::from_str(&text).expect("tracked json");
        let tooling = value.get("tooling");
        c4_absent(tooling, "lastCodingAgent", site, &value);
        c4_absent(tooling, "codingAgents", site, &value);
        c4_absent(tooling, "lastAgentMessageAt", site, &value);
        c4_absent(tooling, "profileContentHash", site, &value);
    }

    fn c4_absent(t: Option<&serde_json::Value>, key: &str, site: &str, all: &serde_json::Value) {
        assert!(
            t.and_then(|t| t.get(key)).is_none(),
            "{site} left {key}: {all}"
        );
    }

    /// C4 (#2470): the state file EXISTS and is exactly `expected`, compared
    /// as parsed JSON so the pretty-printing of `publish_pair_side` is never
    /// the subject. `expected` is always a literal written in the row. Absence
    /// is NOT admitted here and no longer reads as JSON `null`:
    /// `c4_no_state_file` is the only helper that may conclude absence.
    fn c4_state_is(state: &std::path::Path, expected: &str, site: &str) {
        let want: serde_json::Value = serde_json::from_str(expected).expect("expected json");
        let raw = std::fs::read_to_string(state).expect("state file");
        let got: serde_json::Value = serde_json::from_str(&raw).expect("state json");
        assert_eq!(got, want, "{site} state file");
    }

    #[rustfmt::skip]
    const C4_STATE_AFTER_SITE_1: &str =
        r#"{"tooling":{"lastCodingAgent":"codex","codingAgents":{},"lastAgentMessageAt":"2026-06-01T00:00:00Z","profileContentHash":"h"},"split":{"v":1,"keys":["lastCodingAgent","codingAgents","lastAgentMessageAt","profileContentHash"]}}"#;
    #[rustfmt::skip]
    const C4_STATE_AFTER_SITE_2: &str =
        r#"{"tooling":{"lastCodingAgent":"claude","codingAgents":{"claude":{"app":"Claude Code","acSessionId":"sid","lastUsed":"2026-09-02T01:00:00+00:00","command":"claude","identity":{"A":"aa"}}},"lastAgentMessageAt":"2026-01-01T00:00:00Z","profileContentHash":"h"},"split":{"v":1,"keys":["lastCodingAgent","codingAgents","lastAgentMessageAt","profileContentHash"]}}"#;

    /// C4 (#2470) E11, sites 1 and 2: each writes the state file and leaves the
    /// tracked file without a state key.
    #[test]
    fn c4_sites_1_2_leave_no_state_key_in_the_tracked_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _journal = journal_redirect::set(tmp.path());
        let seed = json!({ "repos": ["repo-a"], "tooling": {} });

        let dir1 = tmp.path().join("site-1");
        seed_instance_config(&dir1, &seed);
        let tracked = instance_config(&dir1);
        std::fs::write(c4_state_path(&tracked), C4_SEEDED_STATE).expect("seed state");
        assert_eq!(
            set_last_agent_message_at(&dir1.to_string_lossy(), "2026-06-01T00:00:00Z"),
            Ok(true)
        );
        c4_tracked_is_clean(&tracked, "site 1");
        c4_state_is(&c4_state_path(&tracked), C4_STATE_AFTER_SITE_1, "site 1");
        assert_eq!(stored(&dir1)["repos"], json!(["repo-a"]));

        let dir2 = tmp.path().join("site-2");
        seed_instance_config(&dir2, &seed);
        let tracked = instance_config(&dir2);
        std::fs::write(c4_state_path(&tracked), C4_SEEDED_STATE).expect("seed state");
        assert_eq!(
            upsert_config(
                &tracked,
                "claude",
                &fixed_entry("claude", &[("A", "aa")]),
                true
            ),
            Ok(())
        );
        c4_tracked_is_clean(&tracked, "site 2");
        c4_state_is(&c4_state_path(&tracked), C4_STATE_AFTER_SITE_2, "site 2");
        assert_eq!(stored(&dir2)["repos"], json!(["repo-a"]));
    }
}
