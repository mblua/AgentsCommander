use super::*;

/// 9.2.21 - **the difference from `context_scrape::pattern::compile`.** "Did this string
/// appear" is a complete watcher, and it has no capture group.
#[test]
fn a_pattern_with_zero_capture_groups_compiles() {
    let source = r"Permission required";
    let pattern = compile(source).expect("zero groups must compile");
    assert_eq!(pattern.source(), source);
    assert!(pattern
        .regex()
        .is_match("  Permission required to run bash"));
    assert_eq!(
        pattern.regex().captures_len(),
        1,
        "group 0 only, which is precisely what the context compiler rejects"
    );
}

#[test]
fn a_pattern_with_capture_groups_still_compiles_and_keeps_its_source() {
    let source = r"Read \((.+)\)";
    let pattern = compile(source).expect("compiles");
    let captures = pattern
        .regex()
        .captures("  Read (C:/repo/src/main.rs)")
        .expect("matches");
    assert_eq!(&captures[1], "C:/repo/src/main.rs");
}

#[test]
fn an_uncompilable_pattern_is_rejected_rather_than_panicking() {
    assert!(compile(r"Read \((.+").is_err());
}

/// 9.2.22 - the bound that actually matters. Nested bounded repeats are the cheap way to
/// ask for an enormous program from a short pattern; without the limit this is an
/// allocation, not an error.
#[test]
fn a_pattern_over_the_size_limit_fails_to_compile_rather_than_allocating() {
    let hostile = format!("({})", r"(a{1000}){1000}");
    let err = compile(&hostile).expect_err("must be rejected");
    assert!(
        err.to_lowercase().contains("size"),
        "expected the size limit to be what refuses it, got: {err}"
    );
}

/// 9.2.23 - the source is compiled verbatim. Leading spaces are an anchor, so trimming
/// would make a pattern match rows it was written to exclude.
#[test]
fn whitespace_is_part_of_the_pattern_and_is_never_trimmed() {
    let pattern = compile("  Context ").expect("compiles");
    assert_eq!(pattern.source(), "  Context ");
    assert!(pattern.regex().is_match("  Context left"));
    assert!(
        !pattern.regex().is_match("Context left"),
        "the leading spaces ARE the column anchor; trimming would fail this open"
    );
}
