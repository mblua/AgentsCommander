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
