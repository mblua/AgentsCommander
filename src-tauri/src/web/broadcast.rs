use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

const WS_CLIENT_QUEUE_CAP: usize = 1024;

/// Message types sent to WebSocket clients.
#[derive(Clone, Debug)]
pub enum WsOutMsg {
    /// JSON text frame (events, command responses)
    Text(String),
    /// Binary frame (PTY output: 36-byte UUID prefix + raw bytes)
    Binary(Vec<u8>),
}

/// Fan-out broadcaster for WebSocket clients.
/// Thread-safe (Mutex-based) so it can be called from native PTY read threads.
#[derive(Clone)]
pub struct WsBroadcaster {
    senders: Arc<Mutex<Vec<mpsc::Sender<WsOutMsg>>>>,
}

impl Default for WsBroadcaster {
    fn default() -> Self {
        Self {
            senders: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl WsBroadcaster {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new client's sender. Returns the receiver for the WS write loop.
    pub fn subscribe(&self) -> mpsc::Receiver<WsOutMsg> {
        let (tx, rx) = mpsc::channel(WS_CLIENT_QUEUE_CAP);
        let mut senders = self.senders.lock().unwrap();
        senders.push(tx);
        rx
    }

    /// Broadcast a JSON event to all connected WS clients.
    /// Dead senders are automatically cleaned up via retain().
    pub fn broadcast_event(&self, event: &str, payload: &serde_json::Value) {
        let msg = serde_json::json!({ "event": event, "payload": payload });
        let text = msg.to_string();
        let out = WsOutMsg::Text(text);

        let mut senders = self.senders.lock().unwrap();
        senders.retain(|tx| tx.try_send(out.clone()).is_ok());
    }

    /// Broadcast PTY output as binary frame: 36-byte UUID ASCII + raw bytes.
    /// Called from native PTY read thread — must be non-blocking.
    pub fn broadcast_pty_output(&self, session_id: &str, data: &[u8]) {
        let mut frame = Vec::with_capacity(36 + data.len());
        // Pad or truncate session_id to exactly 36 bytes
        let id_bytes = session_id.as_bytes();
        if id_bytes.len() >= 36 {
            frame.extend_from_slice(&id_bytes[..36]);
        } else {
            frame.extend_from_slice(id_bytes);
            frame.resize(36, b' ');
        }
        frame.extend_from_slice(data);

        let out = WsOutMsg::Binary(frame);
        let mut senders = self.senders.lock().unwrap();
        senders.retain(|tx| tx.try_send(out.clone()).is_ok());
    }

    /// Number of connected clients (for diagnostics).
    pub fn client_count(&self) -> usize {
        self.senders.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_slow_client_when_queue_is_full() {
        let broadcaster = WsBroadcaster::new();
        let _slow_rx = broadcaster.subscribe();
        assert_eq!(broadcaster.client_count(), 1);

        for n in 0..WS_CLIENT_QUEUE_CAP {
            broadcaster.broadcast_event("tick", &serde_json::json!({ "n": n }));
            assert_eq!(broadcaster.client_count(), 1);
        }

        broadcaster.broadcast_event("tick", &serde_json::json!({ "n": "overflow" }));
        assert_eq!(broadcaster.client_count(), 0);
    }
}
