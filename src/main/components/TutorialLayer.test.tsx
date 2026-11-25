// @vitest-environment jsdom
// #5228 - hint, overlay and the DOM probe of the tutorial layer.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const platform = vi.hoisted(() => ({ tauri: true }));
vi.mock("../../shared/platform", () => ({
  get isTauri() { return platform.tauri; },
  get isBrowser() { return !platform.tauri; },
  isWindows: false,
}));

import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings, click, discovery, installBrowserDomStubs, renderWithFakeTransport,
  resetUiStoresForTests, waitFor,
} from "../../shared/testing/ui-harness";
import { __disposeSettingsSubscriptionsForTests } from "../../shared/stores/settings";
import type { ProjectState } from "../../sidebar/stores/project-merge";
import { projectStore } from "../../sidebar/stores/project";
import { __resetHomeStoreForTests, homeStore } from "../stores/home";
import { __resetTutorialStoreForTests, tutorialStore } from "../stores/tutorial";
import {
  __resetTutorialProgressForTests, tutorialProgressStore,
} from "../stores/tutorial-progress";
import { FINAL, HIDDEN, startCtx, type RunCtx } from "../tutorials/create-first-team";
import TutorialLayer from "./TutorialLayer";

const sel = (id: string) => `[data-ac-testid="${id}"]`;
const q = (id: string) => document.body.querySelector<HTMLElement>(sel(id));
const must = (id: string) => {
  const el = q(id);
  if (!el) throw new Error(`Missing ${id}`);
  return el;
};
const cleanups: Array<() => void> = [];
let frames: FrameRequestCallback[] = [];

async function flush(): Promise<number> {
  await Promise.resolve(); // MutationObserver callbacks are microtasks
  const run = frames;
  frames = [];
  run.forEach((cb) => cb(0));
  await Promise.resolve();
  return run.length;
}
function target(id: string, rect = { top: 10, left: 20, width: 30, height: 40 }) {
  const el = document.createElement("button");
  el.setAttribute("data-ac-testid", id);
  el.getBoundingClientRect = () =>
    ({ ...rect, bottom: rect.top + rect.height, right: rect.left + rect.width }) as DOMRect;
  document.body.appendChild(el);
  return el;
}
function fake(flags: object = {}): FakeTransport {
  const f = new FakeTransport();
  f.resolve("get_settings", { ...baseSettings(), ...flags });
  f.resolve("update_settings", undefined);
  return f;
}
async function mountLayer(flags: object = {}) {
  const r = renderWithFakeTransport(() => <TutorialLayer />, fake(flags));
  cleanups.push(r.cleanup);
  await waitFor(() => expect(tutorialProgressStore.loaded).toBe(true));
  await flush();
  return r;
}

beforeEach(() => {
  platform.tauri = true;
  cleanups.push(installBrowserDomStubs());
  frames = [];
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
    frames.push(cb);
    return frames.length;
  });
  vi.stubGlobal("cancelAnimationFrame", () => { frames = []; });
  resetUiStoresForTests();
  __disposeSettingsSubscriptionsForTests();
  __resetHomeStoreForTests();
  __resetTutorialStoreForTests();
  __resetTutorialProgressForTests();
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  while (cleanups.length) cleanups.pop()?.();
  resetUiStoresForTests();
  __disposeSettingsSubscriptionsForTests();
  __resetTutorialStoreForTests();
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("TutorialHint", () => {
  it("shows when all six conditions hold", async () => {
    target("actionBar.home");
    await mountLayer();
    expect(must("tutorial.hint").textContent).toContain("Check the tutorials");
  });
  it("hidden until the flags are loaded", () => {
    target("actionBar.home");
    let resolve!: (v: unknown) => void;
    const f = fake();
    f.onInvoke("get_settings", () => new Promise((r) => { resolve = r; }));
    cleanups.push(renderWithFakeTransport(() => <TutorialLayer />, f).cleanup);
    expect(q("tutorial.hint")).toBeNull();
    resolve(baseSettings());
  });
  it("hidden when dismissed", async () => {
    target("actionBar.home");
    await mountLayer({ tutorialHintDismissed: true });
    expect(q("tutorial.hint")).toBeNull();
  });
  it("hidden when completed", async () => {
    target("actionBar.home");
    await mountLayer({ teamTutorialCompleted: true });
    expect(q("tutorial.hint")).toBeNull();
  });
  it("hidden while Home is open", async () => {
    target("actionBar.home");
    homeStore.show();
    await mountLayer();
    expect(q("tutorial.hint")).toBeNull();
  });
  it("hidden while a run is active", async () => {
    target("actionBar.home");
    tutorialStore.start();
    await mountLayer();
    expect(q("tutorial.hint")).toBeNull();
  });
  it("hidden without the Home button", async () => {
    await mountLayer();
    expect(q("tutorial.hint")).toBeNull();
  });
  it("× calls dismissHint", async () => {
    target("actionBar.home");
    await mountLayer();
    const spy = vi.spyOn(tutorialProgressStore, "dismissHint");
    click(must("tutorial.hint.close"));
    expect(spy).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(q("tutorial.hint")).toBeNull());
  });
});

describe("TutorialOverlay", () => {
  async function startRun() {
    target("actionBar.newOpen");
    await mountLayer({ tutorialHintDismissed: true });
    tutorialStore.start();
    await flush();
  }

  it("ring sits on actionBar.newOpen at openMenu", async () => {
    await startRun();
    const ring = must("tutorial.ring");
    expect(ring.style.top).toBe("6px");
    expect(ring.style.left).toBe("16px");
    expect(ring.style.width).toBe("38px");
    expect(ring.style.height).toBe("48px");
  });

  it("adding actionBar.menu moves the bubble to Click New Project. after one frame", async () => {
    await startRun();
    target("actionBar.menu");
    expect(await flush()).toBe(1);
    expect(must("tutorial.bubble").textContent).toContain("Click New Project.");
  });

  it("three DOM changes in one frame call sync once", async () => {
    await startRun();
    const spy = vi.spyOn(tutorialStore, "sync");
    target("x.one");
    target("x.two");
    must("actionBar.newOpen").setAttribute("data-ac-state", "open");
    await flush();
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("no run and hint dismissed: a DOM change schedules nothing", async () => {
    await mountLayer({ tutorialHintDismissed: true });
    const spy = vi.spyOn(tutorialStore, "sync");
    target("actionBar.menu");
    expect(await flush()).toBe(0);
    expect(spy).not.toHaveBeenCalled();
  });

  it("a target missing for 2 s shows the hidden text, and returns", async () => {
    await startRun();
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const el = must("actionBar.newOpen");
    el.remove();
    await flush();
    vi.advanceTimersByTime(1999);
    expect(must("tutorial.bubble").textContent).not.toContain(HIDDEN);
    vi.advanceTimersByTime(1);
    expect(must("tutorial.bubble").textContent).toContain(HIDDEN);
    document.body.appendChild(el);
    await flush();
    expect(must("tutorial.bubble").textContent).not.toContain(HIDDEN);
  });

  it("Exit tutorial ends the run", async () => {
    await startRun();
    click(must("tutorial.exit"));
    expect(tutorialStore.active).toBe(false);
    expect(q("tutorial.bubble")).toBeNull();
  });

  it("at done, Done calls markCompleted", async () => {
    const path = "/work/done";
    const names = ["personal-assistant", "accountant", "social-media-expert"];
    const p = {
      path, folderName: "done", loops: [], contextTemplateUpdates: [],
      agents: names.map((n) => ({ name: n, path: `${path}/.ac/_agent_${n}`, roleExists: true })),
      teams: [{ name: "my-team", agents: names.map((n) => `done/${n}`),
        coordinator: "done/personal-assistant" }],
      workgroups: [{ name: "room-1", path: `${path}/.ac/room-1`, task: null, teamName: "my-team",
        agents: names.map((n) => ({ name: n, path: `/r/${n}`, repoPaths: [],
          isCoordinator: n === "personal-assistant" })) }],
    } as unknown as ProjectState;
    const ctx: RunCtx = { ...startCtx(0), step: "done", projectPath: path };
    vi.spyOn(projectStore, "projects", "get").mockReturnValue([p]);
    vi.spyOn(tutorialStore, "ctx", "get").mockReturnValue(ctx);
    vi.spyOn(tutorialStore, "active", "get").mockReturnValue(true);
    vi.spyOn(tutorialStore, "sync").mockImplementation(() => {});
    const spy = vi.spyOn(tutorialProgressStore, "markCompleted");
    await mountLayer({ tutorialHintDismissed: true });
    expect(must("tutorial.bubble").textContent).toContain(FINAL);
    expect(q("tutorial.exit")).toBeNull();
    click(must("tutorial.done"));
    expect(spy).toHaveBeenCalledTimes(1);
  });
});

describe("real ActionBar: folder holds through the pick", () => {
  it("stays on folder while the pick is pending, then moves to header", async () => {
    const PATH = "/work/tutorial-project";
    let release!: (v: string) => void;
    const gate = new Promise<string>((r) => { release = r; });
    const f = fake({ tutorialHintDismissed: true });
    f.resolve("get_resource_snapshot", null);
    f.onInvoke("pick_folder", () => gate);
    f.resolve("check_project_path", false);
    f.onInvoke("new_project", (args) => ({ path: args.path }));
    f.resolve("discover_project", discovery());
    const { default: ActionBar } = await import("../../sidebar/components/ActionBar");
    const r = renderWithFakeTransport(() => <><ActionBar /><TutorialLayer /></>, f);
    cleanups.push(r.cleanup);
    await waitFor(() => expect(q("actionBar.settings")).toBeTruthy());
    await waitFor(() => expect(tutorialProgressStore.loaded).toBe(true));
    tutorialStore.start();
    await flush();
    click(must("actionBar.newOpen"));
    await flush();
    await waitFor(() => expect(q("actionBar.menu.newProject")).toBeTruthy());
    await flush();
    expect(tutorialStore.ctx?.step).toBe("newProject");
    click(must("actionBar.menu.newProject"));
    await flush();
    await flush();
    expect(tutorialStore.ctx?.step).toBe("folder");
    await flush();
    expect(tutorialStore.ctx?.step).toBe("folder");
    release(PATH);
    await waitFor(async () => {
      await flush();
      expect(tutorialStore.ctx?.step).toBe("header");
    });
    expect(tutorialStore.ctx?.projectPath).toBe(PATH);
  });
});
