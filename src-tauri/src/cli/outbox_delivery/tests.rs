use super::*;

#[test]
fn delivered_takes_priority_over_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let outbox = temp.path();
    std::fs::create_dir(outbox.join("delivered")).unwrap();
    std::fs::create_dir(outbox.join("rejected")).unwrap();
    std::fs::write(outbox.join("delivered/fixed-id.json"), "").unwrap();
    std::fs::write(outbox.join("rejected/fixed-id.reason.txt"), "rejected").unwrap();

    assert!(confirm(outbox, "fixed-id").is_ok());
}

#[test]
fn rejection_reason_is_trimmed() {
    let temp = tempfile::tempdir().unwrap();
    let outbox = temp.path();
    std::fs::create_dir(outbox.join("rejected")).unwrap();
    std::fs::write(outbox.join("rejected/fixed-id.reason.txt"), "  denied \n").unwrap();

    assert!(matches!(
        confirm(outbox, "fixed-id"),
        Err(ConfirmationError::Rejected(reason)) if reason == "denied"
    ));
}

#[test]
fn unreadable_rejection_uses_unknown_reason() {
    let temp = tempfile::tempdir().unwrap();
    let outbox = temp.path();
    std::fs::create_dir_all(outbox.join("rejected/fixed-id.reason.txt")).unwrap();

    assert!(matches!(
        confirm(outbox, "fixed-id"),
        Err(ConfirmationError::Rejected(reason)) if reason == "unknown reason"
    ));
}

#[test]
fn delivery_publishes_exact_pretty_json_with_original_id() {
    let temp = tempfile::tempdir().unwrap();
    let outbox = temp.path().join("outbox");
    let message = super::super::self_restart::build_self_restart_outbox_message(
        temp.path().to_str().unwrap(),
        None,
        "fixed-id".to_string(),
        "fixed-request".to_string(),
    );
    std::fs::create_dir_all(outbox.join("delivered")).unwrap();
    std::fs::write(outbox.join("delivered/fixed-id.json"), "").unwrap();

    assert!(deliver(&outbox, &message, "self-handoff-and-restart").is_ok());
    let published = std::fs::read_to_string(outbox.join("fixed-id.json")).unwrap();
    assert_eq!(published, serde_json::to_string_pretty(&message).unwrap());
    let parsed: serde_json::Value = serde_json::from_str(&published).unwrap();
    assert_eq!(parsed["id"], "fixed-id");
}

#[test]
fn file_outbox_reports_native_creation_error_without_publication() {
    let temp = tempfile::tempdir().unwrap();
    let outbox = temp.path().join("outbox");
    let message = super::super::self_restart::build_self_restart_outbox_message(
        temp.path().to_str().unwrap(),
        None,
        "fixed-id".to_string(),
        "fixed-request".to_string(),
    );
    std::fs::write(&outbox, "existing file").unwrap();
    let native_error = std::fs::create_dir_all(&outbox).unwrap_err();

    assert_eq!(
        deliver(&outbox, &message, "self-handoff-and-restart"),
        Err(format!("failed to create outbox directory: {native_error}"))
    );
    assert_eq!(std::fs::read_to_string(&outbox).unwrap(), "existing file");
    assert!(!outbox.join("fixed-id.json").exists());
}

#[test]
fn rejected_delivery_preserves_publication_and_action_error() {
    let temp = tempfile::tempdir().unwrap();
    let outbox = temp.path().join("outbox");
    let message = super::super::self_restart::build_self_restart_outbox_message(
        temp.path().to_str().unwrap(),
        None,
        "fixed-id".to_string(),
        "fixed-request".to_string(),
    );
    std::fs::create_dir_all(outbox.join("rejected")).unwrap();
    std::fs::write(outbox.join("rejected/fixed-id.reason.txt"), " denied ").unwrap();

    assert_eq!(
        deliver(&outbox, &message, "self-handoff-and-restart"),
        Err("self-handoff-and-restart rejected - denied".to_string())
    );
    assert_eq!(
        std::fs::read_to_string(outbox.join("fixed-id.json")).unwrap(),
        serde_json::to_string_pretty(&message).unwrap()
    );
}
