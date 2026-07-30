//! #1171 - what the engine makes of one frame: logical rows now, the frame diff in the
//! phase that needs it.
//!
//! `Screen::rows` returns PHYSICAL rows, so a line longer than the terminal width occupies
//! two or more of them and no pattern can match across the break. At 120 columns that is
//! precisely what an absolute file path does, which is this issue's primary use case, so
//! evaluation is on logical rows even though the diff stays on physical ones.

use super::ScreenFrame;

/// One or more physical rows, joined across the wrap flag.
///
/// There is no separator in the join: `write_contents` emits no trailing padding
/// (`vt100 row.rs:122-133`), so concatenating the segments reconstitutes the original line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalRow {
    /// The physical row it starts at. `state` mode's "the lowest match wins" orders by this.
    pub start: usize,
    /// The physical row it ends at, inclusive. Equal to `start` for an unwrapped line.
    pub end: usize,
    pub text: String,
}

/// The logical rows of a frame, top to bottom.
///
/// **A logical row that starts at physical row 0 and spans more than one physical row is
/// skipped.** The mirror is created with ZERO scrollback (`output.rs:112`), so there is
/// nothing above row 0 to check the line's beginning against, and a line that reaches the top
/// edge while still wrapping is exactly the shape of one whose head has just scrolled off.
/// Evaluating it would emit a truncated capture - `/to/file.rs` for `/path/to/file.rs` - which
/// is a WRONG answer, where skipping it is only a missed one. That is the fail-closed
/// direction this module takes everywhere.
///
/// **Known residual, stated rather than hidden.** When a two-physical-row line loses its
/// first row, the survivor is an ordinary unwrapped row 0 and is indistinguishable from a
/// line that genuinely begins there. `vt100` exposes `row_wrapped(i)` - "does row i continue
/// INTO row i+1" - and nothing that says "row i continues FROM row i-1", so with no
/// scrollback that case cannot be detected at all. It is one row of thirty, only while a
/// wrapped line is straddling the top edge, and it self-corrects on the next scroll.
///
/// A tail that runs off the BOTTOM edge is not skipped: the visible part is evaluated, and if
/// the pattern needed the missing end it simply does not match until the next scroll brings
/// it in. Nothing wrong is emitted either way.
pub fn logical_rows(frame: &ScreenFrame) -> Vec<LogicalRow> {
    let mut out = Vec::new();
    let mut start = 0usize;

    while start < frame.rows.len() {
        let mut end = start;
        while end + 1 < frame.rows.len() && frame.wrapped.get(end).copied().unwrap_or(false) {
            end += 1;
        }

        if start == 0 && end > start {
            start = end + 1;
            continue;
        }

        let mut text = String::new();
        for row in &frame.rows[start..=end] {
            text.push_str(row);
        }
        out.push(LogicalRow { start, end, text });
        start = end + 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::watchers::FrameStamp;

    fn frame(rows: &[&str], wrapped: &[bool]) -> ScreenFrame {
        ScreenFrame {
            rows: rows.iter().map(|r| r.to_string()).collect(),
            wrapped: wrapped.to_vec(),
            cursor_row: 0,
            stamp: Some(FrameStamp {
                sequence: 1,
                rows: rows.len() as u16,
                cols: 120,
            }),
        }
    }

    /// 9.4.46 - the issue's primary use case. A path wider than the terminal occupies two
    /// physical rows and matches only when they are joined.
    #[test]
    fn a_line_spanning_two_physical_rows_is_one_logical_row() {
        let frame = frame(
            &[
                "idle",
                "Read (C:/repo/very/long/pa",
                "th/to/main.rs)",
                "idle",
            ],
            &[false, true, false, false],
        );

        let logical = logical_rows(&frame);
        assert_eq!(logical.len(), 3);
        assert_eq!(logical[1].start, 1);
        assert_eq!(logical[1].end, 2);
        assert_eq!(logical[1].text, "Read (C:/repo/very/long/path/to/main.rs)");

        let pattern = regex::Regex::new(r"Read \((.+)\)").expect("compiles");
        assert_eq!(
            &pattern.captures(&logical[1].text).expect("matches")[1],
            "C:/repo/very/long/path/to/main.rs",
            "the whole point: this pattern cannot match either physical row on its own"
        );
        assert!(!pattern.is_match(&frame.rows[1]));
        assert!(!pattern.is_match(&frame.rows[2]));
    }

    /// A continuation is never a logical row of its own, so no fragment is ever evaluated in
    /// the middle of the screen - and a line that wraps twice joins all three of its rows.
    #[test]
    fn a_continuation_never_becomes_a_logical_row_of_its_own() {
        let frame = frame(
            &["alone", "one", "two", "three", "last"],
            &[false, true, true, false, false],
        );

        let logical = logical_rows(&frame);
        let starts: Vec<usize> = logical.iter().map(|r| r.start).collect();
        assert_eq!(starts, vec![0, 1, 4]);
        assert_eq!(logical[1].text, "onetwothree");
        assert_eq!((logical[1].start, logical[1].end), (1, 3));
    }

    /// 9.4.47 - a wrapped line touching the TOP edge may have lost its head, and there is no
    /// scrollback to check. Skipped: a missed detection beats a truncated capture.
    #[test]
    fn a_wrapped_line_at_the_top_edge_is_skipped_rather_than_evaluated_as_a_fragment() {
        let frame = frame(&["th/to/main.rs)", " done", "next"], &[true, false, false]);

        let starts: Vec<usize> = logical_rows(&frame).iter().map(|r| r.start).collect();
        assert_eq!(
            starts,
            vec![2],
            "rows 0-1 are one logical row touching the top edge, so its beginning is unknowable \
             and BOTH are withheld: row 1 is a continuation and was never evaluable on its own"
        );
    }

    /// ...but an ORDINARY row 0 is still evaluated. Skipping every top row would lose one
    /// line of every screen forever, which the fail-closed rule does not ask for.
    #[test]
    fn an_unwrapped_row_zero_is_still_a_logical_row() {
        let frame = frame(&["top", "middle", "bottom"], &[false, false, false]);

        let starts: Vec<usize> = logical_rows(&frame).iter().map(|r| r.start).collect();
        assert_eq!(starts, vec![0, 1, 2]);
    }

    /// A wrap flag on the LAST row points off the bottom of the screen. The visible part is
    /// evaluated; nothing is joined past the end and nothing panics.
    #[test]
    fn a_wrap_flag_on_the_last_row_does_not_run_off_the_end() {
        let frame = frame(&["a", "b continues below"], &[false, true]);

        let logical = logical_rows(&frame);
        assert_eq!(logical.len(), 2);
        assert_eq!(logical[1].start, 1);
        assert_eq!(logical[1].end, 1);
        assert_eq!(logical[1].text, "b continues below");
    }

    #[test]
    fn an_empty_frame_yields_no_logical_rows() {
        assert!(logical_rows(&frame(&[], &[])).is_empty());
    }
}
