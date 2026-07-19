pub mod auto_close;
pub mod context_alerts;
pub mod manager;
// Inner module shares the parent name; renaming would churn every import.
pub mod profile;
pub mod purge_guard;
pub mod selection;
#[allow(clippy::module_inception)]
pub mod session;
pub mod warnings;
