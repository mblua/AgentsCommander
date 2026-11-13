//! #2774 The only disk access for the Hyprland `bindings.lua` managed block (std only).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const REQUIRE_LINE: &str = "require(\"hypr.bindings\")";
const TEMP_NAME: &str = ".bindings.lua.agentscommander-tmp";

pub(crate) enum BindingsWriteError {
    Changed,
    Io(String),
}

fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

/// `Some(<home>/hypr/bindings.lua)` only for the Omarchy layout: a regular `bindings.lua`
/// loaded by an uncommented `require("hypr.bindings")` in a regular `hyprland.lua`.
pub(crate) fn omarchy_bindings_path(config_home: &Path) -> Option<PathBuf> {
    let hypr = config_home.join("hypr");
    let bindings = hypr.join("bindings.lua");
    let main = hypr.join("hyprland.lua");
    if !is_regular_file(&bindings) || !is_regular_file(&main) {
        return None;
    }
    let text = fs::read_to_string(&main).ok()?;
    let loads = text
        .lines()
        .any(|line| !line.trim_start().starts_with("--") && line.contains(REQUIRE_LINE));
    loads.then_some(bindings)
}

/// Read `bindings.lua`: a regular file (no symlink), UTF-8.
pub(crate) fn read_bindings(path: &Path) -> Result<String, String> {
    if !is_regular_file(path) {
        return Err(format!("{} is not a regular file", path.display()));
    }
    fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))
}

/// Replace `path` with `new` if it still holds `expected`, through a sibling temp file and
/// a rename; the original is never truncated. Best effort: a write between the re-read
/// and the rename is not detected.
pub(crate) fn replace_bindings_if_unchanged(
    path: &Path,
    expected: &str,
    new: &str,
) -> Result<(), BindingsWriteError> {
    let io = |e: std::io::Error| BindingsWriteError::Io(e.to_string());
    if fs::read(path).map_err(io)? != expected.as_bytes() {
        return Err(BindingsWriteError::Changed);
    }
    let permissions = fs::metadata(path).map_err(io)?.permissions();
    let temp = path.with_file_name(TEMP_NAME);
    let _ = fs::remove_file(&temp);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.set_permissions(permissions)?;
        file.write_all(new.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&temp);
        return Err(io(e));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
