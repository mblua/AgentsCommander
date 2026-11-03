//! Sole owner of diagnostic JSONL, quotas, ephemeral arms and request receipts.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::operation::{EvidenceWriter, Operation};
use super::schema::{
    Action, Binding, EvidenceError, EvidenceEvent, EvidenceRecord, Receipt, COVERAGE, EXCLUDED,
};
use crate::filesystem::debug_logs;

const OP_BYTES: u64 = 8 * 1024 * 1024;
const INSTANCE_BYTES: u64 = 64 * 1024 * 1024;
const COMPLETED_LIMIT: usize = 16;
const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const LIFETIME: Duration = Duration::from_secs(10 * 60);
const RECEIPT_LIMIT: usize = 256;

pub(crate) trait Clock: Send + Sync {
    fn utc(&self) -> DateTime<Utc>;
    fn elapsed(&self) -> Duration;
}

struct SystemClock(Instant);
impl Clock for SystemClock {
    fn utc(&self) -> DateTime<Utc> {
        Utc::now()
    }
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
}

#[derive(Serialize, Deserialize)]
struct Line {
    version: u8,
    run_id: Uuid,
    operation_id: Uuid,
    binding: Binding,
    reason: String,
    coverage: String,
    excluded: Vec<String>,
    utc: DateTime<Utc>,
    elapsed_ms: u128,
    #[serde(flatten)]
    record: EvidenceRecord,
}

struct FileEntry {
    path: PathBuf,
    bytes: u64,
    complete: bool,
    created: DateTime<Utc>,
    active: bool,
}

struct Arm {
    operation_id: Uuid,
    binding: Binding,
    admitted: Duration,
}

struct RequestEntry {
    action: Action,
    binding: Binding,
    admitted: Duration,
    receipt: Option<Receipt>,
}

#[derive(Default)]
struct Metadata {
    files: HashMap<Uuid, FileEntry>,
    arms: HashMap<Uuid, Arm>,
    requests: HashMap<(String, Uuid), RequestEntry>,
}

struct Shared {
    metadata: Mutex<Metadata>,
    clock: Arc<dyn Clock>,
}

/// Non-cloneable reservation. Release rejected/stale reservations outside product locks.
pub(crate) struct Reservation {
    id: Option<Uuid>,
    owner: Arc<Shared>,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            if let Ok(mut metadata) = self.owner.metadata.lock() {
                if metadata.files.get(&id).is_some_and(|entry| !entry.active) {
                    metadata.files.remove(&id);
                }
            }
        }
    }
}

/// Consumed once by the accepted individual cleanup, never shared by session lookup.
pub(crate) struct ArmClaim {
    transferred: bool,
    owner: Arc<Shared>,
    operation_id: Uuid,
    binding: Binding,
    reason: String,
}

impl Drop for ArmClaim {
    fn drop(&mut self) {
        if self.transferred {
            return;
        }
        if let Ok(mut metadata) = self.owner.metadata.lock() {
            if metadata
                .files
                .get(&self.operation_id)
                .is_some_and(|entry| !entry.active)
            {
                metadata.files.remove(&self.operation_id);
            }
        }
    }
}

impl ArmClaim {
    pub(crate) fn operation_id(&self) -> Uuid {
        self.operation_id
    }

    pub(crate) fn with_reason(mut self, reason: &str) -> Self {
        self.reason = reason.into();
        self
    }
}

pub(crate) enum ArmStatus {
    Armed { operation_id: Uuid },
    AlreadyArmed { operation_id: Uuid },
}

pub(crate) enum RequestAdmission {
    New,
    Pending,
    Cached(Receipt),
}

#[cfg(test)]
type TestWriterFactory = Arc<dyn Fn() -> Box<dyn EvidenceWriter> + Send + Sync>;

pub(crate) struct Diagnostics {
    directory: Option<PathBuf>,
    run_id: Uuid,
    shared: Arc<Shared>,
    #[cfg(test)]
    test_writer: Option<TestWriterFactory>,
}

impl Shared {
    /// Serializes admission and pruning under only the store's private metadata lock.
    fn prune(&self, metadata: &mut Metadata, reserve: u64) -> Result<(), EvidenceError> {
        let now = self.clock.utc();
        let mut completed: Vec<_> = metadata
            .files
            .iter()
            .filter(|(_, entry)| entry.complete && !entry.active)
            .map(|(id, entry)| (*id, entry.created))
            .collect();
        completed.sort_by_key(|(id, created)| (*created, *id));
        let mut count = completed.len();
        let mut total: u64 = metadata.files.values().map(|entry| entry.bytes).sum();
        for (id, created) in completed {
            let old = now
                .signed_duration_since(created)
                .to_std()
                .is_ok_and(|age| age >= RETENTION);
            if !old && count <= COMPLETED_LIMIT && total.saturating_add(reserve) <= INSTANCE_BYTES {
                continue;
            }
            let entry = &metadata.files[&id];
            // Paths enter this table only through a validated name or our own factory.
            debug_logs::remove_app_log(&entry.path)
                .map_err(|error| EvidenceError::new("prune", error))?;
            total = total.saturating_sub(entry.bytes);
            count -= 1;
            metadata.files.remove(&id);
        }
        if total.saturating_add(reserve) > INSTANCE_BYTES {
            return Err(EvidenceError::new(
                "quota",
                "incomplete/active files exhaust instance budget",
            ));
        }
        Ok(())
    }
}

impl Diagnostics {
    pub(crate) fn disabled() -> Self {
        Self::empty(None, Arc::new(SystemClock(Instant::now())))
    }

    fn empty(directory: Option<PathBuf>, clock: Arc<dyn Clock>) -> Self {
        Self {
            directory,
            run_id: Uuid::new_v4(),
            shared: Arc::new(Shared {
                metadata: Mutex::new(Metadata::default()),
                clock,
            }),
            #[cfg(test)]
            test_writer: None,
        }
    }

    pub(crate) fn new(config_dir: &Path) -> Result<Self, EvidenceError> {
        let diagnostics = Self::empty(
            Some(config_dir.join("pretermination-diagnostics")),
            Arc::new(SystemClock(Instant::now())),
        );
        diagnostics.load()?;
        Ok(diagnostics)
    }

    pub(crate) fn is_available(&self) -> bool {
        self.directory.is_some()
    }

    pub(crate) fn operation_path(&self, operation_id: Uuid) -> Option<String> {
        self.filename(operation_id)
            .ok()
            .map(|path| path.to_string_lossy().into_owned())
    }

    pub(crate) fn run_id(&self) -> Uuid {
        self.run_id
    }

    fn metadata(&self) -> Result<MutexGuard<'_, Metadata>, EvidenceError> {
        self.shared
            .metadata
            .lock()
            .map_err(|error| EvidenceError::new("metadata", error))
    }

    fn directory(&self) -> Result<&Path, EvidenceError> {
        self.directory
            .as_deref()
            .ok_or_else(|| EvidenceError::new("open", "not_available"))
    }

    fn filename(&self, operation_id: Uuid) -> Result<PathBuf, EvidenceError> {
        Ok(self
            .directory()?
            .join(format!("{}-{operation_id}.jsonl", self.run_id)))
    }

    fn load(&self) -> Result<(), EvidenceError> {
        let directory = self.directory()?;
        debug_logs::ensure_app_log_directory(directory)
            .map_err(|error| EvidenceError::new("directory", error))?;
        let mut metadata = self.metadata()?;
        for path in debug_logs::list_diagnostic_files(directory)
            .map_err(|error| EvidenceError::new("list", error))?
        {
            let Some((run, operation)) = owned_name(&path) else {
                continue;
            };
            let info = debug_logs::diagnostic_file_metadata(&path)
                .map_err(|error| EvidenceError::new("metadata", error))?;
            if !info.file_type().is_file() {
                continue;
            }
            let bytes = info.len();
            let mut complete = false;
            let mut created = info
                .modified()
                .map(DateTime::<Utc>::from)
                .map_err(|error| EvidenceError::new("metadata", error))?;
            if bytes <= OP_BYTES {
                let content = debug_logs::read_diagnostic_file(&path)
                    .map_err(|error| EvidenceError::new("read", error))?;
                let parsed = parse_complete(&content, run, operation);
                if let Some(date) = parsed {
                    complete = true;
                    created = date;
                }
            }
            if metadata.files.contains_key(&operation) {
                return Err(EvidenceError::new("load", "duplicate operation UUID"));
            }
            metadata.files.insert(
                operation,
                FileEntry {
                    path,
                    bytes,
                    complete,
                    created,
                    active: false,
                },
            );
        }
        self.shared.prune(&mut metadata, 0)
    }

    pub(crate) fn reserve(&self) -> Result<Reservation, EvidenceError> {
        self.directory()?;
        let mut metadata = self.metadata()?;
        self.expire_arms(&mut metadata);
        self.shared.prune(&mut metadata, OP_BYTES)?;
        let id = Uuid::new_v4();
        metadata.files.insert(
            id,
            FileEntry {
                path: self.filename(id)?,
                bytes: OP_BYTES,
                complete: false,
                created: self.shared.clock.utc(),
                active: false,
            },
        );
        Ok(Reservation {
            id: Some(id),
            owner: Arc::clone(&self.shared),
        })
    }

    fn expire_arms(&self, metadata: &mut Metadata) {
        let now = self.shared.clock.elapsed();
        let expired: Vec<_> = metadata
            .arms
            .iter()
            .filter(|(_, arm)| now.saturating_sub(arm.admitted) >= LIFETIME)
            .map(|(session, _)| *session)
            .collect();
        for session in expired {
            if let Some(arm) = metadata.arms.remove(&session) {
                metadata.files.remove(&arm.operation_id);
            }
        }
    }

    /// Memory only. Caller revalidates live/public/local/state + binding under inner.
    pub(crate) fn publish_arm_metadata(
        &self,
        mut reservation: Reservation,
        binding: Binding,
    ) -> Result<ArmStatus, EvidenceError> {
        if binding.run_id != self.run_id
            || !binding
                .root
                .is_some_and(|root| root.pid != 0 && root.start_time != 0)
            || !Arc::ptr_eq(&reservation.owner, &self.shared)
        {
            return Err(EvidenceError::new("arm", "stale binding/reservation"));
        }
        let mut metadata = self.metadata()?;
        self.expire_arms(&mut metadata);
        if let Some(arm) = metadata.arms.get(&binding.session_id) {
            if arm.binding == binding {
                return Ok(ArmStatus::AlreadyArmed {
                    operation_id: arm.operation_id,
                });
            }
        }
        if let Some(old) = metadata.arms.remove(&binding.session_id) {
            metadata.files.remove(&old.operation_id);
        }
        let id = reservation
            .id
            .take()
            .ok_or_else(|| EvidenceError::new("arm", "reservation already consumed"))?;
        metadata.arms.insert(
            binding.session_id,
            Arm {
                operation_id: id,
                binding,
                admitted: self.shared.clock.elapsed(),
            },
        );
        Ok(ArmStatus::Armed { operation_id: id })
    }

    /// Query before reserving: a second active arm needs no additional IO or quota.
    pub(crate) fn arm_status_metadata(&self, binding: &Binding) -> Option<ArmStatus> {
        let mut metadata = self.shared.metadata.lock().ok()?;
        self.expire_arms(&mut metadata);
        let arm = metadata.arms.get(&binding.session_id)?;
        (arm.binding == *binding).then_some(ArmStatus::AlreadyArmed {
            operation_id: arm.operation_id,
        })
    }

    /// Memory only: called in the same inner lock that admits Terminating.
    pub(crate) fn take_arm_metadata(&self, binding: &Binding) -> Option<ArmClaim> {
        let mut metadata = self.shared.metadata.lock().ok()?;
        self.expire_arms(&mut metadata);
        let arm = metadata.arms.remove(&binding.session_id)?;
        if arm.binding == *binding {
            Some(ArmClaim {
                transferred: false,
                owner: Arc::clone(&self.shared),
                operation_id: arm.operation_id,
                binding: arm.binding,
                reason: "individual_cleanup".into(),
            })
        } else {
            metadata.files.remove(&arm.operation_id);
            None
        }
    }

    /// Inspect reserves its own operation and never touches the arm table.
    pub(crate) fn inspect_operation(&self, binding: Binding) -> Result<Operation, EvidenceError> {
        if binding.run_id != self.run_id {
            return Err(EvidenceError::new("inspect", "stale run"));
        }
        let mut reservation = self.reserve()?;
        let operation_id = reservation
            .id
            .take()
            .ok_or_else(|| EvidenceError::new("inspect", "missing reservation"))?;
        self.open_operation(ArmClaim {
            transferred: false,
            owner: Arc::clone(&self.shared),
            operation_id,
            binding,
            reason: "inspect".into(),
        })
    }

    pub(crate) fn open_operation(&self, mut claim: ArmClaim) -> Result<Operation, EvidenceError> {
        if claim.binding.run_id != self.run_id || !Arc::ptr_eq(&claim.owner, &self.shared) {
            return Err(EvidenceError::new("open", "stale run"));
        }
        let path = self.filename(claim.operation_id)?;
        {
            let mut metadata = self.metadata()?;
            let entry = metadata
                .files
                .get_mut(&claim.operation_id)
                .ok_or_else(|| EvidenceError::new("open", "missing reservation"))?;
            if entry.active {
                return Err(EvidenceError::new("open", "reservation already open"));
            }
            entry.active = true;
        }
        let file = match debug_logs::create_app_log(&path) {
            Ok(file) => file,
            Err(error) => {
                if let Ok(mut metadata) = self.shared.metadata.lock() {
                    // Keep the reservation on an unexpected collision; do not erase that file.
                    if error.kind() != std::io::ErrorKind::AlreadyExists {
                        metadata.files.remove(&claim.operation_id);
                    }
                }
                return Err(EvidenceError::new("open", error));
            }
        };
        claim.transferred = true;
        let writer = FileWriter {
            file,
            shared: Arc::clone(&self.shared),
            run_id: self.run_id,
            operation_id: claim.operation_id,
            binding: claim.binding.clone(),
            reason: claim.reason.clone(),
            started: self.shared.clock.elapsed(),
            bytes: 0,
            complete: false,
            #[cfg(test)]
            test_writer: self.test_writer.as_ref().map(|factory| factory()),
        };
        Operation::new(claim.operation_id, Box::new(writer))
    }

    pub(crate) fn admit_request(
        &self,
        principal: &str,
        request_id: Uuid,
        action: Action,
        binding: &Binding,
    ) -> Result<RequestAdmission, EvidenceError> {
        let mut metadata = self.metadata()?;
        let key = (principal.to_owned(), request_id);
        let now = self.shared.clock.elapsed();
        if let Some(entry) = metadata.requests.get(&key) {
            if entry.action != action || entry.binding != *binding {
                return Err(EvidenceError::new("request", "conflict"));
            }
            if now.saturating_sub(entry.admitted) >= LIFETIME {
                return Err(EvidenceError::new("request", "expired"));
            }
            return Ok(match &entry.receipt {
                Some(receipt) => RequestAdmission::Cached(receipt.clone()),
                None => RequestAdmission::Pending,
            });
        }
        // Never evict in-flight work, even after its admission deadline.
        metadata.requests.retain(|_, entry| {
            entry.receipt.is_none() || now.saturating_sub(entry.admitted) < LIFETIME
        });
        if metadata.requests.len() >= RECEIPT_LIMIT {
            return Err(EvidenceError::new(
                "quota",
                "request receipt slots exhausted",
            ));
        }
        metadata.requests.insert(
            key,
            RequestEntry {
                action,
                binding: binding.clone(),
                admitted: now,
                receipt: None,
            },
        );
        Ok(RequestAdmission::New)
    }

    pub(crate) fn cache_receipt(
        &self,
        principal: &str,
        request_id: Uuid,
        receipt: Receipt,
    ) -> Result<(), EvidenceError> {
        let mut metadata = self.metadata()?;
        let entry = metadata
            .requests
            .get_mut(&(principal.to_owned(), request_id))
            .ok_or_else(|| EvidenceError::new("receipt", "request not admitted"))?;
        if let Some(previous) = &entry.receipt {
            if previous != &receipt {
                return Err(EvidenceError::new("receipt", "conflict"));
            }
        } else {
            entry.receipt = Some(receipt);
        }
        Ok(())
    }

    /// Test-only ports shared with P02/P04; no production arming endpoint.
    #[cfg(test)]
    pub(crate) fn with_test_ports(
        config_dir: &Path,
        clock: Arc<dyn Clock>,
        writer: TestWriterFactory,
    ) -> Result<Self, EvidenceError> {
        let mut diagnostics =
            Self::empty(Some(config_dir.join("pretermination-diagnostics")), clock);
        diagnostics.test_writer = Some(writer);
        diagnostics.load()?;
        Ok(diagnostics)
    }
}

/// Recognize only the exact UUID-UUID.jsonl namespace owned by this store.
fn owned_name(path: &Path) -> Option<(Uuid, Uuid)> {
    let name = path.file_name()?.to_str()?;
    if name.len() != 79 || !name.ends_with(".jsonl") || name.as_bytes()[36] != b'-' {
        return None;
    }
    let run = Uuid::parse_str(name.get(..36)?).ok()?;
    let operation = Uuid::parse_str(name.get(37..73)?).ok()?;
    (name == format!("{run}-{operation}.jsonl")).then_some((run, operation))
}

fn parse_complete(content: &[u8], run: Uuid, operation: Uuid) -> Option<DateTime<Utc>> {
    if content.last() != Some(&b'\n') {
        return None;
    }
    let mut last = None;
    let mut created = None;
    for (index, bytes) in content[..content.len() - 1]
        .split(|byte| *byte == b'\n')
        .enumerate()
    {
        let line: Line = serde_json::from_slice(bytes).ok()?;
        if line.version != 1
            || line.run_id != run
            || line.operation_id != operation
            || line.record.seq != u64::try_from(index).ok()?.checked_add(1)?
            || line.binding.run_id != run
            || line.coverage != COVERAGE
        {
            return None;
        }
        if index == 0 {
            if !matches!(line.record.event, EvidenceEvent::OperationBegin) {
                return None;
            }
            created = Some(line.utc);
        }
        if matches!(last, Some(EvidenceEvent::OperationComplete { .. })) {
            return None;
        }
        last = Some(line.record.event);
    }
    matches!(last, Some(EvidenceEvent::OperationComplete { .. })).then_some(created?)
}

struct FileWriter {
    file: File,
    shared: Arc<Shared>,
    run_id: Uuid,
    operation_id: Uuid,
    binding: Binding,
    reason: String,
    started: Duration,
    bytes: u64,
    complete: bool,
    #[cfg(test)]
    test_writer: Option<Box<dyn EvidenceWriter>>,
}

impl FileWriter {
    fn account(&self, complete: bool) -> Result<(), EvidenceError> {
        let mut metadata = self
            .shared
            .metadata
            .lock()
            .map_err(|error| EvidenceError::new("accounting", error))?;
        let entry = metadata
            .files
            .get_mut(&self.operation_id)
            .ok_or_else(|| EvidenceError::new("accounting", "missing operation"))?;
        // metadata.len also counts any prefix written before write_all failed.
        entry.bytes = debug_logs::opened_app_log_len(&self.file)
            .map_err(|error| EvidenceError::new("accounting", error))?;
        entry.complete = complete;
        entry.active = false;
        Ok(())
    }
}

impl EvidenceWriter for FileWriter {
    fn write(&mut self, record: &EvidenceRecord) -> Result<(), EvidenceError> {
        #[cfg(test)]
        if let Some(writer) = &mut self.test_writer {
            writer.write(record)?;
        }
        let line = Line {
            version: 1,
            run_id: self.run_id,
            operation_id: self.operation_id,
            binding: self.binding.clone(),
            reason: self.reason.clone(),
            coverage: COVERAGE.into(),
            excluded: EXCLUDED.iter().map(|value| (*value).into()).collect(),
            utc: self.shared.clock.utc(),
            elapsed_ms: self
                .shared
                .clock
                .elapsed()
                .saturating_sub(self.started)
                .as_millis(),
            record: record.clone(),
        };
        let mut bytes =
            serde_json::to_vec(&line).map_err(|error| EvidenceError::new("serialize", error))?;
        bytes.push(b'\n');
        let length =
            u64::try_from(bytes.len()).map_err(|error| EvidenceError::new("quota", error))?;
        if self.bytes.saturating_add(length) > OP_BYTES {
            return Err(EvidenceError::new(
                "quota",
                "operation byte budget exhausted",
            ));
        }
        debug_logs::append_app_log(&mut self.file, &bytes)
            .map_err(|error| EvidenceError::new("write", error))?;
        self.bytes += length;
        Ok(())
    }

    fn flush_sync(&mut self) -> Result<(), EvidenceError> {
        #[cfg(test)]
        if let Some(writer) = &mut self.test_writer {
            writer.flush_sync()?;
        }
        debug_logs::flush_diagnostic_file(&mut self.file)
            .and_then(|()| debug_logs::sync_diagnostic_file(&self.file))
            .map_err(|error| EvidenceError::new("sync", error))
    }

    fn complete(&mut self) -> Result<(), EvidenceError> {
        #[cfg(test)]
        if let Some(writer) = &mut self.test_writer {
            writer.complete()?;
        }
        self.account(true)?;
        {
            let mut metadata = self
                .shared
                .metadata
                .lock()
                .map_err(|error| EvidenceError::new("accounting", error))?;
            self.shared.prune(&mut metadata, 0)?;
        }
        self.complete = true;
        Ok(())
    }
}

impl Drop for FileWriter {
    fn drop(&mut self) {
        if !self.complete {
            // Never emit a footer on drop/unwind. Keep the incomplete file charged.
            let _ = self.account(false);
        }
    }
}

#[cfg(test)]
mod tests;
