use clap::Args;
use serde::Serialize;

use crate::config::sessions_persistence::{load_sessions_raw, PersistedSession};
use crate::session::session::SessionStatus;

#[derive(Args)]
#[command(after_help = "\
OUTPUT: JSON array of current sessions. Each entry contains:\n  \
  id                Session UUID\n  \
  name              Display name (e.g., \"tech-lead\" or \"Session 1\")\n  \
  workingDirectory  Session's working directory path\n  \
  status            One of: \"active\", \"running\", \"idle\", or {\"exited\": <code>}\n  \
  waitingForInput   true when the session is waiting for user input\n  \
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
    status: serde_json::Value,
    waiting_for_input: bool,
    created_at: String,
    completion_status: String,
}

/// Convert a SessionStatus to its string tag (for filtering).
fn status_tag(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Active => "active",
        SessionStatus::Running => "running",
        SessionStatus::Idle => "idle",
        SessionStatus::Exited(_) => "exited",
    }
}

/// Convert a PersistedSession (with runtime fields) to a CLI output entry.
fn to_entry(s: &PersistedSession) -> SessionEntry {
    let status_value = match &s.status {
        Some(st) => serde_json::to_value(st).unwrap_or(serde_json::json!("unknown")),
        None => serde_json::json!("unknown"),
    };

    SessionEntry {
        id: s.id.clone().unwrap_or_default(),
        name: s.name.clone(),
        working_directory: s.working_directory.clone(),
        status: status_value,
        waiting_for_input: s.waiting_for_input.unwrap_or(false),
        created_at: s.created_at.clone().unwrap_or_default(),
        completion_status: s.completion_status.clone().unwrap_or_else(|| "working".to_string()),
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

    // Read sessions from the persisted file (raw, no deduplication)
    let sessions = load_sessions_raw();

    // Filter out sessions without runtime data (stale restore-only entries)
    // and apply status filter
    let status_filter = args.status.map(|s| s.to_lowercase());

    let entries: Vec<SessionEntry> = sessions
        .iter()
        .filter(|s| s.id.is_some()) // Only include entries with runtime data
        .filter(|s| {
            match (&status_filter, &s.status) {
                (Some(filter), Some(st)) => status_tag(st) == filter.as_str(),
                (Some(_), None) => false,
                (None, _) => true,
            }
        })
        .map(to_entry)
        .collect();

    match serde_json::to_string_pretty(&entries) {
        Ok(json) => {
            println!("{}", json);
            0
        }
        Err(e) => {
            eprintln!("Error: failed to serialize output: {}", e);
            1
        }
    }
}
