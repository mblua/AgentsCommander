use super::*;
use crate::cleanup_diagnostics::schema::{ProcessIdentity, Summary};
use std::sync::atomic::{AtomicU64, Ordering};

struct FakeClock(AtomicU64);
impl Clock for FakeClock {
    fn utc(&self) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + self.0.load(Ordering::SeqCst) as i64, 0).unwrap()
    }
    fn elapsed(&self) -> Duration {
        Duration::from_secs(self.0.load(Ordering::SeqCst))
    }
}

struct PassWriter;
impl EvidenceWriter for PassWriter {
    fn write(&mut self, _: &EvidenceRecord) -> Result<(), EvidenceError> {
        Ok(())
    }
    fn flush_sync(&mut self) -> Result<(), EvidenceError> {
        Ok(())
    }
    fn complete(&mut self) -> Result<(), EvidenceError> {
        Ok(())
    }
}

fn store() -> (tempfile::TempDir, Arc<Diagnostics>, Arc<FakeClock>) {
    let directory = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock(AtomicU64::new(0)));
    let diagnostics = Diagnostics::with_test_ports(
        directory.path(),
        clock.clone(),
        Arc::new(|| Box::new(PassWriter)),
    )
    .unwrap();
    (directory, Arc::new(diagnostics), clock)
}

fn binding(diagnostics: &Diagnostics) -> Binding {
    Binding {
        run_id: diagnostics.run_id(),
        session_id: Uuid::new_v4(),
        root: Some(ProcessIdentity {
            pid: 42,
            start_time: 17,
        }),
        registration_generation: 1,
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

fn complete(diagnostics: &Diagnostics) -> PathBuf {
    let operation = diagnostics.inspect_operation(binding(diagnostics)).unwrap();
    let path = diagnostics.filename(operation.id()).unwrap();
    operation.selection(&[]).unwrap();
    operation.finish(summary()).unwrap();
    drop(operation);
    path
}

#[test]
fn issue_3530_quota_concurrent_reservations() {
    let (_directory, diagnostics, _) = store();
    let threads: Vec<_> = (0..16)
        .map(|_| {
            let diagnostics = Arc::clone(&diagnostics);
            std::thread::spawn(move || diagnostics.reserve())
        })
        .collect();
    let reservations: Vec<_> = threads
        .into_iter()
        .filter_map(|thread| thread.join().unwrap().ok())
        .collect();
    assert_eq!(reservations.len(), 8);
    assert_eq!(
        diagnostics
            .metadata()
            .unwrap()
            .files
            .values()
            .map(|entry| entry.bytes)
            .sum::<u64>(),
        INSTANCE_BYTES
    );
    assert_eq!(diagnostics.reserve().err().unwrap().stage, "quota");
    drop(reservations);
    assert!(diagnostics.metadata().unwrap().files.is_empty());
}

#[test]
fn issue_3530_preserve_incomplete() {
    let (directory, diagnostics, _) = store();
    let operation = diagnostics
        .inspect_operation(binding(&diagnostics))
        .unwrap();
    let path = diagnostics.filename(operation.id()).unwrap();
    operation.selection(&[]).unwrap();
    drop(operation);
    let length = std::fs::metadata(&path).unwrap().len();
    assert!(length > 0 && length < OP_BYTES);
    assert_eq!(
        diagnostics
            .metadata()
            .unwrap()
            .files
            .values()
            .next()
            .unwrap()
            .bytes,
        length
    );
    drop(diagnostics);
    let restarted = Diagnostics::new(directory.path()).unwrap();
    let metadata = restarted.metadata().unwrap();
    assert_eq!(metadata.files.len(), 1);
    assert!(!metadata.files.values().next().unwrap().complete);
    assert!(path.exists());
}

#[test]
fn issue_3530_prune_completed_only() {
    let (directory, diagnostics, clock) = store();
    let completed = complete(&diagnostics);
    let operation = diagnostics
        .inspect_operation(binding(&diagnostics))
        .unwrap();
    let incomplete = diagnostics.filename(operation.id()).unwrap();
    drop(operation);
    let foreign = diagnostics.directory().unwrap().join("app.log");
    std::fs::write(&foreign, b"foreign").unwrap();
    clock.0.store(RETENTION.as_secs() + 1, Ordering::SeqCst);
    let reservation = diagnostics.reserve().unwrap();
    assert!(!completed.exists());
    assert!(incomplete.exists());
    assert_eq!(std::fs::read(&foreign).unwrap(), b"foreign");
    drop(reservation);
    drop(diagnostics);
    let restarted = Diagnostics::new(directory.path()).unwrap();
    assert_eq!(restarted.metadata().unwrap().files.len(), 1);
}

#[test]
fn issue_3530_completed_count_and_escaped_bytes() {
    let (_directory, diagnostics, clock) = store();
    for _ in 0..20 {
        clock.0.fetch_add(1, Ordering::SeqCst);
        complete(&diagnostics);
    }
    let metadata = diagnostics.metadata().unwrap();
    assert!(metadata.files.len() <= COMPLETED_LIMIT);
    for entry in metadata.files.values() {
        assert!(entry.complete);
        assert_eq!(entry.bytes, std::fs::metadata(&entry.path).unwrap().len());
    }
    drop(metadata);
    let operation = diagnostics
        .inspect_operation(binding(&diagnostics))
        .unwrap();
    operation.selection(&[]).unwrap();
    let mut footer = summary();
    footer.errors.push("quote\" newline\n slash\\".into());
    operation.finish(footer).unwrap();
    let path = diagnostics.filename(operation.id()).unwrap();
    let bytes = std::fs::read(path).unwrap();
    assert!(parse_complete(&bytes, diagnostics.run_id(), operation.id()).is_some());
}

#[test]
fn issue_3530_arm_once_and_expire() {
    let (_directory, diagnostics, clock) = store();
    let binding = binding(&diagnostics);
    let status = diagnostics.publish_arm_metadata(diagnostics.reserve().unwrap(), binding.clone());
    let ArmStatus::Armed { operation_id } = status.unwrap() else {
        panic!("not armed")
    };
    let status = diagnostics.publish_arm_metadata(diagnostics.reserve().unwrap(), binding.clone());
    let ArmStatus::AlreadyArmed { operation_id: same } = status.unwrap() else {
        panic!("duplicate arm")
    };
    assert_eq!(operation_id, same);
    assert_eq!(diagnostics.metadata().unwrap().files.len(), 1);
    let inspect = diagnostics.inspect_operation(binding.clone()).unwrap();
    drop(inspect);
    assert!(diagnostics.take_arm_metadata(&binding).is_some());
    assert!(diagnostics.take_arm_metadata(&binding).is_none());
    diagnostics
        .publish_arm_metadata(diagnostics.reserve().unwrap(), binding.clone())
        .unwrap();
    clock.0.store(LIFETIME.as_secs(), Ordering::SeqCst);
    assert!(diagnostics.take_arm_metadata(&binding).is_none());
    diagnostics
        .publish_arm_metadata(diagnostics.reserve().unwrap(), binding.clone())
        .unwrap();
    let mut replacement = binding.clone();
    replacement.registration_generation += 1;
    assert!(diagnostics.take_arm_metadata(&replacement).is_none());
    assert!(diagnostics.take_arm_metadata(&binding).is_none());
}

#[test]
fn issue_3530_request_conflict() {
    let (_directory, diagnostics, _) = store();
    let binding = binding(&diagnostics);
    let request = Uuid::new_v4();
    assert!(matches!(
        diagnostics.admit_request("principal", request, Action::Inspect, &binding),
        Ok(RequestAdmission::New)
    ));
    assert!(matches!(
        diagnostics.admit_request("principal", request, Action::Inspect, &binding),
        Ok(RequestAdmission::Pending)
    ));
    assert!(diagnostics
        .admit_request("principal", request, Action::ArmNextCleanup, &binding)
        .is_err());
    let receipt = Receipt {
        status: "rejected".into(),
        operation_id: None,
        path: None,
        evidence_gap: true,
        errors: vec!["fake failure".into()],
    };
    diagnostics
        .cache_receipt("principal", request, receipt.clone())
        .unwrap();
    let cached = diagnostics
        .admit_request("principal", request, Action::Inspect, &binding)
        .unwrap();
    let RequestAdmission::Cached(cached) = cached else {
        panic!("not cached")
    };
    assert_eq!(cached, receipt);
    assert!(diagnostics.metadata().unwrap().files.is_empty());
}

#[test]
fn issue_3530_active_requests_are_never_evicted() {
    let (_directory, diagnostics, clock) = store();
    let binding = binding(&diagnostics);
    for _ in 0..RECEIPT_LIMIT {
        diagnostics
            .admit_request("p", Uuid::new_v4(), Action::Inspect, &binding)
            .unwrap();
    }
    clock.0.store(LIFETIME.as_secs() + 1, Ordering::SeqCst);
    let error = diagnostics
        .admit_request("p", Uuid::new_v4(), Action::Inspect, &binding)
        .err()
        .unwrap();
    assert_eq!(error.stage, "quota");
    assert_eq!(
        diagnostics.metadata().unwrap().requests.len(),
        RECEIPT_LIMIT
    );
}

#[test]
fn issue_3530_operation_quota_counts_escaped_json_and_no_truncated_complete() {
    let (_directory, diagnostics, _) = store();
    let operation = diagnostics
        .inspect_operation(binding(&diagnostics))
        .unwrap();
    operation.selection(&[]).unwrap();
    let mut footer = summary();
    footer
        .errors
        .push("\n".repeat(usize::try_from(OP_BYTES / 2).unwrap()));
    assert!(operation.finish(footer).is_err());
    let id = operation.id();
    drop(operation);
    let metadata = diagnostics.metadata().unwrap();
    let entry = &metadata.files[&id];
    assert!(!entry.complete);
    assert!(entry.bytes <= OP_BYTES);
    let bytes = std::fs::read(&entry.path).unwrap();
    assert!(parse_complete(&bytes, diagnostics.run_id(), id).is_none());
}
