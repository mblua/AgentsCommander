use super::{scrub, Literals};

#[test]
fn comments_become_one_space() {
    let code = "a // x\nb /* c /* d */ e */ f";
    assert_eq!(scrub(code, Literals::Keep).unwrap(), "a  \nb   f");
}

#[test]
fn literals_are_kept_or_dropped() {
    let code = r##"x("s//t", r#"u"v"#, br"w", 'c', '\n', 'a)"##;
    assert_eq!(
        scrub(code, Literals::Keep).unwrap(),
        r##"x("s//t", r#"u"v"#, br"w", 'c', '\n', 'a)"##
    );
    assert_eq!(scrub(code, Literals::Drop).unwrap(), "x( ,  ,  ,  ,  , 'a)");
}

#[test]
fn an_identifier_ending_in_r_is_not_a_raw_prefix() {
    assert_eq!(scrub(r#"bar"x""#, Literals::Drop).unwrap(), "bar ");
}

#[test]
fn unclosed_input_is_an_error() {
    let cases = [
        (
            "/* x",
            "a block comment is never closed, so the scan cannot be trusted",
        ),
        (
            "r#\"x\"",
            "a raw string is never closed, so the scan cannot be trusted",
        ),
        (
            "\"x",
            "a string literal is never closed, so the scan cannot be trusted",
        ),
        (
            "'\\x",
            "a character literal is never closed, so the scan cannot be trusted",
        ),
    ];
    for (code, reason) in cases {
        assert_eq!(scrub(code, Literals::Drop), Err(reason));
    }
}
