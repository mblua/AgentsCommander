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
import {
  BOOKKEEPER_ID, FINAL, HIDDEN, startCtx, T_INSTALL, T_NAME_SKIP, T_PICK_BK, type RunCtx,
} from "../tutorials/create-first-team";
import { automationIdPart } from "../../sidebar/components/replica-repo-badges";
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
    // Different sources in separate microtasks, all inside one frame.
    target("x.one");
    await Promise.resolve();
    window.dispatchEvent(new Event("resize"));
    await Promise.resolve();
    document.body.dispatchEvent(new Event("scroll"));
    expect(frames.length).toBe(1);
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

  it("team wizard steps 1 -> 2 -> 3 move the bubble and the ring", async () => {
    const path = "/work/team";
    const names = ["personal-assistant", "accountant", "social-media-expert"];
    const p = {
      path, folderName: "team", loops: [], contextTemplateUpdates: [], teams: [], workgroups: [],
      agents: names.map((n) => ({ name: n, path: `${path}/.ac/_agent_${n}`, roleExists: true })),
    } as unknown as ProjectState;
    const ctx: RunCtx = { ...startCtx(0), step: "dialog", projectPath: path, goalKey: "team" };
    vi.spyOn(projectStore, "projects", "get").mockReturnValue([p]);
    vi.spyOn(tutorialStore, "ctx", "get").mockReturnValue(ctx);
    vi.spyOn(tutorialStore, "active", "get").mockReturnValue(true);
    vi.spyOn(tutorialStore, "sync").mockImplementation(() => {});
    const modal = target("newTeam.modal");
    modal.setAttribute("data-ac-state", "step-1");
    const name = target("newTeam.name", { top: 100, left: 20, width: 30, height: 40 });
    await mountLayer({ tutorialHintDismissed: true });
    const text = () => must("tutorial.bubble").textContent ?? "";
    expect(text()).toContain("Name the team my-team");
    expect(must("tutorial.ring").style.top).toBe("96px");
    name.remove();
    target("newTeam.agents", { top: 200, left: 20, width: 30, height: 40 });
    target("newTeam.create", { top: 300, left: 20, width: 30, height: 40 });
    modal.setAttribute("data-ac-state", "step-2");
    await flush();
    expect(text()).toContain("Check personal-assistant");
    expect(must("tutorial.ring").style.top).toBe("196px");
    modal.setAttribute("data-ac-state", "step-3"); // attribute-only change
    await flush();
    expect(text()).toContain("Click Create.");
    expect(must("tutorial.ring").style.top).toBe("296px");
  });

  it("changing only data-ac-value schedules one frame", async () => {
    await startRun();
    const el = target("newAgent.name");
    await flush();
    const spy = vi.spyOn(tutorialStore, "sync");
    el.setAttribute("data-ac-value", "a b");
    expect(await flush()).toBe(1);
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("skipTemplates with no run is a no-op", () => {
    tutorialStore.skipTemplates();
    expect(tutorialStore.ctx).toBeNull();
    expect(tutorialStore.active).toBe(false);
  });

  it("real probe decides the agent 2 text (skip, value, Bookkeeper row)", async () => {
    const path = "/work/agents";
    const p = {
      path, folderName: "agents", loops: [], contextTemplateUpdates: [], teams: [], workgroups: [],
      agents: [{ name: "personal-assistant", path: `${path}/.ac/_agent_personal-assistant`,
        roleExists: true }],
    } as unknown as ProjectState;
    let outcome = { seq: 0, path: null as string | null, created: false };
    vi.spyOn(projectStore, "projects", "get").mockReturnValue([p]);
    vi.spyOn(projectStore, "newProjectOutcome", "get").mockImplementation(() => outcome);
    target("actionBar.newOpen");
    target("actionBar.menu");
    target(`project.action.newAgent.${automationIdPart(path)}.projectMenu`);
    const modal = target("newAgent.modal");
    modal.setAttribute("data-ac-state", "ready");
    const name = target("newAgent.name");
    name.setAttribute("data-ac-value", "accountant");
    await mountLayer({ tutorialHintDismissed: true });
    const text = () => must("tutorial.bubble").textContent ?? "";
    const toDialog = async () => {
      tutorialStore.start();
      outcome = { seq: outcome.seq + 1, path, created: true };
      await flush();
      await flush();
      expect(tutorialStore.ctx).toMatchObject({ step: "dialog", goalKey: "agent:1" });
    };
    await toDialog();
    modal.setAttribute("data-ac-state", "missing");
    await flush();
    expect(text()).toContain(T_INSTALL);
    tutorialStore.skipTemplates();
    expect(tutorialStore.ctx?.skip).toBe(true);
    await flush();
    expect(text()).not.toContain(T_INSTALL);
    expect(text()).toContain("Click Create.");
    name.setAttribute("data-ac-value", "x");
    await flush();
    expect(text()).toContain(T_NAME_SKIP("accountant"));
    tutorialStore.exit();
    modal.setAttribute("data-ac-state", "ready");
    await toDialog();
    expect(tutorialStore.ctx?.skip).toBe(false);
    const row = target("newAgent.template");
    row.setAttribute("data-ac-value", BOOKKEEPER_ID);
    target("newAgent.templateSearch").setAttribute("data-ac-value", "bookkeeper");
    await flush();
    expect(text()).toContain(T_PICK_BK);
    row.remove();
    await flush();
    expect(text()).toContain(T_NAME_SKIP("accountant"));
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
