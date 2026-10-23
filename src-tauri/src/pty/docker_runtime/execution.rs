use std::process::{Command, Stdio};
#[cfg(test)]
use std::sync::Arc;
use std::time::Instant;

use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::container_runtime::ContainerRuntimeControl;

use super::command::{
    read_capped_to_end, redact_command_bytes, redact_command_values, DockerCommandError,
    DockerCommandOutput, DockerCommandOwner, DockerCommandShutdownBehavior, DockerCommandSpec,
    DOCKER_COMMAND_FINALIZATION_RESERVE, DOCKER_COMMAND_OUTPUT_BYTE_LIMIT, DOCKER_COMMAND_POLL,
    DOCKER_COMMAND_TIMEOUT,
};
#[cfg(test)]
use super::command::{
    ActiveDockerChildGuard, ActiveDockerReaderGuard, CappedCommandStream,
    ScriptedDockerCommandResult,
};
use super::DockerRuntime;

impl DockerRuntime {
    pub(super) fn run_command_output(
        &self,
        spec: DockerCommandSpec,
    ) -> Result<DockerCommandOutput, AppError> {
        self.run_command_output_with_control(
            spec,
            &ContainerRuntimeControl::default(),
            DockerCommandShutdownBehavior::ContinueUntilDeadline,
        )
        .map_err(|error| error.source)
    }

    pub(super) fn run_command_output_with_control(
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

    pub(super) fn run_command_output_with_context(
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

    pub(super) fn terminate_or_retain_command(
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

    pub(super) fn retain_reader_owner_at_deadline(
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

    pub(super) fn run_command(&self, spec: DockerCommandSpec) -> Result<String, AppError> {
        let output = self.run_command_output(spec)?;
        Ok(String::from_utf8_lossy(&output.stdout.bytes)
            .trim()
            .to_string())
    }

    pub(super) fn run_command_with_control(
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
}
