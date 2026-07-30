//! #1171 - what the engine makes of one frame: which rows became evaluable, and what text
//! they carry.
//!
//! Two separate things live here, on purpose.
//!
//! **The diff is on PHYSICAL rows**, because physical arrival is what the alignment measures:
//! a scroll moves physical rows, and the question "did this row just arrive" is a question
//! about the grid.
//!
//! **Evaluation is on LOGICAL rows**, because `Screen::rows` returns physical ones and a line
//! longer than the terminal width occupies two or more of them, so no pattern can match across
//! the break. At 120 columns that is precisely what an absolute file path does, which is this
//! issue's primary use case.
//!
//! Everything here is a pure function over hash slices or over a frame, with no locks and no
//! engine state, which is why the riskiest piece of the feature is also the cheapest to test.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use super::{FrameStamp, ScreenFrame};

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

/// One 64-bit hash per physical row.
///
/// The previous frame's TEXT is never kept: hashes are enough for both alignment and change
/// detection, at 8 bytes per row instead of up to 120 - about 240 bytes per session instead of
/// about 3.6 KB. `DefaultHasher` needs no new dependency, and a 64-bit collision costs one
/// missed evaluation, which is the fail-closed direction this module takes everywhere.
pub fn hash_rows(rows: &[String]) -> Vec<u64> {
    rows.iter()
        .map(|row| {
            let mut hasher = DefaultHasher::new();
            row.hash(&mut hasher);
            hasher.finish()
        })
        .collect()
}

/// How well the previous frame lines up with the current one at a shift of `k`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alignment {
    /// How far the screen appears to have scrolled: `prev[i + k]` is believed to be `curr[i]`.
    pub k: usize,
    /// How many of the overlapping rows agreed.
    pub agree: usize,
    /// How many rows overlap at this shift: `R - k`.
    pub overlap: usize,
}

impl Alignment {
    /// "The sampler could not follow the screen": fewer than half the overlapping rows agreed.
    /// A `clear`, an alternate-screen switch, a full repaint, or a scroll burst larger than the
    /// screen all land here, and all of them mean something may have gone past unsampled.
    pub fn lost_the_screen(&self) -> bool {
        2 * self.agree < self.overlap
    }
}

/// Find the shift that best explains the current frame in terms of the previous one.
///
/// **This is a best-shift search and not slice equality.** Requiring `prev[k..] == curr[..R-k]`
/// makes the in-place branch unreachable by construction - exact equality of the overlap leaves
/// nothing to differ - and with any repainting statusline no `k` satisfies it at all, so every
/// tick would fall through to a full re-render, permanently lighting the sampling-loss line.
///
/// Cost is at most `R * (R + 1) / 2` u64 comparisons, 465 at AC's default 30 rows, per changed
/// session per tick.
///
/// Ties break to the SMALLEST k, which declares the fewest rows new: ambiguity under-counts
/// rather than inventing arrivals.
pub fn best_shift(prev: &[u64], curr: &[u64]) -> Alignment {
    let rows = curr.len().min(prev.len());
    let mut best = Alignment {
        k: 0,
        agree: 0,
        overlap: rows,
    };
    for k in 0..rows {
        let overlap = rows - k;
        let agree = (0..overlap).filter(|i| prev[i + k] == curr[*i]).count();
        if agree > best.agree {
            best = Alignment { k, agree, overlap };
        }
    }
    best
}

/// Move the dirty flags up by `k`, clearing the tail.
///
/// `new[i] = old[i + k]` for `i` in `0 .. R - k`, and `false` for the rows that just arrived.
/// This is the step most easily implemented wrong, so it is a named function with a test of
/// its own rather than three lines inside the diff.
pub fn shift_dirty(dirty: &mut [bool], k: usize) {
    if k == 0 {
        return;
    }
    let rows = dirty.len();
    if k >= rows {
        dirty.fill(false);
        return;
    }
    dirty.copy_within(k.., 0);
    dirty[rows - k..].fill(false);
}

/// Everything the diff remembers about one session between ticks.
///
/// About 9 bytes per row: a hash and a flag. At 30 rows that is 240 bytes per session, about
/// 12 KB at fifty sessions.
#[derive(Default)]
pub struct FrameDiffState {
    prev_hashes: Vec<u64>,
    dirty: Vec<bool>,
    size: Option<(u16, u16)>,
    /// Whether anything has been evaluated since the last reseed. A resize that reseeds before
    /// anything was ever evaluated is not a sampling loss, it is a startup.
    evaluated_since_reseed: bool,
    /// Monotonic since the session started. Above zero means "something MAY have been missed",
    /// never "N things were missed". That is a property of sampling, not a defect.
    possibly_missed_frames: u64,
}

impl FrameDiffState {
    pub fn possibly_missed_frames(&self) -> u64 {
        self.possibly_missed_frames
    }

    /// Advance one tick and return the PHYSICAL rows that became evaluable.
    ///
    /// The rules, in order:
    ///
    /// 1. **First tick after registration.** Seed and evaluate nothing. There was no sampling
    ///    gap, and reporting the whole starting screen as fresh activity would be wrong.
    /// 2. **Size change.** A reflow re-lays every row at a new width, so reseed and evaluate
    ///    nothing. This counts as a miss ONLY if something had been evaluated since the last
    ///    reseed: the frontend fits xterm shortly after spawn, so the first resize is near
    ///    universal, and counting it would light the sampling-loss line on nearly every session
    ///    from birth - which is exactly what separating that line from the buffer banner exists
    ///    to avoid.
    /// 3. **New by scroll**, the bottom `k` rows, EXCLUDING the cursor row: evaluated at once.
    ///    They arrived complete from below and need no stabilization.
    /// 4. **In place**, everything else plus the cursor row: evaluated on the next tick in
    ///    which its hash is unchanged.
    ///
    /// **The cursor row is excluded from step 3** because a terminal that prints a newline
    /// lands the cursor on an empty bottom row and only THEN writes it. At a 200 ms cadence
    /// against 4096-byte read chunks, landing mid-write is routine, and evaluating it would
    /// emit one event with a truncated capture followed by a second with the complete one - two
    /// events no dedupe setting can merge, because both the row and the captures differ.
    ///
    /// **There is no "re-render, evaluate everything now" path.** A repaint produces a
    /// low-agreement alignment whose differing rows all land in `dirty`, and they are evaluated
    /// one tick later if the new screen is stable, which it is: one evaluation per row, in
    /// order, with no burst.
    ///
    /// A dirty row that scrolls off the top is lost - the engine never kept its text - and that
    /// does NOT count as a miss, so `possibly_missed_frames` keeps exactly one meaning.
    pub fn advance(&mut self, frame: &ScreenFrame) -> Vec<usize> {
        let curr = hash_rows(&frame.rows);
        let size = frame
            .stamp
            .map(|stamp: FrameStamp| (stamp.rows, stamp.cols));

        // A backend with no stamp to report cannot promise the size is unchanged, so it
        // reseeds every tick and never evaluates. Only the defaulted trait method is in that
        // position, and it already cannot report `Unchanged` either.
        let first_tick = self.prev_hashes.is_empty();
        let reflowed = size.is_none() || self.size != size;
        if first_tick || reflowed {
            if !first_tick && self.evaluated_since_reseed {
                self.possibly_missed_frames += 1;
            }
            self.prev_hashes = curr;
            self.dirty = vec![false; frame.rows.len()];
            self.size = size;
            self.evaluated_since_reseed = false;
            return Vec::new();
        }

        let alignment = best_shift(&self.prev_hashes, &curr);
        if alignment.lost_the_screen() {
            self.possibly_missed_frames += 1;
        }

        let rows = curr.len();
        let k = alignment.k;
        shift_dirty(&mut self.dirty, k);

        let cursor_row = frame.cursor_row as usize;
        let scrolled_in = rows.saturating_sub(k);
        let mut evaluate = Vec::new();

        for (row, hash) in curr.iter().enumerate() {
            let arrived_by_scroll = k >= 1 && row >= scrolled_in;
            if arrived_by_scroll && row != cursor_row {
                evaluate.push(row);
                self.dirty[row] = false;
                continue;
            }

            // In place, and the cursor row that arrived by scroll with it: there is no shifted
            // previous hash for the latter, which is the same thing as "it changed".
            let previous = if arrived_by_scroll {
                None
            } else {
                self.prev_hashes.get(row + k).copied()
            };
            match previous {
                Some(previous) if previous == *hash => {
                    if self.dirty[row] {
                        // Stable for one full tick: whatever was being written is finished.
                        evaluate.push(row);
                        self.dirty[row] = false;
                    }
                }
                _ => self.dirty[row] = true,
            }
        }

        self.prev_hashes = curr;
        self.size = size;
        if !evaluate.is_empty() {
            self.evaluated_since_reseed = true;
        }
        evaluate
    }
}

/// Map the physical rows the diff selected onto the logical rows to evaluate.
///
/// A continuation selected while its start row was not still evaluates the LOGICAL row it
/// belongs to, once: selection is mapped from physical to logical before evaluation, and a
/// logical row is evaluated at most once per tick. A physical row belonging to no logical row -
/// the withheld top-edge case - selects nothing.
pub fn select_logical<'a>(logical: &'a [LogicalRow], physical: &[usize]) -> Vec<&'a LogicalRow> {
    let mut selected: Vec<&LogicalRow> = Vec::new();
    for row in physical {
        if let Some(found) = logical
            .iter()
            .find(|candidate| candidate.start <= *row && *row <= candidate.end)
        {
            if !selected.iter().any(|already| already.start == found.start) {
                selected.push(found);
            }
        }
    }
    selected.sort_by_key(|row| row.start);
    selected
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

    // ---- the frame diff ------------------------------------------------------------------

    /// A frame at a given sequence, with the cursor parked where it cannot interfere.
    fn tick_frame(rows: &[&str], sequence: u64, cursor_row: u16) -> ScreenFrame {
        ScreenFrame {
            rows: rows.iter().map(|r| r.to_string()).collect(),
            wrapped: vec![false; rows.len()],
            cursor_row,
            stamp: Some(FrameStamp {
                sequence,
                rows: rows.len() as u16,
                cols: 120,
            }),
        }
    }

    fn hashes(rows: &[&str]) -> Vec<u64> {
        hash_rows(&rows.iter().map(|r| r.to_string()).collect::<Vec<_>>())
    }

    /// 9.4.38 - nothing moved: shift zero, everything agrees.
    #[test]
    fn best_shift_of_two_identical_frames_is_zero_with_full_agreement() {
        let rows = hashes(&["a", "b", "c", "d", "e"]);

        let alignment = best_shift(&rows, &rows);
        assert_eq!(alignment.k, 0);
        assert_eq!(alignment.agree, 5);
        assert_eq!(alignment.overlap, 5);
        assert!(!alignment.lost_the_screen());
    }

    /// 9.4.39 - a clean scroll of three rows is a shift of three, and exactly the bottom three
    /// rows are declared new.
    #[test]
    fn a_frame_scrolled_by_three_declares_exactly_the_bottom_three_rows_new() {
        let mut state = FrameDiffState::default();
        state.advance(&tick_frame(&["a", "b", "c", "d", "e"], 1, 0));

        let evaluated = state.advance(&tick_frame(&["d", "e", "f", "g", "h"], 2, 0));

        assert_eq!(evaluated, vec![2, 3, 4]);
        assert_eq!(state.possibly_missed_frames(), 0);
    }

    /// 9.4.40 - **the case revision 1's slice-equality predicate made unsatisfiable, and the
    /// normal case for any TUI with a statusline.**
    ///
    /// The screen scrolled by three AND the bottom row was repainted in place. Exact equality
    /// of the overlap cannot describe that at any `k`; best-shift can, and the repainted row
    /// lands in the dirty set instead of taking the whole tick down a re-render path.
    #[test]
    fn a_scroll_whose_bottom_row_also_changed_still_aligns_and_leaves_that_row_dirty() {
        let mut state = FrameDiffState::default();
        // Row 4 is a statusline: it repaints on every tick and never scrolls with the rest.
        state.advance(&tick_frame(&["a", "b", "c", "d", "status 1"], 1, 0));

        let alignment = best_shift(
            &hashes(&["a", "b", "c", "d", "status 1"]),
            &hashes(&["d", "status 1", "f", "g", "status 2"]),
        );
        assert_eq!(alignment.k, 3);
        assert!(!alignment.lost_the_screen());

        let evaluated = state.advance(&tick_frame(&["d", "status 1", "f", "g", "status 2"], 2, 0));
        assert_eq!(
            evaluated,
            vec![2, 3, 4],
            "the three rows that arrived from below, statusline included"
        );

        // The statusline changing again is now an in-place change: dirty, not evaluated.
        let evaluated = state.advance(&tick_frame(&["d", "status 1", "f", "g", "status 3"], 3, 0));
        assert!(evaluated.is_empty());
        // ...and it IS evaluated once it holds still for a tick.
        let evaluated = state.advance(&tick_frame(&["d", "status 1", "f", "g", "status 3"], 4, 0));
        assert_eq!(evaluated, vec![4]);
        assert_eq!(state.possibly_missed_frames(), 0);
    }

    /// 9.4.41 - one statusline row changing is not a lost screen. It aligns at zero with
    /// `R - 1` agreement, evaluates nothing, marks that row dirty, and counts NO miss.
    #[test]
    fn a_single_changed_row_evaluates_nothing_and_counts_no_miss() {
        let mut state = FrameDiffState::default();
        state.advance(&tick_frame(&["a", "b", "c", "d", "status 1"], 1, 0));

        let alignment = best_shift(
            &hashes(&["a", "b", "c", "d", "status 1"]),
            &hashes(&["a", "b", "c", "d", "status 2"]),
        );
        assert_eq!((alignment.k, alignment.agree), (0, 4));
        assert!(!alignment.lost_the_screen());

        let evaluated = state.advance(&tick_frame(&["a", "b", "c", "d", "status 2"], 2, 0));
        assert!(evaluated.is_empty());
        assert_eq!(state.possibly_missed_frames(), 0);
    }

    /// 9.4.42 - a row that changes on every tick is never stable, so it is never evaluated.
    /// This is what keeps a clock or a spinner from emitting five events a second.
    #[test]
    fn a_row_that_changes_every_tick_is_never_evaluated() {
        let mut state = FrameDiffState::default();
        state.advance(&tick_frame(&["a", "b", "spinner 0"], 1, 0));

        for i in 1..10 {
            let row = format!("spinner {i}");
            let evaluated = state.advance(&tick_frame(&["a", "b", &row], i + 1, 0));
            assert!(evaluated.is_empty(), "tick {i} evaluated {evaluated:?}");
        }
        assert_eq!(state.possibly_missed_frames(), 0);
    }

    /// 9.4.43 - the step most easily implemented wrong, on its own.
    #[test]
    fn shift_dirty_moves_the_flags_and_clears_the_tail() {
        let mut dirty = vec![true, false, true, false, true];
        shift_dirty(&mut dirty, 2);
        assert_eq!(dirty, vec![true, false, true, false, false]);

        let mut dirty = vec![true, true, true];
        shift_dirty(&mut dirty, 0);
        assert_eq!(
            dirty,
            vec![true, true, true],
            "a shift of zero moves nothing"
        );

        let mut dirty = vec![true, true, true];
        shift_dirty(&mut dirty, 5);
        assert_eq!(
            dirty,
            vec![false, false, false],
            "a scroll larger than the screen leaves nothing carried over"
        );
    }

    /// 9.4.44 - a full repaint counts a miss, evaluates NOTHING that tick, and evaluates each
    /// changed row exactly once on the next stable tick. There is deliberately no
    /// evaluate-everything-now path: that would be a burst, in no order, relying on the TTL
    /// layer to clean up after it.
    #[test]
    fn a_repaint_counts_a_miss_and_evaluates_each_row_once_on_the_next_stable_tick() {
        let mut state = FrameDiffState::default();
        state.advance(&tick_frame(&["a", "b", "c", "d"], 1, 0));
        // Make the seeded frame have something evaluated, so the state is past its startup.
        state.advance(&tick_frame(&["b", "c", "d", "e"], 2, 0));

        let evaluated = state.advance(&tick_frame(&["w", "x", "y", "z"], 3, 0));
        assert!(
            evaluated.is_empty(),
            "a repaint emits nothing at the moment it lands"
        );
        assert_eq!(state.possibly_missed_frames(), 1);

        let evaluated = state.advance(&tick_frame(&["w", "x", "y", "z"], 4, 0));
        assert_eq!(evaluated, vec![0, 1, 2, 3], "once each, in order");

        let evaluated = state.advance(&tick_frame(&["w", "x", "y", "z"], 5, 0));
        assert!(
            evaluated.is_empty(),
            "and never again while they hold still"
        );
        assert_eq!(state.possibly_missed_frames(), 1, "still exactly one");
    }

    /// 9.4.45 (the diff half) - the cursor row is NOT evaluated on the tick it arrives by
    /// scroll, and IS evaluated once, complete, when it stabilizes.
    ///
    /// A terminal that prints a newline lands the cursor on an empty bottom row and only then
    /// writes it; at 200 ms against 4 KB chunks, landing mid-write is routine.
    #[test]
    fn the_cursor_row_is_evaluated_one_tick_later_rather_than_half_written() {
        let mut state = FrameDiffState::default();
        state.advance(&tick_frame(&["a", "b", "c"], 1, 2));

        // Scrolled by one; the cursor is on the new bottom row, which is still being written.
        let evaluated = state.advance(&tick_frame(&["b", "c", "Read /path/to/fi"], 2, 2));
        assert!(
            evaluated.is_empty(),
            "the half-written cursor row must not be evaluated with a truncated capture"
        );

        // The write finished. Still not stable, so still nothing.
        let evaluated = state.advance(&tick_frame(&["b", "c", "Read /path/to/file.rs"], 3, 2));
        assert!(evaluated.is_empty());

        // Stable: evaluated once, complete.
        let evaluated = state.advance(&tick_frame(&["b", "c", "Read /path/to/file.rs"], 4, 2));
        assert_eq!(evaluated, vec![2]);
    }

    /// 9.4.48 - the first tick after registration evaluates nothing and counts no miss. There
    /// was no sampling gap; the whole starting screen is not fresh activity.
    #[test]
    fn the_first_tick_evaluates_nothing_and_counts_no_miss() {
        let mut state = FrameDiffState::default();

        let evaluated = state.advance(&tick_frame(&["a", "b", "c"], 1, 0));

        assert!(evaluated.is_empty());
        assert_eq!(state.possibly_missed_frames(), 0);
    }

    /// 9.4.49 - a resize before anything was evaluated is a startup, not a loss. After
    /// something HAS been evaluated it is a real gap and is counted.
    ///
    /// Without the distinction, the frontend fitting xterm shortly after spawn would light the
    /// sampling-loss line on nearly every session from birth.
    #[test]
    fn a_resize_counts_a_miss_only_after_something_has_been_evaluated() {
        let mut early = FrameDiffState::default();
        early.advance(&tick_frame(&["a", "b", "c"], 1, 0));
        early.advance(&ScreenFrame {
            rows: vec!["a".into(), "b".into()],
            wrapped: vec![false; 2],
            cursor_row: 0,
            stamp: Some(FrameStamp {
                sequence: 2,
                rows: 2,
                cols: 100,
            }),
        });
        assert_eq!(early.possibly_missed_frames(), 0);

        let mut later = FrameDiffState::default();
        later.advance(&tick_frame(&["a", "b", "c"], 1, 0));
        later.advance(&tick_frame(&["b", "c", "d"], 2, 0));
        assert_eq!(later.possibly_missed_frames(), 0);
        later.advance(&ScreenFrame {
            rows: vec!["b".into(), "c".into()],
            wrapped: vec![false; 2],
            cursor_row: 0,
            stamp: Some(FrameStamp {
                sequence: 3,
                rows: 2,
                cols: 100,
            }),
        });
        assert_eq!(later.possibly_missed_frames(), 1);
    }

    /// 9.4.51 - a row shifted up by a scroll is not re-evaluated. It is the same row, at a new
    /// position, and it was already counted when it arrived.
    #[test]
    fn a_row_shifted_up_by_a_scroll_is_not_re_evaluated() {
        let mut state = FrameDiffState::default();
        state.advance(&tick_frame(&["a", "b", "c", "d"], 1, 0));
        let evaluated = state.advance(&tick_frame(&["b", "c", "d", "hit"], 2, 0));
        assert_eq!(evaluated, vec![3]);

        let evaluated = state.advance(&tick_frame(&["c", "d", "hit", "next"], 3, 0));
        assert_eq!(
            evaluated,
            vec![3],
            "only the newly arrived row: `hit` moved up and must not count twice"
        );
    }

    /// 9.4.57 - `possibly_missed_frames` only ever grows.
    #[test]
    fn possibly_missed_frames_is_monotonic() {
        let mut state = FrameDiffState::default();
        let mut seen = 0;
        for i in 0..30u64 {
            let row = format!("burst {i}");
            state.advance(&tick_frame(&[&row, "x", "y"], i + 1, 0));
            let now = state.possibly_missed_frames();
            assert!(now >= seen, "went backwards at tick {i}: {seen} -> {now}");
            seen = now;
        }
    }

    /// The physical-to-logical mapping: a continuation selected on its own evaluates the whole
    /// logical row it belongs to, once, and a withheld top-edge row selects nothing.
    #[test]
    fn selection_maps_physical_rows_onto_logical_rows_without_duplicates() {
        let top_edge = frame(&["head", "one", "two", "tail"], &[true, true, false, false]);
        let logical = logical_rows(&top_edge);

        // Rows 0-2 are the withheld top-edge logical row; row 3 is its own.
        assert_eq!(select_logical(&logical, &[0]).len(), 0);
        assert_eq!(select_logical(&logical, &[1, 2]).len(), 0);
        let selected = select_logical(&logical, &[3, 3]);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].text, "tail");

        let plain = frame(&["a", "b", "c"], &[false, true, false]);
        let logical = logical_rows(&plain);
        let selected = select_logical(&logical, &[2, 1]);
        assert_eq!(
            selected.len(),
            1,
            "row 2 is a continuation of row 1: one logical row, evaluated once"
        );
        assert_eq!(selected[0].text, "bc");
    }
}
