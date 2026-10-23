use super::*;

#[test]
fn issue_3029_cursor_position_respects_reader_token() {
    let fanout = fanout();
    let id = Uuid::new_v4();
    let token = fanout
        .register_session_for_test(id, IdleTuning::DEFAULT, 24, 80)
        .unwrap();
    assert_eq!(fanout.cursor_position_for_reader(&token), Some((1, 1)));
    fanout.handle_output(&token, &id.to_string(), b"\x1b[3;4Hhi".to_vec());
    let before = fanout.get_screen_snapshot(id).unwrap();
    let history = fanout.screen_parsers.lock().unwrap()[&id].history.clone();
    assert_eq!(fanout.cursor_position_for_reader(&token), Some((3, 6)));
    let after = fanout.get_screen_snapshot(id).unwrap();
    assert_eq!((before.sequence, before.data), (after.sequence, after.data));
    assert_eq!(history, fanout.screen_parsers.lock().unwrap()[&id].history);
    let foreign = super::super::tests::fanout();
    foreign
        .register_session_for_test(id, IdleTuning::DEFAULT, 24, 80)
        .unwrap();
    assert_eq!(foreign.cursor_position_for_reader(&token), None);
    fanout.remove_session(id);
    assert_eq!(fanout.cursor_position_for_reader(&token), None);
    let replacement = fanout
        .register_session_for_test(id, IdleTuning::DEFAULT, 24, 80)
        .unwrap();
    assert_eq!(fanout.cursor_position_for_reader(&token), None);
    assert_eq!(
        fanout.cursor_position_for_reader(&replacement),
        Some((1, 1))
    );
    fanout
        .screen_parsers
        .lock()
        .unwrap()
        .get_mut(&id)
        .unwrap()
        .parser_availability = ParserAvailability::Unavailable;
    assert_eq!(fanout.cursor_position_for_reader(&replacement), None);
}

/// A coding agent's TUI on the way up: hide the cursor, clear, switch to the alternate
/// screen, home, set the title, turn on mouse reporting, reset the attributes. Every byte of
/// it is an escape sequence. It paints NOTHING a human can see, and the gate must stay shut
/// through all of it.
///
/// The charset designator a real TUI also emits here (`ESC ( B`) is deliberately NOT in this
/// constant: it fools the old predicate even UNSPLIT, so putting it here would let the
/// chunk-boundary test below pass for the wrong reason. It gets its own test.
const TUI_PROLOGUE: &[u8] =
    b"\x1b[?25l\x1b[2J\x1b[?1049h\x1b[H\x1b]0;cmd.exe\x07\x1b[?1000h\x1b[?1006h\x1b[m";

/// #973 (B), (a) - CHUNK BOUNDARIES. The trigger must survive the prologue arriving in two
/// reads, split anywhere.
///
/// The old predicate could not. `strip_ansi_csi` is stateless and keeps no residue between
/// calls, but the read loop feeds it raw `read()` chunks, and conhost really does split its
/// output across writes. A chunk ending mid-CSI or mid-OSC hands the tail to the next call
/// with NO leading `ESC`, and `1049h`, `2J` and `cmd.exe` are all printable - so the gate
/// opened on a child that had painted nothing, which is the bug the gate exists to prevent.
///
/// It is not an edge case. **43 of this prologue's 51 interior split points** fool the old
/// predicate: a boundary inside a CSI leaves the tail with no `ESC`, and the tail of nearly
/// every sequence here is printable - `l`, `2J`, `1049h`, `m`, and `cmd.exe` out of the OSC
/// title. Only a boundary that happens to land ON an `ESC` is safe.
///
/// This asserts the hazard is REAL first (some split does fool the old predicate), or the
/// rest of the test would prove nothing, and then walks every interior split point through
/// the new trigger.
#[test]
fn the_gate_holds_a_prologue_split_at_any_chunk_boundary() {
    let fanout = fanout();

    // whole, in one chunk, the old predicate gets right
    assert!(
        !output_has_printable_activity(&String::from_utf8_lossy(TUI_PROLOGUE)),
        "the prologue paints nothing, and unsplit even the old predicate saw that"
    );

    // split, it does not. These are the boundaries that would have opened the gate.
    let fooled: Vec<usize> = (1..TUI_PROLOGUE.len())
        .filter(|&at| {
            let (head, tail) = TUI_PROLOGUE.split_at(at);
            output_has_printable_activity(&String::from_utf8_lossy(head))
                || output_has_printable_activity(&String::from_utf8_lossy(tail))
        })
        .collect();
    assert!(
        !fooled.is_empty(),
        "if no split fools the old predicate, this test is not testing the hazard"
    );

    // the new trigger, at every one of them
    for at in 1..TUI_PROLOGUE.len() {
        let id = session(&fanout);
        let (head, tail) = TUI_PROLOGUE.split_at(at);
        feed(&fanout, id, &[head, tail]);
        assert!(
            !fanout.has_rendered_visible_content(id),
            "split at {at}: the child has painted nothing, so the gate must stay shut"
        );
    }

    // ...and it is a gate, not a wall: the moment the child paints, it opens
    let id = session(&fanout);
    feed(&fanout, id, &[TUI_PROLOGUE]);
    assert!(
        !fanout.has_rendered_visible_content(id),
        "still nothing to see"
    );
    feed(&fanout, id, &[b"> "]);
    assert!(
        fanout.has_rendered_visible_content(id),
        "a glyph the user can see must open the gate"
    );
}

/// #973 (B), (b) - THREE-BYTE ESCAPES. `ESC ( B` is what ncurses and half the TUI world emit
/// on the way up. The old stripper consumed `ESC` plus exactly ONE char, so the `B` survived
/// and read as printable, and the gate opened on a blank screen. Same for `ESC ) 0` (leaks
/// `0`) and `ESC % G` (leaks `G`).
///
/// `ESC # 8` (DECALN) is deliberately not asserted here. It is the fourth three-byte escape
/// and it fools the old stripper too (leaks `8`), but it is the one that genuinely PAINTS: a
/// real terminal fills the screen with `E`. vt100 does not implement it, so our parser shows
/// nothing and this gate would stay shut - which is a limitation of the crate, not a property
/// worth pinning as correct. Pinning it would freeze the wrong answer.
#[test]
fn a_three_byte_charset_designator_does_not_open_the_gate() {
    assert!(
        output_has_printable_activity("\x1b(B"),
        "the old predicate really is fooled by this - that is the whole defect"
    );

    let fanout = fanout();
    let id = session(&fanout);
    feed(&fanout, id, &[b"\x1b(B", b"\x1b)0", b"\x1b%G"]);

    assert!(
        !fanout.has_rendered_visible_content(id),
        "a charset designator paints nothing: the gate must stay shut"
    );
}

/// What "a human can see" means, pinned. A space is not content, even though the cell was
/// written and `Cell::has_contents` says true. A COLOURED space is - TUIs draw status bars
/// and boxes with nothing else, and `Cell::clear` keeps the attributes, so a cleared screen
/// can be solid red while holding no contents at all.
#[test]
fn a_blank_viewport_is_not_content_but_a_coloured_one_is() {
    let fanout = fanout();

    // the TUI paints its still-empty viewport: spaces, and plenty of them. This is the exact
    // moment the gate must stay shut - it is inside the danger window.
    let id = session(&fanout);
    feed(&fanout, id, &[b"\x1b[2J\x1b[H", &[b' '; 240]]);
    assert!(
        !fanout.has_rendered_visible_content(id),
        "a viewport of spaces is still a blank viewport"
    );
    feed(&fanout, id, &[b"ready"]);
    assert!(
        fanout.has_rendered_visible_content(id),
        "a glyph is content"
    );

    // a red status bar: no glyph anywhere, and a human plainly sees it
    let id = session(&fanout);
    feed(&fanout, id, &[b"\x1b[41m", &[b' '; 20]]);
    assert!(
        fanout.has_rendered_visible_content(id),
        "a coloured space is content: it is how a TUI draws a status bar"
    );

    // reverse video does it with the default colours
    let id = session(&fanout);
    feed(&fanout, id, &[b"\x1b[7m", &[b' '; 20]]);
    assert!(
        fanout.has_rendered_visible_content(id),
        "an inverse space renders as a solid block"
    );
}

/// #973 / #955 - a degenerate size must never reach the vt100 parser.
///
/// Red before the guard, and not with an assertion - with a **panic**:
///
/// ```text
/// panicked at vt100-0.15.2/src/grid.rs:74:34: attempt to subtract with overflow
/// ```
///
/// `vt100::grid::set_size` computes `size.rows - 1` on a `u16`, so `rows == 0` underflows.
/// Debug panics; release wraps to 65535. Worse, the panic fires while
/// `resize_screen_and_broadcast` holds `screen_parsers`, poisoning it - and every reader of
/// that mutex swallows the poison silently (`if let Ok` / `.ok()?`), so #955's snapshot,
/// the output sequence numbering and `get_pty_size` would go dead for EVERY session, for
/// the life of the process, without a log line. A black tile on re-attach is what the user
/// would see: exactly the bug #955 shipped to kill.
#[test]
fn a_degenerate_resize_never_reaches_the_vt100_parser() {
    let fanout = fanout();
    let id = Uuid::new_v4();
    fanout
        .register_session_for_test(id, IdleTuning::DEFAULT, 30, 120)
        .expect("register session");
    let token = fanout.registration_token_for_test(id);
    fanout.handle_output(&token, &id.to_string(), b"hello".to_vec());

    fanout.resize_screen_and_broadcast(id, 0, 0);

    let after = fanout.get_screen_snapshot(id).expect("snapshot");
    assert_eq!(
        (after.rows, after.cols),
        (30, 120),
        "the screen must be untouched by a size the child was never given"
    );
    assert!(
        String::from_utf8_lossy(&after.data).contains("hello"),
        "and the child's output must still be in it: an empty snapshot is #955's black tile"
    );

    // a real size still moves the screen - the guard refuses the degenerate, not the resize
    fanout.resize_screen_and_broadcast(id, 80, 24);
    let resized = fanout.get_screen_snapshot(id).expect("snapshot");
    assert_eq!((resized.rows, resized.cols), (24, 80));
}

#[test]
fn printable_activity_ignores_ansi_only_chunks() {
    assert!(!output_has_printable_activity("\x1b[31m\x1b[0m"));
    assert!(!output_has_printable_activity("\x1b]0;title\x07"));
    assert!(output_has_printable_activity("\x1b[31mready\x1b[0m"));
}
