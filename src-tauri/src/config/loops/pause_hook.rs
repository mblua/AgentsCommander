//! #2682 - the Loop write pause point shared by `lock` and `storage`.
//! Inert in production; under test it forwards to the `pause` handshake.

use std::path::Path;

#[cfg(not(test))]
#[inline(always)]
pub(super) fn loop_write_pause_hook(_stage: &str, _dir: &Path) {}

/// #2682 - test-only pause point. Inert unless a test arms it.
#[cfg(test)]
pub(super) fn loop_write_pause_hook(stage: &str, dir: &Path) {
    crate::config::loops::pause::hook(stage, dir);
}
