use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Instant;

use crate::window_capture::{
    DiscoveredWindow, TargetFingerprint, WindowCaptureError, WindowDiagnostics, WindowTargetId,
    TARGET_TTL,
};

pub(crate) const MAX_CALLERS: usize = 128;
pub(crate) const MAX_ENTRIES: usize = 4_096;
pub(crate) const MAX_ENTRIES_PER_CALLER: usize = 256;

/// A SHA-256 caller binding derived by the authenticated-request module. The
/// registry retains only this opaque value, never a bearer, root, or subject.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct CallerBinding([u8; 32]);

impl CallerBinding {
    pub(crate) fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest)
    }
}

pub(crate) struct RegisteredWindow {
    pub(crate) target_id: WindowTargetId,
    pub(crate) diagnostics: WindowDiagnostics,
}

struct TargetEntry {
    caller_binding: CallerBinding,
    expires_at: Instant,
    fingerprint: TargetFingerprint,
}

/// Bounded, caller-bound, monotonic target registry. All mutations update the
/// entry map, caller index, and expiry index while the caller owns one mutex.
pub(crate) struct WindowTargetRegistry {
    entries: HashMap<WindowTargetId, TargetEntry>,
    caller_targets: HashMap<CallerBinding, BTreeSet<WindowTargetId>>,
    expiry_index: BTreeMap<Instant, BTreeSet<WindowTargetId>>,
    max_callers: usize,
    max_entries: usize,
    max_entries_per_caller: usize,
}

impl Default for WindowTargetRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowTargetRegistry {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
            caller_targets: HashMap::new(),
            expiry_index: BTreeMap::new(),
            max_callers: MAX_CALLERS,
            max_entries: MAX_ENTRIES,
            max_entries_per_caller: MAX_ENTRIES_PER_CALLER,
        }
    }

    pub(crate) fn replace_for_caller(
        &mut self,
        caller_binding: CallerBinding,
        candidates: Vec<DiscoveredWindow>,
    ) -> Result<Vec<RegisteredWindow>, WindowCaptureError> {
        self.replace_for_caller_at(caller_binding, candidates, Instant::now())
    }

    pub(crate) fn preflight_for_caller(
        &mut self,
        caller_binding: CallerBinding,
        target_id: &WindowTargetId,
    ) -> Result<(), WindowCaptureError> {
        self.preflight_for_caller_at(caller_binding, target_id, Instant::now())
    }

    pub(crate) fn consume_for_caller(
        &mut self,
        caller_binding: CallerBinding,
        target_id: &WindowTargetId,
    ) -> Result<TargetFingerprint, WindowCaptureError> {
        if self.expire_requested_for_caller(caller_binding, target_id, Instant::now()) {
            return Err(WindowCaptureError::StaleTarget);
        }
        self.prune_expired(Instant::now());

        let owned_by_caller = self
            .caller_targets
            .get(&caller_binding)
            .is_some_and(|target_ids| target_ids.contains(target_id));
        if !owned_by_caller {
            return Err(WindowCaptureError::TargetNotFound);
        }

        self.remove_entry(target_id)
            .map(|entry| entry.fingerprint)
            .ok_or(WindowCaptureError::TargetNotFound)
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.caller_targets.clear();
        self.expiry_index.clear();
    }

    fn replace_for_caller_at(
        &mut self,
        caller_binding: CallerBinding,
        mut candidates: Vec<DiscoveredWindow>,
        now: Instant,
    ) -> Result<Vec<RegisteredWindow>, WindowCaptureError> {
        self.prune_expired(now);
        candidates.sort_by(DiscoveredWindow::compare_for_registry);
        candidates.truncate(self.max_entries_per_caller);

        let prior_count = self
            .caller_targets
            .get(&caller_binding)
            .map_or(0, BTreeSet::len);
        let caller_already_present = prior_count > 0;
        let resulting_entries = self.entries.len() - prior_count + candidates.len();
        let resulting_callers = self.caller_targets.len() - usize::from(caller_already_present)
            + usize::from(!candidates.is_empty());
        if resulting_entries > self.max_entries || resulting_callers > self.max_callers {
            return Err(WindowCaptureError::CaptureBusy);
        }

        let prior_ids = self
            .caller_targets
            .get(&caller_binding)
            .map(|target_ids| target_ids.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for target_id in prior_ids {
            let _ = self.remove_entry(&target_id);
        }

        let expires_at = now + TARGET_TTL;
        let mut registered = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let target_id = self.mint_unused_target_id();
            let (diagnostics, fingerprint) = candidate.into_parts();
            self.entries.insert(
                target_id.clone(),
                TargetEntry {
                    caller_binding,
                    expires_at,
                    fingerprint,
                },
            );
            self.caller_targets
                .entry(caller_binding)
                .or_default()
                .insert(target_id.clone());
            self.expiry_index
                .entry(expires_at)
                .or_default()
                .insert(target_id.clone());
            registered.push(RegisteredWindow {
                target_id,
                diagnostics,
            });
        }

        self.debug_assert_invariants();
        Ok(registered)
    }

    fn preflight_for_caller_at(
        &mut self,
        caller_binding: CallerBinding,
        target_id: &WindowTargetId,
        now: Instant,
    ) -> Result<(), WindowCaptureError> {
        if self.expire_requested_for_caller(caller_binding, target_id, now) {
            return Err(WindowCaptureError::StaleTarget);
        }
        self.prune_expired(now);

        if self
            .caller_targets
            .get(&caller_binding)
            .is_some_and(|target_ids| target_ids.contains(target_id))
        {
            Ok(())
        } else {
            Err(WindowCaptureError::TargetNotFound)
        }
    }

    fn expire_requested_for_caller(
        &mut self,
        caller_binding: CallerBinding,
        target_id: &WindowTargetId,
        now: Instant,
    ) -> bool {
        let expired = self
            .entries
            .get(target_id)
            .is_some_and(|entry| entry.caller_binding == caller_binding && entry.expires_at <= now);
        if expired {
            let _ = self.remove_entry(target_id);
        }
        expired
    }

    fn prune_expired(&mut self, now: Instant) {
        loop {
            let Some((&expires_at, _)) = self.expiry_index.first_key_value() else {
                break;
            };
            if expires_at > now {
                break;
            }

            let Some(expired_ids) = self.expiry_index.remove(&expires_at) else {
                continue;
            };
            for target_id in expired_ids {
                let Some(entry) = self.entries.remove(&target_id) else {
                    continue;
                };
                debug_assert_eq!(entry.expires_at, expires_at);
                if let Some(caller_ids) = self.caller_targets.get_mut(&entry.caller_binding) {
                    caller_ids.remove(&target_id);
                    if caller_ids.is_empty() {
                        self.caller_targets.remove(&entry.caller_binding);
                    }
                }
            }
        }
        self.debug_assert_invariants();
    }

    fn mint_unused_target_id(&self) -> WindowTargetId {
        loop {
            let target_id = WindowTargetId::mint();
            if !self.entries.contains_key(&target_id) {
                return target_id;
            }
        }
    }

    fn remove_entry(&mut self, target_id: &WindowTargetId) -> Option<TargetEntry> {
        let entry = self.entries.remove(target_id)?;
        if let Some(expiry_ids) = self.expiry_index.get_mut(&entry.expires_at) {
            expiry_ids.remove(target_id);
            if expiry_ids.is_empty() {
                self.expiry_index.remove(&entry.expires_at);
            }
        }
        if let Some(caller_ids) = self.caller_targets.get_mut(&entry.caller_binding) {
            caller_ids.remove(target_id);
            if caller_ids.is_empty() {
                self.caller_targets.remove(&entry.caller_binding);
            }
        }
        self.debug_assert_invariants();
        Some(entry)
    }

    fn debug_assert_invariants(&self) {
        debug_assert_eq!(
            self.entries.len(),
            self.caller_targets
                .values()
                .map(BTreeSet::len)
                .sum::<usize>()
        );
        debug_assert_eq!(
            self.entries.len(),
            self.expiry_index.values().map(BTreeSet::len).sum::<usize>()
        );
        for (target_id, entry) in &self.entries {
            debug_assert!(self
                .caller_targets
                .get(&entry.caller_binding)
                .is_some_and(|target_ids| target_ids.contains(target_id)));
            debug_assert!(self
                .expiry_index
                .get(&entry.expires_at)
                .is_some_and(|target_ids| target_ids.contains(target_id)));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{CallerBinding, WindowTargetRegistry};
    use crate::window_capture::{fixture_discovered_window, WindowCaptureError};

    fn caller(byte: u8) -> CallerBinding {
        CallerBinding::from_digest([byte; 32])
    }

    #[test]
    fn replacement_is_caller_scoped_and_invalidates_prior_ids() {
        let now = Instant::now();
        let mut registry = WindowTargetRegistry::new();
        let first = registry
            .replace_for_caller_at(caller(1), vec![fixture_discovered_window(1)], now)
            .unwrap();
        let other = registry
            .replace_for_caller_at(caller(2), vec![fixture_discovered_window(2)], now)
            .unwrap();
        let replacement = registry
            .replace_for_caller_at(caller(1), vec![fixture_discovered_window(3)], now)
            .unwrap();

        assert!(matches!(
            registry.preflight_for_caller_at(caller(1), &first[0].target_id, now),
            Err(WindowCaptureError::TargetNotFound)
        ));
        assert!(registry
            .preflight_for_caller_at(caller(2), &other[0].target_id, now)
            .is_ok());
        assert!(registry
            .preflight_for_caller_at(caller(1), &replacement[0].target_id, now)
            .is_ok());
    }

    #[test]
    fn expired_target_is_stale_only_for_its_caller() {
        let now = Instant::now();
        let mut registry = WindowTargetRegistry::new();
        let registered = registry
            .replace_for_caller_at(caller(1), vec![fixture_discovered_window(1)], now)
            .unwrap();
        let target_id = registered[0].target_id.clone();

        assert!(matches!(
            registry.preflight_for_caller_at(caller(1), &target_id, now + Duration::from_secs(60)),
            Err(WindowCaptureError::StaleTarget)
        ));
        assert!(matches!(
            registry.preflight_for_caller_at(caller(2), &target_id, now),
            Err(WindowCaptureError::TargetNotFound)
        ));
    }

    #[test]
    fn capacity_failure_keeps_the_callers_prior_targets() {
        let now = Instant::now();
        let mut registry = WindowTargetRegistry {
            max_callers: 1,
            max_entries: 2,
            max_entries_per_caller: 2,
            ..WindowTargetRegistry::new()
        };
        let first = registry
            .replace_for_caller_at(caller(1), vec![fixture_discovered_window(1)], now)
            .unwrap();

        assert!(matches!(
            registry.replace_for_caller_at(caller(2), vec![fixture_discovered_window(2)], now),
            Err(WindowCaptureError::CaptureBusy)
        ));
        assert!(registry
            .preflight_for_caller_at(caller(1), &first[0].target_id, now)
            .is_ok());
    }

    #[test]
    fn consumption_is_single_use() {
        let now = Instant::now();
        let mut registry = WindowTargetRegistry::new();
        let registered = registry
            .replace_for_caller_at(caller(1), vec![fixture_discovered_window(1)], now)
            .unwrap();
        let target_id = registered[0].target_id.clone();

        assert!(registry.consume_for_caller(caller(1), &target_id).is_ok());
        assert!(matches!(
            registry.consume_for_caller(caller(1), &target_id),
            Err(WindowCaptureError::TargetNotFound)
        ));
    }
}
