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

/// What a backend can say about one session's rows.
///
/// THREE states, because the oracle behind it has three. A two-state channel here would
/// make `None` mean four different things - session unknown, parser poisoned, child dead,
/// child unqueryable - of which "stop sampling" is right for three and destroys the fourth:
/// a live child whose handle could not be queried for one tick would be deregistered for
/// the rest of its life. `ChildLiveness::Unqueryable` exists precisely so "we could not
/// ask" is never confused with a definite answer, and this enum is what carries that the
/// rest of the way.
pub enum ScreenRowsRead {
    /// The live grid's rows.
    Rows(Vec<String>),
    /// No reading this tick. Says NOTHING about whether the session is over: retry next
    /// tick, keep the entry. (Child alive but unqueryable; parser missing or poisoned.)
    Unavailable,
    /// The session is over. Emit null once, then stop sampling it.
    SessionOver,
}
