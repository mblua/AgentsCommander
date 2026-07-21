use clap::Args;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::teams;
use crate::phone::types::OutboxMessage;

#[derive(Args)]
#[command(after_help = "\
DELIVERY MODES:\n  \
  wake            File messages inject into PTY and can spawn or respawn a persistent session. Logical PTY actions are capability- and idle-gated and can be terminally rejected before spawn.\n\n\
ROUTING: Before delivery, the CLI validates that the sender can reach the destination based on team \
membership and coordinator rules (teams.json). If routing fails, the CLI exits immediately with code 1.\n\n\
DISCOVERY: Use `list-peers-lean` to get valid agent names for --to. The \"name\" field in the JSON output \
is the value to use.\n\n\
FILE-BASED MESSAGING: --send <filename> delivers a Markdown file. For WG \
replicas the file is resolved from <workgroup-root>/messaging/<filename>. \
For the Root Agent the file is resolved from <root-agent-dir>/messaging/<filename>. \
`--send` is a filename only, never a path. Root Agent --to targets must be \
verified WG coordinator replica names returned by list-peers-lean. \
Coordinator --to targets may include the Root Agent canonical name \
`agentscommander://root-agent`; only identity-verified WG coordinator \
replicas may use it.\n\n\
DELIVERY CONFIRMATION: After queuing, send blocks up to --confirm-timeout seconds (default 90) \
waiting for the app's poller to confirm delivery. This bounds ONLY the synchronous confirmation \
handshake, not delivery itself: on confirmation timeout the CLI exits 1, but the message remains \
durably queued in the outbox and is typically still delivered afterwards (e.g. when wake must \
cold-spawn an idle peer). Exit 1 on confirmation timeout does NOT mean the message was lost; \
verify the outbox instead of re-sending. --confirm-timeout is distinct from --timeout, which \
bounds only the --get-output response wait.")]
pub struct SendArgs {
    /// Session token from AGENTSCOMMANDER_TOKEN. Shape-validated in the CLI;
    /// per-session authorization happens at the daemon mailbox. See `--help` TOKEN VALIDATION MODEL.
    #[arg(long)]
    pub token: Option<String>,

    /// Destination agent name (e.g., "repos/my-project"). Use `list-peers-lean` to discover valid names
    #[arg(long)]
    pub to: String,

    /// Filename (not path) of a message file that already exists in
    /// <workgroup-root>/messaging/. Sender writes the file BEFORE calling send.
    /// Cannot be combined with --command.
    #[arg(long, conflicts_with = "command")]
    pub send: Option<String>,

    /// Delivery mode (see DELIVERY MODES below)
    #[arg(long, default_value = "wake")]
    pub mode: String,

    /// Wait for and return the agent's response (blocks until reply or --timeout)
    #[arg(long)]
    pub get_output: bool,

    /// Logical PTY action [possible values: clear, compact]. `clear` starts a
    /// fresh conversation: /new for an exact-stem direct Pi shell and /clear
    /// for direct Claude/Codex/Gemini-family or Cursor agent shells. Pi compact
    /// is unsupported. The mapped session must be idle; unsupported mappings
    /// are terminally rejected before spawn. Not available from the Root Agent.
    /// Cannot be combined with --send
    #[arg(long)]
    pub command: Option<String>,

    /// Configured agent id to use when `wake` spawns a new persistent session for
    /// the destination. `auto` picks the session's saved `lastCodingAgent`.
    #[arg(long, default_value = "auto")]
    pub agent: String,

    /// Timeout in seconds for the --get-output response wait (the
    /// delivery-confirmation wait is bounded separately by --confirm-timeout)
    #[arg(long, default_value = "300")]
    pub timeout: u64,

    /// Timeout in seconds for the delivery-confirmation wait, distinct from
    /// --timeout (which bounds the --get-output response wait). Bounds only the
    /// synchronous confirmation handshake: on timeout the CLI exits 1 but the
    /// message remains durably queued in the outbox and is typically still
    /// delivered. Exit 1 on confirmation timeout does NOT mean the message was
    /// lost; verify the outbox instead of re-sending
    #[arg(long, default_value = "90")]
    pub confirm_timeout: u64,

    /// Agent root directory (required). Your working directory — used to derive your agent name
    #[arg(long)]
    pub root: Option<String>,

    /// Write message to a specific outbox directory instead of <root>/<local-dir>/outbox/
    #[arg(long)]
    pub outbox: Option<String>,
}

/// Derive agent FQN from a path. Delegates to the canonical
/// `config::teams::agent_fqn_from_path` so WG replicas produce
/// `<project>:<wg>/<agent>` and origin agents produce `<project>/<agent>`.
///
/// Single source of truth — keep as a thin wrapper rather than a shadow copy.
pub(crate) fn agent_name_from_root(root: &str) -> String {
    crate::config::teams::agent_fqn_from_path(root)
}

pub(crate) fn sender_for_root(root: &str, root_is_root_agent: bool) -> String {
    if root_is_root_agent {
        crate::config::root_agent::ROOT_AGENT_SENDER.to_string()
    } else {
        agent_name_from_root(root)
    }
}

fn root_agent_target_allowed(target: &str, project_paths: &[String]) -> bool {
    crate::config::teams::verified_wg_coordinator_target(target, project_paths).is_some()
}

fn coordinator_to_root_target_allowed(sender: &str, project_paths: &[String]) -> bool {
    crate::config::teams::verified_wg_coordinator_target(sender, project_paths).is_some()
}

fn validate_root_agent_delivery_kind(
    root_is_root_agent: bool,
    command: Option<&str>,
) -> Result<(), &'static str> {
    if root_is_root_agent && command.is_some() {
        Err("Root Agent messaging is file-based; use --send with a root-to-coordinator Markdown file, not --command")
    } else {
        Ok(())
    }
}

/// Delivery-confirmation ceiling for this invocation: `--confirm-timeout`
/// seconds, default 90 (#782). Single mapping point from the parsed flag to
/// the `Duration` that `execute` hands to `wait_for_delivery_confirmation`,
/// so tests can lock that an override actually reaches the helper.
fn confirm_timeout_from_args(args: &SendArgs) -> std::time::Duration {
    std::time::Duration::from_secs(args.confirm_timeout)
}

fn wait_for_delivery_confirmation(
    outbox_dir: &Path,
    msg_id: &str,
    mode_for_ack: &str,
    to_for_ack: &str,
    confirm_timeout: std::time::Duration,
    confirm_poll: std::time::Duration,
) -> Result<(), String> {
    let delivered_path = outbox_dir
        .join("delivered")
        .join(format!("{}.json", msg_id));
    let rejected_reason_path = outbox_dir
        .join("rejected")
        .join(format!("{}.reason.txt", msg_id));
    let start = std::time::Instant::now();

    loop {
        if delivered_path.exists() {
            crate::cli_println!(
                "Delivered: {} (mode={}, to={})",
                msg_id,
                mode_for_ack,
                to_for_ack
            );
            return Ok(());
        }
        if rejected_reason_path.exists() {
            let reason = std::fs::read_to_string(&rejected_reason_path)
                .unwrap_or_else(|_| "unknown reason".to_string());
            return Err(format!("message rejected: {}", reason.trim()));
        }
        if start.elapsed() >= confirm_timeout {
            return Err(format!(
                "delivery confirmation timeout after {}s (message {} may still be pending in {})",
                confirm_timeout.as_secs(),
                msg_id,
                outbox_dir.display()
            ));
        }
        std::thread::sleep(confirm_poll);
    }
}

/// Poll for the `--get-output` response file, returning its contents once it
/// appears. Mirrors `wait_for_delivery_confirmation`: the file is checked BEFORE
/// the timeout, so a response that lands in the final poll window (or with
/// `timeout == 0`) is still read instead of being dropped. Returns `Err` on
/// timeout or a read failure.
fn wait_for_response(
    response_path: &Path,
    timeout: std::time::Duration,
    poll_interval: std::time::Duration,
) -> Result<String, String> {
    let resp_start = std::time::Instant::now();

    loop {
        if response_path.exists() {
            return std::fs::read_to_string(response_path)
                .map_err(|e| format!("failed to read response file: {}", e));
        }
        if resp_start.elapsed() >= timeout {
            return Err(format!(
                "timeout waiting for response after {}s",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(poll_interval);
    }
}

/// If `root` lives inside `<project_dir>/<Project AC Root>/wg-<N>-*/__agent_*/`,
/// return `project_dir` as a UTF-8 `String` (`None` if the shape does not match
/// or `project_dir` is not valid UTF-8, matching `list_peers::detect_wg_replica`
/// which also uses `to_str()` rather than `to_string_lossy()`).
///
/// Thin wrapper over `config::workspace::wg_replica_layout_from_agent_dir`, the
/// single source of the WG-replica walk-up shared with
/// `list_peers::detect_wg_replica` and
/// `phone::mailbox::derive_project_from_outbox_path`, so `send` resolves WG-peer
/// targets against the same source `list-peers` reports as `reachable: true`.
/// See #228 / #726.
fn derive_root_project_dir(root: &str) -> Result<Option<String>, String> {
    let Some(canon) = std::fs::canonicalize(root).ok() else {
        return Ok(None);
    };
    match crate::config::workspace::wg_replica_layout_from_agent_dir(&canon)? {
        Some(layout) => Ok(layout.project_dir.to_str().map(|path| path.to_string())),
        None => Ok(None),
    }
}

fn ensure_workgroup_root_is_authoritative(wg_root: &Path) -> Result<(), String> {
    let workspace_dir = wg_root.parent().ok_or_else(|| {
        format!(
            "workgroup root '{}' has no parent Project AC Root directory",
            wg_root.display()
        )
    })?;
    crate::config::workspace::ensure_authoritative_workspace_dir(workspace_dir)
}

/// v4 UUID string length. Request ids are `Uuid::new_v4().to_string()`, so the
/// `--get-output` response markers embed two 36-char ids.
const REQUEST_ID_LEN: usize = 36;

/// Byte width the injected PTY notification adds around the message body: the
/// plain wrap plus the sender name. The `get_output` branch additionally sizes
/// the `--get-output` response-marker framing (`PTY_RESPONSE_MARKER_FIXED` plus
/// two request ids), but that framing only reaches the PTY on a non-interactive
/// session (`phone::mailbox` gates it on `!interactive`, unreachable since 0.7.0).
/// The live clamp therefore calls this with `get_output = false`; the `true`
/// branch is inert future-proofing for that not-yet-reachable marker path and is
/// exercised only by the contract tests.
fn notification_pty_overhead(sender_len: usize, get_output: bool) -> usize {
    let mut overhead = crate::phone::messaging::PTY_WRAP_FIXED + sender_len;
    if get_output {
        overhead += crate::phone::messaging::PTY_RESPONSE_MARKER_FIXED + 2 * REQUEST_ID_LEN;
    }
    overhead
}

pub fn execute(args: SendArgs) -> i32 {
    let root = match args.root {
        Some(ref r) => r.clone(),
        None => {
            eprintln!("Error: --root is required. Specify your agent's root directory.");
            return 1;
        }
    };
    let root_is_root_agent = crate::config::root_agent::is_root_agent_path(&root);
    if let Err(reason) =
        validate_root_agent_delivery_kind(root_is_root_agent, args.command.as_deref())
    {
        eprintln!("Error: {}", reason);
        return 1;
    }

    // Validate token before proceeding
    let is_root = match crate::cli::validate_cli_token(&args.token) {
        Ok((_token, root)) => root,
        Err(msg) => {
            eprintln!("{}", msg);
            return 1;
        }
    };

    let sender = sender_for_root(&root, root_is_root_agent);
    let ac_dir = PathBuf::from(&root).join(crate::config::agent_local_dir_name());

    // Validate mode — "queue" is no longer supported
    let valid_modes = ["wake"];
    if !valid_modes.contains(&args.mode.as_str()) {
        eprintln!(
            "Error: invalid mode '{}'. Valid: {}",
            args.mode,
            valid_modes.join(", ")
        );
        return 1;
    }

    // ── Resolve --to against known projects (Decision 2 / §AR2-shared) ─────
    //
    // Qualified FQN → validated shape + existence. Unqualified WG-local →
    // two-level scan, unambiguous → canonical FQN, ambiguous → reject with
    // candidate list, unknown → reject. Origin/bare → pass through.
    //
    // CLI-side resolution is belt-and-braces (§DR1); the mailbox also
    // canonicalizes on receive (§AR2-norm) so direct outbox writes cannot
    // bypass the reject-on-ambiguity rule.
    let settings = crate::config::settings::load_settings();
    // Build an in-memory project-path slice that includes the project
    // derived from --root (if any). Mirrors list-peers's WG-replica walk-up
    // discovery so qualified WG-peer targets that list-peers reports as
    // reachable do not fail send with `not found in any known project`.
    // settings.json is NOT modified. See #228 / plan _plans/228-cli-daemon-laterals.md.
    let mut effective_project_paths = settings.project_paths.clone();
    let root_project = match derive_root_project_dir(&root) {
        Ok(root_project) => root_project,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    if let Some(root_project) = root_project {
        // Canonicalize the target once. Compare by string first and only fall
        // back to a per-path canonicalize syscall when the strings differ, so the
        // common "project already registered" case does no extra filesystem work.
        // If canonicalizing `root_project` itself fails (rare: it resolved
        // milliseconds ago inside `derive_root_project_dir`), the string check
        // still catches an exact duplicate.
        let canon_root_project = std::fs::canonicalize(&root_project).ok();
        let already_present = effective_project_paths.iter().any(|p| {
            if p == &root_project {
                return true;
            }
            match &canon_root_project {
                Some(canon_target) => std::fs::canonicalize(p).ok().as_ref() == Some(canon_target),
                None => false,
            }
        });
        if !already_present {
            effective_project_paths.push(root_project);
        }
    }
    let resolved_to =
        match crate::config::teams::resolve_agent_target(&args.to, &effective_project_paths) {
            Ok(fqn) => fqn,
            Err(e) => {
                eprintln!("Error: {}", e);
                return 1;
            }
        };

    // ── Pre-validate routing ──────────────────────────────────────────────

    if root_is_root_agent {
        if !root_agent_target_allowed(&resolved_to, &effective_project_paths) {
            eprintln!(
                "Error: root-agent routing rejected — '{}' is not a verified WG coordinator replica. Use list-peers-lean from the Root Agent and pass one of its name values.",
                resolved_to
            );
            return 1;
        }
    } else if crate::config::root_agent::is_root_agent_target(&resolved_to) {
        // #293 — coordinator → root. Only identity-verified WG coordinators
        // are allowed to address the Root Agent. Master/root token still goes
        // through this gate intentionally: the URI is meaningful only when
        // paired with a real coordinator identity, and the verified check is
        // cheap.
        if !coordinator_to_root_target_allowed(&sender, &effective_project_paths) {
            eprintln!(
                "Error: routing rejected — '{}' is not a verified WG coordinator replica and cannot message '{}'. Replies to the Root Agent are reserved for verified WG coordinators.",
                sender,
                crate::config::root_agent::ROOT_AGENT_SENDER
            );
            return 1;
        }
    } else if !is_root {
        // Load discovered teams and check if sender can reach destination BEFORE
        // writing to outbox. Fail immediately with a clear error if not.
        let discovered = teams::discover_teams();
        if !teams::can_communicate(&sender, &resolved_to, &discovered) {
            eprintln!(
                "Error: routing rejected — '{}' cannot reach '{}'. \
                 Check team membership and coordinator rules.",
                sender, resolved_to
            );
            return 1;
        }
    }

    // --send + --command mutually exclusive (P0-3)
    if args.send.is_some() && args.command.is_some() {
        eprintln!("Error: --send and --command are mutually exclusive");
        return 1;
    }

    // Resolve message body from --send (file-based messaging per plan §4.1 [r2])
    let message_body = if let Some(ref filename) = args.send {
        if let Err(e) = crate::phone::messaging::validate_filename_only(filename) {
            eprintln!("Error: {}", e);
            return 1;
        }
        let agent_root_path = std::path::Path::new(&root);
        let msg_dir = if root_is_root_agent {
            match crate::phone::messaging::root_messaging_dir(agent_root_path) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("Error: failed to resolve root messaging dir: {}", e);
                    return 1;
                }
            }
        } else {
            let wg_root = match crate::phone::messaging::workgroup_root(agent_root_path) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!(
                        "Error: --send requires --root under a wg-<N>-* ancestor unless --root is the canonical Root Agent directory; {}",
                        e
                    );
                    return 1;
                }
            };
            if let Err(e) = ensure_workgroup_root_is_authoritative(&wg_root) {
                eprintln!("Error: {}", e);
                return 1;
            }
            match crate::phone::messaging::messaging_dir(&wg_root) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("Error: failed to resolve messaging dir: {}", e);
                    return 1;
                }
            }
        };
        let abs = match crate::phone::messaging::resolve_existing_message(&msg_dir, filename) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Error: {}", e);
                return 1;
            }
        };

        // UNC-strip only at the single emission site (plan §13.4).
        let abs_str = abs.to_string_lossy();
        let abs_display = abs_str.trim_start_matches(r"\\?\");
        let body = crate::phone::messaging::format_file_notification(abs_display);

        // Pre-wrap long-body warn (plan §13.4 §7.9).
        if body.len() > 200 {
            log::warn!(
                "[send] notification body length {} is unusually long",
                body.len()
            );
        }

        // PTY_SAFE_MAX clamp. The wrap no longer embeds wg_root or bin_path, so
        // the overhead is the fixed framing plus `from`. The live wake path never
        // injects the `--get-output` response markers (`phone::mailbox` gates them
        // on a non-interactive session, unreachable today), so budget the plain
        // wrap here by passing `false`. Keying this on `args.get_output` would flip
        // a near-limit long path from delivered to rejected over marker bytes that
        // never reach the PTY.
        let overhead = notification_pty_overhead(sender.len(), false);
        if body.len() + overhead > crate::phone::messaging::PTY_SAFE_MAX {
            eprintln!(
                "Error: notification exceeds PTY-safe length (body {} + overhead {} > {}). \
                 Shorten slug or move workgroup to a shallower path.",
                body.len(),
                overhead,
                crate::phone::messaging::PTY_SAFE_MAX
            );
            return 1;
        }

        body
    } else {
        String::new()
    };

    // Require at least --send or --command
    if message_body.is_empty() && args.command.is_none() {
        eprintln!("Error: --send or --command is required");
        return 1;
    }

    // Validate --command if present
    const ALLOWED_COMMANDS: &[&str] = &["clear", "compact"];
    if let Some(ref cmd) = args.command {
        if !ALLOWED_COMMANDS.contains(&cmd.as_str()) {
            eprintln!(
                "Error: unsupported command '{}'. Allowed: {}",
                cmd,
                ALLOWED_COMMANDS.join(", ")
            );
            return 1;
        }
    }

    let msg_id = Uuid::new_v4().to_string();
    let request_id = if args.get_output {
        Some(Uuid::new_v4().to_string())
    } else {
        None
    };

    let mode_for_ack = args.mode.clone();
    let to_for_ack = resolved_to.clone();
    // Resolve before `args` fields move into OutboxMessage below; the whole-struct
    // borrow would be rejected after the partial moves.
    let confirm_timeout = confirm_timeout_from_args(&args);

    let message = OutboxMessage {
        id: msg_id.clone(),
        token: args.token,
        from: sender.clone(),
        to: resolved_to,
        body: message_body,
        mode: args.mode,
        get_output: args.get_output,
        request_id: request_id.clone(),
        sender_agent: None,
        preferred_agent: args.agent,
        priority: "normal".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        command: args.command,
        action: None,
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
    };

    // Write to --outbox if specified, app outbox if root/master token, otherwise <root>/<local_dir>/outbox/
    let outbox_dir = if let Some(ref outbox_path) = args.outbox {
        PathBuf::from(outbox_path)
    } else if is_root {
        // Root/master token: use the app outbox so the MailboxPoller always finds it
        let app_outbox = crate::config::config_dir()
            .map(|d| d.join("app-outbox-path.txt"))
            .and_then(|p| std::fs::read_to_string(&p).ok())
            .map(|s| PathBuf::from(s.trim()));
        match app_outbox {
            Some(p) if p.is_dir() => p,
            _ => ac_dir.join("outbox"),
        }
    } else {
        ac_dir.join("outbox")
    };
    if let Err(e) = std::fs::create_dir_all(&outbox_dir) {
        eprintln!("Error: failed to create outbox directory: {}", e);
        return 1;
    }

    let outbox_path = outbox_dir.join(format!("{}.json", msg_id));
    let json = match serde_json::to_string_pretty(&message) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("Error: failed to serialize message: {}", e);
            return 1;
        }
    };

    if let Err(e) = std::fs::write(&outbox_path, json) {
        eprintln!("Error: failed to write outbox file: {}", e);
        return 1;
    }
    log::info!(
        "[send] queued message {} to '{}' in {}",
        msg_id,
        to_for_ack,
        outbox_dir.display()
    );

    // ── Poll for delivery confirmation ────────────────────────────────────
    // The MailboxPoller will pick up the file and move it to delivered/ or
    // rejected/. Wait until we know the outcome.
    if let Err(e) = wait_for_delivery_confirmation(
        &outbox_dir,
        &msg_id,
        &mode_for_ack,
        &to_for_ack,
        confirm_timeout,
        std::time::Duration::from_millis(250),
    ) {
        log::warn!("[send] {}", e);
        eprintln!("Error: {}", e);
        return 1;
    }

    // ── If --get-output, wait for response after confirmed delivery ───────
    if let Some(rid) = request_id {
        let responses_dir = ac_dir.join("responses");
        let response_path = responses_dir.join(format!("{}.json", rid));
        let timeout = std::time::Duration::from_secs(args.timeout);
        let poll_interval = std::time::Duration::from_secs(2);

        crate::cli_println!("Waiting for response (timeout: {}s)...", args.timeout);

        match wait_for_response(&response_path, timeout, poll_interval) {
            Ok(content) => {
                crate::cli_println!("{}", content);
                return 0;
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                return 1;
            }
        }
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_verified_coordinator_fixture() -> (tempfile::TempDir, Vec<String>) {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("proj-a");
        let workspace_dir = project.join(".ac");
        let team_dir = workspace_dir.join("_team_dev-team");
        let origin_tech_lead = workspace_dir.join("_agent_tech-lead");
        let origin_dev_rust = workspace_dir.join("_agent_dev-rust");
        let wg_dir = workspace_dir.join("wg-1-dev-team");
        let tech_lead_replica = wg_dir.join("__agent_tech-lead");
        let dev_rust_replica = wg_dir.join("__agent_dev-rust");

        for dir in [
            &team_dir,
            &origin_tech_lead,
            &origin_dev_rust,
            &tech_lead_replica,
            &dev_rust_replica,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }

        std::fs::write(
            team_dir.join("config.json"),
            r#"{"agents":["../_agent_dev-rust"],"coordinator":"../_agent_tech-lead"}"#,
        )
        .unwrap();
        std::fs::write(
            tech_lead_replica.join("config.json"),
            r#"{"identity":"../../_agent_tech-lead"}"#,
        )
        .unwrap();
        std::fs::write(
            dev_rust_replica.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust"}"#,
        )
        .unwrap();

        let paths = vec![temp.path().to_string_lossy().to_string()];
        (temp, paths)
    }

    #[test]
    fn root_agent_sender_uses_reserved_constant() {
        assert_eq!(
            sender_for_root("C:/tmp/agentscommander/ac-root-agent", true),
            crate::config::root_agent::ROOT_AGENT_SENDER
        );
        assert_ne!(
            sender_for_root("C:/tmp/agentscommander/_agent_root-agent", false),
            crate::config::root_agent::ROOT_AGENT_SENDER
        );
    }

    #[test]
    fn root_agent_routing_rejects_origin_coordinator() {
        let (_temp, paths) = make_verified_coordinator_fixture();

        assert!(!root_agent_target_allowed("proj-a/tech-lead", &paths));
    }

    #[test]
    fn send_rejects_send_path_for_wg() {
        let filename = r"messaging\20260524-040000-wg1-a-to-wg1-b-x.md";

        assert!(crate::phone::messaging::validate_filename_only(filename).is_err());
    }

    #[test]
    fn send_rejects_send_path_for_root() {
        let filename = r"messaging\20260524-040000-root-to-wg1-tech-lead-x.md";

        assert!(crate::phone::messaging::validate_filename_only(filename).is_err());
    }

    #[test]
    fn root_agent_send_accepts_verified_wg_coordinator() {
        let (_temp, paths) = make_verified_coordinator_fixture();

        assert!(root_agent_target_allowed(
            "proj-a:wg-1-dev-team/tech-lead",
            &paths
        ));
    }

    #[test]
    fn coordinator_to_root_target_allowed_accepts_verified_coordinator() {
        let (_temp, paths) = make_verified_coordinator_fixture();
        assert!(coordinator_to_root_target_allowed(
            "proj-a:wg-1-dev-team/tech-lead",
            &paths
        ));
    }

    #[test]
    fn coordinator_to_root_target_allowed_rejects_non_coordinator() {
        let (_temp, paths) = make_verified_coordinator_fixture();
        assert!(!coordinator_to_root_target_allowed(
            "proj-a:wg-1-dev-team/dev-rust",
            &paths
        ));
    }

    #[test]
    fn coordinator_to_root_target_allowed_rejects_origin_agent() {
        let (_temp, paths) = make_verified_coordinator_fixture();
        assert!(!coordinator_to_root_target_allowed(
            "proj-a/tech-lead",
            &paths
        ));
    }

    #[test]
    fn root_agent_command_clear_and_compact_are_rejected_before_outbox_write() {
        for command in ["clear", "compact"] {
            assert_eq!(
                validate_root_agent_delivery_kind(true, Some(command)),
                Err("Root Agent messaging is file-based; use --send with a root-to-coordinator Markdown file, not --command")
            );
        }
    }

    #[test]
    fn non_root_command_behavior_is_unchanged() {
        assert_eq!(
            validate_root_agent_delivery_kind(false, Some("compact")),
            Ok(())
        );
    }

    #[test]
    fn get_output_does_not_tighten_live_pty_clamp() {
        // The live wake path never injects the `--get-output` response markers
        // (`phone::mailbox` gates them on a non-interactive session, unreachable
        // today), so the CLI clamp must budget a `--get-output` send exactly like
        // a plain send. The live clamp calls `notification_pty_overhead(_, false)`;
        // this locks that a near-limit path is not rejected over marker bytes that
        // never reach the PTY, guarding against re-keying it on `args.get_output`.
        let sender_len = "project:wg-1-team/agent".len();
        let live_overhead = notification_pty_overhead(sender_len, false);
        let plain_overhead = crate::phone::messaging::PTY_WRAP_FIXED + sender_len;

        // Live budget is the plain wrap: get-output adds nothing on the live path.
        assert_eq!(live_overhead, plain_overhead);

        // A body sized to exactly fill the plain-wrap budget must be ACCEPTED by
        // the clamp (`body.len() + overhead > PTY_SAFE_MAX` is false).
        let body_len = crate::phone::messaging::PTY_SAFE_MAX - plain_overhead;
        assert!(body_len + live_overhead <= crate::phone::messaging::PTY_SAFE_MAX);

        // Under the reverted tightening, counting the never-injected marker
        // overhead would have rejected this same body. Confirm the corrected live
        // path does not, so the tightening cannot silently return.
        let tightened_overhead =
            plain_overhead + crate::phone::messaging::PTY_RESPONSE_MARKER_FIXED + 2 * REQUEST_ID_LEN;
        assert!(body_len + tightened_overhead > crate::phone::messaging::PTY_SAFE_MAX);
    }

    #[test]
    fn wait_for_delivery_confirmation_accepts_delivered_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let delivered_dir = temp.path().join("delivered");
        std::fs::create_dir_all(&delivered_dir).unwrap();
        std::fs::write(delivered_dir.join("msg-1.json"), "{}").unwrap();

        let result = wait_for_delivery_confirmation(
            temp.path(),
            "msg-1",
            "wake",
            "project:wg-1-team/agent",
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(1),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn wait_for_delivery_confirmation_reports_rejection_reason() {
        let temp = tempfile::TempDir::new().unwrap();
        let rejected_dir = temp.path().join("rejected");
        std::fs::create_dir_all(&rejected_dir).unwrap();
        std::fs::write(rejected_dir.join("msg-2.reason.txt"), "bad route").unwrap();

        let err = wait_for_delivery_confirmation(
            temp.path(),
            "msg-2",
            "wake",
            "project:wg-1-team/agent",
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(1),
        )
        .unwrap_err();

        assert!(err.contains("message rejected: bad route"));
    }

    #[test]
    fn wait_for_delivery_confirmation_reports_pending_outbox_on_timeout() {
        let temp = tempfile::TempDir::new().unwrap();

        let err = wait_for_delivery_confirmation(
            temp.path(),
            "msg-3",
            "wake",
            "project:wg-1-team/agent",
            std::time::Duration::from_millis(1),
            std::time::Duration::from_millis(1),
        )
        .unwrap_err();

        assert!(err.contains("delivery confirmation timeout"));
        assert!(err.contains("msg-3"));
        assert!(err.contains(&temp.path().display().to_string()));
    }

    #[test]
    fn wait_for_response_returns_present_file_at_zero_timeout() {
        // Regression lock for the ordering bug: a response already on disk must
        // be read even when the timeout has fully elapsed (timeout == 0). The old
        // timeout-first ordering dropped it and returned a timeout error instead.
        let temp = tempfile::TempDir::new().unwrap();
        let response_path = temp.path().join("resp.json");
        std::fs::write(&response_path, "{\"ok\":true}").unwrap();

        let result = wait_for_response(
            &response_path,
            std::time::Duration::from_secs(0),
            std::time::Duration::from_millis(1),
        );

        assert_eq!(result.unwrap(), "{\"ok\":true}");
    }

    #[test]
    fn wait_for_response_returns_present_file_within_window() {
        let temp = tempfile::TempDir::new().unwrap();
        let response_path = temp.path().join("resp.json");
        std::fs::write(&response_path, "hello").unwrap();

        let result = wait_for_response(
            &response_path,
            std::time::Duration::from_millis(50),
            std::time::Duration::from_millis(1),
        );

        assert_eq!(result.unwrap(), "hello");
    }

    #[test]
    fn wait_for_response_times_out_when_absent() {
        // The timeout must still fire (no infinite loop) when no file appears.
        let temp = tempfile::TempDir::new().unwrap();
        let missing = temp.path().join("never.json");

        let err = wait_for_response(
            &missing,
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(1),
        )
        .unwrap_err();

        assert!(err.contains("timeout waiting for response"));
    }

    #[test]
    fn derive_root_project_dir_walks_up_wg_replica_path() {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("proj-x");
        let agent_root = project.join(".ac").join("wg-1-devs").join("__agent_alice");
        std::fs::create_dir_all(&agent_root).unwrap();

        // On Windows, std::fs::canonicalize returns `\\?\C:\...` extended-length
        // paths; compute the expected via canonicalize so the assertion holds on
        // both Windows and Unix. Mirror pattern from list_peers.rs:972-1010
        // (`extended_length_prefix_normalizes`).
        let expected = std::fs::canonicalize(&project)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let got = derive_root_project_dir(agent_root.to_str().unwrap())
            .unwrap()
            .expect("should derive project from WG replica path");
        assert_eq!(got, expected);
    }

    #[test]
    fn derive_root_project_dir_accepts_ac_workspace() {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("proj-x");
        let root = project.join(".ac").join("wg-1-devs").join("__agent_alice");
        std::fs::create_dir_all(&root).unwrap();

        let got = derive_root_project_dir(root.to_str().unwrap())
            .unwrap()
            .expect(".ac workspace should be accepted");
        let expected = std::fs::canonicalize(&project)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(got, expected);
    }

    #[test]
    fn derive_root_project_dir_returns_none_for_non_wg_path() {
        let temp = tempfile::TempDir::new().unwrap();
        // A random subdir without the WG-replica shape.
        let random = temp.path().join("random").join("dir");
        std::fs::create_dir_all(&random).unwrap();
        assert!(derive_root_project_dir(random.to_str().unwrap())
            .unwrap()
            .is_none());
    }

    #[test]
    fn derive_root_project_dir_returns_none_when_one_level_too_high() {
        let temp = tempfile::TempDir::new().unwrap();
        let wg_dir = temp.path().join("proj-x").join(".ac").join("wg-1-devs");
        std::fs::create_dir_all(&wg_dir).unwrap();
        // Pointing at the WG dir, not the __agent_* dir → must return None.
        assert!(derive_root_project_dir(wg_dir.to_str().unwrap())
            .unwrap()
            .is_none());
    }

    // ── clap parse smoke: #782 --confirm-timeout ──

    fn parse_send_args(extra: &[&str]) -> SendArgs {
        use clap::Parser;
        let mut argv = vec![
            "agentscommander",
            "send",
            "--token",
            "11111111-1111-1111-1111-111111111111",
            "--to",
            "proj-a/agent",
            "--send",
            "20260704-000000-wg1-a-to-wg1-b-x.md",
            "--root",
            "anything",
        ];
        argv.extend_from_slice(extra);
        let parsed = crate::cli::Cli::try_parse_from(argv).expect("clap should accept send args");
        match parsed.command.expect("subcommand present") {
            crate::cli::Commands::Send(args) => args,
            _ => panic!("expected Send subcommand"),
        }
    }

    #[test]
    fn send_args_confirm_timeout_defaults_to_90() {
        let args = parse_send_args(&[]);
        assert_eq!(
            args.confirm_timeout, 90,
            "--confirm-timeout must default to 90"
        );
        assert_eq!(
            confirm_timeout_from_args(&args),
            std::time::Duration::from_secs(90),
            "default must map to the 90s ceiling handed to wait_for_delivery_confirmation"
        );
        // The --get-output response wait keeps its own independent default.
        assert_eq!(args.timeout, 300);
    }

    #[test]
    fn send_args_confirm_timeout_override_reaches_confirmation_wait() {
        let args = parse_send_args(&["--confirm-timeout", "7"]);
        assert_eq!(args.confirm_timeout, 7);
        assert_eq!(
            confirm_timeout_from_args(&args),
            std::time::Duration::from_secs(7),
            "override must be the exact Duration execute() hands to wait_for_delivery_confirmation"
        );
        // Overriding the confirmation wait must not touch the --get-output wait.
        assert_eq!(args.timeout, 300);
    }

    #[test]
    fn send_args_timeout_flag_does_not_affect_confirm_timeout() {
        let args = parse_send_args(&["--timeout", "45"]);
        assert_eq!(args.timeout, 45);
        assert_eq!(
            args.confirm_timeout, 90,
            "--timeout bounds the --get-output wait only; it must not override --confirm-timeout"
        );
    }

    #[test]
    fn send_help_documents_logical_actions_and_pi_mapping() {
        use clap::CommandFactory;
        let cmd = crate::cli::Cli::command();
        let mut subcommand = cmd
            .get_subcommands()
            .find(|cmd| cmd.get_name() == "send")
            .expect("send subcommand")
            .clone();
        let help = subcommand.render_long_help().to_string();

        assert!(help.contains("Logical PTY action"), "{help}");
        assert!(help.contains("clear"), "{help}");
        assert!(help.contains("/new"), "{help}");
        assert!(help.contains("/clear"), "{help}");
        assert!(help.contains("exact-stem direct Pi shell"), "{help}");
        assert!(help.contains("Pi compact"), "{help}");
        assert!(help.contains("must be idle"), "{help}");
        assert!(help.contains("terminally rejected before spawn"), "{help}");
        assert!(
            !help.contains("if Exited, respawn. Always delivers"),
            "logical actions must not inherit an unconditional delivery promise: {help}"
        );
    }

    #[test]
    fn send_help_documents_confirm_timeout_semantics() {
        use clap::CommandFactory;
        let cmd = crate::cli::Cli::command();
        let mut subcommand = cmd
            .get_subcommands()
            .find(|cmd| cmd.get_name() == "send")
            .expect("send subcommand")
            .clone();
        let help = subcommand.render_long_help().to_string();

        assert!(help.contains("--confirm-timeout"), "{help}");
        // Queued-vs-confirmed semantics: timeout does not mean the message was lost.
        assert!(help.contains("durably queued"), "{help}");
        assert!(help.contains("does NOT mean the message was lost"), "{help}");
        // The two timeout flags must be distinguished from each other.
        assert!(help.contains("--get-output response wait"), "{help}");
    }
}
