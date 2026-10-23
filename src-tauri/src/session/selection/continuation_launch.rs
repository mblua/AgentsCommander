use std::sync::{Arc, Mutex};

use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::session::manager::{CommitResult, PendingCreateBinding, SessionManager};
use crate::session::session::{SessionInfo, SessionStatus};

use super::envelope::FinalizedOwnedCarryOutcome;
use super::replacement::{
    LifecycleEffectSlot, OwnedEffectState, OwnedLaunchReceipt, OwnedReplacementContinuation,
    OwnedReplacementDescriptor, OwnedSourceStop, OwnedSourceValidation,
    ReplacementCleanupAuthority, ReplacementStage,
};

#[cfg(test)]
use super::replacement_fixture::{
    FixtureContextNotification, NativeAwaitPanicOnce, ReplacementFixtureAction,
    ReplacementFixtureControl, ReplacementFixtureCut, ReplacementFixtureNativeInvocation,
};
#[cfg(test)]
use std::sync::atomic::Ordering;
#[cfg(test)]
use tokio::sync::oneshot;

impl OwnedReplacementContinuation {
    #[cfg(test)]
    pub(crate) fn fixture_take_native_await_panic(
        &self,
        seed: bool,
    ) -> Option<oneshot::Receiver<()>> {
        let control = self.fixture.as_ref()?;
        control.validate_identity(self);
        let mut selected = control
            .native_await_panic
            .lock()
            .expect("native await panic");
        if matches!(
            (&*selected, seed),
            (Some(NativeAwaitPanicOnce::Context(_)), false)
                | (Some(NativeAwaitPanicOnce::Seed(_)), true)
        ) {
            selected.take().map(|selected| match selected {
                NativeAwaitPanicOnce::Context(receiver) | NativeAwaitPanicOnce::Seed(receiver) => {
                    receiver
                }
            })
        } else {
            None
        }
    }

    #[cfg(test)]
    pub(crate) fn fixture_observe_context_failure(
        &self,
        error: &str,
        title: &str,
        message: &str,
    ) -> bool {
        let Some(control) = self.fixture.as_ref() else {
            return false;
        };
        {
            let observation = control
                .context_notification
                .lock()
                .expect("context notification");
            match &*observation {
                FixtureContextNotification::Unarmed => return false,
                FixtureContextNotification::Consumed { .. } => {
                    drop(observation);
                    panic!("duplicate context failure notification");
                }
                FixtureContextNotification::Armed => {}
            }
        }
        control.validate_identity(self);
        let binding = {
            let state = self.owner.state.lock().expect("context failure owner");
            assert_eq!(state.executor_nonce, self.executor_nonce);
            assert!(
                matches!(&state.effects[LifecycleEffectSlot::ContextPreparation as usize],
                OwnedEffectState::Completed { outcome: Err(actual), .. } if actual == error)
            );
            state
                .replacement
                .as_ref()
                .expect("context replacement")
                .binding
        };
        let mut observation = control
            .context_notification
            .lock()
            .expect("context notification");
        if !matches!(*observation, FixtureContextNotification::Armed) {
            drop(observation);
            panic!("context failure notification changed before consumption");
        }
        *observation = FixtureContextNotification::Consumed {
            operation: self.owner.operation,
            executor: self.executor_nonce,
            binding,
            error: error.to_owned(),
            title: title.to_owned(),
            message: message.to_owned(),
        };
        true
    }

    #[cfg(test)]
    pub(crate) fn fixture_assert_context_failure_observer(
        &self,
        error: &str,
        title: &str,
        message: &str,
    ) {
        let Some(original) = self.fixture.as_ref() else {
            return;
        };
        if !matches!(
            *original
                .context_notification
                .lock()
                .expect("context notification"),
            FixtureContextNotification::Armed
        ) {
            return;
        }
        // Reuse this SAME completed native failure for primitive assertions;
        // leave the real command's armed control untouched.
        let temporary = Arc::new(ReplacementFixtureControl::default());
        let mut probe = self.clone();
        probe.fixture = Some(temporary.clone());
        assert!(!probe.fixture_observe_context_failure("not the actual error", title, message));
        assert!(temporary.identity.lock().unwrap().is_none());
        assert!(matches!(
            *temporary.context_notification.lock().unwrap(),
            FixtureContextNotification::Unarmed
        ));
        *temporary.context_notification.lock().unwrap() = FixtureContextNotification::Armed;
        assert!(probe.fixture_observe_context_failure(error, title, message));
        let first = temporary.context_notification.lock().unwrap().clone();
        assert!(matches!(&first, FixtureContextNotification::Consumed {
            operation, executor, binding, error: actual,
            title: actual_title, message: actual_message,
        } if *operation == self.owner.operation && *executor == self.executor_nonce
            && Some(*binding) == self.binding() && actual == error
            && actual_title == title && actual_message == message));
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            probe.fixture_observe_context_failure(error, title, message)
        }))
        .is_err());
        assert_eq!(*temporary.context_notification.lock().unwrap(), first);
        assert!(matches!(
            *original.context_notification.lock().unwrap(),
            FixtureContextNotification::Armed
        ));
    }

    #[cfg(test)]
    pub(crate) fn fixture_inserted(&self) {
        if let Some(control) = self.fixture.as_ref() {
            control.record(self, ReplacementFixtureCut::Inserted);
        }
    }

    #[cfg(test)]
    pub(crate) fn fixture_native(&self, seed: bool) -> Option<ReplacementFixtureNativeInvocation> {
        let control = self.fixture.as_ref()?;
        control.validate_identity(self);
        Some(ReplacementFixtureNativeInvocation {
            control: control.clone(),
            identity: (
                self.owner.operation,
                self.executor_nonce,
                self.binding().expect("actual native B"),
            ),
            enter: if seed {
                ReplacementFixtureCut::SeedNativeEnter
            } else {
                ReplacementFixtureCut::ContextNativeEnter
            },
            returned: if seed {
                ReplacementFixtureCut::SeedNativeReturned
            } else {
                ReplacementFixtureCut::ContextNativeReturned
            },
        })
    }

    #[cfg(test)]
    pub(crate) async fn fixture_reached(&self, cut: ReplacementFixtureCut) -> bool {
        match &self.fixture {
            Some(control) => control.reached(self, cut).await,
            None => true,
        }
    }

    #[cfg(test)]
    pub(crate) fn fixture_bind_twice(&self) -> bool {
        self.fixture
            .as_ref()
            .and_then(|control| {
                control.take_action(self, |action| {
                    matches!(action, ReplacementFixtureAction::BindPreparedTwice)
                })
            })
            .is_some()
    }

    #[cfg(test)]
    pub(crate) fn with_fixture_control(mut self, control: Arc<ReplacementFixtureControl>) -> Self {
        self.fixture = Some(control);
        self
    }

    pub(super) fn reservation_release_is_observed(&self, pty: &Arc<Mutex<PtyManager>>) -> bool {
        let tracker = self
            .owner
            .state
            .lock()
            .ok()
            .and_then(|state| state.replacement.as_ref()?.tracker.clone());
        let Some(tracker) = tracker else {
            return false;
        };
        pty.try_lock().ok().is_some_and(|pty| {
            pty.response_close_state().same_state(&tracker) && tracker.publishers_bound()
        })
    }

    pub(crate) async fn validate_pre_effect_region(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> bool {
        if !self.validate_source().await {
            return false;
        }
        let tracker = match pty.try_lock() {
            Ok(pty) => pty.response_close_state(),
            Err(_) => return false,
        };
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        manager
            .with_current_carrier_snapshot(&permit, |current| {
                self.validate_create_region(&permit, current, &tracker, pty)
            })
            .await
    }

    pub(crate) fn note_retained_work(&self, reason: String) {
        if let Ok(mut state) = self.owner.state.lock() {
            state.retained_reason = Some(reason);
        }
    }

    pub(crate) fn preserve_context_prerequisite(&self) -> bool {
        let Ok(mut state) = self.owner.state.lock() else {
            return false;
        };
        if state.executor_nonce != self.executor_nonce
            || state.stage != ReplacementStage::Admitted
            || !matches!(
                state.effects[LifecycleEffectSlot::ContextPreparation as usize],
                OwnedEffectState::Completed { .. }
            )
            || state
                .replacement
                .as_ref()
                .is_none_or(|progress| progress.context_prerequisite.is_some())
        {
            return false;
        }
        let receipt = std::mem::replace(
            &mut state.effects[LifecycleEffectSlot::ContextPreparation as usize],
            OwnedEffectState::NotStarted,
        );
        if let Some(progress) = state.replacement.as_mut() {
            progress.context_prerequisite = Some(receipt);
        }
        true
    }

    pub(crate) fn with_source_validation(mut self, validation: OwnedSourceValidation) -> Self {
        self.source_validation = Some(validation);
        self
    }

    pub(crate) fn source_stop(&self) -> Option<OwnedSourceStop> {
        self.owner
            .state
            .lock()
            .ok()?
            .replacement
            .as_ref()?
            .source_stop
    }

    pub(crate) async fn validate_source(&self) -> bool {
        if self.source_stop().is_some() {
            return false;
        }
        let result = match self.source_validation.as_ref() {
            Some(validate) => validate().await,
            None => Err(OwnedSourceStop::StoreTransient),
        };
        if let Err(reason) = result {
            if let Ok(mut state) = self.owner.state.lock() {
                if let Some(progress) = state.replacement.as_mut() {
                    progress.source_stop.get_or_insert(reason);
                }
                state.retained_reason = Some(format!("replacement source stopped: {reason:?}"));
            }
            return false;
        }
        self.source_stop().is_none()
    }

    pub(crate) fn operation(&self) -> Uuid {
        self.owner.operation
    }

    pub(crate) fn binding(&self) -> Option<PendingCreateBinding> {
        let state = self.owner.state.lock().ok()?;
        (state.executor_nonce == self.executor_nonce).then_some(state.replacement.as_ref()?.binding)
    }

    pub(crate) fn previous_for_launch(
        &self,
        _permit: &crate::session::manager::CarrierMutationPermit,
    ) -> Option<crate::session::session::Session> {
        let mut state = self.owner.state.lock().ok()?;
        if state.executor_nonce != self.executor_nonce
            || state.stage != ReplacementStage::PendingCreate
        {
            return None;
        }
        let progress = state.replacement.as_mut()?;
        if progress.launch_start_fresh.is_some() {
            return None;
        }
        progress.launch_start_fresh = Some(progress.current_previous.start_fresh_on_restore);
        Some(progress.current_previous.clone())
    }

    pub(crate) fn launch_token(&self) -> Option<Uuid> {
        let state = self.owner.state.lock().ok()?;
        (state.executor_nonce == self.executor_nonce).then(|| {
            state
                .replacement
                .as_ref()?
                .launch
                .as_ref()
                .map(|launch| launch.token)
        })?
    }

    pub(crate) fn matches_captured_launch_route(
        &self,
        proof: &crate::pty::manager::PtySnapshotRouteProof,
    ) -> bool {
        let launch = self
            .owner
            .state
            .lock()
            .ok()
            .and_then(|state| state.replacement.as_ref()?.launch.clone());
        launch.is_some_and(|launch| {
            proof.backend_kind() == launch.backend
                && proof.matches_requester_route(
                    launch.backend,
                    proof.saved_cwd(),
                    proof.saved_replica(),
                )
                && launch.cwd_identity.matches_route_observations(
                    Some(proof.saved_cwd()),
                    proof.saved_replica(),
                    launch.replica_identity.as_ref(),
                )
        })
    }

    pub(crate) fn current_completion_region(
        &self,
        permit: &crate::session::manager::CarrierMutationPermit,
        current: &crate::session::manager::CarrierSnapshot,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> bool {
        let captured = self.owner.state.lock().ok().and_then(|state| {
            if state.executor_nonce != self.executor_nonce
                || state.stage != ReplacementStage::FinalizedPublish
            {
                return None;
            }
            let progress = state.replacement.as_ref()?;
            if !progress.preparing_released
                || !progress.retired
                || !progress.retirement_cleanup_completed
            {
                return None;
            }
            Some((
                progress.launch.clone()?,
                progress.current_checkpoint.clone()?,
            ))
        });
        let Some((launch, carrier)) = captured else {
            return false;
        };
        Self::launch_is_current(&launch, current, pty)
            && crate::config::sessions_persistence::current_protected_carrier(
                permit, &carrier, current,
            )
            .is_some()
    }

    pub(crate) async fn refresh_completion_info(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> Option<SessionInfo> {
        if !self.validate_source().await {
            return None;
        }
        let captured = self.owner.state.lock().ok().and_then(|state| {
            if state.executor_nonce != self.executor_nonce
                || state.stage != ReplacementStage::FinalizedPublish
            {
                return None;
            }
            let progress = state.replacement.as_ref()?;
            if !progress.preparing_released
                || !progress.retired
                || !progress.retirement_cleanup_completed
            {
                return None;
            }
            Some((
                progress.launch.clone()?,
                progress.current_checkpoint.clone()?,
            ))
        })?;
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let observed = manager
            .with_current_carrier_snapshot(&permit, |current| {
                if !Self::launch_is_current(&captured.0, current, pty) {
                    return None;
                }
                let key = crate::config::sessions_persistence::current_protected_carrier(
                    &permit,
                    &captured.1,
                    current,
                )?;
                let row = current
                    .rows
                    .iter()
                    .find(|row| row.id == key.session_id && row.token == key.token)?;
                Some((SessionInfo::from(row), key))
            })
            .await?;
        {
            let mut state = self.owner.state.lock().ok()?;
            if state.executor_nonce != self.executor_nonce
                || state.stage != ReplacementStage::FinalizedPublish
            {
                return None;
            }
            // A new actual strict checkpoint is recovery evidence. The initial
            // post-finalization baseline stays immutable and is never rebased.
            state.replacement.as_mut()?.current_checkpoint = Some(observed.1);
        }
        drop(permit);
        Some(observed.0)
    }

    pub(crate) fn pre_write_work_matches(
        &self,
        current: &crate::session::manager::CarrierSnapshot,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> bool {
        let captured = self.owner.state.lock().ok().and_then(|state| {
            if state.executor_nonce != self.executor_nonce
                || state.stage != ReplacementStage::FinalizedPublish
            {
                return None;
            }
            let progress = state.replacement.as_ref()?;
            if !progress.preparing_released
                || !progress.retired
                || progress.source_stop.is_some()
                || !progress.retirement_cleanup_completed
            {
                return None;
            }
            Some((
                progress.launch.clone()?,
                progress.current_checkpoint.clone()?,
            ))
        });
        let Some((launch, carrier)) = captured else {
            return false;
        };
        Self::launch_is_current(&launch, current, pty)
            && carrier.matches_owned_work_snapshot(current, launch.cwd_identity.canonical_path())
    }

    pub(crate) fn validate_create_region(
        &self,
        permit: &crate::session::manager::CarrierMutationPermit,
        current: &crate::session::manager::CarrierSnapshot,
        tracker: &crate::pty::manager::ResponseCloseState,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> bool {
        use crate::pty::manager::{CreateAdmission, CreateClassification, ResponseRouteIdentity};
        let OwnedReplacementDescriptor::Replacement(descriptor) = &self.owner.descriptor else {
            return false;
        };
        let captured = self.owner.state.lock().ok().and_then(|mut state| {
            if state.executor_nonce != self.executor_nonce
                || !matches!(
                    state.stage,
                    ReplacementStage::Admitted | ReplacementStage::PendingCreate
                )
            {
                return None;
            }
            let progress = state.replacement.as_mut()?;
            if !progress.preparing_started
                || progress.cleanup != ReplacementCleanupAuthority::Executor(self.executor_nonce)
                || !progress
                    .tracker
                    .as_ref()
                    .is_some_and(|saved| saved.same_state(tracker))
            {
                return None;
            }
            Some((
                progress.previous_carrier.clone(),
                progress.binding,
                progress.create_reservation.take(),
            ))
        });
        let Some((carrier, binding, reservation)) = captured else {
            return false;
        };
        // Only this operation's old reservation is released, under continuous U.
        drop(reservation);
        let Some(actual) = crate::config::sessions_persistence::renew_carrier_replacement(
            permit,
            &carrier,
            self.owner.operation,
            descriptor.previous.id,
            binding.session_id(),
            current,
        ) else {
            return false;
        };
        if !descriptor
            .replica_identity
            .as_ref()
            .is_some_and(|physical| actual.retired_local_replica_matches(physical))
        {
            return false;
        }
        let Some(generation) = actual.retired_local_generation() else {
            return false;
        };
        let Some(previous) = current
            .rows
            .iter()
            .find(|row| row.id == actual.session_id && row.token == actual.token)
            .cloned()
        else {
            return false;
        };
        if !pty.try_lock().ok().is_some_and(|pty| {
            pty.backend_kind(actual.session_id).is_none()
                && tracker.generation(actual.session_id).is_none()
        }) {
            return false;
        }
        let epoch = match crate::config::sessions_persistence::classify_create_carrier_scope(
            permit,
            &descriptor.target,
            std::path::Path::new(&descriptor.cwd),
        ) {
            crate::config::sessions_persistence::CreateCarrierScope::Protected {
                epoch,
                preparing_owner: Some(owner),
                ..
            } if owner == self.owner.operation => epoch,
            _ => return false,
        };
        let reservation = match tracker.reserve_create_owner(
            &descriptor.target,
            self.owner.operation,
            CreateClassification::Preparing {
                owner_nonce: self.owner.operation,
                identity: ResponseRouteIdentity {
                    target: descriptor.target.clone(),
                    session_id: actual.session_id,
                    token: actual.token,
                    generation,
                    backend: crate::pty::backend::SessionBackendKind::LocalProcess,
                    carrier_version: actual.version,
                },
                publication_epoch: epoch,
            },
        ) {
            Ok(CreateAdmission::Reserved(reservation)) => reservation,
            _ => return false,
        };
        let Ok(mut state) = self.owner.state.lock() else {
            return false;
        };
        let Some(progress) = state.replacement.as_mut() else {
            return false;
        };
        if progress
            .launch_start_fresh
            .is_some_and(|fresh| fresh != previous.start_fresh_on_restore)
        {
            state.retained_reason =
                Some("replacement launch fresh decision changed; current A preserved".into());
            return false;
        }
        progress.current_previous = previous;
        progress.previous_carrier = actual;
        progress.create_reservation = Some(reservation);
        true
    }

    pub(crate) fn mark_pending(&self, binding: PendingCreateBinding) -> bool {
        let Ok(mut state) = self.owner.state.lock() else {
            return false;
        };
        if state.executor_nonce != self.executor_nonce || state.stage != ReplacementStage::Admitted
        {
            return false;
        }
        if !state
            .replacement
            .as_ref()
            .is_some_and(|progress| progress.binding == binding && progress.preparing_started)
        {
            return false;
        }
        state.stage = ReplacementStage::PendingCreate;
        true
    }

    /// Bind the actual observed spawn once; no launch witness/query invents a
    /// backend result, and a later route is never adopted as the initial launch.
    pub(crate) fn record_launch(
        &self,
        session: &crate::session::session::Session,
        generation: u64,
    ) -> bool {
        let OwnedReplacementDescriptor::Replacement(descriptor) = &self.owner.descriptor else {
            return false;
        };
        if session.shell != descriptor.shell
            || session.backend_kind != descriptor.backend
            || session.working_directory != descriptor.cwd
            || session.agent_id != descriptor.requested_agent_id
        {
            return false;
        }
        let Ok(mut state) = self.owner.state.lock() else {
            return false;
        };
        if state.executor_nonce != self.executor_nonce
            || state.stage != ReplacementStage::PendingCreate
        {
            return false;
        }
        let Some(progress) = state.replacement.as_mut() else {
            return false;
        };
        if progress.binding.session_id() != session.id
            || progress.launch.is_some()
            || progress.cleanup != ReplacementCleanupAuthority::Executor(self.executor_nonce)
        {
            return false;
        }
        progress.launch = Some(OwnedLaunchReceipt {
            operation: self.owner.operation,
            preparing_nonce: progress.preparing_nonce,
            binding: progress.binding,
            token: session.token,
            generation,
            backend: session.backend_kind,
            info: SessionInfo::from(session),
            cwd_identity: descriptor.cwd_identity.clone(),
            replica_identity: descriptor.replica_identity.clone(),
        });
        true
    }

    pub(super) fn launch_is_current(
        launch: &OwnedLaunchReceipt,
        current: &crate::session::manager::CarrierSnapshot,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> bool {
        let Some(row) = current.rows.iter().find(|row| {
            row.id == launch.binding.session_id()
                && row.token == launch.token
                && row.backend_kind == launch.backend
                && !row.is_root_agent
                && !row.is_coordinator
                && !row.was_detached
                && !matches!(row.status, SessionStatus::Exited(_))
                && row.working_directory == launch.info.working_directory
                && row.shell == launch.info.shell
                && row.shell_args == launch.info.shell_args
                && row.effective_shell_args == launch.info.effective_shell_args
                && row.agent_id == launch.info.agent_id
                && row.agent_label == launch.info.agent_label
                && row.agent_kind == launch.info.agent_kind
                && row.requested_profile == launch.info.requested_profile
                && row.effective_profile == launch.info.effective_profile
                && row.profile_fallback_chain == launch.info.profile_fallback_chain
                && row.profile_fallback_applied == launch.info.profile_fallback_applied
                && row.profile_content_hash == launch.info.profile_content_hash
                && row.trusted_configured_spawn == launch.info.trusted_configured_spawn
                && row.effective_codex_home == launch.info.effective_codex_home
        }) else {
            return false;
        };
        pty.try_lock().ok().is_some_and(|pty| {
            let Some((Some(cwd), replica, generation)) = pty.route_identities(row.id) else {
                return false;
            };
            generation == launch.generation
                && pty.backend_kind(row.id) == Some(launch.backend)
                && pty.has_session(row.id)
                && launch.cwd_identity.matches_route_observations(
                    Some(&cwd),
                    replica.as_ref(),
                    launch.replica_identity.as_ref(),
                )
        })
    }

    pub(super) fn retained_commit_copy(&self) -> Option<CommitResult> {
        let state = self.owner.state.lock().ok()?;
        let committed = state.replacement.as_ref()?.committed.as_ref()?;
        Some(CommitResult {
            removed_rows: committed.removed_rows.clone(),
            missing_ids: committed.missing_ids.clone(),
            changed_rows: committed.changed_rows.clone(),
            finalized_rows: committed.finalized_rows.clone(),
            cleared_raise_hand_ids: committed.cleared_raise_hand_ids.clone(),
            selection: committed.selection.clone(),
            activity: committed.activity.clone(),
        })
    }

    #[cfg(test)]
    pub(super) fn fixture_stop_exact_b(
        &self,
        pty: &Arc<Mutex<PtyManager>>,
        launch: &OwnedLaunchReceipt,
        _permit: &crate::session::manager::CarrierMutationPermit,
    ) {
        let stop = self
            .fixture
            .as_ref()
            .and_then(|control| {
                control.take_action(self, |action| {
                    matches!(action, ReplacementFixtureAction::FixtureStopExactB)
                })
            })
            .is_some();
        if stop {
            assert!(
                self.fixture
                    .as_ref()
                    .is_some_and(|control| control.mock_backend.load(Ordering::Acquire)),
                "stop-exact-B requires fixture mock backend"
            );
            let pty = pty.lock().expect("fixture mock backend lock");
            assert_eq!(
                pty.backend_kind(launch.binding.session_id()),
                Some(launch.backend)
            );
            assert_eq!(
                pty.response_close_state()
                    .generation(launch.binding.session_id()),
                Some(launch.generation)
            );
            pty.kill_for_kind(launch.binding.session_id(), launch.backend)
                .expect("fixture stop exact B");
        }
    }

    pub(super) async fn apply_finalized_carry(
        &self,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> bool {
        if !self.validate_source().await {
            return false;
        }
        let Some(effect) = self.start_effect(LifecycleEffectSlot::Carry) else {
            return false;
        };
        #[cfg(test)]
        if !self
            .fixture_reached(ReplacementFixtureCut::BeforeCarry)
            .await
        {
            return false;
        }
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let admission = manager
            .with_current_carrier_snapshot(&permit, |current| {
                self.finalized_carry_capability(&permit, current, pty)
            })
            .await;
        let capability = match admission {
            Some(Some(capability)) => capability,
            Some(None) => {
                // Initial version remains immutable. A newer B is reconciled
                // as it stands and never overwritten by A's old carry.
                effect.complete(Err("carry recapture: newer B preserved".into()));
                if let Ok(mut state) = self.owner.state.lock() {
                    state.stage = ReplacementStage::FinalizedPreRename;
                }
                return true;
            }
            None => return false,
        };
        let outcome = manager
            .apply_finalized_owned_replacement_carry(&permit, capability)
            .await;
        match outcome {
            FinalizedOwnedCarryOutcome::Applied => {
                // Consume Carry synchronously before any further await.
                if let Ok(mut state) = self.owner.state.lock() {
                    state.stage = ReplacementStage::FinalizedPreRename;
                } else {
                    return false;
                }
                effect.complete(Ok(None));
            }
            FinalizedOwnedCarryOutcome::Recapture => {
                effect.complete(Err("carry recapture: finalized binding changed".into()));
                return false;
            }
        }
        // Read the real version after CarrierStateWrite::drop, still under U.
        let launch = self.owner.state.lock().ok().and_then(|state| {
            state
                .replacement
                .as_ref()
                .and_then(|progress| progress.launch.clone())
        });
        let Some(launch) = launch else {
            return false;
        };
        manager
            .with_current_carrier_snapshot(&permit, |current| {
                Self::launch_is_current(&launch, current, pty).then_some(())?;
                current
                    .versions
                    .get(&launch.binding.session_id())
                    .filter(|version| **version < u64::MAX)?;
                Some(())
            })
            .await
            .is_some()
    }
}
