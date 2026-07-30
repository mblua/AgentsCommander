//! #1171 - generic, user-configured regex watchers over the plain de-ANSI'd rows of the
//! `vt100` screen mirror AC already keeps for every session.
//!
//! A sibling of `context_scrape`, not an extension of it. The two engines share only the
//! read boundary on `SessionIoFanout`; `ContextScraper` keeps its 5 s interval, its single
//! pattern per agent and its five sinks, untouched.
//!
//! **The engine cannot act.** Like `ContextScraper` (`context_scrape/mod.rs:5-8`) it holds
//! narrow trait objects and no `AppHandle` and no `PtyManager` of its own, so the worst a
//! hostile or careless pattern can do is put a wrong row in a window and a wrong number in a
//! counter. Injection capability plus a loose pattern would be a feedback loop: inject,
//! printed to the PTY, matches its own injection, inject again.
//!
//! **This is a best-effort indicator and not an audit log.** That is a property of the PTY
//! channel and it is stated in the UI.
//!
//! # Measured cost
//!
//! Taken by `read_seam_timing` below at AC's default 30x120 grid, against a session with a
//! live spinner - `output_sequence` advances on every CHUNK (`output.rs:160`), including
//! chunks that change no character, so a visually still screen is not a quiet one and the
//! unchanged short circuit must not be measured against one. Same form as
//! `context_scrape/mod.rs:22-25`:
//!
//! - `get_screen_rows`: ~81 us
//! - `get_screen_rows_since` on an UNCHANGED frame: ~30 ns
//! - `get_screen_rows_since` on a CHANGED frame: ~82 us
//!
//! So a changed read costs what the existing sample costs, plus the wrap flags and the cursor
//! row, which are O(1) under the guard the row clone already takes. An unchanged read is
//! ~2700x cheaper because it clones nothing, and that is enforced by the TYPE -
//! `ScreenRowsSince::Unchanged` carries no rows - rather than by this measurement.
//!
//! Machine: Windows 11, taken with optimizations on so the numbers are comparable to the
//! ~200 us `context_scrape` records. A plain `cargo test` build is unoptimized and its
//! numbers are not comparable to either; `--release` does not build the test target in this
//! tree, because `load_sessions_raw_from_dir_for_test` is `#[cfg(debug_assertions)]`.

/// Identifies one frame of one session's screen mirror.
///
/// **The size is part of the stamp on purpose.** `resize_screen_and_broadcast` reflows the
/// grid without bumping `output_sequence` (`output.rs:202-212`), so a sequence-only stamp
/// would report `Unchanged` over a screen that was just re-laid at a new width.
///
/// **`sequence` is monotonic only within one parser instance.** `register_session` inserts a
/// fresh parser at `output_sequence: 0` (`output.rs:109-116`). That is safe here only because
/// session ids are minted per spawn and AC never reuses one, not even on respawn
/// (`context_scrape/mod.rs:232-233`). #955's replay tolerates a reset; this engine does not.
/// If a future "reattach in place" ever reuses an id, the stamp would move BACKWARDS and the
/// engine could report `Unchanged` over a completely different screen.
///
/// `output_sequence` itself is never modified by this module: it is also the replay ordering
/// key `get_screen_snapshot` hands the frontend (`output.rs:274-285`, #955), and changing when
/// it advances would change that contract. The seam only reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameStamp {
    /// `ScreenReplayState::output_sequence` (`output.rs:160`).
    pub sequence: u64,
    pub rows: u16,
    pub cols: u16,
}

/// One session's screen, as the engine needs to see it.
///
/// `wrapped` and `cursor_row` are read under the same guard the row clone already takes, at
/// O(1) each. Fetching them separately would mean a second lock acquisition on a possibly
/// different frame.
pub struct ScreenFrame {
    /// One entry per physical row, from `Screen::rows(0, cols)`.
    pub rows: Vec<String>,
    /// `Screen::row_wrapped(i)` for each i: does this physical row continue into the next
    /// one. Same length as `rows`, by construction.
    pub wrapped: Vec<bool>,
    /// `Screen::cursor_position().0`. The row currently being written.
    pub cursor_row: u16,
    /// `None` only from the default trait implementation, which has no sequence to report.
    /// `None` means "treat as changed", so "the default never reports `Unchanged`" falls out
    /// of the type rather than out of a rule someone has to remember.
    pub stamp: Option<FrameStamp>,
}

/// What a read of one session's screen can say.
///
/// The `Missing` and `Gone` split is what preserves the distinction `ScreenRowsRead` argues
/// for at length (`context_scrape/mod.rs:39-45`): "we could not read" must never be confused
/// with "there is nothing here any more".
pub enum ScreenRowsSince {
    /// The stamp matched. NO rows were cloned and no allocation was made.
    Unchanged,
    Frame(ScreenFrame),
    /// No parser for this id. Says NOTHING about whether the session is over, exactly like
    /// `get_screen_rows` returning `None` (`output.rs:287-294`). Keep sampling it.
    Missing,
    /// This backend has no session behind this id. Retire it now.
    Gone,
}

impl ScreenRowsSince {
    /// The frame, when there is one. Convenience for callers that treat every non-frame
    /// outcome the same way.
    pub fn frame(&self) -> Option<&ScreenFrame> {
        match self {
            ScreenRowsSince::Frame(frame) => Some(frame),
            _ => None,
        }
    }
}

/// #1171 - the default `PtyBackend::screen_rows_since`, shared by the trait default so the
/// mapping lives beside the types it maps into.
///
/// Everything that is not rows becomes `Missing`: a backend that only implements
/// `get_screen_rows` has no richer oracle to offer, and `Missing` is the arm that keeps
/// sampling rather than retiring.
pub(crate) fn frame_from_screen_rows_read(
    read: crate::pty::context_scrape::ScreenRowsRead,
) -> ScreenRowsSince {
    match read {
        crate::pty::context_scrape::ScreenRowsRead::Rows(rows) => {
            let wrapped = vec![false; rows.len()];
            ScreenRowsSince::Frame(ScreenFrame {
                rows,
                wrapped,
                cursor_row: 0,
                stamp: None,
            })
        }
        crate::pty::context_scrape::ScreenRowsRead::Unavailable
        | crate::pty::context_scrape::ScreenRowsRead::SessionOver => ScreenRowsSince::Missing,
    }
}

#[cfg(test)]
mod read_seam_tests {
    use super::*;
    use crate::pty::output::{PtyOutputTarget, SessionIoFanout};
    use crate::session::profile::IdleTuning;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;
    use uuid::Uuid;

    fn timing_session_id() -> Uuid {
        Uuid::new_v4()
    }

    fn fanout() -> SessionIoFanout {
        SessionIoFanout::new(
            Arc::new(Mutex::new(HashMap::new())),
            crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {}),
            None,
        )
    }

    fn feed(fanout: &SessionIoFanout, id: Uuid, chunk: &[u8]) {
        fanout.handle_output(&PtyOutputTarget::noop(), id, &id.to_string(), chunk.to_vec());
    }

    /// #1171, section 7.3 - the three numbers recorded in this module's doc comment.
    ///
    /// `#[ignore]`d so it never becomes a flaky timing gate, and kept in the tree so the
    /// numbers can be re-taken rather than guessed at again:
    ///
    /// ```text
    /// cargo test --config profile.dev.opt-level=2 --lib read_seam_timing -- --ignored --nocapture
    /// ```
    ///
    /// The changed-frame measurement feeds a chunk between reads, which is what a session
    /// with a live spinner does several times per second.
    #[test]
    #[ignore]
    fn read_seam_timing() {
        const ITERATIONS: u32 = 2_000;

        let fanout = fanout();
        let id = timing_session_id();
        fanout.register_session(id, IdleTuning::DEFAULT, 30, 120);
        for row in 0..30u16 {
            feed(
                &fanout,
                id,
                format!("\x1b[{};1Hrow {row} of a coding agent's screen, wide enough to be real\r\n", row + 1)
                    .as_bytes(),
            );
        }

        let start = Instant::now();
        for _ in 0..ITERATIONS {
            std::hint::black_box(fanout.get_screen_rows(id));
        }
        let full = start.elapsed() / ITERATIONS;

        let seen = match fanout.get_screen_rows_since(id, None) {
            ScreenRowsSince::Frame(frame) => frame.stamp,
            other => panic!(
                "expected a frame, got {}",
                match other {
                    ScreenRowsSince::Unchanged => "Unchanged",
                    ScreenRowsSince::Missing => "Missing",
                    ScreenRowsSince::Gone => "Gone",
                    ScreenRowsSince::Frame(_) => unreachable!(),
                }
            ),
        };

        let start = Instant::now();
        for _ in 0..ITERATIONS {
            std::hint::black_box(fanout.get_screen_rows_since(id, seen));
        }
        let unchanged = start.elapsed() / ITERATIONS;

        // The chunk that makes the frame change is fed OUTSIDE the timed region: what is
        // being measured is the read, not `handle_output`.
        let mut changed_total = std::time::Duration::ZERO;
        for i in 0..ITERATIONS {
            feed(&fanout, id, format!("\x1b[30;1Hspinner {i}").as_bytes());
            let start = Instant::now();
            std::hint::black_box(fanout.get_screen_rows_since(id, seen));
            changed_total += start.elapsed();
        }
        let changed = changed_total / ITERATIONS;

        println!("[#1171] get_screen_rows:                 {full:?}");
        println!("[#1171] get_screen_rows_since UNCHANGED: {unchanged:?}");
        println!("[#1171] get_screen_rows_since CHANGED:   {changed:?}");
    }

    /// 9.1.1 - an unchanged frame short-circuits, and the variant it returns cannot carry
    /// rows even if someone later wanted it to. This is the acceptance criterion for
    /// contention, enforced by the type rather than by a timing assertion (7.3).
    #[test]
    fn an_unchanged_frame_returns_unchanged_and_carries_no_rows() {
        let fanout = fanout();
        let id = timing_session_id();
        fanout.register_session(id, IdleTuning::DEFAULT, 30, 120);
        feed(&fanout, id, b"hello");

        let seen = fanout
            .get_screen_rows_since(id, None)
            .frame()
            .and_then(|frame| frame.stamp)
            .expect("first read must be a frame carrying a stamp");

        let again = fanout.get_screen_rows_since(id, Some(seen));
        assert!(matches!(again, ScreenRowsSince::Unchanged));
        assert!(
            again.frame().is_none(),
            "Unchanged must carry no frame and therefore no rows"
        );
    }

    /// 9.1.2 - one `handle_output` chunk moves `sequence`, so the next read is a frame.
    #[test]
    fn one_output_chunk_makes_the_next_read_a_frame() {
        let fanout = fanout();
        let id = timing_session_id();
        fanout.register_session(id, IdleTuning::DEFAULT, 30, 120);
        feed(&fanout, id, b"first");

        let seen = fanout
            .get_screen_rows_since(id, None)
            .frame()
            .and_then(|frame| frame.stamp)
            .expect("first read must be a frame");
        assert!(matches!(
            fanout.get_screen_rows_since(id, Some(seen)),
            ScreenRowsSince::Unchanged
        ));

        feed(&fanout, id, b" second");
        let next = fanout.get_screen_rows_since(id, Some(seen));
        let frame = next.frame().expect("a new chunk must produce a frame");
        assert_eq!(frame.stamp.unwrap().sequence, seen.sequence + 1);
    }

    /// 9.1.3 - **the regression a sequence-only stamp would let through.**
    ///
    /// `resize_screen_and_broadcast` reflows the grid and does NOT bump `output_sequence`
    /// (`output.rs:202-212`). With the size out of the stamp this read would return
    /// `Unchanged` over a screen that was just re-laid at a different width.
    #[test]
    fn a_resize_that_does_not_move_the_sequence_still_returns_a_frame() {
        let fanout = fanout();
        let id = timing_session_id();
        fanout.register_session(id, IdleTuning::DEFAULT, 30, 120);
        feed(&fanout, id, b"content");

        let seen = fanout
            .get_screen_rows_since(id, None)
            .frame()
            .and_then(|frame| frame.stamp)
            .expect("first read must be a frame");

        fanout.resize_screen_and_broadcast(id, 100, 24);

        let after = fanout.get_screen_rows_since(id, Some(seen));
        let frame = after
            .frame()
            .expect("a reflow must not be reported as Unchanged");
        let stamp = frame.stamp.unwrap();
        assert_eq!(
            stamp.sequence, seen.sequence,
            "the resize must not have moved the sequence, or this test proves nothing"
        );
        assert_eq!((stamp.rows, stamp.cols), (24, 100));
    }

    /// 9.1.5 - a poisoned `screen_parsers` is `Missing`, never `Unchanged`. Reporting
    /// `Unchanged` would claim the screen is the one the caller last saw, which is precisely
    /// what a lock we could not take cannot say.
    #[test]
    fn a_poisoned_parser_map_is_missing_and_not_unchanged() {
        let fanout = fanout();
        let id = timing_session_id();
        fanout.register_session(id, IdleTuning::DEFAULT, 30, 120);
        feed(&fanout, id, b"content");

        let seen = fanout
            .get_screen_rows_since(id, None)
            .frame()
            .and_then(|frame| frame.stamp)
            .expect("first read must be a frame");

        fanout.poison_screen_parsers_for_test();

        assert!(matches!(
            fanout.get_screen_rows_since(id, Some(seen)),
            ScreenRowsSince::Missing
        ));
    }

    /// 9.1.6 - the changed-frame read returns exactly what `get_screen_rows` returns, row
    /// for row. The seam is a cheaper way to ask the same question, not a different one.
    #[test]
    fn a_changed_frame_returns_the_same_rows_as_get_screen_rows() {
        let fanout = fanout();
        let id = timing_session_id();
        fanout.register_session(id, IdleTuning::DEFAULT, 30, 120);
        feed(
            &fanout,
            id,
            b"alpha\r\nbeta\r\ngamma with rather more text on it\r\n",
        );

        let expected = fanout.get_screen_rows(id).expect("rows");
        let frame = fanout
            .get_screen_rows_since(id, None)
            .frame()
            .map(|frame| frame.rows.clone())
            .expect("frame");
        assert_eq!(frame, expected);
    }

    /// 9.1.7 - `wrapped` and `cursor_row` are the parser's, for every row, and `wrapped` is
    /// the same length as `rows` so the frame diff can index them together without a bound
    /// check of its own.
    #[test]
    fn wrapped_and_cursor_row_mirror_the_parser() {
        let fanout = fanout();
        let id = timing_session_id();
        fanout.register_session(id, IdleTuning::DEFAULT, 4, 10);
        // 14 chars into a 10-column grid: row 0 wraps into row 1.
        feed(&fanout, id, b"0123456789abcd");

        let read = fanout.get_screen_rows_since(id, None);
        let frame = read.frame().expect("frame");
        assert_eq!(frame.rows.len(), frame.wrapped.len());
        assert_eq!(frame.wrapped, vec![true, false, false, false]);
        assert_eq!(frame.cursor_row, 1);
    }

    /// A session the fanout never registered is `Missing` at the fanout boundary: the fanout
    /// knows nothing about children, so it can make no claim about the session (`output.rs:287-294`).
    #[test]
    fn an_unregistered_session_is_missing_at_the_fanout() {
        let fanout = fanout();
        assert!(matches!(
            fanout.get_screen_rows_since(timing_session_id(), None),
            ScreenRowsSince::Missing
        ));
    }

    /// 9.1.8 - the default trait implementation never reports `Unchanged` and never invents a
    /// stamp, whatever it is handed as `seen`.
    #[test]
    fn the_default_mapping_never_reports_unchanged_and_reports_no_stamp() {
        use crate::pty::context_scrape::ScreenRowsRead;

        let mapped = frame_from_screen_rows_read(ScreenRowsRead::Rows(vec![
            "one".to_string(),
            "two".to_string(),
        ]));
        let frame = mapped.frame().expect("rows must map to a frame");
        assert!(frame.stamp.is_none());
        assert_eq!(frame.wrapped, vec![false, false]);
        assert_eq!(frame.cursor_row, 0);

        assert!(matches!(
            frame_from_screen_rows_read(ScreenRowsRead::Unavailable),
            ScreenRowsSince::Missing
        ));
        assert!(matches!(
            frame_from_screen_rows_read(ScreenRowsRead::SessionOver),
            ScreenRowsSince::Missing
        ));
    }
}
