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
