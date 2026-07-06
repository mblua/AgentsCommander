//! `coding-agent` CLI verb (#786) - scriptable create/inspect/update/remove of
//! Coding Agent configs (`settings.agents[]`) without the GUI.
//!
//! No `--token`: this mutates the user-local `settings.json`, which any local
//! process can already write (same boundary argument as `open-project`).
//!
//! Safe-write routing. `list`/`show`/`catalog` always read disk and print JSON.
//! Mutations (`add`/`update`/`remove`) NEVER direct-write while a GUI for this
//! binary identity is running:
//!   - GUI running (probe via the single-instance mutex): the mutation is queued
//!     as a file the GUI's `MailboxPoller` drains against its authoritative
//!     in-memory settings, writing a result this CLI waits on. The poller claims
//!     each request with an atomic `.processing` rename, so a timeout-cancel can
//!     never be reported as "cancelled" for a mutation that actually applied.
//!   - GUI not running: a strict load (refuses to modify a present-but-unparseable
//!     settings.json rather than silently defaulting) then apply then save.
//!
//! Known limitation (documented for scripts): a SettingsModal already open with
//! an unsaved draft can, on its next Save, revert a concurrent CLI mutation. The
//! CLI does not clobber the GUI, but the reverse remains. Run `--launch` (or
//! re-`show`) right after `add` so consumption happens before a Modal Save.

use std::path::Path;
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};

use crate::config::coding_agent_mutations::{
    self as ops, AgentPatch, CodingAgentOp, CodingAgentOpOutcome, CodingAgentRequest,
    CodingAgentResult, DEFAULT_CUSTOM_COLOR,
};
use crate::config::coding_agents_catalog::{load_catalog, CodingAgentDefinition};
use crate::config::settings::{
    load_settings_for_cli_strict, save_settings, validate_user_env_key, AgentConfig,
    CodingAgentEnv, CodingAgentEnvSource, ConfigSeedConfig,
};

const DEFAULT_CONFIRM_TIMEOUT_SECS: u64 = 30;
/// #786 R3: grace window (one poller interval + margin) after an ENOENT cancel.
const GRACE_WAIT: Duration = Duration::from_secs(4);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Args)]
#[command(after_help = "\
PURPOSE: Scriptable management of Coding Agent configs (settings.agents) without \
the GUI. `create-agent` / `create-agent-matrix --launch` consume agents created \
here.\n\n\
OUTPUT: JSON on stdout, diagnostics on stderr, exit 0 on success, 1 on error.\n\n\
GUI ROUTING: while the GUI runs, mutations are routed through it (safe against \
clobber). While it is closed, they write settings.json directly.\n\n\
EXIT 1 is either terminal (validation) or retryable (GUI busy). Only a \
\"cancelled (safe to retry)\" message is blind-retry-safe; a \"may or may not have \
applied\" message is NOT - run `coding-agent show --id <id>` first.\n\n\
PLATFORM: GUI detection is Windows-only; off-Windows a running GUI is not \
detected and the direct write path is always used.")]
pub struct CodingAgentArgs {
    #[command(subcommand)]
    pub cmd: CodingAgentCmd,
}

#[derive(Subcommand)]
pub enum CodingAgentCmd {
    /// List configured coding agents (settings.agents) as a JSON array
    List,
    /// Show one configured coding agent by exact id as JSON
    Show(ShowArgs),
    /// List the read-only coding-agent catalog as JSON
    Catalog,
    /// Add a coding agent (custom, or seeded with --from-catalog <key>)
    Add(AddArgs),
    /// Update fields of an existing coding agent (partial; only given flags change)
    Update(UpdateArgs),
    /// Remove a coding agent by exact id (profile entries are left as orphans)
    Remove(RemoveArgs),
}

#[derive(Args)]
pub struct ShowArgs {
    /// Exact agent id
    #[arg(long)]
    pub id: String,
}

#[derive(Args)]
pub struct AddArgs {
    /// Seed label/command/color/envs/isolatedHome (and optional instructions/seed)
    /// from this catalog key; explicit flags below override seeded values.
    #[arg(long = "from-catalog", value_name = "KEY")]
    pub from_catalog: Option<String>,
    /// Custom agent id (default: a minted `agent_<ms>_<hex>` id)
    #[arg(long)]
    pub id: Option<String>,
    /// Display label (required unless --from-catalog supplies a non-empty one)
    #[arg(long)]
    pub label: Option<String>,
    /// Launch command (required unless --from-catalog supplies it)
    #[arg(long)]
    pub command: Option<String>,
    /// Color as #RRGGBB (default #6366f1 for custom agents)
    #[arg(long)]
    pub color: Option<String>,
    /// Env row KEY=VALUE (repeatable). All CLI envs are source=user, enabled=true.
    #[arg(long = "env", value_name = "KEY=VALUE")]
    pub env: Vec<String>,
    /// Provide an isolated CODEX_HOME at spawn (Codex)
    #[arg(long = "isolated-home", value_name = "true|false", action = clap::ArgAction::Set)]
    pub isolated_home: Option<bool>,
    /// Bare .md filename AC writes into the agent root at launch
    #[arg(long = "instructions-filename", value_name = "NAME.md")]
    pub instructions_filename: Option<String>,
    /// Config-folder seed destination NAME under the replica root (implies enabled)
    #[arg(long = "config-seed-dest", value_name = "FOLDER")]
    pub config_seed_dest: Option<String>,
    /// Enable/disable the config-folder seed
    #[arg(long = "config-seed-enabled", value_name = "true|false", action = clap::ArgAction::Set)]
    pub config_seed_enabled: Option<bool>,
    /// Seconds to wait for the GUI to process the request (daemon path only)
    #[arg(long = "confirm-timeout", default_value_t = DEFAULT_CONFIRM_TIMEOUT_SECS)]
    pub confirm_timeout: u64,
}

#[derive(Args)]
pub struct UpdateArgs {
    /// Exact agent id to update
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub label: Option<String>,
    #[arg(long)]
    pub command: Option<String>,
    /// Color as #RRGGBB
    #[arg(long)]
    pub color: Option<String>,
    /// Env row KEY=VALUE (repeatable). Any --env REPLACES the whole env list
    /// (including source=system rows).
    #[arg(long = "env", value_name = "KEY=VALUE")]
    pub env: Vec<String>,
    /// Clear the whole env list (conflicts with --env)
    #[arg(long = "clear-envs")]
    pub clear_envs: bool,
    #[arg(long = "isolated-home", value_name = "true|false", action = clap::ArgAction::Set)]
    pub isolated_home: Option<bool>,
    #[arg(long = "instructions-filename", value_name = "NAME.md")]
    pub instructions_filename: Option<String>,
    /// Clear the instructions filename (conflicts with --instructions-filename)
    #[arg(long = "clear-instructions-filename")]
    pub clear_instructions_filename: bool,
    #[arg(long = "config-seed-dest", value_name = "FOLDER")]
    pub config_seed_dest: Option<String>,
    #[arg(long = "config-seed-enabled", value_name = "true|false", action = clap::ArgAction::Set)]
    pub config_seed_enabled: Option<bool>,
    /// Clear the config-folder seed (conflicts with the other seed flags)
    #[arg(long = "clear-config-seed")]
    pub clear_config_seed: bool,
    #[arg(long = "confirm-timeout", default_value_t = DEFAULT_CONFIRM_TIMEOUT_SECS)]
    pub confirm_timeout: u64,
}

#[derive(Args)]
pub struct RemoveArgs {
    /// Exact agent id to remove
    #[arg(long)]
    pub id: String,
    #[arg(long = "confirm-timeout", default_value_t = DEFAULT_CONFIRM_TIMEOUT_SECS)]
    pub confirm_timeout: u64,
}

pub fn execute(args: CodingAgentArgs) -> i32 {
    run(args, crate::config::profile::gui_instance_running())
}

/// Split from `execute` so the direct/daemon branch selection is unit-testable
/// with an injected `gui_running` bool (no live mutex needed).
fn run(args: CodingAgentArgs, gui_running: bool) -> i32 {
    let result = match args.cmd {
        CodingAgentCmd::List => cmd_list(),
        CodingAgentCmd::Show(a) => cmd_show(a),
        CodingAgentCmd::Catalog => cmd_catalog(),
        CodingAgentCmd::Add(a) => cmd_add(a, gui_running),
        CodingAgentCmd::Update(a) => cmd_update(a, gui_running),
        CodingAgentCmd::Remove(a) => cmd_remove(a, gui_running),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

// ---- read verbs (R1: strict loader, never a silent-default read) -----------

fn cmd_list() -> Result<(), String> {
    let settings = load_settings_for_cli_strict()?;
    print_json(&settings.agents)
}

fn cmd_show(a: ShowArgs) -> Result<(), String> {
    let settings = load_settings_for_cli_strict()?;
    let agent = settings
        .agents
        .iter()
        .find(|ag| ag.id == a.id)
        .ok_or_else(|| ops::unknown_agent_id_error(&a.id, &settings.agents))?;
    print_json(agent)
}

fn cmd_catalog() -> Result<(), String> {
    let dir = crate::config::config_dir().ok_or("Could not resolve config dir")?;
    let catalog = load_catalog(&dir);
    print_json(&catalog)
}

// ---- mutation verbs --------------------------------------------------------

fn cmd_add(a: AddArgs, gui_running: bool) -> Result<(), String> {
    // R6: strict color/label checks apply ONLY to explicitly typed flags.
    if let Some(color) = &a.color {
        ops::validate_agent_color(color)?;
    }
    if let Some(label) = &a.label {
        if label.trim().is_empty() {
            return Err("--label must not be empty".to_string());
        }
    }
    let confirm_timeout = a.confirm_timeout;

    // Seed from the catalog (Rust port of definitionToSeed) or start blank.
    let mut agent = if let Some(key) = &a.from_catalog {
        let dir = crate::config::config_dir().ok_or("Could not resolve config dir")?;
        let catalog = load_catalog(&dir);
        let def = catalog.iter().find(|d| &d.key == key).ok_or_else(|| {
            let keys: Vec<&str> = catalog.iter().map(|d| d.key.as_str()).collect();
            format!(
                "catalog key '{key}' not found. Available: {}",
                keys.join(", ")
            )
        })?;
        definition_to_agent_seed(def)
    } else {
        blank_agent()
    };

    // Explicit flags override seeded values.
    if let Some(label) = a.label {
        agent.label = label.trim().to_string();
    }
    if let Some(command) = a.command {
        agent.command = command;
    }
    if let Some(color) = a.color {
        agent.color = color;
    }
    if !a.env.is_empty() {
        agent.envs = parse_env_flags(&a.env)?;
    }
    if let Some(isolated) = a.isolated_home {
        agent.isolated_home = isolated;
    }
    if let Some(name) = a.instructions_filename {
        agent.instructions_filename = Some(name);
    }
    apply_seed_flags(&mut agent, a.config_seed_dest, a.config_seed_enabled);

    // Custom (not from-catalog) requires label + command (mirrors Onboarding).
    if a.from_catalog.is_none()
        && (agent.label.trim().is_empty() || agent.command.trim().is_empty())
    {
        return Err("add requires --label and --command (or --from-catalog <key>)".to_string());
    }

    agent.id = match a.id {
        Some(id) => {
            ops::validate_custom_agent_id(&id)?;
            id
        }
        None => ops::mint_agent_id(),
    };

    dispatch_mutation(CodingAgentOp::Add { agent }, gui_running, confirm_timeout)
}

fn cmd_update(a: UpdateArgs, gui_running: bool) -> Result<(), String> {
    // Flag-conflict checks first (fast feedback, before any disk / routing).
    if a.clear_envs && !a.env.is_empty() {
        return Err("--clear-envs conflicts with --env".to_string());
    }
    if a.clear_instructions_filename && a.instructions_filename.is_some() {
        return Err(
            "--clear-instructions-filename conflicts with --instructions-filename".to_string(),
        );
    }
    if a.clear_config_seed && (a.config_seed_dest.is_some() || a.config_seed_enabled.is_some()) {
        return Err(
            "--clear-config-seed conflicts with --config-seed-dest / --config-seed-enabled"
                .to_string(),
        );
    }
    if let Some(color) = &a.color {
        ops::validate_agent_color(color)?;
    }
    if let Some(label) = &a.label {
        if label.trim().is_empty() {
            return Err("--label must not be empty".to_string());
        }
    }

    let envs = if a.clear_envs {
        Some(Vec::new())
    } else if a.env.is_empty() {
        None
    } else {
        Some(parse_env_flags(&a.env)?)
    };

    let patch = AgentPatch {
        label: a.label.map(|l| l.trim().to_string()),
        command: a.command,
        color: a.color,
        envs,
        isolated_home: a.isolated_home,
        instructions_filename: a.instructions_filename,
        clear_instructions_filename: a.clear_instructions_filename,
        config_seed_dest: a.config_seed_dest,
        config_seed_enabled: a.config_seed_enabled,
        clear_config_seed: a.clear_config_seed,
    };
    dispatch_mutation(
        CodingAgentOp::Update { id: a.id, patch },
        gui_running,
        a.confirm_timeout,
    )
}

fn cmd_remove(a: RemoveArgs, gui_running: bool) -> Result<(), String> {
    dispatch_mutation(
        CodingAgentOp::Remove { id: a.id },
        gui_running,
        a.confirm_timeout,
    )
}

// ---- routing ---------------------------------------------------------------

fn dispatch_mutation(
    op: CodingAgentOp,
    gui_running: bool,
    confirm_timeout: u64,
) -> Result<(), String> {
    if gui_running {
        let config_dir = crate::config::config_dir().ok_or("Could not resolve config dir")?;
        dispatch_via_daemon(&config_dir, op, confirm_timeout)
    } else {
        dispatch_direct(op)
    }
}

/// GUI closed: strict load (R1) -> apply -> save. `save_settings` is the
/// preserve-project-paths default (this op never touches the project list).
fn dispatch_direct(op: CodingAgentOp) -> Result<(), String> {
    let mut settings = load_settings_for_cli_strict()?;
    let outcome = ops::apply_coding_agent_op(&mut settings, &op)?;
    save_settings(&settings)?;
    print_outcome(&outcome)
}

/// GUI running: queue the request and wait on the result (R2/R3).
fn dispatch_via_daemon(
    config_dir: &Path,
    op: CodingAgentOp,
    confirm_timeout: u64,
) -> Result<(), String> {
    let requests_dir = config_dir.join(ops::CODING_AGENT_REQUESTS_DIR);
    let results_dir = requests_dir.join(ops::RESULTS_SUBDIR);

    let request_id = uuid::Uuid::new_v4().to_string();
    let request = CodingAgentRequest {
        request_id: request_id.clone(),
        created_at_ms: now_unix_ms(),
        op,
    };
    let request_path = ops::write_coding_agent_request(&requests_dir, &request)?;

    // Poll for the result up to the confirm timeout (check first, so a 0 timeout
    // does one non-blocking check).
    let deadline = Instant::now() + Duration::from_secs(confirm_timeout);
    loop {
        if let Some(result) = take_result(&results_dir, &request_id) {
            return finish_daemon_result(result);
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    // Timeout: cancel by removing our own request file (R2/R3).
    match std::fs::remove_file(&request_path) {
        Ok(()) => Err(format!(
            "GUI did not process the request within {confirm_timeout}s; request cancelled (safe to retry)"
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // The poller already claimed it: bounded grace wait, then honest
            // unknown-state (do NOT report a safe cancel).
            grace_wait_for_result(&results_dir, &request_id)
        }
        Err(e) => Err(format!("failed to cancel request: {e}")),
    }
}

fn grace_wait_for_result(results_dir: &Path, request_id: &str) -> Result<(), String> {
    let deadline = Instant::now() + GRACE_WAIT;
    loop {
        if let Some(result) = take_result(results_dir, request_id) {
            return finish_daemon_result(result);
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    Err(
        "request was claimed by the GUI but no result arrived; the change may or may not have \
applied. Run `coding-agent show --id <id>` before retrying. Do NOT blind-retry."
            .to_string(),
    )
}

/// Read a result and best-effort delete the result file.
fn take_result(results_dir: &Path, request_id: &str) -> Option<CodingAgentResult> {
    let result = ops::read_coding_agent_result(results_dir, request_id)?;
    let _ = std::fs::remove_file(results_dir.join(format!("{request_id}.json")));
    Some(result)
}

fn finish_daemon_result(result: CodingAgentResult) -> Result<(), String> {
    if result.ok {
        let json = if let Some(id) = result.removed_id {
            serde_json::json!({ "ok": true, "op": "remove", "id": id })
        } else {
            serde_json::json!({ "ok": true, "op": result.op, "agent": result.agent })
        };
        print_json(&json)
    } else {
        Err(result.error.unwrap_or_else(|| "request failed".to_string()))
    }
}

// ---- helpers ---------------------------------------------------------------

fn print_outcome(outcome: &CodingAgentOpOutcome) -> Result<(), String> {
    let json = if outcome.op == "remove" {
        serde_json::json!({ "ok": true, "op": "remove", "id": outcome.removed_id })
    } else {
        serde_json::json!({ "ok": true, "op": outcome.op, "agent": outcome.agent })
    };
    print_json(&json)
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize JSON: {e}"))?;
    crate::cli_println!("{}", json);
    Ok(())
}

fn blank_agent() -> AgentConfig {
    AgentConfig {
        id: String::new(),
        label: String::new(),
        command: String::new(),
        color: DEFAULT_CUSTOM_COLOR.to_string(),
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        backend: Default::default(),
    }
}

/// Rust port of `definitionToSeed` (agent-presets.ts): copy
/// label/command/color/envs/isolatedHome; carry instructionsFilename/configSeed
/// only when present; drop key/description/removable; id is minted by the caller.
fn definition_to_agent_seed(def: &CodingAgentDefinition) -> AgentConfig {
    AgentConfig {
        id: String::new(),
        label: def.label.clone(),
        command: def.command.clone(),
        color: def.color.clone(),
        envs: def.envs.clone(),
        isolated_home: def.isolated_home,
        instructions_filename: def.instructions_filename.clone(),
        config_seed: def.config_seed.clone(),
        backend: Default::default(),
    }
}

/// `--config-seed-dest` implies enabled=true unless `--config-seed-enabled false`.
fn apply_seed_flags(agent: &mut AgentConfig, dest: Option<String>, enabled: Option<bool>) {
    if dest.is_none() && enabled.is_none() {
        return;
    }
    let mut seed = agent.config_seed.clone().unwrap_or(ConfigSeedConfig {
        enabled: true,
        dest: String::new(),
    });
    if let Some(dest) = dest {
        seed.dest = dest;
    }
    if let Some(enabled) = enabled {
        seed.enabled = enabled;
    }
    agent.config_seed = Some(seed);
}

/// Parse `--env KEY=VALUE` (C8): split on the FIRST `=`; missing `=` or an
/// invalid key is a CLI error (surfaced here, not via a poller round-trip).
fn parse_env_flags(raw: &[String]) -> Result<Vec<CodingAgentEnv>, String> {
    let mut out = Vec::with_capacity(raw.len());
    for item in raw {
        let (key, value) = item
            .split_once('=')
            .ok_or_else(|| format!("--env '{item}' must be KEY=VALUE"))?;
        validate_user_env_key(key, "--env")?;
        out.push(CodingAgentEnv {
            key: key.to_string(),
            value: value.to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        });
    }
    Ok(out)
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_args() -> AddArgs {
        AddArgs {
            from_catalog: None,
            id: None,
            label: None,
            command: None,
            color: None,
            env: Vec::new(),
            isolated_home: None,
            instructions_filename: None,
            config_seed_dest: None,
            config_seed_enabled: None,
            confirm_timeout: 0,
        }
    }

    fn update_args(id: &str) -> UpdateArgs {
        UpdateArgs {
            id: id.to_string(),
            label: None,
            command: None,
            color: None,
            env: Vec::new(),
            clear_envs: false,
            isolated_home: None,
            instructions_filename: None,
            clear_instructions_filename: false,
            config_seed_dest: None,
            config_seed_enabled: None,
            clear_config_seed: false,
            confirm_timeout: 0,
        }
    }

    // ---- flag-conflict / early errors (return before touching disk) --------

    #[test]
    fn add_custom_requires_label_and_command() {
        // No from-catalog, no label/command -> error before minting/dispatch.
        let code = run(
            CodingAgentArgs {
                cmd: CodingAgentCmd::Add(add_args()),
            },
            false,
        );
        assert_eq!(code, 1);
    }

    #[test]
    fn add_rejects_bad_color_flag() {
        let mut a = add_args();
        a.label = Some("L".into());
        a.command = Some("claude".into());
        a.color = Some("#fff".into());
        assert!(cmd_add(a, false).is_err());
    }

    #[test]
    fn update_clear_envs_conflicts_with_env() {
        let mut a = update_args("x");
        a.clear_envs = true;
        a.env = vec!["FOO=bar".into()];
        assert!(cmd_update(a, false).is_err());
    }

    #[test]
    fn update_clear_instructions_conflicts() {
        let mut a = update_args("x");
        a.clear_instructions_filename = true;
        a.instructions_filename = Some("AGENTS.md".into());
        assert!(cmd_update(a, false).is_err());
    }

    #[test]
    fn update_clear_seed_conflicts() {
        let mut a = update_args("x");
        a.clear_config_seed = true;
        a.config_seed_dest = Some(".claude".into());
        assert!(cmd_update(a, false).is_err());
    }

    #[test]
    fn env_parse_first_equals_and_missing() {
        let envs = parse_env_flags(&["FOO=a=b".to_string()]).unwrap();
        assert_eq!(envs[0].key, "FOO");
        assert_eq!(envs[0].value, "a=b");
        assert!(parse_env_flags(&["NOEQUALS".to_string()]).is_err());
    }

    // ---- branch seam: daemon path writes a request and NEVER saves ---------

    #[test]
    fn daemon_path_queues_request_and_never_saves() {
        let dir = tempfile::tempdir().unwrap();
        let op = CodingAgentOp::Remove {
            id: "whatever".into(),
        };
        // confirm_timeout 0 -> one non-blocking result check, then cancel.
        let res = dispatch_via_daemon(dir.path(), op, 0);
        assert!(res.is_err(), "no poller -> timeout/cancel");
        // A request was written under coding-agent-requests/ (then cancelled),
        // and settings.json was NEVER written by the daemon path.
        assert!(dir.path().join(ops::CODING_AGENT_REQUESTS_DIR).is_dir());
        assert!(!dir.path().join("settings.json").exists());
    }

    // ---- help text ---------------------------------------------------------

    #[test]
    fn help_text_documents_coding_agent() {
        use clap::CommandFactory;
        let help = crate::cli::Cli::command().render_help().to_string();
        assert!(help.contains("coding-agent"), "help missing verb: {help}");
    }
}
