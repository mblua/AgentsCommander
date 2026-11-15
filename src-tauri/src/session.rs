pub(crate) mod auto_close;
pub(crate) mod context_alerts;
pub(crate) mod context_sample_queue;
pub(crate) mod create_contracts;
pub(crate) mod maintenance;
pub mod manager;
// Inner module shares the parent name; renaming would churn every import.
pub mod profile;
pub(crate) mod purge_guard;
pub(crate) mod remote_alerts;
pub mod selection;
#[allow(clippy::module_inception)]
pub mod session;
pub(crate) mod status;
pub(crate) mod warnings;
