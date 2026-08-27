import type { PtyViewport, UiTerminalAutomationTarget } from "./types";

type PtyViewportProbe = () => PtyViewport | null;

export type UiTerminalOperation =
  | { kind: "query" }
  | { kind: "top" }
  | { kind: "bottom" }
  | { kind: "line"; value: number }
  | { kind: "lines"; value: number }
  | { kind: "pages"; value: number };

export interface UiTerminalControllerInput {
  element: HTMLElement;
  sessionId: string;
  operation: UiTerminalOperation;
}

export type UiTerminalControllerError =
  | "terminal_target_mismatch"
  | "terminal_entry_stale"
  | "terminal_session_not_visible";

export type UiTerminalControllerResult =
  | { ok: true; target: UiTerminalAutomationTarget }
  | { ok: false; error: UiTerminalControllerError; message: string };

type UiTerminalController = (
  input: UiTerminalControllerInput,
) => UiTerminalControllerResult;

let probe: PtyViewportProbe | null = null;
let terminalController: {
  controller: UiTerminalController;
  token: symbol;
} | null = null;

const spawnViewports = new Map<string, PtyViewport>();
const MAX_TRACKED_SPAWN_VIEWPORTS = 32;

export const BACKEND_SPAWN_FLOOR: PtyViewport = { cols: 20, rows: 5 };

const MAX_VIEWPORT_DIMENSION = 65535;

const isHonouredByBackend = (viewport: PtyViewport): boolean =>
  Number.isInteger(viewport.cols) &&
  Number.isInteger(viewport.rows) &&
  viewport.cols >= BACKEND_SPAWN_FLOOR.cols &&
  viewport.rows >= BACKEND_SPAWN_FLOOR.rows &&
  viewport.cols <= MAX_VIEWPORT_DIMENSION &&
  viewport.rows <= MAX_VIEWPORT_DIMENSION;

export const registerPtyViewportProbe = (next: PtyViewportProbe): (() => void) => {
  probe = next;
  return () => {
    if (probe === next) {
      probe = null;
    }
  };
};

export const registerUiTerminalController = (
  controller: UiTerminalController,
): (() => void) => {
  const token = Symbol("ui-terminal-controller");
  terminalController = { controller, token };
  return () => {
    if (terminalController?.token === token) {
      terminalController = null;
    }
  };
};

export const executeUiTerminalController = (
  input: UiTerminalControllerInput,
): UiTerminalControllerResult | null =>
  terminalController?.controller(input) ?? null;

export const resetUiTerminalControllerForTests = (): void => {
  terminalController = null;
};

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

export const takeSpawnViewport = (sessionId: string): PtyViewport | null => {
  const viewport = spawnViewports.get(sessionId);
  if (!viewport) {
    return null;
  }

  spawnViewports.delete(sessionId);
  return viewport;
};

export const resetPtyViewportForTests = (): void => {
  probe = null;
  spawnViewports.clear();
};
