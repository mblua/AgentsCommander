use super::AgentUpdateCommandRef;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TargetWork {
    PreProbe,
    UpdaterStep(usize),
    PostProbe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TargetPhase {
    Pending,
    Prompting,
    Probing,
    Starting(TargetWork, u64),
    SpawnCommitted(TargetWork, u64),
    Running(usize, u64),
    Verifying,
    Cleanup(TargetWork, u64),
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TargetClaim {
    Open,
    CancellationReserved,
}

#[derive(Default)]
pub(crate) struct AsyncFence {
    pub(crate) open: AtomicBool,
    pub(crate) notify: tokio::sync::Notify,
}

impl AsyncFence {
    pub(crate) fn open(&self) {
        if !self.open.swap(true, Ordering::AcqRel) {
            self.notify.notify_waiters();
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open.load(Ordering::Acquire)
    }

    pub(crate) async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            if self.is_open() {
                return;
            }
            notified.await;
        }
    }
}

/// Opens response-release fences on normal return and on future cancellation.
/// A disconnected transport therefore cannot strand a terminalizer forever
/// after it reserved cancellation but before it returned a response.
pub(crate) struct ResponseReleaseGuard {
    pub(crate) fences: Vec<Arc<AsyncFence>>,
}

impl ResponseReleaseGuard {
    pub(crate) fn new(fences: Vec<Arc<AsyncFence>>) -> Self {
        Self { fences }
    }

    pub(crate) fn open_all(&mut self) {
        for fence in self.fences.drain(..) {
            fence.open();
        }
    }

    pub(crate) fn release(mut self) {
        self.open_all();
    }
}

impl Drop for ResponseReleaseGuard {
    fn drop(&mut self) {
        self.open_all();
    }
}

pub(crate) struct TargetControl {
    pub(crate) command_ref: AgentUpdateCommandRef,
    pub(crate) phase: TargetPhase,
    pub(crate) claim: TargetClaim,
    pub(crate) next_epoch: u64,
    pub(crate) cancel: tokio::sync::watch::Sender<bool>,
    pub(crate) acceptance: Arc<AsyncFence>,
    pub(crate) response_release: Arc<AsyncFence>,
    pub(crate) terminal_publication: Arc<AsyncFence>,
    pub(crate) terminal_emitted: bool,
    pub(crate) supervisor_failure: bool,
    #[cfg(test)]
    pub(crate) step_admission_hook: Option<StepAdmissionTestHook>,
    #[cfg(test)]
    pub(crate) response_release_hook: Option<ResponseReleaseTestHook>,
    #[cfg(test)]
    pub(crate) post_probe_settled_hook: Option<PostProbeSettledTestHook>,
}

#[cfg(test)]
pub(crate) struct StepAdmissionTestHook {
    pub(crate) after_begin: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) release_commit: Option<tokio::sync::oneshot::Receiver<()>>,
    pub(crate) after_commit: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) release_spawn: Option<tokio::sync::oneshot::Receiver<()>>,
}

#[cfg(test)]
pub(crate) struct ResponseReleaseTestHook {
    pub(crate) ready: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) release: Option<tokio::sync::oneshot::Receiver<()>>,
}

#[cfg(test)]
pub(crate) struct PostProbeSettledTestHook {
    pub(crate) ready: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) release: Option<tokio::sync::oneshot::Receiver<()>>,
}

impl TargetControl {
    pub(crate) fn new(command_ref: AgentUpdateCommandRef) -> Self {
        let (cancel, _receiver) = tokio::sync::watch::channel(false);
        Self {
            command_ref,
            phase: TargetPhase::Pending,
            claim: TargetClaim::Open,
            next_epoch: 0,
            cancel,
            acceptance: Arc::new(AsyncFence::default()),
            response_release: Arc::new(AsyncFence::default()),
            terminal_publication: Arc::new(AsyncFence::default()),
            terminal_emitted: false,
            supervisor_failure: false,
            #[cfg(test)]
            step_admission_hook: None,
            #[cfg(test)]
            response_release_hook: None,
            #[cfg(test)]
            post_probe_settled_hook: None,
        }
    }
}

pub(crate) enum StepAdmission {
    Cancelled,
    Committed {
        epoch: u64,
        cancel: tokio::sync::watch::Receiver<bool>,
    },
}

pub(crate) enum TerminalPublish {
    Published,
    CancellationReserved,
    AlreadyTerminal,
}
