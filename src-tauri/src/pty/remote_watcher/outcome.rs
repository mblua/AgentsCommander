use std::time::Duration;

use tokio::sync::mpsc;

use crate::config::remote_activity_cache::{PersistedCiState, PersistedRepoCi};

use super::{
    axis_label, ci_base_interval, failure_interval, Axis, CiCompletionDecision, CiCompletionState,
    CiState, CompareAnswer, FailureKind, KeyOutcome, PathFacts, QueryKey, RemoteActivity,
    RemoteSweeper, RemoteTransition, RoundCtx, StalenessState, SweeperState, TransitionKind,
    TransitionMeta, CI_RUNNING_INTERVAL_SECS, DEFAULT_BRANCH_LABEL,
};

impl RemoteSweeper {
    pub(crate) fn apply_ci(
        &self,
        key: &QueryKey,
        outcome: KeyOutcome,
        indices: &[usize],
        ctx: &RoundCtx<'_>,
    ) {
        let KeyOutcome {
            ci: result,
            ci_suppressed: suppressed,
            ci_runs: runs,
            ci_completion: completion,
            ci_decision: decision,
            ..
        } = outcome;
        let Some(result) = result else {
            return;
        };
        let mut state = self.lock_state();
        let entry = state.keys.entry(key.clone()).or_default();
        let base = ci_base_interval(entry.ci.confirmed, ctx.ci_dial);

        if let Some(completion) = completion {
            entry.ci.completion = completion;
        }
        if suppressed {
            entry.ci.completion = CiCompletionState::default();
        }
        match result {
            Ok(_) if !suppressed && decision == Some(CiCompletionDecision::Pending) => {
                entry.ci.chip = CiState::Running;
                entry.ci.run_ids = entry.ci.completion.active_ids.clone();
                entry.ci.pull_requests = entry.ci.completion.active_prs.clone();
                entry.ci.failure = None;
                entry.ci.failure_interval = None;
                entry.ci.next_due = Some(ctx.now + Duration::from_secs(CI_RUNNING_INTERVAL_SECS));
            }
            Ok(_) if suppressed => {
                // A suppressed answer has no edge into or out of it: `confirmed =
                // None` breaks the transition chain both ways, like a fresh key.
                // Both suppression rules (resolved default branch; identical to
                // the default branch) publish `Idle` here.
                entry.ci.chip = CiState::Idle;
                entry.ci.confirmed = None;
                entry.ci.last_confirmed_at = Some(ctx.wall);
                entry.ci.failure_interval = None;
                entry.ci.run_ids.clear();
                entry.ci.pull_requests.clear();
                entry.ci.failure = None;
                entry.ci.next_due = Some(ctx.now + ci_base_interval(None, ctx.ci_dial));
                drop(state);
            }
            Ok(answer) => {
                let prior_confirmed = entry.ci.confirmed;
                let prior_last = entry.ci.last_confirmed_at;
                entry.ci.chip = answer;
                entry.ci.confirmed = Some(answer);
                entry.ci.last_confirmed_at = Some(ctx.wall);
                entry.ci.failure_interval = None;
                if answer == CiState::Running {
                    (entry.ci.run_ids, entry.ci.pull_requests) = runs;
                } else {
                    entry.ci.run_ids.clear();
                    entry.ci.pull_requests.clear();
                }
                entry.ci.failure = None;
                entry.ci.next_due = Some(ctx.now + ci_base_interval(Some(answer), ctx.ci_dial));
                drop(state);

                let kind = match (prior_confirmed, answer) {
                    (Some(CiState::Idle), CiState::Running) => Some(TransitionKind::CiStarted),
                    (Some(CiState::Running), CiState::Idle) => Some(TransitionKind::CiFinished),
                    _ => None,
                };
                if let Some(kind) = kind {
                    self.fan_out(
                        key,
                        TransitionMeta {
                            kind,
                            behind_by: None,
                            // The CI template renders no base label, so the CI axis
                            // never waits on, and never spends a call for, one.
                            base_branch: String::new(),
                            last_confirmed_at: prior_last,
                        },
                        indices,
                        ctx,
                    );
                }
            }
            Err(kind) if suppressed => {
                // A failed query on the resolved default branch keeps the chip
                // Idle: no CI is published for that branch either way. The
                // failure is not hidden - it still backs off, warns and can arm
                // the round gate - and `last_confirmed_at` is left untouched.
                let next = failure_interval(kind, base, entry.ci.failure_interval);
                entry.ci.chip = CiState::Idle;
                entry.ci.confirmed = None;
                entry.ci.failure_interval = Some(next);
                entry.ci.run_ids.clear();
                entry.ci.pull_requests.clear();
                entry.ci.failure = None;
                entry.ci.next_due = Some(ctx.now + next);
                drop(state);
                self.warn_failure(key, Axis::Ci, kind);
            }
            Err(kind) => {
                let next = failure_interval(kind, base, entry.ci.failure_interval);
                entry.ci.chip = CiState::Unknown;
                entry.ci.failure_interval = Some(next);
                entry.ci.run_ids.clear();
                entry.ci.pull_requests.clear();
                entry.ci.failure = Some(kind);
                entry.ci.next_due = Some(ctx.now + next);
                drop(state);
                self.warn_failure(key, Axis::Ci, kind);
            }
        }
    }

    pub(crate) fn apply_staleness(
        &self,
        key: &QueryKey,
        result: Option<CompareAnswer>,
        indices: &[usize],
        ctx: &RoundCtx<'_>,
    ) {
        let Some(result) = result else {
            return;
        };
        let mut state = self.lock_state();
        let base_label = state
            .base_branches
            .get(&key.nwo)
            .cloned()
            .unwrap_or_else(|| DEFAULT_BRANCH_LABEL.to_string());
        // #2131: a repo sitting on its own default branch has no branch of its
        // own to rebase, so the notice is suppressed there while the chip keeps
        // its orange bar. Fail open: the sentinel label means the default branch
        // is unresolved, which is not an identity and never suppresses (the same
        // rule #2126 applies to CI).
        let on_default_branch = base_label != DEFAULT_BRANCH_LABEL && key.branch == base_label;
        let entry = state.keys.entry(key.clone()).or_default();
        let base = ctx.staleness_dial;

        match result {
            Ok((answer, behind_by, ahead_by, _identical)) => {
                let prior_confirmed = entry.staleness.confirmed;
                let prior_last = entry.staleness.last_confirmed_at;
                // #2141 precedence, in this order, one decision branch:
                //   1. `on_default_branch`  -> #2131, unchanged, records `answer`.
                //   2. `ahead_by == Some(0)` -> no own commits, records `Current`.
                //   3. otherwise             -> today's behaviour; `None` fails open.
                // A repo on its own default branch also reports `ahead_by == 0`,
                // so case 1 is tested FIRST or the two rules disagree about what
                // was written to `confirmed`.
                let no_own_commits = !on_default_branch && ahead_by == Some(0);
                // Unlike `apply_ci`, which writes `confirmed = None` for a
                // suppressed answer ("no edge into or out of it"), a suppressed
                // staleness answer records `Current`. The axes differ on purpose:
                // CI has no deferred notice to keep alive, staleness does. Writing
                // `None` here, or leaving the prior value, consumes or never opens
                // the `Current -> Stale` edge, and the branch is never told again.
                let recorded = if no_own_commits {
                    StalenessState::Current
                } else {
                    answer
                };
                entry.staleness.chip = answer;
                entry.staleness.confirmed = Some(recorded);
                entry.staleness.behind_by = behind_by;
                entry.staleness.last_confirmed_at = Some(ctx.wall);
                entry.staleness.failure_interval = None;
                entry.staleness.next_due = Some(ctx.now + ctx.staleness_dial);
                drop(state);

                let stale = matches!(
                    (prior_confirmed, answer),
                    (Some(StalenessState::Current), StalenessState::Stale)
                );
                if stale && !on_default_branch && !no_own_commits {
                    self.fan_out(
                        key,
                        TransitionMeta {
                            kind: TransitionKind::BranchStale,
                            behind_by,
                            base_branch: base_label,
                            last_confirmed_at: prior_last,
                        },
                        indices,
                        ctx,
                    );
                }
            }
            Err(kind) => {
                let next = failure_interval(kind, base, entry.staleness.failure_interval);
                entry.staleness.chip = StalenessState::Unknown;
                entry.staleness.failure_interval = Some(next);
                entry.staleness.next_due = Some(ctx.now + next);
                drop(state);
                self.warn_failure(key, Axis::Staleness, kind);
            }
        }
    }

    pub(crate) fn fan_out(
        &self,
        key: &QueryKey,
        meta: TransitionMeta,
        indices: &[usize],
        ctx: &RoundCtx<'_>,
    ) {
        for index in indices {
            let fact = &ctx.facts[*index];
            for room_dir in &fact.room_dirs {
                self.send_transition(RemoteTransition {
                    repo_path: fact.path.clone(),
                    room_dir: room_dir.clone(),
                    nwo: key.nwo.clone(),
                    branch: fact.branch.clone().unwrap_or_default(),
                    head_sha: key.sha40.clone(),
                    kind: meta.kind,
                    behind_by: meta.behind_by,
                    base_branch: meta.base_branch.clone(),
                    observed_at: ctx.wall,
                    last_confirmed_at: meta.last_confirmed_at,
                });
            }
        }
    }

    /// The transition channel is best-effort by design: a stuck consumer must
    /// never slow a round, because chip freshness outranks a notice.
    pub(crate) fn send_transition(&self, transition: RemoteTransition) {
        match self.transitions.try_send(transition) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(dropped)) => {
                let drop_key = format!(
                    "{}|{}|{}",
                    dropped.room_dir, dropped.repo_path, dropped.head_sha
                );
                if self.lock_state().warned_transition_drops.insert(drop_key) {
                    log::warn!(
                        "[RemoteSweeper] transition channel full; dropped transition for {}",
                        dropped.repo_path
                    );
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                let mut state = self.lock_state();
                if !state.closed_warned {
                    state.closed_warned = true;
                    drop(state);
                    log::warn!(
                        "[RemoteSweeper] transition channel closed; transitions are dropped for \
                            the rest of this process"
                    );
                }
            }
        }
    }

    pub(crate) fn warn_failure(&self, key: &QueryKey, axis: Axis, kind: FailureKind) {
        let first = self
            .lock_state()
            .warned_failures
            .insert((key.clone(), axis));
        if first {
            log::warn!(
                "[RemoteSweeper] {} query failed for {}@{}: {:?}",
                axis_label(axis),
                key.nwo,
                key.sha40,
                kind
            );
        }
    }
}

pub(crate) fn persisted_ci_state(state: CiState) -> PersistedCiState {
    match state {
        CiState::Running => PersistedCiState::Running,
        CiState::Idle => PersistedCiState::Idle,
        CiState::Unknown => PersistedCiState::Unknown,
    }
}

/// #2473: the persisted reason code for a CI failure, published only while the
/// chip is `Unknown`.
pub(crate) fn persisted_failure_reason(kind: FailureKind) -> &'static str {
    match kind {
        FailureKind::Timeout => "ci-query-timeout",
        FailureKind::RateLimited => "ci-query-rate-limited",
        FailureKind::SecondaryRateLimited => "ci-query-secondary-rate-limited",
        FailureKind::NotAuthenticated => "ci-query-not-authenticated",
        FailureKind::Incomplete => "ci-query-incomplete",
        FailureKind::Other => "ci-query-failed",
    }
}

pub(crate) fn persisted_repo_ci(activity: &RemoteActivity) -> PersistedRepoCi {
    PersistedRepoCi {
        state: persisted_ci_state(activity.ci),
        run_ids: activity.run_ids.clone(),
        pull_requests: activity.pull_requests.clone(),
        unknown_reason: match activity.ci {
            CiState::Unknown => activity
                .failure
                .map(|kind| persisted_failure_reason(kind).to_string()),
            _ => None,
        },
    }
}

pub(crate) fn activity_for(state: &SweeperState, fact: &PathFacts) -> RemoteActivity {
    let (Some(nwo), Some(head_sha), Some(branch)) = (&fact.nwo, &fact.head_sha, &fact.branch)
    else {
        return RemoteActivity::unknown();
    };
    let key = QueryKey {
        nwo: nwo.clone(),
        sha40: head_sha.clone(),
        branch: branch.clone(),
    };
    match state.keys.get(&key) {
        Some(entry) => RemoteActivity {
            ci: entry.ci.chip,
            staleness: entry.staleness.chip,
            // `Some` only while Stale: the tooltip has nothing to say otherwise.
            behind_by: match entry.staleness.chip {
                StalenessState::Stale => entry.staleness.behind_by,
                _ => None,
            },
            run_ids: entry.ci.run_ids.clone(),
            pull_requests: entry.ci.pull_requests.clone(),
            failure: entry.ci.failure,
        },
        None => RemoteActivity::unknown(),
    }
}
