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
}

/// The compiled form of one configured source.
#[derive(Debug)]
pub enum ResolvedSource {
    ScreenRegex(Arc<ContextPattern>),
}

/// Compile a spec, or say why it cannot be one. For `ScreenRegex` this is #1032's
/// compiler, which already enforces the size limit and requires capture group 1.
pub fn resolve(spec: &SourceSpec) -> Result<ResolvedSource, String> {
    match spec {
        SourceSpec::ScreenRegex { pattern } => pattern::compile(pattern)
            .map(|compiled| ResolvedSource::ScreenRegex(Arc::new(compiled))),
    }
}

/// One reading. A future non-row source ignores `rows`: the documented cost of the
/// one-shape seam.
pub fn sample(source: &ResolvedSource, rows: &[String]) -> Option<u8> {
    match source {
        ResolvedSource::ScreenRegex(pattern) => rows::extract(pattern, rows),
    }
}

/// The string a recompile is decided by. The `"screenRegex:"` prefix is contract: it
/// is what stops a future kind with equal text from reusing a cached compile.
pub fn spec_key(spec: &SourceSpec) -> String {
    match spec {
        SourceSpec::ScreenRegex { pattern } => format!("screenRegex:{pattern}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
