use std::time::Duration;

mod process;
mod publish;
mod request;
mod scanner;
mod shutdown;

#[cfg(test)]
use process::publish_trusted_failure;
#[cfg(test)]
use publish::{path_is_absent, protocol_cleanup_name, publish_response_bytes, safe_remove};
pub(crate) use request::confirmation_tag;
pub(crate) use request::HostTerminalSnapshotRequest;
pub(crate) use scanner::verified_requester_root_from_discovered_path;
pub(crate) use scanner::SnapshotMailboxScanner;
pub(crate) use shutdown::SnapshotScannerDrainResult;
pub(crate) use shutdown::SnapshotScannerShutdownOwner;
#[cfg(test)]
pub(crate) use shutdown::SnapshotScannerTaskStartControl;

const REQUEST_KIND: &str = "terminal-snapshot";
const REQUEST_VERSION: u32 = 1;
const REQUEST_DIRECTORY: &str = "terminal-snapshot-requests";
const RESPONSE_DIRECTORY: &str = "terminal-snapshot-responses";
const RESPONSE_TTL: Duration = Duration::from_secs(60);
const MAX_DIRECTORY_ENTRIES: usize = 4_096;
const MAX_PER_POLL: usize = 256;
const CURSOR_CAP: usize = 4_096;

#[cfg(test)]
mod tests;
