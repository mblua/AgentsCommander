// #2232 phase 3: the sink, its state and the per-session owners join phase 1's
// record. Every module here is a leaf: the phone, session and commands
// subtrees, and the teams and settings config modules, may not be named from
// this subtree, which is what keeps the 88-module SCC from growing.
pub mod armed;
pub mod key;
pub mod record;
pub mod registry;
pub mod sink;
pub mod state;
// #2232 phase 5: the Codex turn-boundary state machine the watcher now feeds.
pub mod turn_codex;
