use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::container_runtime::{
    ContainerCleanupReport, ContainerDiagnostics, ContainerRuntime, ContainerRuntimeControl,
    ContainerRuntimeHandle, ContainerStartRequest, ContainerStateSnapshot,
    RetainedContainerOwnerContext, CONTAINER_STOP_TIMEOUT, DEFAULT_API_HELPER_PATH,
    DEFAULT_BRIDGE_ENTRYPOINT, SESSION_LABEL,
};

const DOCKER_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const DOCKER_COMMAND_FINALIZATION_RESERVE: Duration = Duration::from_secs(5);
const DOCKER_COMMAND_POLL: Duration = Duration::from_millis(10);
const DOCKER_COMMAND_OUTPUT_BYTE_LIMIT: usize = 64 * 1024;

fn normalized_app_error_text(error: &AppError) -> String {
    match error {
        AppError::PtyError(message) => message.clone(),
        _ => error.to_string(),
    }
}

fn redact_command_values(input: &str, spec: &DockerCommandSpec) -> String {
    spec.secret_env
        .values()
        .chain(spec.redacted_values.iter())
        .filter(|value| !value.is_empty())
        .fold(input.to_string(), |text, value| {
            text.replace(value, "[REDACTED]")
        })
}

fn redact_command_bytes(bytes: &mut Vec<u8>, spec: &DockerCommandSpec) {
    for value in spec
        .secret_env
        .values()
        .chain(spec.redacted_values.iter())
        .filter(|value| !value.is_empty())
    {
        let needle = value.as_bytes();
        if !bytes.windows(needle.len()).any(|window| window == needle) {
            continue;
        }
        let original = std::mem::take(bytes);
        let mut redacted = Vec::with_capacity(original.len());
        let mut cursor = 0;
        while cursor < original.len() {
            let next = original[cursor..]
                .windows(needle.len())
                .position(|window| window == needle);
            let Some(offset) = next else {
                redacted.extend_from_slice(&original[cursor..]);
                break;
            };
            let position = cursor + offset;
            redacted.extend_from_slice(&original[cursor..position]);
            redacted.extend_from_slice(b"[REDACTED]");
            cursor = position + needle.len();
        }
        *bytes = redacted;
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DockerCommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub secret_env: BTreeMap<String, String>,
    redacted_values: Vec<String>,
}

impl std::fmt::Debug for DockerCommandSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DockerCommandSpec")
            .field("program", &"[REDACTED_COMMAND]")
            .field("args_count", &self.args.len())
            .field(
                "secret_env_keys",
                &self.secret_env.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
struct CappedCommandStream {
    bytes: Vec<u8>,
    truncated: bool,
}

impl std::fmt::Debug for CappedCommandStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CappedCommandStream")
            .field("bytes", &self.bytes.len())
            .field("truncated", &self.truncated)
            .finish()
    }
}

#[derive(Debug)]
struct DockerCommandOutput {
    stdout: CappedCommandStream,
    stderr: CappedCommandStream,
}

#[derive(Debug)]
struct DockerCommandError {
    source: AppError,
    spawned: bool,
}

impl std::fmt::Display for DockerCommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.fmt(formatter)
    }
}

struct DockerCommandOwner {
    child: Option<Child>,
    stdout: Option<JoinHandle<std::io::Result<CappedCommandStream>>>,
    stderr: Option<JoinHandle<std::io::Result<CappedCommandStream>>>,
    session_id: Option<Uuid>,
    reason: &'static str,
    program: String,
    #[cfg(test)]
    active_child: Option<ActiveDockerChildGuard>,
}

#[derive(Default)]
struct DockerCommandOwnership {
    next_id: AtomicU64,
    entries: Mutex<Vec<DockerCommandOwnershipEntry>>,
    #[cfg(test)]
    retry_gate: Mutex<Option<DockerCommandRetryGate>>,
}

struct DockerCommandOwnershipEntry {
    id: u64,
    owner: Option<DockerCommandOwner>,
    session_id: Option<Uuid>,
    reason: &'static str,
    program: String,
    in_flight: bool,
    last_error: Option<String>,
}

#[cfg(test)]
struct DockerCommandRetryGate {
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[derive(Default)]
struct RetainedDockerStartCleanupRegistry {
    entries: Mutex<HashMap<Uuid, Arc<RetainedDockerStartCleanup>>>,
}

struct RetainedDockerStartCleanup {
    handle: ContainerRuntimeHandle,
    state: Mutex<RetainedDockerStartCleanupState>,
}

#[derive(Default)]
struct RetainedDockerStartCleanupState {
    in_flight: bool,
    last_error: Option<String>,
}

impl DockerCommandOwner {
    fn log_context(&self) -> String {
        format!(
            "session={} reason={}",
            self.session_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string()),
            self.reason
        )
    }

    fn reap_child_nonblocking(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(_)) => {
                self.child = None;
                #[cfg(test)]
                self.active_child.take();
            }
            Ok(None) => {}
            Err(error) => log::warn!(
                "[container-runtime] retained Docker child status failed {} error={}",
                self.log_context(),
                error
            ),
        }
    }

    fn reap_finished_readers(&mut self) {
        if self.stdout.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(reader) = self.stdout.take() {
                if let Err(error) = join_command_reader(reader, "stdout") {
                    log::warn!(
                        "[container-runtime] retained Docker stdout reader failed {} error={}",
                        self.log_context(),
                        error
                    );
                }
            }
        }
        if self.stderr.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(reader) = self.stderr.take() {
                if let Err(error) = join_command_reader(reader, "stderr") {
                    log::warn!(
                        "[container-runtime] retained Docker stderr reader failed {} error={}",
                        self.log_context(),
                        error
                    );
                }
            }
        }
    }

    fn terminate_until(&mut self, deadline: Instant, canceled: bool) -> bool {
        if let Some(child) = self.child.as_mut() {
            if let Err(error) = child.kill() {
                log::debug!(
                    "[container-runtime] Docker command kill returned program={} canceled={} {} error={}",
                    self.program,
                    canceled,
                    self.log_context(),
                    error
                );
            }
        }
        loop {
            self.reap_child_nonblocking();
            self.reap_finished_readers();
            if self.child.is_none() && self.stdout.is_none() && self.stderr.is_none() {
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            std::thread::sleep(DOCKER_COMMAND_POLL.min(remaining));
        }
    }

    fn collect_output_until(
        &mut self,
        deadline: Instant,
    ) -> Option<Result<DockerCommandOutput, AppError>> {
        loop {
            let stdout_ready = self.stdout.as_ref().is_some_and(JoinHandle::is_finished);
            let stderr_ready = self.stderr.as_ref().is_some_and(JoinHandle::is_finished);
            if stdout_ready && stderr_ready {
                let stdout = self.stdout.take()?;
                let stderr = self.stderr.take()?;
                return Some(join_command_readers(stdout, stderr));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            std::thread::sleep(DOCKER_COMMAND_POLL.min(remaining));
        }
    }

    fn is_terminal(&self) -> bool {
        self.child.is_none() && self.stdout.is_none() && self.stderr.is_none()
    }
}

impl DockerCommandOwnershipEntry {
    fn new(id: u64, owner: DockerCommandOwner, last_error: Option<String>) -> Self {
        Self {
            id,
            session_id: owner.session_id,
            reason: owner.reason,
            program: owner.program.clone(),
            owner: Some(owner),
            in_flight: false,
            last_error,
        }
    }
}

impl DockerCommandOwnership {
    fn retain(&self, owner: DockerCommandOwner, last_error: Option<String>) {
        log::error!(
            "[container-runtime] Docker command ownership retained {} state=retained",
            owner.log_context()
        );
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(DockerCommandOwnershipEntry::new(id, owner, last_error));
    }

    fn reap_finished(&self) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        entries.retain_mut(|entry| {
            if entry.in_flight {
                return true;
            }
            let Some(owner) = entry.owner.as_mut() else {
                entry.last_error = Some(
                    "retained Docker command entry lost owner outside an in-flight retry"
                        .to_string(),
                );
                return true;
            };
            owner.reap_child_nonblocking();
            owner.reap_finished_readers();
            !owner.is_terminal()
        });
    }

    fn retry_until(&self, deadline: Instant) {
        loop {
            if Instant::now() >= deadline {
                return;
            }
            let Some((id, mut owner, prior_error)) = ({
                let mut entries = self
                    .entries
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                entries
                    .iter_mut()
                    .find(|entry| !entry.in_flight && entry.owner.is_some())
                    .and_then(|entry| {
                        let owner = entry.owner.take()?;
                        entry.in_flight = true;
                        Some((entry.id, owner, entry.last_error.clone()))
                    })
            }) else {
                return;
            };

            #[cfg(test)]
            self.wait_at_retry_gate_for_test();

            let outcome = catch_unwind(AssertUnwindSafe(|| owner.terminate_until(deadline, true)));
            let (terminal, attempt_error) = match outcome {
                Ok(true) => (true, None),
                Ok(false) => (
                    false,
                    prior_error.or_else(|| {
                        Some(
                            "Docker command termination remained unresolved at cleanup deadline"
                                .to_string(),
                        )
                    }),
                ),
                Err(_) => {
                    log::error!(
                        "[container-runtime] retained Docker command cleanup panicked {} state=retained",
                        owner.log_context()
                    );
                    (false, Some("Docker command cleanup panicked".to_string()))
                }
            };

            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(index) = entries.iter().position(|entry| entry.id == id) {
                if terminal {
                    entries.remove(index);
                } else {
                    let entry = &mut entries[index];
                    entry.owner = Some(owner);
                    entry.in_flight = false;
                    entry.last_error = attempt_error;
                }
            } else if !terminal {
                log::error!(
                    "[container-runtime] retained Docker command diagnostic entry disappeared during retry id={} state=retained",
                    id
                );
                entries.push(DockerCommandOwnershipEntry::new(id, owner, attempt_error));
            }
            drop(entries);
            if !terminal {
                return;
            }
        }
    }

    fn retained_contexts(&self) -> Vec<RetainedContainerOwnerContext> {
        self.reap_finished();
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .map(|entry| RetainedContainerOwnerContext {
                owner: "dockerCommand",
                session_id: entry.session_id,
                reason: entry.reason.to_string(),
                program: Some(entry.program.clone()),
                runtime_handle: None,
                state: if entry.in_flight {
                    "inFlight"
                } else {
                    "retained"
                },
                in_flight: entry.in_flight,
                last_error: entry.last_error.clone(),
            })
            .collect()
    }

    #[cfg(test)]
    fn install_retry_gate_for_test(
        &self,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (entered, entered_receiver) = std::sync::mpsc::channel();
        let (release, release_receiver) = std::sync::mpsc::channel();
        *self
            .retry_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(DockerCommandRetryGate {
            entered,
            release: release_receiver,
        });
        (entered_receiver, release)
    }

    #[cfg(test)]
    fn wait_at_retry_gate_for_test(&self) {
        let gate = self
            .retry_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(gate) = gate {
            if gate.entered.send(()).is_ok() {
                let _ = gate.release.recv();
            }
        }
    }
}

static PROCESS_RETAINED_DOCKER_COMMANDS: OnceLock<Mutex<Vec<DockerCommandOwner>>> = OnceLock::new();

impl Drop for DockerCommandOwnership {
    fn drop(&mut self) {
        let entries = self
            .entries
            .get_mut()
            .unwrap_or_else(|error| error.into_inner());
        let missing_owner_count = entries.iter().filter(|entry| entry.owner.is_none()).count();
        if missing_owner_count > 0 {
            log::error!(
                "[container-runtime] Docker command ownership dropped with in-flight diagnostic entries count={}",
                missing_owner_count
            );
        }
        let mut owners = entries
            .drain(..)
            .filter_map(|entry| entry.owner)
            .collect::<Vec<_>>();
        if owners.is_empty() {
            return;
        }
        PROCESS_RETAINED_DOCKER_COMMANDS
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .append(&mut owners);
    }
}

#[cfg(test)]
#[derive(Clone)]
enum ScriptedDockerCommandResult {
    Output { stdout: Vec<u8>, stderr: Vec<u8> },
    SpawnedError(String),
    ReaderError(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DockerCommandShutdownBehavior {
    CancelOnShutdown,
    ContinueUntilDeadline,
}

impl DockerCommandOutput {
    #[cfg(test)]
    fn empty() -> Self {
        Self {
            stdout: CappedCommandStream::default(),
            stderr: CappedCommandStream::default(),
        }
    }
}

impl CappedCommandStream {
    fn trimmed_text(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.bytes).trim().to_string();
        if self.truncated {
            append_truncation_marker(&mut text);
        }
        text
    }

    fn trim_end_text(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.bytes).trim_end().to_string();
        if self.truncated {
            append_truncation_marker(&mut text);
        }
        text
    }
}

fn append_truncation_marker(text: &mut String) {
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str("[output truncated]");
}

fn read_capped_to_end<R: Read>(
    mut reader: R,
    limit: usize,
) -> std::io::Result<CappedCommandStream> {
    let mut output = CappedCommandStream {
        bytes: Vec::with_capacity(limit.min(8 * 1024)),
        truncated: false,
    };
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        let remaining = limit.saturating_sub(output.bytes.len());
        if remaining > 0 {
            let keep = n.min(remaining);
            output.bytes.extend_from_slice(&chunk[..keep]);
            if keep < n {
                output.truncated = true;
            }
        } else {
            output.truncated = true;
        }
    }
    Ok(output)
}

#[derive(Clone)]
pub struct DockerRuntime {
    program: String,
    command_ownership: Arc<DockerCommandOwnership>,
    retained_start_cleanups: Arc<RetainedDockerStartCleanupRegistry>,
    #[cfg(test)]
    recorded_commands: Option<std::sync::Arc<std::sync::Mutex<Vec<DockerCommandSpec>>>>,
    #[cfg(test)]
    scripted_results: Option<Arc<Mutex<std::collections::VecDeque<ScriptedDockerCommandResult>>>>,
    #[cfg(test)]
    hang_readers: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    active_children: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    active_readers: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
pub(crate) struct DockerCommandReaderReleaseForTest {
    gate: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

#[cfg(test)]
impl DockerCommandReaderReleaseForTest {
    pub(crate) fn release(&self) {
        let (released, changed) = &*self.gate;
        *released.lock().unwrap_or_else(|error| error.into_inner()) = true;
        changed.notify_all();
    }
}

#[cfg(test)]
impl Drop for DockerCommandReaderReleaseForTest {
    fn drop(&mut self) {
        self.release();
    }
}

impl Default for DockerRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl DockerRuntime {
    pub fn new() -> Self {
        Self {
            program: "docker".to_string(),
            command_ownership: Arc::new(DockerCommandOwnership::default()),
            retained_start_cleanups: Arc::new(RetainedDockerStartCleanupRegistry::default()),
            #[cfg(test)]
            recorded_commands: None,
            #[cfg(test)]
            scripted_results: None,
            #[cfg(test)]
            hang_readers: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            #[cfg(test)]
            active_children: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            #[cfg(test)]
            active_readers: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            command_ownership: Arc::new(DockerCommandOwnership::default()),
            retained_start_cleanups: Arc::new(RetainedDockerStartCleanupRegistry::default()),
            recorded_commands: None,
            scripted_results: None,
            hang_readers: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            active_children: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            active_readers: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_recorded_commands(
        program: impl Into<String>,
        recorded_commands: std::sync::Arc<std::sync::Mutex<Vec<DockerCommandSpec>>>,
    ) -> Self {
        Self {
            program: program.into(),
            command_ownership: Arc::new(DockerCommandOwnership::default()),
            retained_start_cleanups: Arc::new(RetainedDockerStartCleanupRegistry::default()),
            recorded_commands: Some(recorded_commands),
            scripted_results: None,
            hang_readers: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            active_children: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            active_readers: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    #[cfg(test)]
    fn with_scripted_commands(
        program: impl Into<String>,
        recorded_commands: Arc<Mutex<Vec<DockerCommandSpec>>>,
        results: Vec<ScriptedDockerCommandResult>,
    ) -> Self {
        Self {
            program: program.into(),
            command_ownership: Arc::new(DockerCommandOwnership::default()),
            retained_start_cleanups: Arc::new(RetainedDockerStartCleanupRegistry::default()),
            recorded_commands: Some(recorded_commands),
            scripted_results: Some(Arc::new(Mutex::new(results.into()))),
            hang_readers: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            active_children: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            active_readers: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    #[cfg(test)]
    fn active_child_count(&self) -> usize {
        self.active_children
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn active_reader_count(&self) -> usize {
        self.active_readers
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn retain_blocked_command_readers_for_test(
        &self,
        reason: &'static str,
        program: impl Into<String>,
        last_error: impl Into<String>,
    ) -> DockerCommandReaderReleaseForTest {
        let ready = Arc::new(std::sync::Barrier::new(3));
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let spawn_reader = |name: &'static str| {
            let ready = Arc::clone(&ready);
            let gate = Arc::clone(&gate);
            let active = Arc::clone(&self.active_readers);
            std::thread::Builder::new()
                .name(name.to_string())
                .spawn(move || {
                    let _active = ActiveDockerReaderGuard::new(active);
                    ready.wait();
                    let (released, changed) = &*gate;
                    let mut released = released.lock().unwrap_or_else(|error| error.into_inner());
                    while !*released {
                        released = changed
                            .wait(released)
                            .unwrap_or_else(|error| error.into_inner());
                    }
                    Ok(CappedCommandStream::default())
                })
                .expect("spawn deterministic blocked Docker reader")
        };
        let owner = DockerCommandOwner {
            child: None,
            stdout: Some(spawn_reader("blocked-docker-stdout")),
            stderr: Some(spawn_reader("blocked-docker-stderr")),
            session_id: None,
            reason,
            program: program.into(),
            active_child: None,
        };
        ready.wait();
        self.command_ownership
            .retain(owner, Some(last_error.into()));
        DockerCommandReaderReleaseForTest { gate }
    }

    #[cfg(test)]
    pub(crate) fn install_command_retry_gate_for_test(
        &self,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        self.command_ownership.install_retry_gate_for_test()
    }

    pub fn build_run_command(
        &self,
        request: &ContainerStartRequest,
    ) -> Result<DockerCommandSpec, AppError> {
        let args_json = serde_json::to_string(&request.args).unwrap_or_else(|_| "[]".to_string());
        let child_env_json =
            serde_json::to_string(&request.child_env).unwrap_or_else(|_| "[]".to_string());
        let env_unset_json = serde_json::to_string(&request.env_unset).map_err(|e| {
            AppError::Other(format!("failed to serialize container env unset: {e}"))
        })?;

        let mut args = vec![
            "run".to_string(),
            "-d".to_string(),
            "--name".to_string(),
            Self::container_name(request.session_id),
            "--label".to_string(),
            format!("{}={}", SESSION_LABEL, request.session_id),
            "--env".to_string(),
            format!("AGENTSCOMMANDER_API_URL={}", request.api_url),
            "--env".to_string(),
            "AGENTSCOMMANDER_API_TOKEN".to_string(),
            "--env".to_string(),
            format!("AGENTSCOMMANDER_SESSION_ID={}", request.session_id),
            "--env".to_string(),
            "AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN".to_string(),
            "--env".to_string(),
            "AGENTSCOMMANDER_ROOT".to_string(),
            "--env".to_string(),
            format!("AGENTSCOMMANDER_LOCAL_DIR={}", request.local_dir),
            "--env".to_string(),
            "AGENTSCOMMANDER_TRANSPORT=api".to_string(),
            "--env".to_string(),
            format!("AGENTSCOMMANDER_BINARY_PATH={DEFAULT_API_HELPER_PATH}"),
            "--env".to_string(),
            format!(
                "AGENTSCOMMANDER_BRIDGE_WORKDIR={}",
                request.container_workdir
            ),
            "--env".to_string(),
            "AGENTSCOMMANDER_BRIDGE_COMMAND".to_string(),
            "--env".to_string(),
            "AGENTSCOMMANDER_BRIDGE_ARGS_JSON".to_string(),
            "--env".to_string(),
            "AGENTSCOMMANDER_BRIDGE_ENV_JSON".to_string(),
            "--env".to_string(),
            // This is a flat key-name array, not key/value pairs. Do not add
            // AGENTSCOMMANDER_BRIDGE_ENV_UNSET_JSON to SENSITIVE_LOG_KEYS:
            // the value carries no secrets, and the redactor treats quoted
            // sensitive key names as keys whose following array item is a value.
            format!("AGENTSCOMMANDER_BRIDGE_ENV_UNSET_JSON={}", env_unset_json),
            "--env".to_string(),
            format!("AGENTSCOMMANDER_BRIDGE_COLS={}", request.cols),
            "--env".to_string(),
            format!("AGENTSCOMMANDER_BRIDGE_ROWS={}", request.rows),
            "--mount".to_string(),
            format!(
                "type=bind,source={},target={}",
                request.host_root, request.container_workdir
            ),
        ];

        // #935 - one read-write --mount per admissible repo, appended BEFORE the
        // workdir/image/entrypoint tail. Pushing after the tail would place them
        // after the image and turn them into container arguments (the image_idx
        // assertions below guard exactly this).
        for mount in &request.repo_mounts {
            args.push("--mount".to_string());
            args.push(format!(
                "type=bind,source={},target={}",
                mount.host_path.display(),
                mount.container_path
            ));
        }

        args.push("--workdir".to_string());
        args.push(request.container_workdir.clone());
        args.push("--".to_string());
        args.push(request.image.clone());
        args.push(DEFAULT_BRIDGE_ENTRYPOINT.to_string());

        args.retain(|arg| !arg.is_empty());
        let secret_env = BTreeMap::from([
            (
                "AGENTSCOMMANDER_API_TOKEN".to_string(),
                request.api_token.clone(),
            ),
            (
                "AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN".to_string(),
                request.registration_ticket.clone(),
            ),
            (
                "AGENTSCOMMANDER_ROOT".to_string(),
                request.host_root.clone(),
            ),
            (
                "AGENTSCOMMANDER_BRIDGE_COMMAND".to_string(),
                request.command.clone(),
            ),
            ("AGENTSCOMMANDER_BRIDGE_ARGS_JSON".to_string(), args_json),
            (
                "AGENTSCOMMANDER_BRIDGE_ENV_JSON".to_string(),
                child_env_json,
            ),
        ]);
        let mut redacted_values = vec![request.host_root.clone()];
        redacted_values.extend(
            request
                .repo_mounts
                .iter()
                .map(|mount| mount.host_path.to_string_lossy().to_string()),
        );
        Ok(DockerCommandSpec {
            program: self.program.clone(),
            args,
            secret_env,
            redacted_values,
        })
    }

    fn container_name(session_id: Uuid) -> String {
        format!("agentscommander-{}", session_id.as_simple())
    }

    pub fn build_stop_command(
        &self,
        handle: &ContainerRuntimeHandle,
        timeout: Duration,
    ) -> DockerCommandSpec {
        DockerCommandSpec {
            program: self.program.clone(),
            args: vec![
                "stop".to_string(),
                "--time".to_string(),
                timeout.as_secs().to_string(),
                handle.container_id.clone(),
            ],
            secret_env: BTreeMap::new(),
            redacted_values: Vec::new(),
        }
    }

    pub fn build_force_remove_command(&self, handle: &ContainerRuntimeHandle) -> DockerCommandSpec {
        DockerCommandSpec {
            program: self.program.clone(),
            args: vec![
                "rm".to_string(),
                "-f".to_string(),
                handle.container_id.clone(),
            ],
            secret_env: BTreeMap::new(),
            redacted_values: Vec::new(),
        }
    }

    pub fn build_inspect_state_command(
        &self,
        handle: &ContainerRuntimeHandle,
    ) -> DockerCommandSpec {
        DockerCommandSpec {
            program: self.program.clone(),
            args: vec![
                "inspect".to_string(),
                "--format".to_string(),
                "{{json .State}}".to_string(),
                handle.container_id.clone(),
            ],
            secret_env: BTreeMap::new(),
            redacted_values: Vec::new(),
        }
    }

    pub fn build_logs_command(
        &self,
        handle: &ContainerRuntimeHandle,
        tail_lines: usize,
    ) -> DockerCommandSpec {
        DockerCommandSpec {
            program: self.program.clone(),
            args: vec![
                "logs".to_string(),
                "--tail".to_string(),
                tail_lines.to_string(),
                handle.container_id.clone(),
            ],
            secret_env: BTreeMap::new(),
            redacted_values: Vec::new(),
        }
    }

    pub fn build_list_labeled_command(&self) -> DockerCommandSpec {
        DockerCommandSpec {
            program: self.program.clone(),
            args: vec![
                "ps".to_string(),
                "-a".to_string(),
                "--filter".to_string(),
                format!("label={}", SESSION_LABEL),
                "--format".to_string(),
                format!("{{{{.ID}}}}\t{{{{.Label \"{}\"}}}}", SESSION_LABEL),
            ],
            secret_env: BTreeMap::new(),
            redacted_values: Vec::new(),
        }
    }

    fn run_command_output(&self, spec: DockerCommandSpec) -> Result<DockerCommandOutput, AppError> {
        self.run_command_output_with_control(
            spec,
            &ContainerRuntimeControl::default(),
            DockerCommandShutdownBehavior::ContinueUntilDeadline,
        )
        .map_err(|error| error.source)
    }

    fn run_command_output_with_control(
        &self,
        spec: DockerCommandSpec,
        control: &ContainerRuntimeControl,
        shutdown_behavior: DockerCommandShutdownBehavior,
    ) -> Result<DockerCommandOutput, DockerCommandError> {
        let started_at = Instant::now();
        let default_deadline =
            started_at + DOCKER_COMMAND_TIMEOUT + DOCKER_COMMAND_FINALIZATION_RESERVE;
        let hard_deadline = control
            .shutdown_deadline()
            .map(|deadline| deadline.min(default_deadline))
            .unwrap_or(default_deadline);
        self.run_command_output_with_context(
            spec,
            control,
            shutdown_behavior,
            None,
            "docker-command",
            hard_deadline,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_command_output_with_context(
        &self,
        spec: DockerCommandSpec,
        control: &ContainerRuntimeControl,
        shutdown_behavior: DockerCommandShutdownBehavior,
        session_id: Option<Uuid>,
        reason: &'static str,
        hard_deadline: Instant,
    ) -> Result<DockerCommandOutput, DockerCommandError> {
        self.command_ownership.reap_finished();
        #[cfg(test)]
        if let Some(recorded_commands) = &self.recorded_commands {
            recorded_commands.lock().unwrap().push(spec.clone());
            if let Some(results) = &self.scripted_results {
                if let Some(result) = results.lock().unwrap().pop_front() {
                    return match result {
                        ScriptedDockerCommandResult::Output { stdout, stderr } => {
                            Ok(DockerCommandOutput {
                                stdout: CappedCommandStream {
                                    bytes: stdout,
                                    truncated: false,
                                },
                                stderr: CappedCommandStream {
                                    bytes: stderr,
                                    truncated: false,
                                },
                            })
                        }
                        ScriptedDockerCommandResult::SpawnedError(error) => {
                            Err(DockerCommandError {
                                source: AppError::PtyError(error),
                                spawned: true,
                            })
                        }
                        ScriptedDockerCommandResult::ReaderError(error) => {
                            Err(DockerCommandError {
                                source: AppError::PtyError(format!(
                                    "container runtime command stdout read failed: {error}"
                                )),
                                spawned: true,
                            })
                        }
                    };
                }
            }
            return Ok(DockerCommandOutput::empty());
        }

        if shutdown_behavior == DockerCommandShutdownBehavior::CancelOnShutdown
            && control.shutdown_requested()
        {
            return Err(DockerCommandError {
                source: AppError::PtyError(
                    "container runtime command canceled by shutdown before spawn".to_string(),
                ),
                spawned: false,
            });
        }
        if control
            .shutdown_deadline()
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(DockerCommandError {
                source: AppError::PtyError(
                    "container runtime command skipped after shutdown deadline".to_string(),
                ),
                spawned: false,
            });
        }

        // #992 - AC is a GUI-subsystem process and owns no console. A console-
        // subsystem child spawned without CREATE_NO_WINDOW makes Windows allocate
        // a NEW console for it, which Win11 delegates to Windows Terminal: a
        // visible tab titled with docker.exe's resolved path, black because both
        // pipes are captured. Same idiom as the other production spawn sites (see
        // config/session_context.rs, commands/repos.rs, pty/local_backend.rs).
        // The flag is a no-op when the parent already owns a console, which is why
        // no `cargo test` process can observe it - see tests/spawn_no_window_guard.rs.
        #[cfg(windows)]
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .envs(&spec.secret_env)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let child = cmd.spawn().map_err(|error| DockerCommandError {
            source: AppError::PtyError(format!("container runtime command failed: {error}")),
            spawned: false,
        })?;
        let mut owner = DockerCommandOwner {
            child: Some(child),
            stdout: None,
            stderr: None,
            session_id,
            reason,
            program: spec.program.clone(),
            #[cfg(test)]
            active_child: Some(ActiveDockerChildGuard::new(Arc::clone(
                &self.active_children,
            ))),
        };
        let Some(mut stdout) = owner.child.as_mut().and_then(|child| child.stdout.take()) else {
            self.terminate_or_retain_command(owner, hard_deadline, false);
            return Err(DockerCommandError {
                source: AppError::PtyError(
                    "container runtime command did not expose stdout".to_string(),
                ),
                spawned: true,
            });
        };
        let Some(mut stderr) = owner.child.as_mut().and_then(|child| child.stderr.take()) else {
            self.terminate_or_retain_command(owner, hard_deadline, false);
            return Err(DockerCommandError {
                source: AppError::PtyError(
                    "container runtime command did not expose stderr".to_string(),
                ),
                spawned: true,
            });
        };
        #[cfg(test)]
        let stdout_readers = Arc::clone(&self.active_readers);
        #[cfg(test)]
        let stdout_hang = Arc::clone(&self.hang_readers);
        let stdout_reader = match std::thread::Builder::new()
            .name("ac-docker-stdout".to_string())
            .spawn(move || {
                #[cfg(test)]
                let _active_reader = ActiveDockerReaderGuard::new(stdout_readers);
                let result = read_capped_to_end(&mut stdout, DOCKER_COMMAND_OUTPUT_BYTE_LIMIT);
                #[cfg(test)]
                if stdout_hang.load(std::sync::atomic::Ordering::Acquire) {
                    loop {
                        std::thread::park();
                    }
                }
                result
            }) {
            Ok(reader) => reader,
            Err(error) => {
                self.terminate_or_retain_command(owner, hard_deadline, false);
                return Err(DockerCommandError {
                    source: AppError::PtyError(format!(
                        "container runtime stdout reader spawn failed: {error}"
                    )),
                    spawned: true,
                });
            }
        };
        owner.stdout = Some(stdout_reader);
        #[cfg(test)]
        let stderr_readers = Arc::clone(&self.active_readers);
        #[cfg(test)]
        let stderr_hang = Arc::clone(&self.hang_readers);
        let stderr_reader = match std::thread::Builder::new()
            .name("ac-docker-stderr".to_string())
            .spawn(move || {
                #[cfg(test)]
                let _active_reader = ActiveDockerReaderGuard::new(stderr_readers);
                let result = read_capped_to_end(&mut stderr, DOCKER_COMMAND_OUTPUT_BYTE_LIMIT);
                #[cfg(test)]
                if stderr_hang.load(std::sync::atomic::Ordering::Acquire) {
                    loop {
                        std::thread::park();
                    }
                }
                result
            }) {
            Ok(reader) => reader,
            Err(error) => {
                self.terminate_or_retain_command(owner, hard_deadline, false);
                return Err(DockerCommandError {
                    source: AppError::PtyError(format!(
                        "container runtime stderr reader spawn failed: {error}"
                    )),
                    spawned: true,
                });
            }
        };
        owner.stderr = Some(stderr_reader);

        let started_at = Instant::now();
        let command_deadline = (started_at + DOCKER_COMMAND_TIMEOUT).min(
            hard_deadline
                .checked_sub(
                    DOCKER_COMMAND_FINALIZATION_RESERVE
                        .min(hard_deadline.saturating_duration_since(started_at) / 4),
                )
                .unwrap_or(started_at),
        );
        let status = loop {
            let dynamic_hard_deadline = control
                .shutdown_deadline()
                .map(|deadline| deadline.min(hard_deadline))
                .unwrap_or(hard_deadline);
            let Some(child) = owner.child.as_mut() else {
                let message =
                    "container runtime command child ownership became unavailable".to_string();
                self.command_ownership.retain(owner, Some(message.clone()));
                return Err(DockerCommandError {
                    source: AppError::PtyError(message),
                    spawned: true,
                });
            };
            match child.try_wait() {
                Ok(Some(status)) => {
                    owner.child = None;
                    #[cfg(test)]
                    owner.active_child.take();
                    break status;
                }
                Ok(None) => {
                    let now = Instant::now();
                    let effective_deadline = dynamic_hard_deadline.min(command_deadline);
                    let canceled = shutdown_behavior
                        == DockerCommandShutdownBehavior::CancelOnShutdown
                        && control.shutdown_requested();
                    if canceled || now >= effective_deadline {
                        self.terminate_or_retain_command(owner, dynamic_hard_deadline, canceled);
                        let reason = if canceled {
                            "canceled by shutdown"
                        } else if control.shutdown_requested() {
                            "exceeded the shared shutdown deadline"
                        } else {
                            "timed out"
                        };
                        return Err(DockerCommandError {
                            source: AppError::PtyError(format!(
                                "container runtime command {reason} program={}",
                                spec.program
                            )),
                            spawned: true,
                        });
                    }
                    std::thread::sleep(
                        DOCKER_COMMAND_POLL.min(effective_deadline.saturating_duration_since(now)),
                    );
                }
                Err(error) => {
                    self.terminate_or_retain_command(owner, dynamic_hard_deadline, false);
                    return Err(DockerCommandError {
                        source: AppError::PtyError(format!(
                            "container runtime command wait failed: {error}"
                        )),
                        spawned: true,
                    });
                }
            }
        };

        let dynamic_hard_deadline = control
            .shutdown_deadline()
            .map(|deadline| deadline.min(hard_deadline))
            .unwrap_or(hard_deadline);
        let mut output = match owner.collect_output_until(dynamic_hard_deadline) {
            Some(Ok(output)) => output,
            Some(Err(error)) => {
                return Err(DockerCommandError {
                    source: error,
                    spawned: true,
                });
            }
            None => {
                return Err(self.retain_reader_owner_at_deadline(owner));
            }
        };
        redact_command_bytes(&mut output.stdout.bytes, &spec);
        redact_command_bytes(&mut output.stderr.bytes, &spec);
        let DockerCommandOutput { stdout, stderr } = output;
        if !status.success() {
            let stderr_text = stderr.trimmed_text();
            let stdout_text = stdout.trimmed_text();
            let detail = if stderr_text.trim().is_empty() {
                stdout_text.trim()
            } else {
                stderr_text.trim()
            };
            let detail = redact_command_values(detail, &spec);
            return Err(DockerCommandError {
                source: AppError::PtyError(format!(
                    "container runtime command exited {}: {}",
                    status, detail
                )),
                spawned: true,
            });
        }
        Ok(DockerCommandOutput { stdout, stderr })
    }

    fn terminate_or_retain_command(
        &self,
        mut owner: DockerCommandOwner,
        deadline: Instant,
        canceled: bool,
    ) {
        if !owner.terminate_until(deadline, canceled) {
            let last_error = if canceled {
                "Docker command termination remained unresolved after shutdown cancellation"
            } else {
                "Docker command termination remained unresolved at the absolute deadline"
            };
            self.command_ownership
                .retain(owner, Some(last_error.to_string()));
        }
    }

    fn retain_reader_owner_at_deadline(&self, owner: DockerCommandOwner) -> DockerCommandError {
        let program = owner.program.clone();
        let message = format!("readers exceeded the absolute deadline program={program}");
        self.command_ownership.retain(owner, Some(message.clone()));
        DockerCommandError {
            source: AppError::PtyError(message),
            spawned: true,
        }
    }

    fn run_command(&self, spec: DockerCommandSpec) -> Result<String, AppError> {
        let output = self.run_command_output(spec)?;
        Ok(String::from_utf8_lossy(&output.stdout.bytes)
            .trim()
            .to_string())
    }

    fn run_command_with_control(
        &self,
        spec: DockerCommandSpec,
        control: &ContainerRuntimeControl,
        shutdown_behavior: DockerCommandShutdownBehavior,
        session_id: Uuid,
        reason: &'static str,
        hard_deadline: Instant,
    ) -> Result<String, DockerCommandError> {
        let output = self.run_command_output_with_context(
            spec,
            control,
            shutdown_behavior,
            Some(session_id),
            reason,
            hard_deadline,
        )?;
        Ok(String::from_utf8_lossy(&output.stdout.bytes)
            .trim()
            .to_string())
    }

    fn parse_labeled_containers(raw: &str) -> Vec<(String, String)> {
        raw.lines()
            .filter_map(|line| {
                let (id, label) = line.split_once('\t')?;
                let id = id.trim();
                let label = label.trim();
                if id.is_empty() || label.is_empty() {
                    None
                } else {
                    Some((id.to_string(), label.to_string()))
                }
            })
            .collect()
    }

    fn parse_container_state(raw: &str) -> Result<ContainerStateSnapshot, AppError> {
        #[derive(serde::Deserialize)]
        struct DockerState {
            #[serde(rename = "Status")]
            status: Option<String>,
            #[serde(rename = "Running")]
            running: Option<bool>,
            #[serde(rename = "ExitCode")]
            exit_code: Option<i64>,
            #[serde(rename = "Error")]
            error: Option<String>,
        }

        let state: DockerState = serde_json::from_str(raw)
            .map_err(|e| AppError::PtyError(format!("container state JSON parse failed: {e}")))?;
        Ok(ContainerStateSnapshot {
            status: state.status,
            running: state.running,
            exit_code: state.exit_code,
            error: state.error,
        })
    }

    fn combined_output_text(output: DockerCommandOutput) -> Option<String> {
        let stdout = output.stdout.trim_end_text();
        let stderr = output.stderr.trim_end_text();
        let stdout = stdout.trim_end();
        let stderr = stderr.trim_end();
        match (stdout.is_empty(), stderr.is_empty()) {
            (true, true) => None,
            (false, true) => Some(stdout.to_string()),
            (true, false) => Some(stderr.to_string()),
            (false, false) => Some(format!("{stdout}\n{stderr}")),
        }
    }

    fn valid_container_id(value: &str) -> bool {
        (12..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    }

    fn retain_ambiguous_start(
        &self,
        handle: ContainerRuntimeHandle,
    ) -> Arc<RetainedDockerStartCleanup> {
        let mut entries = self
            .retained_start_cleanups
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(existing) = entries.get(&handle.session_id) {
            return Arc::clone(existing);
        }
        let entry = Arc::new(RetainedDockerStartCleanup {
            handle,
            state: Mutex::new(RetainedDockerStartCleanupState::default()),
        });
        entries.insert(entry.handle.session_id, Arc::clone(&entry));
        entry
    }

    fn attempt_retained_start_cleanup(
        &self,
        entry: Arc<RetainedDockerStartCleanup>,
        control: &ContainerRuntimeControl,
    ) {
        {
            let mut state = entry
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.in_flight {
                return;
            }
            state.in_flight = true;
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.stop(&entry.handle, CONTAINER_STOP_TIMEOUT, control)
        }));
        match result {
            Ok(Ok(())) => {
                let mut entries = self
                    .retained_start_cleanups
                    .entries
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if entries
                    .get(&entry.handle.session_id)
                    .is_some_and(|owned| Arc::ptr_eq(owned, &entry))
                {
                    entries.remove(&entry.handle.session_id);
                }
                let mut state = entry
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                state.in_flight = false;
                state.last_error = None;
                log::debug!(
                    "[container-runtime] ambiguous Docker start cleanup terminal session={} state=terminal",
                    entry.handle.session_id
                );
            }
            Ok(Err(error)) => {
                let message = normalized_app_error_text(&error);
                let mut state = entry
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.in_flight = false;
                state.last_error = Some(message.clone());
                log::warn!(
                    "[container-runtime] ambiguous Docker start cleanup retained session={} state=retained error={}",
                    entry.handle.session_id,
                    crate::pty::container_runtime::redact_container_diagnostic_text(&message)
                );
            }
            Err(_) => {
                let mut state = entry
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.in_flight = false;
                state.last_error = Some("cleanup panicked".to_string());
                log::error!(
                    "[container-runtime] ambiguous Docker start cleanup panicked session={} state=retained",
                    entry.handle.session_id
                );
            }
        }
    }

    fn ambiguous_start_error(
        &self,
        primary: AppError,
        cleanup_handle: ContainerRuntimeHandle,
        deadline: Instant,
    ) -> AppError {
        let session_id = cleanup_handle.session_id;
        let entry = self.retain_ambiguous_start(cleanup_handle);
        let cleanup_control = ContainerRuntimeControl::default();
        cleanup_control.request_shutdown(deadline);
        self.attempt_retained_start_cleanup(entry, &cleanup_control);
        let retained_entry = self
            .retained_start_cleanups
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&session_id)
            .cloned();
        let cleanup_error = retained_entry.as_ref().and_then(|entry| {
            entry
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .last_error
                .as_deref()
                .map(crate::pty::container_runtime::redact_container_diagnostic_text)
        });
        let retained = retained_entry.is_some();
        let primary_text = crate::pty::container_runtime::redact_container_diagnostic_text(
            &normalized_app_error_text(&primary),
        );
        AppError::Other(format!(
            "{}; deterministic cleanup session={} state={}{}",
            primary_text,
            session_id,
            if retained { "retained" } else { "terminal" },
            cleanup_error
                .map(|error| format!(" cleanupError={error}"))
                .unwrap_or_default()
        ))
    }
}

fn join_command_readers(
    stdout: JoinHandle<std::io::Result<CappedCommandStream>>,
    stderr: JoinHandle<std::io::Result<CappedCommandStream>>,
) -> Result<DockerCommandOutput, AppError> {
    let stdout = join_command_reader(stdout, "stdout")?;
    let stderr = join_command_reader(stderr, "stderr")?;
    Ok(DockerCommandOutput { stdout, stderr })
}

fn join_command_reader(
    reader: JoinHandle<std::io::Result<CappedCommandStream>>,
    stream_name: &'static str,
) -> Result<CappedCommandStream, AppError> {
    reader
        .join()
        .map_err(|_| {
            AppError::PtyError(format!(
                "container runtime command {stream_name} reader panicked"
            ))
        })?
        .map_err(|error| {
            AppError::PtyError(format!(
                "container runtime command {stream_name} read failed: {error}"
            ))
        })
}

#[cfg(test)]
struct ActiveDockerChildGuard {
    active: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl ActiveDockerChildGuard {
    fn new(active: Arc<std::sync::atomic::AtomicUsize>) -> Self {
        active.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self { active }
    }
}

#[cfg(test)]
impl Drop for ActiveDockerChildGuard {
    fn drop(&mut self) {
        self.active
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
struct ActiveDockerReaderGuard {
    active: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl ActiveDockerReaderGuard {
    fn new(active: Arc<std::sync::atomic::AtomicUsize>) -> Self {
        active.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self { active }
    }
}

#[cfg(test)]
impl Drop for ActiveDockerReaderGuard {
    fn drop(&mut self) {
        self.active
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl ContainerRuntime for DockerRuntime {
    fn start(
        &self,
        request: ContainerStartRequest,
        control: &ContainerRuntimeControl,
    ) -> Result<ContainerRuntimeHandle, AppError> {
        let session_id = request.session_id;
        let cleanup_handle = ContainerRuntimeHandle {
            session_id,
            container_id: Self::container_name(session_id),
        };
        let deadline = control.shutdown_deadline().unwrap_or_else(|| {
            Instant::now() + DOCKER_COMMAND_TIMEOUT + DOCKER_COMMAND_FINALIZATION_RESERVE
        });
        let command = self.build_run_command(&request)?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.run_command_with_control(
                command,
                control,
                DockerCommandShutdownBehavior::CancelOnShutdown,
                session_id,
                "start",
                deadline,
            )
        }));
        match result {
            Ok(Ok(stdout)) => {
                let container_id = stdout.lines().next().map(str::trim).unwrap_or_default();
                if Self::valid_container_id(container_id) {
                    return Ok(ContainerRuntimeHandle {
                        session_id,
                        container_id: container_id.to_string(),
                    });
                }
                let primary = if container_id.is_empty() {
                    AppError::PtyError(
                        "container runtime returned an empty container id".to_string(),
                    )
                } else {
                    AppError::PtyError(
                        "container runtime returned a malformed container id".to_string(),
                    )
                };
                Err(self.ambiguous_start_error(primary, cleanup_handle, deadline))
            }
            Ok(Err(error)) if control.shutdown_requested() => {
                log::debug!(
                    "[container-runtime] Docker start canceled session={}; returning owned cleanup handle: {}",
                    session_id,
                    error
                );
                Ok(cleanup_handle)
            }
            Ok(Err(error)) if error.spawned => {
                Err(self.ambiguous_start_error(error.source, cleanup_handle, deadline))
            }
            Ok(Err(error)) => Err(error.source),
            Err(_) => Err(self.ambiguous_start_error(
                AppError::PtyError(
                    "container runtime start panicked after Docker command ownership began"
                        .to_string(),
                ),
                cleanup_handle,
                deadline,
            )),
        }
    }

    fn stop(
        &self,
        handle: &ContainerRuntimeHandle,
        timeout: Duration,
        control: &ContainerRuntimeControl,
    ) -> Result<(), AppError> {
        if !control.shutdown_requested() {
            if let Err(stop_err) = self.run_command_with_control(
                self.build_stop_command(handle, timeout),
                control,
                DockerCommandShutdownBehavior::CancelOnShutdown,
                handle.session_id,
                "stop",
                control
                    .shutdown_deadline()
                    .unwrap_or_else(|| Instant::now() + timeout),
            ) {
                if control.shutdown_requested() {
                    log::debug!(
                        "[container-runtime] graceful stop interrupted by shutdown session={}: {}",
                        handle.session_id,
                        stop_err
                    );
                } else {
                    log::warn!(
                        "[container-runtime] graceful stop failed for session {}: {}",
                        handle.session_id,
                        stop_err
                    );
                }
            }
        }
        self.run_command_with_control(
            self.build_force_remove_command(handle),
            control,
            DockerCommandShutdownBehavior::ContinueUntilDeadline,
            handle.session_id,
            "force-remove",
            control
                .shutdown_deadline()
                .unwrap_or_else(|| Instant::now() + timeout),
        )
        .map(|_| ())
        .map_err(|error| error.source)
    }

    fn diagnostics(
        &self,
        handle: &ContainerRuntimeHandle,
        log_tail_lines: usize,
    ) -> ContainerDiagnostics {
        let (state, inspect_error) =
            match self.run_command(self.build_inspect_state_command(handle)) {
                Ok(raw) => match Self::parse_container_state(&raw) {
                    Ok(state) => (Some(state), None),
                    Err(err) => (None, Some(err.to_string())),
                },
                Err(err) => (None, Some(err.to_string())),
            };
        let (log_tail, logs_error) =
            match self.run_command_output(self.build_logs_command(handle, log_tail_lines)) {
                Ok(output) => (Self::combined_output_text(output), None),
                Err(err) => (None, Some(err.to_string())),
            };
        ContainerDiagnostics {
            container_id: handle.container_id.clone(),
            state,
            inspect_error,
            log_tail,
            logs_error,
        }
    }

    fn cleanup_labeled_orphans(
        &self,
        live_sessions: &HashSet<Uuid>,
        timeout: Duration,
    ) -> Result<ContainerCleanupReport, AppError> {
        let stdout = self.run_command(self.build_list_labeled_command())?;
        let mut report = ContainerCleanupReport::default();
        let mut errors = Vec::new();
        let control = ContainerRuntimeControl::default();
        for (container_id, label) in Self::parse_labeled_containers(&stdout) {
            let Ok(session_id) = Uuid::parse_str(&label) else {
                report.invalid_labels.push(label);
                continue;
            };
            if live_sessions.contains(&session_id) {
                report.skipped_live.push(session_id);
                continue;
            }
            let handle = ContainerRuntimeHandle {
                session_id,
                container_id,
            };
            match self.stop(&handle, timeout, &control) {
                Ok(()) => report.stopped.push(session_id),
                // #992 - no warn! here on purpose. The same `session_id: err` string is
                // returned in the aggregate Err below, which the backend logs at the
                // severity its sweep posture calls for. Warning here as well would fire
                // on every opportunistic pass, which is the noise #992 is about.
                Err(err) => errors.push(format!("{}: {}", session_id, err)),
            }
        }
        if !errors.is_empty() {
            return Err(AppError::PtyError(format!(
                "failed to clean {} labeled orphan container(s): {}",
                errors.len(),
                errors.join("; ")
            )));
        }
        Ok(report)
    }

    fn retry_retained_cleanups(&self, control: &ContainerRuntimeControl) {
        let deadline = control
            .shutdown_deadline()
            .unwrap_or_else(|| Instant::now() + CONTAINER_STOP_TIMEOUT);
        let entries = self
            .retained_start_cleanups
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for entry in entries {
            if Instant::now() >= deadline {
                break;
            }
            self.attempt_retained_start_cleanup(entry, control);
        }
        if Instant::now() < deadline {
            self.command_ownership.retry_until(deadline);
        }
    }

    fn retained_cleanup_contexts(&self) -> Vec<RetainedContainerOwnerContext> {
        let mut contexts = self
            .retained_start_cleanups
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .map(|entry| {
                let state = entry
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                RetainedContainerOwnerContext {
                    owner: "ambiguousStartCleanup",
                    session_id: Some(entry.handle.session_id),
                    reason: "ambiguous-start".to_string(),
                    program: None,
                    runtime_handle: Some(true),
                    state: if state.in_flight {
                        "inFlight"
                    } else {
                        "retained"
                    },
                    in_flight: state.in_flight,
                    last_error: state.last_error.clone(),
                }
            })
            .collect::<Vec<_>>();
        contexts.extend(self.command_ownership.retained_contexts());
        contexts.sort_by_key(RetainedContainerOwnerContext::diagnostic);
        contexts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::container_runtime::{
        ContainerRuntime, ContainerStartRequest, DEFAULT_API_HELPER_PATH,
        DEFAULT_CONTAINER_WORKDIR, SESSION_LABEL,
    };
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    fn request() -> ContainerStartRequest {
        ContainerStartRequest {
            session_id: Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
            image: "ac-bridge:test".to_string(),
            host_root: "C:/project/.ac/wg-1-team/__agent_dev".to_string(),
            container_workdir: DEFAULT_CONTAINER_WORKDIR.to_string(),
            api_url: "http://host.docker.internal:8765".to_string(),
            api_token: "api-secret".to_string(),
            registration_ticket: "ticket-secret".to_string(),
            local_dir: ".agentscommander_ac".to_string(),
            command: "codex".to_string(),
            args: vec!["--version".to_string()],
            child_env: vec![("CODEX_HOME".to_string(), "/workspace/.codex".to_string())],
            env_unset: vec!["CLAUDE_CONFIG_DIR".to_string()],
            cols: 120,
            rows: 30,
            repo_mounts: Vec::new(),
        }
    }

    #[test]
    fn run_command_is_detached_labeled_and_has_expected_env_without_interactive_flags() {
        let runtime = DockerRuntime::with_program("docker-test");
        let spec = runtime.build_run_command(&request()).unwrap();
        let joined = spec.args.join(" ");

        assert_eq!(spec.program, "docker-test");
        assert!(spec.args.iter().any(|arg| arg == "-d"));
        assert!(joined.contains("--name agentscommander-11111111111141118111111111111111"));
        assert!(!spec.args.iter().any(|arg| arg == "--rm"));
        assert!(!spec
            .args
            .iter()
            .any(|arg| arg == "-it" || arg == "-i" || arg == "-t"));
        assert!(joined.contains(&format!(
            "{}=11111111-1111-4111-8111-111111111111",
            SESSION_LABEL
        )));
        assert!(joined.contains("AGENTSCOMMANDER_API_URL=http://host.docker.internal:8765"));
        assert!(joined.contains("--env AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN"));
        assert!(joined.contains("--env AGENTSCOMMANDER_API_TOKEN"));
        assert!(joined.contains("--env AGENTSCOMMANDER_ROOT"));
        assert!(!joined.contains("AGENTSCOMMANDER_ROOT=C:/project"));
        assert!(!joined.contains("ticket-secret"));
        assert!(!joined.contains("api-secret"));
        assert_eq!(
            spec.secret_env
                .get("AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN")
                .map(String::as_str),
            Some("ticket-secret")
        );
        assert_eq!(
            spec.secret_env
                .get("AGENTSCOMMANDER_API_TOKEN")
                .map(String::as_str),
            Some("api-secret")
        );
        assert_eq!(
            spec.secret_env
                .get("AGENTSCOMMANDER_ROOT")
                .map(String::as_str),
            Some("C:/project/.ac/wg-1-team/__agent_dev")
        );
        assert!(joined.contains(&format!(
            "AGENTSCOMMANDER_BINARY_PATH={DEFAULT_API_HELPER_PATH}"
        )));
        assert!(joined.contains("--env AGENTSCOMMANDER_BRIDGE_COMMAND"));
        assert!(joined.contains("--env AGENTSCOMMANDER_BRIDGE_ARGS_JSON"));
        assert!(joined.contains("--env AGENTSCOMMANDER_BRIDGE_ENV_JSON"));
        assert!(!joined.contains("AGENTSCOMMANDER_BRIDGE_COMMAND=codex"));
        assert!(!joined.contains("CODEX_HOME"));
        assert_eq!(
            spec.secret_env
                .get("AGENTSCOMMANDER_BRIDGE_COMMAND")
                .map(String::as_str),
            Some("codex")
        );
        assert_eq!(
            spec.secret_env
                .get("AGENTSCOMMANDER_BRIDGE_ARGS_JSON")
                .map(String::as_str),
            Some("[\"--version\"]")
        );
        assert_eq!(
            spec.secret_env
                .get("AGENTSCOMMANDER_BRIDGE_ENV_JSON")
                .map(String::as_str),
            Some("[[\"CODEX_HOME\",\"/workspace/.codex\"]]")
        );
        assert!(joined.contains("AGENTSCOMMANDER_BRIDGE_ENV_UNSET_JSON=[\"CLAUDE_CONFIG_DIR\"]"));
        assert!(joined
            .contains("type=bind,source=C:/project/.ac/wg-1-team/__agent_dev,target=/workspace"));
        let image_idx = spec
            .args
            .iter()
            .position(|arg| arg == "ac-bridge:test")
            .expect("image arg");
        assert_eq!(spec.args[image_idx - 1], "--");
        assert_eq!(spec.args[image_idx + 1], DEFAULT_BRIDGE_ENTRYPOINT);
        assert!(!joined.contains("docker.sock"));
        assert!(!joined.contains("messaging"));
        assert!(!joined.contains("api-clients.json"));
    }

    #[test]
    fn docker_command_debug_and_captured_output_redact_commands_paths_and_secret_values() {
        let runtime = DockerRuntime::with_program("docker-secret-program");
        let spec = runtime.build_run_command(&request()).unwrap();
        let rendered = format!("{spec:?}");
        for sentinel in [
            "docker-secret-program",
            "C:/project",
            "codex",
            "api-secret",
            "ticket-secret",
        ] {
            assert!(!rendered.contains(sentinel), "debug leaked {sentinel}");
        }
        assert!(rendered.contains("AGENTSCOMMANDER_API_TOKEN"));
        assert!(rendered.contains("AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN"));

        let mut bytes = b"prefix api-secret middle ticket-secret path C:/project/.ac/wg-1-team/__agent_dev suffix".to_vec();
        redact_command_bytes(&mut bytes, &spec);
        let output = String::from_utf8(bytes).unwrap();
        assert_eq!(
            output,
            "prefix [REDACTED] middle [REDACTED] path [REDACTED] suffix"
        );
    }

    #[test]
    fn run_command_terminates_options_before_image() {
        let runtime = DockerRuntime::with_program("docker-test");
        let mut request = request();
        request.image = "--privileged".to_string();
        let spec = runtime.build_run_command(&request).unwrap();
        let image_idx = spec
            .args
            .iter()
            .position(|arg| arg == "--privileged")
            .expect("image arg");

        assert_eq!(spec.args[image_idx - 1], "--");
        assert_eq!(spec.args[image_idx + 1], DEFAULT_BRIDGE_ENTRYPOINT);
    }

    #[test]
    fn run_command_appends_read_write_repo_mounts_before_image() {
        // #935 - each repo renders as its own --mount, AFTER the replica mount
        // and BEFORE the -- / image / entrypoint tail. If the loop were appended
        // after the tail, image_idx + 1 would be --mount and this test fails.
        use crate::pty::container_repos::ContainerRepoMount;
        let runtime = DockerRuntime::with_program("docker-test");
        let mut req = request();
        req.repo_mounts = vec![
            ContainerRepoMount {
                host_path: std::path::PathBuf::from(
                    "C:/project/.ac/wg-1-team/repo-AgentsCommander",
                ),
                container_path: "/repos/repo-AgentsCommander".to_string(),
            },
            ContainerRepoMount {
                host_path: std::path::PathBuf::from("C:/project/.ac/wg-1-team/repo-webpage"),
                container_path: "/repos/repo-webpage".to_string(),
            },
        ];
        let spec = runtime.build_run_command(&req).unwrap();
        let joined = spec.args.join(" ");

        // Replica mount plus the two repos = three --mount flags.
        assert_eq!(spec.args.iter().filter(|a| *a == "--mount").count(), 3);
        let replica = spec
            .args
            .iter()
            .position(|a| a.contains("target=/workspace"))
            .expect("replica mount");
        let repo1 = spec
            .args
            .iter()
            .position(|a| a.contains("target=/repos/repo-AgentsCommander"))
            .expect("repo1 mount");
        let repo2 = spec
            .args
            .iter()
            .position(|a| a.contains("target=/repos/repo-webpage"))
            .expect("repo2 mount");
        // Order: replica first, then repos in config order.
        assert!(replica < repo1 && repo1 < repo2);
        assert!(joined.contains(
            "type=bind,source=C:/project/.ac/wg-1-team/repo-AgentsCommander,target=/repos/repo-AgentsCommander"
        ));
        // Read-write: no readonly token on the repo mounts (Q2 = RW).
        assert!(!joined.contains("readonly"));
        // The workdir stays the replica.
        let workdir_idx = spec
            .args
            .iter()
            .position(|a| a == "--workdir")
            .expect("workdir");
        assert_eq!(spec.args[workdir_idx + 1], "/workspace");
        // The repo mounts precede the image; the -- / entrypoint tail is intact.
        let image_idx = spec
            .args
            .iter()
            .position(|a| a == "ac-bridge:test")
            .expect("image arg");
        assert!(repo2 < image_idx, "repo mounts must precede the image");
        assert_eq!(spec.args[image_idx - 1], "--");
        assert_eq!(spec.args[image_idx + 1], DEFAULT_BRIDGE_ENTRYPOINT);
        // #993 - the resolver strips the Windows verbatim prefix, so the
        // rendered --mount args never carry \\?\ (dockerd rejects such a source).
        assert!(!joined.contains(r"\\?\"), "{joined}");
        // S7 free regression guard, now exercised WITH repo mounts.
        assert!(!joined.contains("messaging"));
    }

    #[test]
    fn stop_and_orphan_list_commands_are_label_based() {
        let runtime = DockerRuntime::with_program("docker-test");
        let handle = ContainerRuntimeHandle {
            session_id: Uuid::nil(),
            container_id: "abc123".to_string(),
        };
        let stop = runtime.build_stop_command(&handle, Duration::from_secs(5));
        assert_eq!(stop.args, vec!["stop", "--time", "5", "abc123"]);

        let inspect = runtime.build_inspect_state_command(&handle);
        assert_eq!(
            inspect.args,
            vec!["inspect", "--format", "{{json .State}}", "abc123"]
        );

        let logs = runtime.build_logs_command(&handle, 80);
        assert_eq!(logs.args, vec!["logs", "--tail", "80", "abc123"]);

        let list = runtime.build_list_labeled_command();
        assert_eq!(list.args[0], "ps");
        assert!(list.args.contains(&format!("label={}", SESSION_LABEL)));
    }

    #[test]
    fn stop_runs_force_remove_after_successful_stop() {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let runtime = DockerRuntime::with_recorded_commands("docker-test", recorded.clone());
        let handle = ContainerRuntimeHandle {
            session_id: Uuid::nil(),
            container_id: "abc123".to_string(),
        };

        runtime
            .stop(
                &handle,
                Duration::from_secs(5),
                &ContainerRuntimeControl::default(),
            )
            .unwrap();

        let commands = recorded.lock().unwrap().clone();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].args, vec!["stop", "--time", "5", "abc123"]);
        assert_eq!(commands[1].args, vec!["rm", "-f", "abc123"]);
    }

    #[test]
    fn ambiguous_start_outcomes_cleanup_the_deterministic_name() {
        let cases = vec![
            (
                "timeout",
                ScriptedDockerCommandResult::SpawnedError("primary timeout".to_string()),
                "primary timeout",
            ),
            (
                "command-error",
                ScriptedDockerCommandResult::SpawnedError("primary command error".to_string()),
                "primary command error",
            ),
            (
                "reader-error",
                ScriptedDockerCommandResult::ReaderError("primary reader error".to_string()),
                "primary reader error",
            ),
            (
                "empty-output",
                ScriptedDockerCommandResult::Output {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                },
                "empty container id",
            ),
            (
                "malformed-output",
                ScriptedDockerCommandResult::Output {
                    stdout: b"not-a-container-id\n".to_vec(),
                    stderr: Vec::new(),
                },
                "malformed container id",
            ),
        ];

        for (case, start_result, primary) in cases {
            let recorded = Arc::new(Mutex::new(Vec::new()));
            let runtime = DockerRuntime::with_scripted_commands(
                "docker-test",
                Arc::clone(&recorded),
                vec![
                    start_result,
                    ScriptedDockerCommandResult::Output {
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                    },
                ],
            );

            let error = runtime
                .start(request(), &ContainerRuntimeControl::default())
                .expect_err("ambiguous start must preserve the primary failure")
                .to_string();
            assert!(error.contains(primary), "case={case} error={error}");
            assert!(
                error.contains("state=terminal"),
                "case={case} error={error}"
            );
            assert!(runtime.retained_cleanup_contexts().is_empty(), "{case}");

            let commands = recorded
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            assert_eq!(commands.len(), 2, "{case}");
            assert_eq!(commands[0].args.first().map(String::as_str), Some("run"));
            assert_eq!(
                commands[1].args,
                vec![
                    "rm",
                    "-f",
                    "agentscommander-11111111111141118111111111111111"
                ],
                "{case}"
            );
        }
    }

    #[test]
    fn ambiguous_start_cleanup_failure_is_retained_and_retried_once() {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let runtime = DockerRuntime::with_scripted_commands(
            "docker-test",
            Arc::clone(&recorded),
            vec![
                ScriptedDockerCommandResult::SpawnedError("primary start timeout".to_string()),
                ScriptedDockerCommandResult::SpawnedError(
                    "cleanup denied OPENAI_API_KEY=sk-proj-super-secret".to_string(),
                ),
                ScriptedDockerCommandResult::Output {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                },
            ],
        );
        let session_id = request().session_id;

        let error = runtime
            .start(request(), &ContainerRuntimeControl::default())
            .expect_err("failed deterministic cleanup keeps the start failed")
            .to_string();
        assert!(error.starts_with("primary start timeout;"), "{error}");
        assert!(error.contains("state=retained"), "{error}");
        assert!(
            error.contains("cleanupError=cleanup denied OPENAI_API_KEY=[REDACTED]"),
            "{error}"
        );
        assert!(!error.contains("sk-proj-super-secret"), "{error}");
        let contexts = runtime.retained_cleanup_contexts();
        assert_eq!(contexts.len(), 1);
        let context = &contexts[0];
        assert_eq!(context.owner, "ambiguousStartCleanup");
        assert_eq!(context.session_id, Some(session_id));
        assert_eq!(context.reason, "ambiguous-start");
        assert_eq!(context.program, None);
        assert_eq!(context.runtime_handle, Some(true));
        assert_eq!(context.state, "retained");
        assert!(!context.in_flight);
        assert!(context
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("sk-proj-super-secret")));
        let diagnostic = context.diagnostic();
        assert!(diagnostic.contains("owner=ambiguousStartCleanup"));
        assert!(diagnostic.contains("state=retained"));
        assert!(diagnostic.contains("lastError=cleanup denied"));
        assert!(diagnostic.contains("OPENAI_API_KEY=[REDACTED]"));
        assert!(!diagnostic.contains("sk-proj-super-secret"));

        let retry_control = ContainerRuntimeControl::default();
        retry_control.request_shutdown(Instant::now() + Duration::from_secs(1));
        runtime.retry_retained_cleanups(&retry_control);
        assert!(runtime.retained_cleanup_contexts().is_empty());
        assert_eq!(
            recorded
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len(),
            3
        );

        runtime.retry_retained_cleanups(&retry_control);
        assert_eq!(
            recorded
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len(),
            3,
            "a terminal deterministic target must not be stopped twice"
        );
    }

    #[test]
    fn permanently_blocked_command_readers_are_retained_at_the_deadline() {
        let runtime = DockerRuntime::new();
        let ready = Arc::new(std::sync::Barrier::new(3));
        let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let spawn_reader = |name: &'static str| {
            let ready = Arc::clone(&ready);
            let release = Arc::clone(&release);
            let active = Arc::clone(&runtime.active_readers);
            std::thread::Builder::new()
                .name(name.to_string())
                .spawn(move || {
                    let _active = ActiveDockerReaderGuard::new(active);
                    ready.wait();
                    let (released, changed) = &*release;
                    let mut released = released.lock().unwrap_or_else(|error| error.into_inner());
                    while !*released {
                        released = changed
                            .wait(released)
                            .unwrap_or_else(|error| error.into_inner());
                    }
                    Ok(CappedCommandStream::default())
                })
                .expect("spawn deterministic blocked Docker reader")
        };
        let mut owner = DockerCommandOwner {
            child: None,
            stdout: Some(spawn_reader("blocked-docker-stdout")),
            stderr: Some(spawn_reader("blocked-docker-stderr")),
            session_id: None,
            reason: "docker-command-reader",
            program: "docker-reader-fixture".to_string(),
            active_child: None,
        };
        ready.wait();
        assert_eq!(runtime.active_reader_count(), 2);

        let budget = Duration::from_millis(300);
        let started_at = Instant::now();
        assert!(owner.collect_output_until(started_at + budget).is_none());
        let error = runtime.retain_reader_owner_at_deadline(owner).to_string();
        let elapsed = started_at.elapsed();

        assert!(
            error.contains("readers exceeded the absolute deadline"),
            "{error}"
        );
        assert!(
            elapsed <= budget + Duration::from_millis(300),
            "blocked reader return exceeded its absolute deadline: {elapsed:?}"
        );
        assert_eq!(runtime.active_child_count(), 0);
        assert_eq!(runtime.active_reader_count(), 2);
        let contexts = runtime.retained_cleanup_contexts();
        assert_eq!(contexts.len(), 1);
        let context = &contexts[0];
        assert_eq!(context.owner, "dockerCommand");
        assert_eq!(context.session_id, None);
        assert_eq!(context.reason, "docker-command-reader");
        assert_eq!(context.program.as_deref(), Some("docker-reader-fixture"));
        assert_eq!(context.runtime_handle, None);
        assert_eq!(context.state, "retained");
        assert!(!context.in_flight);
        assert_eq!(
            context.last_error.as_deref(),
            Some("readers exceeded the absolute deadline program=docker-reader-fixture")
        );
        let diagnostic = context.diagnostic();
        assert!(diagnostic.contains("owner=dockerCommand"));
        assert!(diagnostic.contains("session=none"));
        assert!(diagnostic.contains("reason=docker-command-reader"));
        assert!(diagnostic.contains("program=docker-reader-fixture"));
        assert!(diagnostic.contains(
            "lastError=readers exceeded the absolute deadline program=docker-reader-fixture"
        ));
        assert!(!diagnostic.contains("00000000-0000-0000-0000-000000000000"));

        let (released, changed) = &*release;
        *released.lock().unwrap_or_else(|error| error.into_inner()) = true;
        changed.notify_all();
        runtime
            .command_ownership
            .retry_until(Instant::now() + Duration::from_secs(1));
        assert_eq!(runtime.active_reader_count(), 0);
        assert!(runtime.retained_cleanup_contexts().is_empty());
    }

    #[test]
    fn shutdown_control_kills_and_reaps_command_child_before_deadline() {
        let runtime = Arc::new(DockerRuntime::new());
        let control = ContainerRuntimeControl::default();
        #[cfg(windows)]
        let spec = DockerCommandSpec {
            program: "powershell.exe".to_string(),
            args: vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                "Start-Sleep -Seconds 30".to_string(),
            ],
            secret_env: BTreeMap::new(),
            redacted_values: Vec::new(),
        };
        #[cfg(not(windows))]
        let spec = DockerCommandSpec {
            program: "sleep".to_string(),
            args: vec!["30".to_string()],
            secret_env: BTreeMap::new(),
            redacted_values: Vec::new(),
        };
        let worker_runtime = Arc::clone(&runtime);
        let worker_control = control.clone();
        let command = std::thread::Builder::new()
            .name("docker-runtime-cancellation-test".to_string())
            .spawn(move || {
                worker_runtime.run_command_output_with_control(
                    spec,
                    &worker_control,
                    DockerCommandShutdownBehavior::CancelOnShutdown,
                )
            })
            .expect("spawn Docker cancellation test command");

        let child_deadline = Instant::now() + Duration::from_secs(5);
        while runtime.active_child_count() == 0 {
            assert!(
                Instant::now() < child_deadline,
                "Docker cancellation test child did not start"
            );
            std::thread::yield_now();
        }
        let shutdown_started = Instant::now();
        let shutdown_deadline = shutdown_started + Duration::from_millis(500);
        control.request_shutdown(shutdown_deadline);
        let error = command
            .join()
            .expect("join Docker cancellation test command")
            .expect_err("shutdown cancels the blocking command")
            .to_string();
        assert!(error.contains("canceled by shutdown"), "{error}");
        assert!(
            shutdown_started.elapsed() <= Duration::from_secs(1),
            "Docker child cancellation exceeded its deadline bound"
        );
        assert_eq!(runtime.active_child_count(), 0);
        assert_eq!(runtime.active_reader_count(), 0);
    }

    #[test]
    fn read_capped_to_end_retains_limit_and_reports_truncation() {
        let oversized = vec![b'x'; DOCKER_COMMAND_OUTPUT_BYTE_LIMIT + 17];
        let capped =
            read_capped_to_end(Cursor::new(oversized), DOCKER_COMMAND_OUTPUT_BYTE_LIMIT).unwrap();
        assert_eq!(capped.bytes.len(), DOCKER_COMMAND_OUTPUT_BYTE_LIMIT);
        assert!(capped.truncated);

        let exact = vec![b'y'; DOCKER_COMMAND_OUTPUT_BYTE_LIMIT];
        let uncapped =
            read_capped_to_end(Cursor::new(exact), DOCKER_COMMAND_OUTPUT_BYTE_LIMIT).unwrap();
        assert_eq!(uncapped.bytes.len(), DOCKER_COMMAND_OUTPUT_BYTE_LIMIT);
        assert!(!uncapped.truncated);
    }

    #[test]
    fn parse_labeled_containers_ignores_malformed_rows() {
        let rows = "abc\t11111111-1111-4111-8111-111111111111\nbad-row\n\tmissing\n";
        assert_eq!(
            DockerRuntime::parse_labeled_containers(rows),
            vec![(
                "abc".to_string(),
                "11111111-1111-4111-8111-111111111111".to_string()
            )]
        );
    }

    #[test]
    fn parse_container_state_reads_docker_inspect_state_json() {
        let raw = r#"{"Status":"exited","Running":false,"ExitCode":127,"Error":""}"#;
        let state = DockerRuntime::parse_container_state(raw).unwrap();

        assert_eq!(state.status.as_deref(), Some("exited"));
        assert_eq!(state.running, Some(false));
        assert_eq!(state.exit_code, Some(127));
        assert_eq!(state.error.as_deref(), Some(""));
    }
}
