use super::*;
use crate::pty::output::SessionIoFanout;
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
    let token = fanout
        .registration_token_for_session(id)
        .expect("registered watcher fixture");
    fanout.handle_output(&token, &id.to_string(), chunk.to_vec());
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
    fanout
        .register_session_for_test(id, IdleTuning::DEFAULT, 30, 120)
        .expect("register timing test session");
    for row in 0..30u16 {
        feed(
            &fanout,
            id,
            format!(
                "\x1b[{};1Hrow {row} of a coding agent's screen, wide enough to be real\r\n",
                row + 1
            )
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
    fanout
        .register_session_for_test(id, IdleTuning::DEFAULT, 30, 120)
        .expect("register timing test session");
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
    fanout
        .register_session_for_test(id, IdleTuning::DEFAULT, 30, 120)
        .expect("register timing test session");
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
    fanout
        .register_session_for_test(id, IdleTuning::DEFAULT, 30, 120)
        .expect("register timing test session");
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
    fanout
        .register_session_for_test(id, IdleTuning::DEFAULT, 30, 120)
        .expect("register timing test session");
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
    fanout
        .register_session_for_test(id, IdleTuning::DEFAULT, 30, 120)
        .expect("register timing test session");
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
    fanout
        .register_session_for_test(id, IdleTuning::DEFAULT, 4, 10)
        .expect("register timing test session");
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
