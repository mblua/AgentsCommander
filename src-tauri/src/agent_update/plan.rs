use super::emit_all;
use super::gate::AgentUpdateGate;
use crate::agent_version::{
    probe_version_cancellable_retained, probe_version_in, version_probe_args, AgentInstallCache,
    CancellableProbeOutcome, Completion, InstallState, ProbeOutcome, ProbeTicket,
    RetainedProbeOwner, Scheduling, INSTALL_CACHE_TTL, PROBE_TIMEOUT,
};
use crate::config::agent_command::{
    is_bare_program_token, normalize_legacy_agent_command, resolve_program, resolve_program_in,
};
use crate::config::agent_path::{effective_search_path, warm_login_shell_path};
use crate::config::coding_agents_catalog::{
    load_catalog_for_settings, CatalogUnavailable, CodingAgentDefinition,
};
use crate::config::settings::SettingsState;
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;
use tauri::AppHandle;

/// One command's update run: the shell sequences to execute and where.
#[derive(Debug, Clone)]
pub struct UpdateTarget {
    pub command: String,
    pub label: String,
    pub commands: Vec<String>,
    pub cwd: PathBuf,
}

/// The startup plan: what to prompt about and what to run.
pub struct AgentUpdatePlan {
    pub prompts: Vec<UpdateTarget>,
    pub updates: Vec<UpdateTarget>,
}

/// #1551 - one row of the read-only Settings "Auto-update" table.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateOverviewRow {
    pub key: String,
    pub label: String,
    pub command: String,
    pub color: String,
    pub update_commands: Vec<String>,
    pub install: InstallState,
}

/// Pure plan builder. Binding rules:
/// 0. Only commands in `registered_commands` (the command strings of
///    `settings.agents[]`, i.e. agents the user actually registered) are
///    considered: a catalog-only command is never prompted nor updated.
///    Zero registered commands -> empty plan.
/// 1. The FIRST catalog entry for a command is the effective entry (#1967):
///    distinct commands keep catalog order, and `label`/`commands` come from
///    that first entry. A later duplicate never contributes anything.
/// 2. An empty effective sequence means no prompt and no update: the command is
///    skipped entirely. Nothing is borrowed from a later duplicate, so a
///    non-empty duplicate can never re-enable an empty winner.
/// 3. `answers[command]` absent -> prompt (default No on timeout);
///    `Some(true)` -> update now; `Some(false)` -> nothing.
///
/// Prompts and updates are disjoint by construction.
pub fn build_update_plan(
    catalog: &[crate::config::coding_agents_catalog::CodingAgentDefinition],
    registered_commands: &HashSet<String>,
    answers: &BTreeMap<String, bool>,
    default_cwd: PathBuf,
) -> AgentUpdatePlan {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut prompts: Vec<UpdateTarget> = Vec::new();
    let mut updates: Vec<UpdateTarget> = Vec::new();

    for entry in catalog {
        if !registered_commands.contains(&entry.command) {
            continue; // catalog-only command: never prompted, never updated
        }
        // First occurrence wins - including an empty sequence, which suppresses
        // the command for this pass (a later duplicate cannot re-enable it).
        if !seen.insert(entry.command.as_str()) {
            continue;
        }
        if entry.update_commands.is_empty() {
            continue; // effective sequence empty: no prompt, no update
        }
        let target = UpdateTarget {
            command: entry.command.clone(),
            label: entry.label.clone(),
            commands: entry.update_commands.clone(),
            cwd: default_cwd.clone(),
        };
        match answers.get(&entry.command) {
            None => prompts.push(target),
            Some(true) => updates.push(target),
            Some(false) => {}
        }
    }

    AgentUpdatePlan { prompts, updates }
}

/// #1551/#1967 - one row per DISTINCT effective command with a non-empty
/// sequence, in catalog order. The first entry of a command wins: its
/// key/label/color and its exact sequence are used, a later duplicate adds no
/// row, and an empty first sequence suppresses the command for good (a later
/// non-empty duplicate cannot bring it back). Commands without an install entry
/// are `checking` (seq 0).
pub fn build_update_overview_rows(
    catalog: &[CodingAgentDefinition],
    install_by_command: &HashMap<String, InstallState>,
) -> Vec<AgentUpdateOverviewRow> {
    let mut seen: HashSet<&str> = HashSet::new();
    catalog
        .iter()
        .filter(|entry| {
            if !seen.insert(entry.command.as_str()) {
                return false; // first effective entry wins
            }
            !entry.update_commands.is_empty()
        })
        .map(|entry| AgentUpdateOverviewRow {
            key: entry.key.clone(),
            label: entry.label.clone(),
            command: entry.command.clone(),
            color: entry.color.clone(),
            update_commands: entry.update_commands.clone(),
            install: install_by_command
                .get(&entry.command)
                .cloned()
                .unwrap_or_else(InstallState::checking),
        })
        .collect()
}

/// #1551 - resolve + probe policy for one catalog command. Executes a process ONLY for a
/// bare token whose stem has a built-in probe (plan 5.2): a project-authored catalog can
/// name `claude` and get the user's own PATH `claude`, but can never point the probe at a
/// bundled binary. Explicit paths and unknown stems report presence only (`unprobed`).
pub(crate) fn resolve_command_install_probe(
    command: &str,
) -> Result<(PathBuf, &'static [&'static str]), InstallState> {
    resolve_command_install_probe_with(command, resolve_program)
}

/// #2589 - `resolve_command_install_probe` against an explicit search path.
pub(crate) fn resolve_command_install_probe_in(
    command: &str,
    search_path: &OsStr,
) -> Result<(PathBuf, &'static [&'static str]), InstallState> {
    resolve_command_install_probe_with(command, |token| resolve_program_in(token, search_path))
}

pub(crate) fn resolve_command_install_probe_with(
    command: &str,
    resolve: impl FnOnce(&str) -> Option<PathBuf>,
) -> Result<(PathBuf, &'static [&'static str]), InstallState> {
    let Ok(normalized) = normalize_legacy_agent_command(command) else {
        return Err(InstallState::missing("empty command".to_string()));
    };
    let token = normalized.shell;
    let bare = is_bare_program_token(&token);
    let Some(path) = resolve(&token) else {
        return Err(InstallState::missing(if bare {
            format!("'{token}' was not found on PATH")
        } else {
            format!("'{token}' is not a file")
        }));
    };
    if !bare {
        return Err(InstallState::unprobed(
            &path,
            "explicit path: version not probed".to_string(),
        ));
    }
    let stem = Path::new(&token)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let Some(args) = version_probe_args(&stem) else {
        return Err(InstallState::unprobed(
            &path,
            format!("no built-in version probe for '{stem}'"),
        ));
    };
    Ok((path, args))
}

pub async fn probe_command_install_state(command: &str) -> InstallState {
    warm_login_shell_path().await;
    probe_command_install_state_in(command, &effective_search_path()).await
}

/// #2589 - resolve AND execute the probe with `search_path`, so both the agent
/// and its `#!/usr/bin/env` interpreter are found.
pub(crate) async fn probe_command_install_state_in(
    command: &str,
    search_path: &OsStr,
) -> InstallState {
    let (path, args) = match resolve_command_install_probe_in(command, search_path) {
        Ok(probe) => probe,
        Err(state) => return state,
    };
    match probe_version_in(&path, args, PROBE_TIMEOUT, search_path).await {
        ProbeOutcome::Version(version) => InstallState::installed(version, &path),
        ProbeOutcome::Failed(detail) => InstallState::probe_failed(&path, detail),
    }
}

pub(crate) enum TargetProbeOutcome {
    Completed(InstallState),
    Cancelled,
    CleanupFailed(String),
}

pub(crate) async fn probe_command_install_state_cancellable<I, O>(
    command: &str,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    owner_slot: &mut RetainedProbeOwner,
    owner_installed: I,
    cancellation_observed: O,
) -> TargetProbeOutcome
where
    I: FnOnce(),
    O: FnOnce(),
{
    if *cancel.borrow() {
        return TargetProbeOutcome::Cancelled;
    }
    let (path, args) = match resolve_command_install_probe(command) {
        Ok(probe) => probe,
        Err(state) => return TargetProbeOutcome::Completed(state),
    };
    match probe_version_cancellable_retained(
        &path,
        args,
        PROBE_TIMEOUT,
        &effective_search_path(),
        cancel,
        owner_slot,
        owner_installed,
        cancellation_observed,
    )
    .await
    {
        CancellableProbeOutcome::Completed(ProbeOutcome::Version(version)) => {
            TargetProbeOutcome::Completed(InstallState::installed(version, &path))
        }
        CancellableProbeOutcome::Completed(ProbeOutcome::Failed(detail)) => {
            TargetProbeOutcome::Completed(InstallState::probe_failed(&path, detail))
        }
        CancellableProbeOutcome::Cancelled => TargetProbeOutcome::Cancelled,
        CancellableProbeOutcome::CleanupFailed(detail) => {
            log::warn!("[agent-update] cancellable probe cleanup defective: {detail}");
            TargetProbeOutcome::CleanupFailed(detail)
        }
    }
}

/// #1551 - the probe a scheduled task runs; production = `probe_command_install_state`.
pub type ProbeFn =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = InstallState> + Send>> + Send + Sync>;

pub fn production_probe() -> ProbeFn {
    Arc::new(|command| Box::pin(async move { probe_command_install_state(&command).await }))
}

/// #1551 - instant: never awaits a probe, never waits on the gate, never holds a lock across
/// an await. Probes are scheduled ONLY once the startup pass is finished, so a version probe
/// can never overlap an update of the same CLI, and ONLY through the single-lock
/// `lookup_or_begin`, so two overlapping calls can never open two tickets.
/// #1967 P4 - `Err` when the persisted catalog is unavailable; nothing is probed or
/// scheduled in that case.
pub async fn update_overview(
    app: &AppHandle,
    settings: &SettingsState,
    gate: &AgentUpdateGate,
    cache: &Arc<AgentInstallCache>,
) -> Result<Vec<AgentUpdateOverviewRow>, CatalogUnavailable> {
    update_overview_with(app, settings, gate, cache, production_probe()).await
}

/// `update_overview` with an injectable probe so the scheduling tests control timing.
pub async fn update_overview_with(
    app: &AppHandle,
    settings: &SettingsState,
    gate: &AgentUpdateGate,
    cache: &Arc<AgentInstallCache>,
    probe: ProbeFn,
) -> Result<Vec<AgentUpdateOverviewRow>, CatalogUnavailable> {
    let settings = settings
        .current_snapshot()
        .await
        .map_err(|error| CatalogUnavailable {
            code: "settings_unavailable".to_string(),
            path: String::new(),
            reason: error.to_string(),
        })?;
    let catalog = load_catalog_for_settings(&settings)?;
    // Read the gate ONCE, before any cache operation: that order is what makes
    // "finished implies the post-pass generation" hold (plan 5.4 step 8).
    let pass_finished = gate.is_finished();
    let now = Instant::now();
    let mut install_by_command: HashMap<String, InstallState> = HashMap::new();
    let mut seen: HashSet<&str> = HashSet::new();

    for entry in &catalog {
        // First effective entry wins: an empty winner is skipped (no row, no
        // probe) and a later duplicate is ignored entirely.
        if !seen.insert(entry.command.as_str()) {
            continue;
        }
        if entry.update_commands.is_empty() {
            continue;
        }
        match cache.lookup_or_begin(&entry.command, now, INSTALL_CACHE_TTL, pass_finished) {
            Scheduling::Fresh(state) => {
                install_by_command.insert(entry.command.clone(), state);
            }
            // Another call's probe is running, or the pass is not finished yet:
            // the row stays `checking` and nothing is scheduled.
            Scheduling::InFlight | Scheduling::Deferred => {}
            Scheduling::Began(ticket) => {
                spawn_probe_task(app, &entry.command, ticket, Arc::clone(&probe));
            }
        }
    }

    Ok(build_update_overview_rows(&catalog, &install_by_command))
}

/// #1551 - the ONE spawn site for probe tasks: the Settings-triggered scheduling of
/// `update_overview_with` and the post-pass scheduling of `schedule_post_update_probes`
/// both come through here. `tauri::async_runtime::spawn`, never `tokio::spawn`, so it
/// runs from any caller context.
pub(crate) fn spawn_probe_task(
    app: &AppHandle,
    command: &str,
    ticket: ProbeTicket,
    probe: ProbeFn,
) {
    let app = app.clone();
    let command = command.to_string();
    tauri::async_runtime::spawn(async move {
        probe_commit_announce(&app, &command, ticket, move |c| probe(c)).await;
    });
}

/// #1551 - the probe task body. Announces `agent_install_state_changed` ONLY for a state
/// committed to the current cache generation: a completion rejected by an invalidation is
/// re-probed once on the same (renewed) ticket; a second rejection emits nothing and drops
/// the ticket, which frees the slot so the next overview call schedules fresh work. At most
/// two processes per command per overview call.
pub(crate) async fn probe_commit_announce<F, Fut>(
    app: &AppHandle,
    command: &str,
    ticket: ProbeTicket,
    probe: F,
) where
    F: Fn(String) -> Fut,
    Fut: Future<Output = InstallState>,
{
    let committed = match ticket.complete(probe(command.to_string()).await) {
        Completion::Committed(state) => Some(state),
        Completion::Stale(retry) => match retry.complete(probe(command.to_string()).await) {
            Completion::Committed(state) => Some(state),
            Completion::Stale(abandoned) => {
                drop(abandoned);
                None
            }
        },
    };
    if let Some(install) = committed {
        emit_all(
            app,
            "agent_install_state_changed",
            json!({ "command": command, "install": install }),
        );
    }
}
