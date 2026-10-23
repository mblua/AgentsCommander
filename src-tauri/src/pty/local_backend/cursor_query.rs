use std::io::{Read, Write};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

// #3029: a single startup transaction, owned by the existing Windows reader.
#[cfg(windows)]
#[derive(Clone, Copy)]
pub(super) enum CursorQueryState {
    Ground,
    Escape,
    Csi,
    String(bool), // OSC alone also accepts BEL.
    StringEscape(bool),
}

#[cfg(windows)]
pub(super) struct OneShotCursorQuery {
    pub(super) state: CursorQueryState,
    pub(super) pending: Vec<u8>,
    pub(super) armed: bool,
}

#[cfg(windows)]
impl OneShotCursorQuery {
    pub(super) fn new() -> Self {
        Self {
            state: CursorQueryState::Ground,
            pending: Vec::with_capacity(3),
            armed: true,
        }
    }

    // The callback sees each preceding segment before the query event and the tail after it.
    pub(super) fn feed(&mut self, bytes: &[u8], mut emit: impl FnMut(&[u8], bool)) {
        if !self.armed {
            if !bytes.is_empty() {
                emit(bytes, false);
            }
            return;
        }
        let mut output = Vec::with_capacity(bytes.len() + 3);
        for &byte in bytes {
            if !self.armed {
                output.push(byte);
                continue;
            }
            match (self.pending.len(), byte) {
                (1, b'[') => {
                    self.pending.push(byte);
                    self.state = CursorQueryState::Csi;
                    continue;
                }
                (2, b'6') => {
                    self.pending.push(byte);
                    continue;
                }
                (3, b'n') => {
                    self.pending.clear();
                    self.armed = false;
                    if !output.is_empty() {
                        emit(&output, false);
                        output.clear();
                    }
                    emit(&[], true);
                    continue;
                }
                _ => output.append(&mut self.pending),
            }
            use CursorQueryState::*;
            self.state = match self.state {
                String(_) | StringEscape(_) if byte == 0x18 || byte == 0x1a => Ground,
                String(osc) | StringEscape(osc) if byte == 0x07 && osc => Ground,
                String(osc) if byte == 0x1b => StringEscape(osc),
                StringEscape(_) if byte == 0x5c => Ground,
                StringEscape(osc) if byte == 0x1b => StringEscape(osc),
                String(osc) | StringEscape(osc) => String(osc),
                _ if byte == 0x18 || byte == 0x1a => Ground,
                _ if byte == 0x1b => {
                    self.pending.push(byte);
                    Escape
                }
                Escape => match byte {
                    b']' => String(true),
                    b'P' | b'X' | b'^' | b'_' => String(false),
                    b'[' => Csi,
                    _ => Ground,
                },
                Csi if (0x40..=0x7e).contains(&byte) => Ground,
                Csi => Csi,
                Ground => Ground,
            };
            if byte != 0x1b || matches!(self.state, String(_) | StringEscape(_)) {
                output.push(byte);
            }
        }
        if !output.is_empty() {
            emit(&output, false);
        }
    }

    pub(super) fn finish(&mut self, mut output: impl FnMut(&[u8])) {
        if !self.pending.is_empty() {
            output(&self.pending);
            self.pending.clear();
        }
    }
}

#[cfg(all(test, windows))]
#[derive(Default)]
pub(super) struct CursorReplyObserver {
    pub(super) consumed: AtomicUsize,
    pub(super) written: AtomicUsize,
    pub(super) finished: AtomicBool,
}

#[cfg(windows)]
pub(super) fn write_cursor_reply(
    writer: &std::sync::Weak<Mutex<Box<dyn Write + Send>>>,
    position: Option<(u16, u16)>,
) -> std::io::Result<()> {
    let (row, col) = position.ok_or_else(|| std::io::Error::other("cursor unavailable"))?;
    let writer = writer
        .upgrade()
        .ok_or_else(|| std::io::Error::other("cursor writer retired"))?;
    let mut writer = writer
        .lock()
        .map_err(|_| std::io::Error::other("cursor writer poisoned"))?;
    writer.write_all(format!("\x1b[{row};{col}R").as_bytes())?;
    writer.flush()
}

#[cfg(windows)]
pub(super) fn pump_cursor_query(
    reader: &mut dyn Read,
    mut raw: impl FnMut(&[u8]) -> bool,
    mut output: impl FnMut(&[u8]),
    mut reply: impl FnMut() -> std::io::Result<()>,
    #[cfg(test)] observer: Option<&CursorReplyObserver>,
) {
    let mut query = OneShotCursorQuery::new();
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if !raw(&buf[..n]) {
                    continue;
                }
                query.feed(&buf[..n], |segment, consumed| {
                    if consumed {
                        #[cfg(test)]
                        if let Some(observer) = observer {
                            observer.consumed.fetch_add(1, Ordering::SeqCst);
                        }
                        match reply() {
                            Ok(()) =>
                            {
                                #[cfg(test)]
                                if let Some(observer) = observer {
                                    observer.written.fetch_add(1, Ordering::SeqCst);
                                }
                            }
                            Err(error) => log::warn!(
                                "[pty] startup cursor reply failed: {error}; \
                                    retaining drainer and Stop"
                            ),
                        }
                    } else {
                        output(segment);
                    }
                });
            }
        }
    }
    query.finish(&mut output);
    #[cfg(test)]
    if let Some(observer) = observer {
        observer.finished.store(true, Ordering::SeqCst);
    }
}
