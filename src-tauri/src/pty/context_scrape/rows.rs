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
mod tests {
    use super::*;
    use crate::pty::context_scrape::pattern::compile;

    /// The two patterns the live capture measured, and what #1033's placeholder ships.
    /// Every element of each is load-bearing, and the tests below are what say so.
    const CLAUDE: &str = r"^ {2}Context [░█]+ (\d{1,3})%";
    const CODEX: &str = r"^ {2}.*· Context (\d{1,3})% used";

    /// The real statusline at cols=120, as far as the plan transcribes it. The plan elides
    /// the tail with `…` in prose and the capture itself did not survive the workgroup
    /// purge, so the tail past `16%` is left off rather than invented. Nothing after `0%`
    /// changes an assertion here.
    const REAL_120: &str = "  Context ░░░░░░░░░░ 0% │ Usage ██░░░░░░░░ 16%";

    /// cols=20 at a TRUE 100%. claude-hud's render budget overflows and the row is
    /// TRUNCATED right-to-left with a literal `…` (U+2026); it never wraps. The `…` here
    /// is captured bytes, not an elision.
    const TRUNCATED_100: &str = "  Context ████ 10…";

    fn extract_with(pattern: &str, rows: &[&str]) -> Option<u8> {
        let compiled = compile(pattern).expect("pattern compiles");
        let owned: Vec<String> = rows.iter().map(|r| r.to_string()).collect();
        extract(&compiled, &owned)
    }

    #[test]
    fn the_real_120_col_row_extracts() {
        assert_eq!(extract_with(CLAUDE, &[REAL_120]), Some(0));
    }

    #[test]
    fn truncation_fails_closed_when_the_pattern_requires_percent() {
        // The truth is 100. Truncation eats right-to-left, so it necessarily ate the `%`
        // before it could eat a digit - requiring the `%` is what makes this unavailable
        // instead of confidently wrong.
        assert_eq!(extract_with(CLAUDE, &[TRUNCATED_100]), None);
    }

    #[test]
    fn truncation_without_the_percent_rule_returns_a_wrong_number() {
        // Why that rule exists, pinned so a careless pattern edit cannot drop it silently:
        // the same row reports 10 for a session that is actually FULL.
        assert_eq!(
            extract_with(r"Context [░█]+ (\d+)", &[TRUNCATED_100]),
            Some(10)
        );
    }

    #[test]
    fn the_lowest_matching_row_wins() {
        // A pasted line that matches at column 2, sitting above the real statusline.
        // Bottom-up is the only thing that separates them.
        let rows = ["  Context ██████████ 99% (pasted)", "", REAL_120];
        assert_eq!(extract_with(CLAUDE, &rows), Some(0));

        // ...and the fixture is only worth anything if it distinguishes scan order, which
        // is precisely what the version of this test in an earlier revision did not.
        let compiled = compile(CLAUDE).expect("pattern compiles");
        let top_down = rows.iter().find_map(|row| {
            compiled
                .regex()
                .captures(row)
                .and_then(|c| c.get(1)?.as_str().parse::<u8>().ok())
        });
        assert_eq!(
            top_down,
            Some(99),
            "fixture must distinguish scan order or it pins nothing"
        );
    }

    #[test]
    fn input_box_prose_is_rejected_by_the_column_two_anchor() {
        // Typed into the input box, and it beats every glyph rule: the bar is real, the `%`
        // is real, the number is a lie. `❯ ` puts `Context` at column 16.
        assert_eq!(
            extract_with(CLAUDE, &["❯ The row says Context ██████████ 99% right now"]),
            None
        );
    }

    #[test]
    fn a_wrapped_input_continuation_defeats_the_column_two_anchor() {
        // The accepted residual, pinned as measured rather than left as a surprise: `❯`
        // decorates only the FIRST visual row, so a wrapped continuation of the input box
        // starts at exactly column 2 and the anchor cannot tell it from the statusline.
        // Self-corrects on the next tick once the statusline is back.
        assert_eq!(
            extract_with(CLAUDE, &["  Context ██████████ 99% tail"]),
            Some(99)
        );
    }

    #[test]
    fn the_85_percent_breakdown_still_extracts() {
        // Content follows the percent, so a pattern anchored with `$` after `%` would read
        // this as unavailable.
        assert_eq!(
            extract_with(
                CLAUDE,
                &["  Context █████████░ 87% (in: 12k, cache: 40k) │"]
            ),
            Some(87)
        );
    }

    /// Re-captured 2026-07-17 against claude.exe 2.1.212 at cols 80 and 40, with the rig
    /// the round-1 capture used: portable-pty 0.8.1 + vt100 0.15.2,
    /// `Parser::new(rows, cols, 0)`, rows read back with `contents_between`. The bar really
    /// does shrink 10 -> 6 -> 4 blocks at 120 -> 80 -> 40, exactly as recorded.
    /// Note the SECOND percent on this row (`Usage ... 8%`). What keeps the reading on
    /// the context segment is the literal `Context [░█]+ ` before the capture group: the
    /// pattern has no `.*`, so it can only match where the word `Context` actually is, and
    /// `Usage ... 8%` is not preceded by it. Measured, because the obvious answer is wrong:
    /// the column-2 anchor does NOTHING here (delete it and this row still reads 0), and it
    /// does not save you either (`^ {2}.*(\d{1,3})%` keeps the anchor and reads 8).
    const REAL_80: &str = "  Context ░░░░░░ 0% │ Usage ░░░░░░ 8% (resets in 3h 21m)";

    /// At 40 cols claude-hud DROPS the `│ Usage ...` segment rather than wrapping it, so the
    /// row simply ends at the percent. Nothing is ever split mid-segment.
    const REAL_40: &str = "  Context ░░░░ 0%";

    /// 120 is deliberately not re-asserted here: `the_real_120_col_row_extracts` owns it,
    /// and a test that restates another test earns nothing.
    #[test]
    fn the_placeholder_extracts_at_80_and_40_cols() {
        assert_eq!(extract_with(CLAUDE, &[REAL_80]), Some(0));
        assert_eq!(extract_with(CLAUDE, &[REAL_40]), Some(0));
    }

    #[test]
    fn zero_and_hundred_extract_despite_a_missing_glyph() {
        // `[░█]+` must not require BOTH glyphs: the ends of the range only have one.
        assert_eq!(extract_with(CLAUDE, &["  Context ░░░░░░░░░░ 0%"]), Some(0));
        assert_eq!(
            extract_with(CLAUDE, &["  Context ██████████ 100%"]),
            Some(100)
        );
    }

    /// Codex is glyphless and puts a SECOND `%` on the row (`weekly 83% left`).
    ///
    /// What excludes it is the literal `\u{b7} Context ` immediately before the capture group -
    /// `weekly 83%` is not preceded by it. NOT ` used`: measured, dropping ` used` from the
    /// pattern still reads Some(0). ` used` is harmless, but it is not the defence, and a
    /// pattern that leans on it instead of on the literal prefix is not protected. What
    /// Codex's `.*` DOES buy is real exposure: `^ {2}.*(\d{1,3})% ` reads Some(3) off this
    /// row - greedy `.*` eats the `8` and leaves `3` - so the literal prefix is the whole
    /// defence and must stay adjacent to the capture group.
    #[test]
    fn codex_row_extracts_context_not_weekly() {
        assert_eq!(
            extract_with(CODEX, &["  Ready · Context 0% used · weekly 83% left"]),
            Some(0)
        );
    }

    #[test]
    fn an_out_of_range_match_is_rejected() {
        // 999 is not a u8 at all, so the parse rejects it with or without a range check...
        assert_eq!(extract_with(CLAUDE, &["  Context ██████████ 999%"]), None);
        // ...but 200 is, and only the explicit `0..=100` rule rejects it. This is the line
        // that goes red if that rule is deleted; the plan's own fixture is 999, which does
        // not pin it.
        assert_eq!(extract_with(CLAUDE, &["  Context ██████████ 200%"]), None);
    }

    #[test]
    fn a_zero_percent_row_reports_zero_not_null() {
        // A fresh session's `used_percentage` is null, but claude-hud computes 0/1000000
        // and prints `0%`, byte-identical to a true 0%. Inferring null from Some(0) would
        // erase a genuine low reading, so the engine reports what it reads.
        assert_eq!(extract_with(CLAUDE, &["  Context ░░░░░░░░░░ 0%"]), Some(0));
    }
}
