use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

pub(crate) const ARM_VAR: &str = "AC2786_LOAD_PROBE";

static OBSERVER: Mutex<()> = Mutex::new(());
static ENTRIES: Mutex<Option<Vec<PathBuf>>> = Mutex::new(None);

pub(crate) fn armed() -> bool {
    static ARMED: OnceLock<bool> = OnceLock::new();
    *ARMED.get_or_init(|| std::env::var(ARM_VAR).as_deref() == Ok("1"))
}

pub(crate) struct Observation {
    _slot: MutexGuard<'static, ()>,
}

impl Observation {
    pub(crate) fn open() -> Observation {
        assert!(
            armed(),
            "#2786 C1: Observation::open in an unarmed process; run this test \
             through the dedicated-process harness"
        );
        let slot = OBSERVER.lock().unwrap_or_else(|e| e.into_inner());
        *ENTRIES.lock().unwrap_or_else(|e| e.into_inner()) = Some(Vec::new());
        Observation { _slot: slot }
    }
    pub(crate) fn loads(&self) -> Vec<PathBuf> {
        ENTRIES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_default()
    }
}

// #2786 C1 r29 suppression inventory. The candidate carries exactly ONE
// `allow(dead_code)`, on `NoObservation::open` below. r28 carried two and
// described them with a single shared sentence; that sentence was false for
// the field one, which a reviewer measured separately.
//
// Measured, one allowance at a time, with `cargo clippy -p agentscommander
// --all-targets -- -D warnings`:
//
//  - the FIELD allowance r28 had on the tuple field is GONE, not suppressed:
//    the guard is now the named field `_slot`, exactly like `Observation`,
//    and `dead_code` does not ask for an underscore-prefixed field to be
//    read. With the r28 tuple form and that allowance removed, clippy
//    reported `field 0 is never read` (the reviewer's measurement, and mine).
//    Impact of the representation change: none at run time. The field is
//    held for its RAII effect only, it is never read in either form, and a
//    named field is dropped at exactly the same point as a tuple field.
//
//  - the FUNCTION allowance on `NoObservation::open` is removed too, in the
//    commit that adds the fail-closed row
//    (`a_load_with_no_open_observation_fails_closed`, E15's fourth
//    selector), which constructs a `NoObservation`. That was its stated
//    removal condition, and clippy is clean without it.
//
// The load-probe prototype is the cross-check: it DOES carry that row, and
// with `_slot` it needs zero `allow(dead_code)` attributes and clippy is
// clean, which is the same two conditions stated above, satisfied.
pub(crate) struct NoObservation {
    _slot: MutexGuard<'static, ()>,
}

impl NoObservation {
    pub(crate) fn open() -> NoObservation {
        assert!(
            armed(),
            "#2786 C1: NoObservation::open in an unarmed process"
        );
        let slot = OBSERVER.lock().unwrap_or_else(|e| e.into_inner());
        *ENTRIES.lock().unwrap_or_else(|e| e.into_inner()) = None;
        NoObservation { _slot: slot }
    }
}

impl Drop for Observation {
    fn drop(&mut self) {
        *ENTRIES.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

pub(crate) fn record_load(dir: &Path) {
    if !armed() {
        return;
    }
    let mut g = ENTRIES.lock().unwrap_or_else(|e| e.into_inner());
    match g.as_mut() {
        Some(v) => v.push(dir.to_path_buf()),
        None => panic!("#2786 C1: wrapper load with no open Observation: {dir:?}"),
    }
}
