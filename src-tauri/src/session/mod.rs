pub mod manager;
// Inner module shares the parent name; renaming would churn every import.
#[allow(clippy::module_inception)]
pub mod session;
pub mod profile;
