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
