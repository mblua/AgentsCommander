//! #1171 - layer 2 of the three deduplication layers: key suppression with a TTL.
//!
//! Layer 1 is the frame diff, and it does the work: a logical row is evaluated on the one tick
//! it becomes final. This layer exists only for the residual cases layer 1 cannot separate -
//! differently truncated repeats of the same content, and the statusline of a TUI using a
//! scroll region, which falls inside the scrolled-in range on every scrolling tick.
//!
//! Layer 3 is `possibly_missed_frames`, which is not deduplication at all: it declares
//! uncertainty instead of hiding it.
//!
//! Applies to `occurrence` only. `state` has its generation gate.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// At most this many keys per `(watcher, session)`, oldest-inserted evicted first.
///
/// Without this bound AND the 60 s clamp on the window, a `row` key would grow the key set to
/// every distinct row seen inside the window, which at 8 watchers and 20 sessions is hundreds
/// of megabytes.
pub const MAX_KEYS: usize = 256;

/// The keys one `(watcher, session)` pair has seen recently.
#[derive(Default)]
pub struct DedupeKeys {
    /// key -> when it was last admitted.
    seen: HashMap<String, Instant>,
    /// Insertion order, for eviction. Only ever holds keys present in `seen`.
    order: VecDeque<String>,
}

impl DedupeKeys {
    /// Should this match be let through?
    ///
    /// The window runs from the moment a key was ADMITTED, not from the last time it was seen:
    /// a sliding window would suppress a saturated watcher forever, where this one lets it
    /// through again as soon as the window expires. That is what makes the window a
    /// deduplication window rather than a mute button.
    pub fn admit(&mut self, key: &str, now: Instant, window: Duration) -> bool {
        if let Some(admitted) = self.seen.get(key) {
            if now.duration_since(*admitted) < window {
                return false;
            }
        } else {
            self.order.push_back(key.to_string());
            while self.order.len() > MAX_KEYS {
                if let Some(evicted) = self.order.pop_front() {
                    self.seen.remove(&evicted);
                }
            }
        }
        self.seen.insert(key.to_string(), now);
        true
    }

    /// Drop keys whose window has passed. Called once per tick for the sessions evaluated that
    /// tick, so a quiet session costs nothing and a busy one cannot accumulate.
    pub fn prune(&mut self, now: Instant, window: Duration) {
        self.seen
            .retain(|_, admitted| now.duration_since(*admitted) < window);
        self.order.retain(|key| self.seen.contains_key(key));
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Duration {
        Duration::from_millis(2000)
    }

    /// 9.4.52 (the layer's half) - the same key inside the window is one event, and the same
    /// key outside it is another.
    #[test]
    fn a_repeated_key_is_suppressed_inside_the_window_and_admitted_outside_it() {
        let mut keys = DedupeKeys::default();
        let start = Instant::now();

        assert!(keys.admit("C:/repo/main.rs", start, window()));
        assert!(!keys.admit(
            "C:/repo/main.rs",
            start + Duration::from_millis(1),
            window()
        ));
        assert!(!keys.admit(
            "C:/repo/main.rs",
            start + Duration::from_millis(1999),
            window()
        ));
        assert!(keys.admit(
            "C:/repo/main.rs",
            start + Duration::from_millis(2001),
            window()
        ));
    }

    /// Suppression does not extend itself. A key hit repeatedly inside the window still comes
    /// back at the end of it, rather than being muted for as long as the agent keeps printing.
    #[test]
    fn repeated_suppression_does_not_slide_the_window_forward() {
        let mut keys = DedupeKeys::default();
        let start = Instant::now();
        assert!(keys.admit("k", start, window()));

        for ms in [500, 1000, 1500, 1900] {
            assert!(!keys.admit("k", start + Duration::from_millis(ms), window()));
        }
        assert!(
            keys.admit("k", start + Duration::from_millis(2100), window()),
            "the window runs from admission, not from the last suppressed hit"
        );
    }

    /// Different keys do not suppress each other, which is the whole reason the key is
    /// configurable: `capture` collapses two differently truncated rows, `row` does not.
    #[test]
    fn different_keys_are_independent() {
        let mut keys = DedupeKeys::default();
        let now = Instant::now();

        assert!(keys.admit("a", now, window()));
        assert!(keys.admit("b", now, window()));
        assert!(!keys.admit("a", now, window()));
        assert_eq!(keys.len(), 2);
    }

    /// 9.4.53 - the map never exceeds its bound, and the OLDEST key is the one that goes.
    #[test]
    fn the_key_set_is_bounded_and_evicts_the_oldest_first() {
        let mut keys = DedupeKeys::default();
        let now = Instant::now();

        for i in 0..MAX_KEYS + 50 {
            assert!(keys.admit(&format!("row {i}"), now, window()));
        }

        assert_eq!(keys.len(), MAX_KEYS);
        assert!(
            keys.admit("row 0", now, window()),
            "the oldest key was evicted, so it is new again"
        );
        assert!(
            !keys.admit(&format!("row {}", MAX_KEYS + 49), now, window()),
            "the newest key is still remembered"
        );
    }

    /// 9.4.53 - expired keys are pruned rather than waiting for eviction pressure, so a
    /// session that goes quiet does not hold 256 strings for the rest of its life.
    #[test]
    fn pruning_drops_expired_keys_and_keeps_live_ones() {
        let mut keys = DedupeKeys::default();
        let start = Instant::now();
        keys.admit("old", start, window());
        keys.admit("new", start + Duration::from_millis(1900), window());

        keys.prune(start + Duration::from_millis(2100), window());

        assert_eq!(keys.len(), 1);
        assert!(
            !keys.admit("new", start + Duration::from_millis(2100), window()),
            "the live key survived the prune"
        );
    }

    /// The eviction order list must never outlive the keys it names, or the bound would be
    /// enforced against a list full of ghosts.
    #[test]
    fn pruning_keeps_the_eviction_order_and_the_key_set_in_step() {
        let mut keys = DedupeKeys::default();
        let start = Instant::now();
        for i in 0..10 {
            keys.admit(&format!("k{i}"), start, window());
        }

        keys.prune(start + Duration::from_millis(3000), window());
        assert!(keys.is_empty());
        assert_eq!(keys.order.len(), 0);

        for i in 0..MAX_KEYS + 10 {
            keys.admit(
                &format!("later {i}"),
                start + Duration::from_millis(3000),
                window(),
            );
        }
        assert_eq!(keys.len(), MAX_KEYS);
        assert_eq!(keys.order.len(), MAX_KEYS);
    }
}
