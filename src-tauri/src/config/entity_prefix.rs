//! On-disk prefix of a Room directory, and of the legacy Workgroup directory
//! it replaces (#1614). Phase 2 (#1615) retires the legacy prefix; until then
//! every discovery, identity and authorization gate accepts both.

/// Prefix of every Room directory AgentsCommander creates.
pub const ROOM_DIR_PREFIX: &str = "room-";

/// Prefix of a legacy Workgroup directory. Never produced again (#1614); still
/// discovered, addressed and operated exactly like a Room.
pub const LEGACY_WORKGROUP_DIR_PREFIX: &str = "wg-";

/// The matched prefix, or `None` when `name` is neither.
pub fn entity_prefix_of(name: &str) -> Option<&'static str> {
    if name.starts_with(ROOM_DIR_PREFIX) {
        Some(ROOM_DIR_PREFIX)
    } else if name.starts_with(LEGACY_WORKGROUP_DIR_PREFIX) {
        Some(LEGACY_WORKGROUP_DIR_PREFIX)
    } else {
        None
    }
}

/// `name` with its Room or legacy Workgroup prefix removed.
pub fn strip_entity_prefix(name: &str) -> Option<&str> {
    name.strip_prefix(ROOM_DIR_PREFIX)
        .or_else(|| name.strip_prefix(LEGACY_WORKGROUP_DIR_PREFIX))
}

/// True when `name` carries a Room or legacy Workgroup prefix.
pub fn has_entity_prefix(name: &str) -> bool {
    entity_prefix_of(name).is_some()
}

#[cfg(test)]
mod tests {
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
}
