import { createSignal } from "solid-js";

/**
 * #2236 — the compact sidebar rail width in TypeScript. Pinned equal to the
 * `--ac-rail-width` token phase 1 lands in `src/sidebar/styles/variables.css`
 * by `sidebar-compact.test.ts`; this is the number's only TS home.
 */
export const RAIL_WIDTH_PX = 68;

/** #2236 D16a — declared here and nowhere else; phase 5 consumes it. No
 *  parsing or matching lives in this module. */
export const DEFAULT_SIDEBAR_COMPACT_HOTKEY = "Ctrl+Shift+E";

export type CompactHostHooks = {
  onBeforeModeChange?: (next: boolean) => void;
};

const [sidebarCompact, setSidebarCompactSignal] = createSignal(false);
const [railNudgePx, setRailNudgePx] = createSignal(0);
const [restoreWidthPx, setRestoreWidthPx] = createSignal(0); // epic D22
const [currentHotkey, setSidebarCompactHotkey] = createSignal(
  DEFAULT_SIDEBAR_COMPACT_HOTKEY,
);

type CompactHostRegistration = { hooks: CompactHostHooks };

// Reference-counted (registration identity, not hooks identity): both hosts may
// mount in one document, and each registration's hooks are released with it.
const compactHosts = new Set<CompactHostRegistration>();

export function registerCompactHost(hooks: CompactHostHooks = {}): () => void {
  const registration: CompactHostRegistration = { hooks };
  compactHosts.add(registration);
  let released = false;
  return () => {
    if (released) {
      return;
    }
    released = true;
    compactHosts.delete(registration);
  };
}

export function hasCompactHost(): boolean {
  return compactHosts.size > 0;
}

/**
 * The ONLY way the mode changes. Every route goes through it (epic D21), so
 * the host hook always runs while the signal still holds the outgoing mode —
 * that is what lets a host cancel its pulse, snapshot or restore the width and
 * start the animation against the mode it is leaving.
 *
 * `setSidebarCompactSignal` is deliberately not exported: three routes, one
 * cancellation point, structurally.
 */
export function setSidebarCompactMode(next: boolean): void {
  if (next === sidebarCompact()) {
    return;
  }
  if (!hasCompactHost()) {
    return;
  }
  for (const { hooks } of [...compactHosts]) {
    hooks.onBeforeModeChange?.(next);
  }
  setSidebarCompactSignal(next);
}

/** = setSidebarCompactMode(!sidebarCompact()) */
export function toggleSidebarCompact(): void {
  setSidebarCompactMode(!sidebarCompact());
}

export {
  sidebarCompact,
  railNudgePx,
  setRailNudgePx,
  restoreWidthPx,
  setRestoreWidthPx,
  currentHotkey,
  setSidebarCompactHotkey,
};
