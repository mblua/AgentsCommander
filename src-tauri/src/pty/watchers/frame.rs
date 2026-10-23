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
        // A frame whose row count disagrees with the state carried over is a reseed too, and
        // NOT an assumption to index on. The stamp's `rows` and the actual row count agree for
        // both real backends today, but nothing in the type says they must: a backend
        // reporting an unchanged stamp with a different number of rows would otherwise index
        // `dirty` out of bounds and take the whole engine down with a panic.
        let desynced = self.prev_hashes.len() != curr.len() || self.dirty.len() != curr.len();
        let reflowed = size.is_none() || self.size != size || desynced;
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
            // `.get_mut` and not indexing: the reseed above already guarantees the lengths
            // agree, and this is the one loop where being wrong about that would be a panic
            // inside the engine thread rather than a missed evaluation.
            let Some(dirty) = self.dirty.get_mut(row) else {
                continue;
            };
            let arrived_by_scroll = k >= 1 && row >= scrolled_in;
            if arrived_by_scroll && row != cursor_row {
                evaluate.push(row);
                *dirty = false;
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
                    if *dirty {
                        // Stable for one full tick: whatever was being written is finished.
                        evaluate.push(row);
                        *dirty = false;
                    }
                }
                _ => *dirty = true,
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
mod tests;
