use super::super::{decode_instance_relative, encode_instance_relative, RelDecodeError};
use std::path::{Path, PathBuf};

#[test]
fn encode_child_and_sibling() {
    assert_eq!(
        encode_instance_relative(
            Path::new(r"C:\bundle\projects\alpha"),
            Path::new(r"C:\bundle")
        ),
        Some("projects/alpha".to_string())
    );
    assert_eq!(
        encode_instance_relative(Path::new(r"C:\projects\alpha"), Path::new(r"C:\bundle")),
        Some("../projects/alpha".to_string())
    );
}

#[test]
fn encode_drive_letter_case_insensitive_root() {
    assert_eq!(
        encode_instance_relative(Path::new(r"c:\bundle\alpha"), Path::new(r"C:\bundle")),
        Some("alpha".to_string())
    );
}

#[test]
fn encode_different_drive_has_no_relative_form() {
    assert_eq!(
        encode_instance_relative(Path::new(r"D:\projects\alpha"), Path::new(r"C:\bundle")),
        None
    );
}

#[test]
fn encode_same_unc_share_ok_different_share_none() {
    assert_eq!(
        encode_instance_relative(
            Path::new(r"\\server\share\bundle\alpha"),
            Path::new(r"\\server\share\bundle")
        ),
        Some("alpha".to_string())
    );
    assert_eq!(
        encode_instance_relative(
            Path::new(r"\\server\other\alpha"),
            Path::new(r"\\server\share\bundle")
        ),
        None
    );
}

#[test]
fn encode_verbatim_disk_normalizes_for_comparison() {
    assert_eq!(
        encode_instance_relative(Path::new(r"\\?\C:\bundle\alpha"), Path::new(r"C:\bundle")),
        Some("alpha".to_string())
    );
}

#[test]
fn decode_round_trips_and_rejects_windows_hazards() {
    let base = Path::new(r"C:\bundle");
    assert_eq!(
        decode_instance_relative("projects/alpha", base).unwrap(),
        PathBuf::from(r"C:\bundle\projects\alpha")
    );
    // ADS colon not in drive position → illegal char (drive-position
    // `X:` is caught earlier as DriveRelative, also a rejection).
    assert_eq!(
        decode_instance_relative("foo:bar", base),
        Err(RelDecodeError::IllegalWindowsChar)
    );
    assert_eq!(
        decode_instance_relative("a*b", base),
        Err(RelDecodeError::IllegalWindowsChar)
    );
    assert_eq!(
        decode_instance_relative("a:b", base),
        Err(RelDecodeError::DriveRelative)
    );
    // Trailing dot/space.
    assert_eq!(
        decode_instance_relative("alpha ", base),
        Err(RelDecodeError::TrailingDotOrSpace)
    );
    assert_eq!(
        decode_instance_relative("alpha.", base),
        Err(RelDecodeError::TrailingDotOrSpace)
    );
    // Reserved DOS device names, with and without extension.
    assert_eq!(
        decode_instance_relative("CON", base),
        Err(RelDecodeError::ReservedDosName)
    );
    assert_eq!(
        decode_instance_relative("con.txt", base),
        Err(RelDecodeError::ReservedDosName)
    );
    assert_eq!(
        decode_instance_relative("COM1", base),
        Err(RelDecodeError::ReservedDosName)
    );
    assert_eq!(
        decode_instance_relative("LPT9", base),
        Err(RelDecodeError::ReservedDosName)
    );
    // COM0 / COM10 are not reserved.
    assert!(decode_instance_relative("COM0", base).is_ok());
    assert!(decode_instance_relative("COM10", base).is_ok());
}

#[test]
fn decode_rejects_traversal_above_drive_root() {
    assert_eq!(
        decode_instance_relative("../../x", Path::new(r"C:\bundle")),
        Err(RelDecodeError::EscapesRoot)
    );
}

#[test]
fn decode_rejects_all_illegal_windows_chars_and_controls() {
    let base = Path::new(r"C:\bundle");
    for illegal in ["a<b", "a>b", "a\"b", "a|b", "a?b", "a*b"] {
        assert_eq!(
            decode_instance_relative(illegal, base),
            Err(RelDecodeError::IllegalWindowsChar),
            "must reject {illegal:?}"
        );
    }
    // Control characters (below 0x20) are rejected.
    assert_eq!(
        decode_instance_relative("a\u{0001}b", base),
        Err(RelDecodeError::IllegalWindowsChar)
    );
    assert_eq!(
        decode_instance_relative("a\u{001f}b", base),
        Err(RelDecodeError::IllegalWindowsChar)
    );
}

#[test]
fn encode_device_namespace_has_no_portable_form() {
    // A `\\.\` device-namespace path is unsupported and must NOT be
    // stripped into an ordinary path; it yields no relative companion.
    assert_eq!(
        encode_instance_relative(Path::new(r"\\.\COM1"), Path::new(r"C:\bundle")),
        None
    );
    // A verbatim non-disk device path is likewise unsupported.
    assert_eq!(
        encode_instance_relative(
            Path::new(r"\\?\GLOBALROOT\Device\HarddiskVolume1\x"),
            Path::new(r"C:\bundle")
        ),
        None
    );
}
