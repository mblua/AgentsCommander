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
    /// #2379 - when the manual padlock was closed; `None` when no manual hold
    /// is set. The manual hold has its own clock: it stays closed for twice the
    /// `typing_hold_seconds` window in force at read time, so a forgotten close
    /// cannot defer delivery forever. Only the instant is stored; the deadline
    /// is recomputed on every read, so a settings change applies immediately.
    manual_closed_at: Option<Instant>,
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

    /// #2379 - the manual padlock window: twice the live `window` since the
    /// close. Evaluated against the `window` passed at read time, never against
    /// a captured or hardcoded duration, so a settings change moves the
    /// deadline of an existing close.
    fn manual_window_active(&self, window: Duration) -> bool {
        self.manual_closed_at
            .is_some_and(|closed_at| closed_at.elapsed() <= window.saturating_mul(2))
    }

    fn effective_closed(&self, window: Duration) -> bool {
        self.manual_window_active(window) || self.natural_window_active(window)
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
    #[cfg(test)]
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
    /// effective-open session takes the manual hold, which itself expires after
    /// 2x `window`. Returns the post-toggle snapshot under the same lock, so the
    /// UI cannot observe a half-flip.
    pub fn toggle_manual(&mut self, id: Uuid, window: Duration) -> TypingHoldSnapshot {
        let session = self.sessions.entry(id).or_default();
        if session.effective_closed(window) {
            session.manual_closed_at = None;
            session.release_generation = session.release_generation.wrapping_add(1);
            session.suppressed_generation = Some(session.release_generation);
        } else {
            session.manual_closed_at = Some(Instant::now());
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

    #[cfg(test)]
    pub(crate) fn backdate_manual_close_for_test(&mut self, id: Uuid, age: Duration) {
        if let Some(closed_at) = self
            .sessions
            .get_mut(&id)
            .and_then(|session| session.manual_closed_at.as_mut())
        {
            *closed_at = Instant::now()
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
mod tests;
