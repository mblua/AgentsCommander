// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createSignal } from "solid-js";
import NewWorkgroupModal from "./NewWorkgroupModal";
import type { AcTeam } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { click, discovery, input, renderWithFakeTransport, resetUiStoresForTests, waitFor } from "../../shared/testing/ui-harness";

import { executeAutomationRequest, resetAutomationBridgeForTests } from "../../shared/automation-bridge";
import type { UiAutomationAction } from "../../shared/types";

// Geometry enables bridge dispatch in jsdom; it does not establish Windows
// visibility, hit-testing, native select behavior or IME coverage.
async function automate(action: UiAutomationAction, selector: string, value?: string) {
  expect(document.querySelectorAll('[data-ac-testid="' + selector + '"]')).toHaveLength(1);
  const response = await executeAutomationRequest("main", {
    requestId: action + selector, token: "test", window: "main", action, selector, value,
    expiresAtUnixMs: Date.now() + 5000,
  });
  if (!response.ok) throw new Error(response.error + ": " + response.message);
  return response.target;
}
function stubAutomationGeometry() {
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
    return { x: 100, y: 50, left: 100, top: 50, right: 200, bottom: 70,
      width: this.isConnected ? 100 : 0, height: this.isConnected ? 20 : 0, toJSON: () => ({}) } as DOMRect;
  });
  vi.spyOn(Element.prototype, "getClientRects").mockImplementation(function (this: Element) {
    const list = (this.isConnected ? [this.getBoundingClientRect()] : []) as unknown as DOMRectList;
    Object.defineProperty(list, "item", { value: (index: number) => list[index] ?? null });
    return list;
  });
}

const projectPath = "C:\Project";
const teams = (names: string[]): AcTeam[] => names.map(name => ({ name, agents: [], coordinator: "" }));
let rendered: ReturnType<typeof renderWithFakeTransport> | undefined;
const search = () => document.querySelector<HTMLInputElement>("#new-room-team-search")!;
const select = () => document.querySelector<HTMLSelectElement>("#new-room-team")!;
const title = () => document.querySelector<HTMLInputElement>('input[placeholder="Task title (optional)"]')!;
const create = () => document.querySelector<HTMLButtonElement>(".new-agent-create-btn")!;
const options = () => Array.from(select().options).map(option => option.value);
function choose(name: string) {
  select().value = name;
  select().dispatchEvent(new Event("change", { bubbles: true }));
}
function key(element: Element, key: string, init: KeyboardEventInit = {}) {
  element.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init }));
}
function mount(names = ["Alpha", "Beta"]) {
  const fake = new FakeTransport();
  fake.resolve("create_workgroup", undefined);
  fake.resolve("discover_project", discovery({ teams: teams(names) }));
  const onClose = vi.fn();
  const [currentTeams, setTeams] = createSignal(teams(names));
  rendered = renderWithFakeTransport(() =>
    <NewWorkgroupModal projectPath={projectPath} teams={currentTeams()} onClose={onClose} />, fake);
  return { fake, onClose, setTeams };
}
beforeEach(() => { resetUiStoresForTests(); resetAutomationBridgeForTests(); });
afterEach(() => {
  rendered?.cleanup();
  rendered = undefined;
  resetUiStoresForTests();
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

describe("New Room team search and optional title (#2788)", () => {
  it("exposes the unique R1 targets and selects/invalidates teams through the real bridge", async () => {
    stubAutomationGeometry();
    const { setTeams } = mount(["dev-alpha", "dev-beta", "ops"]);
    expect((await automate("query", "newRoom.modal")).role).toBe("dialog");
    expect((await automate("query", "newRoom.teamSearch")).metadata.detail).toBe("Type to filter teams...");
    expect((await automate("query", "newRoom.taskTitle")).metadata.detail).toBe("Task title (optional)");
    expect((await automate("query", "newRoom.taskTitle.hint")).role).toBe("text");
    expect((await automate("query", "newRoom.taskTitle.hint")).text).toBe("Leave empty to start with Clean.");
    expect((await automate("query", "newRoom.cancel")).text).toBe("Cancel");
    const projection = async () => {
      const detail = (await automate("query", "newRoom.team")).metadata.detail;
      expect(detail.length).toBeLessThanOrEqual(120);
      return JSON.parse(detail);
    };
    expect(await projection()).toEqual({ options: ["dev-alpha", "dev-beta", "ops"], selected: "", size: 6 });
    expect((await automate("query", "newRoom.create")).disabled).toBe(true);
    await automate("setValue", "newRoom.teamSearch", "  DEV ");
    expect(await projection()).toEqual({ options: ["dev-alpha", "dev-beta"], selected: "", size: 6 });
    await automate("setValue", "newRoom.team", "dev-alpha");
    expect(await projection()).toEqual({ options: ["dev-alpha", "dev-beta"], selected: "dev-alpha", size: 6 });
    expect((await automate("query", "newRoom.create")).disabled).toBe(false);
    await automate("setValue", "newRoom.taskTitle", "Draft");
    expect(title().value).toBe("Draft");
    await automate("setValue", "newRoom.teamSearch", "dev");
    expect((await projection()).selected).toBe("");
    expect((await automate("query", "newRoom.create")).disabled).toBe(true);
    await automate("setValue", "newRoom.teamSearch", "missing");
    expect((await automate("query", "newRoom.team.empty")).text).toBe("No teams match your search.");
    expect(document.querySelector('[data-ac-testid="newRoom.team.empty"]')?.getAttribute("role")).toBe("status");
    expect(await projection()).toEqual({ options: [], selected: "", size: 6 });
    expect((await automate("query", "newRoom.create")).disabled).toBe(true);
    await automate("setValue", "newRoom.teamSearch", "");
    expect(document.querySelector('[data-ac-testid="newRoom.team.empty"]')).toBeNull();
    expect(await projection()).toEqual({ options: ["dev-alpha", "dev-beta", "ops"], selected: "", size: 6 });
    setTeams([]);
    expect((await automate("query", "newRoom.team.empty")).text).toBe("No teams available.");
  });
  it("filters names with trimmed case-insensitive substrings, preserves order and requires selection after edits", () => {
    mount(["Beta", "Alpha", "alphabet"]);
    expect(document.activeElement).toBe(search());
    expect(create().disabled).toBe(true);
    input(search(), "  ALP ");
    expect(options()).toEqual(["", "Alpha", "alphabet"]);
    choose("Alpha");
    expect(create().disabled).toBe(false);
    input(search(), "alp");
    expect(select().value).toBe("");
    expect(create().disabled).toBe(true);
    input(search(), "missing");
    expect(document.querySelector('[role="status"]')?.textContent).toBe("No teams match your search.");
    input(search(), "");
    expect(options()).toEqual(["", "Beta", "Alpha", "alphabet"]);
    expect(create().disabled).toBe(true);
  });

  it("shows no teams accessibly and never auto-selects teams added by refresh", () => {
    const { setTeams } = mount([]);
    expect(document.querySelector('[role="status"]')?.textContent).toBe("No teams available.");
    expect(create().disabled).toBe(true);
    setTeams(teams(["Alpha"]));
    expect(select().value).toBe("");
    expect(create().disabled).toBe(true);
  });

  it("preselects only an initially unique team and invalidates removed teams while retaining drafts", () => {
    const { setTeams } = mount(["Alpha"]);
    expect(select().value).toBe("Alpha");
    expect(create().disabled).toBe(false);
    input(search(), "alp");
    choose("Alpha");
    input(title(), "Draft");
    setTeams(teams(["Alpha", "alphabet", "Beta"]));
    expect(select().value).toBe("Alpha");
    expect(options()).toEqual(["", "Alpha", "alphabet"]);
    setTeams(teams(["alphabet", "Beta"]));
    expect(select().value).toBe("");
    expect(create().disabled).toBe(true);
    expect(search().value).toBe("alp");
    expect(title().value).toBe("Draft");
  });

  it("B2: Enter in a selected list consumes the event; Enter in the title creates once with empty title", async () => {
    const { fake, onClose } = mount();
    choose("Beta");
    expect(create().disabled).toBe(false);
    key(select(), "Enter");
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
    expect(select().value).toBe("Beta");
    expect(document.querySelector(".modal-overlay")).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    key(title(), "Enter");
    key(title(), "Enter");
    expect(fake.callsFor("create_workgroup")).toHaveLength(1);
    expect(fake.lastCall("create_workgroup")?.args).toEqual({ projectPath, teamName: "Beta", taskTitle: "" });
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("B2: first Escape in a selected list focuses search; second Escape closes exactly once", () => {
    const { onClose } = mount();
    choose("Alpha");
    expect(create().disabled).toBe(false);
    select().focus();
    key(select(), "Escape");
    expect(onClose).not.toHaveBeenCalled();
    expect(document.querySelector(".modal-overlay")).toBeTruthy();
    expect(document.activeElement).toBe(search());
    key(search(), "Escape");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("ignores Shift+Enter and IME Enter/Escape", () => {
    const { fake, onClose } = mount(["Alpha"]);
    key(title(), "Enter", { shiftKey: true });
    key(title(), "Enter", { isComposing: true });
    key(select(), "Enter", { isComposing: true });
    key(select(), "Escape", { isComposing: true });
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
    expect(onClose).not.toHaveBeenCalled();
  });

  it.each([["   ", ""], ["  Explicit title  ", "Explicit title"]])("sends trimmed title %j as %j", async (draft, expected) => {
    const { fake, onClose } = mount(["Alpha"]);
    input(title(), draft);
    click(create());
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expect(fake.lastCall("create_workgroup")?.args.taskTitle).toBe(expected);
  });

  it("blocks duplicates during pending creation and permits retry after a visible failure", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const { fake, onClose } = mount(["Alpha"]);
    let reject!: (reason: string) => void;
    fake.onInvoke("create_workgroup", () => new Promise((_, fail) => { reject = fail; }));
    click(create());
    key(title(), "Enter");
    click(create());
    expect(create().disabled).toBe(true);
    expect(fake.callsFor("create_workgroup")).toHaveLength(1);
    reject("Clone failed");
    await waitFor(() => expect(document.querySelector(".new-agent-error")?.textContent).toBe("Clone failed"));
    expect(create().disabled).toBe(false);
    fake.resolve("create_workgroup", undefined);
    click(create());
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expect(fake.callsFor("create_workgroup")).toHaveLength(2);
  });
});
