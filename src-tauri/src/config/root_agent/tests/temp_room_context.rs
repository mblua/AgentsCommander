use super::*;
use crate::config::root_agent::context_snapshot_temp_rooms::ROOT_CONTEXT_BEFORE_TEMP_ROOMS_MD;

#[test]
fn root_snapshot_before_temp_rooms_is_base_template() {
    use sha2::{Digest, Sha256};
    let snapshot = ROOT_CONTEXT_BEFORE_TEMP_ROOMS_MD;
    assert_eq!(
        format!("{:x}", Sha256::digest(snapshot.as_bytes())),
        "a10b0dd953986cc09edf598580af5cce73f1b8b06d073d56cbb61359998932fd"
    );
    assert!(!snapshot.contains('\r'));
}

#[test]
fn root_snapshot_before_temp_rooms_is_known_and_differs_from_default() {
    let snapshot = ROOT_CONTEXT_BEFORE_TEMP_ROOMS_MD;
    assert!(is_known_generated_root_context_template(snapshot));
    assert_ne!(snapshot, default_root_context_template());
}

#[test]
fn root_default_allows_temp_room_pair() {
    let source = default_root_context_template();
    assert!(source.matches("root-to-temp-room-trNN-<slug>.md").count() >= 2);
    for needle in [
        "Narrow exception — Temporary Room message files",
        "{{APP_CONFIG_DIR}}/temp-room-trNN/messaging/",
        "agentscommander://temp-room/trNN",
        "verified Room orchestrator replicas and Temporary Rooms only",
        "narrow messaging exceptions above",
    ] {
        assert!(source.contains(needle), "missing: {needle}");
    }
    assert!(!source.contains("Root sessions list verified Room orchestrator replicas only."));
}
