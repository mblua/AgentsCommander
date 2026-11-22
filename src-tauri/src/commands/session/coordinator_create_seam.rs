//! Test-only seam that forces a create to treat a cwd as a coordinator or a root agent.
//! Owns one process-wide `FORCED` set of keys; root keys are stored with a `root:` prefix.
//! `create::core` ORs `forced` and `forced_root` into its real detection.
//! Compiled only under `#[cfg(test)]`; it changes no production behavior.

use std::collections::HashSet;
use std::sync::Mutex;

static FORCED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

pub(crate) fn force_root(key: &str) {
    force(&format!("root:{key}"));
}
pub(crate) fn unforce_root(key: &str) {
    unforce(&format!("root:{key}"));
}
pub(crate) fn forced_root(key: &str) -> bool {
    forced(&format!("root:{key}"))
}

pub(crate) fn force(key: &str) {
    FORCED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_with(HashSet::new)
        .insert(key.to_string());
}

pub(crate) fn unforce(key: &str) {
    if let Some(set) = FORCED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
    {
        set.remove(key);
    }
}

pub(crate) fn forced(key: &str) -> bool {
    FORCED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|set| set.contains(key))
}
