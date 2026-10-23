/// #1171 - one watcher row of the draft the Settings modal holds in memory.
///
/// Only the three fields `reaches` and the budget depend on (plan 4.8). `pattern`, `mode`,
/// `dedupe` and `capturedAgainst` take part in neither and are deliberately not sent: the row
/// already shows its pattern, and `preview_watcher_pattern` answers compilability separately,
/// so carrying it here would inflate every debounced payload to restate an answer that is
/// already on screen next to this one.
#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherDraftEntry {
    pub id: String,
    pub enabled: bool,
    #[serde(default)]
    pub commands: Option<Vec<String>>,
}

/// #1171 - one agent row of the same draft.
///
/// The modal edits agents and watchers in ONE store and one Save writes both, so resolving
/// against the SAVED agent list would answer about a state the user has already left. Two of
/// the three agent edits over-report that way: deleting an agent leaves it named in a reach
/// list it will not be in, and changing an agent's `command` leaves a watcher reported as
/// reaching it under the old stem. Only adding an agent under-reports.
#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherAgentDraftEntry {
    pub id: String,
    pub label: String,
    pub command: String,
}

/// #1171 - one agent that a draft row's selector reaches.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherReachEntry {
    pub agent_id: String,
    pub agent_label: String,
    pub command_stem: String,
    /// Whether this row is enabled in the draft AND holds one of this agent's 8 slots once
    /// every other ENABLED row of the draft is counted. It is membership of the engine's own
    /// `running` list, and NOT a promise that the watcher will emit anything: a resolved
    /// watcher whose pattern does not compile is allocated a slot and is inert. Compilability
    /// is a separate dimension, answered per row by `preview_watcher_pattern`, and this field
    /// deliberately does not restate it. A disabled row is always false here, and the editor,
    /// which owns `enabled`, must say "disabled" rather than "budget".
    pub allocated: bool,
}

/// #1171 - the reach of one draft row.
///
/// Exactly one per requested row, in request order. It carries `id` back because the editor
/// filters unrecognised rows out of the request, so its table positions do not match the
/// response positions.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherReachRow {
    pub id: String,
    /// Every agent this row's SELECTOR reaches, whether or not the row is enabled. Reach is a
    /// property of the selector alone; `allocated` is where enablement and budget land.
    pub entries: Vec<WatcherReachEntry>,
}

/// The two passes behind `preview_watcher_reach`, pure and off the runtime.
///
/// Reach and allocation are different questions, and each gets its own pass over the SAME
/// draft, so no counterfactual budget is computed and no set of answers can disagree with
/// itself:
///
/// - **Pass A, every row forced enabled**, supplies `entries`. Reach does not depend on any
///   other row, so forcing enablement changes no row's answer but its own presence, and a
///   disabled row still shows the agents its selector reaches - the state where the control is
///   needed most.
/// - **Pass B, every row at its real draft `enabled`**, is the engine's own resolution and
///   supplies `allocated`.
///
/// Running one forced-enabled pass PER ROW instead would let nine rows all report a slot on an
/// agent that has eight, because each row's own pass silently displaces a different one.
///
/// Fixed points that the editor cannot produce but the contract still defines: a duplicate id
/// means the later row wins when the map is built and both response rows report that one
/// resolution, and an empty id is a legal key that sorts first and is not special-cased.
pub(super) fn resolve_draft_reach(
    watchers: &[WatcherDraftEntry],
    agents: &[WatcherAgentDraftEntry],
) -> Vec<WatcherReachRow> {
    use crate::config::settings::{WatcherConfig, WatcherEntry, WatcherMode};
    use crate::pty::watchers::{resolve_watchers, WatcherAgent};
    use std::collections::BTreeMap;

    let resolution_agents: Vec<WatcherAgent> = agents
        .iter()
        .map(|agent| WatcherAgent {
            id: agent.id.clone(),
            command: agent.command.clone(),
        })
        .collect();

    // `mode`, `pattern`, `dedupe` and `dedupeWindowMs` take no part in resolution and do not
    // travel, so they are placeholders here and are never read back out.
    let draft_map = |force_enabled: bool| -> BTreeMap<String, WatcherEntry> {
        watchers
            .iter()
            .map(|row| {
                (
                    row.id.clone(),
                    WatcherEntry::Valid(WatcherConfig {
                        enabled: force_enabled || row.enabled,
                        mode: WatcherMode::Occurrence,
                        pattern: String::new(),
                        commands: row.commands.clone(),
                        dedupe: Default::default(),
                        dedupe_window_ms: 0,
                        captured_against: None,
                    }),
                )
            })
            .collect()
    };

    let (reach_pass, _notices) = resolve_watchers(&resolution_agents, &draft_map(true));
    let (allocation_pass, _notices) = resolve_watchers(&resolution_agents, &draft_map(false));

    // Keyed by agent id and not iterated straight off `agents`, so a draft that names the same
    // agent twice produces one entry rather than two, matching the map `resolve_watchers`
    // itself builds.
    let mut agent_meta: BTreeMap<String, (String, String)> = BTreeMap::new();
    for agent in agents {
        let stem =
            crate::config::coding_agents_catalog::command_executable_basename(&agent.command)
                .unwrap_or_default();
        agent_meta.insert(agent.id.clone(), (agent.label.clone(), stem));
    }

    let mut rows = Vec::with_capacity(watchers.len());
    for row in watchers {
        let mut entries: Vec<WatcherReachEntry> = Vec::new();
        for (agent_id, (label, stem)) in &agent_meta {
            // Reach is `running` OR `over_budget`: together they hold everything whose selector
            // matches this agent.
            let reaches = reach_pass.get(agent_id).is_some_and(|resolution| {
                resolution.running.iter().any(|w| w.id == row.id)
                    || resolution.over_budget.iter().any(|id| id == &row.id)
            });
            if !reaches {
                continue;
            }
            let allocated = allocation_pass
                .get(agent_id)
                .is_some_and(|resolution| resolution.running.iter().any(|w| w.id == row.id));
            entries.push(WatcherReachEntry {
                agent_id: agent_id.clone(),
                agent_label: label.clone(),
                command_stem: stem.clone(),
                allocated,
            });
        }
        // `resolve_watchers` returns a `HashMap`, so a stable order has to be imposed here
        // rather than assumed: the Settings list must not reshuffle between keystrokes.
        entries.sort_by(|left, right| {
            left.agent_label
                .cmp(&right.agent_label)
                .then_with(|| left.agent_id.cmp(&right.agent_id))
        });
        rows.push(WatcherReachRow {
            id: row.id.clone(),
            entries,
        });
    }
    rows
}
