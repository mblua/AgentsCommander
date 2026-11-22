//! Groups the phone command helper submodules: `mailbox`, `pty_input`, `response` and
//! `terminal_snapshot`. Declares submodules only; no commands or state live here.
//! All behavior lives in those submodules, each `pub(crate)` to this crate.

pub(crate) mod mailbox;
pub(crate) mod pty_input;
pub(crate) mod response;
pub(crate) mod terminal_snapshot;
