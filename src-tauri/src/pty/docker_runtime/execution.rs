use std::time::Instant;

use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::container_runtime::ContainerRuntimeControl;

use super::DockerRuntime;
use crate::process::docker::{
    DockerCommandControl, DockerCommandError, DockerCommandOutput, DockerCommandShutdownBehavior,
    DockerCommandSpec, DOCKER_COMMAND_FINALIZATION_RESERVE, DOCKER_COMMAND_TIMEOUT,
};

impl DockerCommandControl for ContainerRuntimeControl {
    fn shutdown_requested(&self) -> bool {
        ContainerRuntimeControl::shutdown_requested(self)
    }

    fn shutdown_deadline(&self) -> Option<Instant> {
        ContainerRuntimeControl::shutdown_deadline(self)
    }
}

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
        self.process_state.execute(
            spec,
            control,
            shutdown_behavior,
            session_id,
            reason,
            hard_deadline,
        )
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
