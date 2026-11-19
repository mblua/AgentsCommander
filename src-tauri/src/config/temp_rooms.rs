//! Temp-room identity (#4779): `temp-room-trNN` folders directly in the app config dir.
//! Identity is pure; `context` writes the context cache.

pub(crate) mod context;

use std::path::Path;

pub(crate) const TEMP_ROOM_PREFIX: &str = "temp-room-tr";

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

#[cfg(test)]
mod tests;
