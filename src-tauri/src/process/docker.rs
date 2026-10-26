mod ownership;

pub use ownership::DockerCommandSpec;
pub(crate) use ownership::{
    read_capped_to_end, redact_command_bytes, redact_command_values, DockerCommandError,
    DockerCommandOutput, DockerCommandOwner, DockerCommandShutdownBehavior, DockerProcessState,
    DOCKER_COMMAND_FINALIZATION_RESERVE, DOCKER_COMMAND_OUTPUT_BYTE_LIMIT, DOCKER_COMMAND_POLL,
    DOCKER_COMMAND_TIMEOUT,
};
#[cfg(test)]
pub(crate) use ownership::{
    ActiveDockerChildGuard, ActiveDockerReaderGuard, CappedCommandStream,
    DockerCommandReaderReleaseForTest, ScriptedDockerCommandResult,
};

#[cfg(test)]
mod tests;
