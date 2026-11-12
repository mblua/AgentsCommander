// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";

const mockState = vi.hoisted(() => ({
  sessions: {
    showCategories: true,
    alwaysShowSelectedWorkgroup: true,
    hydrated: true,
    toggleInFlight: false,
    coordSortByActivity: false,
  },
  settings: {
    soundsEnabled: true,
    themeLight: true,
    specBoardEnabled: false,
  } as { soundsEnabled: boolean; themeLight: boolean; specBoardEnabled: boolean } | null,
  isBrowser: false,
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("../stores/project", () => ({
  projectStore: {
    pickAndCheck: vi.fn(),
    createAndLoad: vi.fn(),
    loadProject: vi.fn(),
  },
}));

vi.mock("../stores/sessions", () => ({
  sessionsStore: {
    get showCategories() {
      return mockState.sessions.showCategories;
    },
    get alwaysShowSelectedWorkgroup() {
      return mockState.sessions.alwaysShowSelectedWorkgroup;
    },
    get coordSortByActivity() {
      return mockState.sessions.coordSortByActivity;
    },
    get hydrated() {
      return mockState.sessions.hydrated;
    },
    get toggleInFlight() {
      return mockState.sessions.toggleInFlight;
    },
    toggleShowCategories: vi.fn(),
    toggleAlwaysShowSelectedWorkgroup: vi.fn(async () => {
      mockState.sessions.alwaysShowSelectedWorkgroup = !mockState.sessions.alwaysShowSelectedWorkgroup;
    }),
    toggleCoordSortByActivity: vi.fn(),
  },
}));

vi.mock("../../shared/ipc", () => ({
  ProjectAPI: {
    checkPath: vi.fn(),
  },
  SettingsAPI: {
    setSoundsEnabled: vi.fn(() => Promise.resolve()),
    setThemeLight: vi.fn(() => Promise.resolve()),
    setMainResourceMonitorAttached: vi.fn(() => Promise.resolve()),
  },
  ResourceMonitorAPI: {
    snapshot: vi.fn(() => Promise.resolve(null)),
  },
  SpecBoardAPI: {
    open: vi.fn(),
  },
  emitThemeChanged: vi.fn(() => Promise.resolve()),
  onOpenSettings: vi.fn(),
}));

vi.mock("../../shared/stores/settings", () => ({
  settingsStore: {
    get current() {
      return mockState.settings;
    },
    refresh: vi.fn(),
  },
}));

vi.mock("../../shared/sound", () => ({
  setSoundsEnabled: vi.fn(),
}));

vi.mock("../../shared/platform", () => ({
  get isBrowser() {
    return mockState.isBrowser;
  },
}));

vi.mock("../../main/stores/home", () => ({
  homeStore: {
    visible: false,
    toggle: vi.fn(),
  },
}));

vi.mock("./SettingsModal", () => ({
  default: () => null,
}));

import ActionBar from "./ActionBar";
import { onOpenSettings, ResourceMonitorAPI } from "../../shared/ipc";
import { resourceMonitorStore } from "../../shared/stores/resourceMonitor";

let dispose: (() => void) | undefined;

function renderActionBar() {
  const root = document.createElement("div");
  document.body.appendChild(root);
  dispose = render(() => ActionBar({}), root);
}

function deferSettingsListener() {
  let resolve!: (unlisten: () => void) => void;
  const pending = new Promise<() => void>((done) => { resolve = done; });
  vi.mocked(onOpenSettings).mockReturnValueOnce(pending);
  return resolve;
}

describe("ActionBar disposal during settings listener registration", () => {
  beforeEach(() => { vi.useFakeTimers(); });

  afterEach(() => {
    dispose?.();
    dispose = undefined;
    resourceMonitorStore.stopPolling();
    document.body.innerHTML = "";
    vi.restoreAllMocks();
    vi.clearAllMocks();
    vi.useRealTimers();
  });

  it("releases a late listener without starting polling after disposal", async () => {
    const add = vi.spyOn(document, "addEventListener");
    const remove = vi.spyOn(document, "removeEventListener");
    const resolve = deferSettingsListener();
    const unlisten = vi.fn();
    renderActionBar();
    expect(onOpenSettings).toHaveBeenCalledOnce();
    dispose?.();
    dispose = undefined;
    resolve(unlisten);
    await Promise.resolve();

    expect(resourceMonitorStore.polling).toBe(false);
    const added = add.mock.calls.filter(([type]) => type === "visibilitychange");
    const removed = remove.mock.calls.filter(([type]) => type === "visibilitychange");
    expect(added.length - removed.length).toBe(0);
    expect(unlisten).toHaveBeenCalledOnce();
    await vi.advanceTimersByTimeAsync(300);
    expect(ResourceMonitorAPI.snapshot).not.toHaveBeenCalled();
  });

  it("starts polling while mounted and releases it on disposal", async () => {
    const add = vi.spyOn(document, "addEventListener");
    const remove = vi.spyOn(document, "removeEventListener");
    const resolve = deferSettingsListener();
    const unlisten = vi.fn();
    renderActionBar();
    resolve(unlisten);
    await Promise.resolve();

    expect(resourceMonitorStore.polling).toBe(true);
    const added = add.mock.calls.filter(([type]) => type === "visibilitychange");
    expect(added).toHaveLength(1);
    dispose?.();
    dispose = undefined;
    expect(resourceMonitorStore.polling).toBe(false);
    const removed = remove.mock.calls.filter(([type]) => type === "visibilitychange");
    expect(removed).toHaveLength(1);
    expect(removed[0][1]).toBe(added[0][1]);
    expect(unlisten).toHaveBeenCalledOnce();
  });
});
