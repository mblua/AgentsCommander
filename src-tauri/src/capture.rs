// #2232 phase 3: the sink, its state and the per-session owners join phase 1's
// record. Every module here is a leaf: the phone, session and commands
// subtrees, and the teams and settings config modules, may not be named from
// this subtree, which is what keeps the 88-module SCC from growing.
pub(crate) mod armed;
pub(crate) mod key;
pub(crate) mod record;
pub(crate) mod registry;
pub(crate) mod sink;
pub mod state;
// #2232 phase 5: the Codex turn-boundary state machine the watcher now feeds.
pub mod turn_codex;
// #2232 phase 6: the user catalog, the single typed Jev call and the
// pre-egress secret detector. Every one of the three is a leaf and adds no
// path into the 88-module SCC.
pub(crate) mod catalog;
pub(crate) mod jev;
pub(crate) mod secrets;
