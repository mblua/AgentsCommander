mod execution;
mod ownership;

pub(crate) use execution::DockerProcessState;
pub use ownership::DockerCommandSpec;
#[cfg(test)]
pub(crate) use ownership::{
    read_capped_to_end, redact_command_bytes, ActiveDockerReaderGuard, CappedCommandStream,
    DockerCommandOwner, DockerCommandReaderReleaseForTest, ScriptedDockerCommandResult,
    DOCKER_COMMAND_OUTPUT_BYTE_LIMIT,
};
pub(crate) use ownership::{
    DockerCommandControl, DockerCommandError, DockerCommandOutput, DockerCommandShutdownBehavior,
    DOCKER_COMMAND_FINALIZATION_RESERVE, DOCKER_COMMAND_TIMEOUT,
};

#[cfg(test)]
mod tests;
