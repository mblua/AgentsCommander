//! Voice recording tracker: `VoiceTracker` (shared as `VoiceTrackingState`) records which
//! sessions are recording voice input and whether they got PTY writes meanwhile.
//! It does not capture or transcribe audio.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Tracks which sessions are actively voice-recording so that
/// `pty_write` can detect user typing during a recording session.
pub struct VoiceTracker {
    /// Sessions currently recording voice input.
    recording: HashSet<Uuid>,
    /// Sessions that received PTY writes while recording was active.
    typed: HashSet<Uuid>,
}

pub type VoiceTrackingState = Arc<Mutex<VoiceTracker>>;

impl Default for VoiceTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceTracker {
    pub fn new() -> Self {
        Self {
            recording: HashSet::new(),
            typed: HashSet::new(),
        }
    }

    pub fn set_recording(&mut self, id: Uuid, active: bool) {
        if active {
            self.recording.insert(id);
        } else {
            self.recording.remove(&id);
        }
    }

    pub fn is_recording(&self, id: Uuid) -> bool {
        self.recording.contains(&id)
    }

    pub fn mark_typed(&mut self, id: Uuid) {
        self.typed.insert(id);
    }

    /// Returns `true` if the session had PTY writes during recording,
    /// and clears the flag so the next recording starts clean.
    pub fn drain_typed(&mut self, id: Uuid) -> bool {
        self.typed.remove(&id)
    }
}
