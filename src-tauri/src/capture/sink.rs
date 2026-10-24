//! The one-candidate-per-session slot (#2232 phase 3).
//!
//! The slot holds **the last record received** and nothing else: no
//! concatenation, no window, no accumulator. Phase 5 assembles a multi-record
//! Codex turn inside the watcher, **before** the slot, so what arrives here is
//! already one complete candidate and the slot never holds a fragment.
//!
//! Leaf module: `tokio::sync`, std, `capture::record` and `capture::key` only.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use crate::capture::key::{ConsumptionKey, Cut};
use crate::capture::record::{CapturedRecord, RecordOrigin};

/// What the slot currently holds, with the sequence that identifies it.
///
/// `seq` is strictly increasing across every transition, including
/// invalidation and clearing, so a consumer that captured a sequence can always
/// tell whether the slot moved underneath it.
#[derive(Clone, Debug)]
pub struct SlotState {
    pub seq: u64,
    pub value: SlotValue,
}

#[derive(Clone, Debug)]
pub enum SlotValue {
    Empty,
    Valid(Arc<CapturedRecord>),
    Invalid(String),
}

impl SlotValue {
    pub fn record(&self) -> Option<&Arc<CapturedRecord>> {
        match self {
            Self::Valid(record) => Some(record),
            _ => None,
        }
    }
}

/// Why [`CaptureSlot::offer`] did not publish a record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfferOutcome {
    /// The record is the new candidate.
    Published,
    /// A cut registered over a live reader hides it (section 7).
    HiddenByCut,
    /// A preamble line whose key the slot has already seen or is holding
    /// (section 8): a live re-delivery must not overwrite a pending candidate.
    DuplicatePreamble,
}

/// The per-session candidate slot.
///
/// Cheap to clone: every clone shares one channel and one guard state.
#[derive(Clone, Debug)]
pub struct CaptureSlot {
    tx: Arc<watch::Sender<SlotState>>,
    guards: Arc<Mutex<Guards>>,
}

#[derive(Debug, Default)]
struct Guards {
    cut: Option<Cut>,
    seen_preamble: HashSet<ConsumptionKey>,
}

impl Default for CaptureSlot {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureSlot {
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(SlotState {
            seq: 0,
            value: SlotValue::Empty,
        });
        Self {
            tx: Arc::new(tx),
            guards: Arc::new(Mutex::new(Guards::default())),
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<SlotState> {
        self.tx.subscribe()
    }

    /// Snapshot `(seq, value)`. The read guard is dropped before returning:
    /// never hold it across a consume, which would deadlock against
    /// `send_modify`.
    pub fn snapshot(&self) -> SlotState {
        self.tx.borrow().clone()
    }

    pub fn seq(&self) -> u64 {
        self.tx.borrow().seq
    }

    /// Register the cut for a demand raised over an already-running reader.
    pub fn set_cut(&self, cut: Cut) {
        self.lock_guards().cut = Some(cut);
    }

    pub fn cut(&self) -> Option<Cut> {
        self.lock_guards().cut.clone()
    }

    /// Drop the sequence tie-break of the cut, at reader start and at every
    /// re-anchor, before the first record is processed (section 7).
    pub fn supersede_cut_sequence(&self) {
        if let Some(cut) = self.lock_guards().cut.as_mut() {
            cut.supersede_sequence();
        }
    }

    /// Offer `record`, whose authoritative `epoch` the caller has already
    /// resolved through `capture::state`.
    pub fn offer(&self, record: Arc<CapturedRecord>, epoch: u64) -> OfferOutcome {
        let key = ConsumptionKey::from_record(&record, epoch);
        {
            let mut guards = self.lock_guards();
            if guards
                .cut
                .as_ref()
                .is_some_and(|cut| cut.hides(&record, epoch))
            {
                return OfferOutcome::HiddenByCut;
            }
            if record.origin == RecordOrigin::Preamble {
                let held = self
                    .tx
                    .borrow()
                    .value
                    .record()
                    .map(|held| ConsumptionKey::from_record(held, epoch));
                if held.as_ref() == Some(&key) || guards.seen_preamble.contains(&key) {
                    return OfferOutcome::DuplicatePreamble;
                }
                guards.seen_preamble.insert(key);
            }
        }
        self.publish(SlotValue::Valid(record));
        OfferOutcome::Published
    }

    /// Replace the candidate with a typed invalidation.
    pub fn invalidate(&self, reason: impl Into<String>) {
        self.publish(SlotValue::Invalid(reason.into()));
    }

    pub fn clear(&self) {
        self.publish(SlotValue::Empty);
    }

    /// Every transition, **including publication**, goes through `send_modify`.
    ///
    /// With `send_replace` the new state is built outside the lock and races the
    /// consumer, so `seq` can go backwards or repeat; `send_modify` increments
    /// inside the critical section.
    fn publish(&self, value: SlotValue) {
        self.tx.send_modify(|state| {
            state.seq = state.seq.wrapping_add(1);
            state.value = value;
        });
    }

    /// Consume the candidate only if the slot still holds `expected_seq` **and**
    /// `expected_key`. Otherwise nothing is touched and `None` is returned.
    ///
    /// The read guard is extracted and dropped before `send_modify` runs:
    /// holding both deadlocks.
    pub fn try_consume(
        &self,
        expected_seq: u64,
        expected_key: &ConsumptionKey,
        epoch: u64,
    ) -> Option<Arc<CapturedRecord>> {
        let (seq, candidate) = {
            let state = self.tx.borrow();
            seq_and_record(&state)
        };
        if seq != expected_seq {
            return None;
        }
        let candidate = candidate?;
        if &ConsumptionKey::from_record(&candidate, epoch) != expected_key {
            return None;
        }

        let mut taken = None;
        self.tx.send_modify(|state| {
            // Re-check inside the critical section: a record may have arrived
            // between the snapshot above and this point.
            if state.seq != expected_seq {
                return;
            }
            if let SlotValue::Valid(record) = &state.value {
                if Arc::ptr_eq(record, &candidate) {
                    taken = Some(Arc::clone(record));
                    state.seq = state.seq.wrapping_add(1);
                    state.value = SlotValue::Empty;
                }
            }
        });
        taken
    }

    fn lock_guards(&self) -> std::sync::MutexGuard<'_, Guards> {
        self.guards.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn seq_and_record(state: &SlotState) -> (u64, Option<Arc<CapturedRecord>>) {
    (state.seq, state.value.record().map(Arc::clone))
}

#[cfg(test)]
mod tests;
