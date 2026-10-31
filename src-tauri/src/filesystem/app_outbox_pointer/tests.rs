use super::*;

#[test]
fn absent_config_has_no_destination() {
    assert_eq!(get(None), None);
}

#[test]
fn absent_pointer_has_no_destination() {
    let config = tempfile::tempdir().unwrap();
    assert_eq!(get(Some(config.path())), None);
}

#[test]
fn directory_pointer_has_no_destination() {
    let config = tempfile::tempdir().unwrap();
    std::fs::create_dir(config.path().join(BASENAME)).unwrap();
    assert_eq!(get(Some(config.path())), None);
}

#[test]
fn invalid_utf8_has_no_destination() {
    let config = tempfile::tempdir().unwrap();
    std::fs::write(config.path().join(BASENAME), [0xff, 0xfe]).unwrap();
    assert_eq!(get(Some(config.path())), None);
}

#[test]
fn trims_whitespace_around_directory() {
    let config = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let text = format!(" \t{}\r\n ", destination.path().display());
    std::fs::write(config.path().join(BASENAME), text).unwrap();
    assert_eq!(
        get(Some(config.path())),
        Some(destination.path().to_owned())
    );
}

#[test]
fn file_destination_is_rejected() {
    let config = tempfile::tempdir().unwrap();
    let destination = config.path().join("file");
    std::fs::write(&destination, b"fixture").unwrap();
    std::fs::write(config.path().join(BASENAME), destination.to_str().unwrap()).unwrap();
    assert_eq!(get(Some(config.path())), None);
}

#[test]
fn missing_destination_is_rejected() {
    let config = tempfile::tempdir().unwrap();
    let destination = config.path().join("missing");
    std::fs::write(config.path().join(BASENAME), destination.to_str().unwrap()).unwrap();
    assert_eq!(get(Some(config.path())), None);
}

#[test]
fn empty_and_blank_text_preserve_native_empty_path_semantics() {
    let config = tempfile::tempdir().unwrap();
    let candidate = PathBuf::from("");
    let expected = candidate.is_dir().then_some(candidate);
    for text in ["", " \t\r\n "] {
        std::fs::write(config.path().join(BASENAME), text).unwrap();
        assert_eq!(get(Some(config.path())), expected);
    }
}

#[test]
fn rereads_pointer_after_replacement() {
    let config = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let pointer = config.path().join(BASENAME);
    std::fs::write(&pointer, first.path().to_str().unwrap()).unwrap();
    assert_eq!(get(Some(config.path())), Some(first.path().to_owned()));
    std::fs::write(&pointer, second.path().to_str().unwrap()).unwrap();
    assert_eq!(get(Some(config.path())), Some(second.path().to_owned()));
}

#[test]
fn absent_config_selects_fallback_without_creating_it() {
    let agent = tempfile::tempdir().unwrap();
    let fallback = agent.path().join("outbox");
    assert_eq!(directory(None, agent.path()), fallback);
    assert!(!fallback.exists());
}

#[test]
fn valid_pointer_selects_destination_instead_of_fallback() {
    let config = tempfile::tempdir().unwrap();
    let agent = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    std::fs::write(config.path().join(BASENAME), destination.path().to_str().unwrap()).unwrap();
    let selected = directory(Some(config.path()), agent.path());
    assert_eq!(selected, destination.path());
    assert_ne!(selected, agent.path().join("outbox"));
}

#[test]
fn absent_pointer_selects_fallback_without_creating_it() {
    let config = tempfile::tempdir().unwrap();
    let agent = tempfile::tempdir().unwrap();
    let fallback = agent.path().join("outbox");
    assert_eq!(directory(Some(config.path()), agent.path()), fallback);
    assert!(!fallback.exists());
}
