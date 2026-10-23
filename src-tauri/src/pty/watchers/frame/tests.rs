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

/// A frame whose row count disagrees with the stamp it carries reseeds instead of indexing
/// off the end.
///
/// Both real backends build the stamp from the same grid they clone the rows from, so this
/// cannot happen today - but nothing in the TYPE says so, and the cost of being wrong here
/// is a panic on the engine thread, which stops every session's sampling for the life of
/// the process. Reseeding is the fail-closed answer: lose a tick, not the engine.
#[test]
fn a_frame_whose_row_count_contradicts_its_stamp_reseeds_instead_of_panicking() {
    let mut state = FrameDiffState::default();
    state.advance(&tick_frame(&["a", "b", "c"], 1, 0));
    state.advance(&tick_frame(&["b", "c", "d"], 2, 0));

    // Same stamp size, fewer rows than it claims.
    let lying = ScreenFrame {
        rows: vec!["b".into()],
        wrapped: vec![false],
        cursor_row: 0,
        stamp: Some(FrameStamp {
            sequence: 3,
            rows: 3,
            cols: 120,
        }),
    };
    assert!(
        state.advance(&lying).is_empty(),
        "a reseed evaluates nothing"
    );

    // ...and the state is coherent afterwards, so the next real frame works normally.
    let evaluated = state.advance(&ScreenFrame {
        rows: vec!["z".into()],
        wrapped: vec![false],
        cursor_row: 5,
        stamp: Some(FrameStamp {
            sequence: 4,
            rows: 3,
            cols: 120,
        }),
    });
    assert!(evaluated.is_empty());
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

/// #2879 - test-only instrument, not a product constant: the Codex status header with a
/// complete `esc to interrupt` hint, upstream `fmt_elapsed_compact` durations, and an
/// optional ` · ` context suffix.
const I2879_PATTERN: &str = include_str!("tests/fixtures/working.regex");

/// `(name, sha256, bytes, expected len, row 17 text, expected matches)`.
type I2879OfflineCase = (
    &'static str,
    &'static str,
    &'static [u8],
    usize,
    &'static str,
    Vec<(usize, usize)>,
);

/// `(name, bytes, row 17 text, expected matches)`.
type I2879PrefixCase = (
    &'static str,
    &'static [u8],
    &'static str,
    Vec<(usize, usize)>,
);

/// Everything the real pipeline produces from one 24x80 `vt100` screen.
#[derive(Debug, PartialEq)]
struct I2879Snapshot {
    rows: Vec<String>,
    wrapped: Vec<bool>,
    cursor_row: u16,
    logical: Vec<LogicalRow>,
}

/// Bytes -> vt100 -> `ScreenFrame` (with the real wrap flags) -> `logical_rows`.
fn i2879_snapshot(chunks: &[&[u8]]) -> I2879Snapshot {
    let mut parser = vt100::Parser::new(24, 80, 0);
    for chunk in chunks {
        parser.process(chunk);
    }
    let screen = parser.screen();
    let frame = ScreenFrame {
        rows: screen.rows(0, 80).collect(),
        wrapped: (0..24).map(|i| screen.row_wrapped(i)).collect(),
        cursor_row: screen.cursor_position().0,
        stamp: None,
    };
    let logical = logical_rows(&frame);
    I2879Snapshot {
        rows: frame.rows,
        wrapped: frame.wrapped,
        cursor_row: frame.cursor_row,
        logical,
    }
}

/// `(start, end)` of every logical row the pattern matches.
fn i2879_matches(snapshot: &I2879Snapshot) -> Vec<(usize, usize)> {
    let pattern = super::super::pattern::compile(I2879_PATTERN).expect("compiles");
    snapshot
        .logical
        .iter()
        .filter(|row| pattern.regex().is_match(&row.text))
        .map(|row| (row.start, row.end))
        .collect()
}

/// The whole stream, then every split point `k = 0..=len`, then byte by byte, each from an
/// empty parser. Only the final state is compared.
fn i2879_snapshot_every_split(name: &str, bytes: &[u8]) -> I2879Snapshot {
    let whole = i2879_snapshot(&[bytes]);
    for k in 0..=bytes.len() {
        let split = i2879_snapshot(&[&bytes[..k], &bytes[k..]]);
        assert_eq!(split, whole, "{name}: split at {k}");
        assert_eq!(
            i2879_matches(&split),
            i2879_matches(&whole),
            "{name}: split at {k}"
        );
    }
    let singles: Vec<&[u8]> = bytes.chunks(1).collect();
    let byte_by_byte = i2879_snapshot(&singles);
    assert_eq!(byte_by_byte, whole, "{name}: byte by byte");
    assert_eq!(
        i2879_matches(&byte_by_byte),
        i2879_matches(&whole),
        "{name}: byte by byte"
    );
    whole
}

/// #2879 - the seven offline fixtures, transcribed byte-exact from
/// `room-shared/i2879-offline-evidence/fixtures/*.ans` (SHA-256 per `SHA256SUMS-fixtures`).
/// CUP prefixes and the initial state are derived; f3 is synthetic, not recovered raw
/// bytes. None of this is a full session replay.
#[test]
fn i2879_offline_ansi() {
    const CORE_0S: &str = "Working (0s • esc to interrupt)";
    // (name, sha256, bytes, expected len, row 17 text, expected matches)
    let cases: [I2879OfflineCase; 6] = [
        (
            "f1-paint-csi1c",
            "e164f40c63c7e7a4adf5f4576d1f2f1be39e68b5d20c2acc804b386e99119ce1",
            include_bytes!("tests/fixtures/ansi-1.ans"),
            72,
            CORE_0S,
            vec![(17, 17)],
        ),
        (
            "f2-paint-space",
            "4926ed6fc7d4b30f624244c0745829515d25319a98381c5ef12e5863ed5226b2",
            include_bytes!("tests/fixtures/ansi-2.ans"),
            75,
            "Working (3s • esc to interrupt)",
            vec![(17, 17)],
        ),
        (
            "f3-cup-counter-derived",
            "7d50b3262cb57ccfc2350be08a0c8e33a03edfc42d08f67c922e827b712f5f90",
            include_bytes!("tests/fixtures/ansi-3.ans"),
            84,
            "Working (5s • esc to interrupt)",
            vec![(17, 17)],
        ),
        (
            "f4-clear-el",
            "81038ce1acb72099d6efb83e2277db673090fae93403a7c72fc7387b5759075d",
            include_bytes!("tests/fixtures/ansi-4.ans"),
            82,
            "",
            vec![],
        ),
        (
            "n1-keyword-only",
            "9fe68efa826830437bd57d38c408168dcf03c7531a9cb1305bb516c0e7d629d9",
            b"\x1b[18;1HWorking\x1b[21;3H",
            21,
            "Working",
            vec![],
        ),
        (
            "n2-extra-text",
            "6aa95a220f0f566027febede0ede120286a61e8b23c181b49bbad038bee99381",
            b"\x1b[18;1HWorking (0s \xe2\x80\xa2 esc to interrupt) extra\x1b[21;3H",
            53,
            "Working (0s • esc to interrupt) extra",
            vec![],
        ),
    ];

    for (name, sha256, bytes, len, row17, matches) in cases {
        assert_eq!(bytes.len(), len, "{name} ({sha256}): transcription length");
        let snapshot = i2879_snapshot_every_split(name, bytes);
        assert_eq!(snapshot.rows[17], row17, "{name}: row 17");
        let logical17 = snapshot
            .logical
            .iter()
            .find(|row| row.start == 17)
            .expect("row 17 starts a logical row");
        assert_eq!(
            (logical17.end, logical17.text.as_str()),
            (17, row17),
            "{name}"
        );
        assert_eq!(i2879_matches(&snapshot), matches, "{name}: matches");
    }

    // f5: 80 explicit spaces fill row 16, so the next printable autowraps into row 17.
    let name = "f5-wrap-autowrap";
    let sha256 = "bdd50457c09c56a9e4715cece6a68e17a0d3ad383c12990687d7171fd49448b5";
    let bytes: &[u8] = include_bytes!("tests/fixtures/ansi-5.ans");
    assert_eq!(bytes.len(), 152, "{name} ({sha256}): transcription length");
    let snapshot = i2879_snapshot_every_split(name, bytes);
    let spaces = " ".repeat(80);
    assert_eq!(snapshot.rows[16], spaces, "{name}: row 16");
    assert_eq!(snapshot.rows[17], CORE_0S, "{name}: row 17");
    assert!(snapshot.wrapped[16], "{name}: row 16 wraps into row 17");
    let logical16 = snapshot
        .logical
        .iter()
        .find(|row| row.start == 16)
        .expect("row 16 starts a logical row");
    assert_eq!(logical16.end, 17, "{name}");
    assert_eq!(logical16.text, format!("{spaces}{CORE_0S}"), "{name}");
    assert_eq!(i2879_matches(&snapshot), vec![(16, 17)], "{name}: matches");
}

/// #2879 - upstream `fmt_elapsed_compact` shapes (`Ns`, `Nm SSs`, `Nh MMm SSs`) and the
/// ` · ` context suffix, through the same pipeline, one independent 24x80 parser each.
#[test]
fn i2879_offline_elapsed() {
    let cases: [(&str, bool); 11] = [
        ("Working (59s • esc to interrupt)", true),
        ("Working (1m 00s • esc to interrupt)", true),
        ("Working (59m 59s • esc to interrupt)", true),
        ("Working (1h 00m 00s • esc to interrupt)", true),
        ("Working (2h 03m 09s • esc to interrupt)", true),
        ("Working (1m 00s • esc to interrupt) · context", true),
        ("Working (1m 60s • esc to interrupt)", false),
        ("Working (1h 60m 00s • esc to interrupt)", false),
        ("Working (0s • esc to", false),
        ("Working", false),
        ("Working (1m 00s • esc to interrupt) context", false),
    ];

    for (row17, matches) in cases {
        let bytes = format!("\x1b[18;1H{row17}").into_bytes();
        let snapshot = i2879_snapshot(&[&bytes]);
        assert_eq!(snapshot.rows[17], row17);
        let expected: Vec<(usize, usize)> = if matches { vec![(17, 17)] } else { vec![] };
        assert_eq!(i2879_matches(&snapshot), expected, "{row17}");
    }
}

/// `(start, end)` of every logical row the PRODUCTION Codex Working pattern matches.
fn i2879_production_matches(snapshot: &I2879Snapshot) -> Vec<(usize, usize)> {
    let pattern =
        super::super::pattern::compile(super::super::CODEX_WORKING_PATTERN).expect("compiles");
    snapshot
        .logical
        .iter()
        .filter(|row| pattern.regex().is_match(&row.text))
        .map(|row| (row.start, row.end))
        .collect()
}

/// #2879 - both activity-glyph prefixes, styled, with context, wrapped and at the top
/// edge, through ANSI -> vt100 -> `logical_rows` -> the production pattern, at every
/// split point (CSI and UTF-8 cut anywhere). The row-0 wrap stays skipped: a known limit.
#[test]
fn i2879_feature_ansi_prefix() {
    let spaces = " ".repeat(71);
    let wrapped_indicator = format!("\x1b[17;1H{spaces}• Working (3s • esc to interrupt)");
    let top_edge = format!(
        "\x1b[1;1H{spaces}• Working (3s • esc to interrupt)\
        \x1b[18;1H• Working (4s • esc to interrupt)\
        \x1b[21;3H"
    );
    // (name, bytes, row 17 text, matches)
    let cases: [I2879PrefixCase; 2] = [
        (
            "bullet-styled",
            include_bytes!("tests/fixtures/ansi-6.ans"),
            "• Working (12s • esc to interrupt)",
            vec![(17, 17)],
        ),
        (
            "hollow-context",
            include_bytes!("tests/fixtures/ansi-7.ans"),
            "◦ Working (1m 05s • esc to interrupt) · Running hook",
            vec![(17, 17)],
        ),
    ];
    for (name, bytes, row17, matches) in cases {
        let snapshot = i2879_snapshot_every_split(name, bytes);
        assert_eq!(snapshot.rows[17], row17, "{name}: row 17");
        assert_eq!(
            i2879_production_matches(&snapshot),
            matches,
            "{name}: matches"
        );
        // The control: the same row with its hint truncated must not match.
        let cut = format!("\x1b[18;1H{}", &row17[..row17.find("interrupt").unwrap()]);
        assert!(
            i2879_production_matches(&i2879_snapshot(&[cut.as_bytes()])).is_empty(),
            "{name}: truncated hint"
        );
    }

    let name = "bullet-wrapped";
    let snapshot = i2879_snapshot_every_split(name, wrapped_indicator.as_bytes());
    assert_eq!(
        snapshot.rows[16],
        format!("{spaces}• Working"),
        "{name}: row 16"
    );
    assert_eq!(
        snapshot.rows[17], " (3s • esc to interrupt)",
        "{name}: row 17"
    );
    assert!(snapshot.wrapped[16], "{name}: row 16 wraps into row 17");
    let logical16 = snapshot
        .logical
        .iter()
        .find(|row| row.start == 16)
        .expect("row 16 starts a logical row");
    assert_eq!(
        (logical16.end, logical16.text.as_str()),
        (
            17,
            format!("{spaces}• Working (3s • esc to interrupt)").as_str()
        ),
        "{name}"
    );
    assert_eq!(
        i2879_production_matches(&snapshot),
        vec![(16, 17)],
        "{name}"
    );

    let name = "top-edge";
    let snapshot = i2879_snapshot_every_split(name, top_edge.as_bytes());
    assert!(snapshot.wrapped[0], "{name}: row 0 wraps into row 1");
    assert_eq!(
        snapshot.rows[1], " (3s • esc to interrupt)",
        "{name}: row 1"
    );
    assert!(
        snapshot.logical.iter().all(|row| row.start > 1),
        "{name}: the wrapped group at row 0 is skipped, as logical_rows documents"
    );
    assert_eq!(
        i2879_production_matches(&snapshot),
        vec![(17, 17)],
        "{name}"
    );
}
