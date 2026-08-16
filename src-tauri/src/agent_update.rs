//! Issue #1327 - startup coding-agent update flow.
//!
//! At GUI startup a detached task (spawned in `lib.rs` setup, BEFORE the restore
//! task is submitted) plans and runs the per-command `updateCommands` sequences
//! for every coding agent the user REGISTERED (`AppSettings.agents[].command`)
//! AND enabled through the per-command policy map
//! `AppSettings.agent_auto_update_by_command`. Commands never asked
//! about get a first-time SI/NO prompt (asked once, never again, default No).
//! Updates run in parallel ACROSS commands (each command's sequence stays
//! strictly ordered) with a splash overlay on the sidebar while they run, and a
//! sticky red error toast per failing command.
//!
//! No version detection/comparison by decision: the update command itself
//! decides whether something is new.
//!
//! `AgentUpdateGate` is the process-local blocker every session open waits on
//! (`create_session_inner_impl`, the single chokepoint). The gate is released by
//! a `FinishGuard` on EVERY exit path of the startup task (including panics), so
//! sessions never wedge and the splash never sticks.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::config::coding_agents_catalog::{load_catalog_for_settings, primary_project_root};
use crate::config::settings::SettingsState;

/// Per-prompt cap. On expiry nothing is persisted (the question is asked again
/// next boot) and the command is skipped this boot.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);
/// Per-update-command-step cap. On expiry the whole tree is killed and the step
/// is reported failed; startup continues.
const UPDATE_STEP_TIMEOUT: Duration = Duration::from_secs(300);
/// Captured output tail kept for logging (log-only, never sent to the UI).
const OUTPUT_TAIL_BYTES: usize = 8 * 1024;
/// Bounded wait for the pipe readers to finish draining.
const READER_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// One command's update outcome, shown in the sidebar as a red toast on failure.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateResult {
    pub command: String,
    pub label: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Snapshot of the whole startup run, served to a late-mounting sidebar.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateStatus {
    pub in_progress: bool,
    /// The currently registered-but-unanswered prompt (the sequential prompt
    /// phase has AT MOST one), so a late mount restores a prompt that was
    /// emitted before its listeners registered. Never resurrects a dead prompt:
    /// `drop_pending`, `resolve_answer`, and `mark_finished` all clear it.
    pub prompt: Option<AgentUpdatePrompt>,
    pub results: Vec<AgentUpdateResult>,
}

/// The pending SI/NO question for one command.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdatePrompt {
    pub command: String,
    pub label: String,
}

/// Process-local gate: blocks every session open until the startup update run
/// finishes or times out. Managed as `Arc<AgentUpdateGate>`.
pub struct AgentUpdateGate {
    state: Mutex<GateState>,
    release: tokio::sync::Notify,
}

struct GateState {
    started: bool,
    finished: bool,
    results: Vec<AgentUpdateResult>,
    /// Commands prompted this boot (answer validity + late-answer persistence).
    prompted: HashSet<String>,
    /// Registered-but-unanswered prompts, keyed by command.
    pending: HashMap<String, tokio::sync::oneshot::Sender<bool>>,
    /// Currently registered-but-unanswered prompt, for the snapshot.
    pending_prompt: Option<AgentUpdatePrompt>,
}

impl AgentUpdateGate {
    pub fn new() -> Self {
        AgentUpdateGate {
            state: Mutex::new(GateState {
                started: false,
                finished: false,
                results: Vec::new(),
                prompted: HashSet::new(),
                pending: HashMap::new(),
                pending_prompt: None,
            }),
            release: tokio::sync::Notify::new(),
        }
    }

    pub fn mark_started(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .started = true;
    }

    /// Register the prompt for `command` and return the answer channel. The
    /// command is recorded as prompted (so a late answer still persists) and
    /// becomes the snapshot's pending prompt.
    pub fn register_prompt(
        &self,
        command: &str,
        label: &str,
    ) -> tokio::sync::oneshot::Receiver<bool> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        debug_assert!(!state.pending.contains_key(command), "duplicate prompt for {command}");
        state.prompted.insert(command.to_string());
        let (tx, rx) = tokio::sync::oneshot::channel();
        state.pending.insert(command.to_string(), tx);
        state.pending_prompt = Some(AgentUpdatePrompt {
            command: command.to_string(),
            label: label.to_string(),
        });
        rx
    }

    /// Timeout path: the prompt expired without an answer. Clears the pending
    /// prompt so a snapshot cannot resurrect it.
    pub fn drop_pending(&self, command: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.pending.remove(command);
        if state
            .pending_prompt
            .as_ref()
            .is_some_and(|p| p.command == command)
        {
            state.pending_prompt = None;
        }
    }

    /// Deliver the user's answer. Returns `true` ONLY when a live receiver
    /// accepted the answer (the update runs this boot); `false` when nothing
    /// was pending (late answer) or the receiver was dropped by the prompt
    /// timeout (round-3 F1 pin: a dead receiver must never report "applied
    /// this boot").
    pub fn resolve_answer(&self, command: &str, enabled: bool) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(tx) = state.pending.remove(command) else {
            return false;
        };
        if state
            .pending_prompt
            .as_ref()
            .is_some_and(|p| p.command == command)
        {
            state.pending_prompt = None;
        }
        tx.send(enabled).is_ok()
    }

    pub fn was_prompted(&self, command: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .prompted
            .contains(command)
    }

    /// Release the gate. IDEMPOTENT: once `finished` is set it stays set and a
    /// repeated call only overwrites `results`. FIRST clears the pending prompt
    /// and all pending oneshot senders (in the same lock section), so a
    /// post-panic snapshot never resurrects a dead prompt that no event is left
    /// to clear; THEN sets finished + results and wakes the waiters.
    pub fn mark_finished(&self, results: Vec<AgentUpdateResult>) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.pending.clear();
            state.pending_prompt = None;
            state.finished = true;
            state.results = results;
        }
        self.release.notify_waiters();
    }

    pub fn snapshot(&self) -> AgentUpdateStatus {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        AgentUpdateStatus {
            in_progress: state.started && !state.finished,
            prompt: state.pending_prompt.clone(),
            results: state.results.clone(),
        }
    }

    /// Await gate release. Race-free for late waiters: the notification is
    /// registered BEFORE the finished check, so a `mark_finished` between the
    /// two cannot be missed.
    pub async fn wait_until_done(&self) {
        loop {
            let notified = self.release.notified();
            if self.state.lock().unwrap_or_else(|e| e.into_inner()).finished {
                return;
            }
            notified.await;
        }
    }
}

impl Default for AgentUpdateGate {
    fn default() -> Self {
        Self::new()
    }
}

/// One command's update run: the shell sequences to execute and where.
#[derive(Debug, Clone)]
pub struct UpdateTarget {
    pub command: String,
    pub label: String,
    pub commands: Vec<String>,
    pub cwd: PathBuf,
}

/// The startup plan: what to prompt about and what to run.
pub struct AgentUpdatePlan {
    pub prompts: Vec<UpdateTarget>,
    pub updates: Vec<UpdateTarget>,
}

/// Pure plan builder. Binding rules:
/// 0. Only commands in `registered_commands` (the command strings of
///    `settings.agents[]`, i.e. agents the user actually registered) are
///    considered: a catalog-only command is never prompted nor updated.
///    Zero registered commands -> empty plan.
/// 1. Distinct commands in catalog order (first occurrence wins). `label` and
///    `commands` come from the FIRST catalog entry (in order) whose
///    `update_commands` is non-empty. A command with NO non-empty sequence is
///    skipped entirely: never prompted, never updated.
/// 2. `answers[command]` absent -> prompt (default No on timeout).
/// 3. `Some(true)` -> update now.
/// 4. `Some(false)` -> nothing.
///
/// Prompts and updates are disjoint by construction.
pub fn build_update_plan(
    catalog: &[crate::config::coding_agents_catalog::CodingAgentDefinition],
    registered_commands: &HashSet<String>,
    answers: &BTreeMap<String, bool>,
    default_cwd: PathBuf,
) -> AgentUpdatePlan {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut prompts: Vec<UpdateTarget> = Vec::new();
    let mut updates: Vec<UpdateTarget> = Vec::new();

    for entry in catalog {
        if !registered_commands.contains(&entry.command) {
            continue; // catalog-only command: never prompted, never updated
        }
        if !seen.insert(entry.command.as_str()) {
            continue; // first occurrence wins
        }
        let Some(sequence) = catalog
            .iter()
            .find(|e| e.command == entry.command && !e.update_commands.is_empty())
            .map(|e| e.update_commands.clone())
        else {
            continue; // no entry for this command carries an update sequence
        };
        let target = UpdateTarget {
            command: entry.command.clone(),
            label: entry.label.clone(),
            commands: sequence,
            cwd: default_cwd.clone(),
        };
        match answers.get(&entry.command) {
            None => prompts.push(target),
            Some(true) => updates.push(target),
            Some(false) => {}
        }
    }

    AgentUpdatePlan { prompts, updates }
}

/// Last `OUTPUT_TAIL_BYTES` of a captured pipe, lossy-decoded for logging.
fn output_tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(OUTPUT_TAIL_BYTES);
    String::from_utf8_lossy(&bytes[start..]).to_string()
}

/// Execute one command's update sequence IN ORDER, fail-fast: the first failed
/// step stops the chain. Each step is a full shell string run through
/// `cmd.exe /C` (Windows) or `sh -c` (elsewhere), cwd = the target's directory,
/// no stdin, output tail-captured concurrently with the wait (a chatty command
/// must never block on the 64KB pipe buffer). Per-step timeout kills the WHOLE
/// tree (Windows JobObject, Unix process-group kill).
async fn run_update_sequence(target: &UpdateTarget, step_timeout: Duration) -> AgentUpdateResult {
    let mut first_error: Option<String> = None;

    for cmd in &target.commands {
        log::info!(
            "[agent-update] running '{}' for {} ({})",
            cmd,
            target.label,
            target.command
        );

        let mut command = {
            let mut c = if cfg!(windows) {
                let mut c = tokio::process::Command::new("cmd.exe");
                c.arg("/C").arg(cmd);
                c
            } else {
                let mut c = tokio::process::Command::new("sh");
                c.arg("-c").arg(cmd);
                c
            };
            c.current_dir(&target.cwd);
            c.stdin(Stdio::null());
            c.stdout(Stdio::piped());
            c.stderr(Stdio::piped());
            c.kill_on_drop(true); // safety net: a dropped child never lingers
            c
        };
        // The GUI binary is `windows_subsystem = "windows"` and never owns a
        // console, so UNCONDITIONALLY suppress the child's console window
        // (without the flag every update step pops one over the splash).
        #[cfg(windows)]
        {
            // CREATE_NO_WINDOW. `tokio::process::Command` exposes this as an
            // inherent method on Windows (no trait import needed).
            command.creation_flags(0x0800_0000);
        }
        // Unix tree-kill support: descendants inherit the group (`sh -c` creates
        // no new group); on timeout the whole tree dies with one group kill.
        #[cfg(unix)]
        {
            // `tokio::process::Command` exposes this as an inherent method on
            // Unix, mirroring `creation_flags` above (no trait import needed).
            command.process_group(0);
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                first_error = Some(e.to_string());
                break;
            }
        };
        // F2 (round 3) pin: job dropped at step end on EVERY path - no
        // survivors from an update step on Windows (deliberate, plan 5.2/18).
        // The timeout arm keeps `terminate()` and the R1 truncation path keeps
        // `job.take()`; on the plain-success path KILL_ON_JOB_CLOSE reaps any
        // fully detached descendant the update command left behind. On Unix
        // there is no job: detached descendants survive the step (group-kill
        // runs only on timeout/truncation).
        let mut job: Option<crate::pty::job::JobObject> = {
            #[cfg(windows)]
            {
                child.id().and_then(crate::pty::job::JobObject::for_child)
            }
            #[cfg(not(windows))]
            {
                None
            }
        };

        // Drain the pipes CONCURRENTLY with the wait: each reader task finishes
        // on EOF and hands its buffer over a oneshot.
        let mut stdout_rx = child.stdout.take().map(|mut out| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let _ = tokio::io::AsyncReadExt::read_to_end(&mut out, &mut buf).await;
                let _ = tx.send(buf);
            });
            rx
        });
        let mut stderr_rx = child.stderr.take().map(|mut err| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let _ = tokio::io::AsyncReadExt::read_to_end(&mut err, &mut buf).await;
                let _ = tx.send(buf);
            });
            rx
        });

        async fn collect(
            stdout_rx: &mut Option<tokio::sync::oneshot::Receiver<Vec<u8>>>,
            stderr_rx: &mut Option<tokio::sync::oneshot::Receiver<Vec<u8>>>,
        ) -> (Vec<u8>, Vec<u8>) {
            let stdout = match stdout_rx {
                Some(rx) => rx.await.unwrap_or_default(),
                None => Vec::new(),
            };
            let stderr = match stderr_rx {
                Some(rx) => rx.await.unwrap_or_default(),
                None => Vec::new(),
            };
            (stdout, stderr)
        }

        #[cfg(unix)]
        let pid = child.id().unwrap_or(0);
        let waited = tokio::time::timeout(step_timeout, child.wait()).await;
        let (step_ok, step_error): (bool, Option<String>) = match waited {
            Ok(Ok(status)) => {
                // The parent exited. Join the readers bounded; a descendant
                // (npm -> node) can hold the pipe open past the parent exit.
                let joined =
                    tokio::time::timeout(READER_JOIN_TIMEOUT, collect(&mut stdout_rx, &mut stderr_rx))
                        .await;
                let (stdout, stderr) = match joined {
                    Ok((out, err)) => (out, err),
                    Err(_) => {
                        log::warn!(
                            "[agent-update] output truncated: descendant holds the pipe"
                        );
                        // Kill the tree BEFORE joining again so the readers EOF:
                        // Windows KILL_ON_JOB_CLOSE reaps the lingering tree
                        // member, Unix the process-group kill. The detached
                        // readers' buffers are unreachable, so "take what
                        // arrived" is not an option; a second bounded join
                        // recovers the FULL tail once the tree is dead.
                        if let Some(job) = job.take() {
                            drop(job);
                        }
                        #[cfg(unix)]
                        // SAFETY: `-pid` is the process group created by
                        // `process_group(0)` at spawn; it contains only this
                        // update step's own tree.
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGKILL);
                        }
                        match tokio::time::timeout(
                            READER_JOIN_TIMEOUT,
                            collect(&mut stdout_rx, &mut stderr_rx),
                        )
                        .await
                        {
                            Ok((out, err)) => (out, err),
                            Err(_) => {
                                log::warn!(
                                    "[agent-update] output still truncated after tree kill"
                                );
                                (Vec::new(), Vec::new())
                            }
                        }
                    }
                };
                if status.success() {
                    log::info!(
                        "[agent-update] step ok for {} ({}):\n{}",
                        target.label,
                        target.command,
                        output_tail(&stdout)
                    );
                    (true, None)
                } else {
                    let reason = format!("exit code {}", status.code().unwrap_or(-1));
                    let stderr_tail = output_tail(&stderr);
                    log::warn!(
                        "[agent-update] step FAILED for {} ({}): {}\n{}{}",
                        target.label,
                        target.command,
                        reason,
                        output_tail(&stdout),
                        if stderr_tail.is_empty() {
                            String::new()
                        } else {
                            format!("\nstderr:\n{stderr_tail}")
                        }
                    );
                    (false, Some(reason))
                }
            }
            Ok(Err(e)) => {
                log::warn!(
                    "[agent-update] step spawn/wait error for {} ({}): {e}",
                    target.label,
                    target.command
                );
                (false, Some(e.to_string()))
            }
            Err(_) => {
                // Step timeout: kill the whole tree, bounded, best-effort.
                if let Some(job) = &job {
                    job.terminate();
                }
                let _ = child.kill().await;
                #[cfg(unix)]
                // SAFETY: `-pid` is the process group created by
                // `process_group(0)` at spawn; it contains only this update
                // step's own tree.
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
                let _ =
                    tokio::time::timeout(READER_JOIN_TIMEOUT, child.wait()).await;
                let (stdout, stderr) =
                    tokio::time::timeout(READER_JOIN_TIMEOUT, collect(&mut stdout_rx, &mut stderr_rx))
                        .await
                        .unwrap_or_default();
                let reason = format!("timed out after {}s (killed)", step_timeout.as_secs());
                log::warn!(
                    "[agent-update] step TIMED OUT for {} ({}): {reason}\n{}",
                    target.label,
                    target.command,
                    output_tail(&stdout)
                );
                if !stderr.is_empty() {
                    log::warn!(
                        "[agent-update] stderr tail for {} ({}):\n{}",
                        target.label,
                        target.command,
                        output_tail(&stderr)
                    );
                }
                (false, Some(reason))
            }
        };

        if !step_ok {
            first_error = step_error;
            break; // fail-fast: later steps of this command do not run
        }
    }

    AgentUpdateResult {
        command: target.command.clone(),
        label: target.label.clone(),
        ok: first_error.is_none(),
        error: first_error,
    }
}

/// Releases the gate on EVERY exit path (including panics), and emits the
/// finished event exactly once per started run. Created as the FIRST statement
/// of the startup task body so even a settings/catalog panic unblocks sessions.
struct FinishGuard {
    gate: Arc<AgentUpdateGate>,
    app: AppHandle,
    /// True once `agent_updates_started` was emitted; only then is the finished
    /// event (and the frontend splash teardown) owed.
    emit_finished: bool,
    /// Filled by the explicit completion path; a panic leaves it None -> empty
    /// results (no phantom failures).
    results: Option<Vec<AgentUpdateResult>>,
}

impl FinishGuard {
    fn complete(&mut self, results: Vec<AgentUpdateResult>) {
        self.results = Some(results);
    }
}

impl Drop for FinishGuard {
    fn drop(&mut self) {
        let results = self.results.take().unwrap_or_default();
        self.gate.mark_finished(results.clone()); // idempotent: no-op once finished
        if self.emit_finished {
            // Emitter::emit is synchronous in tauri v2; no await is legal or
            // needed in Drop.
            let _ = self
                .app
                .emit("agent_updates_finished", serde_json::json!({ "results": results }));
        }
    }
}

/// Detached startup task (spawned in `lib.rs` setup before `submit_restore_first`).
/// Plans from the SAME catalog source `get_coding_agent_catalog` serves, prompts
/// sequentially (60s each, default No), runs updates in parallel across commands
/// (300s per step), then releases the gate. Never returns an error: every failure
/// path is a red-toast result, not a startup failure.
pub async fn run_startup_updates(app: AppHandle, gate: Arc<AgentUpdateGate>) {
    // 1. Guard FIRST: a panic anywhere still releases the gate.
    let mut guard = FinishGuard {
        gate: Arc::clone(&gate),
        app: app.clone(),
        emit_finished: false,
        results: None,
    };

    let settings = app.state::<SettingsState>().read().await.clone();
    let catalog = load_catalog_for_settings(&settings);
    let default_cwd = primary_project_root(&settings)
        .or_else(crate::config::config_dir)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // 2. Pure plan. An empty plan releases the gate instantly (guard Drop) and
    // emits nothing: no splash flash on a quiet boot.
    let registered_commands: HashSet<String> = settings
        .agents
        .iter()
        .map(|agent| agent.command.clone())
        .collect();
    let plan = build_update_plan(
        &catalog,
        &registered_commands,
        &settings.agent_auto_update_by_command,
        default_cwd,
    );
    if plan.prompts.is_empty() && plan.updates.is_empty() {
        log::debug!("[agent-update] nothing to prompt or update; skipping");
        return;
    }

    gate.mark_started();
    let _ = app.emit("agent_updates_started", ());
    guard.emit_finished = true;

    // #1341 - the prompt phase was log-silent, which hid the startup freeze
    // (the prompt expired unseen at PROMPT_TIMEOUT while the setup thread was
    // blocked inside the restore block_on). Info-level visibility from here.
    if !plan.prompts.is_empty() {
        log::info!(
            "[agent-update] prompt phase started: {} command(s) await SI/NO ({}s each, default No): [{}]",
            plan.prompts.len(),
            PROMPT_TIMEOUT.as_secs(),
            plan.prompts
                .iter()
                .map(|p| p.command.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // 3. Prompt phase: SEQUENTIAL, catalog order, one modal at a time.
    let mut updates: Vec<UpdateTarget> = plan.updates;
    for pending in &plan.prompts {
        let rx = gate.register_prompt(&pending.command, &pending.label);
        let _ = app.emit(
            "agent_update_prompt",
            &AgentUpdatePrompt {
                command: pending.command.clone(),
                label: pending.label.clone(),
            },
        );
        log::info!(
            "[agent-update] prompting for '{}' ({}) - awaiting SI/NO ({}s, default No)",
            pending.command,
            pending.label,
            PROMPT_TIMEOUT.as_secs()
        );
        match tokio::time::timeout(PROMPT_TIMEOUT, rx).await {
            Ok(Ok(true)) => updates.push(pending.clone()), // answer command already persisted true
            Ok(Ok(false)) => {}                            // persisted false; never asked again
            Ok(Err(_)) | Err(_) => {
                // Timeout / channel dropped: nothing persisted (asked again next
                // boot), nothing runs this boot. was_prompted stays true so a
                // late answer still persists and returns Ok(false).
                gate.drop_pending(&pending.command);
                let _ = app.emit("agent_update_prompt_closed", ());
            }
        }
    }

    // 4. Update phase: PARALLEL across commands (one task per command keeps a
    // panicking command from aborting the others), each sequence strictly
    // ordered inside `run_update_sequence`.
    let handles: Vec<_> = updates
        .iter()
        .map(|t| {
            let target = t.clone();
            tokio::spawn(async move { run_update_sequence(&target, UPDATE_STEP_TIMEOUT).await })
        })
        .collect();
    let joined = futures::future::join_all(handles).await;
    let results: Vec<AgentUpdateResult> = joined
        .into_iter()
        .zip(updates.iter())
        .map(|(r, t)| match r {
            Ok(res) => res,
            Err(_) => AgentUpdateResult {
                command: t.command.clone(),
                label: t.label.clone(),
                ok: false,
                error: Some("update task panicked".to_string()),
            },
        })
        .collect();

    // 5. Exactly one mark, exactly one emit; the guard's Drop fires on return.
    guard.complete(results);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::coding_agents_catalog::CodingAgentDefinition;

    fn entry(command: &str, label: &str, update_commands: Vec<&str>) -> CodingAgentDefinition {
        CodingAgentDefinition {
            key: command.to_string(),
            label: label.to_string(),
            description: String::new(),
            color: String::new(),
            command: command.to_string(),
            instructions_filename: None,
            envs: Vec::new(),
            isolated_home: false,
            config_seed: None,
            removable: true,
            update_commands: update_commands.into_iter().map(str::to_string).collect(),
            auto_update: false,
        }
    }

    fn cwd() -> PathBuf {
        PathBuf::from("/tmp/proj")
    }

    #[test]
    fn build_plan_dedupes_by_command_first_entry_wins() {
        let catalog = vec![
            entry("claude", "Claude (primary)", vec!["claude --update"]),
            entry("claude", "Claude (secondary)", vec!["claude --update --beta"]),
            entry("codex", "Codex", vec!["codex update"]),
        ];
        let answers = BTreeMap::new();
        let registered = HashSet::from(["claude".to_string(), "codex".to_string()]);
        let plan = build_update_plan(&catalog, &registered, &answers, cwd());
        assert_eq!(plan.prompts.len(), 2);
        assert!(plan.updates.is_empty());
        let claude = plan
            .prompts
            .iter()
            .find(|t| t.command == "claude")
            .expect("claude prompt");
        assert_eq!(claude.label, "Claude (primary)");
        assert_eq!(claude.commands, vec!["claude --update"]);
    }

    #[test]
    fn build_plan_first_nonempty_sequence_wins() {
        let catalog = vec![
            entry("claude", "Claude A", vec![]),
            entry("claude", "Claude B", vec!["claude --update"]),
        ];
        let answers = BTreeMap::from([("claude".to_string(), true)]);
        let registered = HashSet::from(["claude".to_string()]);
        let plan = build_update_plan(&catalog, &registered, &answers, cwd());
        assert!(plan.prompts.is_empty());
        assert_eq!(plan.updates.len(), 1);
        let claude = &plan.updates[0];
        assert_eq!(claude.label, "Claude A");
        assert_eq!(claude.commands, vec!["claude --update"]);
    }

    #[test]
    fn build_plan_skips_commands_without_sequence() {
        let catalog = vec![
            entry("claude", "Claude", vec!["claude --update"]),
            entry("hermes", "Hermes", vec![]),
        ];
        let answers = BTreeMap::new();
        let registered = HashSet::from(["claude".to_string(), "hermes".to_string()]);
        let plan = build_update_plan(&catalog, &registered, &answers, cwd());
        assert_eq!(plan.prompts.len(), 1);
        assert_eq!(plan.prompts[0].command, "claude");
        assert!(!plan.prompts.iter().any(|t| t.command == "hermes"));
        assert!(plan.updates.is_empty());
    }

    #[test]
    fn build_plan_answer_kinds_and_disjointness() {
        let catalog = vec![
            entry("claude", "Claude", vec!["claude --update"]),
            entry("codex", "Codex", vec!["codex update"]),
            entry("pi", "Pi", vec!["pi update"]),
        ];
        let answers = BTreeMap::from([("claude".to_string(), true)]);
        let registered =
            HashSet::from(["claude".to_string(), "codex".to_string(), "pi".to_string()]);
        let plan = build_update_plan(&catalog, &registered, &answers, cwd());
        assert_eq!(plan.prompts.len(), 2);
        assert_eq!(plan.updates.len(), 1);
        assert_eq!(plan.updates[0].command, "claude");
        // catalog order preserved for prompts
        let prompt_commands: Vec<&str> = plan.prompts.iter().map(|t| t.command.as_str()).collect();
        assert_eq!(prompt_commands, vec!["codex", "pi"]);
        // disjoint by construction
        for p in &plan.prompts {
            assert!(!plan.updates.iter().any(|u| u.command == p.command));
        }
    }

    #[test]
    fn build_plan_filters_to_registered_commands() {
        let catalog = vec![
            entry("claude", "Claude", vec!["claude --update"]),
            entry("codex", "Codex", vec!["codex update"]),
            entry("pi", "Pi", vec!["pi update"]),
        ];
        let registered = HashSet::from(["claude".to_string(), "pi".to_string()]);
        let answers = BTreeMap::new();
        let plan = build_update_plan(&catalog, &registered, &answers, cwd());
        assert_eq!(plan.prompts.len(), 2);
        assert!(plan.updates.is_empty());
        // catalog order preserved for the registered subset
        let prompt_commands: Vec<&str> = plan.prompts.iter().map(|t| t.command.as_str()).collect();
        assert_eq!(prompt_commands, vec!["claude", "pi"]);
        // catalog-only command skipped: never prompted, never updated
        assert!(!plan.prompts.iter().any(|t| t.command == "codex"));
        assert!(!plan.updates.iter().any(|t| t.command == "codex"));
    }

    #[test]
    fn build_plan_zero_registered_commands_returns_empty_plan() {
        let catalog = vec![
            entry("claude", "Claude", vec!["claude --update"]),
            entry("codex", "Codex", vec!["codex update"]),
        ];
        let registered = HashSet::new();
        let answers = BTreeMap::new();
        let plan = build_update_plan(&catalog, &registered, &answers, cwd());
        assert!(plan.prompts.is_empty());
        assert!(plan.updates.is_empty());
    }

    #[test]
    fn build_plan_registered_filter_preserves_first_entry_semantics() {
        let catalog = vec![
            entry("claude", "Claude (primary)", vec![]),
            entry("claude", "Claude (secondary)", vec!["claude --update"]),
            entry("codex", "Codex", vec!["codex update"]),
        ];
        let registered = HashSet::from(["claude".to_string()]);
        let answers = BTreeMap::from([("claude".to_string(), true)]);
        let plan = build_update_plan(&catalog, &registered, &answers, cwd());
        assert!(plan.prompts.is_empty());
        assert_eq!(plan.updates.len(), 1);
        let claude = &plan.updates[0];
        assert_eq!(claude.label, "Claude (primary)"); // first-entry label wins
        assert_eq!(claude.commands, vec!["claude --update"]); // first non-empty sequence wins
    }

    #[tokio::test]
    async fn gate_wait_returns_after_finish_and_releases_all_waiters() {
        let gate = Arc::new(AgentUpdateGate::new());
        let gate2 = Arc::clone(&gate);
        let waiter = tokio::spawn(async move { gate2.wait_until_done().await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        gate.mark_finished(vec![]);
        tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter released")
            .expect("waiter ok");

        // late waiter returns immediately
        let gate3 = Arc::clone(&gate);
        let late = tokio::spawn(async move { gate3.wait_until_done().await });
        tokio::time::timeout(Duration::from_secs(2), late)
            .await
            .expect("late waiter released")
            .expect("late waiter ok");
    }

    #[tokio::test]
    async fn gate_prompt_registry_and_snapshot() {
        let gate = AgentUpdateGate::new();
        gate.mark_started();
        let rx = gate.register_prompt("claude", "Claude");
        assert!(gate.was_prompted("claude"));
        let snap = gate.snapshot();
        assert!(snap.in_progress);
        assert_eq!(snap.prompt.as_ref().map(|p| p.command.as_str()), Some("claude"));

        assert!(gate.resolve_answer("claude", true));
        assert!(!gate.resolve_answer("claude", true), "second resolve must fail");
        assert!(rx.await.expect("answer delivered"));
        assert!(gate.snapshot().prompt.is_none(), "resolved prompt cleared");

        // dropped-receiver path (round-3 F1): the prompt timeout drops the
        // answer channel; a late resolve must return false without panicking
        let gate2 = AgentUpdateGate::new();
        let rx_dropped = gate2.register_prompt("codex", "Codex");
        drop(rx_dropped);
        assert!(!gate2.resolve_answer("codex", true));
        assert!(
            gate2.snapshot().prompt.is_none(),
            "a dead prompt must not be resurrected by the snapshot"
        );

        // drop_pending path: no answer, snapshot must not resurrect the prompt
        let rx2 = gate.register_prompt("codex", "Codex");
        gate.drop_pending("codex");
        assert!(!gate.resolve_answer("codex", false));
        assert!(rx2.await.is_err(), "sender dropped");
        assert!(gate.snapshot().prompt.is_none());

        // mark_finished clears pending prompt and pending senders
        let rx3 = gate.register_prompt("pi", "Pi");
        gate.mark_finished(vec![AgentUpdateResult {
            command: "pi".to_string(),
            label: "Pi".to_string(),
            ok: true,
            error: None,
        }]);
        assert!(rx3.await.is_err(), "sender dropped by mark_finished");
        let snap = gate.snapshot();
        assert!(!snap.in_progress);
        assert!(snap.prompt.is_none());
        assert_eq!(snap.results.len(), 1);
        assert!(snap.results[0].ok);

        // idempotent: repeated mark_finished keeps finished and replaces results
        gate.mark_finished(vec![]);
        assert!(!gate.snapshot().in_progress);
        assert!(gate.snapshot().results.is_empty());
    }

    /// Platform-shell-aware runner tests. Every command string below is a full
    /// shell string valid under BOTH cmd.exe /C and sh -c.
    async fn run_in_tmpdir(commands: Vec<&str>, step_timeout: Duration) -> AgentUpdateResult {
        let dir = tempfile::tempdir().expect("tempdir");
        run_update_sequence(
            &UpdateTarget {
                command: "test".to_string(),
                label: "Test".to_string(),
                commands: commands.into_iter().map(str::to_string).collect(),
                cwd: dir.path().to_path_buf(),
            },
            step_timeout,
        )
        .await
    }

    #[tokio::test]
    async fn runner_exit_zero_ok() {
        let result = run_in_tmpdir(vec!["exit 0"], UPDATE_STEP_TIMEOUT).await;
        assert!(result.ok, "unexpected: {result:?}");
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn runner_exit_nonzero_fails_with_code() {
        let result = run_in_tmpdir(vec!["exit 3"], UPDATE_STEP_TIMEOUT).await;
        assert!(!result.ok);
        assert_eq!(result.error.as_deref(), Some("exit code 3"));
    }

    #[tokio::test]
    async fn runner_fail_fast_stops_sequence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("marker.txt");
        let result = run_update_sequence(
            &UpdateTarget {
                command: "test".to_string(),
                label: "Test".to_string(),
                commands: vec![
                    "exit 3".to_string(),
                    format!(
                        "echo x > {}",
                        marker.to_string_lossy().replace('\\', "/")
                    ),
                ],
                cwd: dir.path().to_path_buf(),
            },
            UPDATE_STEP_TIMEOUT,
        )
        .await;
        assert!(!result.ok);
        assert!(!marker.exists(), "later step must not run after a failure");
    }

    #[tokio::test]
    async fn runner_timeout_kills_tree() {
        // Windows: ping ~30s (cmd has no sleep); Unix: sleep 30. The step
        // timeout (200ms) must kill the whole tree; the test itself is bounded.
        let step = if cfg!(windows) {
            "ping -n 30 127.0.0.1 >NUL"
        } else {
            "sleep 30"
        };
        let started = std::time::Instant::now();
        let result = run_in_tmpdir(vec![step], Duration::from_millis(200)).await;
        assert!(!result.ok);
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|e| e.contains("timed out")),
            "unexpected: {result:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "tree kill must be prompt, took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn runner_success_with_descendant_holding_pipe() {
        // The parent exits 0 quickly while a descendant keeps the pipes open:
        // the reader-join timeout arm runs (warn + tree-kill FIRST + second
        // bounded join) and the step is reported SUCCESS with the full tail.
        let step = if cfg!(windows) {
            "cmd /C start /B ping -n 30 127.0.0.1 >NUL"
        } else {
            "sh -c 'sleep 30 &'"
        };
        let started = std::time::Instant::now();
        let result = run_in_tmpdir(vec![step], UPDATE_STEP_TIMEOUT).await;
        assert!(result.ok, "unexpected: {result:?}");
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "descendant must be reaped promptly, took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn runner_success_with_detached_survivor() {
        // round-3 F2 pin (deliberate platform divergence): a fully detached
        // (non-pipe-holding) descendant of the update step is reaped on
        // Windows by the per-step JobObject drop (KILL_ON_JOB_CLOSE: no
        // survivors from an update step on Windows) but SURVIVES on Unix (no
        // job; the group-kill runs only on timeout/truncation). The step
        // itself reports success on both platforms, via the plain-success
        // path, NOT the R1 reader-truncation arm: the survivor's handles are
        // redirected away from the step's pipes, so the readers EOF when the
        // parent exits. The survivor writes its marker only after a delay that
        // exceeds the step.
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("marker.txt");
        let marker_str = marker.to_string_lossy().replace('\\', "/");
        let step = if cfg!(windows) {
            // The plan's pinned `cmd /C start /B "" cmd /C "<cmd>"` shape was
            // VERIFIED to hang layer-2 cmd at `start` when spawned through the
            // runner's `cmd.exe /C` chain (start mangles the nested quotes
            // into a recursive `start "" "..."` command, observed in the
            // process tree), so the implementer pinned a quoting-free
            // equivalent with the SAME semantics (plan 9.1: implementer
            // verifies both arms and pins the exact quoting): `start` an exe
            // target - powershell, whose -EncodedCommand carries the whole
            // script with no quotes/spaces - with the handles redirected to
            // NUL on the start line. PowerShell exits right after
            // Start-Process returns (no wait); the detached survivor (the
            // Start-Process child, in the step's job, NOT inheriting the
            // step's pipes) pings ~2s and only then writes the marker - after
            // the step end, so the per-step job drop must reap it before the
            // write.
            use base64::Engine as _;
            let script = format!(
                "Start-Process cmd -ArgumentList '/c','ping -n 3 127.0.0.1 >NUL & echo x > {marker_str}' -WindowStyle Hidden"
            );
            let mut utf16 = Vec::new();
            for u in script.encode_utf16() {
                utf16.extend_from_slice(&u.to_le_bytes());
            }
            let encoded = base64::engine::general_purpose::STANDARD.encode(&utf16);
            format!("start /B powershell -NoProfile -EncodedCommand {encoded} >NUL 2>NUL")
        } else {
            format!("sh -c '(sleep 1; echo x > {marker_str}) >/dev/null 2>&1 &'")
        };
        let started = std::time::Instant::now();
        let result = run_update_sequence(
            &UpdateTarget {
                command: "test".to_string(),
                label: "Test".to_string(),
                commands: vec![step],
                cwd: dir.path().to_path_buf(),
            },
            UPDATE_STEP_TIMEOUT,
        )
        .await;
        assert!(result.ok, "unexpected: {result:?}");
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "completion bounded, took {:?}",
            started.elapsed()
        );
        if cfg!(windows) {
            // The per-step job drop reaped the survivor before its delayed
            // marker write.
            tokio::time::sleep(Duration::from_secs(3)).await;
            assert!(!marker.exists(), "Windows survivor must have been reaped");
        } else {
            // No job on Unix: the detached descendant survives the step and
            // writes the marker within ~3s.
            for _ in 0..30 {
                if marker.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert!(marker.exists(), "Unix survivor must write the marker");
        }
    }

    #[tokio::test]
    async fn pending_prompt_release_requires_only_concurrent_runtime_work() {
        // #1341: pins the cooperative gate-release invariant the spawned
        // restore task relies on (the answerer task is the `agent_update_answer`
        // command; the waiter is the restore wake; both must progress on the
        // same runtime without the main thread doing anything). It does NOT
        // reproduce the #1341 freeze itself (the gate was cooperative pre-fix;
        // the bug was the lib.rs setup `block_on`, unreachable from a unit
        // test) - the freeze regression net is AC2 + AC8 + AC9 + AC11. If the
        // gate wait were ever made thread-blocking, this test would hang on the
        // single-threaded tokio test runtime and the timeout would fail.
        let gate = Arc::new(AgentUpdateGate::new());
        gate.mark_started();
        let _rx = gate.register_prompt("claude", "Claude");
        let waiter = tokio::spawn({
            let gate = Arc::clone(&gate);
            async move { gate.wait_until_done().await }
        });
        let answerer = tokio::spawn({
            let gate = Arc::clone(&gate);
            async move {
                tokio::time::sleep(Duration::from_millis(25)).await;
                assert!(gate.resolve_answer("claude", false), "prompt must still be pending");
                gate.mark_finished(vec![]);
            }
        });
        tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("gate waiter must be released by concurrent runtime work")
            .expect("waiter ok");
        answerer.await.expect("answerer ok");
    }
}
