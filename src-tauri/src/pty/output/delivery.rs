#[cfg(test)]
use super::FanoutTraceEvent;
use super::{
    append_history, history_from_first_line, output_has_printable_activity, scan_response_markers,
    Accumulated, ParserAvailability, PtyOutputRegistrationToken, PtyOutputTarget,
    PtyScreenSnapshot, SessionIoFanout, TerminalOutputAttachError, UI_BATCH_INTERVAL_MS,
    UI_HISTORY_REPLAY_PROLOGUE,
};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use terminal_snapshot_renderer::{MAX_CELLS, MAX_COLUMNS, MAX_ROWS};
use uuid::Uuid;

impl SessionIoFanout {
    pub(crate) fn handle_output(
        &self,
        token: &PtyOutputRegistrationToken,
        session_id_str: &str,
        data: Vec<u8>,
    ) {
        let Some(lease) = self.acquire_reader_lease(token) else {
            return;
        };
        let id = token.identity.session_id;
        self.notify_output_observers(id, &data);

        let (accumulated, registration) = {
            let Ok(mut parsers) = self.screen_parsers.lock() else {
                return;
            };
            let Some(state) = parsers.get_mut(&id) else {
                return;
            };
            if !state
                .registration
                .matches_token(token, &self.fanout_identity)
            {
                return;
            }
            let registration = Arc::clone(&state.registration);
            // The emission predicate is `reader_gate.is_open() && attached`, and it is an AND:
            // the gate is the teardown drain gate, the attached set is delivery, and folding
            // one into the other would let output emit into a session being torn down.
            //
            // It is evaluated independently of `parser_availability` on purpose. A faulted
            // parser must keep emitting, unsequenced, or the terminal goes black for the life
            // of the process, so the `Unavailable` arm below carries the real `ui_open` rather
            // than the `false` the delivery contract used to hardcode there.
            let ui_open = state.reader_gate.is_open();
            let (sequence, parser_fault) = match state.parser_availability {
                ParserAvailability::Unavailable => (None, false),
                ParserAvailability::Available => {
                    let processed = crate::logging::catch_payload_unwind(|| {
                        state.parser.process(&data);
                        let sequence = state.output_sequence.checked_add(1).ok_or(())?;
                        state.output_sequence = sequence;
                        // Order matters, and these two lines must stay contiguous. The ring
                        // may only grow once `output_sequence` has advanced: on overflow the
                        // line above returns `Err` and the parser goes `Unavailable`, and a
                        // ring that grew anyway would make the attach snapshot carry bytes
                        // that `sequence` does not represent. The frontend reconciles by
                        // watermark and only skips what is at or below the snapshot sequence,
                        // so those bytes would be seeded and then written again when they
                        // arrive live: a duplicated block of history.
                        append_history(&mut state.history, &mut state.history_aligned, &data);
                        Ok::<u64, ()>(sequence)
                    });
                    match processed {
                        Ok(Ok(sequence)) => {
                            #[cfg(test)]
                            self.trace(FanoutTraceEvent::ParserProcessed(sequence));
                            (Some(sequence), false)
                        }
                        Ok(Err(())) | Err(_) => {
                            state.parser_availability = ParserAvailability::Unavailable;
                            (None, true)
                        }
                    }
                }
            };
            if parser_fault {
                log::error!("[terminal-snapshot] stage=parser_fault session={id}");
            }
            // Appending under the parser lock is what keeps the batch boundary atomic with the
            // sequence assignment, so no coalesced batch can straddle an attach's snapshot.
            // The emit it may ask for is run below, after the lock is dropped.
            let accumulated = if ui_open {
                self.attachments
                    .accumulate(&registration, sequence, &data, parser_fault)
            } else {
                Accumulated::Unattached
            };
            (accumulated, registration)
        };

        if let Some(ref broadcaster) = self.ws_broadcaster {
            #[cfg(test)]
            self.trace(FanoutTraceEvent::Websocket);
            broadcaster.broadcast_pty_output(session_id_str, &data);
        }

        // The UI EMIT is still the last thing this function does, which is what keeps a slow
        // one from delaying the idle detector, the response-marker scan, the raw sender or the
        // websocket broadcaster. Only the append above runs earlier, and it copies bytes.
        #[cfg(test)]
        if accumulated != Accumulated::Unattached {
            self.trace(FanoutTraceEvent::UiEmit);
        }
        match accumulated {
            Accumulated::Unattached | Accumulated::Pending => {}
            Accumulated::ArmTimer => self.arm_output_flush(id, &registration.target),
            Accumulated::FlushNow => self.attachments.flush(id, false),
        }
        lease.complete();
    }

    fn notify_output_observers(&self, id: Uuid, data: &[u8]) {
        let n = data.len();

        let text = String::from_utf8_lossy(data);
        if text.contains('\u{FFFD}') {
            log::debug!(
                "[PTY] session {} chunk had invalid UTF-8 at buffer boundary ({} bytes, {} \
                    replacement chars)",
                id,
                n,
                text.matches('\u{FFFD}').count()
            );
        }

        // #2124 - the detector decides whether this chunk counts, resets the
        // silence clock or is absorbed into a pending short burst. The trace
        // order stays TouchSilence then PrintableActivity for the burst-off
        // sessions the trace tests use.
        let printable = output_has_printable_activity(&text);
        #[cfg(test)]
        self.trace(FanoutTraceEvent::TouchSilence);
        #[cfg(test)]
        if printable {
            self.trace(FanoutTraceEvent::PrintableActivity);
        }
        self.idle_detector.record_output(id, n, printable);
        if !printable {
            log::trace!(
                "[idle] SKIPPED activity for {} ({} bytes, escape-only output)",
                &id.to_string()[..8],
                n
            );
        }

        #[cfg(test)]
        self.trace(FanoutTraceEvent::ResponseMarkers);
        scan_response_markers(id, &text, &self.response_watchers);

        let output_sender = self
            .output_senders
            .lock()
            .ok()
            .and_then(|senders| senders.get(&id).cloned());
        if let Some(sender) = output_sender {
            #[cfg(test)]
            self.trace(FanoutTraceEvent::OutputSender);
            let _ = sender.try_send(data.to_vec());
        }
    }

    /// Attaches `label`'s window to this session's output and returns the seed for it.
    ///
    /// The whole body runs inside one `screen_parsers` hold and does three things, in this
    /// order: drain the session's pending batch, read the snapshot at `state.output_sequence`,
    /// insert the label. Because no chunk can be processed while that lock is held, the three
    /// are atomic with respect to the ingest, and that removes three hazards at once. No
    /// coalesced batch can straddle the snapshot, so a scalar `sequence` carrying the batch's
    /// last sequence is sufficient and no first-sequence field is needed. There is no window
    /// between reading the snapshot and becoming attached in which chunks are neither seeded
    /// nor emitted. And an attach racing `remove_session` cannot leave a label behind for a
    /// session that no longer exists, because the insert only happens with the session present.
    ///
    /// The insert is the last step, on a path that has already decided to succeed, so the
    /// attach is transactional by construction: no guard, no compensating release.
    ///
    /// Only two conditions fail, and both mean there is nothing to attach to: the session is
    /// absent, or the registration identity does not match. Every other condition attaches
    /// without a snapshot and lets the client write live (see `TerminalOutputAttachError`).
    ///
    /// `include_history` asks for the retained ring instead of the mirrored viewport. It is
    /// safe on every attach because the client applies the snapshot through a reset first:
    /// replaying the ring over a terminal that still holds scrollback would append a duplicate
    /// block, and the reset is what makes that impossible. The ring also keeps filling while a
    /// session is detached, which is what makes a re-attach gap free, so the frontend always
    /// asks for it; the parameter stays so the mirrored viewport remains addressable.
    pub(crate) fn activate_terminal_output(
        &self,
        id: Uuid,
        label: &str,
        include_history: bool,
    ) -> Result<Option<PtyScreenSnapshot>, TerminalOutputAttachError> {
        let Ok(mut parsers) = self.screen_parsers.lock() else {
            // Nothing can be read through a poisoned parser lock, but that is not "nothing to
            // attach to": the window attaches and writes live, exactly as it would for a
            // faulted parser.
            self.attachments.attach(id, label);
            return Ok(None);
        };
        let Some(state) = parsers.get_mut(&id) else {
            return Err(TerminalOutputAttachError::SessionUnavailable);
        };
        if state.registration.session_id != id
            || !Arc::ptr_eq(&state.registration.fanout_identity, &self.fanout_identity)
        {
            return Err(TerminalOutputAttachError::OutputTargetUnavailable);
        }
        let mut reconcile_fault = false;
        // #1458: carried out of the parser lock exactly like `reconcile_fault`. `Some` only
        // when the ring was flagged unaligned; the pair is (ring length, bytes kept).
        let mut history_unaligned: Option<(usize, usize)> = None;
        let snapshot = if state.parser_availability == ParserAvailability::Available {
            let (parser_rows, parser_cols) = state.parser.screen().size();
            if (parser_rows, parser_cols) != state.conpty_size {
                // #1439: the parser grid diverged from the grid the ConPTY took
                // (a skipped follow, or any path that resized one without the
                // other). A seed rendered off this parser adopts the stale grid
                // in the attaching window and replays other-grid bytes into it:
                // the garbled re-attach. Converge the grid now, seed nothing;
                // the frontend attaches live (the no-snapshot path), and the
                // next attach after the child's next repaint seeds cleanly.
                let (conpty_rows, conpty_cols) = state.conpty_size;
                log::warn!(
                    "[terminal-snapshot] stage=attach_grid_mismatch session={id} \
                        parser={parser_cols}x{parser_rows} conpty={conpty_cols}x{conpty_rows} \
                        (#1439)"
                );
                let resized = crate::logging::catch_payload_unwind(|| {
                    state.parser.set_size(conpty_rows, conpty_cols)
                });
                if resized.is_err() {
                    state.parser_availability = ParserAvailability::Unavailable;
                    reconcile_fault = true;
                }
                None
            } else {
                copy_attach_snapshot(state, id, include_history, &mut history_unaligned)
            }
        } else {
            None
        };
        self.attachments.attach(id, label);
        drop(parsers);
        if let Some((ring, kept)) = history_unaligned {
            log::warn!(
                "[terminal-snapshot] stage=attach_history_unaligned session={id} ring={ring} \
                    kept={kept} (#1458)"
            );
        }
        if reconcile_fault {
            log::error!("[terminal-snapshot] stage=parser_fault session={id}");
            // #1439 R2: flush at the transition, OUTSIDE the parser lock. An
            // emit under that lock stalls the PTY reader on its next chunk;
            // both existing fault sites flush only after releasing the lock.
            self.attachments.flush(id, false);
        }
        Ok(snapshot)
    }

    /// Releases `label`'s attachment. Detaching a session this window never attached, and
    /// detaching twice, are window-local no-ops that cannot disturb another window.
    pub(crate) fn detach_terminal_output(&self, id: Uuid, label: &str) {
        self.attachments.detach(id, label);
    }

    /// A window was destroyed: release every attachment it held, with no frontend cooperation.
    /// This is what makes the frontend detach a bandwidth optimization rather than a
    /// correctness dependency, and what lets the close hook avoid blocking the close.
    pub(crate) fn release_window_attachments(&self, label: &str) {
        self.attachments.release_window(label);
    }

    /// One one-shot task per session, armed when that session's accumulator goes from empty to
    /// non-empty.
    ///
    /// Deliberately not a global 16 ms ticker. A ticker runs forever, including with zero
    /// attached sessions, which defeats timer coalescing and costs idle power on a
    /// long-running tray app, and it serializes every attached session's emit onto one task,
    /// so a flooding session's 64 KiB delays the keystroke echo flush of the interactive
    /// session queued behind it. Arming on demand preserves today's idle cost, which is zero,
    /// and today's latency shape, where the first byte after idle is emitted 16 ms later.
    /// Arming is switched at runtime rather than by `cfg(test)`: test builds start it off so
    /// every emit a test observes comes from an explicit `flush_terminal_output_for_test` and
    /// no assertion races a 16 ms task, and the one test that covers the timer turns it on.
    pub(super) fn arm_output_flush(&self, id: Uuid, target: &PtyOutputTarget) {
        if !self.output_timer_enabled.load(Ordering::Relaxed) {
            return;
        }
        let attachments = Arc::clone(&self.attachments);
        target.spawn_flush(Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(UI_BATCH_INTERVAL_MS)).await;
            attachments.flush(id, true);
        }));
    }

    /// The synchronous flush seam. Every backend test drives it instead of sleeping, so the
    /// timer is the only spawned thing on this path and no assertion depends on a wall clock.
    /// It stands in for the timer, so it clears `timer_armed` exactly as the timer does.
    #[cfg(test)]
    pub(crate) fn flush_terminal_output_for_test(&self, id: Uuid) {
        self.attachments.flush(id, true);
    }

    /// Lets the timer's own test run the real arming path. Nothing else switches this on.
    #[cfg(test)]
    pub(crate) fn enable_output_timer_for_test(&self) {
        self.output_timer_enabled.store(true, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn attached_labels_for_test(&self, id: Uuid) -> Vec<String> {
        self.attachments.labels_for_test(id)
    }

    #[cfg(test)]
    pub(crate) fn pending_output_bytes_for_test(&self, id: Uuid) -> Option<usize> {
        self.attachments.pending_bytes_for_test(id)
    }
}

fn copy_attach_snapshot(
    state: &mut super::ScreenReplayState,
    id: Uuid,
    include_history: bool,
    history_unaligned: &mut Option<(usize, usize)>,
) -> Option<PtyScreenSnapshot> {
    let copied = {
        let uses_history = include_history && !state.history.is_empty();
        // `as_slices` keeps this a read: `make_contiguous` needs `&mut` and
        // rotates the buffer during what is otherwise a copy out.
        let replay = if uses_history {
            let (front, back) = state.history.as_slices();
            // #1458: the hot-path realignment is capped at 4 KiB and stays failed
            // for as long as the front sits in a newline-free region, so it can
            // leave the front inside an escape sequence. Attach is cold: pay the
            // full scan here rather than seed a literal sequence tail.
            if state.history_aligned {
                Some((front, back))
            } else {
                history_from_first_line(front, back)
            }
        } else {
            None
        };
        if uses_history && !state.history_aligned {
            *history_unaligned = Some((
                state.history.len(),
                replay.map_or(0, |(front, back)| front.len() + back.len()),
            ));
        }
        crate::logging::catch_payload_unwind(|| {
            let screen = state.parser.screen();
            let (rows, cols) = screen.size();
            let cells = usize::from(rows).checked_mul(usize::from(cols)).ok_or(())?;
            if rows > MAX_ROWS || cols > MAX_COLUMNS || cells > MAX_CELLS {
                return Err(());
            }
            let data = match replay {
                Some((front, back)) => {
                    let mut bytes = Vec::with_capacity(
                        UI_HISTORY_REPLAY_PROLOGUE.len() + front.len() + back.len(),
                    );
                    bytes.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
                    bytes.extend_from_slice(front);
                    bytes.extend_from_slice(back);
                    bytes
                }
                // No line start with content behind it. The ring cannot be
                // replayed from any offset, and in the observed incident it holds
                // nothing but spinner frames anyway. The mirror is a consistent
                // full repaint on a grid the #1439 branch above already validated.
                None => screen.contents_formatted(),
            };
            Ok::<PtyScreenSnapshot, ()>(PtyScreenSnapshot {
                data,
                rows,
                cols,
                sequence: state.output_sequence,
            })
        })
    };
    match copied {
        Ok(Ok(snapshot)) => Some(snapshot),
        Ok(Err(())) => None,
        Err(_) => {
            log::warn!(
                "[terminal-snapshot] stage=render_panic reason=payload_panic \
                session={id} (#1452)"
            );
            state.parser_availability = ParserAvailability::Unavailable;
            None
        }
    }
}
