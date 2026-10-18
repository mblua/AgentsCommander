// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { readFileSync } from "node:fs";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  contextMenu,
  discovery,
  installBrowserDomStubs,
  installDeterministicAnimationFrames,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import type { AcDiscoveryResult, Session } from "../../shared/types";

// #3027: right-clicking a replica row marks that exact row with a halo while
// the replica context menu is open.

// A variable base keeps the real file: URL under jsdom (see AgentUpdateOverlay.test.tsx).
const moduleUrl = import.meta.url;
const sidebarCss = readFileSync(new URL("../styles/sidebar.css", moduleUrl), "utf8").replace(/\r\n/g, "\n");
const projectPath = "C:\\Project";
const wgPath = `${projectPath}\\.ac\\wg-1-dev-team`;
const coordPath = `${wgPath}\\__agent_dev-webpage-ui`;
const memberPath = `${wgPath}\\__agent_dev-rust`;
const grayPath = `${wgPath}\\__agent_dev-docs`;

const coordQuickRowTestId = "replica.row.quick.wg-1-dev-team.dev-webpage-ui";
const memberRowTestId = "replica.row.workgroups.wg-1-dev-team.dev-rust";
const grayRowTestId = "replica.row.workgroups.wg-1-dev-team.dev-docs";
const groupsTriggerTestId = "replica.wg-1-dev-team.groups.trigger";
const groupsFlyoutTestId = "replica.wg-1-dev-team.groups.flyout";

function projectDiscovery(): AcDiscoveryResult {
  const result = discovery({
    workgroups: [
      {
        name: "wg-1-dev-team",
        path: wgPath,
        task: null,
        taskTitle: "Clean",
        agents: [
          { name: "dev-webpage-ui", path: coordPath, identityPath: "../../_agent_dev-webpage-ui", repoPaths: [], isCoordinator: true },
          { name: "dev-rust", path: memberPath, identityPath: "../../_agent_dev-rust", repoPaths: [], isCoordinator: false },
          { name: "dev-docs", path: grayPath, identityPath: "../../_agent_dev-docs", repoPaths: [], isCoordinator: false },
        ],
      },
    ],
  });
  return {
    ...result,
    teams: [{ name: "team-alpha", coordinator: "dev-webpage-ui", agents: ["dev-webpage-ui", "dev-rust", "dev-docs"] }],
  };
}

function sessions(memberStatus: Session["status"] = "running"): Session[] {
  return [
    session({ id: "coord-session", name: "wg-1-dev-team/dev-webpage-ui", workingDirectory: coordPath, status: "running", isCoordinator: true }),
    session({ id: "member-session", name: "wg-1-dev-team/dev-rust", workingDirectory: memberPath, status: memberStatus }),
  ];
}

function domRect(left: number, top: number, width: number, height: number): DOMRect {
  return { left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({}) } as DOMRect;
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
// One real animation frame, then a macrotask: the halo coalesces measurement in a frame.
const frame = () => new Promise((resolve) => window.requestAnimationFrame(() => setTimeout(resolve, 0)));
const menu = () => document.querySelector<HTMLElement>(".session-context-menu");
const halo = () => document.querySelector<HTMLElement>(".replica-menu-halo");
const haloClip = () => document.querySelector<HTMLElement>(".replica-menu-halo-clip");
const byTestId = (id: string) => document.querySelector<HTMLElement>(`[data-ac-testid="${id}"]`);

// Per-row geometry: every replica row gets its own rect, so the halo's style
// says exactly which DOM row it measured.
const rowRects = new Map<HTMLElement, DOMRect>();
const elementRects = new Map<HTMLElement, DOMRect>();
function rowTop(el: HTMLElement): number {
  const rows = Array.from(document.querySelectorAll<HTMLElement>(".replica-item"));
  return 40 + rows.indexOf(el) * 30;
}

type FakeObserver = { callback: ResizeObserverCallback; observed: Element[]; disconnect: () => void };
let observers: FakeObserver[] = [];

describe("ProjectPanel replica context menu origin halo (#3027)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;
  let previousObserver: typeof ResizeObserver;

  beforeEach(() => {
    // This describe models row geometry itself, so the harness fixture stays off.
    cleanupDom = installBrowserDomStubs({ replicaMenuGeometry: false });
    resetUiStoresForTests();
    rowRects.clear();
    elementRects.clear();
    observers = [];
    previousObserver = globalThis.ResizeObserver;
    globalThis.ResizeObserver = class {
      private entry: FakeObserver;
      constructor(callback: ResizeObserverCallback) {
        this.entry = { callback, observed: [], disconnect: vi.fn() };
        observers.push(this.entry);
      }
      observe(el: Element) { this.entry.observed.push(el); }
      unobserve() {}
      disconnect() { this.entry.disconnect(); }
    } as unknown as typeof ResizeObserver;
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if (this.classList.contains("replica-item")) {
        return rowRects.get(this) ?? domRect(8, rowTop(this), 240, 28);
      }
      return elementRects.get(this) ?? domRect(0, 0, 1024, 768);
    });
  });

  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    vi.restoreAllMocks();
    globalThis.ResizeObserver = previousObserver;
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  async function setupPanel(list: Session[] = sessions()) {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", projectDiscovery());
    fake.resolve("get_settings", baseSettings());
    fake.onInvoke("task_get_snapshot_at", ({ workgroupRoot }) => ({
      workgroupRoot: String(workgroupRoot), task: null, taskTitle: "Clean", description: "",
      status: null, revision: "topic:0", statusRecord: null, tailIncomplete: false,
    }));
    sessionsStore.setSessions(list);
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    await projectStore.createAndLoad(projectPath);
    await waitFor(() => expect(rendered!.root.textContent).toContain("dev-docs"));
    return fake;
  }

  async function openOn(row: HTMLElement) {
    contextMenu(row);
    await waitFor(() => expect(menu()).not.toBeNull());
    await flush();
    await flush();
  }

  function expectHaloOn(row: HTMLElement) {
    const rect = row.getBoundingClientRect();
    const node = halo();
    expect(node).not.toBeNull();
    expect(haloClip()!.getAttribute("aria-hidden")).toBe("true");
    expect(node!.style.top).toBe(`${rect.top}px`);
    expect(node!.style.left).toBe(`${rect.left}px`);
    expect(node!.style.width).toBe(`${rect.width}px`);
    expect(node!.style.height).toBe(`${rect.height}px`);
  }

  function coordRows(): HTMLElement[] {
    return Array.from(document.querySelectorAll<HTMLElement>(".replica-item")).filter((row) =>
      (row.getAttribute("data-ac-testid") ?? "").endsWith("wg-1-dev-team.dev-webpage-ui")
    );
  }

  it("rings the row for active, exited and inactive openings", async () => {
    await setupPanel(sessions({ exited: 1 }));
    for (const id of [coordQuickRowTestId, memberRowTestId, grayRowTestId]) {
      await openOn(byTestId(id)!);
      expectHaloOn(byTestId(id)!);
    }
  });

  it("rings only the exact copy that was right-clicked", async () => {
    await setupPanel();
    const copies = coordRows();
    expect(copies.length).toBeGreaterThanOrEqual(2);
    const clicked = copies[copies.length - 1];
    expect(clicked).not.toBe(document.querySelector(".replica-item"));
    await openOn(clicked);
    expectHaloOn(clicked);
    expect(document.querySelectorAll(".replica-menu-halo")).toHaveLength(1);
  });

  it("transfers on A->B and restarts on A->A with a fresh node", async () => {
    await setupPanel();
    await openOn(byTestId(memberRowTestId)!);
    const first = halo();
    await openOn(byTestId(grayRowTestId)!);
    expectHaloOn(byTestId(grayRowTestId)!);
    expect(halo()).not.toBe(first);
    const second = halo();
    await openOn(byTestId(grayRowTestId)!);
    expectHaloOn(byTestId(grayRowTestId)!);
    expect(halo()).not.toBe(second);
    expect(document.querySelectorAll(".replica-menu-halo")).toHaveLength(1);
  });

  it("follows position-only changes without events and leaves stable styles untouched (#3069)", async () => {
    await setupPanel();
    const frames = installDeterministicAnimationFrames();
    const raf = vi.spyOn(window, "requestAnimationFrame");
    try {
      const row = byTestId(memberRowTestId)!;
      await openOn(row);
      await frames.flushFrame();
      const node = halo()!;
      const openedMenu = menu()!;
      const menuStyle = openedMenu.getAttribute("style");
      const selected = sessionsStore.activeId;
      const originObserver = observers.find((o) => o.observed.includes(row))!;
      const observerCount = observers.length;
      rowRects.set(row, domRect(18, 170, 240, 28));
      await frames.flushFrame();
      expectHaloOn(row);
      expect(halo()).toBe(node);
      expect(menu()).toBe(openedMenu);
      expect(openedMenu.getAttribute("style")).toBe(menuStyle);
      expect(sessionsStore.activeId).toBe(selected);
      expect(observers).toHaveLength(observerCount);
      expect(originObserver.disconnect).not.toHaveBeenCalled();

      const mutations: MutationRecord[] = [];
      const observer = new MutationObserver((records) => mutations.push(...records));
      observer.observe(haloClip()!, { attributes: true, subtree: true, attributeFilter: ["style"] });
      try {
        const calls = raf.mock.calls.length;
        window.dispatchEvent(new Event("scroll"));
        window.dispatchEvent(new Event("resize"));
        originObserver.callback([], {} as ResizeObserver);
        expect(raf).toHaveBeenCalledTimes(calls);
        await frames.flushFrame();
        await frames.flushFrame();
        expect(mutations).toHaveLength(0);
        expect(halo()).toBe(node);
      } finally {
        observer.disconnect();
      }
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
      await flush();
      const calls = raf.mock.calls.length;
      const measure = vi.spyOn(row, "getBoundingClientRect");
      const measurements = measure.mock.calls.length;
      await frames.flush();
      expect(measure).toHaveBeenCalledTimes(measurements);
      expect(raf).toHaveBeenCalledTimes(calls);
    } finally {
      rendered?.cleanup();
      rendered = null;
      raf.mockRestore();
      frames.restore();
    }
  });

  it("cancels tracking before unmount and ignores the captured callback", async () => {
    await setupPanel();
    const frames = installDeterministicAnimationFrames();
    const raf = vi.spyOn(window, "requestAnimationFrame");
    const caf = vi.spyOn(window, "cancelAnimationFrame");
    try {
      const row = byTestId(memberRowTestId)!;
      await openOn(row);
      const pending = raf.mock.results[raf.mock.results.length - 1].value as number;
      const captured = raf.mock.calls[raf.mock.calls.length - 1][0];
      rendered!.cleanup();
      rendered = null;
      expect(caf).toHaveBeenCalledWith(pending);
      const measure = vi.spyOn(row, "getBoundingClientRect");
      const measurements = measure.mock.calls.length;
      const calls = raf.mock.calls.length;
      captured(0);
      await frames.flush();
      expect(measure).toHaveBeenCalledTimes(measurements);
      expect(raf).toHaveBeenCalledTimes(calls);
      expect(halo()).toBeNull();
      expect(menu()).toBeNull();
    } finally {
      raf.mockRestore();
      caf.mockRestore();
      frames.restore();
    }
  });

  it("keeps the same node while tracking scroll, resize, observer and reclamp", async () => {
    await setupPanel();
    const row = byTestId(memberRowTestId)!;
    await openOn(row);
    const node = halo();
    expect(node).not.toBeNull();

    rowRects.set(row, domRect(8, 70, 240, 28));
    window.dispatchEvent(new Event("scroll"));
    await frame();
    expect(halo()).toBe(node);
    expect(node!.style.top).toBe("70px");

    rowRects.set(row, domRect(8, 90, 260, 28));
    window.dispatchEvent(new Event("resize"));
    await frame();
    expect(halo()).toBe(node);
    expect(node!.style.top).toBe("90px");
    expect(node!.style.width).toBe("260px");

    const observer = observers.find((o) => o.observed.includes(row));
    expect(observer).toBeDefined();
    rowRects.set(row, domRect(8, 100, 240, 40));
    observer!.callback([], {} as ResizeObserver);
    await frame();
    expect(halo()).toBe(node);
    expect(node!.style.height).toBe("40px");

    // Edit TASK title keeps the menu open and reclamps it (rewrites the menu object).
    const edit = Array.from(menu()!.querySelectorAll("button")).find((b) => b.textContent?.includes("Edit TASK title"));
    expect(edit).toBeDefined();
    click(edit!);
    await flush();
    await flush();
    expect(menu()).not.toBeNull();
    expect(halo()).toBe(node);
  });

  for (const [name, find] of [
    ["team header", () => document.querySelector<HTMLElement>(".ac-team-header")],
    ["Rooms header", () => Array.from(document.querySelectorAll<HTMLElement>(".ac-wg-header")).find((h) => h.textContent?.includes("Rooms")) ?? null],
  ] as const) {
    it(`drops the halo and its resources when the ${name} menu opens`, async () => {
      await setupPanel();
      const row = byTestId(memberRowTestId)!;
      const add = vi.spyOn(window, "addEventListener");
      const remove = vi.spyOn(window, "removeEventListener");
      const raf = vi.spyOn(window, "requestAnimationFrame").mockImplementation(() => 4242);
      const caf = vi.spyOn(window, "cancelAnimationFrame");
      await openOn(row);
      expect(halo()).not.toBeNull();
      const scrollListener = add.mock.calls.find((c) => c[0] === "scroll")![1];
      const resizeListener = add.mock.calls.find((c) => c[0] === "resize")![1];
      const observer = observers.find((o) => o.observed.includes(row))!;
      window.dispatchEvent(new Event("scroll")); // leaves a frame pending
      expect(raf).toHaveBeenCalled();

      const header = find();
      expect(header).not.toBeNull();
      header!.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 }));
      await flush();

      expect(menu()?.textContent ?? "").not.toContain("Edit TASK title");
      expect(halo()).toBeNull();
      expect(haloClip()).toBeNull();
      expect(remove.mock.calls.some((c) => c[0] === "scroll" && c[1] === scrollListener)).toBe(true);
      expect(remove.mock.calls.some((c) => c[0] === "resize" && c[1] === resizeListener)).toBe(true);
      expect(caf).toHaveBeenCalledWith(4242);
      expect(observer.disconnect).toHaveBeenCalled();
    });
  }

  it("does not change the selection", async () => {
    await setupPanel();

    const before = sessionsStore.activeId;
    await openOn(byTestId(memberRowTestId)!);
    expect(halo()).not.toBeNull();
    expect(sessionsStore.activeId).toBe(before);
  });

  it("removes the halo on Escape, outside click and menu action", async () => {
    await setupPanel();
    await openOn(byTestId(memberRowTestId)!);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await flush();
    expect(menu()).toBeNull();
    expect(halo()).toBeNull();

    await openOn(byTestId(memberRowTestId)!);
    click(document.body);
    await flush();
    expect(menu()).toBeNull();
    expect(halo()).toBeNull();

    await openOn(byTestId(grayRowTestId)!);
    const action = Array.from(menu()!.querySelectorAll("button")).find((b) => b.textContent?.includes("Coding Agent"));
    click(action!);
    await flush();
    expect(menu()).toBeNull();
    expect(halo()).toBeNull();
  });

  it("keeps the halo while a flyout is open", async () => {
    await setupPanel();
    await openOn(byTestId(coordQuickRowTestId)!);
    const node = halo();
    byTestId(groupsTriggerTestId)!.dispatchEvent(new MouseEvent("mouseenter", { bubbles: false }));
    await waitFor(() => expect(byTestId(groupsFlyoutTestId)).not.toBeNull());
    expect(menu()).not.toBeNull();
    expect(halo()).toBe(node);
  });

  it("closes the menu when the origin row disconnects or leaves the visible zone", async () => {
    await setupPanel();
    const row = byTestId(memberRowTestId)!;
    await openOn(row);
    row.remove();
    window.dispatchEvent(new Event("scroll"));
    await frame();
    expect(menu()).toBeNull();
    expect(halo()).toBeNull();
  });

  it("closes when the row scrolls fully out of view", async () => {
    await setupPanel();
    const row = byTestId(grayRowTestId)!;
    await openOn(row);
    rowRects.set(row, domRect(8, window.innerHeight + 50, 240, 28));
    window.dispatchEvent(new Event("scroll"));
    await frame();
    expect(menu()).toBeNull();
    expect(halo()).toBeNull();
  });

  it("leaves no listeners, observers or frames behind after closing", async () => {
    await setupPanel();
    const add = vi.spyOn(window, "addEventListener");
    const remove = vi.spyOn(window, "removeEventListener");
    const raf = vi.spyOn(window, "requestAnimationFrame");
    const caf = vi.spyOn(window, "cancelAnimationFrame");
    const row = byTestId(memberRowTestId)!;
    await openOn(row);
    window.dispatchEvent(new Event("scroll"));
    const frames = raf.mock.results.map((r) => r.value);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await flush();
    for (const type of ["scroll", "resize"]) {
      const listener = add.mock.calls.find((c) => c[0] === type)![1];
      expect(remove.mock.calls.some((c) => c[0] === type && c[1] === listener)).toBe(true);
    }
    expect(observers.find((o) => o.observed.includes(row))!.disconnect).toHaveBeenCalled();
    expect(caf.mock.calls.map((c) => c[0])).toEqual(expect.arrayContaining(frames.slice(-1)));
  });

  it("closes when the row is hidden down to zero size", async () => {
    await setupPanel();
    const row = byTestId(memberRowTestId)!;
    await openOn(row);
    expect(halo()).not.toBeNull();
    rowRects.set(row, domRect(0, 0, 0, 0));
    observers.find((o) => o.observed.includes(row))!.callback([], {} as ResizeObserver);
    await frame();
    expect(menu()).toBeNull();
    expect(halo()).toBeNull();
  });

  it("clips the halo to nested scroll ancestors and closes when fully clipped", async () => {
    await setupPanel();
    const row = byTestId(memberRowTestId)!;
    const inner = row.parentElement!;
    const outer = inner.parentElement!;
    inner.style.overflow = "auto";
    outer.style.overflow = "hidden";
    elementRects.set(inner, domRect(0, 100, 1024, 100));
    elementRects.set(outer, domRect(4, 0, 200, 768));
    rowRects.set(row, domRect(8, 80, 240, 28));
    await openOn(row);
    const node = halo();
    expect(node).not.toBeNull();
    // Clip = viewport, inner and outer intersected; the row keeps its full size relative to it.
    expect(haloClip()!.style.left).toBe("4px");
    expect(haloClip()!.style.top).toBe("100px");
    expect(haloClip()!.style.width).toBe("200px");
    expect(haloClip()!.style.height).toBe("100px");
    expect(node!.style.left).toBe("4px");
    expect(node!.style.top).toBe("-20px");
    expect(node!.style.width).toBe("240px");
    expect(node!.style.height).toBe("28px");

    rowRects.set(row, domRect(8, 40, 240, 28));
    window.dispatchEvent(new Event("scroll"));
    await frame();
    expect(menu()).toBeNull();
    expect(halo()).toBeNull();
  });

  it("releases everything when the panel unmounts with the menu open", async () => {
    await setupPanel();
    const add = vi.spyOn(window, "addEventListener");
    const remove = vi.spyOn(window, "removeEventListener");
    const row = byTestId(memberRowTestId)!;
    await openOn(row);
    expect(halo()).not.toBeNull();
    rendered!.cleanup();
    rendered = null;
    expect(halo()).toBeNull();
    expect(haloClip()).toBeNull();
    for (const type of ["scroll", "resize"]) {
      const listener = add.mock.calls.find((c) => c[0] === type)![1];
      expect(remove.mock.calls.some((c) => c[0] === type && c[1] === listener)).toBe(true);
    }
    expect(observers.find((o) => o.observed.includes(row))!.disconnect).toHaveBeenCalled();
  });

  it("cancels a pending frame on a quick close and a stale callback does nothing", async () => {
    await setupPanel();
    const frames = installDeterministicAnimationFrames();
    const raf = vi.spyOn(window, "requestAnimationFrame");
    const caf = vi.spyOn(window, "cancelAnimationFrame");
    try {
      const row = byTestId(memberRowTestId)!;
      await openOn(row);
      const node = halo();
      expect(node).not.toBeNull();
      window.dispatchEvent(new Event("scroll"));
      const pending = raf.mock.results[raf.mock.results.length - 1].value as number;
      const captured = raf.mock.calls[raf.mock.calls.length - 1][0];
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
      await flush();
      expect(caf).toHaveBeenCalledWith(pending);
      expect(halo()).toBeNull();

      // Even if the old measurement ran anyway, it must not revive the halo or close a newer menu.
      await openOn(byTestId(grayRowTestId)!);
      const fresh = halo();
      captured(0);
      await frames.flushFrame();
      expect(menu()).not.toBeNull();
      expect(halo()).toBe(fresh);
      expectHaloOn(byTestId(grayRowTestId)!);
    } finally {
      raf.mockRestore();
      caf.mockRestore();
      frames.restore();
    }
  });

  // B-R4-1: Telegram actions advance the action epoch while the same menu stays
  // open; the halo must keep following its own row, and only its own opening.
  describe("while a Telegram attach keeps the menu open", () => {
    type Bots = { telegramBots: Array<{ id: string; label: string; color: string }> };
    const botOne = { id: "bot-one", label: "Bot One", color: "red" };
    const botTwo = { id: "bot-two", label: "Bot Two", color: "blue" };
    const gates: Array<ReturnType<typeof deferred<Bots>>> = [];

    function deferred<T>() {
      let resolve!: (value: T) => void;
      let reject!: (reason?: unknown) => void;
      const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
      return { promise, resolve, reject };
    }

    const exactButton = (label: string) =>
      Array.from(menu()?.querySelectorAll<HTMLButtonElement>("button") ?? []).find(
        (b) => (b.textContent ?? "").trim().replace(/^[^A-Za-z0-9]+/, "").trim() === label,
      ) ?? null;

    const telegramCalls = (fake: FakeTransport) =>
      fake.callsFor("telegram_attach").length + fake.callsFor("telegram_detach").length;

    afterEach(() => {
      while (gates.length) gates.pop()!.resolve({ telegramBots: [] });
    });

    // Deferred get_settings registered after setup, so initialization keeps its own response.
    async function setupPending() {
      const fake = await setupPanel();
      fake.onInvoke("get_settings", () => {
        const gate = deferred<Bots>();
        gates.push(gate);
        return gate.promise;
      });
      fake.resolve("telegram_attach", null);
      fake.resolve("telegram_detach", null);
      return fake;
    }

    async function clickAttach(fake: FakeTransport, calls: number) {
      click(exactButton("Attach Telegram")!);
      await waitFor(() => expect(fake.callsFor("get_settings")).toHaveLength(calls));
    }

    async function showChooser(fake: FakeTransport) {
      await clickAttach(fake, 1);
      gates[0].resolve({ telegramBots: [botOne, botTwo] });
      await waitFor(() => expect(exactButton(botTwo.label)).not.toBeNull());
      expect(exactButton(botOne.label)).not.toBeNull();
      await flush();
    }

    const observerOf = (row: HTMLElement) => observers.find((o) => o.observed.includes(row))!;

    it("keeps tracking the same node while settings are pending", async () => {
      const fake = await setupPending();
      const row = byTestId(memberRowTestId)!;
      await openOn(row);
      const node = halo();
      const observer = observerOf(row);
      await clickAttach(fake, 1);

      rowRects.set(row, domRect(8, 75, 240, 28));
      window.dispatchEvent(new Event("scroll"));
      await frame();
      expect(halo()).toBe(node);
      expectHaloOn(row);

      rowRects.set(row, domRect(12, 95, 250, 32));
      window.dispatchEvent(new Event("resize"));
      await frame();
      expect(halo()).toBe(node);
      expectHaloOn(row);

      rowRects.set(row, domRect(16, 110, 230, 36));
      observer.callback([], {} as ResizeObserver);
      await frame();
      expect(halo()).toBe(node);
      expectHaloOn(row);
      expect(document.querySelectorAll(".replica-menu-halo")).toHaveLength(1);
      expect(document.querySelectorAll(".session-context-menu")).toHaveLength(1);
      expect(telegramCalls(fake)).toBe(0);
    });

    it("keeps tracking through the chooser and drops everything before attaching", async () => {
      const fake = await setupPending();
      const row = byTestId(memberRowTestId)!;
      await openOn(row);
      const node = halo();
      const observer = observerOf(row);
      await showChooser(fake);
      expect(halo()).toBe(node);

      rowRects.set(row, domRect(8, 130, 240, 28));
      observer.callback([], {} as ResizeObserver);
      await frame();
      expect(halo()).toBe(node);
      expectHaloOn(row);
      rowRects.set(row, domRect(8, 150, 240, 28));
      window.dispatchEvent(new Event("scroll"));
      await frame();
      expect(halo()).toBe(node);
      expectHaloOn(row);

      const raf = vi.spyOn(window, "requestAnimationFrame").mockImplementation(() => 5151);
      const caf = vi.spyOn(window, "cancelAnimationFrame");
      window.dispatchEvent(new Event("scroll")); // coalesces with the tracking frame
      await waitFor(() => expect(raf).toHaveBeenCalled());
      let menuAtAttach: HTMLElement | null | undefined;
      let haloAtAttach: HTMLElement | null | undefined;
      fake.onInvoke("telegram_attach", () => {
        menuAtAttach = menu();
        haloAtAttach = halo();
        return null;
      });
      click(exactButton(botTwo.label)!);
      await waitFor(() => expect(fake.callsFor("telegram_attach")).toHaveLength(1));
      expect(fake.callsFor("telegram_attach")[0].args).toEqual({ sessionId: "member-session", botId: botTwo.id });
      expect(menuAtAttach).toBeNull();
      expect(haloAtAttach).toBeNull();
      expect(observer.disconnect).toHaveBeenCalled();
      expect(raf).toHaveBeenCalled();
      expect(caf).toHaveBeenCalledWith(5151);
    });

    const losses = [
      ["disconnects", (row: HTMLElement) => { row.remove(); window.dispatchEvent(new Event("scroll")); }],
      ["shrinks to zero", (row: HTMLElement) => { rowRects.set(row, domRect(0, 0, 0, 0)); observerOf(row).callback([], {} as ResizeObserver); }],
      ["leaves the view", (row: HTMLElement) => { rowRects.set(row, domRect(8, window.innerHeight + 50, 240, 28)); window.dispatchEvent(new Event("scroll")); }],
    ] as const;
    for (const phase of ["pending", "chooser"] as const) {
      for (const [name, lose] of losses) {
        it(`closes menu and halo when the origin ${name} (${phase})`, async () => {
          const fake = await setupPending();
          const row = byTestId(memberRowTestId)!;
          const remove = vi.spyOn(window, "removeEventListener");
          await openOn(row);
          const observer = observerOf(row);
          if (phase === "pending") await clickAttach(fake, 1);
          else await showChooser(fake);

          lose(row);
          await frame();
          expect(menu()).toBeNull();
          expect(halo()).toBeNull();
          expect(haloClip()).toBeNull();
          expect(observer.disconnect).toHaveBeenCalled();
          expect(remove.mock.calls.some((c) => c[0] === "scroll")).toBe(true);
          expect(remove.mock.calls.some((c) => c[0] === "resize")).toBe(true);

          if (phase === "pending") {
            gates[0].resolve({ telegramBots: [botOne, botTwo] });
            await flush();
            await flush();
          }
          expect(menu()).toBeNull();
          expect(halo()).toBeNull();
          expect(exactButton(botTwo.label)).toBeNull();
          expect(telegramCalls(fake)).toBe(0);
        });
      }
    }

    it("consumes a stale rejection after a second attach and keeps tracking", async () => {
      const fake = await setupPending();
      const row = byTestId(memberRowTestId)!;
      await openOn(row);
      const node = halo();
      await clickAttach(fake, 1);
      await clickAttach(fake, 2);
      gates[0].reject(new Error("stale settings"));
      await flush();
      await flush();

      rowRects.set(row, domRect(8, 120, 240, 28));
      window.dispatchEvent(new Event("scroll"));
      await frame();
      expect(halo()).toBe(node);
      expectHaloOn(row);

      gates[1].resolve({ telegramBots: [botOne, botTwo] });
      await waitFor(() => expect(exactButton(botTwo.label)).not.toBeNull());
      expect(exactButton(botOne.label)).not.toBeNull();
      expect(menu()!.querySelectorAll(".settings-color-dot")).toHaveLength(2);
      expect(halo()).toBe(node);
      expect(telegramCalls(fake)).toBe(0);
    });

    for (const [name, next] of [
      ["A->A", () => byTestId(memberRowTestId)!],
      ["A->B", () => byTestId(grayRowTestId)!],
      ["same session, other copy", () => coordRows()[coordRows().length - 1]],
    ] as const) {
      it(`ignores the old frame and settings after reopening ${name}`, async () => {
        const fake = await setupPending();
        const frames = installDeterministicAnimationFrames();
        const raf = vi.spyOn(window, "requestAnimationFrame");
        try {
          const first = name === "same session, other copy" ? byTestId(coordQuickRowTestId)! : byTestId(memberRowTestId)!;
          await openOn(first);
          window.dispatchEvent(new Event("scroll"));
          const captured = vi.fn(raf.mock.calls[raf.mock.calls.length - 1][0]);
          await clickAttach(fake, 1);

          window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
          await flush();
          expect(menu()).toBeNull();
          const target = next();
          if (name !== "A->A") expect(target).not.toBe(first);
          await openOn(target);
          await frames.flushFrame();
          const fresh = halo();
          expect(fresh).not.toBeNull();
          const style = haloClip()!.getAttribute("style") + "|" + fresh!.getAttribute("style");

          // The old opening's row now reports a lost origin: if its callback acted, it would close the new menu.
          if (target !== first) rowRects.set(first, domRect(0, 0, 0, 0));
          const staleMeasure = vi.spyOn(first, "getBoundingClientRect");
          const measurements = staleMeasure.mock.calls.length;
          const requests = raf.mock.calls.length;
          captured(0);
          expect(staleMeasure).toHaveBeenCalledTimes(measurements);
          expect(raf).toHaveBeenCalledTimes(requests);
          expect(captured).toHaveBeenCalledTimes(1);
          gates[0].resolve({ telegramBots: [botOne, botTwo] });
          await flush();
          await flush();
          await frames.flushFrame();

          expect(menu()).not.toBeNull();
          expect(halo()).toBe(fresh);
          expect(haloClip()!.getAttribute("style") + "|" + fresh!.getAttribute("style")).toBe(style);
          expectHaloOn(target);
          expect(exactButton(botTwo.label)).toBeNull();
          expect(telegramCalls(fake)).toBe(0);
        } finally {
          raf.mockRestore();
          frames.restore();
        }
      });
    }
  });

  it("is click-through, layered under the menu, and fixed under reduced motion", () => {
    const clip = /\.replica-menu-halo-clip\s*\{([^}]*)\}/.exec(sidebarCss)![1];
    expect(clip).toMatch(/pointer-events:\s*none/);
    expect(clip).toMatch(/z-index:\s*9998/);
    expect(/\.replica-menu-halo\s*\{([^}]*)\}/.exec(sidebarCss)![1]).toMatch(/animation:\s*replica-menu-halo-pulse 420ms[^;]*\b2;/);
    expect(sidebarCss).toMatch(/prefers-reduced-motion: reduce\)\s*\{\s*\.replica-menu-halo \{ animation: none; \}/);
  });
});

// The harness fixture that gives jsdom replica rows visible bounds (#3027 B1).
// Independent of the describe above: no ProjectPanel, no per-row spy.
describe("installBrowserDomStubs replica menu geometry fixture (#3027)", () => {
  const cleanups: Array<() => void> = [];
  const install = (options?: { replicaMenuGeometry?: boolean }) => {
    const cleanup = installBrowserDomStubs(options);
    cleanups.push(cleanup);
    return cleanup;
  };

  afterEach(() => {
    vi.restoreAllMocks();
    while (cleanups.length) cleanups.pop()!();
    document.body.replaceChildren();
  });

  function tree() {
    const list = document.createElement("div");
    const row = document.createElement("div");
    row.className = "replica-item";
    list.appendChild(row);
    const other = document.createElement("div");
    const menuEl = document.createElement("div");
    menuEl.className = "session-context-menu";
    document.body.append(list, other, menuEl);
    return { list, row, other, menuEl };
  }

  const size = (r: DOMRect) => [r.left, r.top, r.width, r.height];

  it("gives connected rows and their ancestors bounds and leaves other elements alone", () => {
    install();
    const { list, row, other, menuEl } = tree();
    expect(size(row.getBoundingClientRect())).toEqual([8, 40, 240, 28]);
    expect(size(list.getBoundingClientRect())).toEqual([0, 0, window.innerWidth, window.innerHeight]);
    expect(size(document.body.getBoundingClientRect())).toEqual([0, 0, window.innerWidth, window.innerHeight]);
    expect(size(other.getBoundingClientRect())).toEqual([0, 0, 0, 0]);
    expect(size(menuEl.getBoundingClientRect())).toEqual([0, 0, 0, 0]);
  });

  it("lets a later Element.prototype spy and explicit rects win", () => {
    install();
    const { row, menuEl } = tree();
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
      return this === menuEl ? domRect(100, 100, 180, 200) : domRect(0, 0, 0, 0);
    });
    expect(size(menuEl.getBoundingClientRect())).toEqual([100, 100, 180, 200]);
    vi.restoreAllMocks();
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue(domRect(3, 4, 50, 0));
    expect(size(row.getBoundingClientRect())).toEqual([3, 4, 50, 0]);
    vi.restoreAllMocks();
    // An instance override, even an intentional 0x0, takes precedence.
    Object.defineProperty(row, "getBoundingClientRect", { configurable: true, value: () => domRect(0, 0, 0, 0) });
    expect(size(row.getBoundingClientRect())).toEqual([0, 0, 0, 0]);
  });

  it("does not fake disconnected or hidden rows", () => {
    install();
    const { list, row } = tree();
    list.style.visibility = "hidden";
    expect(size(row.getBoundingClientRect())).toEqual([0, 0, 0, 0]);
    list.style.visibility = "";
    list.style.display = "none";
    expect(size(row.getBoundingClientRect())).toEqual([0, 0, 0, 0]);
    expect(size(list.getBoundingClientRect())).toEqual([0, 0, 0, 0]);
    list.style.display = "";
    row.style.visibility = "collapse";
    expect(size(row.getBoundingClientRect())).toEqual([0, 0, 0, 0]);
    row.style.visibility = "";
    list.remove();
    expect(size(row.getBoundingClientRect())).toEqual([0, 0, 0, 0]);
  });

  it("keeps rows at 0x0 when opted out", () => {
    install({ replicaMenuGeometry: false });
    const { row } = tree();
    expect(Object.prototype.hasOwnProperty.call(HTMLElement.prototype, "getBoundingClientRect")).toBe(false);
    expect(size(row.getBoundingClientRect())).toEqual([0, 0, 0, 0]);
  });

  it("restores the previous descriptor or its absence, without leaking across installs", () => {
    const has = () => Object.prototype.hasOwnProperty.call(HTMLElement.prototype, "getBoundingClientRect");
    expect(has()).toBe(false);
    const first = install();
    expect(has()).toBe(true);
    first();
    cleanups.pop();
    expect(has()).toBe(false);
    install();
    cleanups.pop()!();
    expect(has()).toBe(false);

    // A pre-existing own method is called with the right `this` and restored exactly.
    const seen: Element[] = [];
    const own = function (this: HTMLElement) {
      seen.push(this);
      return domRect(0, 0, 0, 0);
    };
    Object.defineProperty(HTMLElement.prototype, "getBoundingClientRect", { configurable: true, writable: true, value: own });
    try {
      const before = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "getBoundingClientRect");
      const cleanup = install();
      const { row } = tree();
      expect(size(row.getBoundingClientRect())).toEqual([8, 40, 240, 28]);
      expect(seen).toContain(row);
      cleanup();
      cleanups.pop();
      expect(Object.getOwnPropertyDescriptor(HTMLElement.prototype, "getBoundingClientRect")).toEqual(before);
    } finally {
      delete (HTMLElement.prototype as Partial<HTMLElement>).getBoundingClientRect;
    }
  });
});
