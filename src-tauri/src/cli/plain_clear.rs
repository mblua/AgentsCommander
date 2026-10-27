use clap::Args;
use std::time::Duration;
use uuid::Uuid;

#[derive(Args)]
#[command(
    after_help = "Requests an immediate clear attempt for your OWN live session, without \
waiting for idle. No handoff file or resume prompt is used. Acceptance does not confirm \
provider consumption or context erasure. A timeout means the outcome is unknown; it does \
not cancel or automatically retry the request."
)]
pub struct PlainClearArgs {
    /// Live session token. Authorization happens at the daemon.
    #[arg(long)]
    pub token: Option<String>,
    /// Calling agent's root directory (required).
    #[arg(long)]
    pub root: Option<String>,
    /// One deadline in seconds from request publication.
    #[arg(long, default_value = "15")]
    pub timeout: u64,
}

pub(crate) struct Binding<'a> {
    pub action: &'a str,
    pub request_id: &'a str,
    pub requested_by: &'a str,
}

pub(crate) struct Observation {
    pub ack: Result<Option<String>, String>,
    pub rejection: Result<Option<String>, String>,
    /// Auxiliary transport observation; never establishes or blocks acceptance.
    pub delivered: Result<bool, String>,
}

pub(crate) struct Callbacks<P, R, C, W> {
    pub publish: P,
    pub read: R,
    pub now: C,
    pub wait: W,
}

#[derive(Debug)]
pub(crate) struct Outcome {
    pub code: i32,
    pub message: String,
}

fn outcome(code: i32, message: impl Into<String>) -> Outcome {
    Outcome {
        code,
        message: message.into(),
    }
}

fn accepted(content: &str, binding: &Binding<'_>) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return false;
    };
    value.get("action").and_then(|v| v.as_str()) == Some(binding.action)
        && value.get("request_id").and_then(|v| v.as_str()) == Some(binding.request_id)
        && value.get("requested_by").and_then(|v| v.as_str()) == Some(binding.requested_by)
        && value.get("status").and_then(|v| v.as_str()) == Some("accepted")
        && value
            .get("session_id")
            .and_then(|v| v.as_str())
            .is_some_and(|id| Uuid::parse_str(id).is_ok())
}

pub(crate) fn run<P, R, C, W>(
    binding: &Binding<'_>,
    timeout: u64,
    mut callbacks: Callbacks<P, R, C, W>,
) -> Outcome
where
    P: FnOnce() -> Result<(), String>,
    R: FnMut() -> Observation,
    C: FnMut() -> Duration,
    W: FnMut(Duration),
{
    let Callbacks {
        publish,
        mut now,
        mut wait,
        ..
    } = callbacks;
    if let Err(error) = publish() {
        return outcome(1, format!("self-clear request publication failed: {error}"));
    }
    let start = now();
    let budget = Duration::from_secs(timeout);
    loop {
        let Observation {
            ack,
            rejection,
            delivered: _delivered,
        } = (callbacks.read)();
        match ack {
            Ok(Some(bytes)) => {
                return if accepted(&bytes, binding) {
                    outcome(
                        0,
                        "self-clear accepted: immediate attempt requested without idle wait. \
                        Provider consumption and context erasure are not confirmed.",
                    )
                } else {
                    outcome(
                        2,
                        "self-clear outcome unknown: malformed or mismatched receipt; \
                        no cancellation or automatic retry.",
                    )
                };
            }
            Err(error) => {
                return outcome(
                    2,
                    format!(
                        "self-clear receipt read failed; outcome \
                unknown, no cancellation or automatic retry: {error}"
                    ),
                )
            }
            Ok(None) => {}
        }
        match rejection {
            Ok(Some(reason)) => {
                return outcome(1, format!("self-clear rejected: {}", reason.trim()))
            }
            Err(error) => {
                return outcome(
                    2,
                    format!(
                        "self-clear rejection read failed; outcome \
                unknown, no cancellation or automatic retry: {error}"
                    ),
                )
            }
            Ok(None) => {}
        }
        let elapsed = now().saturating_sub(start);
        if elapsed >= budget {
            return outcome(
                2,
                "self-clear timeout: outcome unknown; no cancellation or \
                automatic retry. The request may still be pending.",
            );
        }
        wait(Duration::from_millis(250).min(budget - elapsed));
    }
}

#[cfg(test)]
mod tests;
