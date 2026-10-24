//! The single Jev call and its decision rule (#2232 phase 6, plan sections 4
//! and 6).
//!
//! One candidate becomes one of the catalog's four destinations, or an
//! abstention with a visible reason. All categories travel in a single `POST`,
//! one question per category, and the question order is byte-sorted so it can
//! never depend on a map's iteration order. The winner is the highest `noul`
//! and wins only if it clears both `threshold` and `margin`; the only retry is
//! a single retry on a transport error.
//!
//! `JevSettings` is a plain value type. The caller (tests here, then the
//! phase-7 supervisor) copies the six phase-2 `AppSettings` fields into it; the
//! leaf never imports the settings config module.
//!
//! Leaf module: it names `crate::network` (the `general()` client only, through
//! the outbound limiter), `capture::catalog`, reqwest, serde and std. It never
//! names the phone, session or commands subtrees, or either config module named
//! in plan section 4, which is what keeps it out of the 88-module SCC.

use std::cmp::Ordering;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::capture::catalog::{Catalog, Destination, Resolution};
use crate::network::OutboundNetwork;

/// The wire primitive, fixed by plan section 6.1.
const PRIMITIVE: &str = "noul";

/// The label this call registers with the outbound limiter. It is also what
/// test 18 asserts, and it names the call, not a client.
const NETWORK_LABEL: &str = "jev.classify";

/// One transport failure plus one retry; never more (plan section 6.2).
const MAX_ATTEMPTS: u32 = 2;

/// Everything the call needs, copied field by field from phase 2's
/// `AppSettings` by the caller.
///
/// Holds the Jev API key: never log this struct with `{:?}`, and never log
/// `api_key` in any form (#2455).
#[derive(Clone, Debug, PartialEq)]
pub struct JevSettings {
    pub api_key: String,
    pub model: String,
    pub endpoint: String,
    pub timeout_secs: u64,
    pub threshold: f32,
    pub margin: f32,
}

/// The decision: a destination, or an abstention with a visible reason.
#[derive(Clone, Debug, PartialEq)]
pub enum ClassifyOutcome {
    Classified {
        category: String,
        destination: Destination,
        score: f32,
        runner_up: f32,
    },
    Abstained {
        reason: String,
    },
}

impl ClassifyOutcome {
    /// Build an abstention. Abstention is a first-class outcome, never an `Err`.
    pub fn abstained(reason: impl Into<String>) -> Self {
        Self::Abstained {
            reason: reason.into(),
        }
    }
}

#[derive(Serialize)]
struct JevRequest<'a> {
    model: &'a str,
    primitive: &'a str,
    context: &'a str,
    questions: Vec<JevQuestion<'a>>,
}

#[derive(Serialize)]
struct JevQuestion<'a> {
    id: &'a str,
    text: &'a str,
}

/// The only accepted response shape. `deny_unknown_fields` is deliberately off
/// (plan section 6.1) so a future field does not break the call; every field
/// declared here is required.
#[derive(Deserialize)]
struct JevResponse {
    results: Vec<JevResult>,
}

#[derive(Deserialize)]
struct JevResult {
    id: String,
    noul: f32,
}

/// What one HTTP attempt produced.
enum Attempt {
    /// A 2xx body parsed into the typed response.
    Response(Vec<JevResult>),
    /// 408, 429, 5xx, connect or read. Eligible for the single retry.
    Transport(String),
    /// Timeout, other 4xx, malformed body. Never retried.
    Permanent(String),
}

/// Run the whole decision: preconditions, one call (plus at most one retry),
/// response validation, threshold and margin, then catalog resolution.
pub async fn classify(
    network: &OutboundNetwork,
    settings: &JevSettings,
    catalog: &Catalog,
    text: &str,
    session_tag: &str,
) -> ClassifyOutcome {
    if catalog.is_missing() {
        log::info!("[co-managed] jev classify [{session_tag}]: abstained, no catalog file");
        return ClassifyOutcome::abstained(
            "NoCatalogFile: the room has no catalog, so Co-managed is inert",
        );
    }
    if let Some(reason) = catalog.unparseable_reason() {
        log::info!(
            "[co-managed] jev classify [{session_tag}]: abstained, catalog invalid: {}",
            safe_reason(reason)
        );
        return ClassifyOutcome::abstained(format!("catalog invalid: {reason}"));
    }
    if settings.api_key.trim().is_empty() {
        log::info!("[co-managed] jev classify [{session_tag}]: abstained, no API key configured");
        return ClassifyOutcome::abstained(
            "NoApiKey: no Jev API key is configured, so Co-managed is inert",
        );
    }
    let questions = catalog.questions_in_order();
    if questions.is_empty() {
        log::info!("[co-managed] jev classify [{session_tag}]: abstained, no askable category");
        return ClassifyOutcome::abstained("catalog has no askable categories");
    }

    let _permit = match network.acquire(NETWORK_LABEL).await {
        Ok(permit) => permit,
        Err(reason) => {
            log::info!(
                "[co-managed] jev classify [{session_tag}]: abstained, no network permit: {reason}"
            );
            return ClassifyOutcome::abstained(format!("network limiter unavailable: {reason}"));
        }
    };

    let mut attempt = 0_u32;
    loop {
        attempt += 1;
        match send_once(network, settings, catalog, text, session_tag, attempt).await {
            Attempt::Response(results) => {
                let outcome = decide(settings, catalog, &questions, results);
                match &outcome {
                    ClassifyOutcome::Classified {
                        category,
                        score,
                        runner_up,
                        ..
                    } => log::info!(
                        "[co-managed] jev classify [{session_tag}]: category={category} \
                         score={score} runner_up={runner_up}"
                    ),
                    ClassifyOutcome::Abstained { reason } => log::info!(
                        "[co-managed] jev classify [{session_tag}]: abstained after response: {}",
                        safe_reason(reason)
                    ),
                }
                return outcome;
            }
            Attempt::Transport(reason) => {
                if attempt >= MAX_ATTEMPTS {
                    log::warn!(
                        "[co-managed] jev classify [{session_tag}]: abstained, transport error \
                         after one retry: {}",
                        safe_reason(&reason)
                    );
                    return ClassifyOutcome::abstained(format!(
                        "transport error after one retry: {reason}"
                    ));
                }
            }
            Attempt::Permanent(reason) => {
                log::warn!(
                    "[co-managed] jev classify [{session_tag}]: abstained, permanent error: {}",
                    safe_reason(&reason)
                );
                return ClassifyOutcome::abstained(reason);
            }
        }
    }
}

/// One attempt. Transport errors are classified here so the retry loop can act
/// on the result without re-inspecting a `reqwest::Error`.
async fn send_once(
    network: &OutboundNetwork,
    settings: &JevSettings,
    catalog: &Catalog,
    text: &str,
    session_tag: &str,
    attempt: u32,
) -> Attempt {
    let client = network.general();
    let request = match build_request(client, settings, catalog, text) {
        Ok(request) => request,
        Err(reason) => {
            log::warn!(
                "[co-managed] jev send [{session_tag}]: attempt={attempt} request build failed: {}",
                safe_reason(&reason)
            );
            return Attempt::Permanent(reason);
        }
    };
    let started = std::time::Instant::now();
    match client.execute(request).await {
        Ok(response) => {
            let latency_ms = started.elapsed().as_millis();
            let status = response.status();
            if status.is_success() {
                log::info!(
                    "[co-managed] jev send [{session_tag}]: attempt={attempt} status={status} \
                     latency_ms={latency_ms}"
                );
            } else {
                log::warn!(
                    "[co-managed] jev send [{session_tag}]: attempt={attempt} status={status} \
                     latency_ms={latency_ms}"
                );
            }
            if status.is_success() {
                match response.json::<JevResponse>().await {
                    Ok(parsed) => Attempt::Response(parsed.results),
                    Err(error) => {
                        log::warn!(
                            "[co-managed] jev send [{session_tag}]: attempt={attempt} malformed \
                             body after status={status}: {}",
                            safe_reason(&error.to_string())
                        );
                        Attempt::Permanent(format!("malformed response: {error}"))
                    }
                }
            } else if is_transport_status(status) {
                Attempt::Transport(format!("HTTP {status}"))
            } else {
                Attempt::Permanent(format!("HTTP {status}, not retried"))
            }
        }
        Err(error) => {
            let latency_ms = started.elapsed().as_millis();
            log::warn!(
                "[co-managed] jev send [{session_tag}]: attempt={attempt} error={error} \
                 latency_ms={latency_ms}"
            );
            if error.is_timeout() {
                let seconds = settings.timeout_secs;
                Attempt::Permanent(format!("timeout after {seconds}s, not retried"))
            } else if error.is_decode() {
                Attempt::Permanent(format!("malformed response: {error}"))
            } else {
                Attempt::Transport(format!("{error}"))
            }
        }
    }
}

/// #2455 E2: every reason prefix this module, `capture::catalog` and the
/// supervisor in `lib.rs` build. `safe_reason` emits only these literals.
const SAFE_REASON_PREFIXES: &[&str] = &[
    // `decide` and `send_once`.
    "malformed response: unknown id ",
    "malformed response: duplicate id ",
    "malformed response: noul ",
    "malformed response: missing id(s) ",
    "malformed response: ",
    "winner ",
    "category ",
    "HTTP ",
    "timeout after ",
    "Jev request build failed: ",
    // `classify` and the catalog loader.
    "NoCatalogFile: ",
    "NoApiKey: ",
    "catalog has no askable categories",
    "catalog invalid: catalog is not valid JSON: ",
    "catalog invalid: catalog is unreadable: ",
    "catalog invalid: ",
    "catalog is not valid JSON: ",
    "catalog is unreadable: ",
    "network limiter unavailable: ",
    "transport error after one retry: ",
    // The supervisor's cycle-end reasons.
    "abstained: ",
    "category '",
    "secret detected by rule ",
    "session vanished",
    "baseline record consumed; never routed",
    "route failed: ",
    "BudgetExhausted",
    "PreconditionsStale",
    "PreconditionsRejected",
    "LockBusy",
    "LockUnavailable(",
    "SlotChanged",
    "AlreadyConsumed",
];

/// #2455 E2: the logged projection of a reason this phase did not build.
/// It copies NO byte of `reason`: the output is the longest matching literal
/// from `SAFE_REASON_PREFIXES`, then `<redacted len=N>` for the byte length of
/// the rest, or `<unclassified reason len=N>`. A blocklist that tries to keep
/// the safe bytes of attacker-chosen text loses (round 7), so nothing is kept.
/// `abstained: ` wraps a classify reason, so it is matched once more inside.
/// Only ever applied to what is LOGGED, never to what is returned.
pub(crate) fn safe_reason(reason: &str) -> String {
    safe_reason_at(reason, true)
}

fn safe_reason_at(reason: &str, nest: bool) -> String {
    let Some(prefix) = SAFE_REASON_PREFIXES
        .iter()
        .copied()
        .filter(|prefix| reason.starts_with(prefix))
        .max_by_key(|prefix| prefix.len())
    else {
        return format!("<unclassified reason len={}>", reason.len());
    };
    let rest = &reason[prefix.len()..];
    if nest && prefix == "abstained: " {
        return format!("{prefix}{}", safe_reason_at(rest, false));
    }
    format!("{prefix}<redacted len={}>", rest.len())
}

fn is_transport_status(status: reqwest::StatusCode) -> bool {
    status.as_u16() == 408 || status.as_u16() == 429 || status.is_server_error()
}

/// Build the one request: `POST {endpoint}` with the bearer key, exactly the
/// four body keys of plan section 6.1, and no other credential header.
fn build_request(
    client: &reqwest::Client,
    settings: &JevSettings,
    catalog: &Catalog,
    text: &str,
) -> Result<reqwest::Request, String> {
    let payload = JevRequest {
        model: &settings.model,
        primitive: PRIMITIVE,
        context: text,
        questions: catalog
            .questions_in_order()
            .into_iter()
            .map(|(id, question)| JevQuestion { id, text: question })
            .collect(),
    };
    client
        .post(&settings.endpoint)
        .bearer_auth(&settings.api_key)
        .json(&payload)
        .timeout(Duration::from_secs(settings.timeout_secs))
        .build()
        .map_err(|error| format!("Jev request build failed: {error}"))
}

/// Validate the response against the request, apply threshold and margin, then
/// resolve the winner through the catalog. Every failure is an abstention.
fn decide(
    settings: &JevSettings,
    catalog: &Catalog,
    requested: &[(&str, &str)],
    results: Vec<JevResult>,
) -> ClassifyOutcome {
    let mut seen = std::collections::HashSet::new();
    for result in &results {
        if !requested.iter().any(|(id, _)| *id == result.id) {
            return ClassifyOutcome::abstained(format!(
                "malformed response: unknown id \"{}\"",
                result.id
            ));
        }
        if !seen.insert(result.id.as_str()) {
            return ClassifyOutcome::abstained(format!(
                "malformed response: duplicate id \"{}\"",
                result.id
            ));
        }
        if !(0.0..=1.0).contains(&result.noul) {
            return ClassifyOutcome::abstained(format!(
                "malformed response: noul {} out of [0,1] for \"{}\"",
                result.noul, result.id
            ));
        }
    }
    if seen.len() != requested.len() {
        let missing: Vec<&str> = requested
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| !seen.contains(id))
            .collect();
        return ClassifyOutcome::abstained(format!(
            "malformed response: missing id(s) {}",
            missing.join(", ")
        ));
    }

    // `requested` is byte-sorted, so a stable descending sort keeps the
    // byte-smallest id as the deterministic winner when scores tie.
    let mut ranked: Vec<(&str, f32)> = requested
        .iter()
        .map(|(id, _)| {
            let score = results
                .iter()
                .find(|result| result.id == *id)
                .map(|result| result.noul)
                .unwrap_or(0.0);
            (*id, score)
        })
        .collect();
    ranked.sort_by(|left, right| right.1.partial_cmp(&left.1).unwrap_or(Ordering::Equal));
    // The catalog check above guarantees at least one askable question.
    let (winner_id, winner_score) = ranked[0];
    let runner_up = ranked.get(1).map(|(_, score)| *score).unwrap_or(0.0);

    if winner_score < settings.threshold {
        return ClassifyOutcome::abstained(format!(
            "winner \"{winner_id}\" scored {winner_score:.3}, below threshold {:.3} (margin \
             required {:.3}, runner-up {runner_up:.3}, model {})",
            settings.threshold, settings.margin, settings.model
        ));
    }
    if winner_score - runner_up < settings.margin {
        return ClassifyOutcome::abstained(format!(
            "winner \"{winner_id}\" margin {:.3}, below required {:.3} (score \
             {winner_score:.3}, threshold {:.3}, runner-up {runner_up:.3}, model {})",
            winner_score - runner_up,
            settings.margin,
            settings.threshold,
            settings.model
        ));
    }

    match catalog.resolve(winner_id) {
        Resolution::Valid { destination, .. } => ClassifyOutcome::Classified {
            category: winner_id.to_string(),
            destination,
            score: winner_score,
            runner_up,
        },
        Resolution::Invalid { reason } => {
            ClassifyOutcome::abstained(format!("category \"{winner_id}\" is invalid: {reason}"))
        }
        Resolution::Unknown => {
            ClassifyOutcome::abstained(format!("category \"{winner_id}\" is not in the catalog"))
        }
    }
}

#[cfg(test)]
mod tests;
