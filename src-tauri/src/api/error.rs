//! Typed API errors and their HTTP mapping (#791).
//!
//! Error bodies never leak host absolute paths: `scrub_host_paths` redacts any
//! Windows drive-letter path (e.g. an outbox / messaging dir that an internal
//! error string might carry) before the detail crosses the socket.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::schema::{ErrorBody, API_VERSION};

/// A control-plane API failure with a fixed HTTP mapping.
#[derive(Debug, Clone)]
pub enum ApiError {
    /// 400 - malformed body, missing/forbidden field, unsupported `inline`.
    BadRequest(String),
    /// 401 - missing/invalid/revoked/expired token, or bound replica gone.
    Unauthorized(String),
    /// 403 - valid token but not scoped / routing denied / root-agent excluded.
    Forbidden(String),
    /// 422 - the engine refused delivery (`deliver_wake` returned `Err`).
    Rejected(String),
    /// 429 - per-source failed-auth lockout or per-client rate limit.
    TooManyRequests(String),
    /// 500 - internal error (never leaks a host path).
    Internal(String),
}

impl ApiError {
    fn parts(&self) -> (StatusCode, &'static str, &str) {
        match self {
            ApiError::BadRequest(d) => (StatusCode::BAD_REQUEST, "bad_request", d),
            ApiError::Unauthorized(d) => (StatusCode::UNAUTHORIZED, "unauthorized", d),
            ApiError::Forbidden(d) => (StatusCode::FORBIDDEN, "forbidden", d),
            ApiError::Rejected(d) => (StatusCode::UNPROCESSABLE_ENTITY, "rejected", d),
            ApiError::TooManyRequests(d) => (StatusCode::TOO_MANY_REQUESTS, "rate_limited", d),
            ApiError::Internal(d) => (StatusCode::INTERNAL_SERVER_ERROR, "internal", d),
        }
    }

    /// The HTTP status this error maps to (used by handlers/tests).
    pub fn status(&self) -> StatusCode {
        self.parts().0
    }

    /// The stable machine code (used by tests).
    pub fn code(&self) -> &'static str {
        self.parts().1
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, detail) = self.parts();
        let body = ErrorBody {
            api_version: API_VERSION.to_string(),
            error: code.to_string(),
            detail: scrub_host_paths(detail),
        };
        (status, Json(body)).into_response()
    }
}

/// Redact Windows drive-letter absolute paths from an error detail so a host
/// filesystem layout never crosses the socket. Replaces each `X:\...` /
/// `X:/...` run (up to the next whitespace) with `<path>`. Conservative: it
/// only touches drive-letter runs, leaving FQNs / agent names intact.
pub fn scrub_host_paths(detail: &str) -> String {
    let mut out = String::with_capacity(detail.len());
    let mut chars = detail.char_indices().peekable();
    let bytes = detail.as_bytes();
    while let Some((i, c)) = chars.next() {
        // A drive-letter path starts as `<letter>:` followed by `\` or `/`.
        let is_drive_start = c.is_ascii_alphabetic()
            && bytes.get(i + 1) == Some(&b':')
            && matches!(bytes.get(i + 2), Some(&b'\\') | Some(&b'/'))
            // Must be at a token boundary so `https://` (no drive letter) and
            // mid-word colons are not misread. Preceding char is start-or-space.
            && (i == 0 || bytes.get(i - 1).map(|b| b.is_ascii_whitespace()).unwrap_or(false));
        if is_drive_start {
            out.push_str("<path>");
            // Consume until whitespace.
            for (_, cc) in chars.by_ref() {
                if cc.is_whitespace() {
                    out.push(cc);
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_redacts_windows_drive_path() {
        let s = "failed to open C:\\Users\\maria\\repo\\messaging\\x.md now";
        let scrubbed = scrub_host_paths(s);
        assert!(!scrubbed.contains("C:\\Users"));
        assert!(scrubbed.contains("<path>"));
        assert!(scrubbed.contains("failed to open"));
        assert!(scrubbed.contains("now"));
    }

    #[test]
    fn scrub_redacts_forward_slash_drive_path() {
        let s = "outbox D:/data/outbox failed";
        let scrubbed = scrub_host_paths(s);
        assert!(!scrubbed.contains("D:/data"));
        assert!(scrubbed.contains("<path>"));
    }

    #[test]
    fn scrub_leaves_non_paths_intact() {
        let s = "routing rejected: 'proj:wg-1/dev-rust' cannot reach 'proj:wg-1/tech-lead'";
        assert_eq!(scrub_host_paths(s), s);
    }

    #[test]
    fn scrub_does_not_treat_url_as_drive_path() {
        // `https://host` has no drive letter (h,t,t,p,s before `:`), so it stays.
        let s = "see https://example.com/x for detail";
        assert_eq!(scrub_host_paths(s), s);
    }

    #[test]
    fn error_status_mapping() {
        assert_eq!(ApiError::BadRequest("x".into()).status(), StatusCode::BAD_REQUEST);
        assert_eq!(ApiError::Unauthorized("x".into()).status(), StatusCode::UNAUTHORIZED);
        assert_eq!(ApiError::Forbidden("x".into()).status(), StatusCode::FORBIDDEN);
        assert_eq!(
            ApiError::Rejected("x".into()).status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            ApiError::TooManyRequests("x".into()).status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            ApiError::Internal("x".into()).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
