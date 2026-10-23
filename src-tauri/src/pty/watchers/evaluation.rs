use crate::config::settings::WatcherDedupe;
use crate::pty::watchers::frame;
use crate::pty::watchers::history;
use crate::pty::watchers::pattern;
use crate::pty::watchers::resolution::AgentResolution;
use crate::pty::watchers::resolution::ResolvedWatcher;
use crate::pty::watchers::truncate_row;
use crate::pty::watchers::RegisteredSession;
use crate::pty::watchers::WatcherSessionState;
use std::time::Duration;

/// What a match contributes to a payload, before the engine stamps identity on it.
pub(super) type Found = (Vec<Option<String>>, String, bool);

/// The status of one session at the end of a tick.
///
/// Built from the RESOLVED watcher set rather than from the state map, so a watcher that
/// reaches this agent but has never matched is still listed with a count of 0 - which is what
/// lets the window say "configured and waiting" instead of "nothing reaches this session".
pub(super) fn status_of(
    session: &RegisteredSession,
    agent: &AgentResolution,
) -> history::SessionStatus {
    history::SessionStatus {
        possibly_missed_frames: session.diff.possibly_missed_frames(),
        active_watchers: agent
            .running
            .iter()
            .map(|watcher| {
                let state = session.watchers.get(&watcher.id);
                history::WatcherActivityCounter {
                    watcher_id: watcher.id.clone(),
                    mode: watcher.mode,
                    count: state.map(|state| state.count).unwrap_or(0),
                    degraded: state.is_some_and(|state| state.degraded),
                }
            })
            .collect(),
    }
}

/// Pull the capture groups 1..n out of a match, in order, without group 0.
pub(super) fn captures_of(regex: &regex::Regex, text: &str) -> Vec<Option<String>> {
    regex
        .captures(text)
        .map(|found| {
            found
                .iter()
                .skip(1)
                .map(|group| group.map(|m| m.as_str().to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// The gate movement a `state` match would make **if it is actually emitted**.
///
/// Returned rather than applied, so a match the per-tick cap throws away cannot burn the gate
/// it never passed through. The caller commits it with `commit_gate`, on the one path that
/// emits.
pub(super) struct StateGateCommit {
    pub(super) last: (Vec<Option<String>>, String, u64),
}

impl WatcherSessionState {
    /// Record that this `state` match was emitted. Called only after the caps let it through.
    pub(super) fn commit_gate(&mut self, commit: StateGateCommit) {
        self.last = Some(commit.last);
    }
}

/// `state` mode over the FULL frame: the LOWEST matching logical row wins, mirroring
/// `rows::extract` (`context_scrape/rows.rs:19-26`), because a statusline always sits below
/// the transcript.
///
/// State mode does not use the frame diff at all: the whole frame is the reading, and the
/// engine already holds it whenever the stamp changed.
///
/// A transition to "no match" CLEARS the gate and emits nothing. The only consumer here is an
/// activity log, and "the prompt disappeared" is not a log entry; clearing is what lets an
/// identical re-appearance emit again, and it is why the payload needs no `present` field.
///
/// **What is committed here and what is not.** `generation` and `last_match_count` describe
/// the SCREEN, so they move on every tick that looked at one - and they must, or a count that
/// rose during a dropped tick would rise a second time on the next one. `last` describes what
/// was EMITTED, so it is handed back as a `StateGateCommit` and applied only when the match
/// really goes out; otherwise a match dropped by the cap would match the gate on the next tick
/// and the activation would never reappear.
pub(super) fn evaluate_state(
    state: &mut WatcherSessionState,
    compiled: &pattern::WatcherPattern,
    logical: &[frame::LogicalRow],
) -> Option<(Found, StateGateCommit)> {
    let regex = compiled.regex();
    let mut count = 0usize;
    let mut lowest: Option<&frame::LogicalRow> = None;
    for row in logical {
        if regex.is_match(&row.text) {
            count += 1;
            lowest = Some(row);
        }
    }

    if count > state.last_match_count {
        state.generation += 1;
    }
    state.last_match_count = count;

    let Some(lowest) = lowest else {
        // Nothing matched, so there is no match for a cap to drop: clearing is unconditional.
        state.last = None;
        return None;
    };

    let (row, row_truncated) = truncate_row(&lowest.text);
    let candidate = (captures_of(regex, &lowest.text), row, state.generation);
    if state.last.as_ref() == Some(&candidate) {
        return None;
    }
    Some((
        (candidate.0.clone(), candidate.1.clone(), row_truncated),
        StateGateCommit { last: candidate },
    ))
}

/// `occurrence` mode over the rows the diff declared evaluable: every match that survives the
/// dedupe layer and the cap is an event. There is no equality gate, by definition.
///
/// Returns the matches to emit and whether the cap cut the tick short.
pub(super) fn evaluate_occurrence(
    state: &mut WatcherSessionState,
    watcher: &ResolvedWatcher,
    compiled: &pattern::WatcherPattern,
    selected: &[&frame::LogicalRow],
    now: std::time::Instant,
    budget: usize,
) -> (Vec<Found>, bool) {
    let regex = compiled.regex();
    let window = Duration::from_millis(watcher.dedupe_window_ms);
    let mut found = Vec::new();

    for row in selected {
        if !regex.is_match(&row.text) {
            continue;
        }
        let captures = captures_of(regex, &row.text);

        // Layer 2. The key is over the FULL logical row or its captures, never over the
        // 256-byte payload row: two rows that differ only past the cap are two rows.
        let admitted = match watcher.dedupe {
            WatcherDedupe::None => true,
            WatcherDedupe::Row => state.keys.admit(&row.text, now, window),
            WatcherDedupe::Capture => {
                let key = captures
                    .iter()
                    .map(|group| group.as_deref().unwrap_or("\u{0}"))
                    .collect::<Vec<_>>()
                    .join("\u{1f}");
                state.keys.admit(&key, now, window)
            }
        };
        if !admitted {
            continue;
        }

        // Checked AFTER the dedupe so a suppressed repeat does not consume the budget, and
        // BEFORE the push so the overflow reaches neither the event nor the ring.
        if found.len() >= budget {
            return (found, true);
        }

        let (text, row_truncated) = truncate_row(&row.text);
        found.push((captures, text, row_truncated));
    }

    (found, false)
}
