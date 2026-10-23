use crate::config::coding_agents_catalog::command_executable_basename;
use crate::config::settings::WatcherDedupe;
use crate::config::settings::WatcherEntry;
use crate::config::settings::WatcherMode;
use crate::pty::watchers::MAX_DEDUPE_WINDOW_MS;
use crate::pty::watchers::WATCHERS_PER_AGENT_BUDGET;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

/// One enabled, well-formed watcher, ready for a tick.
///
/// Shared behind an `Arc` because the same watcher usually reaches many agents and its
/// pattern string must not be cloned once per agent per tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWatcher {
    pub id: String,
    pub mode: WatcherMode,
    pub pattern: String,
    pub dedupe: WatcherDedupe,
    /// Already clamped to `MAX_DEDUPE_WINDOW_MS`.
    pub dedupe_window_ms: u64,
}

/// What resolution needs to know about one configured agent: `settings.agents[i]`, reduced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatcherAgent {
    pub id: String,
    pub command: String,
}

/// The watchers that apply to one agent right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentResolution {
    /// In `BTreeMap` key order, at most `WATCHERS_PER_AGENT_BUDGET`.
    pub running: Vec<Arc<ResolvedWatcher>>,
    /// The ids that reach this agent but fell outside the budget, in the same order.
    pub over_budget: Vec<String>,
}

/// Something resolution wants to say, at most once per changed value.
///
/// Returned rather than logged, so the preview commands can run the exact same rule on every
/// keystroke without writing anything to `app.log`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionNotice {
    /// What the notice is ABOUT, kind-prefixed: a watcher id or an agent id. The log-once
    /// map is keyed by this and holds one entry per subject, so it is bounded by the number
    /// of configured watchers and agents. Keying by the message instead would let a user
    /// typing in Settings accumulate one entry per keystroke, which is the mistake
    /// `context_scrape`'s compile cache documents at `mod.rs:127-132`.
    pub subject: String,
    /// The value the message was derived from. A changed detail logs again; an unchanged one
    /// stays silent however many ticks pass.
    pub detail: String,
    pub message: String,
}

/// Cross the configured agents with the configured watchers.
///
/// Pure: no logging, no locks, no I/O. Called every tick by the engine's pattern source and
/// on every keystroke by `preview_watcher_reach`, and both get the same answer because there
/// is only one rule.
///
/// An agent appears in the map only when at least one watcher reaches it, so "the map is
/// empty" is exactly "there is nothing to do this tick" - which is what lets the tick return
/// before touching any session.
pub fn resolve_watchers(
    agents: &[WatcherAgent],
    watchers: &BTreeMap<String, WatcherEntry>,
) -> (HashMap<String, AgentResolution>, Vec<ResolutionNotice>) {
    let mut notices = Vec::new();
    let mut usable: Vec<(Arc<ResolvedWatcher>, Option<Vec<String>>)> = Vec::new();

    for (id, entry) in watchers {
        let config = match entry {
            WatcherEntry::Valid(config) => config,
            WatcherEntry::Invalid(value) => {
                let detail = value.to_string();
                notices.push(ResolutionNotice {
                    subject: format!("invalid:{id}"),
                    message: format!(
                        "[watchers] watcher '{id}' is not a valid watcher and is being skipped; \
                         every other watcher and every other setting is unaffected: {detail}"
                    ),
                    detail,
                });
                continue;
            }
        };

        // Disabled is a state the user chose, not a problem. It keeps its configuration and
        // says nothing.
        if !config.enabled {
            continue;
        }

        // A selector that does not tokenize skips the WHOLE watcher. Never "reaches
        // everything": a typo in one entry must not silently widen a pattern to every agent.
        let selector = match &config.commands {
            None => None,
            Some(list) => {
                let mut stems = Vec::with_capacity(list.len());
                let mut broken: Option<String> = None;
                for token in list {
                    match command_executable_basename(token) {
                        Some(stem) => stems.push(stem),
                        None => {
                            broken = Some(token.clone());
                            break;
                        }
                    }
                }
                if let Some(token) = broken {
                    let detail = list.join("\u{1f}");
                    notices.push(ResolutionNotice {
                        subject: format!("commands:{id}"),
                        message: format!(
                            "[watchers] watcher '{id}' is being skipped: its commands selector \
                             entry '{token}' is not a command. A watcher with an unreadable \
                             selector reaches nobody, never everybody"
                        ),
                        detail,
                    });
                    continue;
                }
                Some(stems)
            }
        };

        let mut dedupe_window_ms = config.dedupe_window_ms;
        if dedupe_window_ms > MAX_DEDUPE_WINDOW_MS {
            notices.push(ResolutionNotice {
                subject: format!("clamp:{id}"),
                message: format!(
                    "[watchers] watcher '{id}' asks for a {dedupe_window_ms} ms dedupe window; \
                     clamping to {MAX_DEDUPE_WINDOW_MS} ms"
                ),
                detail: dedupe_window_ms.to_string(),
            });
            dedupe_window_ms = MAX_DEDUPE_WINDOW_MS;
        }

        usable.push((
            Arc::new(ResolvedWatcher {
                id: id.clone(),
                mode: config.mode,
                pattern: config.pattern.clone(),
                dedupe: config.dedupe,
                dedupe_window_ms,
            }),
            selector,
        ));
    }

    let mut resolved: HashMap<String, AgentResolution> = HashMap::new();
    if usable.is_empty() {
        return (resolved, notices);
    }

    for agent in agents {
        // An agent whose own command does not tokenize is reached by selectorless watchers
        // and by no watcher with a selector: there is no stem to compare against, and
        // guessing one would be inventing a match. `validate_agent_commands`
        // (`settings.rs:1473-1475`) already rejects such a command on save.
        let agent_stem = command_executable_basename(&agent.command);

        let mut running = Vec::new();
        let mut over_budget = Vec::new();
        for (watcher, selector) in &usable {
            let reaches = match selector {
                None => true,
                Some(stems) => agent_stem
                    .as_ref()
                    .is_some_and(|stem| stems.iter().any(|candidate| candidate == stem)),
            };
            if !reaches {
                continue;
            }
            if running.len() < WATCHERS_PER_AGENT_BUDGET {
                running.push(Arc::clone(watcher));
            } else {
                over_budget.push(watcher.id.clone());
            }
        }

        if running.is_empty() {
            continue;
        }
        if !over_budget.is_empty() {
            let detail = over_budget.join(",");
            notices.push(ResolutionNotice {
                subject: format!("budget:{}", agent.id),
                message: format!(
                    "[watchers] agent '{}' is over the {WATCHERS_PER_AGENT_BUDGET}-watcher \
                     budget; these are configured but not running on it: {detail}",
                    agent.id
                ),
                detail,
            });
        }
        resolved.insert(
            agent.id.clone(),
            AgentResolution {
                running,
                over_budget,
            },
        );
    }

    (resolved, notices)
}

/// Log-once bookkeeping for `ResolutionNotice`s, across ticks.
///
/// Resolution runs five times a second, so every one of these messages would otherwise be a
/// five-per-second log line for as long as the configuration stays wrong. Keyed by subject
/// and holding the last detail logged for it: an unchanged problem stays silent, a CHANGED
/// one speaks again, and the map cannot grow past one entry per watcher and agent.
#[derive(Default)]
pub struct ResolutionLog {
    pub(super) last: Mutex<HashMap<String, String>>,
}

impl ResolutionLog {
    /// Log the notices whose subject is new or whose detail changed. Returns how many lines
    /// were actually written, so "logged once across N ticks" is something a test can assert
    /// rather than something a reader has to trust.
    pub fn publish(&self, notices: Vec<ResolutionNotice>) -> usize {
        if notices.is_empty() {
            return 0;
        }
        let mut written = 0;
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        for notice in notices {
            let changed = last
                .get(&notice.subject)
                .is_none_or(|previous| previous != &notice.detail);
            if changed {
                log::warn!("{}", notice.message);
                last.insert(notice.subject, notice.detail);
                written += 1;
            }
        }
        written
    }

    /// Number of subjects currently remembered. The bound this type exists to keep.
    #[cfg(test)]
    pub(super) fn remembered(&self) -> usize {
        self.last.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}
