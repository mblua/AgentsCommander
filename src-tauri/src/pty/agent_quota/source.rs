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
mod tests;
