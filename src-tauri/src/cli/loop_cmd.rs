use std::path::{Path, PathBuf};

use chrono::Utc;
use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

use crate::cli::workgroup::{resolve_cli_ac_root, resolve_cli_project, write_refresh};
use crate::config::loops::{
    apply_loop_update_patch, baseline_loop_state, details_from_parts, discover_loops_in_project,
    loop_dir, read_loop_config, read_loop_state, sanitize_loop_id, validate_loop_config,
    validate_loop_id, write_loop_config, write_loop_state_atomic, AcLoopSummary,
    BusyCoordinatorPolicy, LoopConfigToml, LoopDef, LoopPolicy, LoopPrompt, LoopSessionStart,
    LoopState, LoopTarget, LoopTargetKind, LoopTrigger, LoopTriggerKind, LoopUpdatePatch,
    LOOP_TIMEZONE_LOCAL,
};

pub const MAX_LOOP_PROMPT_FILE_BYTES: u64 = 128 * 1024;

#[derive(Args)]
pub struct LoopArgs {
    #[command(subcommand)]
    command: LoopCommand,
}

#[derive(Subcommand)]
enum LoopCommand {
    /// List loops in a project
    List(LoopListArgs),
    /// Create a loop
    Create(LoopCreateArgs),
    /// Update a loop
    Update(LoopUpdateArgs),
    /// Remove a loop
    Remove(LoopRemoveArgs),
    /// Enable a loop
    Enable(LoopToggleArgs),
    /// Disable a loop
    Disable(LoopToggleArgs),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum BusyCoordinatorCli {
    #[value(name = "wait-until-idle")]
    WaitUntilIdle,
    #[value(name = "force-inject")]
    ForceInject,
    #[value(name = "skip")]
    Skip,
}

impl From<BusyCoordinatorCli> for BusyCoordinatorPolicy {
    fn from(value: BusyCoordinatorCli) -> Self {
        match value {
            BusyCoordinatorCli::WaitUntilIdle => BusyCoordinatorPolicy::WaitUntilIdle,
            BusyCoordinatorCli::ForceInject => BusyCoordinatorPolicy::ForceInject,
            BusyCoordinatorCli::Skip => BusyCoordinatorPolicy::Skip,
        }
    }
}

/// A separate CLI enum rather than a `ValueEnum` derive on `LoopSessionStart`:
/// that would pull `clap` into `config/loops.rs`. Mirrors `BusyCoordinatorCli`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum SessionStartCli {
    #[value(name = "fresh")]
    Fresh,
    #[value(name = "accumulate")]
    Accumulate,
}

impl From<SessionStartCli> for LoopSessionStart {
    fn from(value: SessionStartCli) -> Self {
        match value {
            SessionStartCli::Fresh => LoopSessionStart::Fresh,
            SessionStartCli::Accumulate => LoopSessionStart::Accumulate,
        }
    }
}

#[derive(Args)]
struct LoopListArgs {
    #[arg(long)]
    project: String,
}

#[derive(Args)]
struct LoopCreateArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    id: Option<String>,
    #[arg(long)]
    name: String,
    #[arg(long = "cron")]
    cron: String,
    #[arg(long = "room", alias = "workgroup", value_name = "ROOM")]
    workgroup: String,
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long = "prompt-file")]
    prompt_file: Option<PathBuf>,
    #[arg(long = "busy-coordinator", value_enum)]
    busy_coordinator: Option<BusyCoordinatorCli>,
    #[arg(long = "session-start", value_enum)]
    session_start: Option<SessionStartCli>,
    #[arg(long = "force-inject-when-busy")]
    force_inject_when_busy: bool,
}

#[derive(Args)]
struct LoopUpdateArgs {
    #[arg(long)]
    project: String,
    #[arg(long = "loop")]
    loop_id: String,
    #[arg(long)]
    name: Option<String>,
    #[arg(long = "cron")]
    cron: Option<String>,
    #[arg(long = "room", alias = "workgroup", value_name = "ROOM")]
    workgroup: Option<String>,
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long = "prompt-file")]
    prompt_file: Option<PathBuf>,
    #[arg(long = "busy-coordinator", value_enum)]
    busy_coordinator: Option<BusyCoordinatorCli>,
    #[arg(long = "session-start", value_enum)]
    session_start: Option<SessionStartCli>,
    #[arg(long = "force-inject-when-busy")]
    force_inject_when_busy: bool,
}

#[derive(Args)]
struct LoopRemoveArgs {
    #[arg(long)]
    project: String,
    #[arg(long = "loop")]
    loop_id: String,
}

#[derive(Args)]
struct LoopToggleArgs {
    #[arg(long)]
    project: String,
    #[arg(long = "loop")]
    loop_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LoopListResult {
    project_path: String,
    loops: Vec<AcLoopSummary>,
}

pub fn execute(args: LoopArgs) -> i32 {
    let result = match args.command {
        LoopCommand::List(args) => list(args),
        LoopCommand::Create(args) => create(args),
        LoopCommand::Update(args) => update(args),
        LoopCommand::Remove(args) => remove(args),
        LoopCommand::Enable(args) => set_enabled(args, true),
        LoopCommand::Disable(args) => set_enabled(args, false),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

fn list(args: LoopListArgs) -> Result<(), String> {
    let project_path = resolve_cli_project(&args.project)?;
    let loops = discover_loops_in_project(&project_path);
    print_json(&LoopListResult {
        project_path: project_path.to_string_lossy().to_string(),
        loops,
    })
}

fn create(args: LoopCreateArgs) -> Result<(), String> {
    let project_path = resolve_cli_project(&args.project)?;
    let ac_root = resolve_cli_ac_root(&project_path)?;
    let id = match args.id.as_deref() {
        Some(id) => sanitize_loop_id(id)?,
        None => sanitize_loop_id(&args.name)?,
    };
    let dir = loop_dir(&ac_root, &id);
    if dir.exists() {
        return Err(format!("Loop '{}' already exists", id));
    }

    let prompt = resolve_prompt(args.prompt.as_deref(), args.prompt_file.as_deref())?;
    let busy_coordinator = resolve_busy_policy(args.busy_coordinator, args.force_inject_when_busy)?;
    let config = LoopConfigToml {
        loop_def: LoopDef {
            id: id.clone(),
            name: args.name,
            enabled: true,
        },
        trigger: LoopTrigger {
            kind: LoopTriggerKind::Cron,
            expr: args.cron,
            timezone: LOOP_TIMEZONE_LOCAL.to_string(),
        },
        target: LoopTarget {
            kind: LoopTargetKind::WorkgroupCoordinator,
            workgroup: args.workgroup,
        },
        prompt: LoopPrompt { body: prompt },
        policy: LoopPolicy {
            busy_coordinator,
            session_start: args
                .session_start
                .map(LoopSessionStart::from)
                .unwrap_or_default(),
            ..LoopPolicy::default()
        },
    };
    validate_loop_config(&project_path, &config)?;
    let dir = write_loop_config(&ac_root, &config)?;
    let state = initial_state(&config)?;
    write_loop_state_atomic(&dir, &state)?;
    write_refresh(&project_path, &dir, &id, "loopCreated");
    print_json(&details_from_parts(&dir, &config, &state))
}

fn update(args: LoopUpdateArgs) -> Result<(), String> {
    validate_loop_id(&args.loop_id)?;
    let project_path = resolve_cli_project(&args.project)?;
    let ac_root = resolve_cli_ac_root(&project_path)?;
    let dir = loop_dir(&ac_root, &args.loop_id);
    if !dir.is_dir() {
        return Err(format!("Loop '{}' not found", args.loop_id));
    }
    let mut config = read_loop_config(&dir)?;
    let prompt_body = if args.prompt.is_some() || args.prompt_file.is_some() {
        Some(resolve_prompt(
            args.prompt.as_deref(),
            args.prompt_file.as_deref(),
        )?)
    } else {
        None
    };
    let busy_coordinator = if args.busy_coordinator.is_some() || args.force_inject_when_busy {
        Some(resolve_busy_policy(
            args.busy_coordinator,
            args.force_inject_when_busy,
        )?)
    } else {
        None
    };
    let reset_schedule = apply_loop_update_patch(
        &mut config,
        LoopUpdatePatch {
            name: args.name,
            expr: args.cron,
            workgroup: args.workgroup,
            prompt_body,
            busy_coordinator,
            session_start: args.session_start.map(LoopSessionStart::from),
            enabled: None,
        },
    )?;

    validate_loop_config(&project_path, &config)?;
    let dir = write_loop_config(&ac_root, &config)?;
    let state = if reset_schedule {
        initial_state(&config)?
    } else {
        read_loop_state(&dir).unwrap_or_default()
    };
    if reset_schedule {
        write_loop_state_atomic(&dir, &state)?;
    }
    write_refresh(&project_path, &dir, &args.loop_id, "loopUpdated");
    print_json(&details_from_parts(&dir, &config, &state))
}

fn remove(args: LoopRemoveArgs) -> Result<(), String> {
    validate_loop_id(&args.loop_id)?;
    let project_path = resolve_cli_project(&args.project)?;
    let ac_root = resolve_cli_ac_root(&project_path)?;
    let dir = loop_dir(&ac_root, &args.loop_id);
    if !dir.is_dir() {
        return Err(format!("Loop '{}' not found", args.loop_id));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("Failed to remove Loop directory: {}", e))?;
    write_refresh(&project_path, &dir, &args.loop_id, "loopRemoved");
    if std::env::var_os("AC_MACHINE_OUTPUT").is_some() {
        print_json(&serde_json::json!({
            "id": args.loop_id,
            "removed": true
        }))
    } else {
        crate::cli_println!("Removed loop {}", args.loop_id);
        Ok(())
    }
}

fn set_enabled(args: LoopToggleArgs, enabled: bool) -> Result<(), String> {
    validate_loop_id(&args.loop_id)?;
    let project_path = resolve_cli_project(&args.project)?;
    let ac_root = resolve_cli_ac_root(&project_path)?;
    let dir = loop_dir(&ac_root, &args.loop_id);
    if !dir.is_dir() {
        return Err(format!("Loop '{}' not found", args.loop_id));
    }
    let mut config = read_loop_config(&dir)?;
    let reset_schedule = apply_loop_update_patch(
        &mut config,
        LoopUpdatePatch {
            enabled: Some(enabled),
            ..LoopUpdatePatch::default()
        },
    )?;
    validate_loop_config(&project_path, &config)?;
    let dir = write_loop_config(&ac_root, &config)?;
    let state = if reset_schedule {
        initial_state(&config)?
    } else {
        read_loop_state(&dir).unwrap_or_default()
    };
    if reset_schedule {
        write_loop_state_atomic(&dir, &state)?;
    }
    write_refresh(
        &project_path,
        &dir,
        &args.loop_id,
        if enabled {
            "loopEnabled"
        } else {
            "loopDisabled"
        },
    );
    print_json(&details_from_parts(&dir, &config, &state))
}

fn initial_state(config: &LoopConfigToml) -> Result<LoopState, String> {
    baseline_loop_state(config, Utc::now())
}

fn resolve_busy_policy(
    busy: Option<BusyCoordinatorCli>,
    force_inject_when_busy: bool,
) -> Result<BusyCoordinatorPolicy, String> {
    if force_inject_when_busy {
        if let Some(value) = busy {
            let policy = BusyCoordinatorPolicy::from(value);
            if policy != BusyCoordinatorPolicy::ForceInject {
                return Err("--force-inject-when-busy conflicts with --busy-coordinator values other than force-inject".to_string());
            }
        }
        return Ok(BusyCoordinatorPolicy::ForceInject);
    }
    Ok(busy
        .map(BusyCoordinatorPolicy::from)
        .unwrap_or(BusyCoordinatorPolicy::WaitUntilIdle))
}

fn resolve_prompt(prompt: Option<&str>, prompt_file: Option<&Path>) -> Result<String, String> {
    match (prompt, prompt_file) {
        (Some(_), Some(_)) => Err("Use either --prompt or --prompt-file, not both".to_string()),
        (None, None) => Err("Either --prompt or --prompt-file is required".to_string()),
        (Some(prompt), None) => {
            if prompt.trim().is_empty() {
                Err("--prompt cannot be empty".to_string())
            } else {
                Ok(prompt.to_string())
            }
        }
        (None, Some(path)) => read_prompt_file(path),
    }
}

fn read_prompt_file(path: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|e| format!("Failed to read prompt file '{}': {}", path.display(), e))?;
    if metadata.len() > MAX_LOOP_PROMPT_FILE_BYTES {
        return Err(format!(
            "Prompt file '{}' is larger than {} bytes",
            path.display(),
            MAX_LOOP_PROMPT_FILE_BYTES
        ));
    }
    let bytes = std::fs::read(path)
        .map_err(|e| format!("Failed to read prompt file '{}': {}", path.display(), e))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("Prompt file '{}' is not valid UTF-8", path.display()))?;
    if text.trim().is_empty() {
        return Err(format!("Prompt file '{}' is empty", path.display()));
    }
    Ok(text)
}

fn print_json<T: Serialize>(value: &T) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize JSON output: {}", e))?;
    crate::cli_println!("{}", json);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The arg structs are private to this file, so parsing goes through the
    /// crate's public `Cli` and is then destructured here. Precedent:
    /// `cli/mod.rs`'s `hidden_internal_verbs_still_parse_by_name`.
    fn create_args(extra: &[&str]) -> LoopCreateArgs {
        let mut argv = vec![
            "agentscommander",
            "loop",
            "create",
            "--project",
            "ProjectAlpha",
            "--name",
            "Daily Standup",
            "--cron",
            "0 9 * * *",
            "--room",
            "wg-1-dev-team",
            "--prompt",
            "Summarize status",
        ];
        argv.extend_from_slice(extra);
        let parsed = crate::cli::Cli::try_parse_from(argv).expect("loop create must parse");
        match parsed.command {
            Some(crate::cli::Commands::Loop(LoopArgs {
                command: LoopCommand::Create(args),
            })) => args,
            _ => panic!("expected `loop create`"),
        }
    }

    /// AC-1 - an explicit value parses and converts.
    #[test]
    fn session_start_accumulate_parses_and_converts() {
        let args = create_args(&["--session-start", "accumulate"]);
        assert_eq!(args.session_start, Some(SessionStartCli::Accumulate));
        assert_eq!(
            args.session_start.map(LoopSessionStart::from),
            Some(LoopSessionStart::Accumulate)
        );

        let args = create_args(&["--session-start", "fresh"]);
        assert_eq!(args.session_start, Some(SessionStartCli::Fresh));
        assert_eq!(
            args.session_start.map(LoopSessionStart::from),
            Some(LoopSessionStart::Fresh)
        );
    }

    /// AC-2 - the flag omitted parses as `None`. `create` turns that into
    /// `LoopPolicy::default()`; `update` leaves the stored value alone.
    #[test]
    fn session_start_absent_parses_as_none() {
        assert_eq!(create_args(&[]).session_start, None);

        let parsed = crate::cli::Cli::try_parse_from([
            "agentscommander",
            "loop",
            "update",
            "--project",
            "ProjectAlpha",
            "--loop",
            "daily-standup",
            "--name",
            "Renamed",
        ])
        .expect("loop update must parse");
        match parsed.command {
            Some(crate::cli::Commands::Loop(LoopArgs {
                command: LoopCommand::Update(args),
            })) => {
                assert_eq!(args.session_start, None);
                assert_eq!(args.session_start.map(LoopSessionStart::from), None);
            }
            _ => panic!("expected `loop update`"),
        }
    }

    /// AC-3 - an unrecognized value is a clap parse error naming both accepted
    /// values. No hand-written validation.
    #[test]
    fn session_start_rejects_an_unknown_value_and_lists_the_accepted_ones() {
        let parsed = crate::cli::Cli::try_parse_from([
            "agentscommander",
            "loop",
            "create",
            "--project",
            "ProjectAlpha",
            "--name",
            "Daily Standup",
            "--cron",
            "0 9 * * *",
            "--room",
            "wg-1-dev-team",
            "--prompt",
            "Summarize status",
            "--session-start",
            "resume",
        ]);
        let error = match parsed {
            Ok(_) => panic!("an unknown session-start value must not parse"),
            Err(error) => error.to_string(),
        };

        assert!(error.contains("fresh"), "error must list `fresh`:\n{error}");
        assert!(
            error.contains("accumulate"),
            "error must list `accumulate`:\n{error}"
        );
    }

    /// AC-4 (zero-effect) - adding the new flag does not disturb the busy flag
    /// parsed from the same command line.
    #[test]
    fn session_start_does_not_disturb_the_busy_coordinator_flag() {
        let without = create_args(&["--busy-coordinator", "skip"]);
        let with = create_args(&[
            "--busy-coordinator",
            "skip",
            "--session-start",
            "accumulate",
        ]);

        assert_eq!(without.busy_coordinator, Some(BusyCoordinatorCli::Skip));
        assert_eq!(with.busy_coordinator, without.busy_coordinator);
        assert_eq!(with.force_inject_when_busy, without.force_inject_when_busy);
        assert_eq!(without.session_start, None);
        assert_eq!(with.session_start, Some(SessionStartCli::Accumulate));
    }
}
