//! #1032 - compiling the user's per-agent context regex.
//!
//! The engine ships no anchoring rules of its own: `%`-required, column-2-anchored and
//! glyph-required all live in the pattern, because the pattern is per-agent user
//! configuration. What lives here is the little that must be true of ANY pattern.

use regex::{Regex, RegexBuilder};

/// A compiled context regex, kept with the source string it came from so the scraper can
/// tell a changed pattern from an unchanged one without recompiling to find out.
#[derive(Debug)]
pub struct ContextPattern {
    regex: Regex,
    source: String,
}

impl ContextPattern {
    pub fn regex(&self) -> &Regex {
        &self.regex
    }

    pub fn source(&self) -> &str {
        &self.source
    }
}

/// `regex` 1.x has no backtracking and is linear-time by construction, so a hostile
/// pattern cannot burn CPU here. It can still ask for an enormous compiled program, so the
/// size limit is the bound that matters: past it, compilation FAILS rather than allocating.
const SIZE_LIMIT_BYTES: usize = 1024 * 1024;

/// Compile a user-supplied pattern, or say why it cannot be one.
///
/// The error is for `app.log` and for #1033 if it ever wants to surface it in Settings; the
/// engine itself treats every failure the same way - the reading is unavailable.
pub fn compile(source: &str) -> Result<ContextPattern, String> {
    let regex = RegexBuilder::new(source)
        .size_limit(SIZE_LIMIT_BYTES)
        .build()
        .map_err(|e| e.to_string())?;

    // Capture group 1 IS the reading. A pattern without one can never produce a number, so
    // it is rejected here rather than matching happily forever and reporting nothing.
    if regex.captures_len() < 2 {
        return Err(format!(
            "context regex has no capture group 1 (the percentage): {source}"
        ));
    }

    Ok(ContextPattern {
        regex,
        source: source.to_string(),
    })
}

#[cfg(test)]
mod tests {
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
}
