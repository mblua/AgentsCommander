#[cfg(test)]
use super::{parse_config_scope, ManifestSource, MAX_MANIFEST_ROWS};
use super::{
    CanonicalSnapshot, DecodedPathComponents, ManifestDegradedReason, ManifestFileKind,
    ManifestPathIdentity, ManifestRecordOutcome, ManifestState, ProjectSeedManifestGuard,
    PublishedManifestRow, ResourceBoundKind, SeedManifestError,
};
#[cfg(test)]
use chrono::{DateTime, Utc};
use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use thiserror::Error;

/// #1065 Stage F: soft project-gate acquisition for the automatic, fail-soft
/// context and config publishers (discovery scan, session read/self-heal,
/// explicit overwrite).
///
/// It distinguishes the three frozen degraded cases in plan sections 5.2/6.1:
/// * `Held`: the gate is owned; publish and record under it.
/// * `DegradedUntracked`: a pre-contention lock capability/I-O error left no
///   guard; the target may still publish, but unrecorded.
/// * `Unavailable`: a contention timeout, post-contention lock I/O error, or an
///   unsafe/reparse/identity path; skip the operation rather than race a
///   cooperating writer or follow a substituted path.
///
/// The classification mirrors `commands::entity_creation::acquire_lifecycle_project_gate`
/// (which keeps its `Result<Option<_>, String>` shape for the error-returning
/// deletion contract); this enum form suits the fail-soft automatic publishers.
// The `Held` variant carries the full guard, so the enum is guard-sized. This is a
// transient return value that every caller matches and destructures immediately, so
// the disparity is a single stack move (identical in cost to returning the guard
// itself); boxing it would only add an allocation on the per-spawn gate-acquire path.
#[allow(clippy::large_enum_variant)]
pub(crate) enum SoftProjectGate {
    Held(ProjectSeedManifestGuard),
    DegradedUntracked,
    Unavailable(SeedManifestError),
}

pub(crate) fn acquire_project_gate_soft(project_root: &Path) -> SoftProjectGate {
    match ProjectSeedManifestGuard::acquire(project_root) {
        Ok(guard) => SoftProjectGate::Held(guard),
        Err(
            error @ (SeedManifestError::UnsafePath { .. }
            | SeedManifestError::BusyTimeout { .. }
            | SeedManifestError::LockIo {
                saw_contention: true,
                ..
            }),
        ) => SoftProjectGate::Unavailable(error),
        // Pre-contention capability or other lock-acquisition error: preserve the
        // existing ungated behavior as untracked degradation.
        Err(_) => SoftProjectGate::DegradedUntracked,
    }
}

#[derive(Debug)]
pub(crate) struct ManifestActivationToken {
    pub(crate) _private: (),
}

impl ManifestActivationToken {
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self { _private: () }
    }

    /// The sole production activation constructor.
    ///
    /// Supported publishers and lifecycle hooks thread this token into the
    /// recorder. Historical entries in [`V1_COVERAGE_BOUNDARIES`] need not remain
    /// active publishers. There is deliberately no other non-test construction
    /// path, so code without this token cannot mutate a manifest.
    pub(crate) fn production() -> Self {
        Self { _private: () }
    }

    pub(crate) fn authorize(&self) {}
}

/// Historical Stage F coverage-boundary declaration.
///
/// These stable names preserve the compatibility vocabulary established by the
/// original activation rollout. They include historical publication boundaries
/// that no longer emit canonical rows, so this declaration is not a checklist of
/// current production publishers or wiring evidence.
///
/// #1318 - [`V1CoverageBoundary::CatalogSeed`] is the first coverage-v2-era
/// publisher; the enum keeps its historical name (minimal diff) while the
/// declaration remains the exhaustive historical coverage-boundary checklist
/// across coverage versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum V1CoverageBoundary {
    /// `commands::ac_discovery::create_ac_project` fresh-root context creation.
    DirectCreateAcProjectFreshRoot,
    /// `config::seeded_context_templates` create-missing project context (also the
    /// `config::projects` new-project registration re-ensure path).
    ContextCreate,
    /// `config::seeded_context_templates` recognized-update project context.
    ContextUpdate,
    /// `config::session_context::heal_stale_global_context_template` self-heal.
    ContextSelfHeal,
    /// `config::seeded_context_templates::overwrite_context_template_with_default`.
    ContextOverwrite,
    /// Coordinator pristine v2 -> v4 recognized update.
    CoordinatorStatelessV2ToV4,
    /// Coordinator pristine v3 -> v4 recognized update.
    CoordinatorStatelessV3ToV4,
    /// Coordinator seeded v3 -> v4 recognized update with state-version bump.
    CoordinatorSeededV3ToV4,
    /// Historical stable coverage-boundary name for exact config publication.
    ConfigExactPublish,
    /// Historical stable coverage-boundary name for over-bound config publication.
    ConfigOverBoundPublish,
    /// Historical stable coverage-boundary name for failed config restoration.
    ConfigFailedRestore,
    /// `commands::entity_creation`/`cli::team` replica removal prune.
    LifecycleReplicaRemoval,
    /// `commands::entity_creation`/`cli::workgroup` workgroup removal prune.
    LifecycleWorkgroupRemoval,
    /// `commands::entity_creation::delete_team` team-deletion prune.
    LifecycleTeamDeletion,
    /// `commands::entity_creation::delete_agent_matrix` Agent Matrix prune.
    LifecycleAgentMatrixDeletion,
    /// Agent Matrix pending-inclusive delete protection staged-rollback prune.
    PendingInclusiveDeleteProtection,
    /// `config::coding_agents_catalog` per-project catalog seed + seed-manifest
    /// publication (`ensure_seeded_for_project` / `record_catalog_publication`,
    /// #1318). The first coverage-v2-era publisher: the catalog row kind only
    /// exists at `coverage_version = 2`.
    CatalogSeed,
}

/// The exhaustive, ordered coverage set. Its length equals the number of
/// [`V1CoverageBoundary`] variants; the `v1_coverage_declaration_is_exhaustive`
/// test enforces that a new boundary is added here and matched, so a coverage
/// entry cannot be dropped from the checklist silently.
pub(crate) const V1_COVERAGE_BOUNDARIES: [V1CoverageBoundary; 17] = [
    V1CoverageBoundary::DirectCreateAcProjectFreshRoot,
    V1CoverageBoundary::ContextCreate,
    V1CoverageBoundary::ContextUpdate,
    V1CoverageBoundary::ContextSelfHeal,
    V1CoverageBoundary::ContextOverwrite,
    V1CoverageBoundary::CoordinatorStatelessV2ToV4,
    V1CoverageBoundary::CoordinatorStatelessV3ToV4,
    V1CoverageBoundary::CoordinatorSeededV3ToV4,
    V1CoverageBoundary::ConfigExactPublish,
    V1CoverageBoundary::ConfigOverBoundPublish,
    V1CoverageBoundary::ConfigFailedRestore,
    V1CoverageBoundary::LifecycleReplicaRemoval,
    V1CoverageBoundary::LifecycleWorkgroupRemoval,
    V1CoverageBoundary::LifecycleTeamDeletion,
    V1CoverageBoundary::LifecycleAgentMatrixDeletion,
    V1CoverageBoundary::PendingInclusiveDeleteProtection,
    V1CoverageBoundary::CatalogSeed,
];

pub(crate) const BLOCKING_WORK_QUEUED: u8 = 0;
pub(crate) const BLOCKING_WORK_STARTED: u8 = 1;
pub(crate) const BLOCKING_WORK_CANCELED: u8 = 2;

#[derive(Debug, Error)]
pub(crate) enum BlockingTaskError {
    #[error("blocking task canceled before start")]
    CanceledBeforeStart,
    #[error("blocking task join failed: {0}")]
    Join(#[source] tokio::task::JoinError),
}

pub(crate) enum BlockingWorkResult<T> {
    Completed(T),
    CanceledBeforeStart,
}

pub(crate) struct BlockingWaiterGuard {
    pub(crate) state: Arc<AtomicU8>,
    pub(crate) abort: tokio::task::AbortHandle,
    pub(crate) armed: bool,
}

impl BlockingWaiterGuard {
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for BlockingWaiterGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if self
            .state
            .compare_exchange(
                BLOCKING_WORK_QUEUED,
                BLOCKING_WORK_CANCELED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.abort.abort();
        }
    }
}

/// Runs blocking work with explicit ownership of the queued-to-start boundary.
/// Dropping an unpolled future schedules nothing. Dropping a polled waiter
/// cancels work that is still queued, while already-started work remains owned
/// by the blocking worker and runs to completion.
pub(crate) async fn run_blocking_owned<F, T>(work: F) -> Result<T, BlockingTaskError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let state = Arc::new(AtomicU8::new(BLOCKING_WORK_QUEUED));
    let worker_state = Arc::clone(&state);
    let handle = tokio::task::spawn_blocking(move || {
        if worker_state
            .compare_exchange(
                BLOCKING_WORK_QUEUED,
                BLOCKING_WORK_STARTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return BlockingWorkResult::CanceledBeforeStart;
        }
        BlockingWorkResult::Completed(work())
    });
    let mut waiter = BlockingWaiterGuard {
        state: Arc::clone(&state),
        abort: handle.abort_handle(),
        armed: true,
    };
    let joined = handle.await;
    waiter.disarm();
    match joined {
        Ok(BlockingWorkResult::Completed(result)) => Ok(result),
        Ok(BlockingWorkResult::CanceledBeforeStart) => Err(BlockingTaskError::CanceledBeforeStart),
        Err(error)
            if error.is_cancelled() && state.load(Ordering::Acquire) == BLOCKING_WORK_CANCELED =>
        {
            Err(BlockingTaskError::CanceledBeforeStart)
        }
        Err(error) => Err(BlockingTaskError::Join(error)),
    }
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct PublishedScopeBatch {
    pub(crate) scope: String,
    pub(crate) rows: Vec<PublishedManifestRow>,
}

#[cfg(test)]
impl PublishedScopeBatch {
    pub(crate) fn new(
        scope: String,
        source: ManifestSource,
        files: Vec<ManifestPathIdentity>,
        published_at: DateTime<Utc>,
    ) -> Result<Self, SeedManifestError> {
        parse_config_scope(&scope)?;
        if source == ManifestSource::Builtin {
            return Err(SeedManifestError::Validation(
                "config scope publication cannot use builtin source".to_string(),
            ));
        }
        if files.len() > MAX_MANIFEST_ROWS {
            return Err(SeedManifestError::resource_bound(
                ResourceBoundKind::Rows,
                MAX_MANIFEST_ROWS,
                files.len(),
            ));
        }
        let mut keys = BTreeSet::new();
        let mut rows = Vec::with_capacity(files.len());
        for path in files {
            let row =
                PublishedManifestRow::replica_config(path, scope.clone(), source, published_at)?;
            if !keys.insert(row.path.key()) {
                return Err(SeedManifestError::Validation(
                    "scope publication contains a duplicate path identity".to_string(),
                ));
            }
            rows.push(row);
        }
        Ok(Self { scope, rows })
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ManifestLifecycleFilter {
    ConfigPathPrefix(Vec<String>),
    ReplicaComponent(String),
    WorkgroupComponents(BTreeSet<String>),
}

impl ManifestLifecycleFilter {
    pub(crate) fn config_path_prefix(
        prefix: ManifestPathIdentity,
    ) -> Result<Self, SeedManifestError> {
        let DecodedPathComponents::Utf8(components) = prefix.components else {
            return Err(SeedManifestError::Validation(
                "lifecycle config prefix must use canonical UTF-8 components".to_string(),
            ));
        };
        if components.len() < 2 {
            return Err(SeedManifestError::Validation(
                "lifecycle config prefix must include a room component".to_string(),
            ));
        }
        crate::commands::entity_creation::parse_team_from_workgroup_name(&components[1])
            .map_err(SeedManifestError::Validation)?;
        if let Some(replica) = components.get(2) {
            let agent = replica.strip_prefix("__agent_").ok_or_else(|| {
                SeedManifestError::Validation(format!(
                    "lifecycle replica prefix must be named __agent_<name>: {replica}"
                ))
            })?;
            crate::commands::entity_creation::validate_existing_name(agent, "Agent")
                .map_err(SeedManifestError::Validation)?;
        }
        Ok(Self::ConfigPathPrefix(components))
    }

    pub(crate) fn replica_component(component: String) -> Result<Self, SeedManifestError> {
        let agent = component.strip_prefix("__agent_").ok_or_else(|| {
            SeedManifestError::Validation(format!(
                "replica component must be named __agent_<name>: {component}"
            ))
        })?;
        crate::commands::entity_creation::validate_existing_name(agent, "Agent")
            .map_err(SeedManifestError::Validation)?;
        Ok(Self::ReplicaComponent(component))
    }

    pub(crate) fn workgroup_components(
        workgroups: impl IntoIterator<Item = String>,
    ) -> Result<Self, SeedManifestError> {
        let mut validated = BTreeSet::new();
        for workgroup in workgroups {
            crate::commands::entity_creation::parse_team_from_workgroup_name(&workgroup)
                .map_err(SeedManifestError::Validation)?;
            validated.insert(workgroup);
        }
        Ok(Self::WorkgroupComponents(validated))
    }

    pub(crate) fn matches(&self, row: &PublishedManifestRow) -> bool {
        if row.kind != ManifestFileKind::ReplicaConfigFile {
            return false;
        }
        match self {
            Self::ConfigPathPrefix(prefix) => row.path.starts_with_utf8_components(prefix),
            Self::ReplicaComponent(replica) => row.path.utf8_component_equals(2, replica),
            Self::WorkgroupComponents(workgroups) => workgroups
                .iter()
                .any(|workgroup| row.path.utf8_component_equals(1, workgroup)),
        }
    }
}

pub(crate) enum ProjectPublicationPermitState<'a> {
    Tracked(&'a mut ProjectSeedManifestGuard),
    DegradedUntracked {
        held_guard: Option<&'a mut ProjectSeedManifestGuard>,
        reason: ManifestDegradedReason,
    },
}

pub(crate) struct ProjectPublicationPermit<'a> {
    pub(crate) state: ProjectPublicationPermitState<'a>,
}

impl fmt::Debug for ProjectPublicationPermit<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (mode, reason, holds_guard) = match &self.state {
            ProjectPublicationPermitState::Tracked(_) => ("tracked", None, true),
            ProjectPublicationPermitState::DegradedUntracked { held_guard, reason } => {
                ("degraded_untracked", Some(*reason), held_guard.is_some())
            }
        };
        formatter
            .debug_struct("ProjectPublicationPermit")
            .field("mode", &mode)
            .field("reason", &reason)
            .field("holds_guard", &holds_guard)
            .finish()
    }
}

impl<'a> ProjectPublicationPermit<'a> {
    pub(crate) fn degraded_without_guard(reason: ManifestDegradedReason) -> Self {
        Self {
            state: ProjectPublicationPermitState::DegradedUntracked {
                held_guard: None,
                reason,
            },
        }
    }

    pub(crate) fn is_tracked(&self) -> bool {
        matches!(self.state, ProjectPublicationPermitState::Tracked(_))
    }

    pub(crate) fn record_file(
        self,
        activation: &ManifestActivationToken,
        row: PublishedManifestRow,
    ) -> ManifestRecordOutcome {
        activation.authorize();
        match self.state {
            ProjectPublicationPermitState::Tracked(guard) => guard.record_file(row),
            ProjectPublicationPermitState::DegradedUntracked { reason, .. } => {
                ManifestRecordOutcome::PublishedUnrecorded(reason)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn replace_scope(
        self,
        activation: &ManifestActivationToken,
        batch: PublishedScopeBatch,
    ) -> ManifestRecordOutcome {
        activation.authorize();
        match self.state {
            ProjectPublicationPermitState::Tracked(guard) => guard.replace_scope(batch),
            ProjectPublicationPermitState::DegradedUntracked { reason, .. } => {
                ManifestRecordOutcome::PublishedUnrecorded(reason)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn remove_unrecordable_scope(
        self,
        activation: &ManifestActivationToken,
        scope: String,
        bound: ResourceBoundKind,
    ) -> ManifestRecordOutcome {
        activation.authorize();
        if let Err(error) = parse_config_scope(&scope) {
            log::warn!(
                "[seed_manifest] rejected invalid unrecordable scope={} error={}",
                scope,
                error
            );
            return ManifestRecordOutcome::PublishedUnrecorded(
                ManifestDegradedReason::InvalidCanonical,
            );
        }
        match self.state {
            ProjectPublicationPermitState::Tracked(guard) => {
                guard.remove_scope_as_unrecorded(&scope, bound)
            }
            ProjectPublicationPermitState::DegradedUntracked { reason, .. } => {
                ManifestRecordOutcome::PublishedUnrecorded(reason)
            }
        }
    }

    pub(crate) fn apply_lifecycle_filter(
        self,
        activation: &ManifestActivationToken,
        filter: ManifestLifecycleFilter,
    ) -> ManifestRecordOutcome {
        activation.authorize();
        match self.state {
            ProjectPublicationPermitState::Tracked(guard) => guard.apply_lifecycle_filter(filter),
            ProjectPublicationPermitState::DegradedUntracked { reason, .. } => {
                ManifestRecordOutcome::PublishedUnrecorded(reason)
            }
        }
    }
}

impl ProjectSeedManifestGuard {
    pub(crate) fn writable_state_mut(&mut self) -> Option<&mut ManifestState> {
        match &mut self.snapshot {
            CanonicalSnapshot::Writable { state, .. } => Some(state),
            CanonicalSnapshot::ReadOnly { .. } => None,
        }
    }

    pub(crate) fn record_file(&mut self, row: PublishedManifestRow) -> ManifestRecordOutcome {
        let scope = row.scope.clone();
        let Some(state) = self.writable_state_mut() else {
            return ManifestRecordOutcome::PublishedUnrecorded(
                self.snapshot
                    .reason()
                    .unwrap_or(ManifestDegradedReason::InvalidCanonical),
            );
        };
        let journal = state.upsert(row);
        if !journal.changed(state) {
            journal.rollback(state);
            return ManifestRecordOutcome::Unchanged;
        }
        match self.persist_current_state() {
            Ok(outcome) => outcome,
            Err(SeedManifestError::ResourceBound { kind, .. }) => {
                if let Some(state) = self.writable_state_mut() {
                    journal.rollback(state);
                }
                self.remove_scope_as_unrecorded(&scope, kind)
            }
            Err(error) => {
                if let Some(state) = self.writable_state_mut() {
                    journal.rollback(state);
                }
                log_record_failure(&scope, &error);
                ManifestRecordOutcome::PublishedUnrecorded(error.degraded_reason())
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn replace_scope(&mut self, batch: PublishedScopeBatch) -> ManifestRecordOutcome {
        let Some(state) = self.writable_state_mut() else {
            return ManifestRecordOutcome::PublishedUnrecorded(
                self.snapshot
                    .reason()
                    .unwrap_or(ManifestDegradedReason::InvalidCanonical),
            );
        };
        let journal = state.replace_scope(&batch.scope, batch.rows);
        if !journal.changed(state) {
            journal.rollback(state);
            return ManifestRecordOutcome::Unchanged;
        }
        match self.persist_current_state() {
            Ok(outcome) => outcome,
            Err(SeedManifestError::ResourceBound { kind, .. }) => {
                if let Some(state) = self.writable_state_mut() {
                    journal.rollback(state);
                }
                self.remove_scope_as_unrecorded(&batch.scope, kind)
            }
            Err(error) => {
                if let Some(state) = self.writable_state_mut() {
                    journal.rollback(state);
                }
                log_record_failure(&batch.scope, &error);
                ManifestRecordOutcome::PublishedUnrecorded(error.degraded_reason())
            }
        }
    }

    pub(crate) fn remove_scope_as_unrecorded(
        &mut self,
        scope: &str,
        bound: ResourceBoundKind,
    ) -> ManifestRecordOutcome {
        let Some(state) = self.writable_state_mut() else {
            return ManifestRecordOutcome::PublishedUnrecorded(
                self.snapshot
                    .reason()
                    .unwrap_or(ManifestDegradedReason::ResourceBound(bound)),
            );
        };
        let journal = state.remove_scope(scope);
        if journal.changed() {
            if let Err(error) = self.persist_current_state() {
                if let Some(state) = self.writable_state_mut() {
                    journal.rollback(state);
                }
                log_record_failure(scope, &error);
                return ManifestRecordOutcome::PublishedUnrecorded(error.degraded_reason());
            }
        }
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::ResourceBound(bound))
    }

    pub(crate) fn apply_lifecycle_filter(
        &mut self,
        filter: ManifestLifecycleFilter,
    ) -> ManifestRecordOutcome {
        let Some(state) = self.writable_state_mut() else {
            return ManifestRecordOutcome::PublishedUnrecorded(
                self.snapshot
                    .reason()
                    .unwrap_or(ManifestDegradedReason::InvalidCanonical),
            );
        };
        let journal = state.remove_matching(|row| filter.matches(row));
        if !journal.changed() {
            return ManifestRecordOutcome::Unchanged;
        }
        match self.persist_current_state() {
            Ok(outcome) => outcome,
            Err(error) => {
                if let Some(state) = self.writable_state_mut() {
                    journal.rollback(state);
                }
                log_record_failure("lifecycle-filter", &error);
                ManifestRecordOutcome::PublishedUnrecorded(error.degraded_reason())
            }
        }
    }
}

pub(crate) fn log_record_failure(scope: &str, error: &SeedManifestError) {
    log::warn!(
        "[seed_manifest] target or lifecycle action succeeded but manifest persistence did not \
            scope={} error={}",
        scope,
        error
    );
}
