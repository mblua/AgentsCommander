use super::*;
use crate::cleanup_diagnostics::schema::{ProcessIdentity, Provenance};
use std::sync::Arc;

#[derive(Default)]
struct Recording {
    records: Vec<EvidenceRecord>,
    syncs: usize,
    completed: bool,
    fail_event: Option<&'static str>,
    fail_sync: Option<usize>,
}

struct Writer(Arc<Mutex<Recording>>);
impl EvidenceWriter for Writer {
    fn write(&mut self, record: &EvidenceRecord) -> Result<(), EvidenceError> {
        let mut state = self.0.lock().unwrap();
        let name = match record.event {
            EvidenceEvent::Target { .. } => "target",
            EvidenceEvent::PreCall { .. } => "pre_call",
            EvidenceEvent::ApiReturn { .. } => "api_return",
            EvidenceEvent::ExitObserved { .. } => "exit_observed",
            EvidenceEvent::OperationBegin => "begin",
            EvidenceEvent::SelectionComplete { .. } => "selection",
            EvidenceEvent::OperationComplete { .. } => "footer",
        };
        if state.fail_event == Some(name) {
            return Err(EvidenceError::new("write", "fake writer failed"));
        }
        state.records.push(record.clone());
        Ok(())
    }
    fn flush_sync(&mut self) -> Result<(), EvidenceError> {
        let mut state = self.0.lock().unwrap();
        state.syncs += 1;
        if state.fail_sync == Some(state.syncs) {
            return Err(EvidenceError::new("sync", "fake sync failed"));
        }
        Ok(())
    }
    fn complete(&mut self) -> Result<(), EvidenceError> {
        self.0.lock().unwrap().completed = true;
        Ok(())
    }
}

fn operation() -> (Operation, Arc<Mutex<Recording>>) {
    let records = Arc::new(Mutex::new(Recording::default()));
    let operation = Operation::new(Uuid::new_v4(), Box::new(Writer(Arc::clone(&records)))).unwrap();
    (operation, records)
}

fn row() -> SelectionRow {
    SelectionRow {
        index: 0,
        expected: ProcessIdentity {
            pid: 42,
            start_time: 100,
        },
        retained: None,
        fresh: None,
        provenance: Provenance::Fresh,
        rule: "fresh_wins_depth_desc_identity_desc".into(),
        current: CurrentIdentity::unknown(),
        matches_app: None,
        matches_other_roots: Vec::new(),
    }
}

fn summary() -> Summary {
    Summary {
        partial: false,
        evidence_gap: false,
        killed_processes: 0,
        accounting: "proposed".into(),
        errors: Vec::new(),
    }
}

fn prepare(operation: &Operation) -> CallTicket {
    operation
        .prepare(
            &row(),
            CurrentIdentity::unknown(),
            CurrentIdentity::unknown(),
        )
        .unwrap()
}

fn raw() -> RawApiResult {
    RawApiResult {
        returned: Some(true),
        os_error: None,
        detail: "fake".into(),
    }
}

#[test]
fn issue_3530_writer_fails_selection() {
    let (operation, records) = operation();
    records.lock().unwrap().fail_event = Some("target");
    let error = operation.selection(&[row()]).unwrap_err();
    assert_eq!(error.stage, "selection");
    assert!(!error.effects_started);
    assert!(operation
        .prepare(
            &row(),
            CurrentIdentity::unknown(),
            CurrentIdentity::unknown()
        )
        .is_err());
    assert!(!records.lock().unwrap().completed);
}

#[test]
fn issue_3530_writer_fails_sync() {
    let (operation, records) = operation();
    records.lock().unwrap().fail_sync = Some(2);
    let error = operation.selection(&[row()]).unwrap_err();
    assert_eq!(error.stage, "selection");
    assert!(!error.effects_started);
    assert!(operation
        .prepare(
            &row(),
            CurrentIdentity::unknown(),
            CurrentIdentity::unknown()
        )
        .is_err());
}

#[test]
fn issue_3530_ticket_correlation_and_single_consumption() {
    let (first, _) = operation();
    let (second, _) = operation();
    first.selection(&[row()]).unwrap();
    second.selection(&[row()]).unwrap();
    let ticket = prepare(&first);
    assert!(second.returned(&ticket, raw()).is_err());
    assert!(first.observed(&ticket, ExitObservation::Unknown).is_err());
    first.returned(&ticket, raw()).unwrap();
    assert!(first.returned(&ticket, raw()).is_err());
    first.observed(&ticket, ExitObservation::Timeout).unwrap();
    assert!(first.observed(&ticket, ExitObservation::Timeout).is_err());
}

#[test]
fn issue_3530_return_failure_keeps_known_facts_and_blocks_next_call() {
    let (operation, records) = operation();
    let mut second = row();
    second.index = 1;
    operation.selection(&[row(), second.clone()]).unwrap();
    let ticket = prepare(&operation);
    records.lock().unwrap().fail_event = Some("api_return");
    let error = operation.returned(&ticket, raw()).unwrap_err();
    assert!(error.effects_started);
    assert_eq!(error.known_api_return, Some(raw()));
    assert!(operation
        .prepare(
            &second,
            CurrentIdentity::unknown(),
            CurrentIdentity::unknown()
        )
        .is_err());
}

#[test]
fn issue_3530_footer_sync_precedes_completion() {
    let (operation, records) = operation();
    operation.selection(&[]).unwrap();
    records.lock().unwrap().fail_sync = Some(3);
    assert!(operation.finish(summary()).is_err());
    assert!(!records.lock().unwrap().completed);
    let (operation, records) = self::operation();
    operation.selection(&[]).unwrap();
    operation.finish(summary()).unwrap();
    let state = records.lock().unwrap();
    assert!(state.completed);
    assert_eq!(state.syncs, 3);
    assert!(state
        .records
        .windows(2)
        .all(|pair| pair[0].seq < pair[1].seq));
}

#[test]
fn issue_3530_abandon_without_footer() {
    let (operation, records) = operation();
    operation.selection(&[row()]).unwrap();
    let _ticket = prepare(&operation);
    drop(operation);
    let state = records.lock().unwrap();
    assert!(!state.completed);
    assert!(!state
        .records
        .iter()
        .any(|record| { matches!(record.event, EvidenceEvent::OperationComplete { .. }) }));
}
