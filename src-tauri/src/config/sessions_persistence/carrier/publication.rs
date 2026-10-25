use super::super::record::PersistedSession;
use crate::session::session::SessionStatus;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
pub(in super::super) struct PublicationCheckpoint {
    pub(in super::super) nonce: Uuid,
    pub(in super::super) epoch: u64,
    pub(in super::super) rows: Vec<PersistedSession>,
    pub(in super::super) versions: HashMap<Uuid, u64>,
    pub(in super::super) confirmed: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub(in super::super) enum ProtectionPhase {
    Protected,
    Preparing {
        owner_nonce: Uuid,
        previous: Uuid,
        replacement: Uuid,
    },
    Closing,
}

/// Historical evidence minted only after the real Dormant settlement succeeds.
/// Current authority always comes from a new Confirmed snapshot under U.
#[derive(Clone, PartialEq, Eq)]
pub(in super::super) struct RetiredLocalCarrierFact {
    pub(in super::super) session_id: Uuid,
    pub(in super::super) token: Uuid,
    pub(in super::super) target: String,
    pub(in super::super) protection_nonce: Uuid,
    pub(in super::super) canonical_replica: PathBuf,
    pub(in super::super) physical_replica: crate::session::manager::CarrierPhysicalIdentity,
    pub(in super::super) generation: u64,
    pub(in super::super) committed_version: u64,
    pub(in super::super) settled_version: u64,
    pub(in super::super) checkpoint_nonce: Uuid,
    pub(in super::super) publication_epoch: u64,
}

#[derive(Clone)]
pub(in super::super) struct CarrierProtection {
    pub(in super::super) target: String,
    pub(in super::super) session_id: Uuid,
    pub(in super::super) token: Uuid,
    pub(in super::super) lifetime: std::sync::Weak<()>,
    pub(in super::super) retained: Option<std::sync::Arc<()>>,
    pub(in super::super) pending_lifetime: Option<std::sync::Arc<()>>,
    pub(in super::super) active: Option<std::sync::Arc<()>>,
    pub(in super::super) nonce: Uuid,
    pub(in super::super) epoch: u64,
    pub(in super::super) version: u64,
    pub(in super::super) row: PersistedSession,
    pub(in super::super) canonical_cwd: PathBuf,
    pub(in super::super) phase: ProtectionPhase,
    pub(in super::super) retired_local: Option<RetiredLocalCarrierFact>,
}

#[derive(Clone, Default)]
pub(in super::super) struct PublicationState {
    // Independent mock apps share this process registry in parallel unit tests.
    // Keep alias/replacement protections together in their publication slot.
    #[cfg(test)]
    pub(in super::super) restore_test_scopes: Vec<std::sync::Weak<std::sync::atomic::AtomicBool>>,
    pub(in super::super) canonical_directory: Option<PathBuf>,
    pub(in super::super) epoch: u64,
    pub(in super::super) checkpoint: Option<PublicationCheckpoint>,
    pub(in super::super) protections: Vec<CarrierProtection>,
}

pub(in super::super) type PublicationSlot = std::sync::Arc<Mutex<PublicationState>>;

pub(in super::super) fn publication_slots() -> &'static Mutex<HashMap<PathBuf, PublicationSlot>> {
    static SLOTS: std::sync::OnceLock<Mutex<HashMap<PathBuf, PublicationSlot>>> =
        std::sync::OnceLock::new();
    SLOTS.get_or_init(Default::default)
}

pub(in super::super) fn publication_slot(dir: &Path) -> Option<PublicationSlot> {
    let mut slots = publication_slots().lock().ok()?;
    Some(slots.entry(dir.to_path_buf()).or_default().clone())
}

pub(in super::super) enum PublicationDirectoryIdentity {
    Legacy,
    Known(PathBuf),
    Unknown,
}

/// Native identity probes run before hot U/M capture. The zero-protection
/// legacy path performs no additional filesystem probes.
pub(in super::super) fn probe_publication_directory(
    dir: &Path,
    initial: bool,
) -> PublicationDirectoryIdentity {
    match all_carrier_protections() {
        Some(protections) if protections.is_empty() => {
            if !initial {
                return PublicationDirectoryIdentity::Legacy;
            }
            std::fs::canonicalize(dir)
                .map(PublicationDirectoryIdentity::Known)
                .unwrap_or(PublicationDirectoryIdentity::Legacy)
        }
        Some(_) => std::fs::canonicalize(dir)
            .map(PublicationDirectoryIdentity::Known)
            .unwrap_or(PublicationDirectoryIdentity::Unknown),
        None => PublicationDirectoryIdentity::Unknown,
    }
}

/// Pure alias adoption under U. Every physical directory shares one slot, so
/// an alternate path cannot retain a separately confirmed epoch or protection.
pub(in super::super) fn resolve_publication_slot(
    dir: &Path,
    identity: PublicationDirectoryIdentity,
) -> Option<PublicationSlot> {
    let canonical = match identity {
        PublicationDirectoryIdentity::Legacy => {
            return all_carrier_protections()?
                .is_empty()
                .then(|| publication_slot(dir))
                .flatten();
        }
        PublicationDirectoryIdentity::Known(path) => path,
        PublicationDirectoryIdentity::Unknown => return None,
    };
    let mut slots = publication_slots().lock().ok()?;
    let mut matching: Option<PublicationSlot> = None;
    for slot in slots.values() {
        let state = slot.lock().ok()?;
        if state.canonical_directory.as_ref() == Some(&canonical) {
            if matching
                .as_ref()
                .is_some_and(|previous| !std::sync::Arc::ptr_eq(previous, slot))
            {
                return None;
            }
            matching = Some(slot.clone());
        }
    }
    let slot = matching.unwrap_or_else(|| slots.entry(dir.to_path_buf()).or_default().clone());
    slot.lock().ok()?.canonical_directory = Some(canonical);
    slots.insert(dir.to_path_buf(), slot.clone());
    Some(slot)
}

/// Owned upper binding. The caller supplies the authenticated canonical target
/// and verified replica root; this module never derives target authority from CWD.
#[derive(Clone)]
pub(crate) struct InitialCarrierIntent {
    pub target: String,
    pub session_id: Uuid,
    pub token: Uuid,
    pub version: u64,
    pub canonical_replica: PathBuf,
    pub(in super::super) restore_exclusion: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub(in super::super) lifetime: std::sync::Arc<()>,
}

impl InitialCarrierIntent {
    pub(crate) fn new(
        target: String,
        session_id: Uuid,
        token: Uuid,
        version: u64,
        canonical_replica: PathBuf,
    ) -> Self {
        Self {
            target,
            session_id,
            token,
            version,
            canonical_replica,
            restore_exclusion: None,
            lifetime: std::sync::Arc::new(()),
        }
    }

    /// Upper passes the same data-only restore exclusion held by its coordinator.
    pub(crate) fn with_restore_exclusion(
        mut self,
        exclusion: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.restore_exclusion = Some(exclusion);
        self
    }

    pub(in super::super) fn restore_allows_publication(&self) -> bool {
        !self
            .restore_exclusion
            .as_ref()
            .is_some_and(|active| active.load(Ordering::Acquire))
    }

    pub(in super::super) fn matches(
        &self,
        captured: &crate::session::manager::CarrierSnapshot,
        rows: &[PersistedSession],
    ) -> bool {
        !captured.pending_ids.contains(&self.session_id)
            && captured.versions.get(&self.session_id) == Some(&self.version)
            && captured.rows.iter().any(|row| {
                row.id == self.session_id
                    && row.token == self.token
                    && !row.is_root_agent
                    && !row.is_coordinator
                    && !matches!(row.status, SessionStatus::Exited(_))
            })
            && rows
                .iter()
                .filter(|row| {
                    row.id.as_deref().and_then(|id| Uuid::parse_str(id).ok())
                        == Some(self.session_id)
                })
                .count()
                == 1
    }
}

#[derive(Clone)]
pub(crate) struct PublishedCarrier {
    pub target: String,
    pub session_id: Uuid,
    pub token: Uuid,
    pub nonce: Uuid,
    pub version: u64,
    pub(in super::super) slot: PublicationSlot,
    pub directory: PathBuf,
    pub(in super::super) lifetime: std::sync::Arc<()>,
    pub(in super::super) retired_local: Option<RetiredLocalCarrierFact>,
}

impl PublishedCarrier {
    /// Historical generation, never a live route or writer capability.
    pub(crate) fn retired_local_generation(&self) -> Option<u64> {
        self.retired_local.as_ref().map(|fact| fact.generation)
    }
    pub(crate) fn retired_local_replica_matches(
        &self,
        physical: &crate::session::manager::CarrierPhysicalIdentity,
    ) -> bool {
        self.retired_local
            .as_ref()
            .is_some_and(|fact| fact.physical_replica.same_object(physical))
    }

    /// Read-only first-write check under the injector's existing current M and
    /// route writer guard. No U acquisition, filesystem probe or publication.
    pub(crate) fn matches_owned_work_snapshot(
        &self,
        current: &crate::session::manager::CarrierSnapshot,
        canonical_cwd: &Path,
    ) -> bool {
        if self.version == u64::MAX
            || current.versions.get(&self.session_id) != Some(&self.version)
            || current.pending_ids.contains(&self.session_id)
            || !current
                .rows
                .iter()
                .any(|row| row.id == self.session_id && row.token == self.token)
        {
            return false;
        }
        let Some(protections) = all_carrier_protections() else {
            return false;
        };
        let mut related = protections.iter().filter(|protection| {
            protection.target == self.target
                || canonical_cwd.starts_with(&protection.canonical_cwd)
                || protection.canonical_cwd.starts_with(canonical_cwd)
        });
        let Some(protection) = related.next() else {
            return false;
        };
        if related.next().is_some()
            || protection.target != self.target
            || protection.session_id != self.session_id
            || protection.nonce != self.nonce
            || protection.token != self.token
            || protection.version != self.version
            || protection.phase != ProtectionPhase::Protected
            || !canonical_cwd.starts_with(&protection.canonical_cwd)
        {
            return false;
        }
        let Ok(state) = self.slot.lock() else {
            return false;
        };
        state.checkpoint.as_ref().is_some_and(|checkpoint| {
            checkpoint.confirmed
                && checkpoint.epoch < u64::MAX
                && checkpoint.epoch == state.epoch
                && protection.epoch == checkpoint.epoch
                && checkpoint.versions.get(&self.session_id) == Some(&self.version)
                && checkpoint
                    .rows
                    .iter()
                    .any(|row| same_carrier_projection(row, &protection.row))
        })
    }
}

pub(crate) struct CurrentCarrierCheckpoint {
    pub nonce: Uuid,
    pub epoch: u64,
    pub version: u64,
}

pub(in super::super) fn prepare_protected_publication(
    next: &mut PublicationState,
    rows: &[PersistedSession],
    captured: &crate::session::manager::CarrierSnapshot,
    initial: Option<&InitialCarrierIntent>,
) -> bool {
    next.protections
        .retain(|protection| protection.lifetime.strong_count() != 0);
    for protection in &mut next.protections {
        let mut matches = rows.iter().filter(|row| {
            row.id.as_deref().and_then(|id| Uuid::parse_str(id).ok()) == Some(protection.session_id)
        });
        let Some(row) = matches.next() else {
            return false;
        };
        let Some(version) = captured.versions.get(&protection.session_id) else {
            return false;
        };
        if matches.next().is_some()
            || captured.pending_ids.contains(&protection.session_id)
            || !captured.rows.iter().any(|current| {
                current.id == protection.session_id && current.token == protection.token
            })
        {
            return false;
        }
        if !captured.rows.iter().any(|current| {
            current.id == protection.session_id
                && current.token == protection.token
                && crate::session::manager::CarrierSnapshot::is_local_interactive_carrier(current)
                && matches!(current.status, SessionStatus::Exited(0))
                && current.working_directory == protection.row.working_directory
        }) {
            protection.retired_local = None;
        }
        protection.row = row.clone();
        protection.version = *version;
        protection.epoch = next.epoch;
    }
    let Some(initial) = initial else {
        return true;
    };
    if !initial.matches(captured, rows) {
        return false;
    }
    #[cfg(test)]
    if let Some(scope) = &initial.restore_exclusion {
        next.restore_test_scopes
            .retain(|scope| scope.strong_count() != 0);
        if !next.restore_test_scopes.iter().any(|known| {
            known
                .upgrade()
                .is_some_and(|known| std::sync::Arc::ptr_eq(&known, scope))
        }) {
            next.restore_test_scopes
                .push(std::sync::Arc::downgrade(scope));
        }
    }
    let Some(all) = all_carrier_protections() else {
        return false;
    };
    if all.iter().any(|protection| {
        (protection.session_id == initial.session_id || protection.target == initial.target)
            && !next
                .protections
                .iter()
                .any(|own| own.nonce == protection.nonce && own.session_id == protection.session_id)
    }) {
        return false;
    }
    if let Some(protection) = next
        .protections
        .iter_mut()
        .find(|protection| protection.session_id == initial.session_id)
    {
        if protection.target != initial.target
            || protection.token != initial.token
            || !matches!(protection.phase, ProtectionPhase::Protected)
        {
            return false;
        }
        if protection.retained.is_none() {
            protection.lifetime = std::sync::Arc::downgrade(&initial.lifetime);
            protection.pending_lifetime = Some(initial.lifetime.clone());
        }
        return true;
    }
    let Some(row) = rows.iter().find(|row| {
        row.id.as_deref().and_then(|id| Uuid::parse_str(id).ok()) == Some(initial.session_id)
    }) else {
        return false;
    };
    next.protections.push(CarrierProtection {
        target: initial.target.clone(),
        session_id: initial.session_id,
        token: initial.token,
        nonce: Uuid::new_v4(),
        epoch: next.epoch,
        version: initial.version,
        row: row.clone(),
        canonical_cwd: initial.canonical_replica.clone(),
        phase: ProtectionPhase::Protected,
        retired_local: None,
        lifetime: std::sync::Arc::downgrade(&initial.lifetime),
        retained: None,
        pending_lifetime: Some(initial.lifetime.clone()),
        active: None,
    });
    true
}

pub(in super::super) fn published_initial_carrier(
    slot: &PublicationSlot,
    initial: &InitialCarrierIntent,
    dir: &Path,
) -> Option<PublishedCarrier> {
    let state = slot.lock().ok()?;
    let checkpoint = state.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.confirmed && checkpoint.epoch < u64::MAX && checkpoint.epoch == state.epoch
    })?;
    let protection = state.protections.iter().find(|protection| {
        protection.target == initial.target
            && protection.session_id == initial.session_id
            && protection.token == initial.token
            && protection.version == initial.version
            && protection.epoch == checkpoint.epoch
            && matches!(protection.phase, ProtectionPhase::Protected)
    })?;
    if checkpoint.versions.get(&initial.session_id) != Some(&initial.version) {
        return None;
    }
    let lifetime = protection.lifetime.upgrade()?;
    Some(PublishedCarrier {
        target: protection.target.clone(),
        session_id: protection.session_id,
        token: protection.token,
        nonce: protection.nonce,
        version: protection.version,
        slot: slot.clone(),
        directory: dir.to_path_buf(),
        lifetime,
        retired_local: None,
    })
}

pub(in super::super) fn all_carrier_protections() -> Option<Vec<CarrierProtection>> {
    let slots = publication_slots()
        .lock()
        .ok()?
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let mut protections = Vec::new();
    let mut seen = HashSet::new();
    for slot in slots {
        if !seen.insert(std::sync::Arc::as_ptr(&slot) as usize) {
            continue;
        }
        let state = slot.lock().ok()?;
        for protection in &state.protections {
            if let Some(active) = protection.lifetime.upgrade() {
                let mut protection = protection.clone();
                protection.active = Some(active);
                protections.push(protection);
            }
        }
    }
    Some(protections)
}

pub(in super::super) fn same_carrier_projection(
    a: &PersistedSession,
    b: &PersistedSession,
) -> bool {
    a.id == b.id
        && a.name == b.name
        && a.shell == b.shell
        && a.shell_args == b.shell_args
        && a.working_directory == b.working_directory
        && a.was_active == b.was_active
        && a.git_repos.len() == b.git_repos.len()
        && a.git_repos
            .iter()
            .zip(&b.git_repos)
            .all(|(a, b)| a.label == b.label && a.source_path == b.source_path)
        && a.is_coordinator == b.is_coordinator
        && a.is_root_agent == b.is_root_agent
        && a.agent_id == b.agent_id
        && a.agent_label == b.agent_label
        && a.requested_profile == b.requested_profile
        && a.effective_profile == b.effective_profile
        && a.telegram_bot_id == b.telegram_bot_id
        && a.was_detached == b.was_detached
        && a.start_fresh_on_restore == b.start_fresh_on_restore
        && a.communication == b.communication
        && match (&a.status, &b.status) {
            (Some(SessionStatus::Exited(a)), Some(SessionStatus::Exited(b))) => a == b,
            (Some(SessionStatus::Exited(_)), _) | (_, Some(SessionStatus::Exited(_))) => false,
            _ => true,
        }
}
