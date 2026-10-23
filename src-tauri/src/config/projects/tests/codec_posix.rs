use super::super::{decode_instance_relative, encode_instance_relative, RelDecodeError};
use std::path::{Path, PathBuf};

#[test]
fn encode_child_of_base() {
    assert_eq!(
        encode_instance_relative(
            Path::new("/opt/bundle/projects/alpha"),
            Path::new("/opt/bundle")
        ),
        Some("projects/alpha".to_string())
    );
}

#[test]
fn encode_sibling_uses_dotdot() {
    assert_eq!(
        encode_instance_relative(Path::new("/opt/projects/alpha"), Path::new("/opt/bundle")),
        Some("../projects/alpha".to_string())
    );
}

#[test]
fn encode_base_itself_is_dot() {
    assert_eq!(
        encode_instance_relative(Path::new("/opt/bundle"), Path::new("/opt/bundle")),
        Some(".".to_string())
    );
}

#[test]
fn encode_preserves_case_and_unicode_and_spaces() {
    assert_eq!(
        encode_instance_relative(
            Path::new("/opt/bundle/Pro Jects/Ä lpha"),
            Path::new("/opt/bundle")
        ),
        Some("Pro Jects/Ä lpha".to_string())
    );
}

#[test]
fn encode_component_with_backslash_has_no_portable_form() {
    // A POSIX dir literally named `a\b` cannot be represented.
    assert_eq!(
        encode_instance_relative(Path::new("/opt/bundle/a\\b"), Path::new("/opt/bundle")),
        None
    );
}

#[test]
fn decode_round_trips_encode() {
    let base = Path::new("/opt/bundle");
    let project = Path::new("/opt/projects/alpha");
    let wire = encode_instance_relative(project, base).unwrap();
    assert_eq!(
        decode_instance_relative(&wire, base).unwrap(),
        PathBuf::from("/opt/projects/alpha")
    );
}

#[test]
fn decode_dot_is_base() {
    assert_eq!(
        decode_instance_relative(".", Path::new("/opt/bundle")).unwrap(),
        PathBuf::from("/opt/bundle")
    );
}

#[test]
fn decode_rejects_noncanonical_and_injection() {
    let base = Path::new("/opt/bundle");
    assert_eq!(
        decode_instance_relative("", base),
        Err(RelDecodeError::Empty)
    );
    assert_eq!(
        decode_instance_relative("a\0b", base),
        Err(RelDecodeError::Nul)
    );
    assert_eq!(
        decode_instance_relative("a\\b", base),
        Err(RelDecodeError::Backslash)
    );
    assert_eq!(
        decode_instance_relative("/abs", base),
        Err(RelDecodeError::EmptySegment)
    );
    assert_eq!(
        decode_instance_relative("a/", base),
        Err(RelDecodeError::EmptySegment)
    );
    assert_eq!(
        decode_instance_relative("a//b", base),
        Err(RelDecodeError::EmptySegment)
    );
    assert_eq!(
        decode_instance_relative("a/./b", base),
        Err(RelDecodeError::DotSegment)
    );
    assert_eq!(
        decode_instance_relative("C:/x", base),
        Err(RelDecodeError::DriveRelative)
    );
    assert_eq!(
        decode_instance_relative("C:", base),
        Err(RelDecodeError::DriveRelative)
    );
}

#[test]
fn decode_rejects_traversal_above_root() {
    // /opt/bundle has two normal components; three `..` walk above root.
    assert_eq!(
        decode_instance_relative("../../../etc", Path::new("/opt/bundle")),
        Err(RelDecodeError::EscapesRoot)
    );
}

#[test]
fn decode_allows_sibling_traversal() {
    assert_eq!(
        decode_instance_relative("../projects/alpha", Path::new("/opt/bundle")).unwrap(),
        PathBuf::from("/opt/projects/alpha")
    );
}

#[test]
fn decode_rejects_relative_base() {
    assert_eq!(
        decode_instance_relative("a", Path::new("relative/base")),
        Err(RelDecodeError::BaseNotAbsolute)
    );
}

#[test]
fn posix_backslash_component_is_not_a_separator_but_is_rejected() {
    // Wire never contains `\`; a value with one is rejected outright.
    assert_eq!(
        decode_instance_relative("a\\b/c", Path::new("/opt/bundle")),
        Err(RelDecodeError::Backslash)
    );
}

#[test]
fn posix_paths_are_case_distinct() {
    // Two POSIX projects differing only by case must produce distinct
    // wires (encode preserves case) and distinct decoded targets.
    let base = Path::new("/opt");
    let upper = encode_instance_relative(Path::new("/opt/Repo"), base).unwrap();
    let lower = encode_instance_relative(Path::new("/opt/repo"), base).unwrap();
    assert_eq!(upper, "Repo");
    assert_eq!(lower, "repo");
    assert_ne!(upper, lower);
    assert_ne!(
        decode_instance_relative(&upper, base).unwrap(),
        decode_instance_relative(&lower, base).unwrap()
    );
}

#[test]
fn unicode_and_dot_round_trip() {
    let base = Path::new("/opt/bundle");
    // `.` names the base itself.
    assert_eq!(
        decode_instance_relative(".", base).unwrap(),
        PathBuf::from("/opt/bundle")
    );
    // Non-ASCII components survive an encode/decode round-trip.
    let project = Path::new("/opt/bundle/проект/Ω-α");
    let wire = encode_instance_relative(project, base).unwrap();
    assert_eq!(wire, "проект/Ω-α");
    assert_eq!(decode_instance_relative(&wire, base).unwrap(), project);
}
