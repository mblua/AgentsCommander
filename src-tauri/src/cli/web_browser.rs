//! `web-browser` CLI verb (#3367): drives the embedded web browser shown over
//! the terminal pane. Delivered through the outbox like `raise-hand`.

use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::commands::web_browser::{
    WEB_BROWSER_NAVIGATE_ACTION, WEB_BROWSER_RELOAD_ACTION, WEB_BROWSER_STATUS_ACTION,
};
use crate::filesystem::{directories, documents};
use crate::phone::types::OutboxMessage;

use super::self_clear::resolve_self_clear_sender;

const POLL: Duration = Duration::from_millis(250);
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Args)]
#[command(after_help = "\
Drives the web browser that AgentsCommander shows over the terminal pane (WWW button). \
Every live session may read its status; only room orchestrators may navigate or reload. \
navigate also brings the browser view up when it is hidden.\n\n\
OUTPUT: prints the daemon's JSON response on stdout: action, status, url, title, loading \
and visible. Rejections, stale tokens and timeouts exit non-zero and write errors to stderr.")]
pub struct WebBrowserArgs {
    #[command(subcommand)]
    pub action: WebBrowserAction,
}

#[derive(Subcommand)]
pub enum WebBrowserAction {
    /// Load a URL (http, https, file, or an absolute file path). Orchestrators only.
    Navigate {
        #[command(flatten)]
        session: SessionArgs,
        /// URL or absolute file path to load
        #[arg(long)]
        url: String,
    },
    /// Reload the current page. Orchestrators only.
    Reload {
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Print the current URL, title, loading and visibility. Any agent.
    Status {
        #[command(flatten)]
        session: SessionArgs,
    },
}

#[derive(Args)]
pub struct SessionArgs {
    /// Session token from AGENTSCOMMANDER_TOKEN
    #[arg(long)]
    pub token: Option<String>,
    /// Agent root directory (required). Your working directory.
    #[arg(long)]
    pub root: Option<String>,
    /// Seconds to wait for the daemon's response (default 15)
    #[arg(long, default_value = "15")]
    pub timeout: u64,
}

pub fn execute(args: WebBrowserArgs) -> i32 {
    let (session, action, body) = match args.action {
        WebBrowserAction::Navigate { session, url } => (session, WEB_BROWSER_NAVIGATE_ACTION, url),
        WebBrowserAction::Reload { session } => (session, WEB_BROWSER_RELOAD_ACTION, String::new()),
        WebBrowserAction::Status { session } => (session, WEB_BROWSER_STATUS_ACTION, String::new()),
    };
    match request(session, action, body) {
        Ok(response) => {
            crate::cli_println!("{}", response.trim());
            0
        }
        Err((code, message)) => {
            eprintln!("Error: {message}");
            code
        }
    }
}

fn outbox_message(
    token: Option<String>,
    from: String,
    action: &str,
    body: String,
) -> OutboxMessage {
    OutboxMessage {
        id: Uuid::new_v4().to_string(),
        token,
        from,
        to: String::new(),
        body,
        mode: String::new(),
        get_output: false,
        request_id: Some(Uuid::new_v4().to_string()),
        sender_agent: None,
        preferred_agent: String::new(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        command: None,
        action: Some(action.to_string()),
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    }
}

fn outbox_dir(ac_dir: &Path, is_root: bool) -> PathBuf {
    crate::filesystem::app_outbox_pointer::directory(
        is_root.then(crate::config::config_dir).flatten().as_deref(),
        ac_dir,
    )
}

fn request(session: SessionArgs, action: &str, body: String) -> Result<String, (i32, String)> {
    let root = session.root.ok_or((1, "--root is required.".to_string()))?;
    let (_, is_root) = crate::cli::validate_cli_token(&session.token).map_err(|m| (1, m))?;
    let ac_dir = PathBuf::from(&root).join(crate::config::agent_local_dir_name());
    let message = outbox_message(
        session.token,
        resolve_self_clear_sender(&root),
        action,
        body,
    );
    let outbox = outbox_dir(&ac_dir, is_root);
    directories::ensure_cli_outbox(&outbox)
        .map_err(|e| (1, format!("failed to create outbox directory: {e}")))?;
    let json = serde_json::to_string_pretty(&message)
        .map_err(|e| (1, format!("failed to serialize message: {e}")))?;
    let outbox_file = outbox.join(format!("{}.json", message.id));
    documents::write_cli_outbox_message(&outbox_file, json.as_bytes())
        .map_err(|e| (1, format!("failed to write outbox file: {e}")))?;

    let delivered = outbox
        .join("delivered")
        .join(format!("{}.json", message.id));
    let rejected = outbox
        .join("rejected")
        .join(format!("{}.reason.txt", message.id));
    let start = Instant::now();
    while !directories::cli_outbox_artifact_exists(&delivered) {
        if directories::cli_outbox_artifact_exists(&rejected) {
            let reason = documents::read_cli_outbox_text(&rejected)
                .unwrap_or_else(|_| "unknown reason".to_string());
            return Err((1, format!("{action} rejected - {}", reason.trim())));
        }
        if start.elapsed() >= CONFIRM_TIMEOUT {
            let id = &message.id;
            return Err((
                1,
                format!("delivery confirmation timeout after 30s (request {id})"),
            ));
        }
        std::thread::sleep(POLL);
    }

    let request_id = message.request_id.unwrap_or_default();
    let response = ac_dir.join("responses").join(format!("{request_id}.json"));
    let start = Instant::now();
    while !directories::cli_outbox_artifact_exists(&response) {
        if start.elapsed() >= Duration::from_secs(session.timeout) {
            let secs = session.timeout;
            return Err((
                2,
                format!("{action} delivered but no response within {secs}s"),
            ));
        }
        std::thread::sleep(POLL);
    }
    documents::read_cli_outbox_text(&response)
        .map_err(|e| (1, format!("failed to read response: {e}")))
}
