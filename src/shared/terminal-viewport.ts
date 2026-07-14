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
 * The smallest viewport the backend will actually open a PTY at.
 *
 * MIRRORS `PtyViewport::PLAUSIBLE_MIN` in `src-tauri/src/pty/backend.rs`. The two
 * are not allowed to drift, and a comment cannot enforce that: `terminal-viewport
 * .test.ts` reads the Rust source and fails if this constant stops matching it.
 *
 * Below the floor the backend does not fail the spawn — it opens the PTY at
 * 120x30 and warns. So this side must never SEND a below-floor size, and above
 * all never RECORD one. See `isHonouredByBackend`.
 */
export const BACKEND_SPAWN_FLOOR: PtyViewport = { cols: 20, rows: 5 };

/** `u16` on the Rust side: a bigger number is not a viewport, it is a bug. */
const MAX_VIEWPORT_DIMENSION = 65535;

/**
 * True only when the backend will genuinely open the PTY at this size.
 *
 * This gate is load-bearing, not hygiene. Recording a size the PTY was NOT
 * opened at is the one way this fix can wedge a terminal: the dedup would treat
 * the corrective resize as a no-op and skip it, leaving the PTY at a size the
 * view never agreed to, forever. Three backend rules decide it:
 *
 *   - `create_session` builds a viewport only when BOTH cols and rows arrive.
 *   - `PtyViewport::from_fit` opens at 120x30 instead, for anything below
 *     `BACKEND_SPAWN_FLOOR`. A collapsed-but-laid-out box does NOT measure zero:
 *     xterm's fit clamps to its own MINIMUM_COLS = 2 / MINIMUM_ROWS = 1, so it
 *     hands back a perfectly well-formed 2x1 that sails through a `> 0` check
 *     and that the backend then silently declines to use. Recorded, that 2x1 is
 *     the wedge — and it is not a corner case: the probe is exact by
 *     construction, so a box that is collapsed at create time is still collapsed
 *     at attach time, the fit finds the terminal already at 2x1, and the
 *     corrective resize is deduped away before it is ever sent.
 *   - `u16` on the Rust side, so the range is checked here too.
 *
 * The worst xterm itself can hand back is NaN (`CoreTerminal.resize` rejects it,
 * `FitAddon` can propagate it), which `Number.isInteger` catches. A zero can only
 * come off the WIRE, from a web-transport client that is not xterm.
 */
const isHonouredByBackend = (viewport: PtyViewport): boolean =>
  Number.isInteger(viewport.cols) &&
  Number.isInteger(viewport.rows) &&
  viewport.cols >= BACKEND_SPAWN_FLOOR.cols &&
  viewport.rows >= BACKEND_SPAWN_FLOOR.rows &&
  viewport.cols <= MAX_VIEWPORT_DIMENSION &&
  viewport.rows <= MAX_VIEWPORT_DIMENSION;

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
