use super::*;

#[test]
fn raw_window_id_validation_rejects_noncanonical_values() {
    for raw_window_id in [
        "",
        "-1",
        "+1",
        " 1",
        "1 ",
        "01",
        "18446744073709551616",
        "123456789012345678901",
        "%30",
        "abc",
    ] {
        assert!(parse_window_id(raw_window_id).is_none(), "{raw_window_id}");
    }
    assert_eq!(parse_window_id("0"), Some("0".to_string()));
    assert_eq!(
        parse_window_id("18446744073709551615"),
        Some("18446744073709551615".to_string())
    );
}

#[test]
fn raw_window_id_extraction_preserves_percent_encoding() {
    let uri: axum::http::Uri = match "/api/v1/windows/%FF/screenshot".parse() {
        Ok(uri) => uri,
        Err(error) => panic!("test URI must parse: {error}"),
    };
    let original_uri = OriginalUri(uri);
    assert!(matches!(extract_raw_window_id(&original_uri), Ok("%FF")));
    assert!(parse_window_id("%FF").is_none());
}
