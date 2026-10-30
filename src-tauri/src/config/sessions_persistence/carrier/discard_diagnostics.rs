//! Owned, nonauthoritative observations made only after the discard veto.

use super::publication::{publication_slot_values, ProtectionPhase};
use crate::session::manager::CarrierMutationPermit;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiscardVetoReason {
    ProtectionClosing,
    ProtectionPreparing,
    RegistryLockUnreadable,
    SlotLockUnreadable,
    BlockerUnavailable,
}

impl DiscardVetoReason {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::ProtectionClosing => "protection_closing",
            Self::ProtectionPreparing => "protection_preparing",
            Self::RegistryLockUnreadable => "registry_lock_unreadable",
            Self::SlotLockUnreadable => "slot_lock_unreadable",
            Self::BlockerUnavailable => "blocker_unavailable",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiscardVetoObservation {
    pub session: Uuid,
    pub reason: DiscardVetoReason,
    pub protected_session: Option<Uuid>,
    pub target: Option<String>,
    pub phase: Option<&'static str>,
    pub protection_nonce: Option<Uuid>,
    pub token: Option<Uuid>,
    pub epoch: Option<u64>,
    pub version: Option<u64>,
    pub owner_nonce: Option<Uuid>,
    pub previous: Option<Uuid>,
    pub replacement: Option<Uuid>,
}

impl DiscardVetoObservation {
    fn unknown(session: Uuid, reason: DiscardVetoReason) -> Self {
        Self {
            session,
            reason,
            protected_session: None,
            target: None,
            phase: None,
            protection_nonce: None,
            token: None,
            epoch: None,
            version: None,
            owner_nonce: None,
            previous: None,
            replacement: None,
        }
    }
}

pub(crate) fn observe_discard_veto(
    _permit: &CarrierMutationPermit,
    id: Uuid,
) -> Vec<DiscardVetoObservation> {
    let slots = match publication_slot_values() {
        Some(slots) => slots,
        None => {
            return vec![DiscardVetoObservation::unknown(
                id,
                DiscardVetoReason::RegistryLockUnreadable,
            )];
        }
    };
    let mut observations = Vec::new();
    for slot in slots {
        let Ok(state) = slot.lock() else {
            observations.push(DiscardVetoObservation::unknown(
                id,
                DiscardVetoReason::SlotLockUnreadable,
            ));
            continue;
        };
        for protection in &state.protections {
            let (reason, phase, relation) = match protection.phase {
                ProtectionPhase::Closing if protection.session_id == id => {
                    (DiscardVetoReason::ProtectionClosing, "Closing", None)
                }
                ProtectionPhase::Preparing {
                    owner_nonce,
                    previous,
                    replacement,
                } if protection.session_id == id || previous == id || replacement == id => (
                    DiscardVetoReason::ProtectionPreparing,
                    "Preparing",
                    Some((owner_nonce, previous, replacement)),
                ),
                _ => continue,
            };
            observations.push(DiscardVetoObservation {
                session: id,
                reason,
                protected_session: Some(protection.session_id),
                target: Some(protection.target.clone()),
                phase: Some(phase),
                protection_nonce: Some(protection.nonce),
                token: Some(protection.token),
                epoch: Some(protection.epoch),
                version: Some(protection.version),
                owner_nonce: relation.map(|r| r.0),
                previous: relation.map(|r| r.1),
                replacement: relation.map(|r| r.2),
            });
        }
    }
    if observations.is_empty() {
        observations.push(DiscardVetoObservation::unknown(
            id,
            DiscardVetoReason::BlockerUnavailable,
        ));
    }
    observations
}

#[cfg(test)]
pub(crate) fn poison_discard_slot_for_test(carrier: &super::publication::PublishedCarrier) {
    let slot = carrier.slot.clone();
    assert!(std::thread::spawn(move || {
        let _guard = slot.lock().unwrap();
        panic!("issue_3248 deliberate slot poison");
    })
    .join()
    .is_err());
}
