use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;

pub const WARNING_KIND_SELF_HANDOFF_FAILED: &str = "self-handoff-failed";
pub(crate) const ERR_SELF_HANDOFF_NOTICE_NOT_WAITING: &str = "self-handoff-notice-not-waiting";
pub(crate) const CONTINUATION_FAILURE: &str =
    "handoff continuation injection failed; consult transport/archive diagnostics";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SelfHandoffOutcome {
    Completed,
    Failed(SelfHandoffFailure),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SelfHandoffFailure {
    pub action: &'static str,
    pub attempt: Uuid,
    pub original: Uuid,
    pub current: Option<Uuid>,
    pub root: PathBuf,
    pub continuation: bool,
    pub reason: String,
    pub forgotten: bool,
}

impl SelfHandoffFailure {
    pub fn diagnostic_target(&self) -> Uuid {
        self.current.unwrap_or(self.original)
    }

    pub fn key(&self, suffix: &str) -> String {
        format!("{}:{}:{suffix}", self.action, self.attempt)
    }

    pub fn message(&self, observed: &str) -> String {
        let stage = if self.continuation {
            "continuation"
        } else {
            "before-reset"
        };
        let current = self
            .current
            .map(|id| id.to_string())
            .unwrap_or("unknown".into());
        let mut message = format!(
            "{} failed: original {}, current {}, attempt {}, observed UTC {}, stage {}: {}. ",
            self.action, self.original, current, self.attempt, observed, stage, self.reason
        );
        message.push_str(&format!(
            "This reports the earlier attempt {} at {}; it is not the status of a later operation. \
             Do not replay an operation solely from this notice. ", self.attempt, observed
        ));
        if self.continuation {
            message.push_str(&format!(
                "If this attempt is still the one being recovered, Read {}/SELF-HANDOFF.md \
                 and continue. If it is absent, recover the earlier notes from {}/self-clear/ \
                 and consult the logged archive path; restoration may have failed. ",
                self.root.display(),
                self.root.display()
            ));
        } else {
            message.push_str(
                "Resolve the cause; if no later attempt superseded it, retry. Completion/context \
                 state may be uncertain after partial clear or restart errors; retain the handoff. "
            );
        }
        if self.forgotten {
            message.push_str("SELF-FORGET.md archival was attempted when queued; see log.");
        }
        message
    }
}

pub(crate) async fn consume_outcome<D, R, F>(driver: D, report: R)
where
    D: Future<Output = SelfHandoffOutcome>,
    R: FnOnce(SelfHandoffFailure) -> F,
    F: Future<Output = ()>,
{
    if let SelfHandoffOutcome::Failed(failure) = driver.await {
        report(failure).await;
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NoticeDelivery {
    Delivered,
    Cancelled,
    Unavailable(String),
    Failed(String),
    Exhausted,
}

/// Bounds starts only: never cancel an injector that may already have written its payload.
pub(crate) async fn retry_notice<P, A, F, C>(
    interval: Duration,
    window: Duration,
    mut pending: P,
    mut attempt: A,
    deferred: C,
    pending_error: &str,
) -> NoticeDelivery
where
    P: FnMut() -> bool,
    A: FnMut() -> F,
    F: Future<Output = Result<(), NoticeDelivery>>,
    C: Fn(&str) -> bool,
{
    let deadline = tokio::time::Instant::now() + window;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return NoticeDelivery::Exhausted;
        }
        if pending() {
            return NoticeDelivery::Cancelled;
        }
        match attempt().await {
            Ok(()) => return NoticeDelivery::Delivered,
            Err(NoticeDelivery::Failed(error)) if error.starts_with(pending_error) => {
                return NoticeDelivery::Cancelled;
            }
            Err(NoticeDelivery::Failed(error)) if deferred(&error) => {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                tokio::time::sleep(interval.min(remaining)).await;
            }
            Err(status) => return status,
        }
    }
}

#[cfg(test)]
static INJECT_SIGNALS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<Uuid, std::sync::Arc<tokio::sync::Notify>>>,
> = std::sync::OnceLock::new();

#[cfg(test)]
pub(crate) fn register_inject_signal(
    id: Uuid,
    signal: Option<std::sync::Arc<tokio::sync::Notify>>,
) {
    let mut signals = INJECT_SIGNALS.get_or_init(Default::default).lock().unwrap();
    if let Some(signal) = signal {
        signals.insert(id, signal);
    } else {
        signals.remove(&id);
    }
}

#[cfg(test)]
pub(crate) fn signal_inject(id: Uuid) {
    if let Some(signal) = INJECT_SIGNALS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .get(&id)
    {
        signal.notify_one();
    }
}
