/**
 * #1652 - the renderer half of the IPC black box.
 *
 * Records what only the renderer can see - which `invoke()` calls are in
 * flight, whether the JS task loop is still running, whether frames are still
 * produced, whether pointer input still reaches JS and whether backend events
 * still arrive - into `localStorage`, which survives a force-kill. The next app
 * start hands the previous run's records to phase 1's `ipc_blackbox_report`
 * command, which classifies them against its own freeze marker.
 *
 * MODULE BOUNDARY (load-bearing). This module must not import the IPC funnel,
 * the transport interface or either transport implementation, at value or at
 * type: `transport-tauri.ts` imports THIS file, so any edge back would be a
 * cycle and `npm run check:frontend-dependencies` refuses it. That is why
 * `harvestIpcBlackBox` takes its sender as a parameter instead of reaching for
 * a typed API itself; `main.tsx`, which already imports both sides, wires them.
 *
 * At MODULE SCOPE nothing here touches `window`, `document` or `localStorage`:
 * node-environment vitest files reach this file transitively through the IPC
 * funnel and the Tauri transport. Every DOM and storage access lives inside
 * `installIpcBlackBox()`, the tick, or a listener.
 *
 * Coverage is app commands only - 151 of 151 app command call sites, which is
 * NOT the same as all renderer-to-backend IPC. Tauri plugin IPC
 * (`plugin:window|*`, `plugin:webview|*`, `plugin:dialog|open`,
 * `plugin:event|*`) never passes through a transport and phase 1's observer
 * cannot see it either, so both sides stay symmetric. Phase 1 prints that
 * residual on every record block rather than closing it by wrapping
 * `window.__TAURI_INTERNALS__.invoke`, which would put a diagnostic in the path
 * of every IPC call the app makes.
 */

const SCHEMA_VERSION = 1;
const TICK_MS = 1_000;
const OVERDUE_MS = 5_000;
const POINTER_THROTTLE_MS = 250;
const MAX_PENDING_RECORDED = 32;
const MAX_PER_COMMAND_ENTRIES = 192;
const MAX_HARVESTED_RECORDS = 16;
const CURRENT_PREFIX = "ac.ipc.bb.cur.";
const ROTATED_PREFIX = "ac.ipc.bb.prev.";
const SCAN_PREFIX = "ac.ipc.bb.";

/** Three commands legitimately pend until the user acts: all three are
 *  `rfd::AsyncFileDialog` (`agent_creator.rs:7-19`, `spec_board.rs:246-254`,
 *  `spec_board.rs:332`). They stay in `pending` so the record is complete, but
 *  they are never marked `overdue` and never counted in `overdueTotal`, so the
 *  5 s sweeper does not report a modal file picker as a frozen call. */
const NEVER_OVERDUE = new Set(["pick_folder", "spec_board_pick_open", "spec_board_pick_save"]);

/**
 * Mirrors phase 1's `BlackBoxRecord` field for field, camelCase, 23 fields, and
 * nothing else.
 *
 * `overdueTotal` is the uncapped count of currently-overdue calls, computed over
 * the whole `pending` map rather than over the capped array: with `pending`
 * capped at `MAX_PENDING_RECORDED` the count is not recoverable from the record.
 */
export interface IpcBlackBoxRecord {
  v: number; label: string; windowType: string;
  startedAtMs: number; writtenAtMs: number;
  tickSeq: number; rafSeq: number; lastRafAtMs: number; visible: boolean;
  closedCleanly: boolean;
  lastPointerAtMs: number; lastEventAtMs: number; lastEventName: string;
  probeSeq: number | null; probeAtMs: number | null;
  sent: number; settled: number; lastSettledAtMs: number; lastSentAtMs: number;
  pendingTotal: number; overdueTotal: number;
  pending: { id: number; cmd: string; ageMs: number; overdue: boolean }[];
  perCommand: Record<string, [number, number]>;
}

export interface StoredBlackBox { key: string; json: string }

let nextId = 1;
let pending = new Map<number, { cmd: string; startedAtMs: number }>();
let perCommand = new Map<string, [number, number]>();
let sent = 0;
let settled = 0;
let lastSettledAtMs = 0;
let tickSeq = 0;
let rafSeq = 0;
let lastRafAtMs = 0;
let lastPointerAtMs = 0;
let lastPointerMoveNotedAtMs = 0;
let lastEventAtMs = 0;
let lastEventName = "";
let probeSeq: number | null = null;
let probeAtMs: number | null = null;
let startedAtMs = 0;
let label = "web";
let windowType = "main";
let storageKey = "";

let visible = false;        // seeded from document.visibilityState inside installIpcBlackBox()
let closedCleanly = false;  // set true by the pagehide handler, which then writes at once
let lastSentAtMs = 0;       // stamped by noteInvokeStart, BEFORE the call reaches the transport

/** Stored, not merely a boolean: a second caller must AWAIT the first install
 *  rather than return early, or `harvestIpcBlackBox`'s payload is not
 *  deterministic. */
let installed: Promise<void> | null = null;
let teardown: Array<() => void> = [];

/**
 * Assign an id to an outgoing call and stamp the send-side evidence.
 *
 * `lastSentAtMs` is taken from the SAME clock read as the entry's
 * `startedAtMs`, here rather than after the call, which is what makes it true
 * for a send path that throws as well as one that hangs. It carries one fact:
 * this window handed a call to the transport at that instant. Phase 1's
 * `b/send-path-broken` arm requires it at or after the silence onset, because
 * the silence probe is emitted unscoped to every window and a probe alone
 * proves only that RECEIVE worked.
 *
 * Always active, whether or not `installIpcBlackBox` ran: a call issued before
 * install is still in the registry, and a test that never installs simply never
 * writes storage. Pure in-memory, no I/O.
 */
export function noteInvokeStart(cmd: string): number {
  const now = Date.now();
  const id = nextId++;
  pending.set(id, { cmd, startedAtMs: now });
  sent += 1;
  lastSentAtMs = now;
  const counts = perCommand.get(cmd);
  if (counts) {
    counts[0] += 1;
    return id;
  }
  if (perCommand.size >= MAX_PER_COMMAND_ENTRIES) {
    const oldest = perCommand.keys().next();
    if (!oldest.done) perCommand.delete(oldest.value);
  }
  perCommand.set(cmd, [1, 0]);
  return id;
}

/** Settle a call. Unknown ids are ignored. */
export function noteInvokeSettle(id: number): void {
  const entry = pending.get(id);
  if (!entry) return;
  pending.delete(id);
  settled += 1;
  lastSettledAtMs = Date.now();
  const counts = perCommand.get(entry.cmd);
  if (counts) counts[1] += 1;
}

/** The backend -> frontend liveness leg. `pty_output` is what keeps it warm. */
export function noteEvent(name: string): void {
  lastEventAtMs = Date.now();
  lastEventName = name;
}

function buildRecord(now: number): IpcBlackBoxRecord {
  // `pending` is emitted oldest-id first: ids come from a monotonic counter, so
  // insertion order IS issue order, and the cap therefore KEEPS the oldest
  // outstanding calls rather than dropping them. That is what lets phase 1 read
  // the oldest `overdue` entry off the front of the array without knowing
  // whether the cap bit.
  const emitted: IpcBlackBoxRecord["pending"] = [];
  let pendingTotal = 0;
  let overdueTotal = 0;
  for (const [id, entry] of pending) {
    const ageMs = now - entry.startedAtMs;
    const overdue = ageMs >= OVERDUE_MS && !NEVER_OVERDUE.has(entry.cmd);
    pendingTotal += 1;
    if (overdue) overdueTotal += 1;
    if (emitted.length < MAX_PENDING_RECORDED) {
      emitted.push({ id, cmd: entry.cmd, ageMs, overdue });
    }
  }
  return {
    v: SCHEMA_VERSION,
    label,
    windowType,
    startedAtMs,
    writtenAtMs: now,
    tickSeq,
    rafSeq,
    lastRafAtMs,
    visible,
    closedCleanly,
    lastPointerAtMs,
    lastEventAtMs,
    lastEventName,
    probeSeq,
    probeAtMs,
    sent,
    settled,
    lastSettledAtMs,
    lastSentAtMs,
    pendingTotal,
    overdueTotal,
    pending: emitted,
    perCommand: Object.fromEntries(perCommand),
  };
}

/**
 * The one writer. The heartbeat and the `pagehide` handler both call it, so the
 * record shape cannot drift between them.
 *
 * The clock is read ONCE, into `now`, and that one value serves `writtenAtMs`
 * and every `ageMs`: phase 1 recovers a pending call's issue instant as
 * `writtenAtMs - ageMs`, so the two must share a base.
 *
 * A quota or `SecurityError` must never throw out of the timer and stop the
 * heartbeat, so the write is caught and swallowed - a window that can no longer
 * persist keeps ticking, quietly.
 */
function writeRecord(): void {
  if (!storageKey) return;
  try {
    localStorage.setItem(storageKey, JSON.stringify(buildRecord(Date.now())));
  } catch {
    /* quota or storage unavailable - never break the heartbeat */
  }
}

function tick(): void {
  tickSeq += 1;
  writeRecord();
}

/**
 * Idempotent in the STORED-PROMISE form: a second call, including the one
 * inside `harvestIpcBlackBox`, awaits the first install rather than starting a
 * second one or returning early. Never rejects: every step is individually
 * try/catch-ed.
 */
export function installIpcBlackBox(): Promise<void> {
  if (installed) return installed;
  installed = runInstall();
  return installed;
}

async function runInstall(): Promise<void> {
  startedAtMs = Date.now();

  try {
    const { getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    label = getCurrentWebviewWindow().label;
  } catch {
    // Non-Tauri, or the `metadata` failure the Tauri transport already handles.
    label = "web";
  }

  try {
    windowType = new URLSearchParams(window.location.search).get("window") ?? "main";
  } catch {
    windowType = "main";
  }

  storageKey = CURRENT_PREFIX + label;

  // Rotate BEFORE the first write: this is what stops this run's first tick
  // from overwriting the record the previous run left under the same label.
  try {
    const previous = localStorage.getItem(storageKey);
    if (previous !== null) {
      localStorage.setItem(ROTATED_PREFIX + label, previous);
      localStorage.removeItem(storageKey);
    }
  } catch {
    /* storage unavailable - the live window still ticks */
  }

  // The seed. Module scope may not read `document`, and `visibilitychange`
  // fires only on a transition, so without this a window that never toggles
  // keeps the load-time literal for life: `false` makes phase 1's frame-stall
  // row unreachable, `true` makes every minimized window report it.
  try {
    visible = document.visibilityState === "visible";
  } catch {
    /* keep the literal */
  }

  try {
    const notePointerDown = () => {
      lastPointerAtMs = Date.now();
    };
    const notePointerMove = () => {
      const now = Date.now();
      if (now - lastPointerMoveNotedAtMs < POINTER_THROTTLE_MS) return;
      lastPointerMoveNotedAtMs = now;
      lastPointerAtMs = now;
    };
    window.addEventListener("pointerdown", notePointerDown, { capture: true, passive: true });
    window.addEventListener("pointermove", notePointerMove, { capture: true, passive: true });
    teardown.push(() => {
      window.removeEventListener("pointerdown", notePointerDown, { capture: true });
      window.removeEventListener("pointermove", notePointerMove, { capture: true });
    });
  } catch {
    /* no pointer liveness leg */
  }

  try {
    const noteVisibility = () => {
      visible = document.visibilityState === "visible";
    };
    document.addEventListener("visibilitychange", noteVisibility, { passive: true });
    teardown.push(() => document.removeEventListener("visibilitychange", noteVisibility));
  } catch {
    /* no visibility leg */
  }

  try {
    let handle = requestAnimationFrame(function step() {
      rafSeq += 1;
      lastRafAtMs = Date.now();
      handle = requestAnimationFrame(step);
    });
    teardown.push(() => cancelAnimationFrame(handle));
  } catch {
    // A registration that threw leaves `rafSeq` at 0, which phase 1's block
    // prints: a genuine frame stall carries a non-zero `rafSeq` that stopped
    // advancing.
  }

  // The clean-close mark. NOTHING is removed: `pagehide` is a JS callback, so
  // it discriminates whether the task loop was alive at teardown, not whether
  // the window behaved - and the loop is alive in phase 1's rows (b), (c) and
  // (d), which are reached only when it is. Deleting the record would erase the
  // evidence for three of the four rows. Do not "optimise" this back into a
  // `removeItem`.
  try {
    const markClosed = () => {
      closedCleanly = true;
      writeRecord();
    };
    window.addEventListener("pagehide", markClosed);
    teardown.push(() => window.removeEventListener("pagehide", markClosed));
  } catch {
    /* no clean-close mark - the record still reads as force-killed */
  }

  // Registered directly rather than through the transport on purpose: the probe
  // must be counted separately from ordinary traffic, so it must not move
  // `lastEventAtMs`.
  try {
    const { listen } = await import("@tauri-apps/api/event");
    const unlisten = await listen<{ seq: number; backendNowMs: number }>(
      "ipc_silence_probe",
      (e) => {
        probeSeq = e.payload.seq;
        probeAtMs = Date.now();
      }
    );
    teardown.push(() => unlisten());
  } catch {
    /* non-Tauri or event bus unavailable */
  }

  try {
    const timer = setInterval(tick, TICK_MS);
    teardown.push(() => clearInterval(timer));
  } catch {
    /* no heartbeat */
  }
}

/**
 * Hand every OTHER window's stored record to `send` and delete exactly the keys
 * it asks for.
 *
 * Step 0 - awaiting the install - is what makes the payload deterministic: this
 * window's own key is resolved and already rotated by the time the scan runs.
 * The whole body is caught: a rejected `send` (old backend, command
 * unavailable) must leave the records in place for the next start and must not
 * break startup.
 *
 * Which records belong to a PREVIOUS run is decided by phase 1, by comparing
 * `startedAtMs` against the backend's own start, not here. `send` is therefore
 * called even with nothing found: that call is what lets phase 1's marker-only
 * branch report a freeze whose `localStorage` did not survive.
 */
export async function harvestIpcBlackBox(
  send: (records: StoredBlackBox[]) => Promise<string[]>
): Promise<void> {
  try {
    await installIpcBlackBox();
    const ownKey = CURRENT_PREFIX + label;
    const rotated: StoredBlackBox[] = [];
    const current: StoredBlackBox[] = [];
    for (let i = 0; i < localStorage.length; i += 1) {
      const key = localStorage.key(i);
      if (key === null || !key.startsWith(SCAN_PREFIX) || key === ownKey) continue;
      const json = localStorage.getItem(key);
      if (json === null) continue;
      (key.startsWith(ROTATED_PREFIX) ? rotated : current).push({ key, json });
    }
    const found = rotated.concat(current).slice(0, MAX_HARVESTED_RECORDS);
    const doomed = await send(found);
    for (const key of doomed) {
      localStorage.removeItem(key);
    }
  } catch {
    /* leave every record in place for the next start */
  }
}

export function __resetIpcBlackBoxForTests(): void {
  if (import.meta.env.MODE !== "test") {
    throw new Error("__resetIpcBlackBoxForTests is test-only");
  }

  for (const undo of teardown) {
    try {
      undo();
    } catch {
      /* a listener that never registered */
    }
  }
  teardown = [];
  installed = null;
  nextId = 1;
  pending = new Map();
  perCommand = new Map();
  sent = 0;
  settled = 0;
  lastSettledAtMs = 0;
  tickSeq = 0;
  rafSeq = 0;
  lastRafAtMs = 0;
  lastPointerAtMs = 0;
  lastPointerMoveNotedAtMs = 0;
  lastEventAtMs = 0;
  lastEventName = "";
  probeSeq = null;
  probeAtMs = null;
  startedAtMs = 0;
  label = "web";
  windowType = "main";
  storageKey = "";
  visible = false;
  closedCleanly = false;
  lastSentAtMs = 0;
}
