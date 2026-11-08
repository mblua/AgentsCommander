use clap::Args;
use std::ffi::OsString;

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
    /// Session token from `AGENTSCOMMANDER_TOKEN`. Shape-validated in the CLI;
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
