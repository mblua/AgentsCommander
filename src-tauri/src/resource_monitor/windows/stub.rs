use super::{
    ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory, ProcessTreeBackend,
    ResourceError, TerminateOutcome,
};

pub struct PlatformProcessTreeBackend;

impl Default for PlatformProcessTreeBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformProcessTreeBackend {
    pub fn new() -> Self {
        Self
    }
}

impl ProcessTreeBackend for PlatformProcessTreeBackend {
    fn supports_process_tree_enforcement(&self) -> bool {
        false
    }

    fn observe_tree(&self, _root: ProcessIdentity) -> Result<ObservedProcessTree, ResourceError> {
        Ok(ObservedProcessTree {
            processes: Vec::new(),
            errors: vec!["process tree telemetry unavailable on this platform".to_string()],
        })
    }

    fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
        Ok(Some(ProcessIdentity {
            pid,
            creation_time_100ns: 0,
        }))
    }

    fn terminate_verified(
        &self,
        _process: &ObservedProcess,
    ) -> Result<TerminateOutcome, ResourceError> {
        Err(ResourceError::Message(
            "process termination unavailable on this platform".to_string(),
        ))
    }

    fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
        Ok(ProcessMemory::default())
    }
}

#[cfg(test)]
mod tests;
