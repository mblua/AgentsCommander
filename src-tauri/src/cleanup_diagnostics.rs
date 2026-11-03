//! Durable evidence for the individual reaper, excluding PTY/Job ownership.
// P01 deliberately exposes no production entry point; later phases wire these APIs.
#[allow(dead_code)]
pub(crate) mod operation;
#[allow(dead_code)]
pub(crate) mod schema;
#[allow(dead_code)]
pub(crate) mod store;
