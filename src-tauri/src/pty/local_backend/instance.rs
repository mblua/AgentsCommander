use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{MasterPty, PtySize};
use uuid::Uuid;

use crate::errors::AppError;

pub(super) struct PtyInstance {
    pub(super) master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    pub(super) writer: Arc<Mutex<Box<dyn Write + Send>>>,
    pub(super) child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    pub(super) job: Option<crate::pty::job::JobObject>,
    /// #973 (C) - the size the ConPTY is actually at, so a resize that changes nothing is
    /// not sent to the child. Seeded from the size the PTY was opened at.
    pub(super) size: Mutex<(u16, u16)>,
    /// #973 (B) - held closed until the child has rendered something. See `StartupGate`.
    pub(super) startup_gate: Mutex<StartupGate>,
    /// #973 (B) - the same fact as the gate, as one relaxed atomic, so the PTY read loop
    /// can skip the lock on every chunk once the child is up. The gate is the truth; this
    /// is only a fast path, and a stale `false` costs one no-op call.
    pub(super) rendered: Arc<AtomicBool>,
}

/// #973 (B) - a PTY resize that lands while a coding agent's TUI is starting up makes it
/// redraw its still-empty viewport and lose the wakeup for the content that becomes ready
/// right after. The terminal stays blank, the process stays alive, and any keypress paints
/// it. Measured on a bare ConPTY: a resize in that window blanks Codex 8 times in 10.
///
/// So AC does not resize a child that has not rendered anything yet. Resizes are held, the
/// LAST one is kept, and it is applied the moment the child paints - which is the earliest
/// moment it is safe to.
///
/// **Why the trigger is "has rendered" and not a timer.** The danger window is
/// [TUI is up, content is ready], and both ends move with machine load: in production
/// first-paint spans 324 ms to 2163 ms, while on an idle box the window sits at 180-300 ms.
/// Any fixed delay is a constant fitted to one machine, and on a slower one it lands
/// *inside* the window - a fix that reproduces the bug. A render, by contrast, cannot happen
/// inside the window, because the window is defined as the interval in which the child has
/// rendered nothing. The trigger is immune to load, terminal height and frame count.
///
/// **That argument only holds if the trigger actually tests rendering, so it does.** It asks the
/// real, stateful vt100 parser - the one `handle_output` already feeds every byte of every chunk -
/// whether the screen now holds a cell a human could see:
/// `SessionIoFanout::has_rendered_visible_content`.
///
/// It is worth saying what it must NOT be, because the first version of this gate got it wrong and
/// the mistake was invisible. It used the idle detector's text predicate
/// (`output_has_printable_activity`), which asks whether a printable byte survives a STATELESS,
/// per-chunk escape stripper. That is a different question, and it answers this one wrong twice:
/// a chunk that ends mid-CSI or mid-OSC hands the tail to the next call with no leading `ESC`
/// (`1049h`, `2J` and `cmd.exe` are all printable, and conhost really does split its writes), and
/// a three-byte charset designator like `ESC ( B` - which ncurses and half the TUI world emit on
/// the way up - outran a stripper that consumed `ESC` plus one char. Either one opens the gate on
/// a child that has painted nothing, which is precisely the bug the gate exists to prevent, and
/// it would have failed silently and intermittently inside an already intermittent bug. A parser
/// carries state across reads and knows what an escape is, so both are closed by construction
/// rather than by another special case.
///
/// **Why not #942's paint floor.** That floor is 256 bytes and the blank child sits at 345:
/// it would fire for a child that is hung.
///
/// **Why there is no timeout escape.** A child that never renders is showing nothing, so a
/// resize it never receives changes nothing a user can see - and the instant it does render,
/// the size it should have had is applied. A timeout would be the very constant this design
/// exists to avoid. Shells are unaffected: a prompt is printable, so their gate opens on the
/// first chunk.
pub(super) enum StartupGate {
    /// The child has not rendered yet. Resizes are held here, last one wins.
    Holding(Option<(u16, u16)>),
    /// The child has rendered. Resizes go straight through, forever.
    Open,
}

/// #973 - decide what to do with a resize the view asked for, and do it. This is the whole
/// of B and C, and it is a free function over the instance so it can be tested against a
/// real ConPTY: the backend itself cannot be built in a unit test, because its `GitWatcher`
/// needs a Tauri `AppHandle`, and none of that has anything to do with resizing a PTY.
///
/// Returns whether the ConPTY was actually resized.
pub(super) fn resize_instance(
    instance: &PtyInstance,
    id: Uuid,
    cols: u16,
    rows: u16,
) -> Result<bool, AppError> {
    // #973 - refuse a degenerate size FIRST, ahead of the gate.
    //
    // The gate is last-wins (`StartupGate::on_resize`), so a `0x0` reaching it OVERWRITES the
    // real size the view is waiting to give the child. The hand-over then pops the `0x0`,
    // `send_size_to_conpty` refuses it, the pending slot is consumed, and nothing retries: the
    // ConPTY is wedged at the size it was opened at for good. On cold start that is a 120x30
    // child behind a 74x23 terminal. A size that must never reach the child must never be
    // allowed to displace one that must.
    //
    // Where a zero comes from, since we got this wrong once: NOT from xterm. `fit()` clamps to
    // its own MINIMUM_COLS = 2 / MINIMUM_ROWS = 1 (`@xterm/addon-fit`), and `CoreTerminal.resize`
    // rejects NaN and clamps again - the worst it can produce is NaN, which the frontend's
    // `Number.isInteger` check catches. The zero comes off the WIRE: `pty_resize` on the web
    // transport takes cols/rows straight from a JSON payload (`web/commands.rs`), and a client
    // that is not xterm, or is simply broken, can put a 0 in it.
    if cols == 0 || rows == 0 {
        log::warn!("[pty] refusing degenerate resize {id} to {cols}x{rows} (#973)");
        return Ok(false);
    }

    // B - do not resize a child that has not rendered anything yet. The view fires its fit
    // 300-500 ms after spawn, which is exactly when a coding agent's TUI is coming up, and a
    // resize there costs it its first content render. Hold the size; the read loop hands it
    // over the moment the child paints. See `StartupGate`.
    {
        let mut gate = instance
            .startup_gate
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if gate.on_resize(cols, rows).is_none() {
            log::debug!(
                "[pty] resize {id} to {cols}x{rows} held: the child has not rendered yet (#973)"
            );
            return Ok(false);
        }
    }
    send_size_to_conpty(instance, id, cols, rows)
}

/// #973 (C) - do not tell the child about a resize that is not a resize.
///
/// ConPTY delivers even a same-size `ResizePseudoConsole` to the client as a real event, and
/// the view fires 5-20 identical resizes per attach (`TerminalView.tsx:85-95`, a double
/// `requestAnimationFrame` plus a `ResizeObserver`), so today every attach shakes the child
/// for nothing.
///
/// The cached size is written only AFTER the ConPTY has accepted the new one: if the resize
/// failed and we recorded it anyway, every retry would be skipped as a no-op and the PTY
/// would be wedged at the wrong size forever.
pub(super) fn send_size_to_conpty(
    instance: &PtyInstance,
    id: Uuid,
    cols: u16,
    rows: u16,
) -> Result<bool, AppError> {
    // #973 - refuse a degenerate size. Defence in depth: the request path is guarded ahead of
    // the gate in `resize_instance`, and this is the last line before `master.resize()`, which
    // is also what `hand_over_held_size` calls with a size that has been sitting in the gate.
    //
    // It has to be refused SOMEWHERE, because ConPTY does not refuse it: `master.resize(0x0)`
    // returns Ok and the child is left with no screen at all. A stale size is recoverable; a
    // zero-column terminal is not. The zero itself comes off the wire, not from xterm - see
    // `resize_instance`.
    if cols == 0 || rows == 0 {
        log::warn!("[pty] refusing degenerate resize {id} to {cols}x{rows} (#973)");
        return Ok(false);
    }

    if !instance.size_changed(cols, rows) {
        log::debug!("[pty] resize {id} skipped: already {cols}x{rows} (#973)");
        return Ok(false);
    }

    {
        let master = instance.master.lock().unwrap_or_else(|e| e.into_inner());
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| AppError::PtyError(e.to_string()))?;
    }
    instance.remember_size(cols, rows);
    Ok(true)
}

/// #973 (B) - the child has rendered something, so the startup window is behind us. Open the
/// gate for good and hand the ConPTY the size the view has been waiting to give it.
///
/// Returns the size the ConPTY ACTUALLY took, so the caller can bring the vt100 screen along
/// with it - and `None` if nothing was held, or the held size was refused, or it failed.
///
/// Free over the instance, like `resize_instance`, so the hand-over can be driven by a test
/// against a real ConPTY: `open_startup_gate`, its only caller, needs a `LocalProcessBackend`,
/// whose `GitWatcher` needs a Tauri `AppHandle`. A test that re-implemented these lines rather
/// than calling them would not be testing this code.
pub(super) fn hand_over_held_size(instance: &PtyInstance, id: Uuid) -> Option<(u16, u16)> {
    // One lock covers both "take the held size" and "open the gate", so a resize landing at
    // this exact instant cannot be recorded into a gate that is already open and then dropped
    // on the floor, leaving the terminal stuck at the wrong size.
    let pending = {
        let mut gate = instance
            .startup_gate
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        gate.open()
    };
    // Only a fast path for the read loop; the gate above is the truth. Publishing it late is
    // safe: a stale `false` costs one extra no-op call to this function.
    instance.rendered.store(true, Ordering::Relaxed);

    let (cols, rows) = pending?;

    match send_size_to_conpty(instance, id, cols, rows) {
        Ok(true) => {
            log::info!(
                "[pty] startup gate open for {id}: applied the held resize {cols}x{rows} (#973)"
            );
            Some((cols, rows))
        }
        Ok(false) => None,
        Err(e) => {
            // Non-critical: the child is up and painting, it is just the wrong size.
            log::warn!("[pty] held resize {id} to {cols}x{rows} failed: {e} (#973)");
            None
        }
    }
}

impl StartupGate {
    /// The view asked for a resize. Returns the size to hand the ConPTY, or `None` if the
    /// child is still starting up and it must be held.
    pub(super) fn on_resize(&mut self, cols: u16, rows: u16) -> Option<(u16, u16)> {
        match self {
            StartupGate::Holding(pending) => {
                // Only the last size matters: the frontend fires 5-20 of these per attach.
                *pending = Some((cols, rows));
                None
            }
            StartupGate::Open => Some((cols, rows)),
        }
    }

    /// The child rendered. Open the gate for good and hand back the size that was held.
    pub(super) fn open(&mut self) -> Option<(u16, u16)> {
        match std::mem::replace(self, StartupGate::Open) {
            StartupGate::Holding(pending) => pending,
            StartupGate::Open => None,
        }
    }
}

impl PtyInstance {
    /// #973 (C) - has the size actually moved?
    ///
    /// The frontend calls resize unconditionally (`TerminalView.tsx:85-95`, from a double
    /// `requestAnimationFrame` plus a `ResizeObserver`), so one attach fires 5-20 identical
    /// resizes, and ConPTY hands every one of them to the child as a real event.
    pub(super) fn size_changed(&self, cols: u16, rows: u16) -> bool {
        Self::size_changed_in(&self.size, cols, rows)
    }

    /// Only after the ConPTY has actually accepted it: if `resize` failed, the cached size
    /// must stay stale so the next attempt is not skipped as a no-op and the PTY is not
    /// wedged at the wrong size forever.
    pub(super) fn remember_size(&self, cols: u16, rows: u16) {
        Self::remember_size_in(&self.size, cols, rows);
    }

    // The two free functions above are the whole of C. Split out so they can be tested
    // without a live ConPTY: a `PtyInstance` owns real `MasterPty` handles.
    pub(super) fn size_changed_in(size: &Mutex<(u16, u16)>, cols: u16, rows: u16) -> bool {
        *size.lock().unwrap_or_else(|e| e.into_inner()) != (cols, rows)
    }

    pub(super) fn remember_size_in(size: &Mutex<(u16, u16)>, cols: u16, rows: u16) {
        *size.lock().unwrap_or_else(|e| e.into_inner()) = (cols, rows);
    }
}
