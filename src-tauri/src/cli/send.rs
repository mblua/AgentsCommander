use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::teams;
use crate::phone::types::OutboxMessage;

mod arguments;
mod project;
mod pty_input;

pub use arguments::SendArgs;
use project::derive_root_project_dir;

use pty_input::{execute_pty_input, pty_text_from_args};
#[cfg(test)]
use pty_input::{
    find_pty_input_terminal_artifact, read_bounded_pty_input, validate_pty_artifact,
    ExpectedPtyArtifact,
};

fn validate_send_filename(
    filename: &str,
    root_is_root_agent: bool,
) -> Result<(), crate::phone::messaging::MessagingError> {
    if root_is_root_agent {
        crate::phone::messaging::validate_root_notification_filename(filename)
    } else {
        crate::phone::messaging::validate_filename_only(filename)
    }
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
        Err(
            "Root Agent messaging is file-based; use --send with a root-to-orchestrator Markdown \
            file, not --command",
        )
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

/// #1635-2 D4: `Delivered:` receipt line. The message id remains the first
/// token in every shape (scripts that parse `Delivered: <id>` keep working).
/// The daemon writes the annotated message to `delivered/<id>.json`;
/// `delivered: None` (unreadable / foreign / older daemon file) keeps the
/// legacy byte-identical line — a missing annotation never fails the
/// confirmation.
fn delivered_receipt_line(
    msg_id: &str,
    mode_for_ack: &str,
    to_for_ack: &str,
    delivered: Option<&OutboxMessage>,
) -> String {
    let mut line = format!("Delivered: {msg_id} (mode={mode_for_ack}, to={to_for_ack})");
    if let Some(msg) = delivered {
        if let Some(agent) = msg.effective_agent_id.as_deref() {
            line.push_str(&format!(" agent={agent}"));
        }
        if let Some(profile) = msg.effective_profile.as_deref() {
            line.push_str(&format!(" profile={profile}"));
        }
        if msg.profile_fallback_applied {
            line.push_str(" fallbackApplied");
        }
        if let Some(reason) = msg.dispatch_not_applied.as_deref() {
            line.push_str(&format!(" flagsNotApplied={reason}"));
        }
    }
    line
}

fn wait_for_delivery_confirmation(
    outbox_dir: &Path,
    msg_id: &str,
    mode_for_ack: &str,
    to_for_ack: &str,
    confirm_timeout: std::time::Duration,
    confirm_poll: std::time::Duration,
) -> Result<(), String> {
    let delivered_path = crate::phone::types::outbox_delivered_receipt(outbox_dir, msg_id);
    let rejected_reason_path = crate::phone::types::outbox_rejected_reason(outbox_dir, msg_id);
    let start = std::time::Instant::now();

    loop {
        if crate::filesystem::directories::cli_outbox_artifact_exists(&delivered_path) {
            // #1635-2 D4: read the annotated delivered JSON when the daemon
            // wrote one; a foreign/older daemon (or an unparseable file) falls
            // back to the legacy line without failing the confirmation.
            let delivered = crate::filesystem::documents::read_cli_outbox_text(&delivered_path)
                .ok()
                .and_then(|raw| serde_json::from_str::<OutboxMessage>(&raw).ok());
            crate::cli_println!(
                "{}",
                delivered_receipt_line(msg_id, mode_for_ack, to_for_ack, delivered.as_ref())
            );
            return Ok(());
        }
        if crate::filesystem::directories::cli_outbox_artifact_exists(&rejected_reason_path) {
            let reason = crate::filesystem::documents::read_cli_outbox_text(&rejected_reason_path)
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
        if crate::filesystem::directories::cli_outbox_artifact_exists(response_path) {
            return crate::filesystem::documents::read_cli_outbox_text(response_path)
                .map_err(|e| format!("failed to read response file: {e}"));
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

fn archive_send_file(
    root: &Path,
    root_is_root_agent: bool,
    resolved_to: &str,
    project_paths: &[String],
    source: &Path,
    basename: &str,
) -> Result<PathBuf, String> {
    let room = if root_is_root_agent {
        crate::config::teams::verified_wg_coordinator_target(resolved_to, project_paths)
            .and_then(|target| target.replica_dir.parent().map(Path::to_path_buf))
            .ok_or("cannot verify destination room for archive")?
    } else {
        crate::phone::messaging::workgroup_root(root).map_err(|e| e.to_string())?
    };
    crate::phone::messaging::ensure_workgroup_root_is_authoritative(&room)?;
    let ac_root = room.parent().ok_or("room has no Project AC Root")?;
    let snapshot =
        crate::filesystem::documents::read_cli_message_archive_source(source).map_err(|e| {
            format!(
                "{} -> {}: {e}",
                source.display(),
                ac_root.join("message-archive").display()
            )
        })?;
    crate::phone::messaging::archive_message_snapshot(ac_root, source, basename, &snapshot)
}

/// #2232 phase 7: nothing but the in-process supervisor may write the
/// Co-managed provenance queue. `--outbox` takes an arbitrary path, so without
/// this guard any caller holding a token could forge a Co-managed-origin
/// message. Rejects both the `<room>/.co-managed/queue` shape and any aliased
/// spelling of it that resolves to that shape: `.` and `..` are resolved
/// lexically first (F3), so an alias like
/// `<room>/.co-managed/missing/../queue` is rejected even while `missing` does
/// not exist and `canonicalize` therefore fails.
fn reject_comanaged_queue_outbox(outbox_dir: &Path) -> Result<(), String> {
    /// `Some(true)` = the queue shape; `Some(false)` = provably not; `None` = a
    /// `..` escaped the path's own base, which the caller rejects because it
    /// cannot prove the path is not the queue.
    fn resolved_queue_shape(path: &Path) -> Option<bool> {
        use std::path::Component;
        let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
                Component::ParentDir => {
                    parts.pop()?;
                }
                Component::Normal(part) => parts.push(part),
            }
        }
        let len = parts.len();
        Some(
            len >= 2
                && parts[len - 1]
                    == std::ffi::OsStr::new(crate::config::co_managed::QUEUE_DIR_NAME)
                && parts[len - 2]
                    == std::ffi::OsStr::new(crate::config::co_managed::CO_MANAGED_DIR_NAME),
        )
    }

    // A real path that canonicalizes onto the queue (a symlink or an existing
    // alias) is rejected even when its lexical shape differs.
    let aliased = crate::filesystem::directories::canonical_cli_discovery_path(outbox_dir)
        .is_ok_and(|canonical| resolved_queue_shape(&canonical) == Some(true));
    if resolved_queue_shape(outbox_dir) != Some(false) || aliased {
        return Err(format!(
            "--outbox may not target the Co-managed provenance queue ({}); only the Co-managed \
                supervisor writes there",
            outbox_dir.display()
        ));
    }
    Ok(())
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

/// #1638: `Queued:` receipt line. The message id remains the first token in
/// both shapes (scripts that parse `Queued: <id>` keep working). Requested
/// (not effective) values are printed here — effective values are only
/// knowable at delivery time and are printed by the `Delivered:` line in
/// phase 1635-2.
fn queued_receipt_line(
    msg_id: &str,
    agent_for_ack: &str,
    requested_profile: &Option<String>,
) -> String {
    if agent_for_ack != "auto" || requested_profile.is_some() {
        let mut extra = format!("agent={agent_for_ack}");
        if let Some(letter) = requested_profile.as_deref() {
            extra.push_str(&format!(", profile={letter}"));
        }
        format!("Queued: {msg_id} ({extra})")
    } else {
        format!("Queued: {msg_id}")
    }
}

pub fn execute(args: SendArgs) -> i32 {
    let root = match args.root {
        Some(ref r) => r.clone(),
        None => {
            eprintln!("Error: --root is required. Specify your agent's root directory.");
            return 1;
        }
    };
    let pty_text = match pty_text_from_args(&args) {
        Ok(value) => value,
        Err(code) => {
            eprintln!("Error: {code}");
            return 1;
        }
    };
    if let Some(text) = pty_text {
        return execute_pty_input(args, root, text);
    }

    let root_is_root_agent = crate::config::root_agent::is_root_agent_path(&root);
    if let Err(reason) =
        validate_root_agent_delivery_kind(root_is_root_agent, args.command.as_deref())
    {
        eprintln!("Error: {reason}");
        return 1;
    }

    // Validate token before proceeding
    let is_root = match crate::cli::validate_cli_token(&args.token) {
        Ok((_token, root)) => root,
        Err(msg) => {
            eprintln!("{msg}");
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

    let (resolved_to, effective_project_paths) =
        match resolve_send_target(&args, &root, &sender, root_is_root_agent, is_root) {
            Ok(target) => target,
            Err(code) => return code,
        };

    // --send + --command mutually exclusive (P0-3)
    if args.send.is_some() && args.command.is_some() {
        eprintln!("Error: --send and --command are mutually exclusive");
        return 1;
    }

    let message_body = match send_message_body(
        args.send.as_deref(),
        &root,
        root_is_root_agent,
        &sender,
        &resolved_to,
        &effective_project_paths,
    ) {
        Ok(body) => body,
        Err(code) => return code,
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

    enqueue_delivery(args, sender, resolved_to, message_body, is_root, &ac_dir)
}

fn resolve_send_target(
    args: &SendArgs,
    root: &str,
    sender: &str,
    root_is_root_agent: bool,
    is_root: bool,
) -> Result<(String, Vec<String>), i32> {
    // ── Resolve --to against known projects (Decision 2 / §AR2-shared) ─────
    //
    // Qualified FQN → validated shape + existence. Unqualified WG-local →
    // two-level scan, unambiguous → canonical FQN, ambiguous → reject with
    // candidate list, unknown → reject. Origin/bare → pass through.
    //
    // CLI-side resolution is belt-and-braces (§DR1); the mailbox also
    // canonicalizes on receive (§AR2-norm) so direct outbox writes cannot
    // bypass the reject-on-ambiguity rule.
    let settings = match crate::config::settings::load_settings_for_cli_strict() {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("Error: {error}");
            return Err(1);
        }
    };
    // Build an in-memory project-path slice that includes the project
    // derived from --root (if any). Mirrors list-peers's WG-replica walk-up
    // discovery so qualified WG-peer targets that list-peers reports as
    // reachable do not fail send with `not found in any known project`.
    // settings.json is NOT modified. See #228 / plan _plans/228-cli-daemon-laterals.md.
    let mut effective_project_paths = settings.project_paths.clone();
    let root_project = match derive_root_project_dir(root) {
        Ok(root_project) => root_project,
        Err(e) => {
            eprintln!("Error: {e}");
            return Err(1);
        }
    };
    if let Some(root_project) = root_project {
        // Canonicalize the target once. Compare by string first and only fall
        // back to a per-path canonicalize syscall when the strings differ, so the
        // common "project already registered" case does no extra filesystem work.
        // If canonicalizing `root_project` itself fails (rare: it resolved
        // milliseconds ago inside `derive_root_project_dir`), the string check
        // still catches an exact duplicate.
        let canon_root_project =
            crate::filesystem::directories::canonical_cli_discovery_path(Path::new(&root_project))
                .ok();
        let already_present = effective_project_paths.iter().any(|p| {
            if p == &root_project {
                return true;
            }
            match &canon_root_project {
                Some(canon_target) => {
                    crate::filesystem::directories::canonical_cli_discovery_path(Path::new(p))
                        .ok()
                        .as_ref()
                        == Some(canon_target)
                }
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
                eprintln!("Error: {e}");
                return Err(1);
            }
        };

    // ── Pre-validate routing ──────────────────────────────────────────────

    if root_is_root_agent {
        if !root_agent_target_allowed(&resolved_to, &effective_project_paths) {
            eprintln!(
                "Error: root-agent routing rejected — '{resolved_to}' is not a \
                    verified Room orchestrator replica. Use list-peers-lean from the Root Agent \
                    and pass one of its name values."
            );
            return Err(1);
        }
    } else if crate::config::root_agent::is_root_agent_target(&resolved_to) {
        // #293 — coordinator → root. Only identity-verified WG coordinators
        // are allowed to address the Root Agent. Master/root token still goes
        // through this gate intentionally: the URI is meaningful only when
        // paired with a real coordinator identity, and the verified check is
        // cheap.
        if !coordinator_to_root_target_allowed(sender, &effective_project_paths) {
            eprintln!(
                "Error: routing rejected — '{}' is not a verified Room orchestrator replica and \
                    cannot message '{}'. Replies to the Root Agent are reserved for verified Room \
                    orchestrators.",
                sender,
                crate::config::root_agent::ROOT_AGENT_SENDER
            );
            return Err(1);
        }
    } else if !is_root {
        // Use the unaugmented captured registrations for team authorization BEFORE
        // writing to outbox. Fail immediately with a clear error if not.
        let discovered = teams::discover_teams_from_project_paths(&settings.project_paths);
        if !teams::can_communicate(sender, &resolved_to, &discovered) {
            eprintln!(
                "Error: routing rejected — '{sender}' cannot reach '{resolved_to}'. \
                 Check team membership and orchestrator rules."
            );
            return Err(1);
        }
    }

    Ok((resolved_to, effective_project_paths))
}

fn send_message_body(
    filename: Option<&str>,
    root: &str,
    root_is_root_agent: bool,
    sender: &str,
    resolved_to: &str,
    effective_project_paths: &[String],
) -> Result<String, i32> {
    let message_body = if let Some(filename) = filename {
        if let Err(e) = validate_send_filename(filename, root_is_root_agent) {
            eprintln!("Error: {e}");
            return Err(1);
        }
        let agent_root_path = std::path::Path::new(root);
        let msg_dir = if root_is_root_agent {
            match crate::phone::messaging::root_messaging_dir(agent_root_path) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("Error: failed to resolve root messaging dir: {e}");
                    return Err(1);
                }
            }
        } else {
            let wg_root = match crate::phone::messaging::workgroup_root(agent_root_path) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!(
                        "Error: --send requires --root under a `room-*` or legacy `wg-*` Room \
                            directory unless --root is the canonical Root Agent directory; {e}"
                    );
                    return Err(1);
                }
            };
            if let Err(e) =
                crate::phone::messaging::ensure_workgroup_root_is_authoritative(&wg_root)
            {
                eprintln!("Error: {e}");
                return Err(1);
            }
            match crate::phone::messaging::messaging_dir(&wg_root) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("Error: failed to resolve messaging dir: {e}");
                    return Err(1);
                }
            }
        };
        let abs = match crate::phone::messaging::resolve_existing_message(&msg_dir, filename) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Error: {e}");
                return Err(1);
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
                 Shorten slug or move room to a shallower path.",
                body.len(),
                overhead,
                crate::phone::messaging::PTY_SAFE_MAX
            );
            return Err(1);
        }

        if let Err(error) = archive_send_file(
            Path::new(root),
            root_is_root_agent,
            resolved_to,
            effective_project_paths,
            &abs,
            filename,
        ) {
            eprintln!(
                "Message archive warning: {} -> {}: {}",
                abs.display(),
                resolved_to,
                error
            );
        }
        body
    } else {
        String::new()
    };

    Ok(message_body)
}

fn enqueue_delivery(
    args: SendArgs,
    sender: String,
    resolved_to: String,
    message_body: String,
    is_root: bool,
    ac_dir: &Path,
) -> i32 {
    let msg_id = Uuid::new_v4().to_string();
    let request_id = if args.get_output {
        Some(Uuid::new_v4().to_string())
    } else {
        None
    };

    let to_for_ack = resolved_to.clone();
    // #1638: clone the agent id for the enqueue receipt before the message
    // literal moves `args.agent`.
    let agent_for_ack = args.agent.clone();
    // Resolve before `args` fields move into OutboxMessage below; the whole-struct
    // borrow would be rejected after the partial moves.
    let confirm_timeout = confirm_timeout_from_args(&args);

    let requested_profile = match args.profile.as_deref() {
        None => None,
        Some(raw) => match crate::config::settings::normalize_profile_letter(raw) {
            Some(letter) => Some(letter),
            None => {
                eprintln!("Error: --profile must be a single letter A through Z");
                return 1;
            }
        },
    };
    // #1638: clone for the receipt line — the message literal below moves
    // `requested_profile` into the outbox message.
    let requested_profile_for_ack = requested_profile.clone();

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
        requested_profile,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
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
        pty_input: None,
    };

    let outbox_dir = match publish_outbox_message(&message, args.outbox.as_deref(), is_root, ac_dir)
    {
        Ok(outbox_dir) => outbox_dir,
        Err(code) => return code,
    };
    // #1596: enqueue receipt on stdout — the only observable signal when the
    // caller cannot see the GUI-subsystem binary's exit code (PowerShell
    // direct capture). A missing `Queued:` line means NOT enqueued.
    crate::cli_println!(
        "{}",
        queued_receipt_line(&msg_id, &agent_for_ack, &requested_profile_for_ack)
    );
    log::info!(
        "[send] queued message {} to '{}' in {}",
        msg_id,
        to_for_ack,
        outbox_dir.display()
    );

    finish_delivery(&message, &outbox_dir, ac_dir, confirm_timeout, args.timeout)
}

fn publish_outbox_message(
    message: &OutboxMessage,
    outbox: Option<&str>,
    is_root: bool,
    ac_dir: &Path,
) -> Result<PathBuf, i32> {
    // Write to --outbox if specified, app outbox if root/master token, otherwise
    // <root>/<local_dir>/outbox/
    let outbox_dir = if let Some(outbox_path) = outbox {
        PathBuf::from(outbox_path)
    } else {
        // Root/master token: use the app outbox so the MailboxPoller always finds it
        crate::filesystem::app_outbox_pointer::directory(
            is_root.then(crate::config::config_dir).flatten().as_deref(),
            ac_dir,
        )
    };
    if let Err(reason) = reject_comanaged_queue_outbox(&outbox_dir) {
        eprintln!("Error: {reason}");
        return Err(1);
    }
    if let Err(e) = crate::filesystem::directories::ensure_cli_outbox(&outbox_dir) {
        eprintln!("Error: failed to create outbox directory: {e}");
        return Err(1);
    }

    let outbox_path = outbox_dir.join(format!("{}.json", message.id));
    let json = match serde_json::to_string_pretty(message) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("Error: failed to serialize message: {e}");
            return Err(1);
        }
    };

    if let Err(e) =
        crate::filesystem::documents::write_cli_outbox_message(&outbox_path, json.as_bytes())
    {
        eprintln!("Error: failed to write outbox file: {e}");
        return Err(1);
    }
    Ok(outbox_dir)
}

fn finish_delivery(
    message: &OutboxMessage,
    outbox_dir: &Path,
    ac_dir: &Path,
    confirm_timeout: std::time::Duration,
    timeout_secs: u64,
) -> i32 {
    // ── Poll for delivery confirmation ────────────────────────────────────
    // The MailboxPoller will pick up the file and move it to delivered/ or
    // rejected/. Wait until we know the outcome.
    if let Err(e) = wait_for_delivery_confirmation(
        outbox_dir,
        &message.id,
        &message.mode,
        &message.to,
        confirm_timeout,
        std::time::Duration::from_millis(250),
    ) {
        log::warn!("[send] {e}");
        eprintln!("Error: {e}");
        return 1;
    }

    // ── If --get-output, wait for response after confirmed delivery ───────
    if let Some(rid) = &message.request_id {
        let responses_dir = ac_dir.join("responses");
        let response_path = responses_dir.join(format!("{rid}.json"));
        let timeout = std::time::Duration::from_secs(timeout_secs);
        let poll_interval = std::time::Duration::from_secs(2);

        crate::cli_println!("Waiting for response (timeout: {}s)...", timeout_secs);

        match wait_for_response(&response_path, timeout, poll_interval) {
            Ok(content) => {
                crate::cli_println!("{}", content);
                return 0;
            }
            Err(e) => {
                eprintln!("Error: {e}");
                return 1;
            }
        }
    }

    0
}

#[cfg(test)]
mod tests;
