//! Thread-local capture of activity records emitted on the current thread.
//! `note` appends records and `drain` takes everything noted since the last drain.
//! Owns only the per-thread `CAPTURED` buffer; it never writes logs or files.
//! Building and persisting `ActivityRecord` values lives in the parent `activity_log` module.

use super::ActivityRecord;
use std::cell::RefCell;

thread_local! {
    static CAPTURED: RefCell<Vec<ActivityRecord>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn note(records: &[ActivityRecord]) {
    CAPTURED.with(|captured| captured.borrow_mut().extend(records.iter().cloned()));
}

/// Take everything emitted on this thread since the last call.
pub(crate) fn drain() -> Vec<ActivityRecord> {
    CAPTURED.with(|captured| std::mem::take(&mut *captured.borrow_mut()))
}
