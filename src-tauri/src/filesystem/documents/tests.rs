use super::*;

#[test]
fn publish_web_access_token_exact_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let token = "web-token-é";
    publish_web_access_token(dir.path(), token).unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("web-token.txt")).unwrap(),
        token.as_bytes()
    );
}

#[test]
fn publish_web_access_token_truncates() {
    let dir = tempfile::tempdir().unwrap();
    publish_web_access_token(dir.path(), "long-existing-token").unwrap();
    publish_web_access_token(dir.path(), "short").unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("web-token.txt")).unwrap(),
        b"short"
    );
}

#[test]
fn publish_web_access_token_missing_directory() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing");
    assert!(publish_web_access_token(&missing, "token").is_err());
    assert!(!missing.exists());
}

#[test]
fn publish_web_access_token_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let token = "round-trip-token";
    publish_web_access_token(dir.path(), token).unwrap();
    assert_eq!(
        crate::web::auth::read_persisted_web_token(Some(dir.path())).unwrap(),
        token
    );
}
