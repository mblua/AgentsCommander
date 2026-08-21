// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from "vitest";
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
  GuideAPI: {
    open: vi.fn(),
  },
  SettingsAPI: {
    setSoundsEnabled: vi.fn(() => Promise.resolve()),
    setThemeLight: vi.fn(() => Promise.resolve()),
    setMainResourceMonitorAttached: vi.fn(() => Promise.resolve()),
  },
  SpecBoardAPI: {
    open: vi.fn(),
  },
  emitThemeChanged: vi.fn(() => Promise.resolve()),
  onOpenSettings: vi.fn(() => Promise.resolve(() => undefined)),
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
import { projectStore } from "../stores/project";
import {
  centralViewStore,
  __resetCentralViewStoreForTests,
} from "../../main/stores/centralView";

function renderActionBar() {
  const root = document.createElement("div");
  document.body.appendChild(root);
  const dispose = render(() => ActionBar({}), root);
  const pinButton = root.querySelector<HTMLButtonElement>(
    '[data-ac-testid="actionBar.pinSelectedWorkgroup"]'
  );
  if (!pinButton) throw new Error("pin selected workgroup button not rendered");
  return { dispose, pinButton };
}

describe("ActionBar selected workgroup visibility toggle", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    mockState.sessions.showCategories = true;
    mockState.sessions.alwaysShowSelectedWorkgroup = true;
    mockState.sessions.hydrated = true;
    mockState.sessions.toggleInFlight = false;
    mockState.sessions.coordSortByActivity = false;
    mockState.isBrowser = false;
    mockState.settings = {
      soundsEnabled: true,
      themeLight: true,
      specBoardEnabled: false,
    };
    __resetCentralViewStoreForTests();
    vi.clearAllMocks();
  });

  it("uses positive wording when selected workgroup pinning is on", () => {
    mockState.sessions.alwaysShowSelectedWorkgroup = true;
    const { dispose, pinButton } = renderActionBar();

    expect(pinButton.title).toBe("Always keep selected workgroup visible");
    expect(pinButton.getAttribute("aria-label")).toBe("Always keep selected workgroup visible");
    expect(pinButton.getAttribute("aria-pressed")).toBe("true");
    expect(pinButton.getAttribute("data-ac-state")).toBe("pinned");
    expect(pinButton.title).not.toContain("Don't force");

    dispose();
  });

  it("keeps the same positive label when selected workgroup pinning is off", () => {
    mockState.sessions.alwaysShowSelectedWorkgroup = false;
    const { dispose, pinButton } = renderActionBar();

    expect(pinButton.title).toBe("Always keep selected workgroup visible");
    expect(pinButton.getAttribute("aria-label")).toBe("Always keep selected workgroup visible");
    expect(pinButton.getAttribute("aria-pressed")).toBe("false");
    expect(pinButton.getAttribute("data-ac-state")).toBe("default");
    expect(pinButton.title).not.toContain("Don't force");

    dispose();
  });

  // #587 — the ▦ Resource Monitor button reflects whether RM occupies the
  // central pane (mirrors the 🏠 Home toggle's active state).
  it("marks the Resource Monitor button active when RM is the central view", () => {
    centralViewStore.setInitialView("resourceMonitor");
    const { dispose } = renderActionBar();
    const rmButton = document.body.querySelector<HTMLButtonElement>(
      '[data-ac-testid="actionBar.resourceMonitor"]'
    );
    if (!rmButton) throw new Error("resource monitor button not rendered");

    expect(rmButton.className).toContain("active");
    expect(rmButton.getAttribute("aria-pressed")).toBe("true");

    dispose();
  });

  it("does not mark the Resource Monitor button active when the terminal is shown", () => {
    centralViewStore.setInitialView("terminal");
    const { dispose } = renderActionBar();
    const rmButton = document.body.querySelector<HTMLButtonElement>(
      '[data-ac-testid="actionBar.resourceMonitor"]'
    );
    if (!rmButton) throw new Error("resource monitor button not rendered");

    expect(rmButton.className).not.toContain("active");
    expect(rmButton.getAttribute("aria-pressed")).toBe("false");

    dispose();
  });

  // #289 / dark-default — before settings load, settingsStore.current is null,
  // so the theme glyph falls back to AppSettings::default. That default is now
  // dark (themeLight: false), so the pre-load glyph must be the moon, not the
  // legacy sun.
  it("falls back to the dark theme glyph before settings load", () => {
    mockState.settings = null;
    const { dispose } = renderActionBar();
    const themeButton = document.body.querySelector<HTMLButtonElement>(
      '[data-ac-testid="actionBar.theme"]'
    );
    if (!themeButton) throw new Error("theme button not rendered");

    // 🌙 = moon (dark glyph); ☀ = sun (legacy light glyph).
    expect(themeButton.textContent).toContain("🌙");
    expect(themeButton.textContent).not.toContain("☀");
    expect(themeButton.getAttribute("data-ac-state")).toBe("disabled");

    dispose();
  });

  it("shows the browser create-project notice without invoking the folder picker flow", () => {
    mockState.isBrowser = true;
    const { dispose } = renderActionBar();
    const dropdownButton = document.body.querySelector<HTMLButtonElement>(
      '[data-ac-testid="actionBar.newOpen"]'
    );
    if (!dropdownButton) throw new Error("new/open dropdown button not rendered");

    dropdownButton.click();

    const newProjectItem = document.body.querySelector<HTMLButtonElement>(
      '[data-ac-testid="actionBar.menu.newProject"]'
    );
    if (!newProjectItem) throw new Error("new project menu item not rendered");

    newProjectItem.click();

    expect(projectStore.pickAndCheck).not.toHaveBeenCalled();
    const modal = document.body.querySelector<HTMLElement>(
      '[data-ac-testid="project.browserCreateNotice.dialog"]'
    );
    expect(modal).not.toBeNull();
    expect(modal?.textContent).toContain("Create a project from the desktop app");
    expect(modal?.textContent).toContain(
      "Creating a new project isn't available in the browser view."
    );

    const dismiss = document.body.querySelector<HTMLButtonElement>(
      '[data-ac-testid="project.browserCreateNotice.dismiss"]'
    );
    if (!dismiss) throw new Error("browser create-project dismiss button not rendered");
    dismiss.click();

    expect(
      document.body.querySelector('[data-ac-testid="project.browserCreateNotice.dialog"]')
    ).toBeNull();

    dispose();
  });

  it("keeps the desktop new-project flow unchanged", async () => {
    vi.mocked(projectStore.pickAndCheck).mockResolvedValue({
      picked: "C:\\Projects\\Example",
      hasAcRoot: false,
    });

    const { dispose } = renderActionBar();
    const dropdownButton = document.body.querySelector<HTMLButtonElement>(
      '[data-ac-testid="actionBar.newOpen"]'
    );
    if (!dropdownButton) throw new Error("new/open dropdown button not rendered");

    dropdownButton.click();

    const newProjectItem = document.body.querySelector<HTMLButtonElement>(
      '[data-ac-testid="actionBar.menu.newProject"]'
    );
    if (!newProjectItem) throw new Error("new project menu item not rendered");

    newProjectItem.click();
    await Promise.resolve();
    await Promise.resolve();

    expect(projectStore.pickAndCheck).toHaveBeenCalledTimes(1);
    expect(projectStore.createAndLoad).toHaveBeenCalledWith("C:\\Projects\\Example");
    expect(
      document.body.querySelector('[data-ac-testid="project.browserCreateNotice.dialog"]')
    ).toBeNull();

    dispose();
  });
});
