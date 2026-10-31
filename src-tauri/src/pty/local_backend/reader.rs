#[cfg(all(test, windows))]
use super::cursor_query::CursorReplyObserver;
#[cfg(windows)]
use super::cursor_query::{pump_cursor_query, write_cursor_reply};
use super::LocalProcessBackend;
use crate::pty::output::PtyOutputRegistrationToken;
use crate::pty::spawn_diagnostics::SpawnRecord;
use std::io::Read;
#[cfg(windows)]
use std::io::Write;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[cfg(windows)]
use std::sync::{Mutex, Weak};
#[cfg(windows)]
use std::time::Instant;
use uuid::Uuid;

pub(crate) struct OutputReader {
    pub(crate) reader: Box<dyn Read + Send>,
    pub(crate) record: Arc<SpawnRecord>,
    pub(crate) rendered: Arc<AtomicBool>,
    pub(crate) output_token: PtyOutputRegistrationToken,
    #[cfg(windows)]
    pub(crate) cursor_writer: Weak<Mutex<Box<dyn Write + Send>>>,
    #[cfg(all(test, windows))]
    pub(crate) cursor_observer: Option<Arc<CursorReplyObserver>>,
}

impl OutputReader {
    pub(crate) fn start(self, backend: LocalProcessBackend, id: Uuid) {
        let Self {
            mut reader,
            record,
            rendered,
            output_token,
            #[cfg(windows)]
            cursor_writer,
            #[cfg(all(test, windows))]
            cursor_observer,
        } = self;
        let session_id_str = id.to_string();
        let fanout = backend.fanout.clone();
        let gate_backend = backend;
        std::thread::spawn(move || {
            #[cfg(windows)]
            pump_cursor_query(
                &mut reader,
                |bytes| {
                    let read_at = Instant::now();
                    record.note_output(bytes);
                    fanout.observe_raw_read_for_reader(&output_token, bytes.len(), read_at)
                },
                |bytes| {
                    fanout.handle_output(&output_token, &session_id_str, bytes.to_vec());
                    if !rendered.load(Ordering::Relaxed) && fanout.has_rendered_visible_content(id)
                    {
                        gate_backend.open_startup_gate(id);
                    }
                },
                || {
                    write_cursor_reply(
                        &cursor_writer,
                        fanout.cursor_position_for_reader(&output_token),
                    )
                },
                #[cfg(test)]
                cursor_observer.as_deref(),
            );
            #[cfg(not(windows))]
            {
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            // #942 - time-to-first-output and the retained head bytes. Hot
                            // path: once the first byte is stamped and the head buffer is
                            // full this is two relaxed loads and one relaxed add.
                            record.note_output(&buf[..n]);
                            fanout.handle_output(&output_token, &session_id_str, buf[..n].to_vec());
                            // #973 (B) - has the child PAINTED anything yet? Asked of the real
                            // vt100 parser that `handle_output` has just fed this chunk to, which
                            // is why it is asked after it and not before.
                            //
                            // Once the child has painted, this is a single relaxed load per chunk
                            // and nothing else: no lock, no scan, no allocation. Before it has, it
                            // is one uncontended lock and a bounded scan of the grid - and it
                            // REPLACED a second `from_utf8_lossy` + `strip_ansi_csi` over the whole
                            // chunk, which `handle_output` was already paying for the idle detector
                            // on the very same bytes. One less chunk-sized allocation per chunk,
                            // not one more.
                            //
                            // Locks: the query takes `screen_parsers` and RELEASES it before
                            // `open_startup_gate` takes `ptys`. The two are sequential, never
                            // nested, so the order stays `ptys -> startup_gate -> size -> master`.
                            if !rendered.load(Ordering::Relaxed)
                                && fanout.has_rendered_visible_content(id)
                            {
                                gate_backend.open_startup_gate(id);
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        });
    }
}
