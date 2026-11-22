//! Git child processes, split by purpose into submodules: `cache`, `context`,
//! `mutation` (clone/reset and deletion probes), `read` (discovery and inspection),
//! `remote` and `watcher` (repository watching). This file only declares them.

pub(crate) mod cache;
pub(crate) mod context;
pub(crate) mod mutation;
pub(crate) mod read;
pub(crate) mod remote;
pub(crate) mod watcher;
