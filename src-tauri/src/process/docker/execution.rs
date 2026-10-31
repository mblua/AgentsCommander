use std::process::{Command, Stdio};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
use std::time::Instant;

use uuid::Uuid;

use super::ownership::{
    read_capped_to_end, redact_command_bytes, redact_command_values, DockerCommandControl,
    DockerCommandError, DockerCommandOutput, DockerCommandOwner, DockerCommandOwnership,
    DockerCommandShutdownBehavior, DockerCommandSpec, RetainedDockerCommandContext,
    DOCKER_COMMAND_FINALIZATION_RESERVE, DOCKER_COMMAND_OUTPUT_BYTE_LIMIT, DOCKER_COMMAND_POLL,
    DOCKER_COMMAND_TIMEOUT,
};
#[cfg(test)]
use super::ownership::{
    ActiveDockerChildGuard, ActiveDockerReaderGuard, CappedCommandStream,
    DockerCommandReaderReleaseForTest, ScriptedDockerCommandResult,
};
use crate::errors::AppError;

#[derive(Clone)]
pub(crate) struct DockerProcessState {
    command_ownership: Arc<DockerCommandOwnership>,
    #[cfg(test)]
    pub(crate) recorded_commands: Option<std::sync::Arc<std::sync::Mutex<Vec<DockerCommandSpec>>>>,
    #[cfg(test)]
    pub(crate) scripted_results:
        Option<Arc<Mutex<std::collections::VecDeque<ScriptedDockerCommandResult>>>>,
    #[cfg(test)]
    pub(crate) hang_readers: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    pub(crate) active_children: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    pub(crate) active_readers: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Default for DockerProcessState {
    fn default() -> Self {
        Self {
            command_ownership: Arc::new(DockerCommandOwnership::default()),
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
}

impl DockerProcessState {
    #[cfg(test)]
    pub(crate) fn with_recorded_commands(recorded: Arc<Mutex<Vec<DockerCommandSpec>>>) -> Self {
        Self {
            recorded_commands: Some(recorded),
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn with_scripted_commands(
        recorded: Arc<Mutex<Vec<DockerCommandSpec>>>,
        results: Vec<ScriptedDockerCommandResult>,
    ) -> Self {
        Self {
            scripted_results: Some(Arc::new(Mutex::new(results.into()))),
            ..Self::with_recorded_commands(recorded)
        }
    }

    pub(crate) fn reap_finished(&self) {
        self.command_ownership.reap_finished();
    }

    pub(crate) fn retain(&self, owner: DockerCommandOwner, last_error: Option<String>) {
        self.command_ownership.retain(owner, last_error);
    }

    pub(crate) fn retry_until(&self, deadline: Instant) {
        self.command_ownership.retry_until(deadline);
    }

    pub(crate) fn retained_contexts(&self) -> Vec<RetainedDockerCommandContext> {
        self.command_ownership.retained_contexts()
    }

    #[cfg(test)]
    pub(crate) fn active_child_count(&self) -> usize {
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

    pub(crate) fn terminate_or_retain_command(
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

    pub(crate) fn retain_reader_owner_at_deadline(
        &self,
        owner: DockerCommandOwner,
    ) -> DockerCommandError {
        let program = owner.program.clone();
        let message = format!("readers exceeded the absolute deadline program={program}");
        self.command_ownership.retain(owner, Some(message.clone()));
        DockerCommandError {
            source: AppError::PtyError(message),
            spawned: true,
        }
    }
}

impl DockerProcessState {
    pub(crate) fn execute(
        &self,
        spec: DockerCommandSpec,
        control: &dyn DockerCommandControl,
        shutdown_behavior: DockerCommandShutdownBehavior,
        session_id: Option<Uuid>,
        reason: &'static str,
        hard_deadline: Instant,
    ) -> Result<DockerCommandOutput, DockerCommandError> {
        self.reap_finished();
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
        crate::config::agent_path::apply_search_path_to_std_command(&mut cmd);

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let child = cmd.spawn().map_err(|error| DockerCommandError {
            source: AppError::PtyError(format!("container runtime command failed: {error}")),
            spawned: false,
        })?;
        let owner = DockerCommandOwner {
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
        let owner = self.start_command_readers(owner, hard_deadline)?;

        let started_at = Instant::now();
        let command_deadline = (started_at + DOCKER_COMMAND_TIMEOUT).min(
            hard_deadline
                .checked_sub(
                    DOCKER_COMMAND_FINALIZATION_RESERVE
                        .min(hard_deadline.saturating_duration_since(started_at) / 4),
                )
                .unwrap_or(started_at),
        );
        let (owner, status) = self.wait_for_command(
            owner,
            control,
            shutdown_behavior,
            &spec.program,
            hard_deadline,
            command_deadline,
        )?;

        self.collect_command_output(owner, status, &spec, control, hard_deadline)
    }

    fn collect_command_output(
        &self,
        mut owner: DockerCommandOwner,
        status: std::process::ExitStatus,
        spec: &DockerCommandSpec,
        control: &dyn DockerCommandControl,
        hard_deadline: Instant,
    ) -> Result<DockerCommandOutput, DockerCommandError> {
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
        redact_command_bytes(&mut output.stdout.bytes, spec);
        redact_command_bytes(&mut output.stderr.bytes, spec);
        let DockerCommandOutput { stdout, stderr } = output;
        if !status.success() {
            let stderr_text = stderr.trimmed_text();
            let stdout_text = stdout.trimmed_text();
            let detail = if stderr_text.trim().is_empty() {
                stdout_text.trim()
            } else {
                stderr_text.trim()
            };
            let detail = redact_command_values(detail, spec);
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

    fn start_command_readers(
        &self,
        mut owner: DockerCommandOwner,
        hard_deadline: Instant,
    ) -> Result<DockerCommandOwner, DockerCommandError> {
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
        Ok(owner)
    }

    fn wait_for_command(
        &self,
        mut owner: DockerCommandOwner,
        control: &dyn DockerCommandControl,
        shutdown_behavior: DockerCommandShutdownBehavior,
        program: &str,
        hard_deadline: Instant,
        command_deadline: Instant,
    ) -> Result<(DockerCommandOwner, std::process::ExitStatus), DockerCommandError> {
        let status = loop {
            let dynamic_hard_deadline = control
                .shutdown_deadline()
                .map(|deadline| deadline.min(hard_deadline))
                .unwrap_or(hard_deadline);
            let Some(child) = owner.child.as_mut() else {
                let message =
                    "container runtime command child ownership became unavailable".to_string();
                self.retain(owner, Some(message.clone()));
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
                                program
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
        Ok((owner, status))
    }
}
