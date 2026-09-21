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

/**
 * #2236 D25 — the named seams the #1532 pulse routes its seven width
 * couplings through. Each member is the identity in expanded mode and reads
 * or writes the rail nudge while compact, so the pulse keeps comparing the
 * width it actually owns instead of the hidden expanded width.
 */
export type PulseWidthSeams = {
  readPulseWidth(): number;
  writePulseWidth(px: number): void;
  clampPulseWidth(px: number): number;
  pulseWidthIsApplied(px: number): boolean;
};

/** Empty unless a test installed a partial override. */
let pulseSeamOverrides: Partial<PulseWidthSeams> = {};

/**
 * @internal test seam — replaces individual members of the returned object;
 * empty by default. Cleared by the harness reset, so a leaked override cannot
 * silently disable a later file's negative controls.
 */
export function setPulseSeamOverridesForTests(
  partial: Partial<PulseWidthSeams> | null,
): void {
  pulseSeamOverrides = partial ?? {};
}

export function createPulseWidthSeams(deps: {
  readExpanded(): number;
  writeExpanded(px: number): void;
  clampExpanded(px: number): number;
  paneStyleWidth(): string;
}): PulseWidthSeams {
  const fallback: PulseWidthSeams = {
    readPulseWidth: () =>
      sidebarCompact() ? RAIL_WIDTH_PX + railNudgePx() : deps.readExpanded(),
    writePulseWidth: (px) => {
      if (sidebarCompact()) {
        setRailNudgePx(px - RAIL_WIDTH_PX);
      } else {
        deps.writeExpanded(px);
      }
    },
    clampPulseWidth: (px) => (sidebarCompact() ? px : deps.clampExpanded(px)),
    pulseWidthIsApplied: (px) =>
      sidebarCompact()
        ? railNudgePx() === px - RAIL_WIDTH_PX
        : deps.paneStyleWidth() === `${px}px`,
  };
  return {
    readPulseWidth: () =>
      (pulseSeamOverrides.readPulseWidth ?? fallback.readPulseWidth)(),
    writePulseWidth: (px) =>
      (pulseSeamOverrides.writePulseWidth ?? fallback.writePulseWidth)(px),
    clampPulseWidth: (px) =>
      (pulseSeamOverrides.clampPulseWidth ?? fallback.clampPulseWidth)(px),
    pulseWidthIsApplied: (px) =>
      (pulseSeamOverrides.pulseWidthIsApplied ?? fallback.pulseWidthIsApplied)(px),
  };
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
