use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::pty::idle_detector::IdleDetector;
use crate::session::profile::IdleTuning;
use crate::telegram::manager::OutputSenderMap;

/// Tracks active response marker watchers per session.
/// Key: (session_id, request_id) -> accumulated output buffer.
/// The read loop scans for %%AC_RESPONSE::<rid>::START/END%% markers.
pub type ResponseWatcherMap = Arc<Mutex<HashMap<(Uuid, String), ResponseWatcher>>>;

pub struct ResponseWatcher {
    pub response_dir: std::path::PathBuf,
    pub buffer: Option<String>,
    pub capturing: bool,
}

#[derive(Clone)]
pub struct SessionIoFanout {
    output_senders: OutputSenderMap,
    idle_detector: Arc<IdleDetector>,
    response_watchers: ResponseWatcherMap,
    ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
    screen_parsers: Arc<Mutex<HashMap<Uuid, ScreenReplayState>>>,
}

pub struct PtyScreenSnapshot {
    pub data: Vec<u8>,
    pub rows: u16,
    pub cols: u16,
    pub sequence: u64,
}

struct ScreenReplayState {
    parser: vt100::Parser,
    output_sequence: u64,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PtyOutputPayload {
    session_id: String,
    data: Vec<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sequence: Option<u64>,
}

#[cfg(test)]
pub(crate) type PtyOutputTestEvent = (String, Vec<u8>, Option<u64>);

#[cfg(test)]
pub(crate) type PtyOutputTestSink = Arc<Mutex<Vec<PtyOutputTestEvent>>>;

#[derive(Clone)]
pub struct PtyOutputTarget {
    emit_pty_output: Arc<dyn Fn(PtyOutputPayload) + Send + Sync>,
}

impl PtyOutputTarget {
    pub fn from_app_handle<R: tauri::Runtime>(app_handle: AppHandle<R>) -> Self {
        Self {
            emit_pty_output: Arc::new(move |payload| {
                let _ = app_handle.emit("pty_output", payload);
            }),
        }
    }

    pub fn noop() -> Self {
        Self {
            emit_pty_output: Arc::new(|_| {}),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_test_sink(sink: PtyOutputTestSink) -> Self {
        Self {
            emit_pty_output: Arc::new(move |payload| {
                sink.lock()
                    .unwrap()
                    .push((payload.session_id, payload.data, payload.sequence));
            }),
        }
    }

    fn emit_pty_output(&self, payload: PtyOutputPayload) {
        (self.emit_pty_output)(payload);
    }
}

impl SessionIoFanout {
    pub fn new(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
    ) -> Self {
        Self {
            output_senders,
            idle_detector,
            response_watchers: Arc::new(Mutex::new(HashMap::new())),
            ws_broadcaster,
            screen_parsers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register_session(&self, id: Uuid, idle_tuning: IdleTuning, rows: u16, cols: u16) {
        self.idle_detector.register_session(id, idle_tuning);
        let replay = ScreenReplayState {
            parser: vt100::Parser::new(rows, cols, 0),
            output_sequence: 0,
        };
        self.screen_parsers.lock().unwrap().insert(id, replay);
    }

    pub fn handle_output(
        &self,
        output_target: &PtyOutputTarget,
        id: Uuid,
        session_id_str: &str,
        data: Vec<u8>,
    ) {
        let n = data.len();

        self.idle_detector.touch_silence(id);

        let text = String::from_utf8_lossy(&data);
        if text.contains('\u{FFFD}') {
            log::debug!(
                "[PTY] session {} chunk had invalid UTF-8 at buffer boundary ({} bytes, {} replacement chars)",
                id,
                n,
                text.matches('\u{FFFD}').count()
            );
        }

        if output_has_printable_activity(&text) {
            self.idle_detector.record_activity_with_bytes(id, n);
        } else {
            log::trace!(
                "[idle] SKIPPED activity for {} ({} bytes, escape-only output)",
                &id.to_string()[..8],
                n
            );
        }

        scan_response_markers(id, &text, &self.response_watchers);

        if let Ok(senders) = self.output_senders.lock() {
            if let Some(tx) = senders.get(&id) {
                let _ = tx.try_send(data.clone());
            }
        }

        let sequence = if let Ok(mut parsers) = self.screen_parsers.lock() {
            if let Some(state) = parsers.get_mut(&id) {
                state.parser.process(&data);
                state.output_sequence = state.output_sequence.saturating_add(1);
                Some(state.output_sequence)
            } else {
                None
            }
        } else {
            None
        };

        if let Some(ref bc) = self.ws_broadcaster {
            bc.broadcast_pty_output(session_id_str, &data);
        }

        let payload = PtyOutputPayload {
            session_id: session_id_str.to_string(),
            data,
            sequence,
        };
        output_target.emit_pty_output(payload);
    }

    pub fn record_resize(&self, id: Uuid) {
        self.idle_detector.record_resize(id);
    }

    /// #973 - a degenerate size must never reach the vt100 parser, and this is the boundary
    /// where that invariant actually breaks, so this is where it is enforced.
    ///
    /// `vt100::grid::set_size` computes `size.rows - 1` on a `u16` (grid.rs:73). With
    /// `rows == 0` that UNDERFLOWS: it panics in a debug build and wraps to 65535 in a release
    /// one, leaving a zero-row grid with a scroll region of 65535. And the panic fires while
    /// this function is HOLDING `screen_parsers`, which poisons the mutex - after which
    /// `handle_output`, `get_screen_snapshot` and `get_pty_size` all take the `if let Ok` /
    /// `.ok()?` branch and silently do nothing, for every session, for the life of the process.
    /// One `cols: 0` would take #955's screen snapshot down app-wide, without a single log line.
    ///
    /// The callers guard too (`local_backend` only moves the screen for a size the ConPTY
    /// actually took), but they cannot be the only guard: `container_backend:1099` calls this
    /// with whatever `pty_resize` was given, and there is no gate on that path at all.
    ///
    /// A refused resize did not happen, so the broadcast is skipped with it: clients must not
    /// be told the terminal is 0 columns wide.
    pub fn resize_screen_and_broadcast(&self, id: Uuid, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            log::warn!("[pty] refusing to resize the screen of {id} to {cols}x{rows} (#973)");
            return;
        }

        if let Ok(mut parsers) = self.screen_parsers.lock() {
            if let Some(state) = parsers.get_mut(&id) {
                state.parser.set_size(rows, cols);
            }
        }

        if let Some(ref bc) = self.ws_broadcaster {
            bc.broadcast_event(
                "pty_resized",
                &serde_json::json!({
                    "sessionId": id.to_string(),
                    "cols": cols,
                    "rows": rows,
                }),
            );
        }
    }

    pub fn remove_session(&self, id: Uuid) {
        self.idle_detector.remove_session(id);

        if let Ok(mut watchers) = self.response_watchers.lock() {
            watchers.retain(|(sid, _), _| *sid != id);
        }

        if let Ok(mut parsers) = self.screen_parsers.lock() {
            parsers.remove(&id);
        }
    }

    /// #973 (B) - has the child painted anything a human could see?
    ///
    /// This is the startup gate's trigger, and it asks the REAL, STATEFUL vt100 parser that
    /// `handle_output` has already fed every byte of every chunk. Call it after `handle_output`,
    /// so the chunk that just arrived is in the screen.
    ///
    /// It replaced a per-chunk text predicate (`output_has_printable_activity`, which the idle
    /// detector still uses), and it had to, because that predicate answers a different question
    /// and got this one wrong two ways:
    ///
    /// - **Chunk boundaries.** `strip_ansi_csi` is stateless and carries no residue between
    ///   calls, but the read loop hands it raw `read()` chunks. A chunk that ends mid-CSI or
    ///   mid-OSC delivers the tail in the next chunk with NO leading `ESC`, and `1049h`, `2J` and
    ///   `cmd.exe` are all printable. conhost really does split its output across writes.
    /// - **Three-byte escapes.** It consumed `ESC` plus exactly ONE char. A charset designator is
    ///   three (`ESC ( B`, which ncurses and half the TUI world emit on the way up, plus
    ///   `ESC ) 0` and `ESC % G`), so the third byte survived and read as printable.
    ///
    /// Either one opens the gate on a child that has painted nothing, which is the bug this gate
    /// exists to prevent. A real parser carries state across reads and knows what an escape is,
    /// so both are closed by construction rather than by another special case.
    ///
    /// Returns false when the parser cannot be reached: an unfed parser can never show content
    /// anyway (`handle_output` skips the same poisoned lock), so the gate simply stays shut,
    /// which is the safe direction. It is never resized, and a child showing nothing does not
    /// care what size it is.
    pub fn has_rendered_visible_content(&self, id: Uuid) -> bool {
        let Ok(parsers) = self.screen_parsers.lock() else {
            return false;
        };
        let Some(state) = parsers.get(&id) else {
            return false;
        };
        screen_shows_visible_content(state.parser.screen())
    }

    pub fn get_screen_snapshot(&self, id: Uuid) -> Option<PtyScreenSnapshot> {
        let parsers = self.screen_parsers.lock().ok()?;
        let state = parsers.get(&id)?;
        let screen = state.parser.screen();
        let (rows, cols) = screen.size();
        Some(PtyScreenSnapshot {
            data: screen.contents_formatted(),
            rows,
            cols,
            sequence: state.output_sequence,
        })
    }

    /// #1032 - the live grid's rows, plain text, for the context scrape.
    ///
    /// TWO-state on purpose. The fanout knows nothing about children, so `None` here means
    /// "no parser for this id" and says NOTHING about whether the session is over: only a
    /// backend holds the liveness oracle, so only a backend can make that call.
    ///
    /// Sync, and the rows are cloned out: the guard is a local and is released at the
    /// return, so no caller can hold it across an await.
    pub fn get_screen_rows(&self, id: Uuid) -> Option<Vec<String>> {
        let parsers = self.screen_parsers.lock().ok()?;
        let state = parsers.get(&id)?;
        let screen = state.parser.screen();
        let (_rows, cols) = screen.size();
        Some(screen.rows(0, cols).collect())
    }

    /// #1171 - the same live grid `get_screen_rows` clones, but only when it CHANGED since
    /// the caller last looked, plus the two O(1) facts the watcher engine needs from the same
    /// guard: the per-row wrap flags and the cursor row.
    ///
    /// The unchanged path clones nothing and allocates nothing: `ScreenRowsSince::Unchanged`
    /// carries no rows, so that property is enforced by the type rather than by a comment.
    /// This is what lets a 200 ms sampler cost less than the 5 s one it sits beside.
    ///
    /// `seen` is compared against the WHOLE stamp, sequence and size, because
    /// `resize_screen_and_broadcast` reflows the grid without bumping `output_sequence`
    /// (`:202-212`) - a sequence-only comparison would report `Unchanged` over a screen that
    /// was just re-laid at a new width. `output_sequence` is only READ here; its semantics
    /// are #955's and are untouched.
    ///
    /// TWO-state on the negative side for the same reason `get_screen_rows` is: `Missing`
    /// means "no parser for this id" and says NOTHING about whether the session is over. Only
    /// a backend holds the liveness oracle, so only a backend can turn this into `Gone`.
    ///
    /// Sync, and everything is cloned out: the guard is a local and is released at the return,
    /// so no caller can hold it across an await.
    pub fn get_screen_rows_since(
        &self,
        id: Uuid,
        seen: Option<crate::pty::watchers::FrameStamp>,
    ) -> crate::pty::watchers::ScreenRowsSince {
        use crate::pty::watchers::{FrameStamp, ScreenFrame, ScreenRowsSince};

        let Ok(parsers) = self.screen_parsers.lock() else {
            return ScreenRowsSince::Missing;
        };
        let Some(state) = parsers.get(&id) else {
            return ScreenRowsSince::Missing;
        };
        let screen = state.parser.screen();
        let (grid_rows, cols) = screen.size();
        let stamp = FrameStamp {
            sequence: state.output_sequence,
            rows: grid_rows,
            cols,
        };
        if seen == Some(stamp) {
            return ScreenRowsSince::Unchanged;
        }

        let rows: Vec<String> = screen.rows(0, cols).collect();
        // Indexed off the cloned rows and not off `size().0`, so `wrapped.len() == rows.len()`
        // holds by construction: the frame diff indexes the two together.
        let wrapped: Vec<bool> = (0..rows.len() as u16)
            .map(|row| screen.row_wrapped(row))
            .collect();
        ScreenRowsSince::Frame(ScreenFrame {
            rows,
            wrapped,
            cursor_row: screen.cursor_position().0,
            stamp: Some(stamp),
        })
    }

    pub fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        let parsers = self.screen_parsers.lock().ok()?;
        let state = parsers.get(&id)?;
        Some(state.parser.screen().size())
    }

    /// #1171 - poison `screen_parsers` deterministically, so the "a lock we could not take
    /// makes no claim about the session" arm is covered by a test rather than by reasoning.
    /// Mould: `PtyManager::poison_route_registry_for_test` (`manager.rs:346-355`).
    #[cfg(test)]
    pub(crate) fn poison_screen_parsers_for_test(&self) {
        let parsers = Arc::clone(&self.screen_parsers);
        let result = std::panic::catch_unwind(move || {
            let _guard = parsers.lock().unwrap();
            panic!("poison the screen parser map for deterministic test coverage");
        });
        assert!(result.is_err(), "screen-parser poison fixture must panic");
    }

    pub fn register_response_watcher(
        &self,
        session_id: Uuid,
        request_id: String,
        response_dir: std::path::PathBuf,
    ) {
        if let Ok(mut watchers) = self.response_watchers.lock() {
            watchers.insert(
                (session_id, request_id),
                ResponseWatcher {
                    response_dir,
                    buffer: None,
                    capturing: false,
                },
            );
        }
    }
}

/// #973 (B) - is there a cell on this screen a human could see?
///
/// "Visible" is deliberately not "the child wrote a byte", and not even "a cell has contents":
///
/// - A **space is not content.** `vt100::Cell::has_contents` is true for one (it only asks
///   whether the cell was written), and a TUI painting its still-empty viewport writes plenty.
///   That is the exact moment the gate must stay shut, so the glyph test ignores whitespace.
/// - A **coloured space IS content.** TUIs draw status bars, boxes and selections with nothing
///   but a background colour, and `Cell::clear` KEEPS the attributes, so `ESC[41m ESC[2J` is a
///   red screen holding no contents at all. A human plainly sees it.
/// - **Reverse video** does the same thing with the default colours: a space rendered inverse is
///   a solid block.
///
/// Cost: the attribute scan is allocation-free and exits at the first visible cell.
/// `Screen::contents` is one allocation for the whole grid, where `Cell::contents` would be one
/// per cell. It runs only until the gate opens, and after that the caller's relaxed bool skips
/// it entirely.
fn screen_shows_visible_content(screen: &vt100::Screen) -> bool {
    let (rows, cols) = screen.size();
    for row in 0..rows {
        for col in 0..cols {
            if let Some(cell) = screen.cell(row, col) {
                if cell.inverse() || cell.bgcolor() != vt100::Color::Default {
                    return true;
                }
            }
        }
    }

    screen.contents().chars().any(|c| !c.is_whitespace())
}

/// Strip ANSI escape sequences so marker detection ignores color, cursor,
/// title, hyperlink, shell-integration, and device-control noise.
///
/// #973 - this is the IDLE DETECTOR's predicate, and the startup gate no longer uses it. It is
/// stateless and consumes `ESC` plus exactly one char, which is fine for deciding whether a chunk
/// looked busy and wrong for deciding whether a child has painted. See
/// `SessionIoFanout::has_rendered_visible_content`.
pub(crate) fn strip_ansi_csi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some(&'[') => {
                    chars.next();
                    while let Some(&next) = chars.peek() {
                        chars.next();
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(&']') => {
                    chars.next();
                    while let Some(&ch) = chars.peek() {
                        if ch == '\x07' {
                            chars.next();
                            break;
                        }
                        if ch == '\x1b' {
                            chars.next();
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                                break;
                            }
                            continue;
                        }
                        chars.next();
                    }
                }
                Some(&'P') => {
                    chars.next();
                    while let Some(&ch) = chars.peek() {
                        if ch == '\x1b' {
                            chars.next();
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                                break;
                            }
                            continue;
                        }
                        chars.next();
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub(crate) fn output_has_printable_activity(text: &str) -> bool {
    let is_printable = |c: char| c > ' ' && c != '\u{FFFD}';
    if text.contains('\x1b') {
        strip_ansi_csi(text).chars().any(is_printable)
    } else {
        text.chars().any(is_printable)
    }
}

/// Scan PTY output text for %%AC_RESPONSE::<rid>::START/END%% markers.
pub(crate) fn scan_response_markers(session_id: Uuid, text: &str, watchers: &ResponseWatcherMap) {
    let Ok(mut watchers) = watchers.lock() else {
        return;
    };

    let keys: Vec<(Uuid, String)> = watchers
        .keys()
        .filter(|(sid, _)| *sid == session_id)
        .cloned()
        .collect();

    for key in keys {
        let (_, ref rid) = key;
        let start_marker = format!("%%AC_RESPONSE::{}::START%%", rid);
        let end_marker = format!("%%AC_RESPONSE::{}::END%%", rid);

        let watcher = match watchers.get_mut(&key) {
            Some(w) => w,
            None => continue,
        };

        if watcher.capturing {
            if let Some(end_pos) = text.find(&end_marker) {
                let chunk = &text[..end_pos];
                if let Some(ref mut buf) = watcher.buffer {
                    buf.push_str(chunk);
                }

                let response_content = watcher.buffer.take().unwrap_or_default().trim().to_string();
                write_response_file(&watcher.response_dir, session_id, rid, response_content);
                watchers.remove(&key);
                return;
            } else if let Some(ref mut buf) = watcher.buffer {
                buf.push_str(text);
            }
        } else if let Some(start_pos) = text.find(&start_marker) {
            watcher.capturing = true;
            let after_start = &text[start_pos + start_marker.len()..];

            if let Some(end_pos) = after_start.find(&end_marker) {
                let content = after_start[..end_pos].trim().to_string();
                write_response_file(&watcher.response_dir, session_id, rid, content);
                watchers.remove(&key);
                return;
            } else {
                watcher.buffer = Some(after_start.to_string());
            }
        }
    }
}

fn write_response_file(
    response_dir: &std::path::Path,
    session_id: Uuid,
    request_id: &str,
    content: String,
) {
    let response_path = response_dir.join(format!("{}.json", request_id));
    if let Err(e) = std::fs::create_dir_all(response_dir) {
        log::warn!("Failed to create responses dir: {}", e);
    }

    let response_json = serde_json::json!({
        "requestId": request_id,
        "content": content,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });

    match serde_json::to_string_pretty(&response_json) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&response_path, json) {
                log::warn!("Failed to write response file: {}", e);
            } else {
                log::info!(
                    "Captured response for request {} from session {}",
                    request_id,
                    session_id
                );
            }
        }
        Err(e) => log::warn!("Failed to serialize response: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fanout() -> SessionIoFanout {
        SessionIoFanout::new(
            Arc::new(Mutex::new(HashMap::new())),
            IdleDetector::new(|_| {}, |_| {}),
            None,
        )
    }

    /// A coding agent's TUI on the way up: hide the cursor, clear, switch to the alternate
    /// screen, home, set the title, turn on mouse reporting, reset the attributes. Every byte of
    /// it is an escape sequence. It paints NOTHING a human can see, and the gate must stay shut
    /// through all of it.
    ///
    /// The charset designator a real TUI also emits here (`ESC ( B`) is deliberately NOT in this
    /// constant: it fools the old predicate even UNSPLIT, so putting it here would let the
    /// chunk-boundary test below pass for the wrong reason. It gets its own test.
    const TUI_PROLOGUE: &[u8] =
        b"\x1b[?25l\x1b[2J\x1b[?1049h\x1b[H\x1b]0;cmd.exe\x07\x1b[?1000h\x1b[?1006h\x1b[m";

    fn feed(fanout: &SessionIoFanout, id: Uuid, chunks: &[&[u8]]) {
        for chunk in chunks {
            fanout.handle_output(
                &PtyOutputTarget::noop(),
                id,
                &id.to_string(),
                chunk.to_vec(),
            );
        }
    }

    fn session(fanout: &SessionIoFanout) -> Uuid {
        let id = Uuid::new_v4();
        fanout.register_session(id, IdleTuning::DEFAULT, 30, 120);
        id
    }

    /// #973 (B), (a) - CHUNK BOUNDARIES. The trigger must survive the prologue arriving in two
    /// reads, split anywhere.
    ///
    /// The old predicate could not. `strip_ansi_csi` is stateless and keeps no residue between
    /// calls, but the read loop feeds it raw `read()` chunks, and conhost really does split its
    /// output across writes. A chunk ending mid-CSI or mid-OSC hands the tail to the next call
    /// with NO leading `ESC`, and `1049h`, `2J` and `cmd.exe` are all printable - so the gate
    /// opened on a child that had painted nothing, which is the bug the gate exists to prevent.
    ///
    /// It is not an edge case. **43 of this prologue's 51 interior split points** fool the old
    /// predicate: a boundary inside a CSI leaves the tail with no `ESC`, and the tail of nearly
    /// every sequence here is printable - `l`, `2J`, `1049h`, `m`, and `cmd.exe` out of the OSC
    /// title. Only a boundary that happens to land ON an `ESC` is safe.
    ///
    /// This asserts the hazard is REAL first (some split does fool the old predicate), or the
    /// rest of the test would prove nothing, and then walks every interior split point through
    /// the new trigger.
    #[test]
    fn the_gate_holds_a_prologue_split_at_any_chunk_boundary() {
        let fanout = fanout();

        // whole, in one chunk, the old predicate gets right
        assert!(
            !output_has_printable_activity(&String::from_utf8_lossy(TUI_PROLOGUE)),
            "the prologue paints nothing, and unsplit even the old predicate saw that"
        );

        // split, it does not. These are the boundaries that would have opened the gate.
        let fooled: Vec<usize> = (1..TUI_PROLOGUE.len())
            .filter(|&at| {
                let (head, tail) = TUI_PROLOGUE.split_at(at);
                output_has_printable_activity(&String::from_utf8_lossy(head))
                    || output_has_printable_activity(&String::from_utf8_lossy(tail))
            })
            .collect();
        assert!(
            !fooled.is_empty(),
            "if no split fools the old predicate, this test is not testing the hazard"
        );

        // the new trigger, at every one of them
        for at in 1..TUI_PROLOGUE.len() {
            let id = session(&fanout);
            let (head, tail) = TUI_PROLOGUE.split_at(at);
            feed(&fanout, id, &[head, tail]);
            assert!(
                !fanout.has_rendered_visible_content(id),
                "split at {at}: the child has painted nothing, so the gate must stay shut"
            );
        }

        // ...and it is a gate, not a wall: the moment the child paints, it opens
        let id = session(&fanout);
        feed(&fanout, id, &[TUI_PROLOGUE]);
        assert!(
            !fanout.has_rendered_visible_content(id),
            "still nothing to see"
        );
        feed(&fanout, id, &[b"> "]);
        assert!(
            fanout.has_rendered_visible_content(id),
            "a glyph the user can see must open the gate"
        );
    }

    /// #973 (B), (b) - THREE-BYTE ESCAPES. `ESC ( B` is what ncurses and half the TUI world emit
    /// on the way up. The old stripper consumed `ESC` plus exactly ONE char, so the `B` survived
    /// and read as printable, and the gate opened on a blank screen. Same for `ESC ) 0` (leaks
    /// `0`) and `ESC % G` (leaks `G`).
    ///
    /// `ESC # 8` (DECALN) is deliberately not asserted here. It is the fourth three-byte escape
    /// and it fools the old stripper too (leaks `8`), but it is the one that genuinely PAINTS: a
    /// real terminal fills the screen with `E`. vt100 does not implement it, so our parser shows
    /// nothing and this gate would stay shut - which is a limitation of the crate, not a property
    /// worth pinning as correct. Pinning it would freeze the wrong answer.
    #[test]
    fn a_three_byte_charset_designator_does_not_open_the_gate() {
        assert!(
            output_has_printable_activity("\x1b(B"),
            "the old predicate really is fooled by this - that is the whole defect"
        );

        let fanout = fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"\x1b(B", b"\x1b)0", b"\x1b%G"]);

        assert!(
            !fanout.has_rendered_visible_content(id),
            "a charset designator paints nothing: the gate must stay shut"
        );
    }

    /// What "a human can see" means, pinned. A space is not content, even though the cell was
    /// written and `Cell::has_contents` says true. A COLOURED space is - TUIs draw status bars
    /// and boxes with nothing else, and `Cell::clear` keeps the attributes, so a cleared screen
    /// can be solid red while holding no contents at all.
    #[test]
    fn a_blank_viewport_is_not_content_but_a_coloured_one_is() {
        let fanout = fanout();

        // the TUI paints its still-empty viewport: spaces, and plenty of them. This is the exact
        // moment the gate must stay shut - it is inside the danger window.
        let id = session(&fanout);
        feed(&fanout, id, &[b"\x1b[2J\x1b[H", &[b' '; 240]]);
        assert!(
            !fanout.has_rendered_visible_content(id),
            "a viewport of spaces is still a blank viewport"
        );
        feed(&fanout, id, &[b"ready"]);
        assert!(
            fanout.has_rendered_visible_content(id),
            "a glyph is content"
        );

        // a red status bar: no glyph anywhere, and a human plainly sees it
        let id = session(&fanout);
        feed(&fanout, id, &[b"\x1b[41m", &[b' '; 20]]);
        assert!(
            fanout.has_rendered_visible_content(id),
            "a coloured space is content: it is how a TUI draws a status bar"
        );

        // reverse video does it with the default colours
        let id = session(&fanout);
        feed(&fanout, id, &[b"\x1b[7m", &[b' '; 20]]);
        assert!(
            fanout.has_rendered_visible_content(id),
            "an inverse space renders as a solid block"
        );
    }

    /// #973 / #955 - a degenerate size must never reach the vt100 parser.
    ///
    /// Red before the guard, and not with an assertion - with a **panic**:
    ///
    /// ```text
    /// panicked at vt100-0.15.2/src/grid.rs:74:34: attempt to subtract with overflow
    /// ```
    ///
    /// `vt100::grid::set_size` computes `size.rows - 1` on a `u16`, so `rows == 0` underflows.
    /// Debug panics; release wraps to 65535. Worse, the panic fires while
    /// `resize_screen_and_broadcast` holds `screen_parsers`, poisoning it - and every reader of
    /// that mutex swallows the poison silently (`if let Ok` / `.ok()?`), so #955's snapshot,
    /// the output sequence numbering and `get_pty_size` would go dead for EVERY session, for
    /// the life of the process, without a log line. A black tile on re-attach is what the user
    /// would see: exactly the bug #955 shipped to kill.
    #[test]
    fn a_degenerate_resize_never_reaches_the_vt100_parser() {
        let fanout = fanout();
        let id = Uuid::new_v4();
        fanout.register_session(id, IdleTuning::DEFAULT, 30, 120);
        fanout.handle_output(
            &PtyOutputTarget::noop(),
            id,
            &id.to_string(),
            b"hello".to_vec(),
        );

        fanout.resize_screen_and_broadcast(id, 0, 0);

        let after = fanout.get_screen_snapshot(id).expect("snapshot");
        assert_eq!(
            (after.rows, after.cols),
            (30, 120),
            "the screen must be untouched by a size the child was never given"
        );
        assert!(
            String::from_utf8_lossy(&after.data).contains("hello"),
            "and the child's output must still be in it: an empty snapshot is #955's black tile"
        );

        // a real size still moves the screen - the guard refuses the degenerate, not the resize
        fanout.resize_screen_and_broadcast(id, 80, 24);
        let resized = fanout.get_screen_snapshot(id).expect("snapshot");
        assert_eq!((resized.rows, resized.cols), (24, 80));
    }

    #[test]
    fn printable_activity_ignores_ansi_only_chunks() {
        assert!(!output_has_printable_activity("\x1b[31m\x1b[0m"));
        assert!(!output_has_printable_activity("\x1b]0;title\x07"));
        assert!(output_has_printable_activity("\x1b[31mready\x1b[0m"));
    }

    #[test]
    fn response_marker_capture_writes_trimmed_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_id = Uuid::new_v4();
        let watchers: ResponseWatcherMap = Arc::new(Mutex::new(HashMap::new()));
        watchers.lock().unwrap().insert(
            (session_id, "r1".to_string()),
            ResponseWatcher {
                response_dir: dir.path().to_path_buf(),
                buffer: None,
                capturing: false,
            },
        );

        scan_response_markers(
            session_id,
            "before %%AC_RESPONSE::r1::START%% {\"ok\": true} %%AC_RESPONSE::r1::END%% after",
            &watchers,
        );

        let json = std::fs::read_to_string(dir.path().join("r1.json")).expect("response json");
        assert!(json.contains("\"requestId\": \"r1\""));
        assert!(json.contains("\"content\": \"{\\\"ok\\\": true}\""));
        assert!(watchers.lock().unwrap().is_empty());
    }

    // ---- #1032: the rows accessor, against the real parser -------------------------

    /// The whole transfer claim of the round-1 capture rests on this: the capture read its
    /// bytes with `contents_between`, and the engine reads them with `rows`. If those two
    /// ever disagree, every measured fact about the statusline is about a screen this
    /// accessor does not return.
    ///
    /// Valid rows only: the `Equal` branch of `contents_between` ends in
    /// `.unwrap_or_default()`, so an out-of-range row answers `""` where `rows()` simply
    /// has no index - comparing those would pin the crate's error handling, not the claim.
    #[test]
    fn get_screen_rows_matches_contents_between() {
        let fanout = fanout();
        let id = session(&fanout);
        feed(
            &fanout,
            id,
            &[
                "  Context \u{2591}\u{2591}\u{2588} 42% \u{2502} Usage\r\nprose above it\r\n"
                    .as_bytes(),
            ],
        );

        let rows = fanout
            .get_screen_rows(id)
            .expect("rows for a registered session");

        let parsers = fanout.screen_parsers.lock().unwrap();
        let screen = parsers.get(&id).expect("parser").parser.screen();
        let (row_count, cols) = screen.size();
        assert_eq!(rows.len(), row_count as usize);
        for r in 0..row_count {
            assert_eq!(
                rows[r as usize],
                screen.contents_between(r, 0, r, cols),
                "row {r} disagrees with the accessor the capture measured"
            );
        }
    }

    /// The column-2 anchor is the engine's only defence against single-row input-box prose,
    /// and it is worth exactly as much as this: that the two leading spaces of a row painted
    /// at column 2 survive the round trip out of the grid.
    #[test]
    fn leading_spaces_survive_so_the_column_two_anchor_works() {
        let fanout = fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"  Context 7% and a tail"]);

        let rows = fanout.get_screen_rows(id).expect("rows");
        assert_eq!(rows[0], "  Context 7% and a tail");
        assert!(
            rows[0].starts_with("  Context"),
            "the anchor rests on these two spaces: {:?}",
            rows[0]
        );
    }

    /// Criterion 1. The mirror is keyed by session id and so is the scraper's map; two
    /// concurrent agents must never read each other's number.
    #[test]
    fn two_sessions_never_cross_rows() {
        let fanout = fanout();
        let a = session(&fanout);
        let b = session(&fanout);

        feed(&fanout, a, &[b"  Context 11%"]);
        feed(&fanout, b, &[b"  Context 88%"]);

        assert_eq!(
            fanout.get_screen_rows(a).expect("rows a")[0],
            "  Context 11%"
        );
        assert_eq!(
            fanout.get_screen_rows(b).expect("rows b")[0],
            "  Context 88%"
        );
    }

    /// The fanout's two-state contract: absent parser is `None`, and that is all it means.
    /// Whether the session is OVER is a question this type cannot answer and does not try to.
    #[test]
    fn get_screen_rows_is_none_for_an_unknown_session() {
        let fanout = fanout();
        assert!(fanout.get_screen_rows(Uuid::new_v4()).is_none());
    }

    /// Criterion 2. The reading comes off the mirror, which AC feeds from the PTY read loop
    /// whether or not a terminal was ever mounted: `PtyOutputTarget::noop()`, never resized,
    /// still reads back.
    #[test]
    fn rows_are_readable_for_a_session_with_no_terminal() {
        let fanout = fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"  Context 5%"]);

        let rows = fanout
            .get_screen_rows(id)
            .expect("a never-mounted session still reads");
        assert_eq!(rows[0], "  Context 5%");
    }
}
