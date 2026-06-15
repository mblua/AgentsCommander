import { render } from "solid-js/web";
import type { JSX } from "solid-js";
import { __setTransportForTests } from "../ipc";
import type {
  AcDiscoveryResult,
  AppSettings,
  BridgeInfo,
  CodingAgentProfilesConfig,
  Session,
} from "../types";
import { FakeTransport } from "./fake-transport";
import { projectStore } from "../../sidebar/stores/project";
import { sessionsStore } from "../../sidebar/stores/sessions";
import { bridgesStore } from "../../sidebar/stores/bridges";
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
    webServerEnabled: false,
    webServerPort: 8765,
    webServerBind: "127.0.0.1",
    projectPath: null,
    projectPaths: [],
    sidebarStyle: "noir-minimal",
    onboardingDismissed: true,
    coordSortByActivity: false,
    alwaysShowSelectedWorkgroup: true,
    injectRtkHook: false,
    rtkPromptDismissed: false,
    informWhenRtkInstalled: true,
    autoGenerateTaskTitle: true,
    agentTemplatesPath: null,
    themeLight: true,
    specBoardEnabled: false,
    ...overrides,
    codingAgentProfiles: overrides.codingAgentProfiles ?? defaultCodingAgentProfiles(),
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
    ...overrides,
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
  sessionsStore.setSessions([]);
  sessionsStore.setActiveId(null);
  sessionsStore.setTeams([]);
  sessionsStore.setRepos([]);
  sessionsStore.setAlwaysShowSelectedWorkgroup(true);
  sessionsStore.setCoordSortByActivity(false);
  sessionsStore.clearDetached();
  bridgesStore.setBridges([]);
  terminalStore.setActiveSession(null, "", "", null, "", null);
  terminalStore.setActiveWorkgroupTask(null);
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
