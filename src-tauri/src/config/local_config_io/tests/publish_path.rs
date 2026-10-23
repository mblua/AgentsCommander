use super::*;

#[cfg(windows)]
fn wide_to_string(wide: &[u16]) -> String {
    assert_eq!(wide.last(), Some(&0), "output must be NUL-terminated");
    String::from_utf16(&wide[..wide.len() - 1]).expect("valid UTF-16")
}

#[cfg(windows)]
#[test]
fn publish_path_wide_prefixes_a_plain_drive_path() {
    let out = super::publish_path_wide(Path::new(r"C:\a\config.json"));
    assert_eq!(wide_to_string(&out), r"\\?\C:\a\config.json");
}

#[cfg(windows)]
#[test]
fn publish_path_wide_normalizes_forward_slashes_and_unc() {
    let cases = [
        (r"C:/a/./config.json", r"\\?\C:\a\config.json"),
        (
            r"\\server\share\config.json",
            r"\\?\UNC\server\share\config.json",
        ),
        // Round-2 finding: the raw prefix text `//server/share` must not
        // be copied, or the server would be named `server/share`.
        (
            r"//server/share/config.json",
            r"\\?\UNC\server\share\config.json",
        ),
    ];
    for (input, expected) in cases {
        let out = super::publish_path_wide(Path::new(input));
        assert_eq!(wide_to_string(&out), expected, "input {input}");
    }
}

#[cfg(windows)]
#[test]
fn publish_path_wide_leaves_unconvertible_shapes_byte_identical() {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let mut nul_units: Vec<u16> = r"C:\a\conf".encode_utf16().collect();
    nul_units.push(0);
    nul_units.extend("ig.json".encode_utf16());
    let interior_nul = PathBuf::from(OsString::from_wide(&nul_units));

    let cases: Vec<(&str, PathBuf)> = vec![
        ("relative", PathBuf::from(r"sub\config.json")),
        ("drive-relative", PathBuf::from(r"C:config.json")),
        ("bare drive", PathBuf::from(r"C:")),
        ("parent dir", PathBuf::from(r"C:\a\..\config.json")),
        (
            "trailing-dot file name",
            PathBuf::from(r"C:\a\config.json."),
        ),
        (
            "trailing-space directory",
            PathBuf::from(r"C:\a \config.json"),
        ),
        ("already verbatim", PathBuf::from(r"\\?\C:\a\config.json")),
        ("device namespace", PathBuf::from(r"\\.\PIPE\x")),
        ("interior NUL", interior_nul),
    ];
    for (name, p) in cases {
        let expected: Vec<u16> = p
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>();
        assert_eq!(super::publish_path_wide(&p), expected, "case {name}");
    }
}

/// Builds a non-verbatim directory over 300 wide units holding an existing
/// `config.json` (old bytes) and a temp file (new bytes), and asserts the
/// fixture rules of plan #2378 section 5.1 before the caller acts.
#[cfg(windows)]
fn long_path_publish_fixture(root: &Path) -> (PathBuf, PathBuf) {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Component, Prefix};

    let segment = "a".repeat(40);
    let mut dir = root.to_path_buf();
    while dir.as_os_str().encode_wide().count() <= 300 {
        dir.push(&segment);
    }
    std::fs::create_dir_all(&dir).expect("create long dir");
    let dest = dir.join("config.json");
    use std::io::Write;
    std::fs::write(&dest, b"old content").expect("write dest");
    let guard = super::PairTargetGuard::new(&|| Ok(()));
    let (tmp, mut file) = super::acquire_fresh_config_stage(&dest, &guard).unwrap();
    file.write_all(b"new content").unwrap();
    file.flush().unwrap();
    file.sync_all().unwrap();
    drop(file);

    for p in [&dest, &tmp] {
        match p.components().next() {
            Some(Component::Prefix(prefix)) => assert!(
                matches!(prefix.kind(), Prefix::Disk(_)),
                "fixture must be an ordinary Disk path, got {:?}",
                prefix.kind()
            ),
            other => panic!("fixture must start with a prefix, got {other:?}"),
        }
        let wide_len = p.as_os_str().encode_wide().count();
        assert!(
            wide_len > 260,
            "fixture length {wide_len} must exceed MAX_PATH"
        );
        assert!(p.exists(), "fixture file must exist: {}", p.display());
    }
    (dest, tmp)
}

#[cfg(windows)]
#[test]
fn publish_over_a_destination_longer_than_max_path_succeeds() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (dest, tmp) = long_path_publish_fixture(temp.path());
    assert!(dest.exists());
    assert!(tmp.exists());

    super::publish_temp_config(&tmp, &dest).expect("long-path publish");

    assert_eq!(std::fs::read(&dest).expect("read dest"), b"new content");
    assert!(!tmp.exists(), "temp file must be consumed");
}

/// Inverse control for the long-path publish test: raw non-verbatim
/// arguments still fail with ERROR_PATH_NOT_FOUND, so that test is not
/// vacuous. If this ever goes red, the process gained long-path awareness
/// (for example through a manifest); re-triage #2378 instead of relaxing
/// this test.
#[cfg(windows)]
#[test]
fn raw_non_verbatim_replacefilew_still_fails_over_max_path() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};

    let temp = tempfile::tempdir().expect("tempdir");
    let (dest, tmp) = long_path_publish_fixture(temp.path());
    assert!(dest.exists());
    assert!(tmp.exists());

    let dest_wide: Vec<u16> = dest
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let tmp_wide: Vec<u16> = tmp
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let ok = unsafe {
        ReplaceFileW(
            dest_wide.as_ptr(),
            tmp_wide.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    let err = std::io::Error::last_os_error();
    assert_eq!(ok, 0, "raw ReplaceFileW must fail over MAX_PATH");
    assert_eq!(err.raw_os_error(), Some(3), "{err}");
}
