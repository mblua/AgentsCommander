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
mod tests;
