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
  },
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
});
