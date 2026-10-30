use super::*;

#[test]
fn persisted_token_requires_config_directory() {
    assert_eq!(read_persisted_web_token(None), Err("No config dir".into()));
}

#[test]
fn persisted_token_preserves_unicode_and_whitespace() {
    let dir = tempfile::tempdir().unwrap();
    let token = " \t\n雪-token\u{2003}\r\n";
    std::fs::write(dir.path().join("web-token.txt"), token).unwrap();

    assert_eq!(read_persisted_web_token(Some(dir.path())).unwrap(), token);
}

#[test]
fn persisted_token_accepts_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("web-token.txt"), "").unwrap();

    assert_eq!(read_persisted_web_token(Some(dir.path())).unwrap(), "");
}

#[test]
fn persisted_token_rereads_replaced_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web-token.txt");
    std::fs::write(&path, "first").unwrap();
    assert_eq!(read_persisted_web_token(Some(dir.path())).unwrap(), "first");

    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, "replacement").unwrap();
    assert_eq!(
        read_persisted_web_token(Some(dir.path())).unwrap(),
        "replacement"
    );
}

#[test]
fn persisted_token_preserves_missing_file_error() {
    let dir = tempfile::tempdir().unwrap();
    let error = std::fs::read_to_string(dir.path().join("web-token.txt")).unwrap_err();

    assert_eq!(
        read_persisted_web_token(Some(dir.path())),
        Err(format!("Cannot read web token: {error}"))
    );
}

#[test]
fn persisted_token_preserves_invalid_utf8_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web-token.txt");
    std::fs::write(&path, [0xff]).unwrap();
    let error = std::fs::read_to_string(&path).unwrap_err();

    assert_eq!(
        read_persisted_web_token(Some(dir.path())),
        Err(format!("Cannot read web token: {error}"))
    );
}

#[test]
fn persisted_token_preserves_directory_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web-token.txt");
    std::fs::create_dir(&path).unwrap();
    let error = std::fs::read_to_string(&path).unwrap_err();

    assert_eq!(
        read_persisted_web_token(Some(dir.path())),
        Err(format!("Cannot read web token: {error}"))
    );
}
