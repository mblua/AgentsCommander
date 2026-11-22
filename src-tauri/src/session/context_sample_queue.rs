//! Capacity of the context-sample queue: `CONTEXT_SAMPLE_QUEUE_CAPACITY` (1024).
//! Shared by `session::context_alerts` (which re-exports it) and the PTY wiring.
//! Holds only this constant; no queue logic lives here.

pub(crate) const CONTEXT_SAMPLE_QUEUE_CAPACITY: usize = 1024;
