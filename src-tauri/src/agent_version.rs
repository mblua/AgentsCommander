//! #1551 - installed-version probe engine and session cache for coding-agent CLIs.
//!
//! Install cache and scheduling policy remain here. `process::agent` owns native
//! version probes and reaches `process::host::job` for containment and settlement.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub(crate) use crate::process::agent::probe_version_cancellable_retained;
pub use crate::process::agent::{
    first_text_line, parse_version_token, probe_version, probe_version_cancellable,
    probe_version_in, sanitize_detail, text_lines, CancellableProbeOutcome, ProbeOutcome,
    DETAIL_MAX_CHARS, PROBE_OUTPUT_CAP, PROBE_TIMEOUT,
};
#[cfg(test)]
pub(crate) use crate::process::agent::{
    strip_ansi_and_controls, with_probe_settlement_hook, with_probe_settlement_hook_after_spawns,
    ProbeNativeObservation, ProbeReaderSettlementHook, ProbeSettlementHook,
};

/// A committed install state is served without a process for this long.
pub const INSTALL_CACHE_TTL: Duration = Duration::from_secs(600);

#[cfg(test)]
tokio::task_local! {
    static VERSION_PROBE_ARGS_OVERRIDE: (&'static str, &'static [&'static str]);
}

#[cfg(test)]
pub(crate) async fn with_version_probe_args_for_test<F>(
    stem: &'static str,
    args: &'static [&'static str],
    future: F,
) -> F::Output
where
    F: std::future::Future,
{
    VERSION_PROBE_ARGS_OVERRIDE
        .scope((stem, args), future)
        .await
}

/// #1551 - fixed probe argv per known program stem (lowercase, extension stripped).
/// No catalog/user/project string ever reaches argv. Cursor (`agent`) is absent on
/// purpose: it has no update command and its bare name collides with other vendors.
pub fn version_probe_args(program_stem: &str) -> Option<&'static [&'static str]> {
    #[cfg(test)]
    if let Ok((stem, args)) = VERSION_PROBE_ARGS_OVERRIDE.try_with(|override_| *override_) {
        if program_stem == stem {
            return Some(args);
        }
    }
    match program_stem {
        "claude" | "codex" | "hermes" | "pi" | "opencode" | "agy" => Some(&["--version"]),
        _ => None,
    }
}

/// The five install states a row can carry. `checking` is the client-visible
/// "no committed state yet" value, never a stored one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallStatus {
    Checking,
    Missing,
    Installed,
    ProbeFailed,
    Unprobed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallState {
    pub status: InstallStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// #1551 - commit counter of the install cache that stored this state; 0 until
    /// `ProbeTicket::complete` commits it (so `checking` and un-committed probe results
    /// are 0). Clients keep the highest `seq` per command (plan section 5.11).
    #[serde(default)]
    pub seq: u64,
}

impl InstallState {
    pub fn checking() -> Self {
        Self {
            status: InstallStatus::Checking,
            version: None,
            path: None,
            detail: None,
            seq: 0,
        }
    }

    pub fn missing(detail: String) -> Self {
        Self {
            status: InstallStatus::Missing,
            version: None,
            path: None,
            detail: Some(detail),
            seq: 0,
        }
    }

    pub fn installed(version: String, path: &Path) -> Self {
        Self {
            status: InstallStatus::Installed,
            version: Some(version),
            path: Some(path.display().to_string()),
            detail: None,
            seq: 0,
        }
    }

    pub fn probe_failed(path: &Path, detail: String) -> Self {
        Self {
            status: InstallStatus::ProbeFailed,
            version: None,
            path: Some(path.display().to_string()),
            detail: Some(detail),
            seq: 0,
        }
    }

    pub fn unprobed(path: &Path, detail: String) -> Self {
        Self {
            status: InstallStatus::Unprobed,
            version: None,
            path: Some(path.display().to_string()),
            detail: Some(detail),
            seq: 0,
        }
    }
}

/// #1551 - process-lifetime install-state cache, keyed by the catalog `command`
/// string (the update unit, like `agentAutoUpdateByCommand`).
#[derive(Default)]
pub struct AgentInstallCache {
    inner: Mutex<CacheInner>,
}

#[derive(Default)]
struct CacheInner {
    generation: u64,
    seq: u64,
    entries: HashMap<String, CachedInstall>,
    in_flight: HashSet<String>,
}

struct CachedInstall {
    probed_at: Instant,
    state: InstallState,
}

/// Read-only view of one command's cache slot.
#[derive(Clone, Debug)]
pub enum CacheLookup {
    Fresh(InstallState),
    InFlight,
    Absent,
}

/// #1551 - result of the single-lock scheduling operation.
pub enum Scheduling {
    Fresh(InstallState),
    InFlight,
    Began(ProbeTicket),
    Deferred,
}

/// The exclusive right to probe one command and commit its state.
pub struct ProbeTicket {
    cache: Arc<AgentInstallCache>,
    command: String,
    generation: u64,
    live: bool,
}

/// #1551 - result of closing a ticket: the state is committed to the current generation,
/// or the cache was invalidated meanwhile and the SAME ticket continues (renewed) for a retry.
pub enum Completion {
    Committed(InstallState),
    Stale(ProbeTicket),
}

impl AgentInstallCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, CacheInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Fresh iff an entry exists and `now.duration_since(probed_at) <= ttl` (the
    /// stored state, seq included); else InFlight iff a ticket is open for
    /// `command`; else Absent (an expired entry is removed on the way).
    pub fn lookup(&self, command: &str, now: Instant, ttl: Duration) -> CacheLookup {
        let mut inner = self.lock();
        let expired = match inner.entries.get(command) {
            Some(entry) if now.duration_since(entry.probed_at) <= ttl => {
                return CacheLookup::Fresh(entry.state.clone())
            }
            Some(_) => true,
            None => false,
        };
        if expired {
            inner.entries.remove(command);
        }
        if inner.in_flight.contains(command) {
            return CacheLookup::InFlight;
        }
        CacheLookup::Absent
    }

    /// #1551 - the ONLY production scheduling primitive: ONE critical section decides between
    /// serving a fresh entry, reporting an open ticket, opening the ticket (iff `may_begin`;
    /// `in_flight` insert + generation capture in the same lock take), or deferring. Two concurrent
    /// callers can never both receive `Began` for the same command, and a commit that lands before
    /// this call is always served as `Fresh`: there is no observable state between "looked up" and
    /// "began".
    pub fn lookup_or_begin(
        self: &Arc<Self>,
        command: &str,
        now: Instant,
        ttl: Duration,
        may_begin: bool,
    ) -> Scheduling {
        let mut inner = self.lock();
        let expired = match inner.entries.get(command) {
            Some(entry) if now.duration_since(entry.probed_at) <= ttl => {
                return Scheduling::Fresh(entry.state.clone())
            }
            Some(_) => true,
            None => false,
        };
        if expired {
            inner.entries.remove(command);
        }
        if inner.in_flight.contains(command) {
            return Scheduling::InFlight;
        }
        if !may_begin {
            return Scheduling::Deferred;
        }
        inner.in_flight.insert(command.to_string());
        let generation = inner.generation;
        drop(inner);
        Scheduling::Began(ProbeTicket {
            cache: Arc::clone(self),
            command: command.to_string(),
            generation,
            live: true,
        })
    }

    /// #1551 - after a startup pass every entry is stale: `generation += 1`, `entries.clear()`.
    /// Tickets stay open (their next `complete` returns `Stale`) and `seq` is NOT reset, so every
    /// post-invalidation commit outranks every pre-invalidation state on the clients.
    pub fn invalidate_all(&self) {
        let mut inner = self.lock();
        inner.generation += 1;
        inner.entries.clear();
    }

    /// Test seeding only (opens a ticket without consulting entries); production never schedules
    /// through it.
    #[cfg(test)]
    pub fn try_begin(self: &Arc<Self>, command: &str) -> Option<ProbeTicket> {
        let mut inner = self.lock();
        if !inner.in_flight.insert(command.to_string()) {
            return None;
        }
        let generation = inner.generation;
        drop(inner);
        Some(ProbeTicket {
            cache: Arc::clone(self),
            command: command.to_string(),
            generation,
            live: true,
        })
    }

    #[cfg(test)]
    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    #[cfg(test)]
    pub fn seq(&self) -> u64 {
        self.lock().seq
    }

    #[cfg(test)]
    pub fn in_flight_len(&self) -> usize {
        self.lock().in_flight.len()
    }
}

impl ProbeTicket {
    /// Under the cache lock: if `generation` still matches, `seq += 1`, stamp `state.seq`, insert
    /// the entry (`probed_at = now`), remove `command` from `in_flight`, set `live = false`, return
    /// `Committed(stamped)`. Otherwise adopt the current generation and return `Stale(self)`: the
    /// ticket is NEVER released between the rejection and the retry, so no competing
    /// `lookup_or_begin` can take the slot in between.
    pub fn complete(mut self, state: InstallState) -> Completion {
        // The guard is confined to this block so `self` is only ever dropped
        // after the cache lock was released.
        let committed = {
            let mut inner = self.cache.lock();
            if inner.generation == self.generation {
                inner.seq += 1;
                let mut stamped = state;
                stamped.seq = inner.seq;
                inner.entries.insert(
                    self.command.clone(),
                    CachedInstall {
                        probed_at: Instant::now(),
                        state: stamped.clone(),
                    },
                );
                inner.in_flight.remove(&self.command);
                self.live = false;
                Some(stamped)
            } else {
                self.generation = inner.generation;
                None
            }
        };
        match committed {
            Some(stamped) => Completion::Committed(stamped),
            None => Completion::Stale(self),
        }
    }
}

impl Drop for ProbeTicket {
    /// Removes `command` from `in_flight` ONLY while `live` (an un-completed ticket: task panicked,
    /// aborted, or a second `Stale` was dropped). A committed ticket already cleared `live`, so its
    /// drop can never free the slot of a later ticket for the same command.
    fn drop(&mut self) {
        if self.live {
            let mut inner = self.cache.lock();
            inner.in_flight.remove(&self.command);
        }
    }
}

mod process_owner;

pub(crate) use process_owner::RetainedProbeOwner;

#[cfg(test)]
mod tests;
