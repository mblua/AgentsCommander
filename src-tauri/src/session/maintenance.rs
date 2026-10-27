use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use uuid::Uuid;

/// Owns volatile session-maintenance reservations, including aliases of one operation.
/// The newtype preserves Tauri state identity; a daemon restart drops all reservations.
#[derive(Default)]
pub struct PendingSelfClear {
    entries: Mutex<HashMap<Uuid, Entry>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Lease(Uuid);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MaintenanceError {
    AlreadyLegacy,
    MissingLease,
    AliasOccupied,
    NotPublishing,
    ConflictPlain,
}

impl fmt::Display for MaintenanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AlreadyLegacy => "legacy maintenance already queued for this session",
            Self::MissingLease => "maintenance lease no longer owns a reservation",
            Self::AliasOccupied => "session already has pending maintenance",
            Self::NotPublishing => "plain maintenance is not publishing",
            Self::ConflictPlain => "plain maintenance pending for this session",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlainIdentity {
    pub session_id: Uuid,
    pub request_id: Uuid,
    pub action: String,
    pub requested_by: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PlainReservation {
    New(Lease),
    SamePublishing,
    SameArmed,
    Conflict,
}

#[derive(Clone)]
enum PlainPhase {
    Publishing,
    Armed,
}

#[derive(Clone)]
enum Class {
    Legacy,
    Plain(PlainIdentity, PlainPhase),
}

#[derive(Clone)]
struct Entry {
    lease: Lease,
    class: Class,
}

impl PendingSelfClear {
    pub(crate) fn reserve_legacy(&self, sid: Uuid) -> Result<Lease, MaintenanceError> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entries.get(&sid) {
            return Err(match entry.class {
                Class::Legacy => MaintenanceError::AlreadyLegacy,
                Class::Plain(..) => MaintenanceError::ConflictPlain,
            });
        }
        let lease = Lease(Uuid::new_v4());
        entries.insert(
            sid,
            Entry {
                lease: lease.clone(),
                class: Class::Legacy,
            },
        );
        Ok(lease)
    }

    pub(crate) fn reserve_plain(&self, identity: PlainIdentity) -> PlainReservation {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entries.get(&identity.session_id) {
            return match &entry.class {
                Class::Plain(owner, phase) if owner == &identity => match phase {
                    PlainPhase::Publishing => PlainReservation::SamePublishing,
                    PlainPhase::Armed => PlainReservation::SameArmed,
                },
                _ => PlainReservation::Conflict,
            };
        }
        let lease = Lease(Uuid::new_v4());
        entries.insert(
            identity.session_id,
            Entry {
                lease: lease.clone(),
                class: Class::Plain(identity, PlainPhase::Publishing),
            },
        );
        PlainReservation::New(lease)
    }

    pub(crate) fn arm_plain(&self, lease: &Lease) -> Result<(), MaintenanceError> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = entries
            .values_mut()
            .find(|entry| &entry.lease == lease)
            .ok_or(MaintenanceError::MissingLease)?;
        match &mut entry.class {
            Class::Plain(_, phase @ PlainPhase::Publishing) => {
                *phase = PlainPhase::Armed;
                Ok(())
            }
            _ => Err(MaintenanceError::NotPublishing),
        }
    }

    pub(crate) fn legacy_lease(&self, sid: Uuid) -> Option<Lease> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&sid)
            .filter(|entry| matches!(entry.class, Class::Legacy))
            .map(|entry| entry.lease.clone())
    }

    pub(crate) fn alias(&self, lease: &Lease, sid: Uuid) -> Result<(), MaintenanceError> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = entries
            .values()
            .find(|entry| &entry.lease == lease)
            .cloned()
            .ok_or(MaintenanceError::MissingLease)?;
        if entries.contains_key(&sid) {
            return Err(MaintenanceError::AliasOccupied);
        }
        entries.insert(sid, entry);
        Ok(())
    }

    pub(crate) fn release(&self, lease: &Lease) -> bool {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let before = entries.len();
        entries.retain(|_, entry| &entry.lease != lease);
        before != entries.len()
    }
}

#[cfg(test)]
impl PendingSelfClear {
    pub(crate) fn seed_legacy(&self, sid: Uuid) -> bool {
        self.reserve_legacy(sid).is_ok()
    }

    pub(crate) fn contains(&self, sid: Uuid) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&sid)
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    pub(crate) fn remove_legacy_fixture(&self, sid: Uuid) -> bool {
        self.legacy_lease(sid)
            .is_some_and(|lease| self.release(&lease))
    }
}

#[cfg(test)]
mod tests;
