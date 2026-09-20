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
mod tests {
    use super::*;
    use crate::capture::record::CaptureProvider;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn record(
        text: &str,
        start: Option<u64>,
        seq: u64,
        origin: RecordOrigin,
    ) -> Arc<CapturedRecord> {
        record_in(
            PathBuf::from("/tmp/#2232/session.jsonl"),
            text,
            start,
            seq,
            origin,
        )
    }

    fn record_in(
        file: PathBuf,
        text: &str,
        start: Option<u64>,
        seq: u64,
        origin: RecordOrigin,
    ) -> Arc<CapturedRecord> {
        let text_sha256: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(text.as_bytes()).into();
        Arc::new(CapturedRecord {
            session_id: "s".to_owned(),
            text: text.to_owned(),
            file: file.clone(),
            epoch: 0,
            record_start: start,
            reader_seq: seq,
            text_sha256,
            turn_id: None,
            provider: CaptureProvider::Claude,
            provider_final: false,
            turn_identified: false,
            origin,
            observed_path: file,
            observed_len: 0,
            observed_prefix: Vec::new(),
        })
    }

    fn key(record: &Arc<CapturedRecord>) -> ConsumptionKey {
        ConsumptionKey::from_record(record, 0)
    }

    // Test 1: `send_modify` publication under concurrent consume. `seq` is
    // strictly increasing and never repeats over 1000 interleaved operations.
    //
    // "Never repeats" is proved by counting, not by sampling: every transition
    // increments inside the critical section, so the final sequence must equal
    // the number of transitions exactly — 1000 publications plus one per
    // applied consume. A `send_replace` implementation, which builds the new
    // state outside the lock, loses increments here and lands below the count.
    #[test]
    fn seq_is_strictly_increasing_under_concurrent_publish_and_consume() {
        const PUBLICATIONS: u64 = 1000;
        let slot = CaptureSlot::new();
        let consumed = Arc::new(AtomicU64::new(0));

        let publisher = {
            let slot = slot.clone();
            std::thread::spawn(move || {
                for i in 0..PUBLICATIONS {
                    slot.offer(record("t", Some(i), i, RecordOrigin::Live), 0);
                }
            })
        };
        let consumer = {
            let slot = slot.clone();
            let consumed = Arc::clone(&consumed);
            std::thread::spawn(move || {
                let mut seen = Vec::with_capacity(4096);
                let mut last = 0u64;
                while last < PUBLICATIONS {
                    let state = slot.snapshot();
                    if state.seq != last {
                        assert!(
                            state.seq > last,
                            "seq went backwards or repeated: {last} -> {}",
                            state.seq
                        );
                        seen.push(state.seq);
                        last = state.seq;
                    }
                    if let Some(record) = state.value.record().cloned() {
                        if slot.try_consume(state.seq, &key(&record), 0).is_some() {
                            consumed.fetch_add(1, Ordering::AcqRel);
                        }
                    }
                }
                // Every sequence this thread observed was strictly greater than
                // the one before it, so none repeated.
                assert!(
                    seen.windows(2).all(|w| w[0] < w[1]),
                    "observed sequences are not strictly increasing"
                );
            })
        };
        publisher.join().expect("publisher");
        consumer.join().expect("consumer");

        let applied = consumed.load(Ordering::Acquire);
        assert_eq!(
            slot.seq(),
            PUBLICATIONS + applied,
            "every transition must increment exactly once: {PUBLICATIONS} publications \
             plus {applied} applied consumes"
        );
    }

    // Test 2: a stale `seq` consumes nothing and leaves the slot untouched.
    #[test]
    fn a_stale_seq_consumes_nothing() {
        let slot = CaptureSlot::new();
        let first = record("one", Some(0), 0, RecordOrigin::Live);
        slot.offer(Arc::clone(&first), 0);
        let stale_seq = slot.seq();
        let second = record("two", Some(10), 1, RecordOrigin::Live);
        slot.offer(Arc::clone(&second), 0);
        let after = slot.snapshot();

        assert!(slot.try_consume(stale_seq, &key(&first), 0).is_none());
        let now = slot.snapshot();
        assert_eq!(now.seq, after.seq);
        assert_eq!(
            now.value.record().map(|r| r.text.clone()),
            Some("two".into())
        );
    }

    // Test 3: a stale key consumes nothing and leaves the slot untouched.
    #[test]
    fn a_stale_key_consumes_nothing() {
        let slot = CaptureSlot::new();
        let held = record("held", Some(0), 0, RecordOrigin::Live);
        slot.offer(Arc::clone(&held), 0);
        let seq = slot.seq();
        let other = record("other", Some(0), 0, RecordOrigin::Live);

        assert!(slot.try_consume(seq, &key(&other), 0).is_none());
        let now = slot.snapshot();
        assert_eq!(now.seq, seq);
        assert_eq!(
            now.value.record().map(|r| r.text.clone()),
            Some("held".into())
        );
    }

    // Test 4: a record arriving between revalidate and consume changes `seq`;
    // the consume does not apply and the newer record remains available.
    #[test]
    fn a_record_arriving_between_revalidate_and_consume_wins() {
        let slot = CaptureSlot::new();
        let old = record("old", Some(0), 0, RecordOrigin::Live);
        slot.offer(Arc::clone(&old), 0);
        let revalidated_seq = slot.seq();
        let old_key = key(&old);

        // The newer record arrives before the consumer gets to act.
        let fresh = record("fresh", Some(20), 1, RecordOrigin::Live);
        slot.offer(Arc::clone(&fresh), 0);

        assert!(slot.try_consume(revalidated_seq, &old_key, 0).is_none());
        let state = slot.snapshot();
        assert_eq!(
            state.value.record().map(|r| r.text.clone()),
            Some("fresh".into())
        );
        // And the fresh record is consumable on the next trigger.
        let consumed = slot
            .try_consume(state.seq, &key(&fresh), 0)
            .expect("fresh record consumable");
        assert_eq!(consumed.text, "fresh");
    }

    #[test]
    fn a_matching_seq_and_key_consumes_and_empties_the_slot() {
        let slot = CaptureSlot::new();
        let record = record("go", Some(0), 0, RecordOrigin::Live);
        slot.offer(Arc::clone(&record), 0);
        let seq = slot.seq();
        let consumed = slot.try_consume(seq, &key(&record), 0).expect("consumed");
        assert_eq!(consumed.text, "go");
        assert!(matches!(slot.snapshot().value, SlotValue::Empty));
        // Exactly once: the second attempt finds nothing.
        assert!(slot.try_consume(seq, &key(&record), 0).is_none());
    }

    // Test 11: a fresh reader starting at `reader_seq == 0` after a restart does
    // not ignore everything, because the sequence tie-break is superseded.
    #[test]
    fn a_fresh_reader_after_a_restart_is_not_cut_away() {
        let slot = CaptureSlot::new();
        slot.set_cut(Cut {
            path: crate::capture::key::normalise_path(&PathBuf::from("/tmp/#2232/session.jsonl")),
            epoch: 0,
            len: 0,
            reader_seq: Some(42),
        });
        slot.supersede_cut_sequence();
        let fresh = record("after restart", Some(0), 0, RecordOrigin::Live);
        assert_eq!(slot.offer(fresh, 0), OfferOutcome::Published);
    }

    #[test]
    fn a_cut_hides_a_record_below_the_demand_length() {
        let slot = CaptureSlot::new();
        slot.set_cut(Cut {
            path: crate::capture::key::normalise_path(&PathBuf::from("/tmp/#2232/session.jsonl")),
            epoch: 0,
            len: 500,
            reader_seq: None,
        });
        assert_eq!(
            slot.offer(record("old", Some(100), 7, RecordOrigin::Live), 0),
            OfferOutcome::HiddenByCut
        );
        assert!(matches!(slot.snapshot().value, SlotValue::Empty));
    }

    // Test 12: after the epoch advances the cut is superseded, so a low-offset
    // record is no longer ignored.
    #[test]
    fn an_advanced_epoch_supersedes_the_cut() {
        let slot = CaptureSlot::new();
        slot.set_cut(Cut {
            path: crate::capture::key::normalise_path(&PathBuf::from("/tmp/#2232/session.jsonl")),
            epoch: 0,
            len: 500,
            reader_seq: None,
        });
        assert_eq!(
            slot.offer(
                record("new file, offset 0", Some(0), 0, RecordOrigin::Live),
                1
            ),
            OfferOutcome::Published
        );
    }

    // Test 14: a preamble record whose key is already in the slot does not
    // overwrite it.
    #[test]
    fn a_duplicate_preamble_does_not_overwrite_the_slot() {
        let slot = CaptureSlot::new();
        let preamble = record("preamble body", None, 0, RecordOrigin::Preamble);
        assert_eq!(
            slot.offer(Arc::clone(&preamble), 0),
            OfferOutcome::Published
        );
        let seq = slot.seq();
        assert_eq!(
            slot.offer(Arc::clone(&preamble), 0),
            OfferOutcome::DuplicatePreamble
        );
        assert_eq!(slot.seq(), seq, "a refused offer must not move the slot");

        // Even after the candidate has been consumed, the seen set still refuses
        // a live re-delivery of the same preamble line.
        slot.try_consume(seq, &key(&preamble), 0).expect("consumed");
        assert_eq!(slot.offer(preamble, 0), OfferOutcome::DuplicatePreamble);
    }

    #[test]
    fn invalidation_and_clearing_advance_the_sequence() {
        let slot = CaptureSlot::new();
        let start = slot.seq();
        slot.invalidate("session gone");
        assert!(matches!(slot.snapshot().value, SlotValue::Invalid(_)));
        slot.clear();
        assert!(matches!(slot.snapshot().value, SlotValue::Empty));
        assert_eq!(slot.seq(), start + 2);
    }
}
