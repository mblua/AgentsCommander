//! Turn capture: the per-session sink, its state and owners, the Codex
//! turn-boundary machine, the user catalog, the `Jev` call and the pre-egress
//! secret detector.

#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

// The sink, its state and the per-session owners join the record.
// record. Every module here is a leaf: the phone, session and commands
// subtrees, and the teams and settings config modules, may not be named from
// this subtree, which is what keeps the 88-module SCC from growing.
pub(crate) mod armed;
pub(crate) mod key;
pub(crate) mod record;
pub(crate) mod registry;
pub(crate) mod sink;
pub(crate) mod state;
// The Codex turn-boundary state machine the watcher feeds.
pub(crate) mod turn_codex;
// The user catalog, the single typed Jev call and the
// pre-egress secret detector. Every one of the three is a leaf and adds no
// path into the 88-module SCC.
pub(crate) mod catalog;
pub(crate) mod jev;
pub(crate) mod secrets;
