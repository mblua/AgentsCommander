//! (#871) Substantive-submission classifier for raw PTY keystroke input, and
//! (#2336) the per-session typing-hold state that defers peer wakes while the
//! user is typing.
//!
//! Restart Session stamps a durable "start fresh on restore" intent. That intent
//! must survive an app restart when the user has not actually engaged. The bug:
//! `pty_write` cleared the intent on any byte, including non-substantive terminal
//! writes such as focus/CSI sequences, terminal init, and empty Enter. This
//! module distinguishes a real prompt submission (CR/LF with pending
//! non-whitespace content since the last submit) from those control writes, so
//! only substantive engagement clears the fresh intent.
//!
//! `pty_write` carries user-to-PTY input only; terminal output flows the other
//! way via the `pty_output` event and never reaches here. So we classify the
//! user's own keystrokes plus xterm-generated input, all ESC-introduced and
//! skipped below. We do not reconstruct the child agent's rendered line.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// #1682 - how recent a user-driven PTY write must be for a busy->idle edge to
/// still be attributable to it rather than to agent work. Read by BOTH stamp
/// gates: the aged typing gate over `pending_within` below, and the
/// control-write gate over `IdleDetector::control_write_age`.
///
/// 6000ms = 2500 (`IdleTuning::DEFAULT.idle_threshold`, the silence that
/// defines an edge) + 3000 (`IdleTuning::DEFAULT.resize_grace`, the longest
/// output lag this repository already attributes to a user-driven event rather
/// than to agent work, reused here by analogy for an agent CLI's repaint) + 500
/// (`idle_detector::CHECK_INTERVAL`, the watcher's granularity). A literal
/// rather than a sum over `IdleTuning` on purpose: a `crate::session::profile`
/// reference from this module would add a module arc, and this phase adds none.
pub const USER_WRITE_STAMP_WINDOW: Duration = Duration::from_millis(6000);

/// Per-session tracker of whether non-whitespace printable input is pending
/// since the last submit or fresh boundary.
#[derive(Default)]
pub struct SubstantiveInputTracker {
    pending_nonspace: HashMap<Uuid, bool>,
    /// #1682 - when the most recent chunk that CONTRIBUTED non-whitespace
    /// printable content for this id arrived. Not "the last chunk that left the
    /// flag set": a chunk that leaves the flag alone (an ESC-introduced
    /// sequence, a device reply, a bare control byte) must not refresh it, or
    /// nothing ever ages out in a terminal that is receiving anything.
    /// Maintained by `feed` and `reset`, back-dated in tests by
    /// `backdate_pending_for_test`, read only by `pending_within`; #871 never reads it.
    pending_since: HashMap<Uuid, Instant>,
}

/// Tauri-managed shared state alias.
pub type SubstantiveInputState = Arc<Mutex<SubstantiveInputTracker>>;

/// Build a fresh managed state value.
pub fn new_state() -> SubstantiveInputState {
    Arc::new(Mutex::new(SubstantiveInputTracker::default()))
}

impl SubstantiveInputTracker {
    /// Feed one raw keystroke chunk for `id`. Returns true iff the chunk
    /// completed a substantive submission: a CR/LF encountered while
    /// non-whitespace printable content was pending since the last submit.
    pub fn feed(&mut self, id: Uuid, data: &[u8]) -> bool {
        let pending = self.pending_nonspace.entry(id).or_insert(false);
        let effect = classify_chunk(pending, data);
        // #1682 - the age half of the typing gate. `classify_chunk`'s effect on
        // `*pending` and the returned `submitted` are untouched; this records
        // only WHEN content was last contributed, so a stale half-typed line
        // cannot suppress the stamp for the rest of the session.
        //
        // The refresh is conditioned on `effect.contributed`, NOT on the flag
        // being set after the chunk. A chunk that leaves the flag alone (arrow
        // key, focus report, DSR/DA reply, OSC colour reply, SGR mouse report,
        // any other bare control byte) arrives constantly while an agent works,
        // and refreshing on those would restart the clock forever.
        if !*pending {
            self.pending_since.remove(&id);
        } else if effect.contributed {
            self.pending_since.insert(id, Instant::now());
        }
        effect.submitted
    }

    /// Reset the pending flag for `id`. Call when a fresh boundary is stamped so
    /// pre-boundary keystrokes cannot leak into a post-boundary submit decision.
    pub fn reset(&mut self, id: Uuid) {
        self.pending_nonspace.remove(&id);
        self.pending_since.remove(&id);
    }

    /// #1682 - does `id` have non-whitespace printable input pending since the
    /// last submit or fresh boundary, AND did the chunk that last contributed
    /// that content arrive within `max_age`? Read-only: it consumes nothing, so
    /// the stamp path never mutates #871's state.
    ///
    /// The age half is required, not defensive: `classify_chunk` clears `pending_nonspace` only
    /// on CR/LF-with-content and on Ctrl-C / Ctrl-U, and outside this module only `reset` clears
    /// it, at a session destroy or restart and at a fresh conversation boundary. Neither reaches
    /// a wake-driven agent, so unaged, one printable byte that never receives a CR would suppress
    /// every later stamp for that session's whole life. The age is measured from the last chunk
    /// that CONTRIBUTED non-whitespace printable content, which is deliberate: a person still
    /// typing keeps refreshing it, because every keystroke of a line contributes, and a line
    /// typed and abandoned ages out even while the terminal keeps receiving other input, because
    /// the sequences and control bytes that flow in meanwhile contribute nothing.
    ///
    /// An id this tracker has never seen is `false`. So is an id whose flag is
    /// set with no recorded instant, which `feed` cannot produce; that
    /// direction fails toward stamping.
    pub fn pending_within(&self, id: Uuid, max_age: Duration) -> bool {
        if !self.pending_nonspace.get(&id).copied().unwrap_or(false) {
            return false;
        }
        self.pending_since
            .get(&id)
            .is_some_and(|since| since.elapsed() <= max_age)
    }

    #[cfg(test)]
    pub(crate) fn backdate_pending_for_test(&mut self, id: Uuid, age: Duration) {
        if let Some(since) = self.pending_since.get_mut(&id) {
            *since = Instant::now()
                .checked_sub(age)
                .expect("process uptime exceeds the backdated age");
        }
    }
}

/// What one chunk did, as observed by the single scan in `classify_chunk`.
struct ChunkEffect {
    /// #871 - a CR/LF arrived while non-whitespace printable content was
    /// pending: a real prompt submission. Unchanged by #1682.
    submitted: bool,
    /// #1682 - this chunk itself contributed non-whitespace printable content,
    /// i.e. it took the `*pending = true` arm at least once. Derived from the
    /// same scan and the same arm as `pending`, never from a second scan
    /// beside it, so it cannot drift from #871's rules.
    contributed: bool,
}

/// Pure classifier. `pending` persists across chunks, while the ESC parse state
/// is local to this chunk so it can never swallow a later chunk's content.
fn classify_chunk(pending: &mut bool, data: &[u8]) -> ChunkEffect {
    let mut submitted = false;
    let mut contributed = false;
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        match b {
            0x1b => {
                i += 1;
                if i >= data.len() {
                    break;
                }
                match data[i] {
                    b'[' | b'O' => {
                        i += 1;
                        while i < data.len() && !(0x40..=0x7e).contains(&data[i]) {
                            i += 1;
                        }
                    }
                    b']' | b'P' | b'_' | b'^' | b'X' => {
                        i += 1;
                        while i < data.len() {
                            if data[i] == 0x07 {
                                break;
                            }
                            if data[i] == 0x1b {
                                if i + 1 < data.len() && data[i + 1] == 0x5c {
                                    i += 1;
                                }
                                break;
                            }
                            i += 1;
                        }
                    }
                    _ => {}
                }
            }
            0x0d | 0x0a => {
                if *pending {
                    submitted = true;
                    *pending = false;
                }
            }
            0x03 | 0x15 => {
                *pending = false;
            }
            0x08 | 0x7f | 0x09 | 0x20 => {}
            _ if b < 0x20 => {}
            _ => {
                *pending = true;
                contributed = true;
            }
        }
        i += 1;
    }
    ChunkEffect {
        submitted,
        contributed,
    }
}

/// #2336 - the typing-hold snapshot returned to the frontend padlock. `closed`
/// is the EFFECTIVE hold (manual OR the natural window, suppression included),
/// which is exactly the state the click toggles. `held_count` is the number of
/// unique peer wake message IDs currently recorded as deferred for the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypingHoldSnapshot {
    pub closed: bool,
    pub held_count: u32,
}

/// #2336 - per-session typing-hold state: the automatic window and the manual
/// padlock. Runtime-only; never persisted and never reconciled from disk.
///
/// Deliberately separate from [`SubstantiveInputTracker`]. That tracker's
/// `pending_within` clears on Enter, Ctrl-C and Ctrl-U, so it stops being true
/// exactly when a person submits a line. This hold must survive a submission (a
/// later key refreshes it), so it owns its own clock. Nothing here is read by
/// #871/#1682 and nothing there is read here.
#[derive(Default)]
pub struct TypingHoldTracker {
    sessions: HashMap<Uuid, TypingHoldSession>,
}

#[derive(Default)]
struct TypingHoldSession {
    /// When the most recent qualifying human keystroke arrived. Refreshed by
    /// `note_qualifying_key`, which every desktop `pty_write` chunk that passes
    /// [`chunk_qualifies_typing_hold`] reaches. `None` until the first one.
    last_qualifying_key: Option<Instant>,
    /// Manual padlock: `true` holds until the user releases it, with no expiry.
    manual_closed: bool,
    /// Incremented by every manual release (closed click). The pair with
    /// `suppressed_generation` is how one release suppresses exactly the natural
    /// window that existed at that moment, and nothing later.
    release_generation: u64,
    /// The release generation whose natural window was suppressed. A qualifying
    /// key clears it, so the next window re-arms normally.
    suppressed_generation: Option<u64>,
    /// Unique peer wake message IDs observed deferred for this session in this
    /// run. The padlock count; removed on delivery, permanent rejection, or
    /// session teardown.
    held_message_ids: std::collections::HashSet<String>,
}

impl TypingHoldSession {
    /// The natural window: a qualifying key within `window`, unless the release
    /// generation currently in force suppressed it.
    fn natural_window_active(&self, window: Duration) -> bool {
        if self.suppressed_generation == Some(self.release_generation) {
            return false;
        }
        self.last_qualifying_key
            .is_some_and(|last| last.elapsed() <= window)
    }

    fn effective_closed(&self, window: Duration) -> bool {
        self.manual_closed || self.natural_window_active(window)
    }
}

impl TypingHoldTracker {
    /// Record one qualifying desktop keystroke chunk.
    pub fn note_qualifying_key(&mut self, id: Uuid) {
        let session = self.sessions.entry(id).or_default();
        session.last_qualifying_key = Some(Instant::now());
        // A new key re-arms the natural window even after a manual release.
        session.suppressed_generation = None;
    }

    /// Is the hold active for `id`? Read-only, so an injection attempt never
    /// mutates the state it is evaluating (expiry is evaluated on read).
    pub fn is_hold_active(&self, id: Uuid, window: Duration) -> bool {
        self.sessions
            .get(&id)
            .is_some_and(|session| session.effective_closed(window))
    }

    /// Read-only padlock snapshot. An unseen session is open with no held ids,
    /// and repeated snapshots never change the count.
    pub fn snapshot(&self, id: Uuid, window: Duration) -> TypingHoldSnapshot {
        match self.sessions.get(&id) {
            Some(session) => TypingHoldSnapshot {
                closed: session.effective_closed(window),
                held_count: held_count(&session.held_message_ids),
            },
            None => TypingHoldSnapshot {
                closed: false,
                held_count: 0,
            },
        }
    }

    /// Atomic padlock toggle. An effective-closed session releases: manual state
    /// drops, the generation advances and that generation's natural window is
    /// suppressed, so the pending queue becomes eligible immediately. An
    /// effective-open session takes the manual hold. Returns the post-toggle
    /// snapshot under the same lock, so the UI cannot observe a half-flip.
    pub fn toggle_manual(&mut self, id: Uuid, window: Duration) -> TypingHoldSnapshot {
        let session = self.sessions.entry(id).or_default();
        if session.effective_closed(window) {
            session.manual_closed = false;
            session.release_generation = session.release_generation.wrapping_add(1);
            session.suppressed_generation = Some(session.release_generation);
        } else {
            session.manual_closed = true;
        }
        TypingHoldSnapshot {
            closed: session.effective_closed(window),
            held_count: held_count(&session.held_message_ids),
        }
    }

    /// Record a deferred peer wake message ID. The set makes the count unique,
    /// so a retried poll of the same message never increases it.
    pub fn record_held_message(&mut self, id: Uuid, message_id: &str) {
        self.sessions
            .entry(id)
            .or_default()
            .held_message_ids
            .insert(message_id.to_string());
    }

    /// Remove one message ID on observed delivery or terminal rejection.
    pub fn clear_held_message(&mut self, id: Uuid, message_id: &str) {
        if let Some(session) = self.sessions.get_mut(&id) {
            session.held_message_ids.remove(message_id);
        }
    }

    /// Full per-session teardown, shared with the substantive tracker's reset
    /// sites (destroy, restart, reset). Drops the clock, the manual state, the
    /// generations and every counted ID.
    pub fn reset(&mut self, id: Uuid) {
        self.sessions.remove(&id);
    }

    #[cfg(test)]
    pub(crate) fn backdate_last_key_for_test(&mut self, id: Uuid, age: Duration) {
        if let Some(last) = self
            .sessions
            .get_mut(&id)
            .and_then(|session| session.last_qualifying_key.as_mut())
        {
            *last = Instant::now()
                .checked_sub(age)
                .expect("process uptime exceeds the backdated age");
        }
    }
}

fn held_count(ids: &std::collections::HashSet<String>) -> u32 {
    ids.len().min(u32::MAX as usize) as u32
}

/// Tauri-managed shared state alias.
pub type TypingHoldState = Arc<Mutex<TypingHoldTracker>>;

/// Build a fresh managed typing-hold state value.
pub fn new_typing_hold_state() -> TypingHoldState {
    Arc::new(Mutex::new(TypingHoldTracker::default()))
}

/// #2336 - does one raw `pty_write` chunk contain a keystroke the typing hold
/// counts? Qualifying: printable text, whitespace, Enter, Backspace, Delete,
/// and the human edit controls Ctrl-C / Ctrl-U. Not qualifying: an empty write,
/// anything introduced by ESC (arrows, focus reports, device replies, mouse
/// reports, OSC/DCS replies), and every other bare control byte. The scan skips
/// ESC sequences exactly like `classify_chunk`, so a device reply's printable
/// tail cannot masquerade as typing.
///
/// Called only from the desktop `pty_write` path, so an injected write never
/// reaches it: injections are not user keystrokes.
pub fn chunk_qualifies_typing_hold(data: &[u8]) -> bool {
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        match b {
            0x1b => {
                i += 1;
                if i >= data.len() {
                    break;
                }
                match data[i] {
                    b'[' | b'O' => {
                        i += 1;
                        while i < data.len() && !(0x40..=0x7e).contains(&data[i]) {
                            i += 1;
                        }
                    }
                    b']' | b'P' | b'_' | b'^' | b'X' => {
                        i += 1;
                        while i < data.len() {
                            if data[i] == 0x07 {
                                break;
                            }
                            if data[i] == 0x1b {
                                if i + 1 < data.len() && data[i + 1] == 0x5c {
                                    i += 1;
                                }
                                break;
                            }
                            i += 1;
                        }
                    }
                    _ => {}
                }
            }
            0x0d | 0x0a | 0x08 | 0x7f | 0x09 | 0x20 | 0x03 | 0x15 => return true,
            _ if b < 0x20 => {}
            _ => return true,
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
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
        // it stays closed even after the natural window ages out.
        assert!(!tracker.toggle_manual(id, window).closed);
        let held = tracker.toggle_manual(id, window);
        assert!(held.closed);
        tracker.backdate_last_key_for_test(id, window + Duration::from_secs(1));
        assert!(tracker.is_hold_active(id, window));

        // Clicking again releases the manual hold and suppresses the (expired)
        // window generation.
        let released_again = tracker.toggle_manual(id, window);
        assert!(!released_again.closed);
        assert!(!tracker.is_hold_active(id, window));
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
}
