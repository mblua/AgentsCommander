use super::*;

fn tracker() -> SubstantiveInputTracker {
    SubstantiveInputTracker::default()
}

/// #1682 - the chunk classes the reviewer proved were refreshing the age of
/// an abandoned line. Every one reaches `feed` in production: xterm.js
/// delivers device replies and mouse reports through the same `onData` as
/// keystrokes, and `pty_write` classifies that stream unconditionally.
const NON_CONTRIBUTING_CHUNKS: &[(&str, &[u8])] = &[
    ("arrow key", b"\x1b[A"),
    ("focus report (DECSET 1004)", b"\x1b[I"),
    ("DSR cursor-position reply", b"\x1b[24;80R"),
    ("primary device-attributes reply", b"\x1b[?62;1;6;9;15;22c"),
    ("OSC 11 colour reply", b"\x1b]11;rgb:1234/5678/9abc\x07"),
    ("SGR mouse-move report", b"\x1b[<35;80;24M"),
];

/// One row of the #871 classification matrix.
struct MatrixRow {
    label: &'static str,
    chunk: &'static [u8],
    /// (`*pending` after the chunk, `submitted`) starting from `pending == false`.
    from_clear: (bool, bool),
    /// (`*pending` after the chunk, `submitted`) starting from `pending == true`.
    from_pending: (bool, bool),
    /// #1682's added observation. Not part of #871's contract.
    contributed: bool,
}

/// #1682 regression guard on #871. `classify_chunk` is #871's classifier and
/// #1682 is only a consumer of it, so this pins what the classifier does to
/// `pending_nonspace` and to `submitted` for every chunk class the fix
/// touches, from BOTH starting states, independently of the new field.
#[test]
fn classify_chunk_matrix_pins_871_pending_and_submitted() {
    let rows: &[MatrixRow] = &[
        MatrixRow {
            label: "printable run",
            chunk: b"hello",
            from_clear: (true, false),
            from_pending: (true, false),
            contributed: true,
        },
        MatrixRow {
            label: "printable run then CR",
            chunk: b"hello\r",
            from_clear: (false, true),
            from_pending: (false, true),
            contributed: true,
        },
        MatrixRow {
            label: "bare CR",
            chunk: b"\r",
            from_clear: (false, false),
            from_pending: (false, true),
            contributed: false,
        },
        MatrixRow {
            label: "bare LF",
            chunk: b"\n",
            from_clear: (false, false),
            from_pending: (false, true),
            contributed: false,
        },
        MatrixRow {
            label: "space",
            chunk: b" ",
            from_clear: (false, false),
            from_pending: (true, false),
            contributed: false,
        },
        MatrixRow {
            label: "tab",
            chunk: b"\t",
            from_clear: (false, false),
            from_pending: (true, false),
            contributed: false,
        },
        MatrixRow {
            label: "backspace",
            chunk: b"\x08",
            from_clear: (false, false),
            from_pending: (true, false),
            contributed: false,
        },
        MatrixRow {
            label: "DEL",
            chunk: b"\x7f",
            from_clear: (false, false),
            from_pending: (true, false),
            contributed: false,
        },
        MatrixRow {
            label: "Ctrl-C",
            chunk: b"\x03",
            from_clear: (false, false),
            from_pending: (false, false),
            contributed: false,
        },
        MatrixRow {
            label: "Ctrl-U",
            chunk: b"\x15",
            from_clear: (false, false),
            from_pending: (false, false),
            contributed: false,
        },
        MatrixRow {
            label: "Ctrl-D",
            chunk: b"\x04",
            from_clear: (false, false),
            from_pending: (true, false),
            contributed: false,
        },
        MatrixRow {
            label: "SS3 arrow key",
            chunk: b"\x1bOA",
            from_clear: (false, false),
            from_pending: (true, false),
            contributed: false,
        },
        MatrixRow {
            label: "DCS reply",
            chunk: b"\x1bP1$r0m\x1b\\",
            from_clear: (false, false),
            from_pending: (true, false),
            contributed: false,
        },
        MatrixRow {
            label: "bracketed paste",
            chunk: b"\x1b[200~hi\x1b[201~",
            from_clear: (true, false),
            from_pending: (true, false),
            contributed: true,
        },
    ];

    // The six reviewer classes classify exactly like the arrow key: they
    // leave `pending` alone and submit nothing from either starting state.
    let reviewer_rows: Vec<MatrixRow> = NON_CONTRIBUTING_CHUNKS
        .iter()
        .map(|&(label, chunk)| MatrixRow {
            label,
            chunk,
            from_clear: (false, false),
            from_pending: (true, false),
            contributed: false,
        })
        .collect();

    for row in rows.iter().chain(reviewer_rows.iter()) {
        let mut pending = false;
        let effect = classify_chunk(&mut pending, row.chunk);
        assert_eq!(
            (pending, effect.submitted),
            row.from_clear,
            "{}: (pending, submitted) from a clear flag",
            row.label
        );
        assert_eq!(
            effect.contributed, row.contributed,
            "{}: contributed from a clear flag",
            row.label
        );

        let mut pending = true;
        let effect = classify_chunk(&mut pending, row.chunk);
        assert_eq!(
            (pending, effect.submitted),
            row.from_pending,
            "{}: (pending, submitted) from a set flag",
            row.label
        );
        assert_eq!(
            effect.contributed, row.contributed,
            "{}: contributed from a set flag",
            row.label
        );
    }
}

/// #1682 - the property the unconditional refresh made false. Silence-only
/// aging already passed and is not the broken case: the line must age out
/// while the terminal keeps receiving the traffic an agent's own child
/// produces.
#[test]
fn an_abandoned_line_ages_out_under_a_stream_of_non_submit_chunks() {
    let mut t = tracker();
    let id = Uuid::new_v4();
    t.feed(id, b"half a line");
    assert!(t.pending_within(id, USER_WRITE_STAMP_WINDOW));
    t.backdate_pending_for_test(id, USER_WRITE_STAMP_WINDOW + Duration::from_secs(1));
    assert!(
        !t.pending_within(id, USER_WRITE_STAMP_WINDOW),
        "the abandoned line must be aged out before the stream starts"
    );

    let forever = Duration::from_secs(3600);
    for &(label, chunk) in NON_CONTRIBUTING_CHUNKS {
        assert!(
            !t.feed(id, chunk),
            "{}: must not report a submission",
            label
        );
        assert!(
            !t.pending_within(id, USER_WRITE_STAMP_WINDOW),
            "{}: must not refresh a line abandoned before it arrived",
            label
        );
        // #871's flag itself is untouched: it is the AGE that expired, not
        // `pending_nonspace`, which only CR/LF-with-content, Ctrl-C/Ctrl-U
        // and `reset` may clear.
        assert!(
            t.pending_within(id, forever),
            "{}: must leave `pending_nonspace` set",
            label
        );
    }

    // All six in one chunk, as they arrive when a child answers a burst of
    // queries, is still not a refresh.
    let burst: Vec<u8> = NON_CONTRIBUTING_CHUNKS
        .iter()
        .flat_map(|(_, chunk)| chunk.iter().copied())
        .collect();
    assert!(!t.feed(id, &burst));
    assert!(!t.pending_within(id, USER_WRITE_STAMP_WINDOW));
    assert!(t.pending_within(id, forever));
}

/// #1682 - the control for the test above: the fix must not pass by never
/// refreshing. A person still typing keeps the line fresh, on every
/// contributing chunk and not just the one that first set the flag.
#[test]
fn a_person_still_typing_keeps_the_line_fresh() {
    let mut t = tracker();
    let id = Uuid::new_v4();
    t.feed(id, b"hel");
    t.backdate_pending_for_test(id, USER_WRITE_STAMP_WINDOW + Duration::from_secs(1));
    assert!(!t.pending_within(id, USER_WRITE_STAMP_WINDOW));

    // The next keystroke of the same line makes it fresh again.
    t.feed(id, b"l");
    assert!(t.pending_within(id, USER_WRITE_STAMP_WINDOW));

    // And so does the one after it, so a line typed slowly over more than
    // one window never ages out mid-typing.
    t.backdate_pending_for_test(id, USER_WRITE_STAMP_WINDOW + Duration::from_secs(1));
    assert!(!t.pending_within(id, USER_WRITE_STAMP_WINDOW));
    t.feed(id, b"o");
    assert!(t.pending_within(id, USER_WRITE_STAMP_WINDOW));

    // A keystroke buried in a chunk that also carries non-contributing
    // sequences still counts: `contributed` is per chunk, not per byte.
    t.backdate_pending_for_test(id, USER_WRITE_STAMP_WINDOW + Duration::from_secs(1));
    assert!(!t.pending_within(id, USER_WRITE_STAMP_WINDOW));
    t.feed(id, b"\x1b[A!\x1b[I");
    assert!(t.pending_within(id, USER_WRITE_STAMP_WINDOW));
}

#[test]
fn typed_prompt_then_enter_is_substantive() {
    let mut t = tracker();
    let id = Uuid::new_v4();
    assert!(!t.feed(id, b"hello"));
    assert!(t.feed(id, b"\r"));
}

#[test]
fn single_chunk_prompt_is_substantive() {
    let mut t = tracker();
    assert!(t.feed(Uuid::new_v4(), b"hello world\r"));
}

#[test]
fn empty_enter_is_not_substantive() {
    let mut t = tracker();
    assert!(!t.feed(Uuid::new_v4(), b"\r"));
}

#[test]
fn focus_in_out_is_not_substantive() {
    let mut t = tracker();
    let id = Uuid::new_v4();
    assert!(!t.feed(id, b"\x1b[I"));
    assert!(!t.feed(id, b"\x1b[O"));
}

#[test]
fn cursor_keys_are_not_substantive() {
    let mut t = tracker();
    let id = Uuid::new_v4();
    assert!(!t.feed(id, b"\x1b[A\x1b[B\x1b[C\x1b[D"));
}

#[test]
fn dsr_response_is_not_substantive() {
    let mut t = tracker();
    assert!(!t.feed(Uuid::new_v4(), b"\x1b[24;80R"));
}

#[test]
fn bracketed_paste_then_enter_is_substantive() {
    let mut t = tracker();
    assert!(t.feed(Uuid::new_v4(), b"\x1b[200~hi\x1b[201~\r"));
}

#[test]
fn content_spanning_chunks_then_enter_is_substantive() {
    let mut t = tracker();
    let id = Uuid::new_v4();
    assert!(!t.feed(id, b"hel"));
    assert!(!t.feed(id, b"lo"));
    assert!(t.feed(id, b"\r"));
}

#[test]
fn non_ascii_content_is_substantive() {
    let mut t = tracker();
    assert!(t.feed(Uuid::new_v4(), "\u{00e9}\r".as_bytes()));
}

#[test]
fn reset_forgets_pending_content() {
    let mut t = tracker();
    let id = Uuid::new_v4();
    assert!(!t.feed(id, b"hello"));
    t.reset(id);
    assert!(!t.feed(id, b"\r"));
}

#[test]
fn osc_color_reply_then_enter_is_not_substantive() {
    let mut t = tracker();
    let id = Uuid::new_v4();
    assert!(!t.feed(id, b"\x1b]11;rgb:1234/5678/9abc\x07"));
    assert!(!t.feed(id, b"\r"));
}

#[test]
fn dcs_reply_then_enter_is_not_substantive() {
    let mut t = tracker();
    let id = Uuid::new_v4();
    assert!(!t.feed(id, b"\x1bP1$r0m\x1b\\"));
    assert!(!t.feed(id, b"\r"));
}

#[test]
fn osc_reply_and_enter_in_one_chunk_is_not_substantive() {
    let mut t = tracker();
    assert!(!t.feed(Uuid::new_v4(), b"\x1b]11;rgb:1234/5678/9abc\x07\r"));
}

#[test]
fn control_only_bytes_are_not_substantive() {
    let mut t = tracker();
    let id = Uuid::new_v4();
    assert!(!t.feed(id, b"\x03"));
    assert!(!t.feed(id, b"\x04"));
    assert!(!t.feed(id, b"\x15"));
    assert!(!t.feed(id, b"\r"));
}

#[test]
fn ctrl_c_cancels_pending_line_so_enter_is_not_substantive() {
    let mut t = tracker();
    let id = Uuid::new_v4();
    assert!(!t.feed(id, b"do the thing"));
    assert!(!t.feed(id, b"\x03"));
    assert!(!t.feed(id, b"\r"));
}

#[test]
fn ctrl_u_cancels_pending_line_so_enter_is_not_substantive() {
    let mut t = tracker();
    let id = Uuid::new_v4();
    assert!(!t.feed(id, b"partial"));
    assert!(!t.feed(id, b"\x15"));
    assert!(!t.feed(id, b"\r"));
}

#[test]
fn real_line_after_ctrl_c_then_enter_is_substantive() {
    let mut t = tracker();
    let id = Uuid::new_v4();
    assert!(!t.feed(id, b"oops"));
    assert!(!t.feed(id, b"\x03"));
    assert!(!t.feed(id, b"real prompt"));
    assert!(t.feed(id, b"\r"));
}

#[test]
fn pending_within_tracks_unsubmitted_input_and_expires() {
    use crate::session::profile::IdleTuning;

    let mut t = tracker();
    let id = Uuid::new_v4();
    // (a) The predicate itself, with an age large enough not to matter.
    let forever = Duration::from_secs(3600);
    assert!(!t.pending_within(id, forever));
    t.feed(id, b"\x1b[A");
    assert!(!t.pending_within(id, forever));
    t.feed(id, b" ");
    assert!(!t.pending_within(id, forever));
    t.feed(id, b"\x08");
    assert!(!t.pending_within(id, forever));
    t.feed(id, b"hel");
    assert!(t.pending_within(id, forever));
    t.feed(id, b"\r");
    assert!(!t.pending_within(id, forever));
    t.feed(id, b"oops");
    assert!(t.pending_within(id, forever));
    t.feed(id, b"\x03");
    assert!(!t.pending_within(id, forever));
    t.feed(id, b"more");
    assert!(t.pending_within(id, forever));
    t.reset(id);
    assert!(!t.pending_within(id, forever));

    // (b) The age bound: the same set flag ages out, and a later chunk
    // refreshes the instant rather than only setting it once.
    t.feed(id, b"hel");
    assert!(t.pending_within(id, USER_WRITE_STAMP_WINDOW));
    t.backdate_pending_for_test(id, USER_WRITE_STAMP_WINDOW + Duration::from_secs(1));
    assert!(!t.pending_within(id, USER_WRITE_STAMP_WINDOW));
    t.feed(id, b"p");
    assert!(t.pending_within(id, USER_WRITE_STAMP_WINDOW));

    // (c) The constant's derivation, as a tripwire. The 500ms term is
    // `idle_detector::CHECK_INTERVAL`, which is private to that module and
    // therefore cannot be referenced here.
    assert_eq!(
        USER_WRITE_STAMP_WINDOW,
        IdleTuning::DEFAULT.idle_threshold
            + IdleTuning::DEFAULT.resize_grace
            + Duration::from_millis(500)
    );
}

/// #2336 - the exact chunk classes the typing hold accepts and rejects.
/// The rejected rows are the same device-reply/mouse/focus classes #1682
/// proved arrive through the same `onData` stream while a person is NOT
/// typing.
#[test]
fn typing_hold_classifier_accepts_human_keys_only() {
    for accepted in [
        b"hello".as_slice(),
        b" ".as_slice(),
        b"\t".as_slice(),
        b"\r".as_slice(),
        b"\n".as_slice(),
        b"\x08".as_slice(),
        b"\x7f".as_slice(),
        b"\x03".as_slice(),
        b"\x15".as_slice(),
        b"h\x03i".as_slice(),
    ] {
        assert!(chunk_qualifies_typing_hold(accepted), "{accepted:?}");
    }
    for rejected in [
        b"".as_slice(),
        b"\x1b".as_slice(),
        b"\x1b[A".as_slice(),
        b"\x1b[I".as_slice(),
        b"\x1b[24;80R".as_slice(),
        b"\x1b[?62;1;6;9;15;22c".as_slice(),
        b"\x1b]11;rgb:1234/5678/9abc\x07".as_slice(),
        b"\x1b[<35;80;24M".as_slice(),
        b"\x1bP1$r0m\x1b\\".as_slice(),
        b"\x01\x02\x04".as_slice(),
    ] {
        assert!(!chunk_qualifies_typing_hold(rejected), "{rejected:?}");
    }
}

/// #2336 - a qualifying key opens the natural window; the window ages out on
/// its own; a later key refreshes it.
#[test]
fn typing_hold_natural_window_arms_refreshes_and_expires() {
    let mut tracker = TypingHoldTracker::default();
    let id = Uuid::new_v4();
    let window = Duration::from_secs(30);

    assert!(!tracker.is_hold_active(id, window));
    tracker.note_qualifying_key(id);
    assert!(tracker.is_hold_active(id, window));
    assert!(tracker.snapshot(id, window).closed);

    tracker.backdate_last_key_for_test(id, window + Duration::from_secs(1));
    assert!(!tracker.is_hold_active(id, window));
    assert!(!tracker.snapshot(id, window).closed);

    tracker.note_qualifying_key(id);
    assert!(tracker.is_hold_active(id, window));
}

/// #2336 - per-session isolation: a hold in one session never leaks into
/// another, and a teardown reset drops only its own entry.
#[test]
fn typing_hold_is_per_session() {
    let mut tracker = TypingHoldTracker::default();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let window = Duration::from_secs(30);

    tracker.note_qualifying_key(a);
    assert!(tracker.is_hold_active(a, window));
    assert!(!tracker.is_hold_active(b, window));
    assert_eq!(tracker.snapshot(b, window).held_count, 0);

    tracker.record_held_message(a, "m-a");
    tracker.record_held_message(b, "m-b");
    tracker.reset(b);
    assert_eq!(tracker.snapshot(a, window).held_count, 1);
    assert_eq!(tracker.snapshot(b, window).held_count, 0);
}

/// #2336 - a closed click releases, suppresses only the natural window that
/// existed then, and a later key re-arms. Manual close/open needs no key.
#[test]
fn typing_hold_manual_release_suppresses_then_next_key_rearms() {
    let mut tracker = TypingHoldTracker::default();
    let id = Uuid::new_v4();
    let window = Duration::from_secs(30);

    // A key arms the natural window; the first click (effective-closed)
    // releases it and shows open, suppression in force.
    tracker.note_qualifying_key(id);
    let released = tracker.toggle_manual(id, window);
    assert!(!released.closed);
    assert!(!tracker.is_hold_active(id, window));

    // The next qualifying key clears suppression and re-arms.
    tracker.note_qualifying_key(id);
    assert!(tracker.is_hold_active(id, window));

    // Release the re-armed window, then an open click takes the MANUAL hold:
    // it outlives the natural window...
    assert!(!tracker.toggle_manual(id, window).closed);
    let held = tracker.toggle_manual(id, window);
    assert!(held.closed);
    tracker.backdate_last_key_for_test(id, window + Duration::from_secs(1));
    assert!(tracker.is_hold_active(id, window));

    // ...and an explicit click before its own deadline still releases now.
    assert!(!tracker.toggle_manual(id, window).closed);
    assert!(!tracker.is_hold_active(id, window));

    // A fresh manual close auto-releases after 2x the window with no click.
    assert!(tracker.toggle_manual(id, window).closed);
    tracker.backdate_manual_close_for_test(id, window.saturating_mul(2) + Duration::from_secs(1));
    assert!(!tracker.is_hold_active(id, window));
}

/// #2379 - a manual close expires at exactly twice the window passed in:
/// inside 2x it stays closed (a 1x implementation fails this), past 2x it
/// opens with no click (a 3x implementation fails this), and the held count
/// survives the auto-release. Re-taking the hold then clicking before its
/// deadline releases immediately.
#[test]
fn typing_hold_manual_close_expires_at_twice_the_window_and_count_survives() {
    let mut tracker = TypingHoldTracker::default();
    let id = Uuid::new_v4();
    let window = Duration::from_secs(30);
    let deadline = window.saturating_mul(2);

    tracker.record_held_message(id, "msg-1");
    tracker.toggle_manual(id, window);
    assert!(tracker.snapshot(id, window).closed);

    tracker.backdate_manual_close_for_test(id, deadline - Duration::from_secs(1));
    assert!(
        tracker.is_hold_active(id, window),
        "inside 2x the window the manual hold stands"
    );

    tracker.backdate_manual_close_for_test(id, deadline + Duration::from_secs(1));
    let snapshot = tracker.snapshot(id, window);
    assert!(
        !snapshot.closed,
        "a manual close must auto-release after 2x the window"
    );
    assert_eq!(
        snapshot.held_count, 1,
        "auto-release must not touch the held count"
    );

    assert!(tracker.toggle_manual(id, window).closed);
    assert!(!tracker.toggle_manual(id, window).closed);
    assert!(!tracker.is_hold_active(id, window));
}

/// #2379 - the manual deadline is recomputed from the window passed at
/// evaluation time, not from a hardcoded 60 and not from a deadline frozen
/// at close time. The same close after 50s of silence: closed under the 30s
/// window (2x = 60s), already open under a 20s window (2x = 40s). A
/// hardcoded 60 keeps it closed here and fails this test.
#[test]
fn typing_hold_manual_expiry_follows_the_live_window_not_a_hardcoded_sixty() {
    let mut tracker = TypingHoldTracker::default();
    let id = Uuid::new_v4();

    let thirty = Duration::from_secs(30);
    tracker.toggle_manual(id, thirty);
    tracker.backdate_manual_close_for_test(id, Duration::from_secs(50));

    assert!(tracker.snapshot(id, thirty).closed, "50s is inside 2x30s");
    let twenty = Duration::from_secs(20);
    assert!(
        !tracker.snapshot(id, twenty).closed,
        "50s is past 2x20s; the deadline must follow the live window"
    );
}

/// #2336 - the count is a unique set of message IDs, repeated recordings do
/// not increase it, reads do not mutate it, and terminal cleanup removes it.
#[test]
fn typing_hold_counts_unique_ids_and_snapshot_is_read_only() {
    let mut tracker = TypingHoldTracker::default();
    let id = Uuid::new_v4();
    let window = Duration::from_secs(30);

    tracker.record_held_message(id, "msg-1");
    tracker.record_held_message(id, "msg-1");
    tracker.record_held_message(id, "msg-2");
    for _ in 0..5 {
        assert_eq!(tracker.snapshot(id, window).held_count, 2);
        assert_eq!(tracker.snapshot(id, window).held_count, 2);
    }

    tracker.clear_held_message(id, "msg-1");
    assert_eq!(tracker.snapshot(id, window).held_count, 1);
    tracker.reset(id);
    assert_eq!(tracker.snapshot(id, window).held_count, 0);
    assert!(!tracker.is_hold_active(id, window));
}
