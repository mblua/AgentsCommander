//! Temp-room identity (#4779): `temp-room-trNN` folders directly in the app config dir.
//! Identity is pure; `context` writes the context cache.

pub(crate) mod context;

use std::path::Path;

pub(crate) const TEMP_ROOM_PREFIX: &str = "temp-room-tr";

/// Messaging address scheme of a temp room (`agentscommander://temp-room/tr01`).
pub(crate) const TEMP_ROOM_ADDRESS_PREFIX: &str = "agentscommander://temp-room/";

pub(crate) fn temp_room_dir_name(n: u32) -> String {
    format!("{TEMP_ROOM_PREFIX}{n:02}")
}

/// The number of a canonical temp-room folder name (`temp-room-tr01` -> 1), else `None`.
pub(crate) fn parse_temp_room_number(name: &str) -> Option<u32> {
    let digits = name.strip_prefix(TEMP_ROOM_PREFIX)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    (n >= 1 && temp_room_dir_name(n) == name).then_some(n)
}

/// Canonical temp-room folder names under `base` (real dirs only), sorted by number.
pub(crate) fn temp_room_names_in(base: &Path) -> Result<Vec<String>, String> {
    let mut rooms: Vec<(u32, String)> = crate::filesystem::temp_rooms::list_child_entries(base)?
        .into_iter()
        .filter(|entry| entry.real_dir)
        .filter_map(|entry| parse_temp_room_number(&entry.name).map(|n| (n, entry.name)))
        .collect();
    rooms.sort_by_key(|(n, _)| *n);
    Ok(rooms.into_iter().map(|(_, name)| name).collect())
}

/// True iff `cwd` is `<base>/temp-room-trNN`.
pub(crate) fn is_temp_room_path_in(cwd: &str, base: &Path) -> bool {
    let path = Path::new(cwd);
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if parse_temp_room_number(name).is_none() {
        return false;
    }
    path.parent()
        .is_some_and(|parent| crate::path_identity::paths_equivalent(parent, base))
}

/// True iff `cwd` is a temp room of this app's config dir; false when it is unresolved.
pub(crate) fn is_temp_room_path(cwd: &str) -> bool {
    crate::config::config_dir().is_some_and(|base| is_temp_room_path_in(cwd, &base))
}

/// The messaging address of the temp room at `cwd`, else `None`.
pub(crate) fn temp_room_address_for_path(cwd: &str) -> Option<String> {
    if !is_temp_room_path(cwd) {
        return None;
    }
    let name = Path::new(cwd).file_name()?.to_str()?;
    let suffix = name.strip_prefix("temp-room-")?;
    Some(format!("{TEMP_ROOM_ADDRESS_PREFIX}{suffix}"))
}

/// The canonical folder name (`temp-room-trNN`) a temp-room address names, else `None`.
pub(crate) fn temp_room_name_for_address(addr: &str) -> Option<String> {
    let suffix = addr.strip_prefix(TEMP_ROOM_ADDRESS_PREFIX)?;
    let name = format!("temp-room-{suffix}");
    parse_temp_room_number(&name).map(|_| name)
}

/// True iff `addr` is a canonical temp-room address.
pub(crate) fn is_temp_room_address(addr: &str) -> bool {
    temp_room_name_for_address(addr).is_some()
}

#[cfg(test)]
mod tests;
