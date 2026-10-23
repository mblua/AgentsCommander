use crate::resource_monitor::registry::is_missing_root_cleanup_error;
use crate::resource_monitor::registry::join_errors;
use crate::resource_monitor::registry::merge_observed;
use crate::resource_monitor::registry::ResourceError;
use crate::resource_monitor::registry::ResourceMonitorState;
use crate::resource_monitor::registry::SampleOutcome;
use crate::resource_monitor::registry::MAX_TERMINATED_RETAINED;
use crate::resource_monitor::registry::REAP_STRIKES;
use crate::resource_monitor::types::ObservedProcessTree;
use crate::resource_monitor::types::ProcessIdentity;
use crate::resource_monitor::types::ResourceAgentGroupSnapshot;
use crate::resource_monitor::types::ResourceGroupState;
use crate::resource_monitor::types::ResourceLimits;
use std::collections::BTreeSet;
use std::time::Instant;
use uuid::Uuid;

impl ResourceMonitorState {
    pub fn sample_for_watchdog(
        &self,
        limits: ResourceLimits,
    ) -> Vec<(Uuid, ResourceAgentGroupSnapshot)> {
        let snapshot = self.snapshot(limits);
        snapshot
            .groups
            .into_iter()
            .filter_map(|group| {
                Uuid::parse_str(&group.session_id)
                    .ok()
                    .map(|id| (id, group))
            })
            .collect()
    }

    pub(super) fn sample_running_groups(
        &self,
    ) -> Vec<(Uuid, Result<ObservedProcessTree, ResourceError>)> {
        let roots = self
            .inner
            .lock()
            .map(|inner| {
                inner
                    .groups
                    .iter()
                    .filter_map(|(id, group)| {
                        // #647 C: include Quarantined so its process list is
                        // re-observed each snapshot (unfreezes the display for the
                        // root-alive / EDR-residual case). Reap-safe: merge_sample
                        // only strike-counts/nominates Running groups.
                        matches!(
                            group.state,
                            ResourceGroupState::Running
                                | ResourceGroupState::Terminating
                                | ResourceGroupState::Quarantined
                        )
                        .then_some((*id, group.root_identity))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        roots
            .into_iter()
            .map(|(id, root)| (id, self.backend.observe_tree(root)))
            .collect()
    }

    pub(super) fn merge_tree(&self, session_id: Uuid, tree: ObservedProcessTree) -> Option<String> {
        let mut inner = self.inner.lock().ok()?;
        let group = inner.groups.get_mut(&session_id)?;
        group.descendants_observed |= tree.processes.len() > 1;
        merge_observed(&mut group.observed_processes, tree.processes);
        let joined = join_errors(tree.errors);
        if joined.is_some() {
            group.last_error = joined.clone();
        }
        joined
    }

    /// #559 (H1) - snapshot-path merge. Advances the missing-root strike counter and,
    /// when it reaches REAP_STRIKES, NOMINATES the group for reap (returns its root
    /// identity) without reaping: the reap is gated by a direct observe_identity
    /// confirm done outside the lock in snapshot(). Preserves merge_tree's warning
    /// contract, except on the nominating sample, where it returns no warning so the
    /// raw missing-root string does not also flip overall_state to Warn (G4).
    pub(super) fn merge_sample(
        &self,
        session_id: Uuid,
        tree: ObservedProcessTree,
    ) -> SampleOutcome {
        let none = SampleOutcome {
            warning: None,
            reap_candidate: None,
        };
        let Ok(mut inner) = self.inner.lock() else {
            return none;
        };
        let Some(group) = inner.groups.get_mut(&session_id) else {
            return none;
        };

        // #559 (H1) - read the signal from the structured errors BEFORE moving the tree.
        let root_missing = tree
            .errors
            .iter()
            .any(|e| is_missing_root_cleanup_error(e, group.root_identity));
        let root_alive = tree
            .processes
            .iter()
            .any(|p| p.identity == group.root_identity);

        // #632 B2b - capture the live identity set and whether the sample is CLEAN
        // before merge_observed consumes the tree. "Clean" = no enumeration errors at
        // all (no missing root, no per-pid "identity unavailable" placeholder), so a
        // partial/racy toolhelp snapshot can never drop a live descendant.
        let live_identities: BTreeSet<ProcessIdentity> =
            tree.processes.iter().map(|p| p.identity).collect();
        let sample_clean = tree.errors.is_empty();

        group.descendants_observed |= tree.processes.len() > 1;
        merge_observed(&mut group.observed_processes, tree.processes);

        // #632 B2b - on a CLEAN sample with the root observed alive, the fresh tree is
        // the authoritative live set, so drop entries no longer present. Gated on
        // `root_alive && sample_clean` (MED-3): a missing-root, partial, or
        // ACCESS_DENIED-placeholder sample keeps the last-known set - that case is
        // owned by the #559 strike/reap logic below, and keeping it avoids dropping a
        // live child a racy snapshot momentarily missed (which would weaken the no-job
        // reaper fallback). A clean tree always contains the root, so the root is
        // retained.
        //
        // KNOWN RESIDUAL (#637): a still-alive grandchild whose intermediate parent
        // exited is unreachable from the root in this BFS (Windows does not reparent)
        // yet the sample still looks clean, so it is pruned here. Harmless for jobbed
        // sessions (the Job Object kills the whole tree regardless of reachability);
        // for a job-less session it extends the MED-2 reaper-only orphan residual.
        if root_alive && sample_clean {
            group
                .observed_processes
                .retain(|identity, _| live_identities.contains(identity));
        }
        let joined = join_errors(tree.errors);
        if joined.is_some() {
            group.last_error = joined.clone();
        }

        if matches!(group.state, ResourceGroupState::Running) && root_missing && !root_alive {
            group.missing_root_strikes = group.missing_root_strikes.saturating_add(1);
            if group.missing_root_strikes >= REAP_STRIKES {
                // Nominate; suppress the raw warning on this sample (G4).
                return SampleOutcome {
                    warning: None,
                    reap_candidate: Some(group.root_identity),
                };
            }
        } else {
            group.missing_root_strikes = 0;
        }

        SampleOutcome {
            warning: joined,
            reap_candidate: None,
        }
    }

    /// #559 (H1) - perform the confirmed reap under the lock. Re-checks the group is
    /// still Running, still at the strike threshold, and still the same root, so a
    /// concurrent kill_group that already terminated it wins and this becomes a no-op.
    pub(super) fn apply_reap(&self, session_id: Uuid, root: ProcessIdentity) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let Some(group) = inner.groups.get_mut(&session_id) else {
            return;
        };
        if matches!(group.state, ResourceGroupState::Running)
            && group.missing_root_strikes >= REAP_STRIKES
            && group.root_identity == root
        {
            group.state = ResourceGroupState::Terminated;
            group.permit_released = true;
            group.terminated_at = Some(Instant::now());
            group.missing_root_strikes = 0;
            group.last_error = Some("agent process exited; resource slot reclaimed".to_string());
            log::info!(
                "[resource-monitor] reaped exited group session={} root_pid={}",
                session_id,
                root.pid
            );
        }
    }

    /// #559 (H1) - the confirm said the root is alive (or was unverifiable); clear the
    /// strikes so a future genuine exit starts counting fresh.
    pub(super) fn reset_missing_root_strikes(&self, session_id: Uuid, root: ProcessIdentity) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(group) = inner.groups.get_mut(&session_id) {
                if matches!(group.state, ResourceGroupState::Running) && group.root_identity == root
                {
                    group.missing_root_strikes = 0;
                }
            }
        }
    }

    /// #559 (H3) - bound retained Terminated rows so inner.groups cannot grow without
    /// limit. Keeps the MAX_TERMINATED_RETAINED most recent; evicts the oldest.
    /// Released-permit Terminated groups never count toward active_count, so eviction
    /// never affects the cap.
    pub(super) fn prune_terminated_groups(&self) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let mut terminated: Vec<(Uuid, Instant)> = inner
            .groups
            .iter()
            .filter(|(_, g)| matches!(g.state, ResourceGroupState::Terminated) && g.permit_released)
            .map(|(id, g)| (*id, g.terminated_at.unwrap_or_else(Instant::now)))
            .collect();
        if terminated.len() <= MAX_TERMINATED_RETAINED {
            return;
        }
        terminated.sort_by_key(|(_, at)| *at); // oldest first
        let remove = terminated.len() - MAX_TERMINATED_RETAINED;
        for (id, _) in terminated.into_iter().take(remove) {
            inner.groups.remove(&id);
        }
    }

    pub(super) fn set_group_error(&self, session_id: Uuid, error: String) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(group) = inner.groups.get_mut(&session_id) {
                group.last_error = Some(error);
            }
        }
    }
}
