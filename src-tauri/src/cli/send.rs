use clap::Args;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::teams;
use crate::phone::types::OutboxMessage;

#[derive(Args)]
#[command(after_help = "\
DELIVERY MODES:\n  \
  wake            Inject into PTY. If no session exists, spawn a persistent one; if Exited, respawn. Always delivers.\n\n\
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
replicas may use it.")]
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

    /// Remote command to execute on the agent's PTY [possible values: clear, compact].
    /// Not available from the Root Agent; Root Agent messaging is file-based.
    /// The agent must be idle. Cannot be combined with --send
    #[arg(long)]
    pub command: Option<String>,

    /// Configured agent id to use when `wake` spawns a new persistent session for
    /// the destination. `auto` picks the session's saved `lastCodingAgent`.
    #[arg(long, default_value = "auto")]
    pub agent: String,

    /// Timeout in seconds for --get-output
    #[arg(long, default_value = "300")]
    pub timeout: u64,

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

/// If `root` lives inside `<project_dir>/<workspace>/wg-<N>-*/__agent_*/`,
/// return `project_dir` as a UTF-8 `String`. Returns `None` if `root` is not
/// inside a WG-replica shape OR if the resulting `project_dir` is not valid
/// UTF-8 (parity with `list_peers::detect_wg_replica`, which also uses
/// `to_str()?` rather than `to_string_lossy()`).
///
/// Mirrors the WG-replica detection in `list_peers::detect_wg_replica` so that
/// `send` resolves WG-peer targets against the same root-walk-up source that
/// `list-peers` uses to emit them with `reachable: true`. See #228.
///
/// Keep in lockstep with `list_peers::detect_wg_replica` and
/// `phone::mailbox::derive_project_from_outbox_path`.
fn derive_root_project_dir(root: &str) -> Result<Option<String>, String> {
    let Some(canon) = std::fs::canonicalize(root).ok() else {
        return Ok(None);
    };
    let Some(my_dir_name) = canon.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    if !my_dir_name.starts_with("__agent_") {
        return Ok(None);
    }
    let Some(wg_dir) = canon.parent() else {
        return Ok(None);
    };
    let Some(wg_name) = wg_dir.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    if !wg_name.starts_with("wg-") {
        return Ok(None);
    }
    let Some(workspace_dir) = wg_dir.parent() else {
        return Ok(None);
    };
    if !workspace_dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(crate::config::workspace::is_workspace_dir_name)
        .unwrap_or(false)
    {
        return Ok(None);
    }
    crate::config::workspace::ensure_authoritative_workspace_dir(workspace_dir)?;
    let Some(project_dir) = workspace_dir.parent() else {
        return Ok(None);
    };
    Ok(project_dir.to_str().map(|path| path.to_string()))
}

fn ensure_workgroup_root_is_authoritative(wg_root: &Path) -> Result<(), String> {
    let workspace_dir = wg_root.parent().ok_or_else(|| {
        format!(
            "workgroup root '{}' has no parent workspace directory",
            wg_root.display()
        )
    })?;
    crate::config::workspace::ensure_authoritative_workspace_dir(workspace_dir)
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
        // Canonicalize once outside the dedup closure. If canonicalization of
        // `root_project` itself fails (rare — it just resolved milliseconds
        // ago inside `derive_root_project_dir`), fall back to string-equality
        // rather than a broken `None == None` match.
        let canon_root_project = std::fs::canonicalize(&root_project).ok();
        let already_present = effective_project_paths
            .iter()
            .any(|p| match &canon_root_project {
                Some(canon_target) => std::fs::canonicalize(p).ok().as_ref() == Some(canon_target),
                None => p == &root_project,
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

        // PTY_SAFE_MAX clamp (trimmed overhead: the wrap no longer embeds
        // wg_root or bin_path — only `from` and the fixed framing remain).
        let overhead = crate::phone::messaging::PTY_WRAP_FIXED + sender.len();
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

    // ── Poll for delivery confirmation ────────────────────────────────────
    // The MailboxPoller will pick up the file and move it to delivered/ or
    // rejected/. Wait until we know the outcome.
    let delivered_path = outbox_dir
        .join("delivered")
        .join(format!("{}.json", msg_id));
    let rejected_reason_path = outbox_dir
        .join("rejected")
        .join(format!("{}.reason.txt", msg_id));

    let confirm_timeout = std::time::Duration::from_secs(30);
    let confirm_poll = std::time::Duration::from_millis(250);
    let start = std::time::Instant::now();

    loop {
        if delivered_path.exists() {
            crate::cli_println!(
                "Delivered: {} (mode={}, to={})",
                msg_id,
                mode_for_ack,
                to_for_ack
            );
            break;
        }
        if rejected_reason_path.exists() {
            let reason = std::fs::read_to_string(&rejected_reason_path)
                .unwrap_or_else(|_| "unknown reason".to_string());
            eprintln!("Error: message rejected — {}", reason.trim());
            return 1;
        }
        if start.elapsed() >= confirm_timeout {
            eprintln!(
                "Error: delivery confirmation timeout after 30s (message {} may still be pending)",
                msg_id
            );
            return 1;
        }
        std::thread::sleep(confirm_poll);
    }

    // ── If --get-output, wait for response after confirmed delivery ───────
    if let Some(rid) = request_id {
        let responses_dir = ac_dir.join("responses");
        let response_path = responses_dir.join(format!("{}.json", rid));
        let timeout = std::time::Duration::from_secs(args.timeout);
        let poll_interval = std::time::Duration::from_secs(2);
        let resp_start = std::time::Instant::now();

        crate::cli_println!("Waiting for response (timeout: {}s)...", args.timeout);

        loop {
            if resp_start.elapsed() >= timeout {
                eprintln!(
                    "Error: timeout waiting for response after {}s",
                    args.timeout
                );
                return 1;
            }

            if response_path.exists() {
                match std::fs::read_to_string(&response_path) {
                    Ok(content) => {
                        crate::cli_println!("{}", content);
                        return 0;
                    }
                    Err(e) => {
                        eprintln!("Error: failed to read response file: {}", e);
                        return 1;
                    }
                }
            }

            std::thread::sleep(poll_interval);
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
}
