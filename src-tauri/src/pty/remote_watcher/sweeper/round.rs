use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use futures::stream::StreamExt;

use crate::config::remote_activity_cache::PersistedRepoCi;

use super::super::{
    activity_for, due, persisted_repo_ci, query_key, CiState, KeyOutcome, KeyPlan, PathFacts,
    QueryKey, RemoteActivity, RemoteActivityPayload, SnapshotPersistence, StalenessCarry,
    DEFAULT_BRANCH_LABEL, GH_BUDGET_BURST, MAX_GH_CALLS_PER_MINUTE, QUERY_CONCURRENCY,
};
use super::RemoteSweeper;

pub(super) struct RoundReservations {
    pub(super) pending_base: Vec<(String, String)>,
    pub(super) admitted: Vec<(QueryKey, KeyPlan)>,
    pub(super) completion_reserved: HashSet<QueryKey>,
    pub(super) identity_reserved: HashSet<QueryKey>,
}

impl RemoteSweeper {
    /// Plan due queries and refresh the round budget gate.
    pub(super) fn plan_round_queries(
        &self,
        groups: &BTreeMap<QueryKey, Vec<usize>>,
        now: Instant,
        ci_enabled: bool,
        staleness_enabled: bool,
        gh_path: Option<&Path>,
    ) -> (Vec<(QueryKey, KeyPlan)>, bool) {
        let mut plans: Vec<(QueryKey, KeyPlan)> = Vec::with_capacity(groups.len());
        {
            let state = self.lock_state();
            for key in groups.keys() {
                let ci_due = match state.keys.get(key) {
                    Some(entry) => due(now, entry.ci.next_due),
                    None => true,
                };
                let staleness_due = match state.keys.get(key) {
                    Some(entry) => due(now, entry.staleness.next_due),
                    None => true,
                };
                plans.push((
                    key.clone(),
                    KeyPlan {
                        ci: ci_enabled && gh_path.is_some() && ci_due,
                        staleness: staleness_enabled && gh_path.is_some() && staleness_due,
                    },
                ));
            }
        }

        // Refill the global `gh` budget before any call is considered, then
        // read the gate. A gated round issues NO `gh` call: no resolution, no
        // query. Nothing else about the round changes.
        let throttled = {
            let mut state = self.lock_state();
            let elapsed = state
                .budget_refilled_at
                .map(|last| now.saturating_duration_since(last))
                .unwrap_or_default();
            state.budget_tokens = (state.budget_tokens
                + elapsed.as_secs_f64() * MAX_GH_CALLS_PER_MINUTE / 60.0)
                .min(GH_BUDGET_BURST);
            state.budget_refilled_at = Some(now);
            match state.throttled_until {
                Some(until) if now < until => true,
                _ => {
                    state.throttled_until = None;
                    false
                }
            }
        };

        // Priority order, built once: a key the user is watching (`Running`)
        // first, then the least recently confirmed. A missing entry or a `None`
        // timestamp sorts first (never checked, so most owed) and the trailing
        // key makes the order total.
        let mut due_keys: Vec<(QueryKey, KeyPlan)> = plans
            .iter()
            .filter(|(_, plan)| plan.ci || plan.staleness)
            .cloned()
            .collect();
        {
            let state = self.lock_state();
            due_keys.sort_by_cached_key(|(key, _)| {
                let entry = state.keys.get(key);
                (
                    if entry.map(|e| e.ci.confirmed) == Some(Some(CiState::Running)) {
                        0u8
                    } else {
                        1u8
                    },
                    entry.and_then(|e| e.ci.last_confirmed_at),
                    entry.and_then(|e| e.staleness.last_confirmed_at),
                    key.clone(),
                )
            });
        }

        (due_keys, throttled)
    }

    pub(super) fn carry_staleness(&self, groups: &BTreeMap<QueryKey, Vec<usize>>) {
        // #2141: retire keys no path maps to any more, carrying the STALENESS
        // axis to a live key on the same `(nwo, branch)` first. A local commit
        // moves `head_sha`, so it retires the old key and creates a new one that
        // would otherwise start at `confirmed = None`; `None -> Stale` is not the
        // edge the notice fires on, so the branch would never be told.
        //
        // This runs BEFORE the apply loop, which is the hard requirement: the old
        // retirement site sat after it, so the commit's own round decided with
        // `None` and recorded `Stale`, and without a rebase no edge ever occurred
        // again. Running it before `plans` as well is safe, not required: an
        // absent key is already due, and a seeded one is due only because
        // `next_due` is NOT carried (see below).
        //
        // Only the staleness axis moves. A new commit has genuinely unknown CI,
        // and seeding it would assert a run state nobody observed.
        //
        // The carry lives inside ONE round and is not a guarantee: if the old key
        // is retired with no live key on its `(nwo, branch)` to receive it — the
        // path left the work list, the last room closed, `.git` went unreadable,
        // the repo was archived — the memory is gone, and the next observation of
        // that branch starts at `None`, which is not an edge. That case is silent
        // today too, so nothing regresses, but do not read this block as a
        // promise that survives a path dropping out of a round.
        {
            let mut state = self.lock_state();
            // Donors are computed BEFORE anything is deleted, and only from keys
            // this round retires. A live key is never a donor: two paths on one
            // branch at different commits (worktrees, or two clones on the same
            // branch) must not seed each other.
            let mut donors: HashMap<(String, String), (QueryKey, StalenessCarry)> = HashMap::new();
            for (key, entry) in state
                .keys
                .iter()
                .filter(|(key, _)| !groups.contains_key(*key))
            {
                let Some(confirmed) = entry.staleness.confirmed else {
                    continue;
                };
                let carry = StalenessCarry {
                    confirmed,
                    last_confirmed_at: entry.staleness.last_confirmed_at,
                };
                let id = (key.nwo.clone(), key.branch.clone());
                match donors.get(&id) {
                    // Most recently confirmed wins; `None` never beats `Some`; a
                    // remaining tie goes to the greater `sha40`, so the choice is
                    // deterministic rather than hash-order dependent.
                    Some((held_key, held))
                        if (held.last_confirmed_at, &held_key.sha40)
                            >= (carry.last_confirmed_at, &key.sha40) => {}
                    _ => {
                        donors.insert(id, (key.clone(), carry));
                    }
                }
            }

            state.keys.retain(|key, _| groups.contains_key(key));

            if !donors.is_empty() {
                for key in groups.keys() {
                    let Some((_, carry)) = donors.get(&(key.nwo.clone(), key.branch.clone()))
                    else {
                        continue;
                    };
                    let entry = state.keys.entry(key.clone()).or_default();
                    // A key that already has a confirmed answer of its own keeps
                    // it. `chip`, `behind_by`, `next_due` and `failure_interval`
                    // are never carried: the new commit is un-queried, and a
                    // carried `next_due` would leave it not due in the very round
                    // that has to decide.
                    if entry.staleness.confirmed.is_none() {
                        entry.staleness.confirmed = Some(carry.confirmed);
                        entry.staleness.last_confirmed_at = carry.last_confirmed_at;
                    }
                }
            }
        }
    }

    pub(super) fn reserve_queries(
        &self,
        due_keys: &[(QueryKey, KeyPlan)],
        groups: &BTreeMap<QueryKey, Vec<usize>>,
        facts: &[PathFacts],
        gh_path: Option<&Path>,
        throttled: bool,
    ) -> RoundReservations {
        // Reserve-before-issue: every `gh` call this round can make - the
        // per-`nwo` base-branch resolution, the axis queries and the extra
        // identity compare - is subtracted from the bucket BEFORE it is issued.
        // A key that cannot pay is dropped from the round: no outcome, no
        // failure, chip and `next_due` untouched, first in line next round.
        //
        // The label chain runs on either axis, once per distinct nwo, and is
        // cached for the process lifetime: CI needs the name to decide the
        // identical-to-default suppression, staleness to render `%BASE%`.
        let mut pending_base: Vec<(String, String)> = Vec::new();
        let mut admitted: Vec<(QueryKey, KeyPlan)> = Vec::new();
        let mut completion_reserved: HashSet<QueryKey> = HashSet::new();
        let mut identity_reserved: HashSet<QueryKey> = HashSet::new();
        let mut unresolvable_nwos: HashSet<String> = HashSet::new();
        if gh_path.is_some() && !throttled {
            let mut state = self.lock_state();
            for (key, plan) in due_keys {
                // A key is never queried with a base label a resolution would
                // have supplied, so suppression decisions are unchanged.
                if unresolvable_nwos.contains(&key.nwo) {
                    continue;
                }
                let resolved = state.base_branches.contains_key(&key.nwo)
                    || pending_base.iter().any(|(nwo, _)| nwo == &key.nwo);
                // An unresolved label must assume the worst case; step 7 refunds
                // the reservation when the compare is not issued.
                let identity = u32::from(
                    plan.ci
                        && !plan.staleness
                        && state.base_branches.get(&key.nwo).is_none_or(|label| {
                            label.as_str() != DEFAULT_BRANCH_LABEL && *label != key.branch
                        }),
                );
                let corroboration = plan.ci
                    && state.keys.get(key).is_some_and(|entry| {
                        entry.ci.confirmed == Some(CiState::Running)
                            && !entry.ci.completion.known.is_empty()
                    });
                let cost = u32::from(plan.staleness)
                    + u32::from(plan.ci)
                    + identity
                    + if corroboration { 3 } else { 0 };
                let needed = f64::from(cost) + if resolved { 0.0 } else { 1.0 };
                if state.budget_tokens < needed {
                    if !resolved {
                        unresolvable_nwos.insert(key.nwo.clone());
                    }
                    continue;
                }
                if !resolved {
                    let Some(index) = groups.get(key).and_then(|group| group.first()) else {
                        continue;
                    };
                    pending_base.push((key.nwo.clone(), facts[*index].path.clone()));
                }
                state.budget_tokens -= needed;
                if identity == 1 {
                    identity_reserved.insert(key.clone());
                }
                if corroboration {
                    completion_reserved.insert(key.clone());
                }
                admitted.push((key.clone(), *plan));
            }
        }
        RoundReservations {
            pending_base,
            admitted,
            completion_reserved,
            identity_reserved,
        }
    }

    pub(super) async fn query_round(
        &self,
        gh_path: Option<&Path>,
        admitted: Vec<(QueryKey, KeyPlan)>,
        completion_reserved: &HashSet<QueryKey>,
    ) -> Vec<(QueryKey, KeyOutcome)> {
        let mut outcomes: Vec<(QueryKey, KeyOutcome)> = Vec::new();
        if let Some(gh) = gh_path {
            let query_keys: Vec<(QueryKey, KeyPlan)> = admitted;
            let defaults: HashMap<String, Option<String>> = {
                let state = self.lock_state();
                query_keys
                    .iter()
                    .map(|(key, _)| {
                        (
                            key.nwo.clone(),
                            state
                                .base_branches
                                .get(&key.nwo)
                                .filter(|label| label.as_str() != DEFAULT_BRANCH_LABEL)
                                .cloned(),
                        )
                    })
                    .collect()
            };
            let completion_inputs: HashMap<_, _> = {
                let state = self.lock_state();
                query_keys
                    .iter()
                    .map(|(key, _)| {
                        (
                            key.clone(),
                            state
                                .keys
                                .get(key)
                                .map(|entry| (entry.ci.completion.clone(), entry.ci.confirmed))
                                .unwrap_or_default(),
                        )
                    })
                    .collect()
            };
            let spawner = Arc::clone(&self.spawner);
            let gh = gh.to_path_buf();
            outcomes = futures::stream::iter(query_keys.into_iter().map(|(key, plan)| {
                let spawner = Arc::clone(&spawner);
                let gh = gh.clone();
                let default_branch = defaults.get(&key.nwo).cloned().flatten();
                let (completion, confirmed) =
                    completion_inputs.get(&key).cloned().unwrap_or_default();
                let reserved = completion_reserved.contains(&key);
                async move {
                    let outcome = query_key(
                        &spawner,
                        &gh,
                        &key,
                        plan,
                        default_branch.as_deref(),
                        (completion, confirmed),
                        reserved,
                    )
                    .await;
                    (key, outcome)
                }
            }))
            .buffer_unordered(QUERY_CONCURRENCY)
            .collect()
            .await;
        }

        outcomes
    }

    pub(super) async fn publish_round(&self, paths: &[String], facts: &[PathFacts], now: Instant) {
        let live: HashSet<String> = paths.iter().cloned().collect();
        let mut activities: Vec<RemoteActivity> = Vec::with_capacity(facts.len());
        {
            let state = self.lock_state();
            for fact in facts {
                activities.push(activity_for(&state, fact));
            }
        }
        self.remote_state
            .publish(facts.iter().map(|fact| &fact.path).zip(&activities), &live);

        let mut payload = RemoteActivityPayload {
            repo_paths: Vec::with_capacity(facts.len()),
            ci_states: Vec::with_capacity(facts.len()),
            staleness_states: Vec::with_capacity(facts.len()),
            behind_by: Vec::with_capacity(facts.len()),
        };
        for (fact, activity) in facts.iter().zip(activities) {
            payload.repo_paths.push(fact.path.clone());
            payload.ci_states.push(activity.ci);
            payload.staleness_states.push(activity.staleness);
            payload.behind_by.push(activity.behind_by);
        }

        let changed = self.lock_state().last_payload.as_ref() != Some(&payload);
        if changed {
            {
                let mut emit = self.emit.lock().unwrap_or_else(|e| e.into_inner());
                (emit)(&payload);
            }
            self.lock_state().last_payload = Some(payload);
        }

        // #2374 - publish the neutral whole-file snapshot. The entries are
        // cloned from the same post-GC snapshot the payload was built from, and
        // this runs AFTER transitions were processed and the payload was emitted,
        // so persistence can never delay or suppress either. The publication
        // instant is read HERE, not taken from the round-start `wall`: a round
        // longer than the freshness window must not publish bytes that are
        // already stale. The blocking serialization/write is offloaded, so a
        // slow disk cannot stall the sweeper's async worker.
        if self.snapshot_persistence == SnapshotPersistence::Enabled {
            if let Some(snapshot_dir) = &self.snapshot_dir {
                let snapshot_dir = snapshot_dir.clone();
                let entries: Vec<(String, PersistedRepoCi)> =
                    self.remote_state.read_snapshot(|snapshot| {
                        snapshot
                            .iter()
                            .map(|(path, activity)| (path.clone(), persisted_repo_ci(activity)))
                            .collect()
                    });
                let generated_at = (self.publication_clock)();
                let write = tokio::task::spawn_blocking(move || {
                    crate::config::remote_activity_cache::write_snapshot_to_dir(
                        &snapshot_dir,
                        generated_at,
                        &entries,
                    )
                })
                .await;
                match write {
                    Ok(Ok(())) => self.lock_state().snapshot_warnings.reset(),
                    Ok(Err(error)) => self.warn_snapshot_failure(now, error),
                    Err(error) => self
                        .warn_snapshot_failure(now, format!("snapshot write task failed: {error}")),
                }
            }
        }
    }

    pub(super) fn refund_round_reservations(
        &self,
        outcomes: &[(QueryKey, KeyOutcome)],
        identity_reserved: &HashSet<QueryKey>,
        completion_reserved: &HashSet<QueryKey>,
    ) {
        // A reservation returns a call that was never made; it never forgives
        // one that was.
        if !identity_reserved.is_empty() {
            let refund = outcomes
                .iter()
                .filter(|(key, outcome)| {
                    identity_reserved.contains(key) && !outcome.ci_identity_call
                })
                .count() as f64;
            if refund > 0.0 {
                let mut state = self.lock_state();
                state.budget_tokens = (state.budget_tokens + refund).min(GH_BUDGET_BURST);
            }
        }

        let refund: f64 = outcomes
            .iter()
            .filter(|(key, _)| completion_reserved.contains(key))
            .map(|(_, outcome)| f64::from(3 - outcome.ci_corroboration_calls))
            .sum();
        if refund > 0.0 {
            let mut state = self.lock_state();
            state.budget_tokens = (state.budget_tokens + refund).min(GH_BUDGET_BURST);
        }
    }
}
