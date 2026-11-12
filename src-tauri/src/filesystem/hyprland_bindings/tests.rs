use std::os::unix::fs::PermissionsExt;

use super::*;

const MAIN: &str = "-- Omarchy\nrequire(\"hypr.bindings\")\n";

fn layout(main: Option<&str>) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let hypr = dir.path().join("hypr");
    fs::create_dir(&hypr).unwrap();
    fs::write(hypr.join("bindings.lua"), "-- binds\n").unwrap();
    if let Some(main) = main {
        fs::write(hypr.join("hyprland.lua"), main).unwrap();
    }
    let bindings = hypr.join("bindings.lua");
    (dir, bindings)
}

#[test]
fn path_needs_the_omarchy_layout() {
    let (dir, bindings) = layout(Some(MAIN));
    assert_eq!(omarchy_bindings_path(dir.path()), Some(bindings));
    let (dir, _) = layout(None);
    assert_eq!(omarchy_bindings_path(dir.path()), None);
    let (dir, _) = layout(Some("require(\"hypr.other\")\n"));
    assert_eq!(omarchy_bindings_path(dir.path()), None);
    let (dir, _) = layout(Some("  -- require(\"hypr.bindings\")\n"));
    assert_eq!(omarchy_bindings_path(dir.path()), None);
}

#[test]
fn replace_succeeds_and_leaves_no_temp_file() {
    let (_dir, bindings) = layout(Some(MAIN));
    assert!(replace_bindings_if_unchanged(&bindings, "-- binds\n", "new\n").is_ok());
    assert_eq!(read_bindings(&bindings).unwrap(), "new\n");
    assert!(!bindings.with_file_name(TEMP_NAME).exists());
}

#[test]
fn changed_file_keeps_the_external_bytes() {
    let (_dir, bindings) = layout(Some(MAIN));
    fs::write(&bindings, "external\n").unwrap();
    let result = replace_bindings_if_unchanged(&bindings, "-- binds\n", "new\n");
    assert!(matches!(result, Err(BindingsWriteError::Changed)));
    assert_eq!(read_bindings(&bindings).unwrap(), "external\n");
}

#[test]
fn replace_keeps_permissions() {
    let (_dir, bindings) = layout(Some(MAIN));
    fs::set_permissions(&bindings, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(replace_bindings_if_unchanged(&bindings, "-- binds\n", "new\n").is_ok());
    let mode = fs::metadata(&bindings).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640);
}

#[test]
fn symlinked_bindings_are_not_the_omarchy_layout() {
    // User-granted exception (#2774 P04 only, P12): a symlinked bindings.lua.
    let (dir, bindings) = layout(Some(MAIN));
    let real = dir.path().join("real-bindings.lua");
    fs::rename(&bindings, &real).unwrap();
    std::os::unix::fs::symlink(&real, &bindings).unwrap();
    assert_eq!(omarchy_bindings_path(dir.path()), None);
}
