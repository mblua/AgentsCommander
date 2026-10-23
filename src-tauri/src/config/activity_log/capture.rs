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
