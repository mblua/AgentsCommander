import type { PtyViewport } from "./types";

/**
 * #973 — the size the PTY is opened at.
 *
 * AC opened every ConPTY at a hardcoded 120x30 and let the terminal correct it
 * a few hundred milliseconds later. dev-rust measured what that correction does
 * when it lands inside a coding agent's TUI startup: the child redraws its
 * still-empty viewport, loses the wakeup for the content that becomes ready
 * right after, and the tile stays blank. 8/10 on a bare ConPTY, no Tauri and no
 * webview.
 *
 * The fix is to open the PTY at the size the view has ALREADY fitted to, so no
 * resize has to reach a starting child at all. That only works if the size
 * handed to `create_session` is byte-exact with the size the terminal's own
 * post-mount fit produces:
 *
 *   - a redundant SAME-size resize is harmless ......... 0/10 blank
 *   - ONE ROW of drift puts the bug back at ............ 6/10 blank
 *
 * So this module does not PREDICT the fit, because a prediction that is one row
 * off is worse than no prediction at all. `TerminalView` registers a probe that
 * runs the SAME computation the post-mount fit runs — `FitAddon
 * .proposeDimensions()` — against a terminal that is already on screen in the
 * same host. Every `.terminal-instance` is `position: absolute; inset: 0` inside
 * one shared `.terminal-host`, so they all have the identical box, the same
 * `.terminal-host .xterm` padding, the same `createTerminalOptions`, and the
 * same (already resolved) font metrics. The answer is exact by construction.
 *
 * When there is nothing on screen to measure — no session is active, so
 * `TerminalView` is not even mounted — the probe returns null, the caller sends
 * no size, and the backend keeps its historical 120x30. That case is not
 * covered by this fix; it is covered by the backend's startup-resize guard.
 */

type PtyViewportProbe = () => PtyViewport | null;

let probe: PtyViewportProbe | null = null;

/**
 * The size each session's PTY was actually OPENED at, recorded at create time
 * and consumed when the terminal for it is built.
 *
 * `TerminalView` needs this for two things: it starts xterm at that exact size
 * (so the first fit finds the size it already has and resizes nothing), and it
 * seeds the resize dedup with it (so the post-mount `syncViewport` does not send
 * the size the PTY is already open at).
 *
 * Bounded: an entry is consumed the moment the session is attached to a
 * terminal, so this holds at most the handful of sessions created but not yet
 * shown. The cap is a backstop against a caller that creates sessions it never
 * attaches.
 */
const spawnViewports = new Map<string, PtyViewport>();
const MAX_TRACKED_SPAWN_VIEWPORTS = 32;

/**
 * True only when the backend will genuinely open the PTY at this size.
 *
 * This gate is load-bearing, not hygiene. Recording a size the PTY was NOT
 * opened at is the one way this fix can wedge a terminal: the dedup would treat
 * the corrective resize as a no-op and skip it, leaving the PTY at a size the
 * view never agreed to, forever. Two backend rules decide it:
 *
 *   - `create_session` builds a viewport only when BOTH cols and rows arrive.
 *   - `PtyViewport::from_fit` silently falls back to 120x30 on a zero dimension
 *     (xterm's fit really does return one while its container is being laid
 *     out), so a zero must never be sent, let alone recorded.
 *
 * `u16` on the Rust side, so the range is checked here too.
 */
const isHonouredByBackend = (viewport: PtyViewport): boolean =>
  Number.isInteger(viewport.cols) &&
  Number.isInteger(viewport.rows) &&
  viewport.cols > 0 &&
  viewport.rows > 0 &&
  viewport.cols <= 65535 &&
  viewport.rows <= 65535;

/** `TerminalView` publishes how it measures its own tile. Returns the unregister. */
export const registerPtyViewportProbe = (next: PtyViewportProbe): (() => void) => {
  probe = next;
  return () => {
    if (probe === next) {
      probe = null;
    }
  };
};

/**
 * The size a terminal created right now would fit to, or null when there is
 * nothing on screen to measure. Never throws: a DOM read must not be able to
 * fail a session create.
 */
export const measurePtyViewport = (): PtyViewport | null => {
  if (!probe) {
    return null;
  }

  let measured: PtyViewport | null;
  try {
    measured = probe();
  } catch (err) {
    console.warn("[terminal] viewport probe failed:", err);
    return null;
  }

  if (!measured || !isHonouredByBackend(measured)) {
    return null;
  }

  return measured;
};

/** Record the size the backend just opened this session's PTY at. */
export const rememberSpawnViewport = (
  sessionId: string,
  viewport: PtyViewport
): void => {
  if (!isHonouredByBackend(viewport)) {
    return;
  }

  if (spawnViewports.size >= MAX_TRACKED_SPAWN_VIEWPORTS) {
    const oldest = spawnViewports.keys().next();
    if (!oldest.done) {
      spawnViewports.delete(oldest.value);
    }
  }

  spawnViewports.set(sessionId, { cols: viewport.cols, rows: viewport.rows });
};

/**
 * The size this session's PTY was opened at, consumed on read.
 *
 * Consumed, because it is only true for the FIRST terminal built for the
 * session. A later re-attach (a detached window, a re-created tile) must fit and
 * resize normally: by then the child has long since started, the resize is safe,
 * and the tile may genuinely be a different size.
 */
export const takeSpawnViewport = (sessionId: string): PtyViewport | null => {
  const viewport = spawnViewports.get(sessionId);
  if (!viewport) {
    return null;
  }

  spawnViewports.delete(sessionId);
  return viewport;
};

/** Test seam: drop the probe and every recorded spawn size. */
export const resetPtyViewportForTests = (): void => {
  probe = null;
  spawnViewports.clear();
};
