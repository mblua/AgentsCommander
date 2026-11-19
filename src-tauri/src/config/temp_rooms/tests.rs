use super::*;
use std::path::PathBuf;

#[test]
fn temp_room_dir_names_are_two_digit_padded() {
    assert_eq!(temp_room_dir_name(1), "temp-room-tr01");
    assert_eq!(temp_room_dir_name(9), "temp-room-tr09");
    assert_eq!(temp_room_dir_name(10), "temp-room-tr10");
    assert_eq!(temp_room_dir_name(99), "temp-room-tr99");
    assert_eq!(temp_room_dir_name(100), "temp-room-tr100");
}

#[test]
fn temp_room_parse_accepts_canonical_names() {
    for n in [1, 9, 10, 99, 100] {
        assert_eq!(parse_temp_room_number(&temp_room_dir_name(n)), Some(n));
    }
}

#[test]
fn temp_room_parse_rejects_non_canonical_names() {
    for name in [
        "temp-room-tr1",
        "temp-room-tr001",
        "temp-room-tr00",
        "temp-room-tr",
        "temp-room-tr01x",
        "temp-room-tr01-a",
        "temp-room-tr+1",
        "TEMP-ROOM-TR01",
        "Temp-room-tr01",
        "room-1-x",
        "wg-1-x",
        "",
    ] {
        assert_eq!(parse_temp_room_number(name), None, "{name}");
    }
}

#[test]
fn temp_room_path_in_requires_base_parent() {
    let base = std::env::temp_dir().join("ac-temp-rooms-base");
    let other = std::env::temp_dir().join("ac-temp-rooms-other");
    let s = |p: PathBuf| p.to_string_lossy().into_owned();
    assert!(is_temp_room_path_in(&s(base.join("temp-room-tr01")), &base));
    assert!(!is_temp_room_path_in(
        &s(base.join("x").join("temp-room-tr01")),
        &base
    ));
    assert!(!is_temp_room_path_in(&s(base.join("ac-root-agent")), &base));
    assert!(!is_temp_room_path_in(
        &s(other.join("temp-room-tr01")),
        &base
    ));
    assert!(!is_temp_room_path_in(&s(base.join("temp-room-tr1")), &base));
}

#[test]
fn temp_room_path_uses_config_dir() {
    let base = crate::config::config_dir().expect("config dir");
    let cwd = base.join("temp-room-tr01");
    assert!(is_temp_room_path(&cwd.to_string_lossy()));
    let project = base.join("project").join(".ac").join("temp-room-tr01");
    assert!(!is_temp_room_path(&project.to_string_lossy()));
}

#[test]
fn temp_room_artifact_rows_use_the_writer_prefix() {
    use crate::config::instance_artifacts::{Disposition, INSTANCE_ARTIFACTS};
    for name in [
        format!("{TEMP_ROOM_PREFIX}*"),
        format!(".deleting-{TEMP_ROOM_PREFIX}*"),
    ] {
        let rows = INSTANCE_ARTIFACTS
            .iter()
            .filter(|row| row.name == name && row.disposition == Disposition::Ignore)
            .count();
        assert_eq!(rows, 1, "missing Ignore row {name}");
    }
}

#[test]
fn address_for_temp_room_path() {
    let base = crate::config::config_dir().expect("config dir");
    let cwd = base.join("temp-room-tr01");
    assert_eq!(
        temp_room_address_for_path(&cwd.to_string_lossy()).as_deref(),
        Some("agentscommander://temp-room/tr01")
    );
}

#[test]
fn address_none_for_other_paths() {
    let base = crate::config::config_dir().expect("config dir");
    for cwd in [
        base.join("temp-room-tr1"),
        base.join("ac-root-agent"),
        base.join("project").join(".ac").join("temp-room-tr01"),
    ] {
        assert_eq!(temp_room_address_for_path(&cwd.to_string_lossy()), None);
    }
}

#[test]
fn name_for_address_canonical_only() {
    assert_eq!(
        temp_room_name_for_address("agentscommander://temp-room/tr01").as_deref(),
        Some("temp-room-tr01")
    );
    assert_eq!(
        temp_room_name_for_address("agentscommander://temp-room/tr100").as_deref(),
        Some("temp-room-tr100")
    );
    for addr in ["tr1", "tr00", "tr01/x", ""] {
        let addr = format!("{TEMP_ROOM_ADDRESS_PREFIX}{addr}");
        assert_eq!(temp_room_name_for_address(&addr), None, "{addr}");
    }
}

#[test]
fn is_temp_room_address_rejects_lookalikes() {
    assert!(is_temp_room_address("agentscommander://temp-room/tr01"));
    for addr in [
        "agentscommander://temp-room/tr1",
        "agentscommander://temp-room/",
        "agentscommander://temp-room/temp-room-tr01",
        "agentscommander://root-agent",
        "AGENTSCOMMANDER://temp-room/tr01",
        "agentscommander://temp-room/tr01 ",
        "temp-room-tr01",
        "proj:temp-room-tr01/agent",
    ] {
        assert!(!is_temp_room_address(addr), "{addr}");
    }
}
