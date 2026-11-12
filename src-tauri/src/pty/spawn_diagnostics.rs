//! #942 - spawn diagnostics for local PTY sessions.
//!
//! Diagnostics only. Nothing in this module changes how a session is spawned,
//! killed or read; it only records evidence so an intermittent coding-agent
//! blank-terminal hang explains itself after the fact.
//!
//! ## Events
//!
//! | Event | Level | When |
//! |---|---|---|
//! | `[pty] spawn-record` | INFO | once, at spawn (fields below) |
//! | `[pty] first-output` | DEBUG | first byte off the PTY |
//! | `[pty] first-paint` | INFO (WARN if it recovers a stall) | output passes the paint floor |
//! | `[pty] startup-stall` | WARN | at the deadline, when the child never painted |
//! | `[pty] startup-checkpoint` | INFO | at the deadline otherwise |
//! | `[pty] child-exit` | INFO (WARN when unexpected) | once, when the child ends |
//!
//! `spawn-record` carries argv as executed, cwd, CLI, profile, `CODEX_HOME` and
//! concurrency.
//!
//! The startup report (`startup-stall` / `startup-checkpoint`) fires for **every**
//! session at the deadline, whatever it painted and whoever launched it, and it always
//! carries the retained head bytes. A hang that gets as far as entering the alt screen
//! and clearing it (easily past the paint floor) would otherwise leave no trace at
//! all, and "alt screen entered, nothing drawn" is exactly what a blank terminal looks
//! like.
//!
//! ## Two identities, and why the CLI one is load-bearing
//!
//! `cli=` is [`CodingAgentKind`], detected from the argv by the canonical detector
//! (`session/profile.rs`). `agent_profile=` is the configured coding-agent profile id
//! (`settings.agents[].id`), an opaque string like `agent_1782513272568_0`. They are
//! not interchangeable: a Codex launch can carry no profile id at all
//! (`commands/session.rs`, `is_agent_owned_launch`), and several distinct profiles can
//! all run the `codex` CLI against the same global `~/.codex`. The stall predicate and
//! the concurrency counter therefore key on the **CLI**; the profile id is logged
//! beside it, never used as the identity.
//!
//! ## Windows caveats this module refuses to paper over
//!
//! - The first bytes off the PTY are never the child. Measured on the shipping spawn
//!   shape (`cmd.exe /C codex ...`, which is what AC does for every npm-installed agent
//!   CLI), **71 bytes arrive within ~45ms with the child still doing nothing**: `ConPTY`'s
//!   own handshake (`ESC[?9001h ESC[?1004h`), its screen setup (`ESC[?25l ESC[2J ESC[m
//!   ESC[H`), and the npm shim's own `title %COMSPEC%` line, which `ConPTY` renders as an
//!   OSC title sequence. The paint floor has to sit ABOVE that band or a hung agent
//!   reads as painted: 64 did not, 256 does. A real coding-agent TUI crosses 256 bytes in
//!   ~350ms when healthy (measured, fresh codex) and takes seconds when it is hanging.
//! - `portable_pty::Child::try_wait` cannot tell "running" from "we cannot query this
//!   handle": `WinChild::is_complete` swallows a failed `GetExitCodeProcess` and
//!   returns `Ok(None)`, and its `STILL_ACTIVE` sentinel (259) is also a legal exit
//!   code. AV/EDR stripping query rights off our handles is a known scenario here
//!   (#647). The Windows probe in `local_backend` therefore calls
//!   `WaitForSingleObject` directly and reports [`ChildLiveness::Unqueryable`] when the
//!   OS refuses to answer. The kill path still uses `try_wait`, so a child that exits
//!   with code 259 stays invisible to it: an upstream limitation, documented here
//!   rather than asserted away.
//!
//! ## Accepted gap: container transport
//!
//! Container sessions produce **nothing** here: no spawn record, no deadline report, no
//! exit event (`container_backend` registers no record, so `mark_ac_stop` is a genuine
//! no-op for them). A containerised coding-agent hang leaves zero evidence. That is an
//! accepted gap: #942 is a local-spawn bug. It never `MISreports` a container session, it
//! simply says nothing about one.
//!
//! ## Exit attribution: where the witness comes from, and the one path without it
//!
//! Every AC stop publishes a **pre-stop witness**: the liveness of the child probed
//! before anything is touched. `kill`, `terminate_job_for_session`, `kill_all_jobs` and
//! the session-destroy path all publish one, so a child that was already dead when AC
//! asked can never be charged to us. The resource-monitor **watchdog** is the exception:
//! it calls `kill_group` straight from `resource_monitor/watchdog.rs` with no access to
//! the PTY child, so it publishes no witness. There the monitor falls back to the stop
//! flag as read before the probe, which means a child that died on its own in the
//! microseconds before the watchdog fired is attributed `ac-requested`. The watchdog
//! only fires on a group that is measurably burning CPU or memory, so the child is alive
//! by construction; the residue is a microsecond-wide race, and it is stated here rather
//! than papered over.
//!
//! One more residual, stated rather than "fixed": an exit inside the attribution window
//! that follows a stop which FAILED (a Quarantined kill leaves the agent running) is
//! attributed to AC. We err this way deliberately. The alternative mislabels a
//! slow-but-successful kill as a spontaneous death, which is the fabrication this module
//! exists to prevent, and `Quarantined` means "AC could not verify the termination", not
//! "the kill failed". `stop_source` and `stop_age_ms` are printed on every exit event, so
//! a human can see a 30-second gap for what it is.
//!
//! ## Lock order
//!
//! `ptys` -> diagnostics registry (`kill_all_jobs` holds the PTY map while tagging
//! records). Never the reverse: the monitor thread probes under `ptys` but only ever
//! touches its own `Arc<SpawnRecord>`, never the registry.
//!
//! ## Tunables (env vars, read once, clamped)
//!
//! | Var | Default | Ceiling |
//! |---|---|---|
//! | `AC_SPAWN_STALL_TIMEOUT_MS` | 5000 (`0` disables the startup report) | 600000 |
//! | `AC_SPAWN_PAINT_FLOOR_BYTES` | 64 | 65536 |
//! | `AC_SPAWN_HEAD_BYTES` | 512 | 65536 |
//! | `AC_SPAWN_CONCURRENCY_WINDOW_MS` | 10000 | 3600000 |
//! | `AC_SPAWN_STOP_ATTRIBUTION_MS` | 60000 | 3600000 |

//! [`CodingAgentKind`]: crate::session::profile::CodingAgentKind

mod registry;
mod thresholds;

pub use registry::{
    forget, is_secret_env_key, mark_ac_stop, note_spawn_attempt, record_for, register, watch_child,
    ChildLiveness, ExitCause, SpawnRecord, SpawnRecordInit, SpawnRegistry, SpawnWindow,
    StopSnapshot,
};
pub use thresholds::Thresholds;
