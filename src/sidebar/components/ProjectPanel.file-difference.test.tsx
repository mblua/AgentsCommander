// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { click, discovery, installBrowserDomStubs, renderWithFakeTransport, resetUiStoresForTests, session } from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";

const path = "C:\Project";
const command = "get_project_file_difference";
const unknownLabel = "File difference from origin/main unavailable";
const label = (n: number) => `${n} files differ from origin/main, including untracked files`;
const settle = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
let cleanupDom: (() => void) | undefined;
let rendered: ReturnType<typeof renderWithFakeTransport> | undefined;

async function mount(fake: FakeTransport, paths = [path], data = discovery()) {
  fake.onInvoke("new_project", ({ path }) => ({ path, registered: true, created: false }));
  fake.resolve("discover_project", data);
  fake.resolve("list_unresolved_loop_targets", []);
  rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
  for (const projectPath of paths) await projectStore.createAndLoad(projectPath);
  await settle();
  return rendered.root;
}
const pill = (root: Element) => root.querySelector<HTMLElement>(".project-file-difference");

beforeEach(() => {
  cleanupDom = installBrowserDomStubs();
  resetUiStoresForTests();
  vi.useFakeTimers();
});
afterEach(() => {
  rendered?.cleanup();
  rendered = undefined;
  vi.useRealTimers();
  resetUiStoresForTests();
  cleanupDom?.();
  document.body.replaceChildren();
});

describe("Project root file difference", () => {
  it("positive control: renders the actual 7 response outside the collapse button before the filter", async () => {
    const fake = new FakeTransport();
    fake.onInvoke(command, (args) => {
      expect(args).toEqual({ path });
      return { count: 7 };
    });
    const root = await mount(fake);
    const badge = pill(root)!;
    expect(badge.tagName).toBe("SPAN");
    expect(badge.textContent).toBe("7");
    expect(badge.title).toBe(label(7));
    expect(badge.getAttribute("aria-label")).toBe(label(7));
    expect(badge.getAttribute("data-ac-testid")).toMatch(/^project\.fileDifference\./);
    expect(badge.hasAttribute("aria-live")).toBe(false);
    expect(badge.closest("button")).toBeNull();
    const button = root.querySelector<HTMLButtonElement>(".project-header-main")!;
    expect(button.textContent).toContain("Project: " + projectStore.projects[0].folderName);
    expect(button.textContent).not.toContain("7");
    expect(badge.nextElementSibling).toBe(root.querySelector(".project-filter-row"));
    click(button);
    expect(button.getAttribute("aria-expanded")).toBe("false");
    expect(pill(root)?.textContent).toBe("7");
    await vi.advanceTimersByTimeAsync(15000);
    expect(fake.callsFor(command)).toHaveLength(2);
    click(button);
    expect(button.getAttribute("aria-expanded")).toBe("true");
    const filter = root.querySelector<HTMLButtonElement>(".project-filter-toggle")!;
    click(filter);
    expect(filter.getAttribute("aria-expanded")).toBe("true");
    click(filter);
    expect(filter.getAttribute("aria-expanded")).toBe("false");
  });

  it("hides only verified zero and keeps independent exact unabridged counts for two projects", async () => {
    const fake = new FakeTransport();
    fake.onInvoke(command, ({ path: requested }) => ({ count: requested === path ? 0 : 123456789 }));
    const root = await mount(fake, [path, "C:\Other"]);
    const headers = root.querySelectorAll(".project-header");
    expect(pill(headers[0])).toBeNull();
    expect(headers[0].classList.contains("project-header--has-indicator")).toBe(false);
    expect(pill(headers[1])?.textContent).toBe("123456789");
    expect(pill(headers[1])?.title).toBe(label(123456789));
    expect(fake.callsFor(command).map(call => call.args.path)).toEqual([path, "C:\Other"]);
  });

  it.each([null, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, "7", undefined, NaN, Infinity])("treats invalid/null count %s as unknown and recovers", async (count) => {
    const fake = new FakeTransport();
    fake.resolve(command, { count });
    const root = await mount(fake);
    expect(pill(root)?.textContent).toBe("—");
    expect(pill(root)?.title).toBe(unknownLabel);
    expect(pill(root)?.getAttribute("aria-label")).toBe(unknownLabel);
    fake.resolve(command, { count: 19 });
    await vi.advanceTimersByTimeAsync(15000);
    expect(pill(root)?.textContent).toBe("19");
  });

  it("clears a previous count on rejection and recovers without an unhandled rejection", async () => {
    const fake = new FakeTransport();
    fake.resolve(command, { count: 7 });
    const root = await mount(fake);
    fake.reject(command, "disconnected");
    await vi.advanceTimersByTimeAsync(15000);
    expect(pill(root)?.textContent).toBe("—");
    expect(pill(root)?.title).toBe(unknownLabel);
    fake.resolve(command, { count: 3 });
    await vi.advanceTimersByTimeAsync(15000);
    expect(pill(root)?.textContent).toBe("3");
  });

  it("waits 15 seconds after settlement without overlap and discards a late disposed response", async () => {
    const fake = new FakeTransport();
    let resolve!: (value: { count: number }) => void;
    fake.onInvoke(command, () => new Promise<{ count: number }>(r => { resolve = r; }));
    const root = await mount(fake);
    const badge = pill(root)!;
    expect(badge.textContent).toBe("—");
    await vi.advanceTimersByTimeAsync(60000);
    expect(fake.callsFor(command)).toHaveLength(1);
    resolve({ count: 7 });
    await settle();
    expect(badge.textContent).toBe("7");
    await vi.advanceTimersByTimeAsync(14999);
    expect(fake.callsFor(command)).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(fake.callsFor(command)).toHaveLength(2);
    rendered!.cleanup();
    rendered = undefined;
    resolve({ count: 99 });
    await settle();
    expect(badge.textContent).toBe("7");
    await vi.advanceTimersByTimeAsync(60000);
    expect(fake.callsFor(command)).toHaveLength(2);
  });

  it("reserves 32px exactly once for pill/blocked/both/neither combinations", async () => {
    const fake = new FakeTransport();
    fake.resolve(command, { count: 7 });
    const roomPath = path + "\.ac\room-test";
    const replicaPath = roomPath + "\worker";
    sessionsStore.setSessions([session({ name: "room-test/worker", workingDirectory: replicaPath, communication: { kind: "blockedMenu", visible: true, updatedAt: "2026-10-19T00:00:00Z", message: "Waiting" } })]);
    const root = await mount(fake, [path], discovery({ workgroups: [{ name: "room-test", path: roomPath, task: null, taskTitle: null, agents: [{ name: "worker", path: replicaPath, repoPaths: [], isCoordinator: false }] }] }));
    const header = root.querySelector(".project-header")!;
    const blocked = header.querySelector('[data-ac-testid="project.header.blockedMenu"]')!;
    expect(blocked).not.toBeNull();
    expect(header.classList.contains("project-header--has-indicator")).toBe(true);
    expect(header.querySelectorAll(".project-header-indicator--last")).toHaveLength(1);
    expect(pill(header)?.classList.contains("project-header-indicator--last")).toBe(true);
    expect(blocked.nextElementSibling).toBe(pill(header));
    fake.resolve(command, { count: 0 });
    await vi.advanceTimersByTimeAsync(15000);
    expect(pill(header)).toBeNull();
    expect(blocked.classList.contains("project-header-indicator--last")).toBe(true);
    expect(header.querySelectorAll(".project-header-indicator--last")).toHaveLength(1);
    sessionsStore.setSessions([]);
    expect(header.querySelectorAll(".project-header-indicator--last")).toHaveLength(0);
    expect(header.classList.contains("project-header--has-indicator")).toBe(false);
    fake.resolve(command, { count: 7 });
    await vi.advanceTimersByTimeAsync(15000);
    expect(header.classList.contains("project-header--has-indicator")).toBe(true);
    expect(header.querySelectorAll(".project-header-indicator--last")).toHaveLength(1);
  });

  it("cancels a scheduled poll when the project is removed", async () => {
    const fake = new FakeTransport();
    fake.resolve(command, { count: 7 });
    fake.resolve("remove_project", undefined);
    const root = await mount(fake);
    await projectStore.removeProject(path);
    expect(pill(root)).toBeNull();
    await vi.advanceTimersByTimeAsync(60000);
    expect(fake.callsFor(command)).toHaveLength(1);
  });
});

describe("file difference CSS contract (static, no geometry/color proof)", () => {
  const moduleUrl = import.meta.url;
  const css = readFileSync(new URL("../styles/sidebar.css", moduleUrl), "utf8");
  const rule = (selector: string) => {
    const start = css.indexOf(`${selector} {`);
    expect(start).toBeGreaterThanOrEqual(0);
    return css.slice(start, css.indexOf("}", start) + 1);
  };
  it("preserves overlay anchor, height, field width and transitions", () => {
    const overlay = rule(".project-filter-row");
    for (const declaration of ["position: absolute", "top: 50%", "right: var(--spacing-md)", "left: calc(var(--spacing-md) + 20px)", "height: 26px", "transform: translateY(-50%)"]) expect(overlay).toContain(declaration);
    expect(rule(".project-filter-field.open")).toContain("width: min(230px, calc(100% - 32px))");
    expect(rule(".project-filter-field")).toContain("transition: width 180ms ease-out, opacity 150ms ease-out");
  });
  it("transfers a single reserve to the last indicator and preserves shrinking title", () => {
    expect(rule(".project-header-main")).toContain("padding: 0 32px 0 0");
    expect(rule(".project-header-main")).toContain("flex: 1 1 auto");
    expect(rule(".project-header-main")).toContain("min-width: 0");
    expect(rule(".project-header--has-indicator .project-header-main")).toContain("padding-right: 0");
    expect(rule(".project-header .project-header-indicator--last")).toContain("margin-right: 32px");
    for (const declaration of ["min-width: 0", "overflow: hidden", "text-overflow: ellipsis", "white-space: nowrap"]) expect(rule(".project-title")).toContain(declaration);
    const badge = rule(".project-file-difference");
    for (const declaration of ["flex-shrink: 0", "font-size: 10px", "border: 1px solid var(--sidebar-border)", "border-radius: 999px"]) expect(badge).toContain(declaration);
    expect(badge).not.toMatch(/transition|animation/);
  });
});
