use super::*;

#[test]
fn entity_prefix_accepts_room_and_legacy_and_rejects_others() {
    assert!(has_entity_prefix("room-1-t"));
    assert!(has_entity_prefix("wg-1-t"));
    for reject in [
        "roomx",
        "wgx",
        "_team_t",
        "__agent_x",
        "",
        "Room-1-t",
        "WG-1-t",
    ] {
        assert!(!has_entity_prefix(reject), "must reject {reject:?}");
    }

    assert_eq!(strip_entity_prefix("room-1-t"), Some("1-t"));
    assert_eq!(strip_entity_prefix("wg-1-t"), Some("1-t"));
    assert_eq!(strip_entity_prefix("roomx"), None);
    assert_eq!(strip_entity_prefix(""), None);

    assert_eq!(entity_prefix_of("room-1-t"), Some(ROOM_DIR_PREFIX));
    assert_eq!(
        entity_prefix_of("wg-1-t"),
        Some(LEGACY_WORKGROUP_DIR_PREFIX)
    );
    assert_eq!(entity_prefix_of("neither"), None);

    // The two prefixes are disjoint, so evaluation order cannot matter; it
    // is fixed anyway so the function is deterministic under review.
    assert!(!ROOM_DIR_PREFIX.starts_with(LEGACY_WORKGROUP_DIR_PREFIX));
    assert!(!LEGACY_WORKGROUP_DIR_PREFIX.starts_with(ROOM_DIR_PREFIX));
}
