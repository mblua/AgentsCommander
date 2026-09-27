//! #2482 - the pure half of the quota reading: compile one configured source and
//! turn a session's screen rows into a USED percentage of the 7-day window.
//!
//! Pure by construction: no locks, no vt100, no Tauri, no settings type. Everything
//! that can go wrong here is `None`, which the engine reports as unavailable and
//! NEVER as `0` or `100`.

use std::sync::Arc;

use crate::pty::context_scrape::pattern::{self, ContextPattern};
use crate::pty::context_scrape::rows;

/// The engine-facing, settings-free description of one ENABLED source. The `lib.rs`
/// adapter maps `QuotaSourceConfig` onto this; the engine never names the settings
/// type, which is what keeps `agent_quota` out of the crate's cyclic SCC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceSpec {
    ScreenRegex { pattern: String },
    ScreenRegexRemaining { pattern: String },
}

/// The compiled form of one configured source.
#[derive(Debug)]
pub enum ResolvedSource {
    ScreenRegex(Arc<ContextPattern>),
    ScreenRegexRemaining(Arc<ContextPattern>),
}

/// Compile a spec, or say why it cannot be one. For `ScreenRegex` this is #1032's
/// compiler, which already enforces the size limit and requires capture group 1.
pub fn resolve(spec: &SourceSpec) -> Result<ResolvedSource, String> {
    match spec {
        SourceSpec::ScreenRegex { pattern } => pattern::compile(pattern)
            .map(|compiled| ResolvedSource::ScreenRegex(Arc::new(compiled))),
        SourceSpec::ScreenRegexRemaining { pattern } => pattern::compile(pattern)
            .map(|compiled| ResolvedSource::ScreenRegexRemaining(Arc::new(compiled))),
    }
}

/// One reading. A future non-row source ignores `rows`: the documented cost of the
/// one-shape seam.
pub fn sample(source: &ResolvedSource, rows: &[String]) -> Option<u8> {
    match source {
        ResolvedSource::ScreenRegex(pattern) => rows::extract(pattern, rows),
        // Group 1 is the REMAINING percentage; the engine speaks USED. `100 - r` cannot
        // underflow: `rows::extract` returns `None` for anything above 100, so every
        // `Some(r)` it yields has `r <= 100`. No `saturating_sub` on purpose: it would hide
        // a regression in that guarantee, which the `101` test catches instead.
        ResolvedSource::ScreenRegexRemaining(pattern) => {
            rows::extract(pattern, rows).map(|r| 100 - r)
        }
    }
}

/// The string a recompile is decided by. The `"screenRegex:"` and
/// `"screenRegexRemaining:"` prefixes are contract: they are what stop a kind with equal
/// text from reusing another kind's cached compile.
pub fn spec_key(spec: &SourceSpec) -> String {
    match spec {
        SourceSpec::ScreenRegex { pattern } => format!("screenRegex:{pattern}"),
        SourceSpec::ScreenRegexRemaining { pattern } => {
            format!("screenRegexRemaining:{pattern}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The 37 ASCII bytes the settings file holds. RAW string: the backslash survives.
    const CODEX_PATTERN: &str = r"(?:^|[ \u00b7])Weekly (\d{1,3})% left";
    // The row Codex draws. Real U+00B7 characters, written with a Rust escape.
    const CODEX_ROW: &str = "Context 0% used \u{b7} Weekly 100% left \u{b7} GPT-6-Sol low";

    const PATTERN: &str = r"Weekly (\d{1,3})% used";

    fn spec(pattern: &str) -> SourceSpec {
        SourceSpec::ScreenRegex {
            pattern: pattern.to_string(),
        }
    }

    #[test]
    fn a_screen_regex_spec_resolves_and_samples_the_lowest_matching_row() {
        let source = resolve(&spec(PATTERN)).expect("valid pattern resolves");
        let rows = vec![
            "Weekly 10% used".to_string(),
            "prose".to_string(),
            "Weekly 37% used".to_string(),
            String::new(),
        ];
        assert_eq!(sample(&source, &rows), Some(37));
        assert_eq!(sample(&source, &["nothing here".to_string()]), None);
    }

    #[test]
    fn a_pattern_without_capture_group_one_is_rejected_by_resolve() {
        assert!(resolve(&spec(r"Weekly \d+% used")).is_err());
        assert!(resolve(&spec(r"(unclosed")).is_err());
    }

    #[test]
    fn a_value_over_one_hundred_is_rejected_rather_than_clamped() {
        let source = resolve(&spec(PATTERN)).unwrap();
        assert_eq!(sample(&source, &["Weekly 101% used".to_string()]), None);
        assert_eq!(
            sample(&source, &["Weekly 100% used".to_string()]),
            Some(100)
        );
        assert_eq!(sample(&source, &["Weekly 0% used".to_string()]), Some(0));
    }

    #[test]
    fn spec_key_carries_the_kind_discriminant_prefix() {
        let spec = SourceSpec::ScreenRegex {
            pattern: r"ctx (\d+)".into(),
        };
        assert!(spec_key(&spec).starts_with("screenRegex:"));
        assert_ne!(spec_key(&spec), r"ctx (\d+)");
    }

    fn remaining(pattern: &str) -> SourceSpec {
        SourceSpec::ScreenRegexRemaining {
            pattern: pattern.to_string(),
        }
    }

    fn one(row: &str) -> Vec<String> {
        vec![row.to_string()]
    }

    #[test]
    fn a_remaining_spec_samples_the_complement_of_the_lowest_matching_row() {
        let source = resolve(&remaining(r"Weekly (\d{1,3})% left")).unwrap();
        let rows = vec![
            "Weekly 10% left".to_string(),
            "prose".to_string(),
            "Weekly 30% left".to_string(),
        ];
        assert_eq!(sample(&source, &rows), Some(70));
    }

    #[test]
    fn a_remaining_reading_of_one_hundred_is_zero_used_and_zero_is_one_hundred() {
        let source = resolve(&remaining(r"Weekly (\d{1,3})% left")).unwrap();
        assert_eq!(sample(&source, &one("Weekly 100% left")), Some(0));
        assert_eq!(sample(&source, &one("Weekly 0% left")), Some(100));
        assert_eq!(sample(&source, &one("Weekly 101% left")), None);
    }

    #[test]
    fn the_persisted_codex_pattern_is_thirty_seven_ascii_bytes() {
        assert_eq!(CODEX_PATTERN.len(), 37);
        assert!(CODEX_PATTERN.contains(r"\u00b7"));
    }

    #[test]
    fn the_real_codex_statusline_row_reads_one_hundred_remaining() {
        let source = resolve(&remaining(CODEX_PATTERN)).unwrap();
        assert_eq!(sample(&source, &one(CODEX_ROW)), Some(0));
        assert_eq!(
            sample(&source, &one("Context 0% used \u{b7} Weekly 100%")),
            None
        );
    }

    #[test]
    fn a_remaining_pattern_without_capture_group_one_is_rejected_by_resolve() {
        assert!(resolve(&remaining(r"Weekly \d+% left")).is_err());
        assert!(resolve(&remaining(r"(unclosed")).is_err());
    }

    #[test]
    fn spec_key_separates_the_two_kinds_for_an_identical_pattern() {
        let text = r"Weekly (\d+)% left";
        let remaining_key = spec_key(&remaining(text));
        assert!(remaining_key.starts_with("screenRegexRemaining:"));
        assert_ne!(remaining_key, spec_key(&spec(text)));
    }
}
