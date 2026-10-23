use std::collections::HashMap;
use std::sync::Mutex;

use uuid::Uuid;

use crate::pty::context_scrape::{ContextSessionLiveness, ScreenRowsRead};
use crate::pty::output::SessionIoFanout;
use crate::pty::spawn_diagnostics::ChildLiveness;

use super::PtyInstance;

/// #942 - the body of `probe_child`, free over the map so #1032's liveness gate can be
/// driven by a test against a real ConPTY child.
///
/// The guard is a local and `ChildLiveness` is owned, so the `ptys` lock is released at the
/// return. That is what makes the gate below safe by construction rather than by care: the
/// `match` scrutinee holds no borrow of the map, so no temporary-lifetime extension can
/// carry the guard into an arm and nest `screen_parsers` inside `ptys`.
pub(super) fn probe_child_in(ptys: &Mutex<HashMap<Uuid, PtyInstance>>, id: Uuid) -> ChildLiveness {
    let mut ptys = ptys.lock().unwrap_or_else(|e| e.into_inner());
    let Some(instance) = ptys.get_mut(&id) else {
        return ChildLiveness::Gone;
    };
    let Some(child) = instance.child.as_mut() else {
        return ChildLiveness::Gone;
    };
    probe_child_contained(child)
}

/// #1032 - a local session's screen rows, gated on its child actually being alive.
///
/// Why a gate at all: a coding agent's statusline SURVIVES on the frozen grid after the
/// child dies. Verbatim, ~8s after a confirmed `code: 0`, on every exit path that matters -
/// a killed process cannot repaint, and Claude Code never uses the alternate screen, never
/// clears and never restores. PTY EOF never arrives either. Nothing signals the death from
/// any direction, so the frozen grid presents a perfectly well-formed row - glyph present,
/// `%` present, column 2, last match on the grid - that passes every defence the user's
/// pattern has. Liveness is not a regex problem, and asking the child is the only thing
/// that can tell a live 42% from a dead one.
///
/// Free over the map and the fanout, like `resize_instance` above, so the gate can be driven
/// by a test against a real ConPTY child: `LocalProcessBackend` itself cannot be built in a
/// unit test (its `GitWatcher` needs a Tauri `AppHandle`), and none of that has anything to
/// do with whether a child is alive.
pub(crate) fn context_liveness_from_child_liveness(
    liveness: &ChildLiveness,
) -> ContextSessionLiveness {
    match liveness {
        ChildLiveness::Alive => ContextSessionLiveness::Live,
        ChildLiveness::Unqueryable(_) => ContextSessionLiveness::Unavailable,
        ChildLiveness::Exited { .. } | ChildLiveness::Gone => ContextSessionLiveness::SessionOver,
    }
}

pub(super) fn screen_rows_if_child_alive(
    ptys: &Mutex<HashMap<Uuid, PtyInstance>>,
    fanout: &SessionIoFanout,
    id: Uuid,
) -> ScreenRowsRead {
    match probe_child_in(ptys, id) {
        ChildLiveness::Alive => match fanout.get_screen_rows(id) {
            Some(rows) => ScreenRowsRead::Rows(rows),
            // The child is alive, so the session is NOT over. A missing or poisoned parser
            // here is a desync or a poisoned lock, never a statement about the session.
            None => ScreenRowsRead::Unavailable,
        },
        ChildLiveness::Exited { .. } | ChildLiveness::Gone => ScreenRowsRead::SessionOver,
        // "We could not ask" is NOT "the child is dead". #942 built a three-valued oracle
        // exactly so those two never merge, and this is the arm that keeps them apart: the
        // same running process reads Alive through a full-rights handle and Unqueryable
        // through one stripped of SYNCHRONIZE. Reporting that as over would deregister a
        // live session permanently, on a child that never died.
        ChildLiveness::Unqueryable(_) => ScreenRowsRead::Unavailable,
    }
}

/// #942 - probe a child while the caller holds the `ptys` guard, and CONTAIN any panic.
///
/// portable-pty locks a mutex of its own inside the child and unwraps it (`try_wait`,
/// `do_kill`, `process_id` and `as_raw_handle` all do). If that mutex is ever poisoned,
/// an unwind from the probe would escape while the `ptys` guard is held and poison the
/// PTY map itself, which every terminal write, resize and kill locks on: a diagnostics
/// probe would silently take the terminal subsystem down with it. Catching AT the call
/// means the guard above us is released normally and the map stays usable.
pub(super) fn probe_child_contained(
    child: &mut Box<dyn portable_pty::Child + Send + Sync>,
) -> ChildLiveness {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| probe_child_liveness(child)))
        .unwrap_or_else(|_| {
            ChildLiveness::Unqueryable("portable-pty child lock is poisoned".to_string())
        })
}

/// #942 - ask Windows directly instead of going through `portable_pty::Child::try_wait`.
/// `WinChild::is_complete` cannot answer honestly: it swallows a failed
/// `GetExitCodeProcess` and returns "not exited" (so a handle whose query rights were
/// stripped by AV/EDR, a known scenario here, reads as ALIVE), and its `STILL_ACTIVE`
/// sentinel is 259, which is also a legal exit code. The process handle is signalled if
/// and only if the child is gone, whatever it exited with, and a handle we cannot wait
/// on fails loudly instead of pretending.
#[cfg(windows)]
pub(super) fn probe_child_liveness(
    child: &mut Box<dyn portable_pty::Child + Send + Sync>,
) -> ChildLiveness {
    use windows_sys::Win32::Foundation::{GetLastError, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};

    let Some(handle) = child.as_raw_handle() else {
        return ChildLiveness::Unqueryable("child exposes no process handle".to_string());
    };

    match unsafe { WaitForSingleObject(handle as _, 0) } {
        WAIT_TIMEOUT => ChildLiveness::Alive,
        WAIT_OBJECT_0 => {
            let mut code: u32 = 0;
            if unsafe { GetExitCodeProcess(handle as _, &mut code) } == 0 {
                let os_error = unsafe { GetLastError() };
                return ChildLiveness::Unqueryable(format!(
                    "GetExitCodeProcess failed (os error {os_error})"
                ));
            }
            ChildLiveness::Exited {
                code,
                success: code == 0,
            }
        }
        WAIT_FAILED => {
            let os_error = unsafe { GetLastError() };
            ChildLiveness::Unqueryable(format!("WaitForSingleObject failed (os error {os_error})"))
        }
        other => ChildLiveness::Unqueryable(format!("WaitForSingleObject returned {other}")),
    }
}

/// #942 - on Unix `try_wait` is `waitpid(WNOHANG)`: it reports a failed poll as `Err`,
/// so it can be trusted to tell "running" from "we could not ask".
#[cfg(not(windows))]
pub(super) fn probe_child_liveness(
    child: &mut Box<dyn portable_pty::Child + Send + Sync>,
) -> ChildLiveness {
    match child.try_wait() {
        Ok(Some(status)) => ChildLiveness::from(&status),
        Ok(None) => ChildLiveness::Alive,
        Err(e) => ChildLiveness::Unqueryable(e.to_string()),
    }
}
