use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::container_runtime::{
    ContainerCleanupReport, ContainerDiagnostics, ContainerRuntime, ContainerRuntimeControl,
    ContainerRuntimeHandle, ContainerStartRequest, ContainerStateSnapshot,
    RetainedContainerOwnerContext, CONTAINER_STOP_TIMEOUT, DEFAULT_API_HELPER_PATH,
    DEFAULT_BRIDGE_ENTRYPOINT, SESSION_LABEL,
};

use crate::process::docker::DockerProcessState;
#[cfg(test)]
use crate::process::docker::{
    read_capped_to_end, redact_command_bytes, ActiveDockerReaderGuard, CappedCommandStream,
    DockerCommandOwner, ScriptedDockerCommandResult, DOCKER_COMMAND_OUTPUT_BYTE_LIMIT,
};
use crate::process::docker::{
    DockerCommandOutput, DockerCommandShutdownBehavior, DOCKER_COMMAND_FINALIZATION_RESERVE,
    DOCKER_COMMAND_TIMEOUT,
};
use command::{
    normalized_app_error_text, RetainedDockerStartCleanup, RetainedDockerStartCleanupRegistry,
    RetainedDockerStartCleanupState,
};

mod command;
mod execution;

#[cfg(test)]
pub(crate) use crate::process::docker::DockerCommandReaderReleaseForTest;
pub use crate::process::docker::DockerCommandSpec;

#[derive(Clone)]
pub struct DockerRuntime {
    program: String,
    process_state: DockerProcessState,
    retained_start_cleanups: Arc<RetainedDockerStartCleanupRegistry>,
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
            process_state: DockerProcessState::default(),
            retained_start_cleanups: Arc::new(RetainedDockerStartCleanupRegistry::default()),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            process_state: DockerProcessState::default(),
            retained_start_cleanups: Arc::new(RetainedDockerStartCleanupRegistry::default()),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_recorded_commands(
        program: impl Into<String>,
        recorded_commands: std::sync::Arc<std::sync::Mutex<Vec<DockerCommandSpec>>>,
    ) -> Self {
        Self {
            program: program.into(),
            process_state: DockerProcessState::with_recorded_commands(recorded_commands),
            retained_start_cleanups: Arc::new(RetainedDockerStartCleanupRegistry::default()),
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
            process_state: DockerProcessState::with_scripted_commands(recorded_commands, results),
            retained_start_cleanups: Arc::new(RetainedDockerStartCleanupRegistry::default()),
        }
    }

    #[cfg(test)]
    fn active_child_count(&self) -> usize {
        self.process_state.active_child_count()
    }

    #[cfg(test)]
    pub(crate) fn active_reader_count(&self) -> usize {
        self.process_state.active_reader_count()
    }

    #[cfg(test)]
    pub(crate) fn retain_blocked_command_readers_for_test(
        &self,
        reason: &'static str,
        program: impl Into<String>,
        last_error: impl Into<String>,
    ) -> DockerCommandReaderReleaseForTest {
        self.process_state
            .retain_blocked_command_readers_for_test(reason, program, last_error)
    }

    #[cfg(test)]
    pub(crate) fn install_command_retry_gate_for_test(
        &self,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        self.process_state.install_command_retry_gate_for_test()
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
                    "[container-runtime] ambiguous Docker start cleanup terminal session={} \
                    state=terminal",
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
                    "[container-runtime] ambiguous Docker start cleanup retained session={} \
                    state=retained error={}",
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
                    "[container-runtime] ambiguous Docker start cleanup panicked session={} \
                    state=retained",
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
                    "[container-runtime] Docker start canceled session={}; returning owned cleanup \
                    handle: {}",
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
            self.process_state.retry_until(deadline);
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
        contexts.extend(
            self.process_state
                .retained_contexts()
                .into_iter()
                .map(|context| RetainedContainerOwnerContext {
                    owner: "dockerCommand",
                    session_id: context.session_id,
                    reason: context.reason,
                    program: Some(context.program),
                    runtime_handle: None,
                    state: context.state,
                    in_flight: context.in_flight,
                    last_error: context.last_error,
                }),
        );
        contexts.sort_by_key(RetainedContainerOwnerContext::diagnostic);
        contexts
    }
}

#[cfg(test)]
mod tests;
