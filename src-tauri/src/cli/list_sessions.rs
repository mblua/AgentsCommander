use clap::Args;
use serde::{Deserialize, Serialize};

#[derive(Args)]
#[command(after_help = "\
OUTPUT: JSON array of current sessions in the running app instance. Each entry contains:\n  \
  id                Session UUID\n  \
  name              Display name (e.g., \"tech-lead\" or \"Session 1\")\n  \
  workingDirectory  Session's working directory path\n  \
  status            One of: \"active\", \"running\", \"idle\", or {\"exited\": <code>}\n  \
  waitingForInput   true when the session is waiting for user input\n  \
  createdAt         ISO 8601 timestamp of session creation\n\n\
REQUIREMENTS: The app must be running with the web server enabled.\n\
The CLI reads the web server port and token from the config directory.\n\
Use --port or --token to override.\n\n\
EXAMPLES:\n  \
  {bin} list-sessions\n  \
  {bin} list-sessions --status active\n  \
  {bin} list-sessions --port 9876")]
pub struct ListSessionsArgs {
    /// Filter by status (active, running, idle, exited)
    #[arg(long)]
    pub status: Option<String>,

    /// Web server port override (reads from settings by default)
    #[arg(long)]
    pub port: Option<u16>,

    /// Web access token override (reads from config dir web-token.txt by default)
    #[arg(long)]
    pub token: Option<String>,
}

/// Subset of session fields for CLI output.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionEntry {
    id: String,
    name: String,
    working_directory: String,
    status: serde_json::Value,
    waiting_for_input: bool,
    created_at: String,
}

/// Read the web access token from the config directory.
fn read_web_token() -> Option<String> {
    let token_path = crate::config::config_dir()?.join("web-token.txt");
    std::fs::read_to_string(token_path).ok().map(|s| s.trim().to_string())
}

pub fn execute(args: ListSessionsArgs) -> i32 {
    // Determine web server port
    let port = args.port.unwrap_or_else(|| {
        let settings = crate::config::settings::load_settings();
        settings.web_server_port
    });

    // Resolve web access token
    let token = args.token.or_else(read_web_token);

    // Validate status filter before making request
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

    // Build request with proper query encoding
    let url = format!("http://127.0.0.1:{}/api/sessions", port);
    let client = reqwest::blocking::Client::new();
    let mut req = client.get(&url);
    if let Some(ref t) = token {
        req = req.query(&[("token", t)]);
    }
    if let Some(ref status) = args.status {
        req = req.query(&[("status", status)]);
    }

    // Query the running app
    let response = match req.send() {
        Ok(r) => r,
        Err(e) => {
            if e.is_connect() {
                eprintln!(
                    "Error: could not connect to app on port {}. \
                     Is the app running with the web server enabled?",
                    port
                );
            } else {
                eprintln!("Error: HTTP request failed: {}", e);
            }
            return 1;
        }
    };

    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        eprintln!("Error: unauthorized. Could not read web token from config directory.");
        return 1;
    }

    if !response.status().is_success() {
        eprintln!("Error: server returned {}", response.status());
        return 1;
    }

    // Parse the API response and project to our subset
    let sessions: Vec<serde_json::Value> = match response.json() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: failed to parse response: {}", e);
            return 1;
        }
    };

    let entries: Vec<SessionEntry> = sessions
        .into_iter()
        .map(|s| SessionEntry {
            id: s.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            name: s.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            working_directory: s
                .get("workingDirectory")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            status: s.get("status").cloned().unwrap_or(serde_json::json!("unknown")),
            waiting_for_input: s
                .get("waitingForInput")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            created_at: s
                .get("createdAt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        })
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
