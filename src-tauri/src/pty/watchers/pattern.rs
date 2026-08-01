//! #1171 - compiling a user-configured watcher pattern.
//!
//! A separate compiler from `context_scrape/pattern.rs` and NOT a reuse of it, for exactly
//! one reason: that one rejects any pattern without capture group 1 (`pattern.rs:44-48`,
//! pinned by its own test at `:68-75`), because group 1 IS the context percentage there. A
//! watcher pattern legitimately has zero groups - "did this appear at all" is a complete
//! question - so the same rule would reject the simplest useful watcher.
//!
//! Everything else is identical, including the reasoning.

use regex::{Regex, RegexBuilder};

/// A compiled watcher pattern, kept with the source string it came from so the engine can
/// tell a changed pattern from an unchanged one without recompiling to find out.
#[derive(Debug)]
pub struct WatcherPattern {
    regex: Regex,
    source: String,
}

impl WatcherPattern {
    pub fn regex(&self) -> &Regex {
        &self.regex
    }

    pub fn source(&self) -> &str {
        &self.source
    }
}

/// `regex` 1.x has no backtracking and is linear-time by construction, so a hostile pattern
/// cannot burn CPU here and no matching timeout is needed - there is nothing to time out. It
/// can still ask for an enormous compiled program, so the size limit is the bound that
/// matters: past it, compilation FAILS rather than allocating.
const SIZE_LIMIT_BYTES: usize = 1024 * 1024;

/// Compile a user-supplied pattern, or say why it cannot be one.
///
/// The source is handed over VERBATIM by every caller, never trimmed: leading spaces are
/// anchors, and the pattern is the only defence this feature has, so editing it can only
/// weaken it (`lib.rs:589-601`).
pub fn compile(source: &str) -> Result<WatcherPattern, String> {
    let regex = RegexBuilder::new(source)
        .size_limit(SIZE_LIMIT_BYTES)
        .build()
        .map_err(|e| e.to_string())?;

    Ok(WatcherPattern {
        regex,
        source: source.to_string(),
    })
}

#[cfg(test)]
mod tests {
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
}
