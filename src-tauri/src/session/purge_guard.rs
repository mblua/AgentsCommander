use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

/// (#885) Cross-cutting gate for `purge-wg`. Two jobs:
///
/// J1: no daemon-mediated input reaches a peer between the readiness snapshot
///     and that peer's destroy.
/// J2: no in-flight wake resurrects a peer the purge already destroyed.
///     `deliver_wake` falls through to spawn-persistent when a peer has no
///     SessionManager record, so a wake landing mid-loop would cold-spawn an
///     agent we are purging and the verb would report a success it did not
///     achieve. J2 alone justifies this type.
///
/// The `tokio::sync::Mutex` serializes concurrent purges. The `AtomicBool`
/// lets the actuation choke points do a non-blocking load and refuse fast,
/// rather than parking a Tauri command behind the destroy loop.
///
/// The target sets are carried so that sessions OUTSIDE the purge scope keep
/// working. A global block held across a `--graceful --timeout 30` purge would
/// freeze typing app-wide for 30 seconds.
#[derive(Default)]
pub struct PurgeGuard {
    active: AtomicBool,
    target_sids: std::sync::RwLock<HashSet<Uuid>>,
    target_fqns: std::sync::RwLock<HashSet<String>>,
    lock: tokio::sync::Mutex<()>,
}

pub struct PurgeLease<'a> {
    guard: &'a PurgeGuard,
    _mutex: tokio::sync::MutexGuard<'a, ()>,
}

impl PurgeGuard {
    /// True while any purge holds a lease. Used by the #791 DB dispatcher,
    /// which must skip its tick BEFORE leasing a row and therefore does not
    /// yet know the row's target (§5.5e, F-5).
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    /// Fast path: one atomic load when no purge is running.
    pub fn blocks_session(&self, sid: Uuid) -> bool {
        self.is_active()
            && self
                .target_sids
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&sid)
    }

    /// Wake delivery targets an FQN, and J2 is precisely the case where the
    /// session record is already gone, so this cannot be keyed on a Uuid.
    pub fn blocks_agent(&self, fqn: &str) -> bool {
        self.is_active()
            && self
                .target_fqns
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .contains(fqn)
    }

    /// Acquire exclusive purge rights. Blocks behind any in-flight purge.
    pub async fn acquire(&self, sids: HashSet<Uuid>, fqns: HashSet<String>) -> PurgeLease<'_> {
        let mutex = self.lock.lock().await;
        *self.target_sids.write().unwrap_or_else(|e| e.into_inner()) = sids;
        *self.target_fqns.write().unwrap_or_else(|e| e.into_inner()) = fqns;
        self.active.store(true, Ordering::SeqCst);
        PurgeLease {
            guard: self,
            _mutex: mutex,
        }
    }
}

impl Drop for PurgeLease<'_> {
    fn drop(&mut self) {
        self.guard.active.store(false, Ordering::SeqCst);
        self.guard
            .target_sids
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.guard
            .target_fqns
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn purge_guard_blocks_only_targets() {
        let guard = PurgeGuard::default();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let fqn_a = "proj:wg-1-devs/alice".to_string();
        let fqn_b = "proj:wg-1-devs/bob".to_string();

        let mut sids = HashSet::new();
        sids.insert(a);
        let mut fqns = HashSet::new();
        fqns.insert(fqn_a.clone());

        let lease = guard.acquire(sids, fqns).await;

        assert!(guard.blocks_session(a));
        assert!(!guard.blocks_session(b));
        assert!(guard.blocks_agent(&fqn_a));
        assert!(!guard.blocks_agent(&fqn_b));

        drop(lease);
        assert!(!guard.blocks_session(a));
        assert!(!guard.blocks_agent(&fqn_a));
    }

    #[tokio::test]
    async fn purge_guard_serializes_concurrent_purges() {
        let guard = Arc::new(PurgeGuard::default());
        let g1 = Arc::clone(&guard);
        let g2 = Arc::clone(&guard);

        let h1 = tokio::spawn(async move {
            let _lease = g1.acquire(HashSet::new(), HashSet::new()).await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        });
        let h2 = tokio::spawn(async move {
            // This must wait for h1's lease to drop.
            let _lease = g2.acquire(HashSet::new(), HashSet::new()).await;
        });

        let (r1, r2) = tokio::join!(h1, h2);
        assert!(r1.is_ok());
        assert!(r2.is_ok());
        // After both complete, no lease is held.
        assert!(!guard.is_active());
    }
}
