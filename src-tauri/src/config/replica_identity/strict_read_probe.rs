use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static STRICT_READS: Mutex<BTreeMap<PathBuf, usize>> = Mutex::new(BTreeMap::new());

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(super) fn record(replica_dir: &Path) {
    // Inactive fast path: no registered root, no canonicalization, so an
    // unrelated measurement run pays only this lock.
    if STRICT_READS.lock().map_or(true, |roots| roots.is_empty()) {
        return;
    }
    let replica_dir = canonical(replica_dir);
    if let Ok(mut roots) = STRICT_READS.lock() {
        for (root, count) in roots.iter_mut() {
            if replica_dir.starts_with(root) {
                *count += 1;
            }
        }
    }
}

/// Start counting strict reads under `root` (from zero).
pub(crate) fn register(root: &Path) {
    STRICT_READS
        .lock()
        .expect("strict reads")
        .insert(canonical(root), 0);
}

/// Stop counting under `root` and return its final count.
pub(crate) fn unregister(root: &Path) -> usize {
    STRICT_READS
        .lock()
        .expect("strict reads")
        .remove(&canonical(root))
        .unwrap_or(0)
}

pub(crate) fn count(root: &Path) -> usize {
    STRICT_READS
        .lock()
        .expect("strict reads")
        .get(&canonical(root))
        .copied()
        .unwrap_or(0)
}
