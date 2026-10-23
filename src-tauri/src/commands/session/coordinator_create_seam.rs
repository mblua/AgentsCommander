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
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashSet::new)
        .insert(key.to_string());
}

pub(crate) fn unforce(key: &str) {
    if let Some(set) = FORCED.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        set.remove(key);
    }
}

pub(crate) fn forced(key: &str) -> bool {
    FORCED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|set| set.contains(key))
}
