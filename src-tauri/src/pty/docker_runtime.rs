use std::collections::HashSet;
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::container_runtime::{
    ContainerCleanupReport, ContainerRuntime, ContainerRuntimeHandle, ContainerStartRequest,
    DEFAULT_API_HELPER_PATH, DEFAULT_BRIDGE_ENTRYPOINT, SESSION_LABEL,
};

const DOCKER_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const DOCKER_COMMAND_POLL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerCommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DockerRuntime {
    program: String,
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
        }
    }

    #[cfg(test)]
    pub(crate) fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }

    pub fn build_run_command(&self, request: &ContainerStartRequest) -> DockerCommandSpec {
        let args_json = serde_json::to_string(&request.args).unwrap_or_else(|_| "[]".to_string());
        let child_env_json =
            serde_json::to_string(&request.child_env).unwrap_or_else(|_| "[]".to_string());

        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
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
            format!("AGENTSCOMMANDER_BRIDGE_COLS={}", request.cols),
            "--env".to_string(),
            format!("AGENTSCOMMANDER_BRIDGE_ROWS={}", request.rows),
            "--mount".to_string(),
            format!(
                "type=bind,source={},target={}",
                request.host_root, request.container_workdir
            ),
            "--workdir".to_string(),
            request.container_workdir.clone(),
            request.image.clone(),
            DEFAULT_BRIDGE_ENTRYPOINT.to_string(),
        ];

        args.retain(|arg| !arg.is_empty());
        DockerCommandSpec {
            program: self.program.clone(),
            args,
        }
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

    fn run_command(&self, spec: DockerCommandSpec) -> Result<String, AppError> {
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
            let mut buf = Vec::new();
            stdout.read_to_end(&mut buf).map(|_| buf)
        });
        let stderr_reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            stderr.read_to_end(&mut buf).map(|_| buf)
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
            let stderr = String::from_utf8_lossy(&stderr);
            return Err(AppError::PtyError(format!(
                "container runtime command exited {}: {}",
                status,
                stderr.trim()
            )));
        }
        Ok(String::from_utf8_lossy(&stdout).trim().to_string())
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
}

fn join_command_reader(
    reader: JoinHandle<std::io::Result<Vec<u8>>>,
    stream_name: &'static str,
) -> Result<Vec<u8>, AppError> {
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
        let stdout = self.run_command(self.build_run_command(&request))?;
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
        match self.run_command(self.build_stop_command(handle, timeout)) {
            Ok(_) => Ok(()),
            Err(stop_err) => {
                log::warn!(
                    "[container-runtime] graceful stop failed for session {}: {}",
                    handle.session_id,
                    stop_err
                );
                self.run_command(self.build_force_remove_command(handle))
                    .map(|_| ())
            }
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
        ContainerStartRequest, DEFAULT_API_HELPER_PATH, DEFAULT_CONTAINER_WORKDIR, SESSION_LABEL,
    };

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
            cols: 120,
            rows: 30,
        }
    }

    #[test]
    fn run_command_is_detached_labeled_and_has_expected_env_without_interactive_flags() {
        let runtime = DockerRuntime::with_program("docker-test");
        let spec = runtime.build_run_command(&request());
        let joined = spec.args.join(" ");

        assert_eq!(spec.program, "docker-test");
        assert!(spec.args.iter().any(|arg| arg == "-d"));
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
        assert!(joined
            .contains("type=bind,source=C:/project/.ac/wg-1-team/__agent_dev,target=/workspace"));
        assert!(!joined.contains("docker.sock"));
        assert!(!joined.contains("messaging"));
        assert!(!joined.contains("api-clients.json"));
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

        let list = runtime.build_list_labeled_command();
        assert_eq!(list.args[0], "ps");
        assert!(list.args.contains(&format!("label={}", SESSION_LABEL)));
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
}
