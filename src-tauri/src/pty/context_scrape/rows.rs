//! #1032 - the pure half of the context scrape: turn the plain rows of a session's
//! screen mirror into a percentage, using that agent's user-configured regex.
//!
//! Pure by construction: no locks, no vt100, no Tauri. Everything that can go wrong here
//! is `None`, which the scraper reports as unavailable and NEVER as `0`.

use super::pattern::ContextPattern;

/// The reading for one sample.
///
/// Scans the screen's PHYSICAL rows bottom-up and takes the first row that matches. Prose
/// and transcript always render above the statusline, and only `⏸ manual mode on` and
/// blanks render below it, so the lowest match is the statusline whenever the statusline
/// is on the grid. Measured on real bytes, not inferred.
///
/// Capture group 1 is the percentage; `compile` has already refused a pattern that has
/// none. A match whose group 1 is missing, unparseable, or outside `0..=100` is rejected -
/// the row that matched still wins, and the reading is simply unavailable.
pub fn extract(pattern: &ContextPattern, rows: &[String]) -> Option<u8> {
    let captures = rows
        .iter()
        .rev()
        .find_map(|row| pattern.regex().captures(row))?;
    let percent: u8 = captures.get(1)?.as_str().parse().ok()?;
    (percent <= 100).then_some(percent)
}

#[cfg(test)]
mod tests;
