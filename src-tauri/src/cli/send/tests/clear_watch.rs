use super::super::wait_for_delivery_confirmation;
use std::time::Duration;

#[test]
fn issue_3371_confirmation_preserves_exact_failure_and_healthy_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let reason = "codex_clear_failed: provider reported fresh-session startup failure";
    std::fs::create_dir(temp.path().join("rejected")).unwrap();
    std::fs::write(temp.path().join("rejected/failed.reason.txt"), reason).unwrap();
    assert_eq!(
        wait_for_delivery_confirmation(
            temp.path(),
            "failed",
            "wake",
            "target",
            Duration::ZERO,
            Duration::ZERO
        ),
        Err(format!("message rejected: {reason}"))
    );
    std::fs::create_dir(temp.path().join("delivered")).unwrap();
    std::fs::write(temp.path().join("delivered/healthy.json"), "{}").unwrap();
    assert_eq!(
        wait_for_delivery_confirmation(
            temp.path(),
            "healthy",
            "wake",
            "target",
            Duration::ZERO,
            Duration::ZERO
        ),
        Ok(())
    );
}

#[test]
fn issue_3371_confirmation_timeout_keeps_id_and_pending_bytes_without_resending() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("pending.json");
    let bytes = br#"{"id":"pending","command":"clear"}"#;
    std::fs::write(&source, bytes).unwrap();
    let error = wait_for_delivery_confirmation(
        temp.path(),
        "pending",
        "wake",
        "target",
        Duration::ZERO,
        Duration::ZERO,
    )
    .unwrap_err();
    assert!(error.contains("pending"));
    assert!(error.contains("may still be pending"));
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
}
