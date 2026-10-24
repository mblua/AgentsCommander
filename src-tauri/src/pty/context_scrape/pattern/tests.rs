use super::*;

#[test]
fn a_pattern_with_a_capture_group_compiles_and_keeps_its_source() {
    let source = r"^ {2}Context [░█]+ (\d{1,3})%";
    let pattern = compile(source).expect("compiles");
    assert_eq!(pattern.source(), source);
    assert!(pattern.regex().is_match("  Context ░░░░░░░░░░ 0% │ Usage"));
}

#[test]
fn a_pattern_without_a_capture_group_is_rejected() {
    let err = compile(r"^ {2}Context [░█]+ \d{1,3}%").expect_err("must be rejected");
    assert!(
        err.contains("capture group 1"),
        "the error must say what is missing, since app.log is all the user gets: {err}"
    );
}

#[test]
fn an_uncompilable_pattern_is_rejected_rather_than_panicking() {
    assert!(compile(r"^ {2}Context (\d{1,3}%").is_err());
}

#[test]
fn a_pattern_over_the_size_limit_fails_to_compile_rather_than_allocating() {
    // Nested bounded repeats are the cheap way to ask for an enormous program from a
    // short pattern. Without the limit this is an allocation, not an error.
    let hostile = format!("({})", r"(a{1000}){1000}");
    let err = compile(&hostile).expect_err("must be rejected");
    assert!(
        err.to_lowercase().contains("size"),
        "expected the size limit to be what refuses it, got: {err}"
    );
}
