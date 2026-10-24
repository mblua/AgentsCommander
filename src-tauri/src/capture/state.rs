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

#[cfg(test)]
use std::collections::HashSet;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::capture::key::{
    classify_observation, extend_observation, ConsumptionKey, Cut, FileObservation,
};
use crate::capture::record::{CapturedRecord, RecordOrigin};
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
#[cfg(test)]
pub fn epoch_of(room_root: &Path, path: &Path) -> u64 {
    let path = crate::capture::key::normalise_path(path);
    with_room(room_root, |room| {
        room.epochs.get(&path).map_or(0, |entry| entry.epoch)
    })
}

/// `consumed_through` for `path`. `None` means "nothing consumed", so a record
/// starting at offset 0 **is** acted on.
#[cfg(test)]
pub fn watermark(room_root: &Path, path: &Path) -> Option<u64> {
    let path = crate::capture::key::normalise_path(path);
    with_room(room_root, |room| room.watermarks.get(&path).copied())
}

/// Should a record at `record_start` on `path` be acted on?
///
/// * start > watermark: new, act.
/// * start <= watermark and in the recent set: just consumed by this run.
/// * start <= watermark and not in the recent set: consumed and pruned, abstain.
#[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
pub fn register_cut(room_root: &Path, slot: &CaptureSlot, cut: Cut) {
    with_room(room_root, |room| {
        room.cut = Some(cut.clone());
        room.dirty = true;
    });
    slot.set_cut(cut);
}

#[cfg(test)]
pub fn cut(room_root: &Path) -> Option<Cut> {
    with_room(room_root, |room| room.cut.clone())
}

// ---------------------------------------------------------------------------
// Budget
// ---------------------------------------------------------------------------

#[cfg(test)]
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
mod tests;
