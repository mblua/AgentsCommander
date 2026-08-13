import { render } from "solid-js/web";
import type { JSX } from "solid-js";
import { __setTransportForTests } from "../ipc";
import type {
  AcDiscoveryResult,
  AppSettings,
  BridgeInfo,
  CodingAgentProfilesConfig,
  ProjectPathResolution,
  Session,
  SettingsSnapshot,
} from "../types";
import { FakeTransport } from "./fake-transport";
import { toastStore } from "../stores/toasts";
import { projectStore } from "../../sidebar/stores/project";
import { replicaVolatileStore } from "../../sidebar/stores/replica-volatile";
import { autoUnarchiveStore } from "../../sidebar/stores/auto-unarchive";
import { sessionsStore } from "../../sidebar/stores/sessions";
import { bridgesStore } from "../../sidebar/stores/bridges";
import { workgroupGroupsStore } from "../../sidebar/stores/workgroup-groups";
import { projectCollapseStore } from "../../sidebar/stores/project-collapse";
import { railCollapseStore } from "../../sidebar/stores/rail-collapse";
import { codingAgentsStore } from "../../sidebar/stores/coding-agents";
import { terminalStore } from "../../terminal/stores/terminal";
import { __resetHomeStoreForTests } from "../../main/stores/home";

export function renderWithFakeTransport(
  component: () => JSX.Element,
  fake = new FakeTransport()
): {
  fake: FakeTransport;
  root: HTMLDivElement;
  cleanup: () => void;
} {
  const restoreTransport = __setTransportForTests(fake);
  const root = document.createElement("div");
  document.body.appendChild(root);
  const dispose = render(component, root);

  return {
    fake,
    root,
    cleanup: () => {
      dispose();
      restoreTransport();
      root.remove();
    },
  };
}

export function click(el: Element): void {
  el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
}

export function contextMenu(el: Element): void {
  el.dispatchEvent(
    new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
      clientX: 80,
      clientY: 96,
    })
  );
}

export function input(el: HTMLInputElement, value: string): void {
  el.value = value;
  el.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertText", data: value }));
}

export async function waitFor(
  assertion: () => void | Promise<void>,
  timeoutMs = 1000
): Promise<void> {
  const started = Date.now();
  let lastError: unknown;

  while (Date.now() - started < timeoutMs) {
    try {
      await assertion();
      return;
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
  }

  if (lastError instanceof Error) {
    throw lastError;
  }
  throw new Error(`Timed out after ${timeoutMs}ms`);
}

function defaultCodingAgentProfiles(): CodingAgentProfilesConfig {
  return {
    schemaVersion: 2,
    profileSlots: {
      A: { label: "" },
    },
    defaultProfileByAgent: {},
    profilesByAgent: {},
    profileLabelsByAgent: {},
  };
}

export function baseSettings(overrides: Partial<AppSettings> = {}): AppSettings {
  return {
    defaultShell: "pwsh",
    defaultShellArgs: [],
    agents: [],
    telegramBots: [],
    telegramNetworkPollErrorLogging: {
      firstFailureLevel: "warn",
      transientRepeatLevel: "debug",
      sustainedLevel: "warn",
      sustainedAfterSeconds: 60,
      sustainedRepeatSeconds: 300,
      recoveryLevel: "info",
    },
    restoreCoordinatorWakeState: true,
    sidebarAlwaysOnTop: false,
    raiseTerminalOnClick: true,
    soundsEnabled: false,
    teamIdleBeepEnabled: false,
    voiceToTextEnabled: false,
    geminiApiKey: "",
    geminiModel: "gemini-2.5-flash",
    voiceAutoExecute: false,
    voiceAutoExecuteDelay: 15,
    sidebarZoom: 1,
    terminalZoom: 1,
    guideZoom: 1,
    mainZoom: 1,
    sidebarGeometry: null,
    terminalGeometry: null,
    mainGeometry: null,
    mainSidebarWidth: 360,
    mainSidebarSide: "right",
    mainAlwaysOnTop: false,
    mainResourceMonitorAttached: false,
    webServerEnabled: false,
    webServerPort: 8765,
    webServerBind: "127.0.0.1",
    apiServerEnabled: false,
    apiServerPort: 8766,
    apiServerBind: "127.0.0.1",
    terminalSnapshotsEnabled: false,
    projectPath: null,
    projectPaths: [],
    archivedProjectPaths: [],
    sidebarStyle: "noir-minimal",
    onboardingDismissed: true,
    coordSortByActivity: false,
    alwaysShowSelectedWorkgroup: true,
    autoGenerateTaskTitle: true,
    agentTemplatesPath: null,
    themeLight: false,
    specBoardEnabled: false,
    gitSweepConcurrency: 1,
    gitSweepMinIntervalSecs: 10,
    resourceMonitorEnabled: true,
    maxConcurrentAgentProcesses: 3,
    resourceWatchdogAction: "warn",
    agentGroupWarnPrivateBytes: 8 * 1024 ** 3,
    agentGroupKillPrivateBytes: 12 * 1024 ** 3,
    agentProcessKillPrivateBytes: 12 * 1024 ** 3,
    resourceKeepLastSnapshot: true,
    resourceBackoffPolling: true,
    coordinatorIdleBadgeYellowMinutes: 30,
    coordinatorIdleBadgeRedMinutes: 60,
    coordinatorAutoCloseEnabled: true,
    coordinatorAutoCloseMinutes: 60,
    coordinatorAutoCloseSkipTelegramAssigned: false,
    coordinatorCascadeCloseEnabled: true,
    npmUpdateNotificationsEnabled: true,
    autoSelfClearEnabled: true,
    autoSelfClearByAgent: {},
    agentAutoUpdateByCommand: {},
    containerCredentialsFromHost: true,
    logLevel: null,
    activityLogEnabled: false,
    ...overrides,
    codingAgentProfiles: overrides.codingAgentProfiles ?? defaultCodingAgentProfiles(),
  };
}

// #1077: baseSettings() stays legacy-shaped (no report) so existing suites keep
// exercising the absent-report legacy fallback. These focused factories build
// the new snapshot/report shape only where a test needs it.
export function projectPathResolution(
  overrides: Partial<ProjectPathResolution> = {}
): ProjectPathResolution {
  return {
    activeRegistrationCount: 0,
    archivedRegistrationCount: 0,
    issues: [],
    reconciliationError: null,
    ...overrides,
  };
}

export function settingsSnapshot(
  settingsOverrides: Partial<AppSettings> = {},
  resolutionOverrides: Partial<ProjectPathResolution> = {}
): SettingsSnapshot {
  return {
    ...baseSettings(settingsOverrides),
    projectPathResolution: projectPathResolution(resolutionOverrides),
  };
}

export function session(overrides: Partial<Session> = {}): Session {
  return {
    id: "session-1",
    name: "wg-1-dev-team/architect",
    shell: "pwsh",
    shellArgs: [],
    effectiveShellArgs: [],
    createdAt: "2026-06-13T00:00:00.000Z",
    workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
    status: "running",
    waitingForInput: false,
    communication: null,
    pendingReview: false,
    lastPrompt: null,
    agentId: null,
    agentLabel: null,
    gitRepos: [],
    workgroupTask: null,
    isCoordinator: false,
    isRootAgent: false,
    token: "",
    agentKind: "codex",
    ...overrides,
    requestedProfile: overrides.requestedProfile ?? null,
    effectiveProfile: overrides.effectiveProfile ?? null,
    profileFallbackChain: overrides.profileFallbackChain ?? [],
    profileFallbackApplied: overrides.profileFallbackApplied ?? false,
  };
}

export function discovery(
  overrides: Partial<AcDiscoveryResult> = {}
): AcDiscoveryResult {
  return {
    agents: [],
    teams: [],
    workgroups: [],
    loops: [],
    ...overrides,
    contextTemplateUpdates: overrides.contextTemplateUpdates ?? [],
  };
}

export function bridge(overrides: Partial<BridgeInfo> = {}): BridgeInfo {
  return {
    sessionId: "session-1",
    botId: "bot-1",
    botLabel: "Ops Bot",
    status: "active",
    color: "#4ade80",
    ...overrides,
  };
}

export function resetUiStoresForTests(): void {
  projectStore.clear();
  // #943 B2 - the volatile layer outlives a render, so without this a test that
  // lands a branch event leaks its repoBranch/repoBranchByPath into the next test
  // in the same file (order-dependent, and silently wrong rather than red).
  replicaVolatileStore.clearAll();
  // #1033 - same hazard, same reason: the context reading map is event-fed and is
  // deliberately out of setSessions' reach, so it survives into the next test.
  sessionsStore.resetContextReadingsForTests();
  sessionsStore.setSessions([]);
  sessionsStore.resetSelectionForTests();
  sessionsStore.setTeams([]);
  sessionsStore.setRepos([]);
  sessionsStore.setAlwaysShowSelectedWorkgroup(true);
  sessionsStore.setCoordSortByActivity(false);
  sessionsStore.clearDetached();
  workgroupGroupsStore.resetForTests();
  projectCollapseStore.resetForTests();
  // #965 - the rail's own collapse state is a module-level signal, so it outlives a
  // render exactly like the stores above. Without this, a test that collapses a
  // header leaks a folded rail into the next test in the same file.
  railCollapseStore.resetForTests();
  codingAgentsStore.resetForTests();
  bridgesStore.setBridges([]);
  terminalStore.resetForTests();
  terminalStore.setActiveWorkgroupTask(null);
  autoUnarchiveStore.acknowledge();
  toastStore.clear();
  __resetHomeStoreForTests();
}

export function installBrowserDomStubs(): () => void {
  const previousResizeObserver = globalThis.ResizeObserver;
  const previousRequestAnimationFrame = globalThis.requestAnimationFrame;
  const previousCancelAnimationFrame = globalThis.cancelAnimationFrame;
  const previousClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
  const previousCanvasGetContext = HTMLCanvasElement.prototype.getContext;

  class NoopResizeObserver implements ResizeObserver {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
  Object.defineProperty(globalThis, "ResizeObserver", {
    configurable: true,
    writable: true,
    value: previousResizeObserver ?? NoopResizeObserver,
  });

  Object.defineProperty(globalThis, "requestAnimationFrame", {
    configurable: true,
    writable: true,
    value:
      previousRequestAnimationFrame ??
      ((callback: FrameRequestCallback) =>
        window.setTimeout(() => callback(performance.now()), 0)),
  });

  Object.defineProperty(globalThis, "cancelAnimationFrame", {
    configurable: true,
    writable: true,
    value: previousCancelAnimationFrame ?? ((handle: number) => window.clearTimeout(handle)),
  });

  if (!navigator.clipboard) {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        readText: async () => "",
        writeText: async () => {},
      },
    });
  }

  const getContextStub = function getContext(this: HTMLCanvasElement) {
    return {
      canvas: this,
      clearRect: () => {},
      fillRect: () => {},
      getImageData: () => ({ data: new Uint8ClampedArray(4) }),
      putImageData: () => {},
      createImageData: () => ({ data: new Uint8ClampedArray(4) }),
      setTransform: () => {},
      drawImage: () => {},
      save: () => {},
      restore: () => {},
      beginPath: () => {},
      moveTo: () => {},
      lineTo: () => {},
      closePath: () => {},
      stroke: () => {},
      translate: () => {},
      scale: () => {},
      rotate: () => {},
      arc: () => {},
      fill: () => {},
      measureText: () => ({ width: 0 }),
      transform: () => {},
      rect: () => {},
      clip: () => {},
    } as unknown as CanvasRenderingContext2D;
  } as unknown as typeof HTMLCanvasElement.prototype.getContext;
  HTMLCanvasElement.prototype.getContext = getContextStub;

  return () => {
    Object.defineProperty(globalThis, "ResizeObserver", {
      configurable: true,
      writable: true,
      value: previousResizeObserver,
    });
    Object.defineProperty(globalThis, "requestAnimationFrame", {
      configurable: true,
      writable: true,
      value: previousRequestAnimationFrame,
    });
    Object.defineProperty(globalThis, "cancelAnimationFrame", {
      configurable: true,
      writable: true,
      value: previousCancelAnimationFrame,
    });
    if (previousClipboard) {
      Object.defineProperty(navigator, "clipboard", previousClipboard);
    } else {
      delete (navigator as unknown as { clipboard?: Clipboard }).clipboard;
    }
    HTMLCanvasElement.prototype.getContext = previousCanvasGetContext;
  };
}

interface PendingAnimationFrame {
  handle: number;
  callback: FrameRequestCallback;
  cancelled: boolean;
}

export interface DeterministicAnimationFrames {
  /** Runs exactly one frame: the callbacks queued right now, and nothing they
   *  queue in turn. Returns whether any callback actually ran, so a caller can
   *  drive frames until an observable appears without guessing a count. A
   *  session switch landing between the two frames of a double
   *  `requestAnimationFrame` cannot be expressed without this. */
  flushFrame: () => Promise<boolean>;
  /** Runs frames until none is left queued. Drains a nested double rAF. */
  flush: () => Promise<void>;
  restore: () => void;
}

/** Ceiling for any loop that drives frames, so an invariant violation fails
 *  bounded instead of hanging. Exported so a caller driving `flushFrame()`
 *  itself uses the same bound rather than inventing one. */
export const MAX_ANIMATION_FRAME_PASSES = 50;
const ANIMATION_FRAME_INTERVAL_MS = 16;

/**
 * Replaces `requestAnimationFrame` with a queue the test drains itself.
 *
 * Opt-in: `installBrowserDomStubs` does not call it and no existing caller
 * changes. Install it AFTER the browser stubs so it is restored BEFORE them —
 * their cleanup reinstalls whatever `requestAnimationFrame` is current, so
 * tearing them down first would leak this one globally.
 *
 * A drained queue means the animation-frame queue is empty. It is NOT viewport
 * quiescence: the 120/240/360 ms `pty_resize` retries and the 500 ms snapshot
 * settle timer are unaffected and still need `waitFor`. And a drained queue is
 * not proof that work happened — a callback that returns at a guard is consumed
 * and does nothing, so a test that needs the work must flush where the guard
 * still passes, or assert the observable result.
 */
export function installDeterministicAnimationFrames(): DeterministicAnimationFrames {
  const previousRequestAnimationFrame = globalThis.requestAnimationFrame;
  const previousCancelAnimationFrame = globalThis.cancelAnimationFrame;

  let queued: PendingAnimationFrame[] = [];
  let running: PendingAnimationFrame[] = [];
  let nextHandle = 1;
  let timestamp = 0;

  const define = (name: string, value: unknown): void => {
    Object.defineProperty(globalThis, name, { configurable: true, writable: true, value });
  };

  const restore = (): void => {
    queued = [];
    running = [];
    define("requestAnimationFrame", previousRequestAnimationFrame);
    define("cancelAnimationFrame", previousCancelAnimationFrame);
  };

  const requestFrame = (callback: FrameRequestCallback): number => {
    const handle = nextHandle;
    nextHandle += 1;
    queued.push({ handle, callback, cancelled: false });
    return handle;
  };

  // Cancelling has to reach the batch currently being invoked as well: if the
  // first callback of a frame cancels the second, the second must not run.
  const cancelFrame = (handle: number): void => {
    for (const frame of queued) {
      if (frame.handle === handle) frame.cancelled = true;
    }
    for (const frame of running) {
      if (frame.handle === handle) frame.cancelled = true;
    }
  };

  const flushFrame = async (): Promise<boolean> => {
    running = queued;
    queued = [];

    // One shared timestamp per frame, as a browser gives every callback in a
    // frame; a per-callback step models no real scheduler.
    const frameTimestamp = timestamp;
    timestamp += ANIMATION_FRAME_INTERVAL_MS;

    let ran = false;
    try {
      for (const frame of running) {
        if (frame.cancelled) continue;
        ran = true;
        frame.callback(frameTimestamp);
      }
    } finally {
      running = [];
    }

    // Yield to both queues so the promises the callbacks started — transport
    // calls in particular — have resolved before the caller asserts.
    await Promise.resolve();
    await new Promise((resolve) => setTimeout(resolve, 0));

    return ran;
  };

  const flush = async (): Promise<void> => {
    for (let pass = 0; pass < MAX_ANIMATION_FRAME_PASSES; pass += 1) {
      await flushFrame();
      if (queued.length === 0) return;
    }

    throw new Error(
      `installDeterministicAnimationFrames: flush() did not settle in ` +
        `${MAX_ANIMATION_FRAME_PASSES} passes, ${queued.length} frame(s) still queued`
    );
  };

  // Cleanup after a failed install, per property and best effort. `restore()`
  // stops at its first failing `define`, which on a partial install would leave
  // the other global in place; each property is attempted on its own here so
  // whatever can be undone is undone.
  const restoreAfterFailedInstall = (): void => {
    queued = [];
    running = [];

    for (const [name, previous] of [
      ["requestAnimationFrame", previousRequestAnimationFrame],
      ["cancelAnimationFrame", previousCancelAnimationFrame],
    ] as const) {
      try {
        define(name, previous);
      } catch {
        // Swallowed on purpose: a cleanup failure must not become the reported
        // cause. See the rethrow below.
      }
    }
  };

  try {
    define("requestAnimationFrame", requestFrame);
    define("cancelAnimationFrame", cancelFrame);
  } catch (error) {
    // 5.4: cleanup "must not mask an error thrown during install". The call is
    // guarded as well as best effort, so `throw error` is reached even if this
    // cleanup path is ever changed into one that can throw again.
    try {
      restoreAfterFailedInstall();
    } catch {
      // Swallowed on purpose: the install error is the one that must surface.
    }
    throw error;
  }

  return { flushFrame, flush, restore };
}
