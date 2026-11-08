//! `CodingAgentKind` definition, split out so `pty_submission` can name it
//! without depending on its parent module (#4008).

use serde::{Deserialize, Serialize};

/// Identity of a coding agent. `Option<CodingAgentKind>` on a session: `None`
/// means "not a recognised coding agent" (a plain shell).
///
/// Mutual exclusion is **structural** — a session is exactly one kind or none.
/// This enum is what let #260 delete the `debug_assert!` that guarded the old
/// three-bool representation in `derive_reader`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingAgentKind {
    Claude,
    Codex,
    Antigravity,
    Pi,
    /// #1873 - Muse Code. Detected only as a direct shell whose exact stem is
    /// `muse` (bare or absolute path); prefix names and argument-only identity
    /// inside `env`/shell/PowerShell/`cmd`/compound wrappers are never Muse.
    Muse,
}
