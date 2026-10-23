use super::*;

// ---- #1032: the rows accessor, against the real parser -------------------------

/// The whole transfer claim of the round-1 capture rests on this: the capture read its
/// bytes with `contents_between`, and the engine reads them with `rows`. If those two
/// ever disagree, every measured fact about the statusline is about a screen this
/// accessor does not return.
///
/// Valid rows only: the `Equal` branch of `contents_between` ends in
/// `.unwrap_or_default()`, so an out-of-range row answers `""` where `rows()` simply
/// has no index - comparing those would pin the crate's error handling, not the claim.
#[test]
fn get_screen_rows_matches_contents_between() {
    let fanout = fanout();
    let id = session(&fanout);
    feed(
        &fanout,
        id,
        &[
            "  Context \u{2591}\u{2591}\u{2588} 42% \u{2502} Usage\r\nprose above it\r\n"
                .as_bytes(),
        ],
    );

    let rows = fanout
        .get_screen_rows(id)
        .expect("rows for a registered session");

    let parsers = fanout.screen_parsers.lock().unwrap();
    let screen = parsers.get(&id).expect("parser").parser.screen();
    let (row_count, cols) = screen.size();
    assert_eq!(rows.len(), row_count as usize);
    for r in 0..row_count {
        assert_eq!(
            rows[r as usize],
            screen.contents_between(r, 0, r, cols),
            "row {r} disagrees with the accessor the capture measured"
        );
    }
}

/// The column-2 anchor is the engine's only defence against single-row input-box prose,
/// and it is worth exactly as much as this: that the two leading spaces of a row painted
/// at column 2 survive the round trip out of the grid.
#[test]
fn leading_spaces_survive_so_the_column_two_anchor_works() {
    let fanout = fanout();
    let id = session(&fanout);
    feed(&fanout, id, &[b"  Context 7% and a tail"]);

    let rows = fanout.get_screen_rows(id).expect("rows");
    assert_eq!(rows[0], "  Context 7% and a tail");
    assert!(
        rows[0].starts_with("  Context"),
        "the anchor rests on these two spaces: {:?}",
        rows[0]
    );
}

/// Criterion 1. The mirror is keyed by session id and so is the scraper's map; two
/// concurrent agents must never read each other's number.
#[test]
fn two_sessions_never_cross_rows() {
    let fanout = fanout();
    let a = session(&fanout);
    let b = session(&fanout);

    feed(&fanout, a, &[b"  Context 11%"]);
    feed(&fanout, b, &[b"  Context 88%"]);

    assert_eq!(
        fanout.get_screen_rows(a).expect("rows a")[0],
        "  Context 11%"
    );
    assert_eq!(
        fanout.get_screen_rows(b).expect("rows b")[0],
        "  Context 88%"
    );
}

/// The fanout's two-state contract: absent parser is `None`, and that is all it means.
/// Whether the session is OVER is a question this type cannot answer and does not try to.
#[test]
fn get_screen_rows_is_none_for_an_unknown_session() {
    let fanout = fanout();
    assert!(fanout.get_screen_rows(Uuid::new_v4()).is_none());
}

/// Criterion 2. The reading comes off the mirror, which AC feeds from the PTY read loop
/// whether or not a terminal was ever mounted: `PtyOutputTarget::noop()`, never resized,
/// still reads back.
#[test]
fn rows_are_readable_for_a_session_with_no_terminal() {
    let fanout = fanout();
    let id = session(&fanout);
    feed(&fanout, id, &[b"  Context 5%"]);

    let rows = fanout
        .get_screen_rows(id)
        .expect("a never-mounted session still reads");
    assert_eq!(rows[0], "  Context 5%");
}

#[test]
fn terminal_screen_copy_preserves_cells_styles_colors_and_wide_pairs() {
    let fanout = fanout();
    let id = Uuid::new_v4();
    fanout
        .register_session_for_test(id, IdleTuning::DEFAULT, 2, 4)
        .expect("register test session");
    feed(
        &fanout,
        id,
        &["\x1b[?1049h\x1b[31;44;1;3;4;7mA界".as_bytes()],
    );

    let copied = match fanout.copy_terminal_screen(id) {
        crate::pty::backend::TerminalScreenCopyRead::Copied(copied) => copied,
        _ => panic!("expected compact viewport copy"),
    };
    let model = copied
        .into_model(id, SessionBackendKind::LocalProcess)
        .expect("valid owned model");
    assert_eq!(model.screen.sequence, 1);
    assert_eq!(model.screen.active_buffer, TerminalActiveBuffer::Alternate);
    assert_eq!(model.screen.lines.len(), 2);
    assert_eq!(model.screen.lines[0].cells.len(), 4);
    let first = &model.screen.lines[0].cells[0];
    assert_eq!(first.text, "A");
    assert_eq!(first.foreground, TerminalColor::Indexed { index: 1 });
    assert_eq!(first.background, TerminalColor::Indexed { index: 4 });
    assert!(first.style.bold && first.style.italic && first.style.underline);
    assert!(first.style.inverse);
    assert_eq!(
        model.screen.lines[0].cells[1].width,
        TerminalCellWidth::WideLead
    );
    assert_eq!(
        model.screen.lines[0].cells[2].width,
        TerminalCellWidth::WideContinuation
    );
    assert!(model.screen.lines[0].cells[2].text.is_empty());
}

#[test]
fn capture_and_model_result_debug_omit_cell_osc_and_session_canaries() {
    const CELL_CANARY: &str = "CELL_1173_VT_Q8L5";
    const OSC_CANARY: &str = "OSC_1173_VT_Q8L5";
    let fanout = fanout();
    let id = Uuid::parse_str("11730000-0000-4000-8000-00000000a815").unwrap();
    fanout
        .register_session_for_test(id, IdleTuning::DEFAULT, 2, 40)
        .expect("register test session");
    let output = format!("{CELL_CANARY}\x1b]0;{OSC_CANARY}\x07");
    feed(&fanout, id, &[output.as_bytes()]);

    let copied = fanout.copy_terminal_screen(id);
    let copied_diagnostic = format!("{copied:?}");
    let captured = match copied {
        crate::pty::backend::TerminalScreenCopyRead::Copied(captured) => captured,
        _ => panic!("expected compact viewport copy"),
    };
    let model = captured
        .into_model(id, SessionBackendKind::LocalProcess)
        .expect("valid owned model");
    let read = crate::pty::backend::TerminalScreenRead::Captured(model);
    let diagnostic = format!(
        "{copied_diagnostic}\n{read:?}\n{:?}\n{:?}",
        crate::pty::backend::TerminalScreenCopyRead::Unavailable,
        crate::pty::backend::TerminalScreenRead::TooLarge,
    );
    let id_text = id.to_string();
    for forbidden in [CELL_CANARY, OSC_CANARY, id_text.as_str()] {
        assert!(!diagnostic.contains(forbidden));
    }
    for structural in [
        "rows: 2",
        "columns: 40",
        "cells: 80",
        "TerminalScreenRead::Captured",
        "TerminalScreenCopyRead::Unavailable",
        "TerminalScreenRead::TooLarge",
    ] {
        assert!(diagnostic.contains(structural));
    }
}

#[test]
fn output_and_capture_race_is_wholly_before_or_after_one_chunk() {
    let fanout = fanout();
    for _ in 0..64 {
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 1, 2)
            .expect("register test session");
        let writer = fanout.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let writer_barrier = Arc::clone(&barrier);
        let thread = std::thread::spawn(move || {
            writer_barrier.wait();
            feed(&writer, id, &[b"A"]);
        });
        barrier.wait();
        let copied = fanout.copy_terminal_screen(id);
        thread.join().unwrap();
        let model = match copied {
            crate::pty::backend::TerminalScreenCopyRead::Copied(copied) => copied
                .into_model(id, SessionBackendKind::LocalProcess)
                .unwrap(),
            _ => panic!("expected coherent capture"),
        };
        let first = &model.screen.lines[0].cells[0].text;
        assert!(
            (model.screen.sequence == 0 && first.is_empty())
                || (model.screen.sequence == 1 && first == "A")
        );
        fanout.remove_session(id);
    }
}

#[test]
fn resize_and_capture_race_returns_one_complete_dimension_set() {
    let fanout = fanout();
    for _ in 0..64 {
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 2, 2)
            .expect("register test session");
        let writer = fanout.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let writer_barrier = Arc::clone(&barrier);
        let thread = std::thread::spawn(move || {
            writer_barrier.wait();
            writer.resize_screen_and_broadcast(id, 4, 3);
        });
        barrier.wait();
        let copied = fanout.copy_terminal_screen(id);
        thread.join().unwrap();
        let model = match copied {
            crate::pty::backend::TerminalScreenCopyRead::Copied(copied) => copied
                .into_model(id, SessionBackendKind::LocalProcess)
                .unwrap(),
            _ => panic!("expected coherent capture"),
        };
        let dimensions = model.screen.dimensions;
        assert!(
            (dimensions.rows == 2 && dimensions.columns == 2)
                || (dimensions.rows == 3 && dimensions.columns == 4)
        );
        assert_eq!(model.screen.lines.len(), usize::from(dimensions.rows));
        assert!(model
            .screen
            .lines
            .iter()
            .all(|line| line.cells.len() == usize::from(dimensions.columns)));
        assert_eq!(model.screen.sequence, 0);
        fanout.remove_session(id);
    }
}

#[test]
fn large_and_partial_osc_state_is_excluded_from_the_viewport_copy() {
    let fanout = fanout();
    let id = Uuid::new_v4();
    fanout
        .register_session_for_test(id, IdleTuning::DEFAULT, 1, 4)
        .expect("register test session");
    let sentinel = "snapshot-osc-sentinel".repeat(8_192);
    let first = format!("\x1b]0;{sentinel}");
    feed(&fanout, id, &[first.as_bytes()]);
    let copied = match fanout.copy_terminal_screen(id) {
        crate::pty::backend::TerminalScreenCopyRead::Copied(copied) => copied,
        _ => panic!("partial OSC must not break capture"),
    };
    let model = copied
        .into_model(id, SessionBackendKind::LocalProcess)
        .unwrap();
    assert!(model
        .screen
        .lines
        .iter()
        .flat_map(|line| &line.cells)
        .all(|cell| !cell.text.contains("snapshot-osc-sentinel")));
}

#[test]
fn hostile_one_column_wide_input_removes_only_its_parser() {
    let fanout = fanout();
    let faulty = Uuid::new_v4();
    let healthy = Uuid::new_v4();
    fanout
        .register_session_for_test(faulty, IdleTuning::DEFAULT, 1, 1)
        .expect("register faulty test session");
    fanout
        .register_session_for_test(healthy, IdleTuning::DEFAULT, 2, 4)
        .expect("register healthy test session");

    feed(&fanout, faulty, &["界".as_bytes()]);
    feed(&fanout, healthy, &[b"ok"]);

    assert!(matches!(
        fanout.copy_terminal_screen(faulty),
        crate::pty::backend::TerminalScreenCopyRead::Unavailable
    ));
    assert!(matches!(
        fanout.copy_terminal_screen(healthy),
        crate::pty::backend::TerminalScreenCopyRead::Copied(_)
    ));
}
