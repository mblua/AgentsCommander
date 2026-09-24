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
            redact_quoted(reason)
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
                        "[co-managed] jev classify [{session_tag}]: category={category} score={score} runner_up={runner_up}"
                    ),
                    ClassifyOutcome::Abstained { reason } => log::info!(
                        "[co-managed] jev classify [{session_tag}]: abstained after response: {}",
                        redact_quoted(reason)
                    ),
                }
                return outcome;
            }
            Attempt::Transport(reason) => {
                if attempt >= MAX_ATTEMPTS {
                    log::warn!(
                        "[co-managed] jev classify [{session_tag}]: abstained, transport error after one retry: {}",
                        redact_quoted(&reason)
                    );
                    return ClassifyOutcome::abstained(format!(
                        "transport error after one retry: {reason}"
                    ));
                }
            }
            Attempt::Permanent(reason) => {
                log::warn!(
                    "[co-managed] jev classify [{session_tag}]: abstained, permanent error: {}",
                    redact_quoted(&reason)
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
                redact_quoted(&reason)
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
                    "[co-managed] jev send [{session_tag}]: attempt={attempt} status={status} latency_ms={latency_ms}"
                );
            } else {
                log::warn!(
                    "[co-managed] jev send [{session_tag}]: attempt={attempt} status={status} latency_ms={latency_ms}"
                );
            }
            if status.is_success() {
                match response.json::<JevResponse>().await {
                    Ok(parsed) => Attempt::Response(parsed.results),
                    Err(error) => {
                        log::warn!(
                            "[co-managed] jev send [{session_tag}]: attempt={attempt} malformed body after status={status}: {}",
                            redact_quoted(&error.to_string())
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
                "[co-managed] jev send [{session_tag}]: attempt={attempt} error={error} latency_ms={latency_ms}"
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

/// #2455 E2: the logged projection of a reason this module did not build.
/// Every double-quoted span (serde's rejected value, a rejected response id)
/// becomes a fixed placeholder; the diagnosis around it survives. Only ever
/// applied to what is LOGGED, never to what is returned.
pub(crate) fn redact_quoted(reason: &str) -> String {
    let mut out = String::with_capacity(reason.len());
    let mut quoted = false;
    for ch in reason.chars() {
        if ch == '"' {
            if !quoted {
                out.push_str("\"<redacted>\"");
            }
            quoted = !quoted;
        } else if !quoted {
            out.push(ch);
        }
    }
    out
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
            "winner \"{winner_id}\" scored {winner_score:.3}, below threshold {:.3} (margin required {:.3}, runner-up {runner_up:.3}, model {})",
            settings.threshold, settings.margin, settings.model
        ));
    }
    if winner_score - runner_up < settings.margin {
        return ClassifyOutcome::abstained(format!(
            "winner \"{winner_id}\" margin {:.3}, below required {:.3} (score {winner_score:.3}, threshold {:.3}, runner-up {runner_up:.3}, model {})",
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
mod tests {
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Arc;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    fn catalog(pairs: &[(&str, &str)]) -> Catalog {
        // Built textually, not through `serde_json::json!`, so the insertion
        // order of the input survives parsing and test 1 can really shuffle it.
        let entries: Vec<String> = pairs
            .iter()
            .map(|(name, destination)| {
                format!(
                    r#""{name}": {{"destination": "{destination}", "question": "is this {name}?"}}"#
                )
            })
            .collect();
        Catalog::from_json_str(&format!("{{\"categories\": {{{}}}}}", entries.join(",")))
    }

    fn settings_for(endpoint: &str) -> JevSettings {
        JevSettings {
            api_key: "test-key".to_string(),
            model: "jev-1.13.0".to_string(),
            endpoint: endpoint.to_string(),
            timeout_secs: 5,
            threshold: 0.70,
            margin: 0.15,
        }
    }

    fn body_with(pairs: &[(&str, f32)]) -> String {
        serde_json::json!({
            "results": pairs
                .iter()
                .map(|(id, noul)| serde_json::json!({ "id": id, "noul": noul }))
                .collect::<Vec<_>>(),
        })
        .to_string()
    }

    /// One scripted HTTP/1.1 response per hit; the last entry repeats. The task
    /// ignores write errors, so a client that times out and drops the socket
    /// cannot panic it.
    async fn serve(
        script: Vec<(u16, String)>,
        delay: Option<Duration>,
    ) -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback listener");
        let port = listener.local_addr().expect("listener address").port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let index = counter.fetch_add(1, AtomicOrdering::SeqCst);
                let (status, body) = script
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| script.last().cloned().expect("a scripted response"));
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                while let Ok(read) = stream.read(&mut buffer).await {
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                if let Some(delay) = delay {
                    tokio::time::sleep(delay).await;
                }
                let head = format!(
                    "HTTP/1.1 {status} Status\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(body.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        (format!("http://127.0.0.1:{port}/v1/systemone"), hits)
    }

    /// Test 1: byte-sorted questions, identical serialised body over 100
    /// different catalog insertion orders.
    #[tokio::test]
    async fn request_questions_are_byte_sorted_and_stable_across_100_shuffled_catalogs() {
        let names = ["zeta", "alpha", "mu", "beta", "omega"];
        let client = reqwest::Client::new();
        let settings = settings_for("http://127.0.0.1:1/v1/systemone");
        let mut previous: Option<String> = None;
        for run in 0..100 {
            let mut pairs: Vec<(&str, &str)> = names.iter().map(|name| (*name, "user")).collect();
            pairs.rotate_left(run % names.len());
            let catalog = catalog(&pairs);
            let request =
                build_request(&client, &settings, &catalog, "candidate").expect("request builds");
            let body = request
                .body()
                .and_then(reqwest::Body::as_bytes)
                .expect("json body is in memory");
            let serialized = String::from_utf8(body.to_vec()).expect("body is UTF-8");
            let value: serde_json::Value = serde_json::from_str(&serialized).expect("body is JSON");
            let ids: Vec<&str> = value["questions"]
                .as_array()
                .expect("questions is an array")
                .iter()
                .map(|question| question["id"].as_str().expect("id is a string"))
                .collect();
            assert_eq!(ids, vec!["alpha", "beta", "mu", "omega", "zeta"]);
            if let Some(previous) = &previous {
                assert_eq!(previous, &serialized, "run {run} changed the wire body");
            }
            previous = Some(serialized);
        }
    }

    /// Test 2: exactly the four top-level body keys, and `Authorization` is the
    /// only credential header.
    #[tokio::test]
    async fn request_body_has_exactly_four_keys_and_authorization_is_the_only_credential_header() {
        let client = reqwest::Client::new();
        let mut settings = settings_for("http://127.0.0.1:1/v1/systemone");
        settings.api_key = "super-secret-key".to_string();
        let catalog = catalog(&[("a", "user")]);
        let request =
            build_request(&client, &settings, &catalog, "the candidate").expect("request builds");

        let body = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .expect("json body is in memory");
        let value: serde_json::Value = serde_json::from_slice(body).expect("body is JSON");
        let keys: BTreeSet<&str> = value
            .as_object()
            .expect("body is an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from(["context", "model", "primitive", "questions"])
        );
        assert_eq!(value["primitive"], "noul");
        assert_eq!(value["model"], "jev-1.13.0");
        assert_eq!(value["context"], "the candidate");

        let headers = request.headers();
        assert_eq!(
            headers
                .get(reqwest::header::AUTHORIZATION)
                .expect("authorization is set"),
            "Bearer super-secret-key"
        );
        for name in headers.keys() {
            let lowered = name.as_str().to_ascii_lowercase();
            if lowered == "authorization" {
                continue;
            }
            assert!(
                !lowered.contains("key")
                    && !lowered.contains("token")
                    && !lowered.contains("secret"),
                "unexpected credential header {name}"
            );
        }
        assert!(!String::from_utf8_lossy(body).contains("super-secret-key"));
    }

    /// Test 3: winner 0.85, runner-up 0.60 is accepted.
    #[tokio::test]
    async fn winner_above_threshold_and_margin_is_classified() {
        let (url, _hits) = serve(vec![(200, body_with(&[("a", 0.85), ("b", 0.60)]))], None).await;
        let network = OutboundNetwork::new_for_tests(4);
        let settings = settings_for(&url);
        let catalog = catalog(&[("a", "user"), ("b", "root")]);
        let outcome = classify(&network, &settings, &catalog, "candidate", "test").await;
        match outcome {
            ClassifyOutcome::Classified {
                category,
                destination,
                score,
                runner_up,
            } => {
                assert_eq!(category, "a");
                assert_eq!(destination, Destination::User);
                assert!((score - 0.85).abs() < 1e-6);
                assert!((runner_up - 0.60).abs() < 1e-6);
            }
            other => panic!("expected a classification, got {other:?}"),
        }
    }

    /// Test 4: winner 0.85, runner-up 0.80 abstains on the margin.
    #[tokio::test]
    async fn winner_below_margin_abstains() {
        let (url, _hits) = serve(vec![(200, body_with(&[("a", 0.85), ("b", 0.80)]))], None).await;
        let network = OutboundNetwork::new_for_tests(4);
        let settings = settings_for(&url);
        let catalog = catalog(&[("a", "user"), ("b", "user")]);
        let outcome = classify(&network, &settings, &catalog, "candidate", "test").await;
        match outcome {
            ClassifyOutcome::Abstained { reason } => {
                assert!(reason.contains("margin"), "{reason}");
                assert!(reason.contains("0.05"), "{reason}");
            }
            other => panic!("expected an abstention, got {other:?}"),
        }
    }

    /// Test 5: a single category below the threshold fails it; above it passes
    /// both the threshold and the margin with runner-up 0.0.
    #[tokio::test]
    async fn single_category_below_threshold_abstains_and_above_it_passes() {
        let catalog = catalog(&[("only", "user")]);
        let network = OutboundNetwork::new_for_tests(4);

        let (low_url, _) = serve(vec![(200, body_with(&[("only", 0.65)]))], None).await;
        let low = classify(
            &network,
            &settings_for(&low_url),
            &catalog,
            "candidate",
            "test",
        )
        .await;
        match low {
            ClassifyOutcome::Abstained { reason } => {
                assert!(reason.contains("threshold"), "{reason}")
            }
            other => panic!("0.65 must abstain, got {other:?}"),
        }

        let (high_url, _) = serve(vec![(200, body_with(&[("only", 0.85)]))], None).await;
        let high = classify(
            &network,
            &settings_for(&high_url),
            &catalog,
            "candidate",
            "test",
        )
        .await;
        assert!(
            matches!(high, ClassifyOutcome::Classified { .. }),
            "0.85 alone must pass, got {high:?}"
        );
    }

    /// Test 6: 429 and 503 each retry exactly once, then abstain.
    #[tokio::test]
    async fn transport_errors_retry_exactly_once_then_abstain() {
        for status in [429_u16, 503] {
            let (url, hits) =
                serve(vec![(status, String::new()), (status, String::new())], None).await;
            let network = OutboundNetwork::new_for_tests(4);
            let settings = settings_for(&url);
            let catalog = catalog(&[("a", "user")]);
            let outcome = classify(&network, &settings, &catalog, "candidate", "test").await;
            assert!(
                matches!(outcome, ClassifyOutcome::Abstained { .. }),
                "HTTP {status} must abstain after the retry, got {outcome:?}"
            );
            assert_eq!(
                hits.load(AtomicOrdering::SeqCst),
                2,
                "HTTP {status} must retry exactly once"
            );
        }
    }

    /// Test 7: a 403 is permanent; zero retries.
    #[tokio::test]
    async fn permanent_4xx_abstains_with_zero_retries() {
        let (url, hits) = serve(vec![(403, String::new())], None).await;
        let network = OutboundNetwork::new_for_tests(4);
        let settings = settings_for(&url);
        let catalog = catalog(&[("a", "user")]);
        let outcome = classify(&network, &settings, &catalog, "candidate", "test").await;
        match outcome {
            ClassifyOutcome::Abstained { reason } => {
                assert!(reason.contains("403"), "{reason}")
            }
            other => panic!("403 must abstain, got {other:?}"),
        }
        assert_eq!(hits.load(AtomicOrdering::SeqCst), 1, "403 is never retried");
    }

    /// Test 8: an out-of-range noul, a missing `results`, a duplicate id and an
    /// unknown id are all malformed with zero retries. Each case also asserts
    /// the rule that fired, so deleting one rule cannot leave this test green.
    #[tokio::test]
    async fn malformed_bodies_abstain_with_zero_retries() {
        let cases = [
            (body_with(&[("a", 1.4)]), "out of [0,1]"),
            (r#"{"other": []}"#.to_string(), "malformed response"),
            (
                body_with(&[("a", 0.9), ("b", 0.9), ("a", 0.5)]),
                "duplicate id",
            ),
            (
                body_with(&[("a", 0.9), ("never-requested", 0.9)]),
                "unknown id",
            ),
        ];
        for (body, expected) in cases {
            let (url, hits) = serve(vec![(200, body.clone())], None).await;
            let network = OutboundNetwork::new_for_tests(4);
            let settings = settings_for(&url);
            let catalog = catalog(&[("a", "user"), ("b", "user")]);
            let outcome = classify(&network, &settings, &catalog, "candidate", "test").await;
            match outcome {
                ClassifyOutcome::Abstained { reason } => assert!(
                    reason.contains(expected),
                    "body {body} must fail with {expected:?}, got {reason:?}"
                ),
                other => panic!("body {body} must abstain, got {other:?}"),
            }
            assert_eq!(
                hits.load(AtomicOrdering::SeqCst),
                1,
                "body {body} must not be retried"
            );
        }
    }

    /// Test 9: the request timeout bounds the call and is never retried.
    #[tokio::test]
    async fn timeout_abstains_without_retry() {
        let (url, hits) = serve(
            vec![(200, body_with(&[("a", 0.9)]))],
            Some(Duration::from_secs(3)),
        )
        .await;
        let network = OutboundNetwork::new_for_tests(4);
        let mut settings = settings_for(&url);
        settings.timeout_secs = 1;
        let catalog = catalog(&[("a", "user")]);
        let started = std::time::Instant::now();
        let outcome = classify(&network, &settings, &catalog, "candidate", "test").await;
        assert!(
            matches!(outcome, ClassifyOutcome::Abstained { .. }),
            "a timeout must abstain, got {outcome:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the call must be bounded by the request timeout"
        );
        assert_eq!(
            hits.load(AtomicOrdering::SeqCst),
            1,
            "a timeout is never retried"
        );
    }

    /// Test 10 (jev half): an absent catalog is inert and issues no request.
    #[tokio::test]
    async fn absent_catalog_is_inert_and_issues_no_request() {
        let (url, hits) = serve(vec![(200, body_with(&[("a", 0.9)]))], None).await;
        let network = OutboundNetwork::new_for_tests(4);
        let settings = settings_for(&url);
        let outcome = classify(
            &network,
            &settings,
            &Catalog::missing(),
            "candidate",
            "test",
        )
        .await;
        match outcome {
            ClassifyOutcome::Abstained { reason } => {
                assert!(reason.contains("NoCatalogFile"), "{reason}")
            }
            other => panic!("a missing catalog must be inert, got {other:?}"),
        }
        assert_eq!(
            hits.load(AtomicOrdering::SeqCst),
            0,
            "no request may be issued"
        );
    }

    #[tokio::test]
    async fn a_missing_api_key_is_inert_and_issues_no_request() {
        let (url, hits) = serve(vec![(200, body_with(&[("a", 0.9)]))], None).await;
        let network = OutboundNetwork::new_for_tests(4);
        let mut settings = settings_for(&url);
        settings.api_key = "  ".to_string();
        let catalog = catalog(&[("a", "user")]);
        let outcome = classify(&network, &settings, &catalog, "candidate", "test").await;
        match outcome {
            ClassifyOutcome::Abstained { reason } => {
                assert!(reason.contains("NoApiKey"), "{reason}")
            }
            other => panic!("a missing API key must be inert, got {other:?}"),
        }
        assert_eq!(
            hits.load(AtomicOrdering::SeqCst),
            0,
            "no request may be issued"
        );
    }

    /// Test 11: a fifth destination wins the call and abstains; a valid sibling
    /// still classifies.
    #[tokio::test]
    async fn a_fifth_destination_abstains_while_a_valid_sibling_classifies() {
        let catalog = catalog(&[("a", "user"), ("b", "banana")]);

        let (url, _) = serve(vec![(200, body_with(&[("a", 0.10), ("b", 0.90)]))], None).await;
        let network = OutboundNetwork::new_for_tests(4);
        let outcome = classify(&network, &settings_for(&url), &catalog, "candidate", "test").await;
        match outcome {
            ClassifyOutcome::Abstained { reason } => {
                assert!(reason.contains("\"b\""), "{reason}");
                assert!(reason.contains("banana"), "{reason}");
            }
            other => panic!("the fifth destination must abstain, got {other:?}"),
        }

        let (url, _) = serve(vec![(200, body_with(&[("a", 0.90), ("b", 0.10)]))], None).await;
        let outcome = classify(&network, &settings_for(&url), &catalog, "candidate", "test").await;
        assert!(
            matches!(outcome, ClassifyOutcome::Classified { ref category, .. } if category == "a"),
            "the valid sibling must still classify, got {outcome:?}"
        );
    }

    /// Test 12 (jev half): an orchestrator without a peer and a default_reply
    /// without a reply each abstain, naming the category.
    #[tokio::test]
    async fn orchestrator_without_peer_and_default_reply_without_reply_abstain() {
        for (name, destination) in [
            ("needs-peer", "orchestrator"),
            ("needs-reply", "default_reply"),
        ] {
            let (url, _) = serve(vec![(200, body_with(&[(name, 0.90)]))], None).await;
            let network = OutboundNetwork::new_for_tests(4);
            let catalog = catalog(&[(name, destination)]);
            let outcome =
                classify(&network, &settings_for(&url), &catalog, "candidate", "test").await;
            match outcome {
                ClassifyOutcome::Abstained { reason } => {
                    assert!(reason.contains(name), "{reason}")
                }
                other => panic!("{name} must abstain, got {other:?}"),
            }
        }
    }

    /// Test 18: the call takes the outbound limiter permit under its own label
    /// and the production half of this file names `general()`, never the
    /// Telegram or Gemini client.
    #[tokio::test]
    async fn jev_uses_the_general_client_and_its_own_permit() {
        let (url, _) = serve(vec![(200, body_with(&[("a", 0.9)]))], None).await;
        let network = OutboundNetwork::new_for_tests(4);
        let catalog = catalog(&[("a", "user")]);
        let outcome = classify(&network, &settings_for(&url), &catalog, "candidate", "test").await;
        assert!(
            matches!(outcome, ClassifyOutcome::Classified { .. }),
            "got {outcome:?}"
        );
        assert_eq!(network.acquired_labels_for_tests(), vec![NETWORK_LABEL]);

        let source = include_str!("jev.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("production half");
        assert!(production.contains("network.general()"));
        assert!(!production.contains(".telegram()"));
        assert!(!production.contains(".gemini()"));
    }

    /// Test 19: a model other than `jev-1.13.0` still classifies, and an
    /// abstention names the model in use.
    #[tokio::test]
    async fn a_non_default_model_classifies_and_names_itself_in_abstentions() {
        let catalog = catalog(&[("a", "user")]);

        let (url, _) = serve(vec![(200, body_with(&[("a", 0.90)]))], None).await;
        let network = OutboundNetwork::new_for_tests(4);
        let mut settings = settings_for(&url);
        settings.model = "jev-9.9.9".to_string();
        let classified = classify(&network, &settings, &catalog, "candidate", "test").await;
        assert!(
            matches!(classified, ClassifyOutcome::Classified { .. }),
            "got {classified:?}"
        );
        let request = build_request(network.general(), &settings, &catalog, "candidate")
            .expect("request builds");
        let body: serde_json::Value = serde_json::from_slice(
            request
                .body()
                .and_then(reqwest::Body::as_bytes)
                .expect("json body"),
        )
        .expect("body is JSON");
        assert_eq!(body["model"], "jev-9.9.9");

        let (url, _) = serve(vec![(200, body_with(&[("a", 0.10)]))], None).await;
        let mut settings = settings_for(&url);
        settings.model = "jev-9.9.9".to_string();
        let abstained = classify(&network, &settings, &catalog, "candidate", "test").await;
        match abstained {
            ClassifyOutcome::Abstained { reason } => {
                assert!(reason.contains("jev-9.9.9"), "{reason}")
            }
            other => panic!("a low score must abstain, got {other:?}"),
        }
    }
}
