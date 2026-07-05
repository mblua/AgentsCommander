//! `GET /api/v1/peers` (#791 §6.2). Mirrors `list-peers-lean` for the caller's
//! bound replica; `reachable` is computed from the bound FQN, so a client sees
//! exactly the peers its identity may reach.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use uuid::Uuid;

use crate::api::actuation;
use crate::api::auth::SCOPE_LIST_PEERS;
use crate::api::error::ApiError;
use crate::api::schema::{PeersResponse, API_VERSION};
use crate::api::{handlers, ApiState};

/// Axum entry point for `GET /api/v1/peers`.
pub async fn handle(
    State(state): State<ApiState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    match handle_inner(&state, addr.ip(), &headers, query.as_deref()) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => e.into_response(),
    }
}

fn handle_inner(
    state: &ApiState,
    ip: std::net::IpAddr,
    headers: &HeaderMap,
    query: Option<&str>,
) -> Result<PeersResponse, ApiError> {
    let client = handlers::authenticate(state, headers, ip, SCOPE_LIST_PEERS)?;
    // Validate the bound replica still exists and is not the root agent. The
    // derived FQN is not needed here (lean_peers computes reachability from the
    // root itself), but the validation must run.
    crate::api::identity::resolve_from(&client)?;

    let mut peers = actuation::lean_peers_via_api(&client.bound_root)?;

    let filters = parse_peer_filters(query);
    if !filters.is_empty() {
        peers.retain(|p| filters.iter().any(|f| f == &p.name));
    }

    Ok(PeersResponse {
        api_version: API_VERSION.to_string(),
        // list-peers-lean is safely re-runnable, so opId is server-generated.
        op_id: Uuid::new_v4().to_string(),
        peers,
    })
}

/// Parse repeatable `?peer=<fqn>` filters from a raw query string, percent-
/// decoding each value. Non-`peer` keys are ignored.
fn parse_peer_filters(query: Option<&str>) -> Vec<String> {
    let Some(query) = query else {
        return Vec::new();
    };
    query
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            if key == "peer" {
                Some(percent_decode(value))
            } else {
                None
            }
        })
        .filter(|v| !v.is_empty())
        .collect()
}

/// Minimal `application/x-www-form-urlencoded` value decode: `+` -> space and
/// `%XX` -> byte. Invalid escapes are passed through literally.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_query_yields_no_filters() {
        assert!(parse_peer_filters(None).is_empty());
        assert!(parse_peer_filters(Some("")).is_empty());
    }

    #[test]
    fn parses_repeatable_peer_filter() {
        let q = "peer=proj%3Awg-1%2Fdev&peer=proj%3Awg-1%2Flead&other=x";
        let filters = parse_peer_filters(Some(q));
        assert_eq!(filters, vec!["proj:wg-1/dev", "proj:wg-1/lead"]);
    }

    #[test]
    fn ignores_non_peer_keys() {
        let filters = parse_peer_filters(Some("token=abc&foo=bar"));
        assert!(filters.is_empty());
    }

    #[test]
    fn percent_decode_handles_plus_and_escapes() {
        assert_eq!(percent_decode("a%3Ab"), "a:b");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("plain"), "plain");
        // Invalid escape is passed through literally.
        assert_eq!(percent_decode("a%zz"), "a%zz");
    }
}
