use super::super::{
    acquire_carrier_mutation_permit, CommitCapability, CommitDecision, LifecycleMutations,
    OwnedAction, OwnedActionFacts, OwnedActionRole, OwnedActionWitness, OwnedMutationCell,
    OwnedMutationFailure, OwnedMutationHeader, OwnedMutationObservation, OwnedMutationReceipt,
    OwnedMutationReservation, OwnedRowIdentity, PendingCreateBinding, SelectionCause,
    SessionManager,
};
use super::{live_witness, pending_fixture};
use crate::config::sessions_persistence::{
    run_observed_owned_core, OperationProbe, OwnedTaskSet, ProbeEvent,
};
use crate::session::session::Session;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

struct Fixture {
    manager: SessionManager,
    slot: Arc<RwLock<SessionManager>>,
    a: Session,
    b: Session,
    binding: PendingCreateBinding,
    header: OwnedMutationHeader,
    original_a: OwnedRowIdentity,
}

impl Fixture {
    async fn new() -> Self {
        let manager = SessionManager::new();
        let (a, a_binding) = pending_fixture(&manager, false).await;
        let mut mutations = LifecycleMutations::default();
        mutations.finalize_live(a_binding, live_witness(a.id));
        manager
            .commit_selection_transition(
                &CommitCapability::for_test(),
                CommitDecision::Live(live_witness(a.id)),
                SelectionCause::Restore,
                mutations,
            )
            .await
            .unwrap();
        let (b, binding) = pending_fixture(&manager, false).await;
        let original_a = OwnedRowIdentity {
            id: a.id,
            token: a.token,
            version: manager.state.read().await.carrier_versions[&a.id],
        };
        let header = OwnedMutationHeader {
            operation: Uuid::new_v4(),
            executor_nonce: Uuid::new_v4(),
            preparing_nonce: Uuid::new_v4(),
            binding,
            role: OwnedActionRole::FinalizeB,
        };
        Self {
            slot: Arc::new(RwLock::new(manager.clone())),
            manager,
            a,
            b,
            binding,
            header,
            original_a,
        }
    }

    fn cell(&self, role: OwnedActionRole) -> Arc<OwnedMutationCell> {
        OwnedMutationCell::reserve(
            OwnedMutationReservation {
                header: OwnedMutationHeader {
                    role,
                    ..self.header
                },
                original_a: self.original_a,
            },
            &self.slot,
        )
    }

    fn witness(&self, cell: &Arc<OwnedMutationCell>, action: OwnedAction) -> OwnedActionWitness {
        let role = match &action {
            OwnedAction::FinalizeB { .. } => OwnedActionRole::FinalizeB,
            OwnedAction::RollbackPendingB { .. } => OwnedActionRole::RollbackPendingB,
            OwnedAction::RemoveA { .. } => OwnedActionRole::RemoveA,
        };
        OwnedActionWitness::from_trusted_owner(
            &CommitCapability::for_test(),
            cell,
            OwnedActionFacts {
                header: OwnedMutationHeader {
                    role,
                    ..self.header
                },
                action,
            },
        )
        .unwrap()
    }

    fn launch_input(&self) -> OwnedAction {
        // Real insertion token; generation is trusted upper INPUT, no native proof.
        OwnedAction::FinalizeB {
            token: self.b.token,
            generation: 11,
        }
    }

    async fn finalize(&self, cell: &Arc<OwnedMutationCell>) -> OwnedMutationObservation {
        self.finalize_with_decision(cell, CommitDecision::Keep)
            .await
    }
    async fn finalize_with_decision(
        &self,
        cell: &Arc<OwnedMutationCell>,
        decision: CommitDecision,
    ) -> OwnedMutationObservation {
        let claim = self
            .manager
            .claim_owned_mutation(cell, self.witness(cell, self.launch_input()))
            .unwrap();
        let mut mutations = LifecycleMutations::default();
        mutations.finalize_live(self.binding, live_witness(self.b.id));
        self.manager
            .commit_owned_transition(
                acquire_carrier_mutation_permit().await,
                &CommitCapability::for_test(),
                decision,
                SelectionCause::Restore,
                mutations,
                claim,
            )
            .await
    }

    async fn rollback(&self, cell: &Arc<OwnedMutationCell>) -> OwnedMutationObservation {
        let witness = self.witness(cell, OwnedAction::RollbackPendingB { launch: None });
        let claim = self.manager.claim_owned_mutation(cell, witness).unwrap();
        self.manager
            .rollback_owned_pending(acquire_carrier_mutation_permit().await, self.binding, claim)
            .await
    }
}

fn applied(observation: OwnedMutationObservation) -> Arc<OwnedMutationReceipt> {
    match observation {
        OwnedMutationObservation::Applied(receipt) => receipt,
        other => panic!("expected real Applied, got {other:?}"),
    }
}

#[tokio::test]
async fn issue_3263_manager_unbound_reservation_and_late_original_launch() {
    let f = Fixture::new().await;
    let cell = f.cell(OwnedActionRole::FinalizeB);
    assert!(matches!(
        f.manager.observe_owned_mutation(&cell),
        OwnedMutationObservation::NotEntered
    ));
    let receipt = applied(f.finalize(&cell).await);
    assert_eq!(
        receipt.initial_b,
        Some((
            f.b.token,
            11,
            f.manager.state.read().await.carrier_versions[&f.b.id]
        ))
    );
    assert!(receipt.pending_before);
    assert!(!receipt.pending_after);
    assert!(receipt.tracker_pending); // Not a completed settlement/tracker receipt.
    assert_eq!(receipt.result.as_ref().unwrap().finalized_rows.len(), 1);
    assert!(receipt.removed_row.is_none());
    assert_eq!(receipt.before.len(), 2);
}

#[tokio::test]
async fn issue_3263_manager_pending_binding_only_removes_once_and_reobserves_tombstone() {
    let f = Fixture::new().await;
    let cell = f.cell(OwnedActionRole::RollbackPendingB);
    // Fixture starts no native process. Manager removal is not upper NotLaunched/join proof.
    let receipt = applied(f.rollback(&cell).await);
    assert!(receipt.initial_b.is_none());
    assert_eq!(receipt.removed_row.as_ref().unwrap().token, f.b.token);
    assert!(f.manager.get_pending_session(f.binding).await.is_none());
    let version = f.manager.state.read().await.carrier_versions[&f.b.id];
    let repeated = applied(f.rollback(&cell).await);
    assert!(Arc::ptr_eq(&receipt, &repeated));
    assert_eq!(
        f.manager.state.read().await.carrier_versions[&f.b.id],
        version
    );
}

#[tokio::test]
async fn issue_3263_manager_absent_rollback_without_receipt_is_not_applied() {
    let f = Fixture::new().await;
    let cell = f.cell(OwnedActionRole::RollbackPendingB);
    f.manager.rollback_pending_create(f.binding).await.unwrap();
    assert!(matches!(
        f.rollback(&cell).await,
        OwnedMutationObservation::NotApplied(OwnedMutationFailure::AbsentWithoutReceipt)
    ));
    assert!(matches!(
        f.manager.observe_owned_mutation(&cell),
        OwnedMutationObservation::NotApplied(_)
    ));
}

#[tokio::test]
async fn issue_3263_manager_pre_u_claim_wins_once_and_cancellation_consumes_step() {
    let f = Fixture::new().await;
    let cell = f.cell(OwnedActionRole::FinalizeB);
    let claim = f
        .manager
        .claim_owned_mutation(&cell, f.witness(&cell, f.launch_input()))
        .unwrap();
    assert!(matches!(
        f.manager
            .claim_owned_mutation(&cell, f.witness(&cell, f.launch_input())),
        Err(OwnedMutationObservation::InProgress)
    ));
    drop(claim);
    assert!(matches!(
        f.manager
            .claim_owned_mutation(&cell, f.witness(&cell, f.launch_input())),
        Err(OwnedMutationObservation::NotApplied(_))
    ));
    assert!(f.manager.get_pending_session(f.binding).await.is_some());
}

#[tokio::test]
async fn issue_3263_manager_original_token_rejects_real_pending_row_change_and_rebinding() {
    let f = Fixture::new().await;
    let cell = f.cell(OwnedActionRole::FinalizeB);
    let witness = f.witness(&cell, f.launch_input());
    let claim = f.manager.claim_owned_mutation(&cell, witness).unwrap();
    let changed_token = Uuid::new_v4();
    let permit = acquire_carrier_mutation_permit().await;
    f.manager
        .update_pending_with_carrier_permit(&permit, f.binding, |row| row.token = changed_token)
        .await
        .unwrap();
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(f.binding, live_witness(f.b.id));
    assert!(matches!(
        f.manager
            .commit_owned_transition(
                permit,
                &CommitCapability::for_test(),
                CommitDecision::Keep,
                SelectionCause::Restore,
                mutations,
                claim
            )
            .await,
        OwnedMutationObservation::NotApplied(_)
    ));
    let changed = OwnedAction::FinalizeB {
        token: changed_token,
        generation: 11,
    };
    assert!(matches!(
        f.manager
            .claim_owned_mutation(&cell, f.witness(&cell, changed)),
        Err(OwnedMutationObservation::ExitedUnknown(
            OwnedMutationFailure::Conflict
        ))
    ));
    assert_eq!(
        f.manager
            .get_pending_session(f.binding)
            .await
            .unwrap()
            .token,
        changed_token
    );
}

#[tokio::test]
async fn issue_3263_manager_wrong_header_binding_role_cell_and_manager_do_not_enter() {
    let f = Fixture::new().await;
    let cell = f.cell(OwnedActionRole::FinalizeB);
    let foreign = Fixture::new().await;
    for field in 0..5 {
        let mut header = f.header;
        match field {
            0 => header.operation = Uuid::new_v4(),
            1 => header.executor_nonce = Uuid::new_v4(),
            2 => header.preparing_nonce = Uuid::new_v4(),
            3 => header.binding = foreign.binding,
            _ => header.role = OwnedActionRole::RollbackPendingB,
        }
        assert!(OwnedActionWitness::from_trusted_owner(
            &CommitCapability::for_test(),
            &cell,
            OwnedActionFacts {
                header,
                action: f.launch_input()
            }
        )
        .is_err());
    }
    let other_step = f.cell(OwnedActionRole::FinalizeB);
    assert!(matches!(
        f.manager
            .claim_owned_mutation(&other_step, f.witness(&cell, f.launch_input())),
        Err(OwnedMutationObservation::ExitedUnknown(_))
    ));
    assert!(matches!(
        foreign
            .manager
            .claim_owned_mutation(&cell, f.witness(&cell, f.launch_input())),
        Err(OwnedMutationObservation::ExitedUnknown(_))
    ));
    assert!(matches!(
        f.manager.observe_owned_mutation(&cell),
        OwnedMutationObservation::NotEntered
    ));
    let slot_guard = f.slot.write().await;
    assert!(matches!(
        f.manager
            .claim_owned_mutation(&cell, f.witness(&cell, f.launch_input())),
        Err(OwnedMutationObservation::NotEntered)
    ));
    drop(slot_guard);
    *f.slot.write().await = foreign.manager.clone();
    assert!(matches!(
        f.manager
            .claim_owned_mutation(&cell, f.witness(&cell, f.launch_input())),
        Err(OwnedMutationObservation::ExitedUnknown(_))
    ));
}

#[tokio::test]
async fn issue_3263_manager_changed_generation_or_variant_cannot_rebind() {
    let f = Fixture::new().await;
    let cell = f.cell(OwnedActionRole::FinalizeB);
    let claim = f
        .manager
        .claim_owned_mutation(&cell, f.witness(&cell, f.launch_input()))
        .unwrap();
    let changed = OwnedAction::FinalizeB {
        token: f.b.token,
        generation: 12,
    };
    assert!(matches!(
        f.manager
            .claim_owned_mutation(&cell, f.witness(&cell, changed)),
        Err(OwnedMutationObservation::ExitedUnknown(_))
    ));
    assert!(OwnedActionWitness::from_trusted_owner(
        &CommitCapability::for_test(),
        &cell,
        OwnedActionFacts {
            header: f.header,
            action: OwnedAction::RollbackPendingB { launch: None }
        }
    )
    .is_err());
    drop(claim);
}

#[tokio::test]
async fn issue_3263_manager_pre_mutation_error_releases_m_before_not_applied() {
    let f = Fixture::new().await;
    let cell = f.cell(OwnedActionRole::FinalizeB);
    let claim = f
        .manager
        .claim_owned_mutation(&cell, f.witness(&cell, f.launch_input()))
        .unwrap();
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(f.binding, live_witness(f.b.id));
    f.manager.state.write().await.revision = u64::MAX;
    let before = f.manager.state.read().await.carrier_versions[&f.b.id];
    let observation = f
        .manager
        .commit_owned_transition(
            acquire_carrier_mutation_permit().await,
            &CommitCapability::for_test(),
            CommitDecision::Live(live_witness(f.b.id)),
            SelectionCause::Restore,
            mutations,
            claim,
        )
        .await;
    assert!(matches!(
        observation,
        OwnedMutationObservation::NotApplied(_)
    ));
    assert_eq!(
        f.manager.state.try_read().unwrap().carrier_versions[&f.b.id],
        before
    );
    assert!(f.manager.get_pending_session(f.binding).await.is_some());
    assert!(acquire_carrier_mutation_permit()
        .await
        .owned_cell()
        .is_none());
}

#[tokio::test]
async fn issue_3263_manager_stale_binding_and_tracker_failure_reject_before_mutation() {
    for tracker_failure in [false, true] {
        let f = Fixture::new().await;
        let cell = f.cell(OwnedActionRole::RollbackPendingB);
        if tracker_failure {
            f.manager.response_close_state().fail_closed();
        }
        let witness = f.witness(&cell, OwnedAction::RollbackPendingB { launch: None });
        let claim = f.manager.claim_owned_mutation(&cell, witness).unwrap();
        let binding = if tracker_failure {
            f.binding
        } else {
            f.manager.reserve_carrier_create_binding()
        };
        assert!(matches!(
            f.manager
                .rollback_owned_pending(acquire_carrier_mutation_permit().await, binding, claim)
                .await,
            OwnedMutationObservation::NotApplied(_)
        ));
        assert!(f.manager.get_pending_session(f.binding).await.is_some());
    }
}

#[tokio::test]
async fn issue_3263_manager_remove_a_requires_original_committed_b_receipt() {
    for select_b_first in [false, true] {
        let f = Fixture::new().await;
        let finalize = f.cell(OwnedActionRole::FinalizeB);
        let decision = if select_b_first {
            CommitDecision::Live(live_witness(f.b.id))
        } else {
            CommitDecision::Keep
        };
        let committed_b = applied(f.finalize_with_decision(&finalize, decision).await);
        assert_eq!(
            f.manager.state.read().await.carrier_versions[&f.a.id],
            f.original_a.version + u64::from(select_b_first)
        );
        let foreign = Fixture::new().await;
        let foreign_cell = foreign.cell(OwnedActionRole::FinalizeB);
        let foreign_receipt = applied(foreign.finalize(&foreign_cell).await);
        let cell = f.cell(OwnedActionRole::RemoveA);
        let header = OwnedMutationHeader {
            role: OwnedActionRole::RemoveA,
            ..f.header
        };
        assert!(OwnedActionWitness::from_trusted_owner(
            &CommitCapability::for_test(),
            &cell,
            OwnedActionFacts {
                header,
                action: OwnedAction::RemoveA {
                    committed_b: foreign_receipt
                }
            }
        )
        .is_err());
        let witness = f.witness(
            &cell,
            OwnedAction::RemoveA {
                committed_b: committed_b.clone(),
            },
        );
        let claim = f.manager.claim_owned_mutation(&cell, witness).unwrap();
        let mut mutations = LifecycleMutations::default();
        mutations.remove(f.a.id);
        let receipt = applied(
            f.manager
                .commit_owned_transition(
                    acquire_carrier_mutation_permit().await,
                    &CommitCapability::for_test(),
                    CommitDecision::Live(live_witness(f.b.id)),
                    SelectionCause::Restore,
                    mutations,
                    claim,
                )
                .await,
        );
        assert_eq!(
            receipt.result.as_ref().unwrap().removed_rows[0].id,
            f.a.id.to_string()
        );
        assert!(f.manager.get_session(f.a.id).await.is_none());
        assert!(f.manager.get_session(f.b.id).await.is_some());
        let witness = f.witness(&cell, OwnedAction::RemoveA { committed_b });
        let claim = f.manager.claim_owned_mutation(&cell, witness).unwrap();
        let mut repeated = LifecycleMutations::default();
        repeated.remove(f.a.id);
        let again = applied(
            f.manager
                .commit_owned_transition(
                    acquire_carrier_mutation_permit().await,
                    &CommitCapability::for_test(),
                    CommitDecision::Live(live_witness(f.b.id)),
                    SelectionCause::Restore,
                    repeated,
                    claim,
                )
                .await,
        );
        assert!(Arc::ptr_eq(&receipt, &again));
    }
}

#[tokio::test]
async fn issue_3263_manager_probe_reaches_guard_finalization() {
    let f = Fixture::new().await;
    let probe = Arc::new(OperationProbe::default());
    let tasks = OwnedTaskSet::new_with_probe(f.header.operation, probe.clone());
    let gate = probe.gate("owned_source", ProbeEvent::ManagerReceiptFinalized);
    let cell = f.cell(OwnedActionRole::FinalizeB);
    let original = cell.clone();
    let manager = f.manager.clone();
    let before = manager.state.read().await.carrier_versions[&f.b.id];
    let id = f.b.id;
    let operation = f.header.operation;
    let observer = run_observed_owned_core(tasks.clone(), async move { f.finalize(&cell).await });
    let context = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        probe.observed("owned_source", ProbeEvent::ManagerReceiptFinalized),
    )
    .await
    .unwrap();
    assert_eq!(context.operation, operation);
    assert!(context.task.is_some());
    // task/attempt are not step_nonce: this original cell supplies step-specific proof.
    let receipt = applied(manager.observe_owned_mutation(&original));
    assert_eq!(receipt.initial_b.unwrap().2, before + 1);
    assert_eq!(
        manager.state.try_read().unwrap().carrier_versions[&id],
        before + 1
    );
    let permit = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        acquire_carrier_mutation_permit(),
    )
    .await
    .unwrap();
    drop(permit); // U and M were released before the captured producer probe paused.
    drop(observer); // Public observer loss does not cancel the retained original source.
    gate.release();
    tasks.join_producers().await.unwrap();
    assert!(Arc::ptr_eq(
        &receipt,
        &applied(manager.observe_owned_mutation(&original))
    ));
}

#[tokio::test]
async fn issue_3263_manager_copy_and_finish_panics_are_unknown_no_replay_versions_once() {
    for point in [1, 2] {
        let f = Fixture::new().await;
        let canonical_slot = f.slot.clone();
        let cell = f.cell(OwnedActionRole::FinalizeB);
        cell.panic_point
            .store(point, std::sync::atomic::Ordering::SeqCst);
        let original = cell.clone();
        let manager = f.manager.clone();
        let b = f.b.id;
        let header = f.header;
        let token = f.b.token;
        let before = manager.state.read().await.carrier_versions[&b];
        let probe = Arc::new(OperationProbe::default());
        let tasks = OwnedTaskSet::new_with_probe(f.header.operation, probe.clone());
        let observer =
            run_observed_owned_core(tasks.clone(), async move { f.finalize(&cell).await });
        assert!(observer.result().await.is_err());
        assert!(tasks.join_producers().await.is_err()); // Actual failed-source join observed.
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            probe.observed("owned_source", ProbeEvent::ProducerJoined),
        )
        .await
        .unwrap();
        assert!(matches!(
            manager.observe_owned_mutation(&original),
            OwnedMutationObservation::ExitedUnknown(_)
        ));
        assert_eq!(manager.state.read().await.carrier_versions[&b], before + 1);
        assert!(matches!(
            manager.observe_owned_mutation(&original),
            OwnedMutationObservation::ExitedUnknown(_)
        ));
        assert!(Arc::ptr_eq(
            &canonical_slot.read().await.state,
            &manager.state
        ));
        let witness = OwnedActionWitness::from_trusted_owner(
            &CommitCapability::for_test(),
            &original,
            OwnedActionFacts {
                header,
                action: OwnedAction::FinalizeB {
                    token,
                    generation: 11,
                },
            },
        )
        .unwrap();
        assert!(matches!(
            manager.claim_owned_mutation(&original, witness),
            Err(OwnedMutationObservation::ExitedUnknown(
                OwnedMutationFailure::PartialMutation
            ))
        ));
        assert!(original.partial_receipt().is_some());
    }
}

#[tokio::test]
async fn issue_3263_manager_terminal_receipt_does_not_certify_later_row() {
    let f = Fixture::new().await;
    let cell = f.cell(OwnedActionRole::FinalizeB);
    let receipt = applied(f.finalize(&cell).await);

    for request in 0..3 {
        let claim = f
            .manager
            .claim_owned_mutation(&cell, f.witness(&cell, f.launch_input()))
            .unwrap();
        let rejected = if request == 0 {
            f.manager
                .rollback_owned_pending(acquire_carrier_mutation_permit().await, f.binding, claim)
                .await
        } else {
            let mut wrong = LifecycleMutations::default();
            if request == 2 {
                let mut witness = live_witness(f.b.id);
                witness.has_pty = false;
                wrong.finalize_live(f.binding, witness);
            }
            f.manager
                .commit_owned_transition(
                    acquire_carrier_mutation_permit().await,
                    &CommitCapability::for_test(),
                    CommitDecision::Keep,
                    SelectionCause::Restore,
                    wrong,
                    claim,
                )
                .await
        };
        assert!(matches!(
            rejected,
            OwnedMutationObservation::ExitedUnknown(OwnedMutationFailure::Conflict)
        ));
        assert!(Arc::ptr_eq(
            &receipt,
            &applied(f.manager.observe_owned_mutation(&cell))
        ));
        assert_eq!(
            f.manager.state.read().await.carrier_versions[&f.b.id],
            receipt.initial_b.unwrap().2
        );
    }
    assert!(Arc::ptr_eq(&receipt, &applied(f.finalize(&cell).await)));
    let tracker = f.manager.response_close_state();
    assert!(tracker.is_healthy());
    tracker.register_route(f.b.id, 12, std::time::Instant::now());
    assert!(tracker.is_healthy());
    assert_eq!(tracker.generation(f.b.id), Some(12));
    assert!(matches!(
        f.finalize(&cell).await,
        OwnedMutationObservation::ExitedUnknown(_)
    ));
    tracker.remove_route(f.b.id, 12);
    assert!(tracker.is_healthy());
    assert!(tracker.generation(f.b.id).is_none());
    let rollback_fixture = Fixture::new().await;
    let rollback_cell = rollback_fixture.cell(OwnedActionRole::RollbackPendingB);
    let rollback_receipt = applied(rollback_fixture.rollback(&rollback_cell).await);
    let witness = rollback_fixture.witness(
        &rollback_cell,
        OwnedAction::RollbackPendingB { launch: None },
    );
    let claim = rollback_fixture
        .manager
        .claim_owned_mutation(&rollback_cell, witness)
        .unwrap();
    let rejected = rollback_fixture
        .manager
        .rollback_owned_pending(acquire_carrier_mutation_permit().await, f.binding, claim)
        .await;
    assert!(matches!(
        rejected,
        OwnedMutationObservation::ExitedUnknown(OwnedMutationFailure::Conflict)
    ));
    assert!(Arc::ptr_eq(
        &rollback_receipt,
        &applied(
            rollback_fixture
                .manager
                .observe_owned_mutation(&rollback_cell)
        )
    ));
    let mut exit = LifecycleMutations::default();
    exit.mark_exited(f.b.id, 0);
    // Real guarded lifecycle write publishes the changed carrier version.
    f.manager
        .commit_selection_transition(
            &CommitCapability::for_test(),
            CommitDecision::Keep,
            SelectionCause::LivenessReconcile,
            exit,
        )
        .await
        .unwrap();
    assert!(f.manager.state.read().await.carrier_versions[&f.b.id] > receipt.initial_b.unwrap().2);
    let reentry = f.finalize(&cell).await;
    assert!(
        matches!(reentry, OwnedMutationObservation::ExitedUnknown(_)),
        "{reentry:?}"
    );
    assert!(Arc::ptr_eq(
        &receipt,
        &applied(f.manager.observe_owned_mutation(&cell))
    ));
}

#[tokio::test]
async fn issue_3263_manager_overflow_and_tracker_generation_cannot_publish_applied() {
    for overflow in [false, true] {
        let f = Fixture::new().await;
        let cell = f.cell(OwnedActionRole::FinalizeB);
        if overflow {
            f.manager
                .state
                .write()
                .await
                .carrier_versions
                .insert(f.b.id, u64::MAX);
        } else {
            f.manager
                .response_close_state()
                .register_route(f.b.id, 12, std::time::Instant::now());
        }
        let observation = f.finalize(&cell).await;
        assert!(!matches!(observation, OwnedMutationObservation::Applied(_)));
        if overflow {
            assert_eq!(
                f.manager.state.read().await.carrier_versions[&f.b.id],
                u64::MAX
            );
        } else {
            assert!(f.manager.get_pending_session(f.binding).await.is_some());
        }
    }
}

#[tokio::test]
async fn issue_3263_manager_same_reserved_rollback_after_actual_source_failure_before_claim() {
    let f = Fixture::new().await;
    let cell = f.cell(OwnedActionRole::RollbackPendingB);
    let tasks = OwnedTaskSet::new(f.header.operation);
    let observer = run_observed_owned_core(tasks.clone(), async { panic!("before manager claim") });
    assert!(observer.result().await.is_err());
    assert!(tasks.join_producers().await.is_err());
    assert!(matches!(
        f.manager.observe_owned_mutation(&cell),
        OwnedMutationObservation::NotEntered
    ));
    assert!(f.manager.get_pending_session(f.binding).await.is_some());
    applied(f.rollback(&cell).await); // Same cell; only manager proof, no upper settlement claim.
}
