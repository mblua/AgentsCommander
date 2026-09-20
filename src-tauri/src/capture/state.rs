//! Per-room capture state: epochs, watermark, recent set, cut and budget, plus
//! the single synchronous effect commit (#2232 phase 3).
//!
//! Leaf module. It names `capture::key`, `capture::record`, `config::co_managed`
//! (for the room's `.co-managed` directory and its advisory lock path), serde
//! and std. It never names the phone, session or commands subtrees, nor either
//! config module named in section 4, which is what keeps [`commit_effect`]
//! callable from the phase-7 supervisor without dragging the 88-module SCC in.
//!
//! Two properties are load-bearing and are pinned by tests:
//!
//! * [`commit_effect`] is **synchronous** and holds **no `await`**. Its caller
//!   runs it inside `tokio::task::spawn_blocking`, never `block_in_place`, so a
//!   100 ms lock wait cannot park a Tokio worker.
//! * [`LOCK_WAIT_BUDGET`] is **strictly below** [`STALE_PRECONDITIONS`]. If the
//!   lock wait could exhaust the staleness window, every contended call would
//!   return `PreconditionsStale` and the caller's bounded retry would turn the
//!   feature into a silent no-op behind a plausible-looking reason.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::capture::key::{
    classify_observation, extend_observation, ConsumptionKey, Cut, FileObservation,
};
use crate::capture::record::{CaptureProvider, CapturedRecord, RecordOrigin};
use crate::capture::sink::CaptureSlot;
use crate::config::co_managed::{co_managed_dir, lock_path};

/// How old a precondition snapshot may be and still count as a revalidation.
pub const STALE_PRECONDITIONS: Duration = Duration::from_millis(250);

/// How long [`commit_effect`] may wait for the room's advisory lock.
///
/// Strictly below [`STALE_PRECONDITIONS`], so a lock wait can never on its own
/// exhaust the staleness window. This path is **not** governed by the 2 s
/// config-command bound of phase 2, which still applies to `co_managed_get` and
/// `co_managed_set_enabled`.
pub const LOCK_WAIT_BUDGET: Duration = Duration::from_millis(100);

// Acceptance criterion 10: a later edit of either number cannot silently
// re-open the ordering, because this fails the build.
const _: () = assert!(
    LOCK_WAIT_BUDGET.as_millis() < STALE_PRECONDITIONS.as_millis(),
    "LOCK_WAIT_BUDGET must stay strictly below STALE_PRECONDITIONS"
);

const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// The automatic-action budget, initial value and cap, per room.
pub const BUDGET_CAP: u32 = 3;

/// How many consumed keys are remembered per session.
///
/// It exists so a record whose start falls at or below the watermark can still
/// be told apart between "just consumed by this run" and "consumed long ago and
/// pruned". 64 is a bound, not a measurement, and it is not configurable.
pub const RECENT_CAPACITY: usize = 64;

/// `N`: the cap on the observation prefix carried on a `CapturedRecord`.
pub const OBSERVED_PREFIX_CAP: usize = 4096;

/// Minimum interval between two `state.json` writes for one room.
pub const FLUSH_DEBOUNCE: Duration = Duration::from_secs(5);

const STATE_FILE_NAME: &str = "state.json";

/// The head of `bytes`, capped at [`OBSERVED_PREFIX_CAP`].
///
/// The watcher calls this on bytes it has **already** read; it performs no I/O.
pub fn observation_prefix(bytes: &[u8]) -> Vec<u8> {
    bytes[..bytes.len().min(OBSERVED_PREFIX_CAP)].to_vec()
}

/// Rebuild the head of a file from lines the reader already holds, capped at
/// [`OBSERVED_PREFIX_CAP`].
///
/// **This reconstruction is the only permitted source of an observation
/// prefix**, and the pin is not cosmetic. What comes back is each accepted line
/// followed by one `\n`, which is *not* the file's true first bytes: a `\r\n`
/// file, or one whose head the reader never saw, reconstructs differently.
/// Every producer using this one function stays self-consistent, so
/// [`classify_observation`] compares like with like and is correct. The day any
/// producer supplies true file bytes instead, the two prefixes differ over the
/// shared range, the verdict flips to `Replaced`, the epoch advances
/// spuriously, and every watermark for that path is invalidated. Mixing the two
/// sources is therefore forbidden; `prefix_source_is_the_reconstruction_only`
/// is the executable form of this rule.
///
/// Only a read that started at offset 0 carries head evidence, and only the
/// lines that begin inside the cap contribute. This never reads the file again.
pub fn head_from_lines(lines: &[(u64, String)], read_start: u64) -> Vec<u8> {
    if read_start > 0 {
        return Vec::new();
    }
    let mut head = Vec::new();
    for (start, line) in lines {
        if usize::try_from(*start).unwrap_or(usize::MAX) >= OBSERVED_PREFIX_CAP {
            break;
        }
        head.extend_from_slice(line.as_bytes());
        head.push(b'\n');
    }
    observation_prefix(&head)
}

/// Is this record a baseline that is consumed but **never routed** (section 8)?
///
/// Two of section 8's three baselines are properties of the record itself and
/// are decided here:
///
/// 1. **Preamble**: the §J first-attach scan replays what was already on screen
///    before the reader existed.
/// 3. **Rotation backfill**: the Claude reader reads a rotated transcript from
///    zero, so its first sweep is history, not a new turn. **Declared
///    divergence**: Telegram does send that content today, so a legitimate
///    first turn can be suppressed. Deliberate, recorded in `epic.md` 5.1.
///
/// The second baseline, the cut, is a property of a consumer demand rather than
/// of a record, so the sink applies it before the record ever reaches the slot.
pub fn is_baseline(record: &CapturedRecord) -> bool {
    matches!(
        record.origin,
        RecordOrigin::Preamble | RecordOrigin::RotationBackfill
    )
}

/// The authoritative [`ConsumptionKey`] for `record`.
///
/// **Phase 7 must build keys through this function, never from
/// `record.epoch`.** The epoch the watcher stamped on the record is its own
/// reader-local counter, which restarts at zero with the reader; the persisted
/// epoch lives here and survives restarts. A key built from the advisory value
/// would miss the recent set after any restart, and a consumed record would be
/// acted on a second time.
pub fn consumption_key(room_root: &Path, record: &CapturedRecord) -> ConsumptionKey {
    let epoch = observe(
        room_root,
        &record.file,
        FileObservation::new(record.observed_len, record.observed_prefix.clone()),
    );
    ConsumptionKey::from_record(record, epoch)
}

/// What `commit_effect` was asked to do. The text-to-user channel is a real
/// effect that must be marked consumed, but it does not spend budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectKind {
    /// A routed automatic action. Spends one unit of budget.
    Automatic,
    /// Text sent to the user. Marked consumed, budget untouched.
    TextToUser,
}

/// Everything the supervisor resolved asynchronously, frozen at `observed_at`.
///
/// Round 1 required both "revalidate session alive, same id, uniqueness,
/// pending input" and "nothing awaits while the lock is held", which cannot
/// both hold: session state is a `tokio::sync::RwLock`. The contradiction is
/// resolved by moving the async reads **out** of the lock and passing their
/// results **in** as this typed value.
#[derive(Clone, Debug)]
pub struct EffectPreconditions {
    pub session_alive: bool,
    pub session_id: String,
    pub anchor: String,
    pub provider: CaptureProvider,
    pub unique_live_session_for_cwd: bool,
    pub no_pending_user_input: bool,
    pub effective_ready: bool,
    pub observed_at: Instant,
    /// Which channel the supervisor is about to use. Not in round 2's struct;
    /// without it the budget rule "the text-to-user channel does not decrement"
    /// has no input to read.
    pub kind: EffectKind,
}

impl EffectPreconditions {
    fn all_true(&self) -> bool {
        self.session_alive
            && self.unique_live_session_for_cwd
            && self.no_pending_user_input
            && self.effective_ready
    }
}

/// What a successful commit reserved.
///
/// `routable` is the section 8 answer and the supervisor **must** read it: a
/// baseline is consumed so it can never be offered again, but it is never acted
/// on. A commit with `routable == false` spends no budget.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommittedEffect {
    pub session_id: String,
    pub key: ConsumptionKey,
    pub budget_remaining: u32,
    pub kind: EffectKind,
    /// False for a baseline that is consumed but never routed (section 8).
    pub routable: bool,
}

/// Why a commit took nothing.
///
/// `LockBusy` and `PreconditionsStale` are deliberately **distinct**: the two
/// failure modes look identical in a log otherwise, and the retry policy for
/// them is not the same.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AbstainReason {
    /// The snapshot was already older than [`STALE_PRECONDITIONS`], before or
    /// after the lock wait.
    PreconditionsStale,
    /// The advisory lock was not acquired inside [`LOCK_WAIT_BUDGET`].
    LockBusy,
    /// The lock could not be opened at all.
    LockUnavailable(String),
    /// A boolean in the snapshot was false, or the session id moved.
    PreconditionsRejected,
    /// The slot no longer holds the expected sequence, key or a valid value.
    SlotChanged,
    /// This key was already consumed.
    AlreadyConsumed,
    /// No automatic budget left.
    BudgetExhausted,
}

// ---------------------------------------------------------------------------
// In-memory room state
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
struct EpochEntry {
    epoch: u64,
    #[serde(flatten)]
    observation: FileObservation,
}

/// The persisted shape of `state.json`. Every field has a default, so a hand
/// edit or a partial file repairs to a usable state instead of breaking the
/// room, exactly as phase 2's config loader does.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct PersistedRoomState {
    epochs: HashMap<String, EpochEntry>,
    watermarks: HashMap<String, u64>,
    recent: HashMap<String, Vec<ConsumptionKey>>,
    /// The remaining budget, written for a human reading the file. The
    /// authoritative value is [`PersistedRoomState::spends`].
    budget: Option<u32>,
    /// Automatic actions spent since the recharge named by `recharge_stamp`.
    spends_since_recharge: Option<u32>,
    /// Monotonic counter of recharges, used to reconcile an in-memory recharge
    /// against a `state.json` written by another instance.
    recharge_stamp: u64,
    cut: Option<Cut>,
}

impl PersistedRoomState {
    /// The spend count, falling back to one derived from `budget` for a file
    /// written before the field existed or edited by hand.
    fn spends(&self) -> u32 {
        self.spends_since_recharge.unwrap_or_else(|| {
            BUDGET_CAP.saturating_sub(self.budget.unwrap_or(BUDGET_CAP).min(BUDGET_CAP))
        })
    }
}

// Derivable now that the budget is a spend count: every field's zero value is
// its correct initial state, including `spends_since_recharge: 0`, which means
// a full budget.
#[derive(Debug, Default)]
struct RoomState {
    epochs: HashMap<PathBuf, EpochEntry>,
    watermarks: HashMap<PathBuf, u64>,
    recent: HashMap<String, VecDeque<ConsumptionKey>>,
    /// Automatic actions spent since the last recharge, **not** a remaining
    /// count. Storing the spend rather than the remainder is what lets
    /// [`reconcile`] merge two instances without losing either a recharge or a
    /// decrement: both are monotonic within one recharge generation.
    spends_since_recharge: u32,
    recharge_stamp: u64,
    cut: Option<Cut>,
    dirty: bool,
    loaded: bool,
    last_flush: Option<Instant>,
    flush_scheduled: bool,
}

impl RoomState {
    fn to_persisted(&self) -> PersistedRoomState {
        PersistedRoomState {
            epochs: self
                .epochs
                .iter()
                .map(|(path, entry)| (path.to_string_lossy().into_owned(), entry.clone()))
                .collect(),
            watermarks: self
                .watermarks
                .iter()
                .map(|(path, mark)| (path.to_string_lossy().into_owned(), *mark))
                .collect(),
            recent: self
                .recent
                .iter()
                .map(|(session, keys)| (session.clone(), keys.iter().cloned().collect()))
                .collect(),
            budget: Some(self.budget()),
            spends_since_recharge: Some(self.spends_since_recharge),
            recharge_stamp: self.recharge_stamp,
            cut: self.cut.clone(),
        }
    }

    fn adopt(&mut self, persisted: PersistedRoomState) {
        let spends = persisted.spends();
        self.epochs = persisted
            .epochs
            .into_iter()
            .map(|(path, entry)| (PathBuf::from(path), entry))
            .collect();
        self.watermarks = persisted
            .watermarks
            .into_iter()
            .map(|(path, mark)| (PathBuf::from(path), mark))
            .collect();
        self.recent = persisted
            .recent
            .into_iter()
            .map(|(session, keys)| (session, keys.into_iter().collect()))
            .collect();
        self.spends_since_recharge = spends;
        self.recharge_stamp = persisted.recharge_stamp;
        // A restored cut keeps its file, epoch and length, but **never** its
        // sequence tie-break: the reader that produced those sequences is gone,
        // and a fresh reader starts at `reader_seq == 0`, so a surviving
        // comparison would ignore everything it produces. This is the same
        // supersession the reader performs at every re-anchor, applied at load.
        self.cut = persisted.cut.map(|mut cut| {
            cut.supersede_sequence();
            cut
        });
    }

    /// Remaining automatic actions, derived from the spend count.
    fn budget(&self) -> u32 {
        BUDGET_CAP.saturating_sub(self.spends_since_recharge)
    }

    fn remember(&mut self, session_id: &str, key: ConsumptionKey) {
        let recent = self.recent.entry(session_id.to_owned()).or_default();
        if recent.len() >= RECENT_CAPACITY {
            recent.pop_front();
        }
        recent.push_back(key);
    }

    fn was_consumed(&self, session_id: &str, key: &ConsumptionKey) -> bool {
        self.recent
            .get(session_id)
            .is_some_and(|recent| recent.contains(key))
    }
}

type Rooms = HashMap<PathBuf, RoomState>;

fn rooms() -> MutexGuard<'static, Rooms> {
    static ROOMS: OnceLock<Mutex<Rooms>> = OnceLock::new();
    ROOMS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn room_key(room_root: &Path) -> PathBuf {
    std::fs::canonicalize(room_root).unwrap_or_else(|_| room_root.to_path_buf())
}

fn state_path(room_root: &Path) -> PathBuf {
    co_managed_dir(room_root).join(STATE_FILE_NAME)
}

fn read_persisted(room_root: &Path) -> PersistedRoomState {
    let Ok(raw) = std::fs::read_to_string(state_path(room_root)) else {
        return PersistedRoomState::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Run `f` against the room's in-memory state, loading `state.json` on first
/// touch. Reading the file is deliberately **not** done under the advisory
/// lock: a stale read is reconciled inside [`commit_effect`], and taking the
/// lock here would put a file lock on the watcher's path.
fn with_room<T>(room_root: &Path, f: impl FnOnce(&mut RoomState) -> T) -> T {
    let key = room_key(room_root);
    let needs_load = {
        let rooms = rooms();
        !rooms.get(&key).is_some_and(|room| room.loaded)
    };
    let persisted = needs_load.then(|| read_persisted(room_root));

    let mut rooms = rooms();
    let room = rooms.entry(key).or_default();
    if let Some(persisted) = persisted {
        if !room.loaded {
            room.adopt(persisted);
            room.loaded = true;
        }
    }
    f(room)
}

// ---------------------------------------------------------------------------
// Epoch, watermark and cut
// ---------------------------------------------------------------------------

/// Resolve the authoritative epoch for one observation, in memory.
///
/// The epoch advances **only** when that file is observed truncated or
/// replaced, never when the reader moves to a different file. Alongside the
/// epoch the observation fingerprint is stored, so a second observer of the
/// same truncation finds it already recorded and does **not** increment again.
///
/// When the epoch advances, the cut is superseded: otherwise a later truncation
/// would permanently hide new low-offset records.
pub fn observe(room_root: &Path, path: &Path, observation: FileObservation) -> u64 {
    let path = crate::capture::key::normalise_path(path);
    with_room(room_root, |room| {
        match room.epochs.get_mut(&path) {
            Some(entry) => {
                let verdict = classify_observation(&entry.observation, &observation);
                if verdict.advances_epoch() {
                    entry.epoch += 1;
                    entry.observation = observation;
                    let advanced = entry.epoch;
                    if room.cut.as_ref().is_some_and(|cut| cut.path == path) {
                        room.cut = None;
                    }
                    room.dirty = true;
                    advanced
                } else {
                    extend_observation(&mut entry.observation, &observation);
                    let epoch = entry.epoch;
                    room.dirty = true;
                    epoch
                }
            }
            None => {
                // First sight of this file. Returning to a file read earlier
                // finds its entry again, so its epoch is unchanged merely
                // because another file was read in between.
                room.epochs.insert(
                    path,
                    EpochEntry {
                        epoch: 0,
                        observation,
                    },
                );
                room.dirty = true;
                0
            }
        }
    })
}

/// The current epoch for `path`, without recording an observation.
pub fn epoch_of(room_root: &Path, path: &Path) -> u64 {
    let path = crate::capture::key::normalise_path(path);
    with_room(room_root, |room| {
        room.epochs.get(&path).map_or(0, |entry| entry.epoch)
    })
}

/// `consumed_through` for `path`. `None` means "nothing consumed", so a record
/// starting at offset 0 **is** acted on.
pub fn watermark(room_root: &Path, path: &Path) -> Option<u64> {
    let path = crate::capture::key::normalise_path(path);
    with_room(room_root, |room| room.watermarks.get(&path).copied())
}

/// Should a record at `record_start` on `path` be acted on?
///
/// * start > watermark: new, act.
/// * start <= watermark and in the recent set: just consumed by this run.
/// * start <= watermark and not in the recent set: consumed and pruned, abstain.
pub fn is_actionable(
    room_root: &Path,
    session_id: &str,
    key: &ConsumptionKey,
    record_start: Option<u64>,
) -> bool {
    with_room(room_root, |room| {
        if room.was_consumed(session_id, key) {
            return false;
        }
        match (room.watermarks.get(&key.path).copied(), record_start) {
            (None, _) => true,
            (Some(_), None) => true,
            (Some(mark), Some(start)) => start > mark,
        }
    })
}

/// Drop the state of paths that no longer exist. Only those are pruned.
pub fn prune_missing_paths(room_root: &Path) {
    with_room(room_root, |room| {
        let gone: Vec<PathBuf> = room
            .epochs
            .keys()
            .chain(room.watermarks.keys())
            .filter(|path| !path.exists())
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        for path in gone {
            room.epochs.remove(&path);
            room.watermarks.remove(&path);
            room.dirty = true;
        }
    });
}

/// Record the cut for a demand raised over an already-running reader, under the
/// same critical section as the demand, and mirror it onto `slot`.
pub fn register_cut(room_root: &Path, slot: &CaptureSlot, cut: Cut) {
    with_room(room_root, |room| {
        room.cut = Some(cut.clone());
        room.dirty = true;
    });
    slot.set_cut(cut);
}

pub fn cut(room_root: &Path) -> Option<Cut> {
    with_room(room_root, |room| room.cut.clone())
}

// ---------------------------------------------------------------------------
// Budget
// ---------------------------------------------------------------------------

pub fn budget(room_root: &Path) -> u32 {
    with_room(room_root, |room| room.budget())
}

/// Recharge the room's budget back to the cap, in memory only.
///
/// Called from the single user-input choke point for **substantive terminal**
/// input. It takes no file lock: the debounce worker persists it, and a busy
/// lock cannot lose it, because the dirty stamp survives until a successful
/// flush and [`commit_effect`] reconciles the stamp under its own lock.
pub fn recharge_in_memory(room_root: &Path) {
    with_room(room_root, |room| {
        room.spends_since_recharge = 0;
        room.recharge_stamp += 1;
        room.dirty = true;
    });
}

// ---------------------------------------------------------------------------
// The advisory lock, bounded at LOCK_WAIT_BUDGET
// ---------------------------------------------------------------------------

/// A held advisory lock. The `File` IS the lock: dropping the guard closes the
/// handle and releases the OS lock. The lock file is left on disk.
struct RoomLock {
    _file: std::fs::File,
}

enum LockAttempt {
    Held(RoomLock),
    Busy,
    Unavailable(String),
}

/// Take `<room-root>/.co-managed/lock`, waiting at most `budget`.
///
/// Phase 2's own acquisition is private and carries the 2 s config-command
/// bound; this path needs the 100 ms bound of section 9.1, so it opens the same
/// path with its own budget rather than relaxing phase 2's.
fn acquire_lock(room_root: &Path, budget: Duration) -> LockAttempt {
    let dir = co_managed_dir(room_root);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return LockAttempt::Unavailable(format!("captureLockDirFailed: {}: {e}", dir.display()));
    }
    let path = lock_path(room_root);
    // Advisory token only: created if absent, never truncated, since a
    // concurrent holder may have it open.
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) => {
            return LockAttempt::Unavailable(format!(
                "captureLockOpenFailed: {}: {e}",
                path.display()
            ))
        }
    };

    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return LockAttempt::Held(RoomLock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => {
                let elapsed = started.elapsed();
                if elapsed >= budget {
                    return LockAttempt::Busy;
                }
                std::thread::sleep(LOCK_POLL_INTERVAL.min(budget - elapsed));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return LockAttempt::Unavailable(format!(
                    "captureLockFailed: {}: {e}",
                    path.display()
                ))
            }
        }
    }
}

fn write_state_atomic(room_root: &Path, persisted: &PersistedRoomState) -> Result<(), String> {
    let dir = co_managed_dir(room_root);
    let path = state_path(room_root);
    let mut bytes = serde_json::to_vec_pretty(persisted)
        .map_err(|e| format!("captureStateSerializeFailed: {e}"))?;
    bytes.push(b'\n');

    let tmp = dir.join(format!("{STATE_FILE_NAME}.{}.tmp", std::process::id()));
    let written = std::fs::write(&tmp, &bytes)
        .map_err(|e| format!("captureStateTempWriteFailed: {}: {e}", tmp.display()));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("captureStatePublishFailed: {}: {e}", path.display())
    })
}

/// The result of one flush attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FlushOutcome {
    /// `state.json` was rewritten and the room is clean.
    Written,
    /// Nothing to write.
    Clean,
    /// The lock was busy inside [`LOCK_WAIT_BUDGET`]; the room stays dirty for
    /// the next scheduled flush.
    LockBusy,
    Failed(String),
}

/// Persist the room, bounded by [`LOCK_WAIT_BUDGET`]. **Blocking**: only ever
/// called on a blocking thread.
pub fn flush_room(room_root: &Path) -> FlushOutcome {
    let snapshot = with_room(room_root, |room| {
        room.dirty
            .then(|| (room.to_persisted(), room.recharge_stamp))
    });
    let Some((persisted, stamp)) = snapshot else {
        return FlushOutcome::Clean;
    };

    match acquire_lock(room_root, LOCK_WAIT_BUDGET) {
        LockAttempt::Busy => FlushOutcome::LockBusy,
        LockAttempt::Unavailable(e) => FlushOutcome::Failed(e),
        LockAttempt::Held(lock) => {
            let result = write_state_atomic(room_root, &persisted);
            drop(lock);
            match result {
                Ok(()) => {
                    with_room(room_root, |room| {
                        // Only clear the dirty flag when nothing changed while
                        // the write was in flight.
                        if room.recharge_stamp == stamp {
                            room.dirty = false;
                        }
                        room.last_flush = Some(Instant::now());
                        room.flush_scheduled = false;
                    });
                    FlushOutcome::Written
                }
                Err(e) => {
                    with_room(room_root, |room| room.flush_scheduled = false);
                    FlushOutcome::Failed(e)
                }
            }
        }
    }
}

/// Schedule at most one `state.json` write per [`FLUSH_DEBOUNCE`] per room.
///
/// The job runs on `spawn_blocking`, so it never waits on a Tokio worker and
/// never on the watcher thread. A busy lock leaves the room dirty for the next
/// scheduled flush; so does a failed write.
pub fn schedule_flush(room_root: &Path) {
    let due_in = with_room(room_root, |room| {
        if !room.dirty || room.flush_scheduled {
            return None;
        }
        room.flush_scheduled = true;
        Some(match room.last_flush {
            Some(last) => FLUSH_DEBOUNCE.saturating_sub(last.elapsed()),
            None => Duration::ZERO,
        })
    });
    let Some(due_in) = due_in else { return };
    if tokio::runtime::Handle::try_current().is_err() {
        // No runtime (a unit test, or a shutdown path): the room stays dirty and
        // the next scheduled flush picks it up.
        with_room(room_root, |room| room.flush_scheduled = false);
        return;
    }
    let room_root = room_root.to_path_buf();
    tokio::spawn(async move {
        if !due_in.is_zero() {
            tokio::time::sleep(due_in).await;
        }
        let _ = tokio::task::spawn_blocking(move || flush_room(&room_root)).await;
    });
}

// ---------------------------------------------------------------------------
// The effect commit
// ---------------------------------------------------------------------------

/// Reserve the right to perform one effect, exactly once.
///
/// **Synchronous and `await`-free by construction.** The caller runs it inside
/// `tokio::task::spawn_blocking` and awaits the join handle; a panic inside the
/// closure surfaces as a join error and is treated as an abstention, never as a
/// silent success.
///
/// Order, exactly:
///
/// 1. the supervisor gathered `pre` with whatever `await`s it needed, holding
///    no lock of ours;
/// 2. **before touching the lock**, the staleness check — beyond the bound the
///    snapshot is not worth a lock attempt, so it abstains having taken nothing
///    and having created no lock file;
/// 3. the advisory lock, bounded at [`LOCK_WAIT_BUDGET`];
/// 4. under the lock, with no `await` anywhere: staleness again (now including
///    the lock wait), every boolean and the session id, the slot's sequence and
///    key and validity, not-already-consumed, budget;
/// 5. still under the lock: watermark, recent set and budget, in one section;
/// 6. release;
/// 7. the supervisor performs the effect.
///
/// **Declared residual**: there is a window between step 5 and step 7. On
/// failure there is no retry; the orphan is reported to the user.
pub fn commit_effect(
    room_root: &Path,
    slot: &CaptureSlot,
    expected_seq: u64,
    expected_key: &ConsumptionKey,
    pre: &EffectPreconditions,
) -> Result<CommittedEffect, AbstainReason> {
    // Step 2. Deliberately before the lock: test 19 asserts no lock file is
    // created, which is the executable proof of this ordering.
    if pre.observed_at.elapsed() > STALE_PRECONDITIONS {
        return Err(AbstainReason::PreconditionsStale);
    }

    // Step 3.
    let lock = match acquire_lock(room_root, LOCK_WAIT_BUDGET) {
        LockAttempt::Held(lock) => lock,
        LockAttempt::Busy => return Err(AbstainReason::LockBusy),
        LockAttempt::Unavailable(e) => return Err(AbstainReason::LockUnavailable(e)),
    };

    // Step 4, now including the lock wait.
    if pre.observed_at.elapsed() > STALE_PRECONDITIONS {
        drop(lock);
        return Err(AbstainReason::PreconditionsStale);
    }
    if !pre.all_true() {
        drop(lock);
        return Err(AbstainReason::PreconditionsRejected);
    }

    let state = slot.snapshot();
    if state.seq != expected_seq {
        drop(lock);
        return Err(AbstainReason::SlotChanged);
    }
    let Some(candidate) = state.value.record().cloned() else {
        drop(lock);
        return Err(AbstainReason::SlotChanged);
    };
    if candidate.session_id != pre.session_id {
        drop(lock);
        return Err(AbstainReason::PreconditionsRejected);
    }
    if &ConsumptionKey::from_record(&candidate, expected_key.epoch) != expected_key {
        drop(lock);
        return Err(AbstainReason::SlotChanged);
    }

    // Section 8: a baseline is consumed but never routed, so an `Automatic`
    // request over one is demoted here rather than refused. Refusing would leave
    // it in the slot to be offered again; consuming it without routing is what
    // the plan asks for, and it spends no budget.
    let routable = pre.kind == EffectKind::Automatic && !is_baseline(&candidate);
    let spends_budget = routable;

    // Reconcile against a `state.json` another instance may have written since
    // this room was last touched, then apply step 5 in one section.
    let disk = read_persisted(room_root);
    let outcome = with_room(room_root, |room| {
        reconcile(room, disk);

        if room.was_consumed(&pre.session_id, expected_key) {
            return Err(AbstainReason::AlreadyConsumed);
        }
        if spends_budget && room.budget() == 0 {
            return Err(AbstainReason::BudgetExhausted);
        }

        // Step 5.
        if let Some(start) = expected_key.record_start {
            let mark = room
                .watermarks
                .entry(expected_key.path.clone())
                .or_insert(0);
            *mark = (*mark).max(start);
        }
        room.remember(&pre.session_id, expected_key.clone());
        if spends_budget {
            room.spends_since_recharge = room.spends_since_recharge.saturating_add(1);
        }
        room.dirty = true;
        Ok(CommittedEffect {
            session_id: pre.session_id.clone(),
            key: expected_key.clone(),
            budget_remaining: room.budget(),
            kind: pre.kind,
            routable,
        })
    });

    // Beyond section 9 step 5, deliberately: the step-5 section is in memory,
    // but "at most once" has to survive a crash between step 5 and step 7, and
    // an in-memory-only mark does not. So the reservation is published inside
    // the same lock that took it. The cost is one synchronous, already-bounded
    // write on the effect path; test 20 is what depends on it.
    if outcome.is_ok() {
        let persisted = with_room(room_root, |room| room.to_persisted());
        if write_state_atomic(room_root, &persisted).is_ok() {
            with_room(room_root, |room| {
                room.dirty = false;
                room.last_flush = Some(Instant::now());
            });
        }
    }

    // Step 6.
    drop(lock);
    outcome
}

/// Merge a `state.json` written elsewhere into the in-memory room.
///
/// Watermarks and consumption evidence are additive: the higher watermark and
/// the union of the recent sets always win, because both only ever say "this
/// was already acted on".
///
/// The budget is merged through the **spend count within a recharge
/// generation**, not through the remaining count. Comparing remainders cannot
/// work: whichever side is smaller would have to win to be safe, which loses a
/// recharge, or whichever is larger, which loses a decrement. With a generation
/// stamp and a monotonic spend count inside it, both survive:
///
/// * a higher disk generation means the other instance recharged after this
///   one, so its generation and its spends are adopted whole;
/// * the same generation means both sides are counting the same recharge, so
///   the larger spend count is the true one;
/// * a higher in-memory generation means this instance recharged after the
///   file was written, so the file's spends belong to a superseded generation.
///
/// The residual is narrow and multi-instance only: a decrement another instance
/// makes without having yet seen this instance's recharge lands in the older
/// generation and is dropped. Closing it needs a shared sequencer, which this
/// phase does not have.
fn reconcile(room: &mut RoomState, disk: PersistedRoomState) {
    let disk_spends = disk.spends();
    let disk_recharge_stamp = disk.recharge_stamp;
    for (path, mark) in disk.watermarks {
        let path = PathBuf::from(path);
        let entry = room.watermarks.entry(path).or_insert(mark);
        *entry = (*entry).max(mark);
    }
    for (session, keys) in disk.recent {
        for key in keys {
            if !room.was_consumed(&session, &key) {
                room.remember(&session, key);
            }
        }
    }
    match disk_recharge_stamp.cmp(&room.recharge_stamp) {
        std::cmp::Ordering::Greater => {
            room.recharge_stamp = disk_recharge_stamp;
            room.spends_since_recharge = disk_spends;
        }
        std::cmp::Ordering::Equal => {
            room.spends_since_recharge = room.spends_since_recharge.max(disk_spends);
        }
        std::cmp::Ordering::Less => {}
    }
}

/// Drop one room from the in-memory map. Tests only.
///
/// Scoped to a single room on purpose: the map is global and the test binary is
/// multi-threaded, so clearing all of it would tear down rooms other tests are
/// in the middle of using. It is also how a test simulates a restart, since the
/// next touch reloads `state.json`.
#[cfg(test)]
pub(crate) fn forget_room_for_tests(room_root: &Path) {
    rooms().remove(&room_key(room_root));
}

/// Spend `units` of the room's budget without running a commit. Tests only:
/// the PTY input path needs an exhausted budget to make a recharge observable,
/// and
/// driving three full commits through it would test the wrong thing.
#[cfg(test)]
pub(crate) fn spend_budget_for_tests(room_root: &Path, units: u32) {
    with_room(room_root, |room| {
        room.spends_since_recharge = room.spends_since_recharge.saturating_add(units);
        room.dirty = true;
    });
}

/// Hold the room's advisory lock until the returned guard is dropped. Tests
/// only: it is how a test proves a path is not waiting on the file lock.
#[cfg(test)]
pub(crate) fn hold_room_lock_for_tests(room_root: &Path) -> impl Send {
    match acquire_lock(room_root, Duration::from_secs(5)) {
        LockAttempt::Held(lock) => lock,
        _ => panic!("the test holder must get the room lock"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::record::{CapturedRecord, RecordOrigin};
    use std::sync::Arc;

    fn room(temp: &tempfile::TempDir) -> PathBuf {
        let room = temp.path().join("room-1-dev-team");
        std::fs::create_dir_all(&room).expect("room dir");
        forget_room_for_tests(&room);
        room
    }

    fn obs(len: u64, prefix: &[u8]) -> FileObservation {
        FileObservation::new(len, prefix.to_vec())
    }

    fn record(file: &Path, text: &str, start: Option<u64>, seq: u64) -> Arc<CapturedRecord> {
        record_with_origin(file, text, start, seq, RecordOrigin::Live)
    }

    fn record_with_origin(
        file: &Path,
        text: &str,
        start: Option<u64>,
        seq: u64,
        origin: RecordOrigin,
    ) -> Arc<CapturedRecord> {
        let text_sha256: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(text.as_bytes()).into();
        Arc::new(CapturedRecord {
            session_id: "session-a".to_owned(),
            text: text.to_owned(),
            file: file.to_path_buf(),
            epoch: 0,
            record_start: start,
            reader_seq: seq,
            text_sha256,
            turn_id: None,
            provider: CaptureProvider::Claude,
            provider_final: false,
            turn_identified: false,
            origin,
            observed_path: file.to_path_buf(),
            observed_len: 0,
            observed_prefix: Vec::new(),
        })
    }

    fn preconditions(kind: EffectKind) -> EffectPreconditions {
        EffectPreconditions {
            session_alive: true,
            session_id: "session-a".to_owned(),
            anchor: "agent".to_owned(),
            provider: CaptureProvider::Claude,
            unique_live_session_for_cwd: true,
            no_pending_user_input: true,
            effective_ready: true,
            observed_at: Instant::now(),
            kind,
        }
    }

    /// Publish `record` and return `(seq, key)` for a commit.
    fn arm(slot: &CaptureSlot, record: Arc<CapturedRecord>, epoch: u64) -> (u64, ConsumptionKey) {
        slot.offer(Arc::clone(&record), epoch);
        (slot.seq(), ConsumptionKey::from_record(&record, epoch))
    }

    // Acceptance criterion 10, in a test as well as in the const assertion.
    #[test]
    fn the_lock_wait_is_strictly_below_the_staleness_bound() {
        assert!(LOCK_WAIT_BUDGET < STALE_PRECONDITIONS);
        assert_eq!(LOCK_WAIT_BUDGET, Duration::from_millis(100));
        assert_eq!(STALE_PRECONDITIONS, Duration::from_millis(250));
    }

    // Test 7: truncation advances the epoch exactly once; a second observer of
    // the same truncation does not advance it again.
    #[test]
    fn truncation_advances_the_epoch_exactly_once() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");

        assert_eq!(observe(&root, &file, obs(100, b"head-aaaa")), 0);
        assert_eq!(observe(&root, &file, obs(10, b"head-aaaa")), 1);
        // A second observer sees the same truncation already recorded.
        assert_eq!(observe(&root, &file, obs(10, b"head-aaaa")), 1);
        assert_eq!(epoch_of(&root, &file), 1);
    }

    #[test]
    fn appending_never_advances_the_epoch() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");
        assert_eq!(observe(&root, &file, obs(100, b"aaaa")), 0);
        assert_eq!(observe(&root, &file, obs(110, b"aaaa")), 0);
        assert_eq!(observe(&root, &file, obs(4096, b"aaaa")), 0);
    }

    // Test 10: reading file B and returning to file A recovers A's watermark,
    // and A's epoch is unchanged.
    #[test]
    fn returning_to_a_file_recovers_its_watermark_and_epoch() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let a = temp.path().join("a.jsonl");
        let b = temp.path().join("b.jsonl");
        std::fs::write(&a, b"a").expect("write a");
        std::fs::write(&b, b"b").expect("write b");

        observe(&root, &a, obs(100, b"aaaa"));
        observe(&root, &a, obs(40, b"aaaa")); // truncation: epoch 1
        let epoch_a = epoch_of(&root, &a);
        assert_eq!(epoch_a, 1);

        let slot = CaptureSlot::new();
        let record_a = record(&a, "from a", Some(30), 0);
        let (seq, key_a) = arm(&slot, record_a, epoch_a);
        commit_effect(
            &root,
            &slot,
            seq,
            &key_a,
            &preconditions(EffectKind::Automatic),
        )
        .expect("committed");
        assert_eq!(watermark(&root, &a), Some(30));

        // Read B in between; it neither moves A's epoch nor its watermark.
        observe(&root, &b, obs(10, b"bbbb"));
        observe(&root, &b, obs(1, b"bbbb"));
        assert_eq!(epoch_of(&root, &a), epoch_a);
        assert_eq!(watermark(&root, &a), Some(30));

        // Back to A: an appending observation keeps the epoch.
        assert_eq!(observe(&root, &a, obs(50, b"aaaa")), epoch_a);
    }

    // Test 9: watermark `None` plus a record at offset 0 acts.
    #[test]
    fn an_unconsumed_file_acts_on_offset_zero() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");
        let key = ConsumptionKey::from_record(&record(&file, "first", Some(0), 0), 0);
        assert_eq!(watermark(&root, &file), None);
        assert!(is_actionable(&root, "session-a", &key, Some(0)));
    }

    #[test]
    fn a_record_at_or_below_the_watermark_and_out_of_the_recent_set_abstains() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");
        let slot = CaptureSlot::new();
        let (seq, key) = arm(&slot, record(&file, "first", Some(120), 0), 0);
        commit_effect(
            &root,
            &slot,
            seq,
            &key,
            &preconditions(EffectKind::Automatic),
        )
        .expect("committed");

        // In the recent set: known-consumed, not actionable.
        assert!(!is_actionable(&root, "session-a", &key, Some(120)));
        // A different record below the watermark: consumed and pruned, abstain.
        let older = ConsumptionKey::from_record(&record(&file, "older", Some(30), 1), 0);
        assert!(!is_actionable(&root, "session-a", &older, Some(30)));
        // Above the watermark: new, act.
        let newer = ConsumptionKey::from_record(&record(&file, "newer", Some(400), 2), 0);
        assert!(is_actionable(&root, "session-a", &newer, Some(400)));
    }

    #[test]
    fn only_missing_paths_are_pruned() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let alive = temp.path().join("alive.jsonl");
        let gone = temp.path().join("gone.jsonl");
        std::fs::write(&alive, b"x").expect("write");
        std::fs::write(&gone, b"x").expect("write");
        observe(&root, &alive, obs(1, b"x"));
        observe(&root, &gone, obs(1, b"x"));
        std::fs::remove_file(&gone).expect("remove");

        prune_missing_paths(&root);
        assert_eq!(epoch_of(&root, &alive), 0);
        assert!(with_room(&root, |room| room
            .epochs
            .keys()
            .all(|path| path != gone.as_path())));
    }

    // Test 12 (state half): an advanced epoch drops the recorded cut.
    #[test]
    fn an_advanced_epoch_supersedes_the_recorded_cut() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");
        std::fs::write(&file, b"x").expect("write");
        let slot = CaptureSlot::new();
        observe(&root, &file, obs(500, b"aaaa"));
        register_cut(
            &root,
            &slot,
            Cut {
                path: crate::capture::key::normalise_path(&file),
                epoch: 0,
                len: 500,
                reader_seq: Some(9),
            },
        );
        assert!(cut(&root).is_some());
        observe(&root, &file, obs(10, b"aaaa"));
        assert!(cut(&root).is_none(), "truncation must supersede the cut");
    }

    // Test 15: three automatic actions with no human input leave the fourth
    // refused.
    #[test]
    fn the_fourth_automatic_action_is_refused() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");
        let slot = CaptureSlot::new();
        for i in 0..3u64 {
            let (seq, key) = arm(
                &slot,
                record(&file, &format!("turn {i}"), Some(i * 10), i),
                0,
            );
            let committed = commit_effect(
                &root,
                &slot,
                seq,
                &key,
                &preconditions(EffectKind::Automatic),
            )
            .expect("committed");
            assert_eq!(committed.budget_remaining, 2 - u32::try_from(i).unwrap());
            slot.try_consume(seq, &key, 0);
        }
        let (seq, key) = arm(&slot, record(&file, "turn 3", Some(40), 3), 0);
        assert_eq!(
            commit_effect(
                &root,
                &slot,
                seq,
                &key,
                &preconditions(EffectKind::Automatic)
            ),
            Err(AbstainReason::BudgetExhausted)
        );
        assert_eq!(budget(&root), 0);

        // Substantive human input recharges to the cap.
        recharge_in_memory(&root);
        assert_eq!(budget(&root), BUDGET_CAP);
        let (seq, key) = arm(&slot, record(&file, "turn 4", Some(50), 4), 0);
        assert!(commit_effect(
            &root,
            &slot,
            seq,
            &key,
            &preconditions(EffectKind::Automatic)
        )
        .is_ok());
    }

    // Test 16: a text-to-user action does not decrement.
    #[test]
    fn a_text_to_user_action_does_not_decrement() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");
        let slot = CaptureSlot::new();
        for i in 0..5u64 {
            let (seq, key) = arm(
                &slot,
                record(&file, &format!("note {i}"), Some(i * 10), i),
                0,
            );
            let committed = commit_effect(
                &root,
                &slot,
                seq,
                &key,
                &preconditions(EffectKind::TextToUser),
            )
            .expect("committed");
            assert_eq!(committed.budget_remaining, BUDGET_CAP);
            slot.try_consume(seq, &key, 0);
        }
        assert_eq!(budget(&root), BUDGET_CAP);
    }

    // Test 18: flipping `enabled` to false between the snapshot and the commit
    // yields zero effects.
    #[test]
    fn a_room_turned_off_after_the_snapshot_commits_nothing() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");
        let slot = CaptureSlot::new();
        let (seq, key) = arm(&slot, record(&file, "turn", Some(10), 0), 0);

        let mut pre = preconditions(EffectKind::Automatic);
        pre.effective_ready = false;
        assert_eq!(
            commit_effect(&root, &slot, seq, &key, &pre),
            Err(AbstainReason::PreconditionsRejected)
        );
        assert_eq!(budget(&root), BUDGET_CAP);
        assert_eq!(watermark(&root, &file), None);
        assert!(matches!(
            slot.snapshot().value,
            crate::capture::sink::SlotValue::Valid(_)
        ));
    }

    // Test 19: an aged snapshot abstains with `PreconditionsStale`, mutates
    // nothing, and creates no lock file — which proves the check ran before the
    // acquisition.
    #[test]
    fn an_aged_snapshot_abstains_before_touching_the_lock() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");
        let slot = CaptureSlot::new();
        let (seq, key) = arm(&slot, record(&file, "turn", Some(10), 0), 0);

        let mut pre = preconditions(EffectKind::Automatic);
        pre.observed_at = Instant::now() - STALE_PRECONDITIONS - Duration::from_millis(10);
        assert_eq!(
            commit_effect(&root, &slot, seq, &key, &pre),
            Err(AbstainReason::PreconditionsStale)
        );
        assert_eq!(budget(&root), BUDGET_CAP);
        assert_eq!(watermark(&root, &file), None);
        assert!(
            !lock_path(&root).exists(),
            "the staleness check must run before the lock is opened"
        );
        assert!(!co_managed_dir(&root).exists());
    }

    // Test 20: a crash between step 5 and step 7 leaves the key marked consumed
    // and the budget decremented: at-most-once, no double action.
    #[test]
    fn a_crash_after_the_commit_cannot_double_act() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");
        let slot = CaptureSlot::new();
        let (seq, key) = arm(&slot, record(&file, "turn", Some(10), 0), 0);
        let committed = commit_effect(
            &root,
            &slot,
            seq,
            &key,
            &preconditions(EffectKind::Automatic),
        )
        .expect("committed");
        assert_eq!(committed.budget_remaining, BUDGET_CAP - 1);

        // The supervisor dies here: the effect never happens. Simulate the
        // restart by dropping the in-memory room and reloading `state.json`.
        forget_room_for_tests(&root);
        assert_eq!(budget(&root), BUDGET_CAP - 1);
        assert!(!is_actionable(&root, "session-a", &key, Some(10)));
        assert_eq!(
            commit_effect(
                &root,
                &slot,
                seq,
                &key,
                &preconditions(EffectKind::Automatic)
            ),
            Err(AbstainReason::AlreadyConsumed),
            "the orphan is reported, never retried into a second action"
        );
    }

    // Test 21: `commit_effect` holds no `await`. It is not `async`, and a
    // current-thread runtime with no reactor work drives it to completion.
    #[test]
    fn commit_effect_completes_on_a_current_thread_runtime() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let slot = CaptureSlot::new();
            let (seq, key) = arm(&slot, record(&file, "turn", Some(10), 0), 0);
            let outcome: Result<CommittedEffect, AbstainReason> = commit_effect(
                &root,
                &slot,
                seq,
                &key,
                &preconditions(EffectKind::Automatic),
            );
            assert!(outcome.is_ok());
        });
    }

    /// Hold `.co-managed/lock` for `hold`, signalling once it is really held.
    fn hold_lock(
        root: &Path,
        hold: Duration,
    ) -> (std::sync::mpsc::Receiver<()>, std::thread::JoinHandle<()>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let root = root.to_path_buf();
        let handle = std::thread::spawn(move || {
            let LockAttempt::Held(lock) = acquire_lock(&root, Duration::from_secs(5)) else {
                panic!("the holder must get the lock");
            };
            tx.send(()).expect("signal");
            std::thread::sleep(hold);
            drop(lock);
        });
        (rx, handle)
    }

    // Test 24: contention is `LockBusy`, not `PreconditionsStale`, and it is
    // bounded. This is the test round 2's staleness test could not have
    // written, because it never took contention.
    #[test]
    fn contention_is_lock_busy_and_bounded() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");
        let slot = CaptureSlot::new();
        let (seq, key) = arm(&slot, record(&file, "turn", Some(10), 0), 0);

        let (ready, holder) = hold_lock(&root, Duration::from_millis(300));
        ready.recv().expect("holder ready");

        let pre = preconditions(EffectKind::Automatic); // a fresh snapshot
        let started = Instant::now();
        let outcome = commit_effect(&root, &slot, seq, &key, &pre);
        let elapsed = started.elapsed();
        holder.join().expect("holder");

        assert_eq!(outcome, Err(AbstainReason::LockBusy));
        assert!(
            elapsed <= LOCK_WAIT_BUDGET + Duration::from_millis(150),
            "gave up after {elapsed:?}"
        );
        assert!(
            pre.observed_at.elapsed() < STALE_PRECONDITIONS + Duration::from_millis(150),
            "the snapshot was still fresh when the call gave up"
        );
        assert_eq!(budget(&root), BUDGET_CAP);
        assert_eq!(watermark(&root, &file), None);
    }

    // Test 25: the staleness window is not consumed by the lock wait. Together
    // with test 24 this pins the ordering of the two bounds, not their values
    // in isolation.
    #[test]
    fn a_short_lock_wait_still_leaves_the_snapshot_fresh() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");
        let slot = CaptureSlot::new();
        let (seq, key) = arm(&slot, record(&file, "turn", Some(10), 0), 0);

        let (ready, holder) = hold_lock(&root, Duration::from_millis(25));
        ready.recv().expect("holder ready");

        let mut pre = preconditions(EffectKind::Automatic);
        pre.observed_at = Instant::now() - Duration::from_millis(75);
        let outcome = commit_effect(&root, &slot, seq, &key, &pre);
        holder.join().expect("holder");

        assert!(outcome.is_ok(), "expected success, got {outcome:?}");
        assert_eq!(watermark(&root, &file), Some(10));
    }

    // Test 26: a held lock bounds the debounce flush, leaves the dirty stamp
    // intact, and a later flush after release writes the recharged budget.
    #[test]
    fn a_held_lock_bounds_the_flush_and_the_next_one_writes() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        recharge_in_memory(&root);
        with_room(&root, |room| {
            room.spends_since_recharge = 2;
            room.dirty = true;
        });
        recharge_in_memory(&root);

        let (ready, holder) = hold_lock(&root, Duration::from_millis(300));
        ready.recv().expect("holder ready");
        let started = Instant::now();
        let outcome = flush_room(&root);
        let elapsed = started.elapsed();

        assert_eq!(outcome, FlushOutcome::LockBusy);
        assert!(
            elapsed <= LOCK_WAIT_BUDGET + Duration::from_millis(150),
            "the flush blocked for {elapsed:?}"
        );
        assert!(
            with_room(&root, |room| room.dirty),
            "a busy lock must leave the room dirty for the next flush"
        );
        assert!(!state_path(&root).exists());

        holder.join().expect("holder");
        assert_eq!(flush_room(&root), FlushOutcome::Written);
        assert!(!with_room(&root, |room| room.dirty));
        let persisted = read_persisted(&root);
        assert_eq!(persisted.budget, Some(BUDGET_CAP));
    }

    #[test]
    fn a_recharge_survives_a_state_file_written_by_another_instance() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");

        // Another instance leaves an exhausted budget on disk.
        std::fs::create_dir_all(co_managed_dir(&root)).expect("dir");
        write_state_atomic(
            &root,
            &PersistedRoomState {
                budget: Some(0),
                recharge_stamp: 0,
                ..PersistedRoomState::default()
            },
        )
        .expect("seed");

        // This instance observes substantive human input.
        assert_eq!(budget(&root), 0);
        recharge_in_memory(&root);

        let slot = CaptureSlot::new();
        let (seq, key) = arm(&slot, record(&file, "turn", Some(10), 0), 0);
        let committed = commit_effect(
            &root,
            &slot,
            seq,
            &key,
            &preconditions(EffectKind::Automatic),
        )
        .expect("the recharge must not be lost under the lock");
        assert_eq!(committed.budget_remaining, BUDGET_CAP - 1);
    }

    #[test]
    fn the_recent_set_is_bounded_at_its_capacity() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");
        for i in 0..(RECENT_CAPACITY as u64 + 10) {
            let key = ConsumptionKey::from_record(&record(&file, &format!("t{i}"), Some(i), i), 0);
            with_room(&root, |room| room.remember("session-a", key));
        }
        let held = with_room(&root, |room| room.recent["session-a"].len());
        assert_eq!(held, RECENT_CAPACITY);
    }

    // Test 13: preamble and rotation-backfill records are consumed and never
    // marked routable. Without this a rotated Claude transcript's first sweep
    // would be acted on automatically, which is the case section 8.3 forbids.
    #[test]
    fn a_baseline_is_consumed_but_never_routed() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");
        let slot = CaptureSlot::new();

        for (origin, start) in [
            (RecordOrigin::Preamble, None),
            (RecordOrigin::RotationBackfill, Some(0)),
        ] {
            let record = record_with_origin(&file, &format!("{origin:?}"), start, 0, origin);
            let (seq, key) = arm(&slot, Arc::clone(&record), 0);

            let committed = commit_effect(
                &root,
                &slot,
                seq,
                &key,
                &preconditions(EffectKind::Automatic),
            )
            .expect("a baseline commits, so it can never be offered again");

            assert!(
                !committed.routable,
                "{origin:?} must never be routed (section 8)"
            );
            assert_eq!(
                committed.budget_remaining, BUDGET_CAP,
                "{origin:?} must not spend budget"
            );
            // Consumed: the key is remembered, so a re-delivery abstains.
            assert!(!is_actionable(&root, "session-a", &key, start));
            slot.try_consume(seq, &key, 0);
        }

        assert_eq!(budget(&root), BUDGET_CAP);
        assert!(is_baseline(&record_with_origin(
            &file,
            "p",
            None,
            0,
            RecordOrigin::Preamble
        )));
        assert!(is_baseline(&record_with_origin(
            &file,
            "r",
            Some(0),
            0,
            RecordOrigin::RotationBackfill
        )));
        assert!(!is_baseline(&record(&file, "live", Some(0), 0)));
    }

    /// A live record in the same room still routes and still spends, so test 13
    /// pins the gate rather than a blanket refusal.
    #[test]
    fn a_live_record_is_still_routable_after_a_baseline() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");
        let slot = CaptureSlot::new();

        let backfill =
            record_with_origin(&file, "history", Some(0), 0, RecordOrigin::RotationBackfill);
        let (seq, key) = arm(&slot, backfill, 0);
        commit_effect(
            &root,
            &slot,
            seq,
            &key,
            &preconditions(EffectKind::Automatic),
        )
        .expect("baseline committed");
        slot.try_consume(seq, &key, 0);

        let (seq, key) = arm(&slot, record(&file, "a real turn", Some(500), 1), 0);
        let committed = commit_effect(
            &root,
            &slot,
            seq,
            &key,
            &preconditions(EffectKind::Automatic),
        )
        .expect("committed");
        assert!(committed.routable);
        assert_eq!(committed.budget_remaining, BUDGET_CAP - 1);
    }

    /// Grinch finding: the authoritative epoch is the one [`observe`] returns,
    /// and `CapturedRecord.epoch` is the reader's advisory counter. After a
    /// restart the two disagree, so a key built from the record's own field
    /// would miss the recent set and the record would be acted on twice.
    /// [`consumption_key`] is the only correct constructor and phase 7 must use
    /// it.
    #[test]
    fn the_authoritative_epoch_is_the_persisted_one_not_the_records_field() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");

        // The room has already seen this file truncated once.
        observe(&root, &file, obs(100, b"head"));
        observe(&root, &file, obs(10, b"head"));
        assert_eq!(epoch_of(&root, &file), 1);

        // A reader that started after that restart stamps its own epoch 0.
        let mut fresh = record(&file, "turn", Some(20), 0);
        Arc::get_mut(&mut fresh).expect("sole owner").epoch = 0;
        Arc::get_mut(&mut fresh).expect("sole owner").observed_len = 20;
        Arc::get_mut(&mut fresh)
            .expect("sole owner")
            .observed_prefix = b"head".to_vec();

        let authoritative = consumption_key(&root, &fresh);
        assert_eq!(
            authoritative.epoch, 1,
            "the persisted epoch survives the reader that produced the record"
        );
        assert_ne!(
            authoritative.epoch, fresh.epoch,
            "the record's own field is advisory and disagrees after a restart"
        );

        // Acting through the authoritative key marks THAT key consumed.
        let slot = CaptureSlot::new();
        slot.offer(Arc::clone(&fresh), authoritative.epoch);
        let seq = slot.seq();
        commit_effect(
            &root,
            &slot,
            seq,
            &authoritative,
            &preconditions(EffectKind::Automatic),
        )
        .expect("committed");

        // The advisory key is a different key: proof that using it would let the
        // same record through a second time.
        let advisory = ConsumptionKey::from_record(&fresh, fresh.epoch);
        assert_ne!(advisory, authoritative);
        assert!(!is_actionable(&root, "session-a", &authoritative, Some(20)));
    }

    /// Grinch finding: a cut restored from `state.json` must not keep the
    /// sequence tie-break of the reader that is gone. A fresh reader starts at
    /// `reader_seq == 0`, so a surviving comparison would hide everything it
    /// produces — exactly the bug section 7 paragraph 2 closes.
    #[test]
    fn a_cut_restored_from_disk_loses_its_sequence_tie_break() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");
        std::fs::write(&file, b"x").expect("write");
        let slot = CaptureSlot::new();

        register_cut(
            &root,
            &slot,
            Cut {
                path: crate::capture::key::normalise_path(&file),
                epoch: 0,
                len: 0,
                reader_seq: Some(42),
            },
        );
        assert_eq!(
            flush_room(&root),
            FlushOutcome::Written,
            "the cut has to reach disk for this test to mean anything"
        );
        assert_eq!(
            cut(&root).and_then(|c| c.reader_seq),
            Some(42),
            "the live cut keeps its tie-break within one reader lifetime"
        );

        // Restart.
        forget_room_for_tests(&root);
        let restored = cut(&root).expect("the cut is persisted");
        assert_eq!(restored.path, crate::capture::key::normalise_path(&file));
        assert_eq!(restored.len, 0);
        assert_eq!(
            restored.reader_seq, None,
            "the sequence tie-break must not survive the reader"
        );

        // And a fresh reader's first record is not hidden by it.
        let fresh = CaptureSlot::new();
        fresh.set_cut(restored);
        assert_eq!(
            fresh.offer(record(&file, "first after restart", Some(0), 0), 0),
            crate::capture::sink::OfferOutcome::Published
        );
    }

    /// Grinch finding: a decrement another instance made inside the same
    /// recharge generation must survive reconciliation. The budget is merged
    /// through the spend count, so neither side's work is discarded.
    #[test]
    fn a_second_instances_decrement_is_not_lost_by_reconciliation() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        let file = temp.path().join("a.jsonl");

        // This instance recharges, so generation 1 is current, and spends one.
        recharge_in_memory(&root);
        let slot = CaptureSlot::new();
        let (seq, key) = arm(&slot, record(&file, "mine", Some(10), 0), 0);
        commit_effect(
            &root,
            &slot,
            seq,
            &key,
            &preconditions(EffectKind::Automatic),
        )
        .expect("committed");
        slot.try_consume(seq, &key, 0);
        assert_eq!(budget(&root), BUDGET_CAP - 1);

        // A second instance, in the SAME generation, spends two more.
        let mut disk = read_persisted(&root);
        disk.spends_since_recharge = Some(3);
        disk.budget = Some(0);
        write_state_atomic(&root, &disk).expect("seed the other instance's file");

        // This instance reconciles: three spends in generation 1, not one.
        let (seq, key) = arm(&slot, record(&file, "fourth", Some(20), 1), 0);
        assert_eq!(
            commit_effect(
                &root,
                &slot,
                seq,
                &key,
                &preconditions(EffectKind::Automatic)
            ),
            Err(AbstainReason::BudgetExhausted),
            "the other instance's decrements must not vanish"
        );
        assert_eq!(budget(&root), 0);
    }

    /// The other direction, already covered by
    /// `a_recharge_survives_a_state_file_written_by_another_instance`: a
    /// superseded generation on disk does not undo this instance's recharge.
    #[test]
    fn a_superseded_generation_on_disk_does_not_undo_a_recharge() {
        let temp = tempfile::tempdir().expect("temp");
        let root = room(&temp);
        std::fs::create_dir_all(co_managed_dir(&root)).expect("dir");
        write_state_atomic(
            &root,
            &PersistedRoomState {
                budget: Some(0),
                spends_since_recharge: Some(3),
                recharge_stamp: 0,
                ..PersistedRoomState::default()
            },
        )
        .expect("seed");
        assert_eq!(budget(&root), 0);

        recharge_in_memory(&root);
        let mut room_snapshot = RoomState {
            recharge_stamp: 1,
            ..Default::default()
        };
        reconcile(&mut room_snapshot, read_persisted(&root));
        assert_eq!(room_snapshot.budget(), BUDGET_CAP);
    }

    /// Grinch finding: the reconstruction in [`head_from_lines`] is the only
    /// permitted prefix source. This test names that rule and shows what
    /// happens if it is broken.
    #[test]
    fn prefix_source_is_the_reconstruction_only() {
        let lines = vec![
            (0u64, "{\"a\":1}".to_string()),
            (8u64, "{\"b\":2}".to_string()),
        ];
        let reconstructed = head_from_lines(&lines, 0);
        assert_eq!(reconstructed, b"{\"a\":1}\n{\"b\":2}\n".to_vec());

        // A read that did not start at offset 0 carries no head evidence.
        assert!(head_from_lines(&lines, 4096).is_empty());

        // Self-consistent: the same lines reconstruct identically, so the
        // verdict is `Append` and the epoch holds.
        let stored = FileObservation::new(18, reconstructed.clone());
        let again = FileObservation::new(18, head_from_lines(&lines, 0));
        assert_eq!(
            classify_observation(&stored, &again),
            crate::capture::key::ObservationVerdict::Append
        );

        // The hazard, made executable: true file bytes with CRLF endings are a
        // DIFFERENT byte string, so mixing the two sources flips the verdict to
        // `Replaced`, advances the epoch spuriously and invalidates every
        // watermark for that path. This is why the reconstruction is pinned.
        let true_bytes = FileObservation::new(20, b"{\"a\":1}\r\n{\"b\":2}\r\n".to_vec());
        assert_eq!(
            classify_observation(&stored, &true_bytes),
            crate::capture::key::ObservationVerdict::Replaced,
            "mixing a true-bytes prefix with the reconstruction is forbidden"
        );
    }
}
