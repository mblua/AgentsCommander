#[cfg(has_embedded_dist)]
use axum::body::Body;
#[cfg(has_embedded_dist)]
use axum::http::{header, HeaderValue, StatusCode, Uri};
#[cfg(has_embedded_dist)]
use axum::response::{IntoResponse, Response};
#[cfg(has_embedded_dist)]
use rust_embed::RustEmbed;

#[cfg(any(has_embedded_dist, test))]
const INDEX_HTML: &str = "index.html";
#[cfg(has_embedded_dist)]
const LONG_CACHE: &str = "public, max-age=31536000, immutable";
#[cfg(has_embedded_dist)]
const NO_CACHE: &str = "no-cache";

#[cfg(has_embedded_dist)]
#[derive(RustEmbed)]
#[folder = "../dist"]
struct EmbeddedDist;

#[cfg(any(has_embedded_dist, test))]
#[derive(Debug, PartialEq, Eq)]
enum StaticRequest {
    Invalid,
    Exact(String),
    SpaFallback,
}

#[cfg(has_embedded_dist)]
pub async fn embedded_static_handler(uri: Uri) -> Response {
    match classify_request_path(uri.path()) {
        StaticRequest::Invalid => not_found(),
        StaticRequest::SpaFallback => serve_embedded(INDEX_HTML, true).unwrap_or_else(not_found),
        StaticRequest::Exact(path) => {
            if let Some(response) = serve_embedded(&path, false) {
                return response;
            }

            if should_spa_fallback(&path) {
                serve_embedded(INDEX_HTML, true).unwrap_or_else(not_found)
            } else {
                not_found()
            }
        }
    }
}

#[cfg(any(has_embedded_dist, test))]
fn classify_request_path(raw_path: &str) -> StaticRequest {
    if is_suspicious_path(raw_path) {
        return StaticRequest::Invalid;
    }

    let path = raw_path.trim_start_matches('/');
    if path.is_empty() || path == INDEX_HTML {
        return StaticRequest::Exact(INDEX_HTML.to_string());
    }

    if path.ends_with('/') {
        if path.starts_with("assets/") || path.starts_with("api/") || path == "ws/" {
            return StaticRequest::Exact(path.to_string());
        }
        return StaticRequest::SpaFallback;
    }

    StaticRequest::Exact(path.to_string())
}

#[cfg(any(has_embedded_dist, test))]
fn is_suspicious_path(raw_path: &str) -> bool {
    let lower = raw_path.to_ascii_lowercase();

    raw_path.contains('\\')
        || lower.contains("%2e")
        || lower.contains("%2f")
        || lower.contains("%5c")
        || raw_path.split('/').any(|part| part == "." || part == "..")
}

#[cfg(any(has_embedded_dist, test))]
fn should_spa_fallback(path: &str) -> bool {
    if path == INDEX_HTML
        || path.starts_with("assets/")
        || path.starts_with("api/")
        || path == "ws"
        || path.starts_with("ws/")
    {
        return false;
    }

    let last_segment = path.rsplit('/').next().unwrap_or(path);
    !last_segment.contains('.')
}

#[cfg(has_embedded_dist)]
fn serve_embedded(path: &str, spa_fallback: bool) -> Option<Response> {
    let file = EmbeddedDist::get(path)?;
    let mut response = Response::new(Body::from(file.data.into_owned()));

    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, content_type_for(path));
    headers.insert(header::CACHE_CONTROL, cache_control_for(path, spa_fallback));

    Some(response)
}

#[cfg(has_embedded_dist)]
fn content_type_for(path: &str) -> HeaderValue {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    HeaderValue::from_str(mime.essence_str())
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"))
}

#[cfg(has_embedded_dist)]
fn cache_control_for(path: &str, spa_fallback: bool) -> HeaderValue {
    if spa_fallback || path == INDEX_HTML {
        HeaderValue::from_static(NO_CACHE)
    } else if path.starts_with("assets/") {
        HeaderValue::from_static(LONG_CACHE)
    } else {
        HeaderValue::from_static(NO_CACHE)
    }
}

#[cfg(has_embedded_dist)]
fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "Not found").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_and_index_resolve_to_index() {
        assert_eq!(
            classify_request_path("/"),
            StaticRequest::Exact("index.html".to_string())
        );
        assert_eq!(
            classify_request_path("/index.html"),
            StaticRequest::Exact("index.html".to_string())
        );
    }

    #[test]
    fn exact_asset_paths_are_asset_candidates() {
        assert_eq!(
            classify_request_path("/assets/app.123.js"),
            StaticRequest::Exact("assets/app.123.js".to_string())
        );
    }

    #[test]
    fn extensionless_app_routes_use_spa_fallback() {
        assert!(should_spa_fallback("sessions/abc"));
        assert_eq!(
            classify_request_path("/sessions/"),
            StaticRequest::SpaFallback
        );
    }

    #[test]
    fn file_like_and_api_paths_do_not_use_spa_fallback() {
        assert!(!should_spa_fallback("assets/missing.js"));
        assert!(!should_spa_fallback("favicon.ico"));
        assert!(!should_spa_fallback("api/unknown"));
        assert!(!should_spa_fallback("ws/unknown"));
    }

    #[test]
    fn suspicious_paths_are_rejected() {
        assert_eq!(classify_request_path("/../secret"), StaticRequest::Invalid);
        assert_eq!(classify_request_path("/foo\\bar"), StaticRequest::Invalid);
        assert_eq!(
            classify_request_path("/%2e%2e/secret"),
            StaticRequest::Invalid
        );
        assert_eq!(
            classify_request_path("/foo/%5c/bar"),
            StaticRequest::Invalid
        );
    }

    #[cfg(has_embedded_dist)]
    #[test]
    fn cache_headers_match_asset_type() {
        assert_eq!(
            cache_control_for(INDEX_HTML, false),
            HeaderValue::from_static(NO_CACHE)
        );
        assert_eq!(
            cache_control_for("sessions/abc", true),
            HeaderValue::from_static(NO_CACHE)
        );
        assert_eq!(
            cache_control_for("assets/app.123.js", false),
            HeaderValue::from_static(LONG_CACHE)
        );
    }

    #[cfg(has_embedded_dist)]
    #[test]
    fn content_type_uses_mime_guess() {
        assert_eq!(
            content_type_for("assets/app.123.css"),
            HeaderValue::from_static("text/css")
        );
        assert_eq!(
            content_type_for("assets/app.123.js"),
            HeaderValue::from_static("text/javascript")
        );
    }
}
