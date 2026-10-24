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
mod tests;
