use std::path::Path;

fn source(relative: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

#[test]
fn every_production_pty_input_surface_uses_the_per_session_permit() {
    for relative in [
        "src/commands/pty.rs",
        "src/web/mod.rs",
        "src/web/commands.rs",
        "src/pty/inject.rs",
        "src/phone/mailbox.rs",
    ] {
        let body = source(relative);
        assert!(
            body.contains("write_with_permit"),
            "{relative} must route PTY input through write_with_permit"
        );
    }

    let manager = source("src/pty/manager.rs");
    assert!(manager.contains("pub fn write_with_permit"));
    assert!(manager.contains("pub async fn acquire_input_writer"));
    assert!(
        !manager.contains("pub fn write(&self, id: Uuid"),
        "a public permitless PtyManager::write facade must not return"
    );

    for relative in [
        "src/commands/pty.rs",
        "src/web/mod.rs",
        "src/web/commands.rs",
        "src/pty/inject.rs",
        "src/phone/mailbox.rs",
    ] {
        let body = source(relative);
        assert!(
            !body.contains(".write(session_id,"),
            "{relative} contains a permitless session write"
        );
        assert!(
            !body.contains(".write(uuid,"),
            "{relative} contains a permitless UUID write"
        );
    }
}
