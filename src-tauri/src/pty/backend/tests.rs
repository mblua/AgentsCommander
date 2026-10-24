use super::*;

/// #973 - the size a real view fitted is handed down untouched. That is the whole point of
/// the type: the PTY opens at the size the terminal already is, so nothing has to resize it
/// during the child's startup.
#[test]
fn a_real_fitted_size_is_handed_down_as_is() {
    assert_eq!(
        PtyViewport::from_fit(74, 23),
        PtyViewport { cols: 74, rows: 23 }
    );
    // right on the floor, and still a real terminal
    assert_eq!(
        PtyViewport::from_fit(20, 5),
        PtyViewport { cols: 20, rows: 5 }
    );
}

/// #973 - the case a zero check misses. xterm clamps a collapsed container to 2x1 rather
/// than reporting a zero, so `2x1` is what a degenerate box actually looks like coming out
/// of `fit()` - well-formed, positive, and not a terminal anybody is looking at.
#[test]
fn a_collapsed_box_does_not_open_the_pty_at_two_by_one() {
    assert_eq!(PtyViewport::from_fit(2, 1), PtyViewport::DEFAULT);
    assert_eq!(PtyViewport::from_fit(19, 40), PtyViewport::DEFAULT);
    assert_eq!(PtyViewport::from_fit(80, 4), PtyViewport::DEFAULT);
    assert_eq!(PtyViewport::from_fit(0, 0), PtyViewport::DEFAULT);
}
