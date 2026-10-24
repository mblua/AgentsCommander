use super::*;

fn catalog(categories: &str) -> Catalog {
    Catalog::from_json_str(&format!("{{\"categories\": {{{categories}}}}}"))
}

#[test]
fn questions_are_byte_sorted_by_category_id() {
    let catalog = catalog(
        r#""zeta": {"destination": "user", "question": "z?"},
           "alpha": {"destination": "root", "question": "a?"},
           "mu": {"destination": "user", "question": "m?"}"#,
    );
    let ids: Vec<&str> = catalog
        .questions_in_order()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, vec!["alpha", "mu", "zeta"]);
}

#[test]
fn a_fifth_destination_marks_only_that_category_invalid() {
    let catalog = catalog(
        r#""a": {"destination": "user", "question": "a?"},
           "b": {"destination": "banana", "question": "b?"}"#,
    );
    assert!(matches!(
        catalog.resolve("a"),
        Resolution::Valid {
            destination: Destination::User,
            ..
        }
    ));
    match catalog.resolve("b") {
        Resolution::Invalid { reason } => {
            assert!(reason.contains("category \"b\""), "{reason}");
            assert!(reason.contains("banana"), "{reason}");
        }
        other => panic!("a fifth destination must be invalid, got {other:?}"),
    }
    let ids: Vec<&str> = catalog
        .questions_in_order()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, vec!["a", "b"], "an invalid sibling must stay askable");
}

#[test]
fn orchestrator_without_peer_is_invalid() {
    let catalog = catalog(r#""needs-peer": {"destination": "orchestrator", "question": "?"}"#);
    match catalog.resolve("needs-peer") {
        Resolution::Invalid { reason } => {
            assert!(reason.contains("needs-peer"), "{reason}");
            assert!(reason.contains("peer"), "{reason}");
        }
        other => panic!("orchestrator without peer must be invalid, got {other:?}"),
    }
}

#[test]
fn default_reply_without_reply_is_invalid() {
    let catalog = catalog(r#""needs-reply": {"destination": "default_reply", "question": "?"}"#);
    match catalog.resolve("needs-reply") {
        Resolution::Invalid { reason } => {
            assert!(reason.contains("needs-reply"), "{reason}");
            assert!(reason.contains("reply"), "{reason}");
        }
        other => panic!("default_reply without reply must be invalid, got {other:?}"),
    }
}

#[test]
fn approval_replies_are_rejected_at_load_with_category_and_phrase() {
    let catalog = Catalog::from_json_str(include_str!("tests/fixtures/approval-reply.json"));
    match catalog.resolve("answer") {
        Resolution::Invalid { reason } => {
            assert!(reason.contains("answer"), "{reason}");
            assert!(reason.contains("approved"), "{reason}");
        }
        other => panic!("an approval reply must be rejected, got {other:?}"),
    }
}

#[test]
fn a_valid_default_reply_is_accepted() {
    let catalog = Catalog::from_json_str(include_str!("tests/fixtures/valid-reply.json"));
    assert!(matches!(
        catalog.resolve("note"),
        Resolution::Valid {
            destination: Destination::DefaultReply,
            ..
        }
    ));
}

#[test]
fn a_missing_file_is_the_inert_state() {
    let catalog = Catalog::load(Path::new("/nonexistent/catalog-phase6.json"));
    assert!(catalog.is_missing());
    assert!(catalog.questions_in_order().is_empty());
    assert_eq!(catalog.resolve("anything"), Resolution::Unknown);
}

#[test]
fn an_unparseable_document_is_whole_catalog_invalid() {
    let catalog = Catalog::from_json_str("not json");
    assert!(!catalog.is_missing());
    assert!(catalog.unparseable_reason().is_some());
    assert!(catalog.questions_in_order().is_empty());
}

#[test]
fn a_sibling_with_a_missing_question_does_not_poison_the_valid_one() {
    let catalog = catalog(
        r#""a": {"destination": "user", "question": "a?"},
           "b": {"destination": "user"}"#,
    );
    assert!(matches!(
        catalog.resolve("a"),
        Resolution::Valid {
            destination: Destination::User,
            ..
        }
    ));
    assert!(matches!(catalog.resolve("b"), Resolution::Invalid { .. }));
    let ids: Vec<&str> = catalog
        .questions_in_order()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, vec!["a"]);
}

#[test]
fn loads_a_valid_document_from_disk() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.json");
    std::fs::write(&path, include_str!("tests/fixtures/valid-reply.json")).unwrap();

    let catalog = Catalog::load(&path);
    assert!(!catalog.is_missing());
    assert_eq!(catalog.unparseable_reason(), None);
    assert!(matches!(
        catalog.resolve("note"),
        Resolution::Valid {
            destination: Destination::DefaultReply,
            ..
        }
    ));
}

#[test]
fn invalid_utf8_preserves_the_unreadable_catalog_reason() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.json");
    std::fs::write(&path, [0xff]).unwrap();
    let error = std::fs::read_to_string(&path).unwrap_err();
    let expected = format!("catalog is unreadable: {error}");

    let catalog = Catalog::load(&path);
    assert!(!catalog.is_missing());
    assert_eq!(catalog.unparseable_reason(), Some(expected.as_str()));
    assert!(catalog.questions_in_order().is_empty());
    assert_eq!(catalog.resolve("note"), Resolution::Unknown);
}

#[test]
fn a_directory_preserves_the_non_missing_read_error() {
    let directory = tempfile::tempdir().unwrap();
    let error = std::fs::read_to_string(directory.path()).unwrap_err();
    assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
    let expected = format!("catalog is unreadable: {error}");

    let catalog = Catalog::load(directory.path());
    assert!(!catalog.is_missing());
    assert_eq!(catalog.unparseable_reason(), Some(expected.as_str()));
    assert!(catalog.questions_in_order().is_empty());
    assert_eq!(catalog.resolve("note"), Resolution::Unknown);
}
