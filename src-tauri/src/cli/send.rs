use clap::Args;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::teams;
use crate::phone::types::OutboxMessage;

mod pty_input;

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

#[derive(Args)]
#[group(skip)]
#[command(group(
    clap::ArgGroup::new("send_payload")
        .required(true)
        .multiple(false)
        .args(["send", "command", "pty_input", "pty_input_stdin"])
))]
#[command(after_help = "\
DELIVERY MODES:\n  \
  wake            File messages inject into PTY and can spawn or respawn a persistent session. \
Logical PTY actions are capability- and idle-gated and can be terminally rejected before spawn.\n\n\
ROUTING: Before delivery, the CLI validates that the sender can reach the destination based on \
team \
membership and orchestrator rules (teams.json). If routing fails, the CLI exits immediately with \
code 1.\n\n\
DISCOVERY: Use `list-peers-lean` to get valid agent names for --to. The \"name\" field in the JSON \
output is the value to use.\n\n\
FILE-BASED MESSAGING: --send <filename> delivers a Markdown file. For Room \
replicas the file is resolved from <room-root>/messaging/<filename>. \
For the Root Agent the file is resolved from <root-agent-dir>/messaging/<filename>. \
`--send` is a filename only, never a path. Root Agent --to targets must be \
verified Room orchestrator replica names returned by list-peers-lean. \
Orchestrator --to targets may include the Root Agent canonical name \
`agentscommander://root-agent`; only identity-verified Room orchestrator \
replicas may use it.\n\n\
FILENAME FORMAT: YYYYMMDD-HHMMSS-<from>-to-<to>-<slug>[.N].md. \
Use exactly 8/6 ASCII digits for date/time (UTC convention); calendar dates and time ranges \
are not checked. from/to/slug use non-empty [a-z0-9]+ segments joined by hyphens. The literal \
to separator needs at least one segment before and two after it (recipient and slug). \
Names do not authenticate identities; routing uses --to and the authenticated sender. \
Use lowercase .md; no /, backslash, colon, traversal or other dots except optional .N. \
.N is one/two digits with value 1-99: .1, .01 and .99 pass; .0, .00 and .100 fail. \
Use a kebab-case slug: sanitize_slug generates up to 50 characters, but existing filenames \
have no validator slug-length limit. Root senders require root-to immediately after the \
timestamp; root-agent-to is invalid for Root. Room short names use room21-ac-dev-rust-v5; \
legacy wg21-ac-dev-rust-v5 remains supported.\n\n\
EXAMPLES:\n\
20261022-092418-root-to-room21-ac-tech-lead-v5-fixer-installation-confirmed.md\n\
20261022-093000-room21-ac-dev-rust-v5-to-room21-ac-tech-lead-v5-status.1.md\n\n\
PRIVILEGED PTY INPUT: --pty-input and --pty-input-stdin submit validated exact UTF-8 text to one \
authorized coding-agent PTY. This never directly executes a host or container OS shell command. \
The caller's shell performs quoting and expansion before AC receives an argument, so prefer stdin \
for multiline, leading-hyphen, clipboard, process-list-sensitive, or otherwise sensitive text. \
`Queued` is not `Injected`; after a confirmation timeout keep the reported operation ID and do not \
resubmit under a new ID.\n\n\
DELIVERY CONFIRMATION: After queuing, send blocks up to --confirm-timeout seconds (default 90) \
waiting for the app's poller to confirm delivery. This bounds ONLY the synchronous confirmation \
handshake, not delivery itself: on confirmation timeout the CLI exits 1, but the message remains \
durably queued in the outbox and is typically still delivered afterwards (e.g. when wake must \
cold-spawn an idle peer). Exit 1 on confirmation timeout does NOT mean the message was lost; \
verify the outbox instead of re-sending. --confirm-timeout is distinct from --timeout, which \
bounds only the --get-output response wait. On enqueue, the CLI prints `Queued: <message-id>` to \
stdout; a missing `Queued:` line means the message was NOT enqueued. \
Queued confirms enqueue only, not delivery or a reply. Delivered confirms system delivery, \
not reading or completed work. Keep the message ID and check delivered/<id>.json, \
rejected/<id>.json and rejected/<id>.reason.txt in the effective outbox before resending. \
The effective outbox is --outbox if supplied; otherwise a master token uses the app outbox \
when available, falling back to <root>/<local-dir>/outbox. Session tokens use \
<root>/<local-dir>/outbox. local-dir is configured per instance.")]
pub struct SendArgs {
    /// Session token from AGENTSCOMMANDER_TOKEN. Shape-validated in the CLI;
    /// per-session authorization happens at the daemon mailbox. See `--help` TOKEN VALIDATION
    /// MODEL.
    #[arg(long)]
    pub token: Option<String>,

    /// Destination agent name (e.g., "repos/my-project"). Use `list-peers-lean` to discover valid
    /// names
    #[arg(long)]
    pub to: String,

    /// Filename (not path) of a message file that already exists in
    /// <room-root>/messaging/ or <root-agent-dir>/messaging/ for Root.
    /// Use YYYYMMDD-HHMMSS-<from>-to-<to>-<slug>[.N].md; Root requires root-to.
    /// Sender writes the file BEFORE calling send. See FILENAME FORMAT below.
    /// Mutually exclusive with --command and the PTY input forms.
    #[arg(long)]
    pub send: Option<String>,

    /// Delivery mode (see DELIVERY MODES below)
    #[arg(long, default_value = "wake")]
    pub mode: String,

    /// Wait for and return the agent's response (blocks until reply or --timeout)
    #[arg(long, conflicts_with_all = ["pty_input", "pty_input_stdin"])]
    pub get_output: bool,

    /// Logical PTY action [possible values: clear, compact]. `clear` starts a
    /// fresh conversation: /new for an exact-stem direct Pi shell and /clear
    /// for direct Claude/Codex/Antigravity-family or Cursor agent shells. Pi compact
    /// is unsupported. The mapped session must be idle; unsupported mappings
    /// are terminally rejected before spawn. Not available from the Root Agent.
    /// Cannot be combined with --send
    #[arg(long)]
    pub command: Option<String>,

    /// Submit exact validated UTF-8 text to one authorized coding-agent PTY.
    /// The caller's shell performs quoting and expansion before AC receives the
    /// value. Prefer --pty-input-stdin for multiline, leading-hyphen, clipboard,
    /// process-list-sensitive, or otherwise sensitive text.
    #[arg(long, allow_hyphen_values = true)]
    pub pty_input: Option<OsString>,

    /// Read exact PTY input from stdin with a 65,536-byte UTF-8 ceiling.
    #[arg(long)]
    pub pty_input_stdin: bool,

    /// Configured agent id to use when `wake` spawns a new persistent session for
    /// the destination. `auto` picks the session's saved `lastCodingAgent`.
    #[arg(long, default_value = "auto")]
    pub agent: String,

    /// Profile slot letter A-Z applied to the coding agent the wake spawns or
    /// respawns. A disabled or missing letter walks down to the nearest enabled
    /// cell and the receipt reports `fallbackApplied`. Never writes the replica's
    /// tooling.profile/currentCodingAgent/lastCodingAgent. Not usable with PTY input.
    #[arg(long, conflicts_with_all = ["pty_input", "pty_input_stdin"])]
    pub profile: Option<String>,

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
    #[arg(long, default_value = "90", value_parser = clap::value_parser!(u64).range(0..=3600))]
    pub confirm_timeout: u64,

    /// Agent root directory (required). Your working directory — used to derive your agent name
    #[arg(long)]
    pub root: Option<String>,

    /// Write message to a specific outbox directory instead of <root>/<local-dir>/outbox/
    #[arg(long, conflicts_with_all = ["pty_input", "pty_input_stdin"])]
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
    let mut line = format!(
        "Delivered: {} (mode={}, to={})",
        msg_id, mode_for_ack, to_for_ack
    );
    if let Some(msg) = delivered {
        if let Some(agent) = msg.effective_agent_id.as_deref() {
            line.push_str(&format!(" agent={}", agent));
        }
        if let Some(profile) = msg.effective_profile.as_deref() {
            line.push_str(&format!(" profile={}", profile));
        }
        if msg.profile_fallback_applied {
            line.push_str(" fallbackApplied");
        }
        if let Some(reason) = msg.dispatch_not_applied.as_deref() {
            line.push_str(&format!(" flagsNotApplied={}", reason));
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
    let delivered_path = outbox_dir
        .join("delivered")
        .join(format!("{}.json", msg_id));
    let rejected_reason_path = outbox_dir
        .join("rejected")
        .join(format!("{}.reason.txt", msg_id));
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
/// Thin wrapper over `config::ac_root::wg_replica_layout_from_agent_dir`, the
/// single source of the WG-replica walk-up shared with
/// `list_peers::detect_wg_replica` and
/// `phone::mailbox::derive_project_from_outbox_path`, so `send` resolves WG-peer
/// targets against the same source `list-peers` reports as `reachable: true`.
/// See #228 / #726.
fn derive_root_project_dir(root: &str) -> Result<Option<String>, String> {
    let Some(canon) =
        crate::filesystem::directories::canonical_cli_discovery_path(Path::new(root)).ok()
    else {
        return Ok(None);
    };
    match crate::config::ac_root::wg_replica_layout_from_agent_dir(&canon)? {
        Some(layout) => Ok(layout.project_dir.to_str().map(|path| path.to_string())),
        None => Ok(None),
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
        .ok()
        .is_some_and(|canonical| resolved_queue_shape(&canonical) == Some(true));
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
        let mut extra = format!("agent={}", agent_for_ack);
        if let Some(letter) = requested_profile.as_deref() {
            extra.push_str(&format!(", profile={}", letter));
        }
        format!("Queued: {} ({})", msg_id, extra)
    } else {
        format!("Queued: {}", msg_id)
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
            eprintln!("Error: {}", error);
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
            eprintln!("Error: {}", e);
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
                eprintln!("Error: {}", e);
                return Err(1);
            }
        };

    // ── Pre-validate routing ──────────────────────────────────────────────

    if root_is_root_agent {
        if !root_agent_target_allowed(&resolved_to, &effective_project_paths) {
            eprintln!(
                "Error: root-agent routing rejected — '{}' is not a verified Room orchestrator \
                    replica. Use list-peers-lean from the Root Agent and pass one of its name \
                    values.",
                resolved_to
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
                "Error: routing rejected — '{}' cannot reach '{}'. \
                 Check team membership and orchestrator rules.",
                sender, resolved_to
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
            eprintln!("Error: {}", e);
            return Err(1);
        }
        let agent_root_path = std::path::Path::new(root);
        let msg_dir = if root_is_root_agent {
            match crate::phone::messaging::root_messaging_dir(agent_root_path) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("Error: failed to resolve root messaging dir: {}", e);
                    return Err(1);
                }
            }
        } else {
            let wg_root = match crate::phone::messaging::workgroup_root(agent_root_path) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!(
                        "Error: --send requires --root under a `room-*` or legacy `wg-*` Room \
                            directory unless --root is the canonical Root Agent directory; {}",
                        e
                    );
                    return Err(1);
                }
            };
            if let Err(e) =
                crate::phone::messaging::ensure_workgroup_root_is_authoritative(&wg_root)
            {
                eprintln!("Error: {}", e);
                return Err(1);
            }
            match crate::phone::messaging::messaging_dir(&wg_root) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("Error: failed to resolve messaging dir: {}", e);
                    return Err(1);
                }
            }
        };
        let abs = match crate::phone::messaging::resolve_existing_message(&msg_dir, filename) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Error: {}", e);
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
    } else if is_root {
        // Root/master token: use the app outbox so the MailboxPoller always finds it
        let app_outbox = crate::config::config_dir()
            .map(|d| d.join("app-outbox-path.txt"))
            .and_then(|p| crate::filesystem::documents::read_cli_outbox_text(&p).ok())
            .map(|s| PathBuf::from(s.trim()));
        match app_outbox {
            Some(p) if crate::filesystem::directories::cli_agent_root_is_dir(&p) => p,
            _ => ac_dir.join("outbox"),
        }
    } else {
        ac_dir.join("outbox")
    };
    if let Err(reason) = reject_comanaged_queue_outbox(&outbox_dir) {
        eprintln!("Error: {}", reason);
        return Err(1);
    }
    if let Err(e) = crate::filesystem::directories::ensure_cli_outbox(&outbox_dir) {
        eprintln!("Error: failed to create outbox directory: {}", e);
        return Err(1);
    }

    let outbox_path = outbox_dir.join(format!("{}.json", message.id));
    let json = match serde_json::to_string_pretty(message) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("Error: failed to serialize message: {}", e);
            return Err(1);
        }
    };

    if let Err(e) =
        crate::filesystem::documents::write_cli_outbox_message(&outbox_path, json.as_bytes())
    {
        eprintln!("Error: failed to write outbox file: {}", e);
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
        log::warn!("[send] {}", e);
        eprintln!("Error: {}", e);
        return 1;
    }

    // ── If --get-output, wait for response after confirmed delivery ───────
    if let Some(rid) = &message.request_id {
        let responses_dir = ac_dir.join("responses");
        let response_path = responses_dir.join(format!("{}.json", rid));
        let timeout = std::time::Duration::from_secs(timeout_secs);
        let poll_interval = std::time::Duration::from_secs(2);

        crate::cli_println!("Waiting for response (timeout: {}s)...", timeout_secs);

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
mod tests;
