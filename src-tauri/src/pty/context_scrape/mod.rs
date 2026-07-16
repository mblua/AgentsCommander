//! #1032 - a per-session, best-effort reading of a coding agent's context-window usage,
//! taken by running a per-agent, user-configured regex over the plain de-ANSI'd rows of the
//! `vt100` screen mirror AC already keeps for every session.
//!
//! **The percentage is a signal for a human. It never drives an action.** That is not a
//! rule anyone here has to remember: the scraper holds three narrow trait objects and
//! nothing else - no `AppHandle`, no `PtyManager` - so the capability to write to a PTY or
//! kill a session is not reachable from this module at all.

pub mod pattern;
pub mod rows;
