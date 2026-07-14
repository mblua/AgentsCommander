use std::collections::HashSet;
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::container_runtime::{
    ContainerCleanupReport, ContainerDiagnostics, ContainerRuntime, ContainerRuntimeHandle,
    ContainerStartRequest, ContainerStateSnapshot, DEFAULT_API_HELPER_PATH,
    DEFAULT_BRIDGE_ENTRYPOINT, SESSION_LABEL,
};

const DOCKER_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const DOCKER_COMMAND_POLL: Duration = Duration::from_millis(50);
const DOCKER_COMMAND_OUTPUT_BYTE_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerCommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CappedCommandStream {
    bytes: Vec<u8>,
    truncated: bool,
}

struct DockerCommandOutput {
    stdout: CappedCommandStream,
    stderr: CappedCommandStream,
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

#[derive(Debug, Clone)]
pub struct DockerRuntime {
    program: String,
    #[cfg(test)]
    recorded_commands: Option<std::sync::Arc<std::sync::Mutex<Vec<DockerCommandSpec>>>>,
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
            #[cfg(test)]
            recorded_commands: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            recorded_commands: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_recorded_commands(
        program: impl Into<String>,
        recorded_commands: std::sync::Arc<std::sync::Mutex<Vec<DockerCommandSpec>>>,
    ) -> Self {
        Self {
            program: program.into(),
            recorded_commands: Some(recorded_commands),
        }
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
            "--label".to_string(),
            format!("{}={}", SESSION_LABEL, request.session_id),
            "--env".to_string(),
            format!("AGENTSCOMMANDER_API_URL={}", request.api_url),
            "--env".to_string(),
            format!("AGENTSCOMMANDER_API_TOKEN={}", request.api_token),
            "--env".to_string(),
            format!("AGENTSCOMMANDER_SESSION_ID={}", request.session_id),
            "--env".to_string(),
            format!(
                "AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN={}",
                request.registration_ticket
            ),
            "--env".to_string(),
            format!("AGENTSCOMMANDER_ROOT={}", request.host_root),
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
            format!("AGENTSCOMMANDER_BRIDGE_COMMAND={}", request.command),
            "--env".to_string(),
            format!("AGENTSCOMMANDER_BRIDGE_ARGS_JSON={}", args_json),
            "--env".to_string(),
            format!("AGENTSCOMMANDER_BRIDGE_ENV_JSON={}", child_env_json),
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
        Ok(DockerCommandSpec {
            program: self.program.clone(),
            args,
        })
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
        }
    }

    fn run_command_output(&self, spec: DockerCommandSpec) -> Result<DockerCommandOutput, AppError> {
        #[cfg(test)]
        if let Some(recorded_commands) = &self.recorded_commands {
            recorded_commands.lock().unwrap().push(spec);
            return Ok(DockerCommandOutput::empty());
        }

        let mut child = Command::new(&spec.program)
            .args(&spec.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AppError::PtyError(format!("container runtime command failed: {e}")))?;

        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::PtyError(
                "container runtime command did not expose stdout".to_string(),
            ));
        };
        let Some(mut stderr) = child.stderr.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::PtyError(
                "container runtime command did not expose stderr".to_string(),
            ));
        };
        let stdout_reader = std::thread::spawn(move || {
            read_capped_to_end(&mut stdout, DOCKER_COMMAND_OUTPUT_BYTE_LIMIT)
        });
        let stderr_reader = std::thread::spawn(move || {
            read_capped_to_end(&mut stderr, DOCKER_COMMAND_OUTPUT_BYTE_LIMIT)
        });

        let deadline = Instant::now() + DOCKER_COMMAND_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = stdout_reader.join();
                        let _ = stderr_reader.join();
                        return Err(AppError::PtyError(format!(
                            "container runtime command timed out after {:?}",
                            DOCKER_COMMAND_TIMEOUT
                        )));
                    }
                    std::thread::sleep(DOCKER_COMMAND_POLL);
                }
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(AppError::PtyError(format!(
                        "container runtime command wait failed: {e}"
                    )));
                }
            }
        };

        let stdout = join_command_reader(stdout_reader, "stdout")?;
        let stderr = join_command_reader(stderr_reader, "stderr")?;
        if !status.success() {
            let stderr_text = stderr.trimmed_text();
            let stdout_text = stdout.trimmed_text();
            let detail = if stderr_text.trim().is_empty() {
                stdout_text.trim()
            } else {
                stderr_text.trim()
            };
            return Err(AppError::PtyError(format!(
                "container runtime command exited {}: {}",
                status, detail
            )));
        }
        Ok(DockerCommandOutput { stdout, stderr })
    }

    fn run_command(&self, spec: DockerCommandSpec) -> Result<String, AppError> {
        let output = self.run_command_output(spec)?;
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
        .map_err(|e| {
            AppError::PtyError(format!(
                "container runtime command {stream_name} read failed: {e}"
            ))
        })
}

impl ContainerRuntime for DockerRuntime {
    fn start(&self, request: ContainerStartRequest) -> Result<ContainerRuntimeHandle, AppError> {
        let session_id = request.session_id;
        let stdout = self.run_command(self.build_run_command(&request)?)?;
        let container_id = stdout
            .lines()
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppError::PtyError("container runtime returned an empty container id".to_string())
            })?
            .to_string();
        Ok(ContainerRuntimeHandle {
            session_id,
            container_id,
        })
    }

    fn stop(&self, handle: &ContainerRuntimeHandle, timeout: Duration) -> Result<(), AppError> {
        if let Err(stop_err) = self.run_command(self.build_stop_command(handle, timeout)) {
            log::warn!(
                "[container-runtime] graceful stop failed for session {}: {}",
                handle.session_id,
                stop_err
            );
        }
        self.run_command(self.build_force_remove_command(handle))
            .map(|_| ())
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
            match self.stop(&handle, timeout) {
                Ok(()) => report.stopped.push(session_id),
                Err(err) => {
                    log::warn!(
                        "[container-runtime] failed to clean orphan container for session {}: {}",
                        session_id,
                        err
                    );
                    errors.push(format!("{}: {}", session_id, err));
                }
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
        assert!(joined.contains("AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN=ticket-secret"));
        assert!(joined.contains("AGENTSCOMMANDER_API_TOKEN=api-secret"));
        assert!(joined.contains(&format!(
            "AGENTSCOMMANDER_BINARY_PATH={DEFAULT_API_HELPER_PATH}"
        )));
        assert!(joined.contains("AGENTSCOMMANDER_BRIDGE_COMMAND=codex"));
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

        runtime.stop(&handle, Duration::from_secs(5)).unwrap();

        let commands = recorded.lock().unwrap().clone();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].args, vec!["stop", "--time", "5", "abc123"]);
        assert_eq!(commands[1].args, vec!["rm", "-f", "abc123"]);
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
