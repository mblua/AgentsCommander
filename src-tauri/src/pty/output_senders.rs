//! `OutputSenderMap`: the shared per-session map of mpsc senders the PTY read loop uses to
//! copy output to an active bridge.
//! Type alias only, no logic.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use uuid::Uuid;

/// Shared map of `session_id` → mpsc sender. The PTY read loop checks this
/// to clone bytes to an active bridge. Uses `std::sync::Mutex` because the
/// PTY read loop runs on a `std::thread`, not tokio.
pub type OutputSenderMap = Arc<Mutex<HashMap<Uuid, tokio::sync::mpsc::Sender<Vec<u8>>>>>;
