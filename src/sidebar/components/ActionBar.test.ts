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

vi.mock("../../shared/stores/resourceMonitor", () => ({
  resourceMonitorStore: {
    snapshot: null,
    error: null,
    startPolling: vi.fn(() => vi.fn()),
    stopPolling: vi.fn(),
  },
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

import { onOpenSettings } from "../../shared/ipc";
import { resourceMonitorStore } from "../../shared/stores/resourceMonitor";

import ActionBar, { activeClass, computeResourceBadgeState, triState } from "./ActionBar";
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

    expect(pinButton.title).toBe("Always keep selected room visible");
    expect(pinButton.getAttribute("aria-label")).toBe("Always keep selected room visible");
    expect(pinButton.getAttribute("aria-pressed")).toBe("true");
    expect(pinButton.getAttribute("data-ac-state")).toBe("pinned");
    expect(pinButton.title).not.toContain("Don't force");

    dispose();
  });

  it("keeps the same positive label when selected workgroup pinning is off", () => {
    mockState.sessions.alwaysShowSelectedWorkgroup = false;
    const { dispose, pinButton } = renderActionBar();

    expect(pinButton.title).toBe("Always keep selected room visible");
    expect(pinButton.getAttribute("aria-label")).toBe("Always keep selected room visible");
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
      hasWorkspace: false,
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
  // #2611 — byte-equal class / data-ac-state strings for the toolbar buttons
  // whose ternaries moved into triState/activeClass.
  function buttonAttrs(testId: string): [string | null, string | null] {
    const el = document.body.querySelector<HTMLElement>(`[data-ac-testid="${testId}"]`);
    if (!el) throw new Error(`missing ${testId}`);
    return [el.getAttribute("class"), el.getAttribute("data-ac-state")];
  }

  it("renders the default toolbar class and state strings", () => {
    const { dispose } = renderActionBar();
    expect(buttonAttrs("actionBar.newOpen")).toEqual(["action-bar-dropdown-btn", "closed"]);
    expect(buttonAttrs("actionBar.home")).toEqual(["toolbar-gear-btn ", "hidden"]);
    expect(buttonAttrs("actionBar.sortCoordinators")).toEqual([
      "toolbar-gear-btn coord-sort-activity-btn ",
      "default",
    ]);
    expect(buttonAttrs("actionBar.sounds")).toEqual(["toolbar-gear-btn ", "audible"]);
    expect(buttonAttrs("actionBar.categories")).toEqual([
      "toolbar-gear-btn show-categories-btn active",
      "visible",
    ]);
    expect(buttonAttrs("actionBar.pinSelectedWorkgroup")).toEqual([
      "toolbar-gear-btn show-categories-btn active",
      "pinned",
    ]);
    expect(buttonAttrs("actionBar.theme")).toEqual(["toolbar-gear-btn", "light"]);
    expect(buttonAttrs("actionBar.resourceMonitor")[0]).toBe(
      "toolbar-gear-btn resource-monitor-btn state-unknown ",
    );
    dispose();
  });

  it("renders the toggled toolbar class and state strings", () => {
    mockState.sessions.coordSortByActivity = true;
    mockState.sessions.showCategories = false;
    mockState.sessions.alwaysShowSelectedWorkgroup = false;
    mockState.settings = { soundsEnabled: false, themeLight: false, specBoardEnabled: false };
    const { dispose } = renderActionBar();
    document.body.querySelector<HTMLButtonElement>('[data-ac-testid="actionBar.newOpen"]')!.click();
    expect(buttonAttrs("actionBar.newOpen")[1]).toBe("open");
    expect(buttonAttrs("actionBar.menu.newProject")[1]).toBe("ready");
    expect(buttonAttrs("actionBar.sortCoordinators")).toEqual([
      "toolbar-gear-btn coord-sort-activity-btn active",
      "recent",
    ]);
    expect(buttonAttrs("actionBar.sounds")).toEqual([
      "toolbar-gear-btn active",
      "muted",
    ]);
    expect(buttonAttrs("actionBar.categories")).toEqual([
      "toolbar-gear-btn show-categories-btn ",
      "hidden",
    ]);
    expect(buttonAttrs("actionBar.pinSelectedWorkgroup")).toEqual([
      "toolbar-gear-btn show-categories-btn ",
      "default",
    ]);
    expect(buttonAttrs("actionBar.theme")[1]).toBe("dark");
    dispose();
  });

  it("renders disabled states when settings and sessions are not ready", () => {
    mockState.sessions.hydrated = false;
    mockState.settings = null;
    const { dispose } = renderActionBar();
    expect(buttonAttrs("actionBar.sortCoordinators")[1]).toBe("disabled");
    expect(buttonAttrs("actionBar.sounds")[1]).toBe("disabled");
    expect(buttonAttrs("actionBar.theme")[1]).toBe("disabled");
    dispose();
  });
});

describe("ActionBar helpers (#2611)", () => {
  type Monitor = Parameters<typeof computeResourceBadgeState>[1];
  function snap(overrides: Record<string, unknown> = {}): Monitor {
    return {
      snapshot: {
        overallState: "ok",
        activeAgentGroups: 1,
        maxConcurrentAgentGroups: 4,
        ...overrides,
      },
      error: null,
    } as unknown as Monitor;
  }

  it.each([
    ["disabled", false, snap({ overallState: "critical" })],
    ["unknown", true, { snapshot: null, error: null } as unknown as Monitor],
    ["unknown", undefined, { ...snap(), error: "boom" } as unknown as Monitor],
    ["critical", true, snap({ overallState: "critical" })],
    ["enforcing", true, snap({ overallState: "enforcing" })],
    ["warn", true, snap({ overallState: "warn", activeAgentGroups: 9 })],
    ["limit", true, snap({ activeAgentGroups: 4 })],
    ["ok", true, snap({ activeAgentGroups: 9, maxConcurrentAgentGroups: 0 })],
    ["unknown", true, snap({ overallState: "unknown" })],
    // retired-field probe for step 7: an unknown value of the removed per-agent socket field must not gate the badge
    ["ok", true, snap({ ["network" + "State"]: "unknown" })],
    ["ok", undefined, snap()],
  ])("computeResourceBadgeState -> %s", (expected, enabled, monitor) => {
    expect(computeResourceBadgeState(enabled as boolean | undefined, monitor)).toBe(expected);
  });

  it("triState reads `on` only when not disabled", () => {
    const on = vi.fn(() => true);
    expect(triState(true, on, ["d", "on", "off"])).toBe("d");
    expect(on).not.toHaveBeenCalled();
    expect(triState(false, on, ["d", "on", "off"])).toBe("on");
    expect(triState(false, () => false, ["d", "on", "off"])).toBe("off");
    expect(on).toHaveBeenCalledTimes(1);
  });

  it("activeClass returns the old class suffix strings", () => {
    expect(activeClass(true)).toBe("active");
    expect(activeClass(false)).toBe("");
  });
});


describe("ActionBar pending mount cleanup (#4441)", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  function pendingRegistration() {
    let resolve!: (unlisten: () => void) => void;
    const pending = new Promise<() => void>((done) => { resolve = done; });
    vi.mocked(onOpenSettings).mockReturnValueOnce(pending);
    vi.mocked(resourceMonitorStore.startPolling).mockClear();
    return resolve;
  }

  it("releases late registration and skips polling after disposal", async () => {
    const resolve = pendingRegistration();
    const unlisten = vi.fn();
    const { dispose } = renderActionBar();
    dispose();
    resolve(unlisten);
    await Promise.resolve();

    expect(resourceMonitorStore.startPolling).not.toHaveBeenCalled();
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it("retains polling while mounted and releases both handles on disposal", async () => {
    const resolve = pendingRegistration();
    const unlisten = vi.fn();
    const { dispose } = renderActionBar();
    resolve(unlisten);
    await Promise.resolve();

    expect(resourceMonitorStore.startPolling).toHaveBeenCalledOnce();
    const stop = vi.mocked(resourceMonitorStore.startPolling).mock.results[0].value;
    expect(stop).not.toHaveBeenCalled();
    expect(unlisten).not.toHaveBeenCalled();
    dispose();
    expect(stop).toHaveBeenCalledOnce();
    expect(unlisten).toHaveBeenCalledOnce();
  });
});

describe("ActionBar theme toggle document class (#4499)", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    document.documentElement.classList.remove("light-theme");
    mockState.settings = { soundsEnabled: true, themeLight: true, specBoardEnabled: false };
    vi.clearAllMocks();
  });

  it("adds, removes and restores the light-theme class on the document", async () => {
    const { SettingsAPI } = await import("../../shared/ipc");
    mockState.settings = { soundsEnabled: true, themeLight: false, specBoardEnabled: false };
    document.documentElement.classList.remove("light-theme");
    const { dispose } = renderActionBar();
    const themeButton = document.body.querySelector<HTMLButtonElement>(
      '[data-ac-testid="actionBar.theme"]'
    );
    if (!themeButton) throw new Error("theme button not rendered");
    const classes = document.documentElement.classList;

    expect(classes.contains("light-theme")).toBe(false);
    themeButton.click();
    expect(classes.contains("light-theme")).toBe(true);
    await Promise.resolve();
    themeButton.click();
    expect(classes.contains("light-theme")).toBe(false);
    await Promise.resolve();

    vi.mocked(SettingsAPI.setThemeLight).mockRejectedValueOnce(new Error("boom"));
    themeButton.click();
    expect(classes.contains("light-theme")).toBe(true);
    await vi.waitFor(() => expect(classes.contains("light-theme")).toBe(false));

    dispose();
  });

  it("forces the light-theme class on instead of flipping a stale class", () => {
    mockState.settings = { soundsEnabled: true, themeLight: false, specBoardEnabled: false };
    document.documentElement.classList.add("light-theme");
    const { dispose } = renderActionBar();
    const themeButton = document.body.querySelector<HTMLButtonElement>(
      '[data-ac-testid="actionBar.theme"]'
    );
    if (!themeButton) throw new Error("theme button not rendered");

    themeButton.click();
    expect(document.documentElement.classList.contains("light-theme")).toBe(true);

    dispose();
  });
});

describe("ActionBar toggle button labels (#4693)", () => {
  afterEach(async () => {
    const { homeStore } = await import("../../main/stores/home");
    (homeStore as { visible: boolean }).visible = false;
    document.body.innerHTML = "";
    mockState.settings = { soundsEnabled: true, themeLight: true, specBoardEnabled: false };
    __resetCentralViewStoreForTests();
    vi.clearAllMocks();
  });

  function labels(testId: string): [string, string | null] {
    const el = document.body.querySelector<HTMLButtonElement>(`[data-ac-testid="${testId}"]`);
    if (!el) throw new Error(`missing ${testId}`);
    return [el.title, el.getAttribute("aria-label")];
  }

  it.each([
    [false, "Show Home"],
    [true, "Hide Home"],
  ])("home button with visible=%s -> %s", async (visible, expected) => {
    const { homeStore } = await import("../../main/stores/home");
    (homeStore as { visible: boolean }).visible = visible;
    const { dispose } = renderActionBar();
    expect(labels("actionBar.home")).toEqual([expected, expected]);
    dispose();
  });

  it.each([
    ["terminal", "Show web browser"],
    ["browser", "Hide web browser"],
  ] as const)("web browser button in %s view -> %s", (view, expected) => {
    centralViewStore.setInitialView(view);
    const { dispose } = renderActionBar();
    expect(labels("actionBar.webBrowser")).toEqual([expected, expected]);
    dispose();
  });

  it.each([
    [true, "Mute all app sounds"],
    [false, "Unmute app sounds"],
  ])("sounds button with soundsEnabled=%s -> %s", (soundsEnabled, expected) => {
    mockState.settings = { soundsEnabled, themeLight: true, specBoardEnabled: false };
    const { dispose } = renderActionBar();
    expect(labels("actionBar.sounds")).toEqual([expected, expected]);
    dispose();
  });
});
