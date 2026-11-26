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
import { bubblePos, dialogPos, placeBubble, union, type Box } from "./TutorialOverlay";

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
type R = { left: number; top: number; width: number; height: number };
/** Like target(), inside `parent`, with extra attributes. */
function box(id: string | null, rect: R, parent: HTMLElement = document.body,
  attrs: Record<string, string> = {}) {
  const el = document.createElement("div");
  if (id) el.setAttribute("data-ac-testid", id);
  for (const [k, val] of Object.entries(attrs)) el.setAttribute(k, val);
  el.getBoundingClientRect = () =>
    ({ ...rect, bottom: rect.top + rect.height, right: rect.left + rect.width }) as DOMRect;
  parent.appendChild(el);
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
    const list = target("newAgent.template.list");
    const row = document.createElement("button");
    row.setAttribute("data-ac-value", BOOKKEEPER_ID);
    list.appendChild(row);
    target("newAgent.templateSearch").setAttribute("data-ac-value", "bookkeeper");
    await flush();
    expect(text()).toContain(T_PICK_BK);
    row.remove();
    await flush();
    expect(text()).toContain(T_NAME_SKIP("accountant"));
  });

  it("real probe: a wrong typed first name warns, the right one does not", async () => {
    const path = "/work/first";
    const p = {
      path, folderName: "first", loops: [], contextTemplateUpdates: [], teams: [], workgroups: [],
      agents: [],
    } as unknown as ProjectState;
    let outcome = { seq: 0, path: null as string | null, created: false };
    vi.spyOn(projectStore, "projects", "get").mockReturnValue([p]);
    vi.spyOn(projectStore, "newProjectOutcome", "get").mockImplementation(() => outcome);
    target("actionBar.newOpen");
    target("actionBar.menu");
    target(`project.action.newAgent.${automationIdPart(path)}.projectMenu`);
    target("newAgent.modal");
    const name = target("newAgent.name");
    name.setAttribute("data-ac-state", "typed");
    name.setAttribute("data-ac-value", "personal-assitant");
    await mountLayer({ tutorialHintDismissed: true });
    tutorialStore.start();
    outcome = { seq: 1, path, created: true };
    await flush();
    await flush();
    expect(tutorialStore.ctx).toMatchObject({ step: "dialog", goalKey: "agent:0" });
    const bubble = () => must("tutorial.bubble");
    expect(bubble().textContent).toContain("The name must be exactly personal-assistant.");
    expect(bubble().classList.contains("tutorial-bubble--warn")).toBe(true);
    name.setAttribute("data-ac-value", "personal-assistant");
    await flush();
    expect(bubble().textContent).not.toContain("The name must be exactly");
    expect(bubble().classList.contains("tutorial-bubble--warn")).toBe(false);
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

describe("P09: skip download, menu ring, bubble placement", () => {
  const agentsProject = (path: string) => ({
    path, folderName: "agents", loops: [], contextTemplateUpdates: [], teams: [], workgroups: [],
    agents: [{ name: "personal-assistant", path: `${path}/.ac/_agent_personal-assistant`,
      roleExists: true }],
  } as unknown as ProjectState);
  function atMenuItem(path: string) {
    vi.spyOn(projectStore, "projects", "get").mockReturnValue([agentsProject(path)]);
    const ctx: RunCtx = { ...startCtx(0), step: "menuItem", projectPath: path };
    vi.spyOn(tutorialStore, "ctx", "get").mockReturnValue(ctx);
    vi.spyOn(tutorialStore, "active", "get").mockReturnValue(true);
    vi.spyOn(tutorialStore, "sync").mockImplementation(() => {});
  }
  const menuId = (path: string) => `project.action.newAgent.${automationIdPart(path)}.projectMenu`;

  it("Skip download on the install view sets skip and moves on", async () => {
    const path = "/work/skip";
    let outcome = { seq: 0, path: null as string | null, created: false };
    vi.spyOn(projectStore, "projects", "get").mockReturnValue([agentsProject(path)]);
    vi.spyOn(projectStore, "newProjectOutcome", "get").mockImplementation(() => outcome);
    target("actionBar.newOpen");
    target("actionBar.menu");
    target(menuId(path));
    target("newAgent.modal").setAttribute("data-ac-state", "missing");
    target("newAgent.installTemplates");
    target("newAgent.name").setAttribute("data-ac-value", "x");
    await mountLayer({ tutorialHintDismissed: true });
    tutorialStore.start();
    outcome = { seq: 1, path, created: true };
    await flush();
    await flush();
    expect(tutorialStore.ctx).toMatchObject({ step: "dialog", goalKey: "agent:1" });
    expect(must("tutorial.bubble").textContent).toContain(T_INSTALL);
    click(must("tutorial.skip"));
    expect(tutorialStore.ctx?.skip).toBe(true);
    await flush();
    expect(must("tutorial.bubble").querySelector("p")?.textContent)
      .toBe("Name it accountant (no spaces), then click Create. No template is fine.");
    expect(q("tutorial.skip")).toBeNull();
  });

  it("control: no Skip download at openMenu", async () => {
    target("actionBar.newOpen");
    await mountLayer({ tutorialHintDismissed: true });
    tutorialStore.start();
    await flush();
    expect(tutorialStore.ctx?.step).toBe("openMenu");
    expect(q("tutorial.skip")).toBeNull();
    expect(q("tutorial.exit")).not.toBeNull();
  });

  it("menu step ring sits on the menu item, not the header", async () => {
    const path = "/work/menu";
    atMenuItem(path);
    target(`project.header.${automationIdPart(path)}`, { top: 300, left: 5, width: 50, height: 20 });
    target(menuId(path), { top: 100, left: 40, width: 180, height: 24 });
    await mountLayer({ tutorialHintDismissed: true });
    const ring = must("tutorial.ring");
    expect([ring.style.top, ring.style.left, ring.style.width, ring.style.height])
      .toEqual(["96px", "36px", "188px", "32px"]);
  });

  it("bubbleStyle uses bubblePos: bubble near the bottom goes above the ring", async () => {
    const path = "/work/bottom";
    vi.stubGlobal("innerHeight", 600);
    vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(80);
    atMenuItem(path);
    target(menuId(path), { top: 560, left: 40, width: 180, height: 24 });
    await mountLayer({ tutorialHintDismissed: true });
    await flush();
    const ring = must("tutorial.ring");
    const rTop = parseFloat(ring.style.top);
    const rBottom = rTop + parseFloat(ring.style.height);
    const top = parseFloat(must("tutorial.bubble").style.top);
    expect(top + 80 <= rTop || top >= rBottom).toBe(true);
    expect(top + 80).toBeLessThanOrEqual(rTop);
  });
});

describe("bubblePos", () => {
  const H = 80;
  const box = (left: number, top: number, w: number, h: number): Box =>
    ({ left, top, right: left + w, bottom: top + h });
  const overlaps = (r: Box, p: { left: number; top: number }) =>
    p.top < r.bottom && p.top + H > r.top && p.left < r.right && p.left + 260 > r.left;
  const cases: Array<[string, Box, number, number, { left: number; top: number }]> = [
    ["below", box(40, 100, 180, 24), 1000, 600, { left: 40, top: 132 }],
    ["above", box(40, 560, 180, 24), 1000, 600, { left: 40, top: 472 }],
    ["right", box(40, 20, 180, 150), 1000, 180, { left: 228, top: 20 }],
    ["left", box(500, 20, 180, 150), 800, 180, { left: 232, top: 20 }],
  ];
  it.each(cases)("%s of the target, no overlap", (_n, r, vw, vh, want) => {
    const p = bubblePos(r, H, vw, vh);
    expect(p).toEqual(want);
    expect(overlaps(r, p)).toBe(false);
  });
  it("near bottom: no vertical overlap; control: the v5 formula overlaps", () => {
    const r = box(40, 560, 180, 24);
    const p = bubblePos(r, H, 1000, 600);
    expect(p.top + H <= r.top || p.top >= r.bottom).toBe(true);
    const v5 = Math.min(r.bottom + 8, 600 - H - 8);
    expect(v5 < r.bottom && v5 + H > r.top).toBe(true);
  });
  it("window too small: rule 5", () => {
    const r = box(40, 20, 180, 150);
    expect(bubblePos(r, H, 300, 180)).toEqual({ left: 32, top: 92 });
  });
});

describe("P11: bubble placement around menus and dialogs", () => {
  const NAMES = ["personal-assistant", "accountant", "social-media-expert"];
  const DIALOG = { "data-ac-role": "dialog" };
  const rect = (left: number, top: number, width: number, height: number) =>
    ({ left, top, width, height });
  const proj = (path: string, n: number, team = false) => ({
    path, folderName: "agents", loops: [], contextTemplateUpdates: [], workgroups: [],
    agents: NAMES.slice(0, n).map((a) => ({ name: a, path: `${path}/.ac/_agent_${a}`,
      roleExists: true })),
    teams: team ? [{ name: "my-team", agents: NAMES.map((a) => `agents/${a}`),
      coordinator: "agents/personal-assistant" }] : [],
  } as unknown as ProjectState);
  function mockRun(p: ProjectState | null, ctx: Partial<RunCtx>) {
    vi.spyOn(projectStore, "projects", "get").mockReturnValue(p ? [p] : []);
    const c: RunCtx = { ...startCtx(0), projectPath: p?.path ?? null, ...ctx };
    vi.spyOn(tutorialStore, "ctx", "get").mockReturnValue(c);
    vi.spyOn(tutorialStore, "active", "get").mockReturnValue(true);
    vi.spyOn(tutorialStore, "sync").mockImplementation(() => {});
  }
  const pos = () => {
    const b = must("tutorial.bubble");
    return { left: b.style.left, top: b.style.top };
  };
  beforeEach(() => {
    vi.stubGlobal("innerWidth", 1000);
    vi.stubGlobal("innerHeight", 900);
    vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(80);
  });

  const KIND_CASES: Array<[string, number, boolean, number]> = [
    ["newAgent", 1, false, 100], ["newTeam", 3, false, 124], ["newRoom", 3, true, 148],
  ];
  it.each(KIND_CASES)("M1 %s: bubble below the whole project menu", async (kind, n, t, top) => {
    const path = `/work/m1-${kind}`;
    mockRun(proj(path, n, t), { step: "menuItem" });
    const menu = box(null, rect(40, 100, 180, 200), document.body,
      { class: "session-context-menu" });
    box(`project.action.${kind}.${automationIdPart(path)}.projectMenu`,
      rect(40, top, 180, 24), menu);
    await mountLayer({ tutorialHintDismissed: true });
    await flush();
    expect(pos()).toEqual({ left: "40px", top: "308px" });
    expect(must("tutorial.ring").style.top).toBe(`${top - 4}px`);
  });

  it("M2 New / Open menu: bubble below the menu", async () => {
    mockRun(null, { step: "newProject" });
    const menu = box("actionBar.menu", rect(40, 60, 180, 60), document.body,
      { "data-ac-role": "menu" });
    box("actionBar.menu.newProject", rect(40, 60, 180, 24), menu);
    await mountLayer({ tutorialHintDismissed: true });
    await flush();
    expect(must("tutorial.bubble").textContent).toContain("Click New Project.");
    expect(pos().top).toBe("128px");
  });

  /** Real store into the agent 2 dialog, search empty, Bookkeeper row present. */
  async function toSearchStep(modalRect: R, w: number) {
    const path = "/work/d";
    let outcome = { seq: 0, path: null as string | null, created: false };
    vi.spyOn(projectStore, "projects", "get").mockReturnValue([proj(path, 1)]);
    vi.spyOn(projectStore, "newProjectOutcome", "get").mockImplementation(() => outcome);
    target("actionBar.newOpen");
    target("actionBar.menu");
    target(`project.action.newAgent.${automationIdPart(path)}.projectMenu`);
    const modal = box("newAgent.modal", modalRect, document.body,
      { ...DIALOG, "data-ac-state": "ready" });
    box("newAgent.templateSearch", rect(120, 260, w, 30), modal);
    const list = box("newAgent.template.list", rect(120, 296, w, 168), modal);
    box(null, rect(120, 296, w, 24), list, { "data-ac-value": BOOKKEEPER_ID });
    await mountLayer({ tutorialHintDismissed: true });
    tutorialStore.start();
    outcome = { seq: 1, path, created: true };
    await flush();
    await flush();
    await flush();
    expect(tutorialStore.ctx).toMatchObject({ step: "dialog", goalKey: "agent:1" });
    expect(must("tutorial.bubble").textContent).toContain("Type bookkeeper in the search box");
  }

  it("D1 New Agent search step: bubble above the dialog", async () => {
    await toSearchStep(rect(100, 200, 800, 600), 760);
    expect(pos()).toEqual({ left: "120px", top: "112px" });
    expect(112 + 80).toBeLessThanOrEqual(200);
  });
  it("D2 no room above: bubble right of the dialog", async () => {
    await toSearchStep(rect(100, 40, 500, 800), 460);
    expect(pos()).toEqual({ left: "608px", top: "260px" });
  });
  it("D3 no room above or at the sides: bubble below the template list", async () => {
    await toSearchStep(rect(20, 40, 960, 800), 760);
    expect(pos()).toEqual({ left: "120px", top: "472px" });
  });

  function roomDialog(modalRect: R) {
    mockRun(proj("/work/room", 3, true), { step: "dialog", goalKey: "room" });
    const modal = box("newRoom.modal", modalRect, document.body, DIALOG);
    box("newRoom.teamSearch", rect(120, 360, 760, 30), modal);
    return modal;
  }
  it("D4 New Room: bubble above the dialog", async () => {
    roomDialog(rect(100, 300, 800, 400));
    await mountLayer({ tutorialHintDismissed: true });
    await flush();
    expect(must("tutorial.bubble").textContent).toContain("Choose my-team");
    expect(pos()).toEqual({ left: "120px", top: "212px" });
  });
  it("D4 New Room, list open: bubble below the team list", async () => {
    const modal = roomDialog(rect(20, 40, 960, 800));
    box("newRoom.team.list", rect(120, 394, 760, 180), modal);
    await mountLayer({ tutorialHintDismissed: true });
    await flush();
    expect(pos().top).toBe("582px");
  });
  it("D4 New Room, list hidden: rule A step 5 above the field", async () => {
    vi.stubGlobal("innerHeight", 470);
    const modal = roomDialog(rect(20, 40, 960, 420));
    box("newRoom.team.list", rect(0, 0, 0, 0), modal);
    await mountLayer({ tutorialHintDismissed: true });
    await flush();
    expect(pos()).toEqual({ left: "120px", top: "272px" });
  });

  it("D5 New Team wizard step 1: bubble above the dialog", async () => {
    mockRun(proj("/work/team5", 3), { step: "dialog", goalKey: "team" });
    const modal = box("newTeam.modal", rect(100, 300, 800, 400), document.body,
      { ...DIALOG, "data-ac-state": "step-1" });
    box("newTeam.name", rect(120, 360, 760, 30), modal);
    await mountLayer({ tutorialHintDismissed: true });
    await flush();
    expect(must("tutorial.bubble").textContent).toContain("Name the team my-team");
    expect(pos().top).toBe("212px");
  });
});

describe("dialogPos", () => {
  const H = 80;
  const b = (left: number, top: number, w: number, h: number): Box =>
    ({ left, top, right: left + w, bottom: top + h });
  const t = b(120, 260, 200, 30);
  const g = b(120, 260, 200, 200);
  const cases: Array<[string, Box, number, number, { left: number; top: number }]> = [
    ["above", b(100, 200, 400, 400), 1000, 900, { left: 120, top: 112 }],
    ["right", b(100, 40, 400, 800), 1000, 900, { left: 508, top: 260 }],
    ["left", b(400, 40, 590, 800), 1000, 900, { left: 132, top: 260 }],
    ["below g", b(20, 40, 960, 800), 1000, 900, { left: 120, top: 468 }],
    ["fallback", b(20, 40, 960, 500), 1000, 550, { left: 120, top: 172 }],
  ];
  it.each(cases)("rule A: %s", (_n, d, vw, vh, want) => {
    expect(dialogPos(t, d, g, H, vw, vh)).toEqual(want);
  });
  it("placeBubble: no area is bubblePos(t); a menu area is bubblePos(union)", () => {
    expect(placeBubble(t, t, null, H, 1000, 900)).toEqual(bubblePos(t, H, 1000, 900));
    const menu = b(100, 200, 300, 300);
    expect(placeBubble(t, t, { dialog: false, box: menu }, H, 1000, 900))
      .toEqual(bubblePos(union(t, menu), H, 1000, 900));
    expect(placeBubble(t, t, { dialog: false, box: menu }, H, 1000, 900))
      .toEqual({ left: 100, top: 508 });
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
