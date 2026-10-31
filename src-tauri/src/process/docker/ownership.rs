use std::collections::BTreeMap;
use std::io::Read;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::errors::AppError;

pub(crate) const DOCKER_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const DOCKER_COMMAND_FINALIZATION_RESERVE: Duration = Duration::from_secs(5);
pub(crate) const DOCKER_COMMAND_POLL: Duration = Duration::from_millis(10);
pub(crate) const DOCKER_COMMAND_OUTPUT_BYTE_LIMIT: usize = 64 * 1024;

pub(crate) fn redact_command_values(input: &str, spec: &DockerCommandSpec) -> String {
    spec.secret_env
        .values()
        .chain(spec.redacted_values.iter())
        .filter(|value| !value.is_empty())
        .fold(input.to_string(), |text, value| {
            text.replace(value, "[REDACTED]")
        })
}

pub(crate) fn redact_command_bytes(bytes: &mut Vec<u8>, spec: &DockerCommandSpec) {
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
    pub(crate) redacted_values: Vec<String>,
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
pub(crate) struct CappedCommandStream {
    pub(crate) bytes: Vec<u8>,
    pub(crate) truncated: bool,
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
pub(crate) struct DockerCommandOutput {
    pub(crate) stdout: CappedCommandStream,
    pub(crate) stderr: CappedCommandStream,
}

#[derive(Debug)]
pub(crate) struct DockerCommandError {
    pub(crate) source: AppError,
    pub(crate) spawned: bool,
}

impl std::fmt::Display for DockerCommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.fmt(formatter)
    }
}

pub(crate) struct DockerCommandOwner {
    pub(crate) child: Option<Child>,
    pub(crate) stdout: Option<JoinHandle<std::io::Result<CappedCommandStream>>>,
    pub(crate) stderr: Option<JoinHandle<std::io::Result<CappedCommandStream>>>,
    pub(crate) session_id: Option<Uuid>,
    pub(crate) reason: &'static str,
    pub(crate) program: String,
    #[cfg(test)]
    pub(crate) active_child: Option<ActiveDockerChildGuard>,
}

#[derive(Default)]
pub(crate) struct DockerCommandOwnership {
    pub(crate) next_id: AtomicU64,
    pub(crate) entries: Mutex<Vec<DockerCommandOwnershipEntry>>,
    #[cfg(test)]
    pub(crate) retry_gate: Mutex<Option<DockerCommandRetryGate>>,
}

pub(crate) struct DockerCommandOwnershipEntry {
    pub(crate) id: u64,
    pub(crate) owner: Option<DockerCommandOwner>,
    pub(crate) session_id: Option<Uuid>,
    pub(crate) reason: &'static str,
    pub(crate) program: String,
    pub(crate) in_flight: bool,
    pub(crate) last_error: Option<String>,
}

#[cfg(test)]
pub(crate) struct DockerCommandRetryGate {
    pub(crate) entered: std::sync::mpsc::Sender<()>,
    pub(crate) release: std::sync::mpsc::Receiver<()>,
}

impl DockerCommandOwner {
    pub(crate) fn log_context(&self) -> String {
        format!(
            "session={} reason={}",
            self.session_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string()),
            self.reason
        )
    }

    pub(crate) fn reap_child_nonblocking(&mut self) {
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

    pub(crate) fn reap_finished_readers(&mut self) {
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

    pub(crate) fn terminate_until(&mut self, deadline: Instant, canceled: bool) -> bool {
        if let Some(child) = self.child.as_mut() {
            if let Err(error) = child.kill() {
                log::debug!(
                    "[container-runtime] Docker command kill returned program={} canceled={} {} \
                    error={}",
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

    pub(crate) fn collect_output_until(
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

    pub(crate) fn is_terminal(&self) -> bool {
        self.child.is_none() && self.stdout.is_none() && self.stderr.is_none()
    }
}

impl DockerCommandOwnershipEntry {
    pub(crate) fn new(id: u64, owner: DockerCommandOwner, last_error: Option<String>) -> Self {
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
    pub(crate) fn retain(&self, owner: DockerCommandOwner, last_error: Option<String>) {
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

    pub(crate) fn reap_finished(&self) {
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

    pub(crate) fn retry_until(&self, deadline: Instant) {
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
                        "[container-runtime] retained Docker command cleanup panicked {} \
                        state=retained",
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
                    "[container-runtime] retained Docker command diagnostic entry disappeared \
                    during retry id={} state=retained",
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

    pub(crate) fn retained_contexts(&self) -> Vec<RetainedDockerCommandContext> {
        self.reap_finished();
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .map(|entry| RetainedDockerCommandContext {
                session_id: entry.session_id,
                reason: entry.reason.to_string(),
                program: entry.program.clone(),
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
    pub(crate) fn install_retry_gate_for_test(
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
    pub(crate) fn wait_at_retry_gate_for_test(&self) {
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
                "[container-runtime] Docker command ownership dropped with in-flight diagnostic \
                entries count={}",
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
pub(crate) enum ScriptedDockerCommandResult {
    Output { stdout: Vec<u8>, stderr: Vec<u8> },
    SpawnedError(String),
    ReaderError(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DockerCommandShutdownBehavior {
    CancelOnShutdown,
    ContinueUntilDeadline,
}

impl DockerCommandOutput {
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            stdout: CappedCommandStream::default(),
            stderr: CappedCommandStream::default(),
        }
    }
}

impl CappedCommandStream {
    pub(crate) fn trimmed_text(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.bytes).trim().to_string();
        if self.truncated {
            append_truncation_marker(&mut text);
        }
        text
    }

    pub(crate) fn trim_end_text(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.bytes).trim_end().to_string();
        if self.truncated {
            append_truncation_marker(&mut text);
        }
        text
    }
}

pub(crate) fn append_truncation_marker(text: &mut String) {
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str("[output truncated]");
}

pub(crate) fn read_capped_to_end<R: Read>(
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

pub(crate) fn join_command_readers(
    stdout: JoinHandle<std::io::Result<CappedCommandStream>>,
    stderr: JoinHandle<std::io::Result<CappedCommandStream>>,
) -> Result<DockerCommandOutput, AppError> {
    let stdout = join_command_reader(stdout, "stdout")?;
    let stderr = join_command_reader(stderr, "stderr")?;
    Ok(DockerCommandOutput { stdout, stderr })
}

pub(crate) fn join_command_reader(
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
pub(crate) struct ActiveDockerChildGuard {
    pub(crate) active: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl ActiveDockerChildGuard {
    pub(crate) fn new(active: Arc<std::sync::atomic::AtomicUsize>) -> Self {
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
pub(crate) struct ActiveDockerReaderGuard {
    pub(crate) active: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl ActiveDockerReaderGuard {
    pub(crate) fn new(active: Arc<std::sync::atomic::AtomicUsize>) -> Self {
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

#[cfg(test)]
pub(crate) struct DockerCommandReaderReleaseForTest {
    pub(crate) gate: Arc<(Mutex<bool>, std::sync::Condvar)>,
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

pub(crate) struct RetainedDockerCommandContext {
    pub(crate) session_id: Option<Uuid>,
    pub(crate) reason: String,
    pub(crate) program: String,
    pub(crate) in_flight: bool,
    pub(crate) last_error: Option<String>,
    pub(crate) state: &'static str,
}

pub(crate) trait DockerCommandControl {
    fn shutdown_requested(&self) -> bool;
    fn shutdown_deadline(&self) -> Option<Instant>;
}
