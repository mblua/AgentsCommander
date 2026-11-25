//! `list-sessions` CLI verb: reads `sessions.json` directly and prints sessions with runtime
//! data as JSON, optionally filtered by `--status`. Owns `ListSessionsArgs` and the
//! `SessionEntry` output shape, including the derived `raisedHand` flag. Warns on stderr when
//! no running daemon is detected, since the file may then be stale. It does not ask the
//! daemon for live state.

use clap::Args;
use serde::Serialize;

use crate::config::sessions_persistence::{load_sessions_raw, PersistedSession};
use crate::session::session::{SessionCommunicationKind, SessionStatus};

#[derive(Args)]
#[command(after_help = "\
OUTPUT: JSON array of current sessions. Each entry contains:\n  \
  id                Session UUID\n  \
  name              Display name (e.g., \"tech-lead\" or \"Session 1\")\n  \
  workingDirectory  Session's working directory path\n  \
  status            One of: \"active\", \"running\", \"idle\", or {\"exited\": <code>}\n  \
  waitingForInput   true when the session is waiting for user input\n  \
  raisedHand        true when a visible raise-hand request is active (persists across \
app restarts until real user input)\n  \
  createdAt         ISO 8601 timestamp of session creation\n\n\
REQUIREMENTS: The app must be running (sessions.json is kept up-to-date while the app runs).\n\n\
EXAMPLES:\n  \
  {bin} list-sessions\n  \
  {bin} list-sessions --status active")]
pub struct ListSessionsArgs {
    /// Filter by status (active, running, idle, exited)
    #[arg(long)]
    pub status: Option<String>,
}

/// Subset of session fields for CLI output.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionEntry {
    id: String,
    name: String,
    working_directory: String,
    status: EntryStatus,
    waiting_for_input: bool,
    raised_hand: bool,
    created_at: String,
}

/// Output `status`: the persisted status, or the `"unknown"` sentinel when absent.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum EntryStatus {
    Known(SessionStatus),
    Unknown(UnknownStatus),
}

/// Serializes as the string `"unknown"`.
#[derive(Debug, Serialize)]
enum UnknownStatus {
    #[serde(rename = "unknown")]
    Unknown,
}

/// Convert a `SessionStatus` to its string tag (for filtering).
fn status_tag(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Active => "active",
        SessionStatus::Running => "running",
        SessionStatus::Idle => "idle",
        SessionStatus::Exited(_) => "exited",
    }
}

/// Derive the public `raisedHand` boolean from a persisted row.
/// True when the row is a coordinator, has a present status, and carries a
/// visible `RaiseHand` communication. `Exited` status is deliberately NOT a
/// disqualifier: a dormant restored coordinator legitimately keeps
/// its persisted raised hand until real user input clears it. Every real-exit
/// path clears `communication` in the same critical section (`mark_exited`),
/// so an exited row with a visible hand can only be a dormant restore.
/// Missing status, missing/hidden/non-raise communication, non-coordinator,
/// and restore-only rows map to false.
fn raised_hand_from_persisted(s: &PersistedSession) -> bool {
    if s.status.is_none() || !s.is_coordinator {
        return false;
    }
    s.communication.as_ref().is_some_and(|communication| {
        communication.kind == SessionCommunicationKind::RaiseHand && communication.visible
    })
}

/// Convert a `PersistedSession` (with runtime fields) to a CLI output entry.
fn to_entry(s: &PersistedSession) -> SessionEntry {
    let status = match &s.status {
        Some(st) => EntryStatus::Known(st.clone()),
        None => EntryStatus::Unknown(UnknownStatus::Unknown),
    };

    SessionEntry {
        id: s.id.clone().unwrap_or_default(),
        name: s.name.clone(),
        working_directory: s.working_directory.clone(),
        status,
        waiting_for_input: s.waiting_for_input.unwrap_or(false),
        raised_hand: raised_hand_from_persisted(s),
        created_at: s.created_at.clone().unwrap_or_default(),
    }
}

pub fn execute(args: ListSessionsArgs) -> i32 {
    // Validate status filter
    if let Some(ref status) = args.status {
        let valid = ["active", "running", "idle", "exited"];
        if !valid.contains(&status.to_lowercase().as_str()) {
            eprintln!(
                "Error: invalid status '{}'. Must be one of: {}",
                status,
                valid.join(", ")
            );
            return 1;
        }
    }

    // Detect whether the daemon is running before trusting
    // sessions.json. Warn on stderr if not — preserve stdout JSON and exit 0.
    //
    // When the caller filters by `--status exited`, the natural expectation is
    // that the daemon may already be dead (exited sessions are the leftover
    // state). Suppress the stale-daemon warning in that case so the consumer
    // does not get noise on a query whose entire purpose is post-mortem
    // inspection.
    let suppress_stale_warning = args
        .status
        .as_deref()
        .is_some_and(|s| s.eq_ignore_ascii_case("exited"));
    if !suppress_stale_warning {
        match crate::config::daemon_pid::detect_daemon_state() {
            crate::config::daemon_pid::DaemonState::Running { .. } => {}
            crate::config::daemon_pid::DaemonState::NoPidFile => {
                if crate::config::environment::machine_output_string().is_err() {
                    eprintln!(
                        "[WARN] sessions.json may be stale — no AgentsCommander daemon detected \
                         (no daemon.pid file). list-sessions reads the file directly; results may \
                         not reflect current state."
                    );
                } else {
                    log::warn!(
                        "sessions.json may be stale — no AgentsCommander daemon detected \
                         (no daemon.pid file). list-sessions reads the file directly; results may \
                         not reflect current state."
                    );
                }
            }
            crate::config::daemon_pid::DaemonState::StalePidFile { pid } => {
                if crate::config::environment::machine_output_string().is_err() {
                    eprintln!(
                        "[WARN] sessions.json may be stale — AgentsCommander daemon pid {pid} \
                         is not running (stale daemon.pid file). list-sessions reads the file \
                         directly; results reflect the daemon's last persisted state."
                    );
                } else {
                    log::warn!(
                        "sessions.json may be stale — AgentsCommander daemon pid {pid} is not \
                         running (stale daemon.pid file). list-sessions reads the file directly; \
                         results reflect the daemon's last persisted state."
                    );
                }
            }
            crate::config::daemon_pid::DaemonState::MalformedPidFile => {
                if crate::config::environment::machine_output_string().is_err() {
                    eprintln!(
                        "[WARN] sessions.json may be stale — daemon.pid file is malformed. \
                         list-sessions reads the file directly; results may not reflect \
                         current state."
                    );
                } else {
                    log::warn!(
                        "sessions.json may be stale — daemon.pid file is malformed. \
                         list-sessions reads the file directly; results may not reflect \
                         current state."
                    );
                }
            }
        }
    }

    // Read sessions from the persisted file (raw, no deduplication)
    let sessions = load_sessions_raw();

    // Filter out sessions without runtime data (stale restore-only entries)
    // and apply status filter
    let status_filter = args.status.map(|s| s.to_lowercase());

    let entries: Vec<SessionEntry> = sessions
        .iter()
        .filter(|s| s.id.is_some()) // Only include entries with runtime data
        .filter(|s| match (&status_filter, &s.status) {
            (Some(filter), Some(st)) => status_tag(st) == filter.as_str(),
            (Some(_), None) => false,
            (None, _) => true,
        })
        .map(to_entry)
        .collect();

    match serde_json::to_string_pretty(&entries) {
        Ok(json) => {
            crate::cli_println!("{}", json);
            0
        }
        Err(e) => {
            eprintln!("Error: failed to serialize output: {e}");
            1
        }
    }
}
