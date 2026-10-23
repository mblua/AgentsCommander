use super::*;

fn mirrored_screen(fanout: &SessionIoFanout, id: Uuid) -> Vec<u8> {
    let parsers = fanout.screen_parsers.lock().expect("parser state");
    parsers
        .get(&id)
        .expect("registered session")
        .parser
        .screen()
        .contents_formatted()
}

/// The ring stays under its limit, keeps the reserved capacity it was built with, and
/// starts on a line boundary. The capacity assert is the point: `len` is bounded by the
/// trim under every reserve variant, so without it this test passes just as happily with
/// a ring that quietly grew to twice its ceiling.
///
/// The oversized chunk covers the one case where the trim arithmetic gets written as a
/// plain subtraction. That panic would not be local: the caller flips the parser to
/// `Unavailable`, which leaves the console dead for the rest of the process.
///
/// The line length is load bearing and must stay a non divisor of the limit. With lines
/// of 128 bytes against a 65 536 byte ceiling the space trim lands on a line boundary by
/// arithmetic alone, so the front assert passes just as happily with the line-boundary
/// trim deleted: verified by mutation, both variants passed.
#[test]
fn history_ring_is_bounded_and_line_aligned() {
    let fanout = fanout();
    let id = session(&fanout);
    let mut line = [b'-'; 100];
    line[0] = b'>';
    line[99] = b'\n';
    let chunk: Vec<u8> = line.repeat(3);
    for _ in 0..512 {
        feed(&fanout, id, &[&chunk]);
    }
    {
        let parsers = fanout.screen_parsers.lock().expect("parser state");
        let state = parsers.get(&id).expect("registered session");
        assert!(state.history.len() <= UI_HISTORY_LIMIT_BYTES);
        assert_eq!(state.history.capacity(), UI_HISTORY_LIMIT_BYTES);
        assert_eq!(state.history.front().copied(), Some(b'>'));
    }

    let oversized = vec![b'x'; UI_HISTORY_LIMIT_BYTES + 4_096];
    feed(&fanout, id, &[&oversized]);
    let parsers = fanout.screen_parsers.lock().expect("parser state");
    let state = parsers.get(&id).expect("registered session");
    assert_eq!(state.history.len(), UI_HISTORY_LIMIT_BYTES);
    assert_eq!(state.history.capacity(), UI_HISTORY_LIMIT_BYTES);
    assert_eq!(state.parser_availability, ParserAvailability::Available);
}

/// #1458. A ring saturated by a newline-free stream (a coding agent's spinner rewriting one
/// line with `\r`) leaves the ring's front at an arbitrary byte offset, which lands inside an
/// escape sequence most of the time. The seed must never emit that sequence's literal tail;
/// with no `\n` anywhere in the ring there is nothing to realign to, so the attach takes the
/// parser mirror.
#[test]
fn a_newline_free_ring_seeds_the_mirror_instead_of_a_partial_sequence() {
    let fanout = fanout();
    let id = session(&fanout);
    // A realistic spinner frame: truecolor SGR, label, carriage return. No `\n`. 33 bytes, so
    // the steady-state front lands 2 bytes into the SGR, exactly where the incident cut.
    let frame = b"\x1b[38;2;153;153;153m* Drizzling..\r";
    for _ in 0..(UI_HISTORY_LIMIT_BYTES / frame.len() + 64) {
        feed(&fanout, id, &[frame]);
    }
    {
        // The precondition of the defect, asserted rather than assumed.
        let parsers = fanout.screen_parsers.lock().expect("parser state");
        let state = parsers.get(&id).expect("registered session");
        assert_eq!(state.history.len(), UI_HISTORY_LIMIT_BYTES);
        assert!(!state.history_aligned);
    }

    let expected = mirrored_screen(&fanout, id);
    let data = activation_data(&fanout, id, true);

    assert!(!data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
    assert_eq!(data, expected);
}

/// #1458. The healthy path must stay byte identical: when the capped trim did realign the
/// ring, the seed is the ring verbatim, from its very first byte. Asserting the whole body
/// against the ring is the point. An alignment scan applied unconditionally would silently
/// drop the ring's first line here, and a `starts_with` on the line's prefix would still pass,
/// because every line of such a replay begins with the same SGR bytes.
#[test]
fn a_line_aligned_ring_still_seeds_the_whole_ring() {
    let fanout = fanout();
    let id = session(&fanout);
    // 102 bytes per line, not a divisor of the 65 536 byte ring, so the space trim lands off a
    // line boundary and the realignment has to do real work: 50 bytes drained for space and 52
    // more to realign, on every overflow.
    for index in 0..2_000 {
        feed(
            &fanout,
            id,
            &[format!("\x1b[38;2;153;153;153m>{index:081}\n").as_bytes()],
        );
    }
    let expected = {
        let parsers = fanout.screen_parsers.lock().expect("parser state");
        let state = parsers.get(&id).expect("registered session");
        assert!(state.history_aligned);
        let (front, back) = state.history.as_slices();
        [front, back].concat()
    };

    let data = activation_data(&fanout, id, true);

    assert!(data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
    assert_eq!(
        &data[UI_HISTORY_REPLAY_PROLOGUE.len()..],
        expected.as_slice()
    );
}

/// #1458 edge case. A ring whose only `\n` is its last byte does have a line start, but has
/// nothing after it: aligning to it would seed the prologue and zero bytes of content, which
/// blanks the attaching terminal. That case must take the mirror, exactly like a ring with no
/// `\n` at all.
#[test]
fn a_ring_whose_only_newline_is_its_last_byte_seeds_the_mirror() {
    let fanout = fanout();
    let id = session(&fanout);
    let frame = b"\x1b[38;2;153;153;153m* Drizzling..\r";
    for _ in 0..(UI_HISTORY_LIMIT_BYTES / frame.len() + 64) {
        feed(&fanout, id, &[frame]);
    }
    // One frame that ends in the ring's only newline. The capped trim scan still sees no `\n`
    // in the first 4 KiB, so the ring stays flagged unaligned.
    feed(&fanout, id, &[b"\x1b[38;2;153;153;153m* Drizzling..\n"]);
    {
        let parsers = fanout.screen_parsers.lock().expect("parser state");
        let state = parsers.get(&id).expect("registered session");
        assert!(!state.history_aligned);
        assert_eq!(state.history.back().copied(), Some(b'\n'));
        assert_eq!(
            state.history.iter().filter(|byte| **byte == b'\n').count(),
            1
        );
    }

    let expected = mirrored_screen(&fanout, id);
    let data = activation_data(&fanout, id, true);

    assert!(!data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
    assert_eq!(data, expected);
}

/// #1458. The recovering case, and the only one that exercises `history_from_first_line`'s
/// `Some` arm: an unaligned ring that still holds lines must seed from the byte after its
/// first `\n`, not fall back to the mirror. Asserting the whole body is the point; a stub
/// helper that always returns `None` passes every other test in this file.
#[test]
fn an_unaligned_ring_with_a_later_newline_seeds_from_that_line() {
    let fanout = fanout();
    let id = session(&fanout);
    let frame = b"\x1b[38;2;153;153;153m* Drizzling..\r"; // 33 bytes, no `\n`
    for _ in 0..(UI_HISTORY_LIMIT_BYTES / frame.len() + 64) {
        feed(&fanout, id, &[frame]);
    }
    for index in 0..40 {
        feed(
            &fanout,
            id,
            &[format!("\x1b[38;2;153;153;153mrecovered line {index:03}\n").as_bytes()],
        );
    }
    let expected = {
        let parsers = fanout.screen_parsers.lock().expect("parser state");
        let state = parsers.get(&id).expect("registered session");
        // The 4 KiB hot scan still sees only spinner at the front, so the flag stays false
        // even though the ring now holds 40 newlines further in.
        assert!(!state.history_aligned);
        let (front, back) = state.history.as_slices();
        let ring = [front, back].concat();
        let newline = ring
            .iter()
            .position(|byte| *byte == b'\n')
            .expect("a newline");
        ring[newline + 1..].to_vec()
    };

    let data = activation_data(&fanout, id, true);

    assert!(data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
    assert_eq!(
        &data[UI_HISTORY_REPLAY_PROLOGUE.len()..],
        expected.as_slice()
    );
}

/// #1458. The helper's four decided branches, pinned without a fixture because which half of
/// the ring holds the first `\n` is a `VecDeque` layout detail no fanout test can choose.
/// Covers 7.2 rows 8, 9 and 10 and the both-halves emptiness check.
#[test]
fn history_from_first_line_decides_every_branch() {
    // A newline in `front`, content behind it: seed from the byte after it, keep all of `back`.
    assert_eq!(
        history_from_first_line(b"ab\ncd", b""),
        Some((&b"cd"[..], &b""[..]))
    );
    // The newline is `front`'s last byte but `back` is not empty: still a normal seed. The
    // emptiness check is on BOTH halves for exactly this row.
    assert_eq!(
        history_from_first_line(b"ab\n", b"cd"),
        Some((&b""[..], &b"cd"[..]))
    );
    // No newline in `front`, one in `back` with content behind it: the whole of `front` is
    // unreplayable and is dropped. Deleting the second scan makes this row dead.
    assert_eq!(
        history_from_first_line(b"ab", b"cd\nef"),
        Some((&b""[..], &b"ef"[..]))
    );
    // The ring's only newline is its last byte: nothing survives it, so the caller must take
    // the mirror rather than seed a prologue and zero bytes.
    assert_eq!(history_from_first_line(b"ab", b"cd\n"), None);
    // No newline anywhere: the reported incident.
    assert_eq!(history_from_first_line(b"ab", b"cd"), None);
}

/// The regression the issue reports: a session that produced output while another one was
/// selected must come back with history, not with a single viewport.
#[test]
fn activation_payload_replays_history_for_background_session() {
    let fanout = fanout();
    let foreground = session(&fanout);
    let background = session(&fanout);
    activation_data(&fanout, foreground, false);

    for index in 0..200 {
        let line = format!("history line {index}\r\n");
        feed(&fanout, background, &[line.as_bytes()]);
    }

    // The first line scrolled out of the 30 row mirror, so it is only reachable through
    // the ring. Without this the payload assert could pass on the viewport alone.
    let mirror = String::from_utf8_lossy(&mirrored_screen(&fanout, background)).into_owned();
    assert!(!mirror.contains("history line 0\r\n"));

    let data = activation_data(&fanout, background, true);
    // Asserting the prologue keeps a silent fallback to `contents_formatted()` from
    // passing this test for the wrong reason.
    assert!(data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
    let replayed = String::from_utf8_lossy(&data);
    assert!(replayed.contains("history line 0\r\n"));
    assert!(replayed.contains("history line 199"));
}

#[test]
fn activation_payload_falls_back_to_screen_when_history_empty() {
    let fanout = fanout();
    let id = session(&fanout);
    let expected = mirrored_screen(&fanout, id);

    let data = activation_data(&fanout, id, true);
    assert_eq!(data, expected);
    assert!(!data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
}

/// Pins the parameter's contract: `false` is the mirrored viewport, never the ring. The
/// frontend asks for the ring on every attach and applies it after a reset, which is what
/// makes the replay safe there, but the viewport-only read stays addressable.
#[test]
fn activation_payload_ignores_history_when_not_requested() {
    let fanout = fanout();
    let id = session(&fanout);
    for index in 0..200 {
        let line = format!("history line {index}\r\n");
        feed(&fanout, id, &[line.as_bytes()]);
    }
    let expected = mirrored_screen(&fanout, id);

    let data = activation_data(&fanout, id, false);
    assert_eq!(data, expected);
    assert!(!data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
}
