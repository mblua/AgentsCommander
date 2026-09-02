//! (#871) Substantive-submission classifier for raw PTY keystroke input.
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
    /// #1682 - when the most recent chunk that left `pending_nonspace` set for
    /// this id arrived. Maintained by `feed` and `reset`, back-dated in tests by
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
        let submitted = classify_chunk(pending, data);
        // #1682 - the age half of the typing gate. `classify_chunk` and the
        // returned `submitted` are untouched; this records only WHEN the flag
        // was last left set, so a stale half-typed line cannot suppress the
        // stamp for the rest of the session.
        if *pending {
            self.pending_since.insert(id, Instant::now());
        } else {
            self.pending_since.remove(&id);
        }
        submitted
    }

    /// Reset the pending flag for `id`. Call when a fresh boundary is stamped so
    /// pre-boundary keystrokes cannot leak into a post-boundary submit decision.
    pub fn reset(&mut self, id: Uuid) {
        self.pending_nonspace.remove(&id);
        self.pending_since.remove(&id);
    }

    /// #1682 - does `id` have non-whitespace printable input pending since the
    /// last submit or fresh boundary, AND did the chunk that left it pending
    /// arrive within `max_age`? Read-only: it consumes nothing, so the stamp
    /// path never mutates #871's state.
    ///
    /// The age half is required, not defensive: `classify_chunk` clears `pending_nonspace` only
    /// on CR/LF-with-content and on Ctrl-C / Ctrl-U, and outside this module only `reset` clears
    /// it, at a session destroy or restart and at a fresh conversation boundary. Neither reaches
    /// a wake-driven agent, so unaged, one printable byte that never receives a CR would suppress
    /// every later stamp for that session's whole life. The age is measured from the LAST chunk
    /// that left content pending, which is deliberate: a person still typing keeps refreshing it,
    /// and a line typed and abandoned ages out.
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

/// Pure classifier. `pending` persists across chunks, while the ESC parse state
/// is local to this chunk so it can never swallow a later chunk's content.
fn classify_chunk(pending: &mut bool, data: &[u8]) -> bool {
    let mut submitted = false;
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
            }
        }
        i += 1;
    }
    submitted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> SubstantiveInputTracker {
        SubstantiveInputTracker::default()
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
}
