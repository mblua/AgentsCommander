// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createSignal } from "solid-js";
import NewWorkgroupModal from "./NewWorkgroupModal";
import type { AcTeam } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { click, discovery, input, renderWithFakeTransport, resetUiStoresForTests, waitFor } from "../../shared/testing/ui-harness";

import { executeAutomationRequest, resetAutomationBridgeForTests } from "../../shared/automation-bridge";
import type { UiAutomationAction } from "../../shared/types";
import { stubAutomationGeometry } from "../../shared/testing/automation-geometry";

// Geometry enables bridge dispatch in jsdom; it does not establish Windows
// visibility, hit-testing, physical keyboard/focus behavior or real IME coverage.
async function request(action: UiAutomationAction, selector: string, value?: string) {
  return executeAutomationRequest("main", {
    requestId: action + selector, token: "test", window: "main", action, selector, value,
    expiresAtUnixMs: Date.now() + 5000,
  });
}
async function automate(action: UiAutomationAction, selector: string, value?: string) {
  expect(document.querySelectorAll('[data-ac-testid="' + selector + '"]')).toHaveLength(1);
  const response = await request(action, selector, value);
  if (!response.ok) throw new Error(response.error + ": " + response.message);
  return response.target;
}

async function expectConfirmedListHidden() {
  expect((await automate("query", "newRoom.create")).disabled).toBe(false);
  expect(list().isConnected).toBe(true);
  expect(rows()).toHaveLength(1);
  for (const selector of ["newRoom.team.list", "newRoom.team.option.0"]) {
    const hidden = await request("query", selector);
    expect(hidden.ok).toBe(false);
    if (hidden.ok) throw new Error("Expected hidden target rejection: " + selector);
    expect(hidden.error).toBe("target_hidden");
  }
}

const projectPath = "C:\Project";
const teams = (names: string[]): AcTeam[] => names.map(name => ({ name, agents: [], coordinator: "" }));
let rendered: ReturnType<typeof renderWithFakeTransport> | undefined;
const originalScrollIntoView = Object.getOwnPropertyDescriptor(Element.prototype, "scrollIntoView");
const search = () => document.querySelector<HTMLInputElement>("#new-room-team-search")!;
const list = () => document.querySelector<HTMLDivElement>("#new-room-team-list")!;
const title = () => document.querySelector<HTMLInputElement>('input[placeholder="Task title (optional)"]')!;
const create = () => document.querySelector<HTMLButtonElement>(".new-agent-create-btn")!;
const rows = () => Array.from(list().querySelectorAll<HTMLDivElement>('[role="option"]'));
const options = () => rows().map(row => row.textContent);
const confirmed = () => document.querySelector('[data-ac-testid="newRoom.team.confirmed"]')!;
function choose(name: string) {
  const row = rows().find(row => row.textContent === name);
  expect(row).toBeDefined();
  row!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
}
function key(element: Element, key: string, init: KeyboardEventInit = {}) {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
  element.dispatchEvent(event);
  return event;
}
function composition(type: "compositionstart" | "compositionend") {
  search().dispatchEvent(new CompositionEvent(type, { bubbles: true }));
}
function reopen() { title().focus(); search().focus(); }
function expectActive(index: number) {
  expect(search().getAttribute("aria-activedescendant")).toBe(index < 0 ? null : `new-room-team-option-${index}`);
  expect(rows().filter(row => row.getAttribute("aria-selected") === "true")).toHaveLength(index < 0 ? 0 : 1);
  rows().forEach((row, i) => {
    expect(row.getAttribute("aria-selected")).toBe(String(i === index));
    expect(row.getAttribute("data-ac-state")).toBe(i === index ? "active" : "inactive");
  });
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
beforeEach(() => {
  resetUiStoresForTests();
  resetAutomationBridgeForTests();
  Object.defineProperty(Element.prototype, "scrollIntoView", { configurable: true, writable: true, value: vi.fn() });
});
afterEach(() => {
  rendered?.cleanup();
  rendered = undefined;
  resetUiStoresForTests();
  document.body.replaceChildren();
  vi.restoreAllMocks();
  if (originalScrollIntoView) Object.defineProperty(Element.prototype, "scrollIntoView", originalScrollIntoView);
  else delete (Element.prototype as Partial<Element>).scrollIntoView;
});

describe("New Room focused button Enter (#2809)", () => {
  it.each([
    { names: ["Alpha"], draft: "", createEnabled: true },
    { names: ["Alpha"], draft: "Draft", createEnabled: true },
    { names: ["Alpha", "Beta"], draft: "", createEnabled: false },
  ])("Cancel Enter with $names and title '$draft' closes without creating", async ({ names, draft, createEnabled }) => {
    const { fake, onClose } = mount(names);
    input(title(), draft);
    expect(create().disabled).toBe(!createEnabled);
    const cancel = document.querySelector<HTMLButtonElement>('[data-ac-testid="newRoom.cancel"]')!;
    cancel.focus();
    expect(document.activeElement).toBe(cancel);
    expect(key(cancel, "Enter").defaultPrevented).toBe(false);
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
    await Promise.resolve();
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
    // jsdom does not synthesize native keyboard clicks; model the default action explicitly.
    click(cancel);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
  });

  it("Create Enter leaves native activation to create exactly once", async () => {
    const { fake, onClose } = mount(["Alpha"]);
    expect(create().disabled).toBe(false);
    create().focus();
    expect(document.activeElement).toBe(create());
    expect(key(create(), "Enter").defaultPrevented).toBe(false);
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
    // Model the native keyboard click that jsdom does not synthesize.
    click(create());
    await waitFor(() => {
      expect(fake.callsFor("create_workgroup")).toHaveLength(1);
      expect(onClose).toHaveBeenCalledTimes(1);
    });
  });

  it("disabled Cancel stays inert during pending creation", async () => {
    const { fake, onClose } = mount(["Alpha"]);
    let resolve!: () => void;
    fake.onInvoke("create_workgroup", () => new Promise<void>((done) => { resolve = done; }));
    click(create());
    const cancel = document.querySelector<HTMLButtonElement>('[data-ac-testid="newRoom.cancel"]')!;
    expect(cancel.disabled).toBe(true);
    key(cancel, "Enter");
    cancel.click();
    expect(onClose).not.toHaveBeenCalled();
    expect(fake.callsFor("create_workgroup")).toHaveLength(1);
    resolve();
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expect(fake.callsFor("create_workgroup")).toHaveLength(1);
  });
});

describe("New Room team search and optional title (#2788)", () => {
  it("multiple teams mount focused/open without active option; Enter consumes without creating", () => {
    const { fake, onClose } = mount();
    expect(document.activeElement).toBe(search());
    expect(search().getAttribute("aria-expanded")).toBe("true");
    expect(list().hidden).toBe(false);
    expectActive(-1);
    expect(key(search(), "Enter").defaultPrevented).toBe(true);
    expect(key(search(), "Enter", { shiftKey: true }).defaultPrevented).toBe(true);
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
    expect(onClose).not.toHaveBeenCalled();
    expect(list().hidden).toBe(false);
  });

  it("D2-b: unique team mounts confirmed/closed and the very first Enter creates exactly once", async () => {
    const { fake, onClose } = mount(["Alpha"]);
    expect(document.activeElement).toBe(search());
    expect(search().value).toBe("Alpha");
    expect(search().getAttribute("aria-expanded")).toBe("false");
    expectActive(-1);
    expect(list().hidden).toBe(true);
    expect(options()).toEqual(["Alpha"]);
    expect(create().disabled).toBe(false);
    expect(confirmed().textContent).toBe("Selected team: Alpha");
    key(search(), "Enter");
    expect(fake.callsFor("create_workgroup")).toHaveLength(1);
    expect(fake.lastCall("create_workgroup")?.args).toEqual({ projectPath, teamName: "Alpha", taskTitle: "" });
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("D2-b: later blur/focus opens with no active option and Enter does not create", () => {
    const { fake, onClose } = mount(["Alpha"]);
    reopen();
    expect(list().hidden).toBe(false);
    expectActive(-1);
    expect(key(search(), "Enter").defaultPrevented).toBe(true);
    expect(create().disabled).toBe(false);
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
    expect(onClose).not.toHaveBeenCalled();
  });

  it("D2-b: suppression is released synchronously even when initial focus emits no event", () => {
    const focus = vi.spyOn(HTMLInputElement.prototype, "focus").mockImplementation(() => {});
    mount(["Alpha"]);
    expect(list().hidden).toBe(true);
    focus.mockRestore();
    search().focus();
    expect(document.activeElement).toBe(search());
    expect(list().hidden).toBe(false);
    expectActive(-1);
  });

  it("D2-b: arrows open, editing invalidates and two Escapes close list then modal", () => {
    const { fake, onClose } = mount(["Alpha"]);
    key(search(), "ArrowDown");
    expect(list().hidden).toBe(false);
    expectActive(0);
    input(search(), "Alpha");
    expectActive(-1);
    expect(create().disabled).toBe(true);
    expect(list().hidden).toBe(false);
    expect(key(search(), "Escape").defaultPrevented).toBe(true);
    expect(list().hidden).toBe(true);
    expect(onClose).not.toHaveBeenCalled();
    key(search(), "Escape");
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
  });

  it("clamps arrows without wrapping, updates active ARIA, scrolls nearest and never confirms", () => {
    mount();
    expect(key(search(), "ArrowDown").defaultPrevented).toBe(true);
    expectActive(0);
    expect(rows()[0].scrollIntoView).toHaveBeenLastCalledWith({ block: "nearest" });
    expect(vi.mocked(rows()[0].scrollIntoView).mock.contexts.slice(-1)[0]).toBe(rows()[0]);
    key(search(), "ArrowDown");
    expectActive(1);
    expect(vi.mocked(rows()[1].scrollIntoView).mock.contexts.slice(-1)[0]).toBe(rows()[1]);
    key(search(), "ArrowDown");
    expectActive(1);
    key(search(), "ArrowUp");
    expectActive(0);
    key(search(), "ArrowUp");
    expectActive(0);
    expect(create().disabled).toBe(true);
    key(search(), "Escape");
    expectActive(-1);
    expect(key(search(), "ArrowUp").defaultPrevented).toBe(true);
    expect(list().hidden).toBe(false);
    expectActive(0);
    expect(confirmed().textContent).toBe("No team selected.");
  });

  it("zero results have no active row, confirmation or creation even after arrows/Enter", () => {
    const { fake, onClose } = mount([]);
    expect(document.activeElement).toBe(search());
    expect(list().hidden).toBe(false);
    key(search(), "ArrowDown");
    key(search(), "ArrowUp");
    expectActive(-1);
    key(search(), "Enter");
    expect(create().disabled).toBe(true);
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
    expect(onClose).not.toHaveBeenCalled();
  });

  it("B2-A: Enter selects/closes first, then a later closed-input Enter creates once", async () => {
    const { fake, onClose } = mount();
    choose("Alpha");
    reopen();
    expectActive(-1);
    expect(key(search(), "Enter").defaultPrevented).toBe(true);
    expect(list().hidden).toBe(false);
    expect(create().disabled).toBe(false);
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
    expect(onClose).not.toHaveBeenCalled();
    key(search(), "ArrowDown");
    key(search(), "Enter");
    expect(list().hidden).toBe(true);
    expectActive(-1);
    expect(search().value).toBe("Alpha");
    expect(confirmed().textContent).toBe("Selected team: Alpha");
    expect(document.querySelector(".modal-overlay")).toBeTruthy();
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
    expect(onClose).not.toHaveBeenCalled();
    key(search(), "Enter");
    key(search(), "Enter");
    expect(fake.callsFor("create_workgroup")).toHaveLength(1);
    expect(fake.lastCall("create_workgroup")?.args).toEqual({ projectPath, teamName: "Alpha", taskTitle: "" });
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("Tab closes without confirming/consuming; explicit title focus preserves draft and blur preserves confirmation", () => {
    const { fake } = mount();
    input(title(), "Draft");
    input(search(), "Alpha");
    key(search(), "ArrowDown");
    expect(key(search(), "Tab").defaultPrevented).toBe(false);
    expectActive(-1);
    expect(list().hidden).toBe(true);
    expect(create().disabled).toBe(true);
    // jsdom does not perform Tab's default navigation: focus explicitly.
    title().focus();
    expect(document.activeElement).toBe(title());
    expect(title().value).toBe("Draft");
    search().focus();
    choose("Alpha");
    reopen();
    title().focus();
    expect(confirmed().textContent).toBe("Selected team: Alpha");
    expect(list().hidden).toBe(true);
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
    expect(Array.from(document.querySelectorAll(".new-room-modal input, .new-room-modal button"))).toEqual([
      search(), title(), document.querySelector('[data-ac-testid="newRoom.cancel"]'), create(),
    ]);
  });

  it("row mousedown is canceled to retain focus; click confirms without focusable row or reopening", () => {
    mount();
    const row = rows()[1];
    expect(row.hasAttribute("tabindex")).toBe(false);
    const down = new MouseEvent("mousedown", { bubbles: true, cancelable: true });
    row.dispatchEvent(down);
    expect(down.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(search());
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.activeElement).toBe(search());
    expect(search().value).toBe("Beta");
    expect(list().hidden).toBe(true);
    expect(confirmed().textContent).toBe("Selected team: Beta");
    expectActive(-1);
  });

  it("composition flag guards keys without isComposing; compositionend permits selection", () => {
    const { fake, onClose } = mount(["Alpha"]);
    composition("compositionstart");
    key(search(), "Enter");
    key(search(), "Escape");
    key(title(), "Enter");
    key(title(), "Escape");
    expect(list().hidden).toBe(true);
    input(search(), "Al");
    expect(create().disabled).toBe(true);
    for (const isComposing of [true, false]) {
      for (const eventKey of ["ArrowDown", "ArrowUp", "Enter", "Escape"]) {
        key(search(), eventKey, { isComposing });
      }
    }
    expect(list().hidden).toBe(false);
    expectActive(-1);
    expect(confirmed().textContent).toBe("No team selected.");
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
    expect(onClose).not.toHaveBeenCalled();
    composition("compositionend");
    key(search(), "ArrowDown");
    key(search(), "Enter");
    expect(list().hidden).toBe(true);
    expect(confirmed().textContent).toBe("Selected team: Alpha");
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
    expect(onClose).not.toHaveBeenCalled();
  });

  it("compositionstart then real blur without compositionend releases the document Escape guard", () => {
    const { fake, onClose } = mount();
    composition("compositionstart");
    input(search(), "Al");
    title().focus();
    expect(document.activeElement).toBe(title());
    expect(list().hidden).toBe(true);
    expectActive(-1);
    key(title(), "Escape");
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
    expect(confirmed().textContent).toBe("No team selected.");
  });

  it("refresh preserves open/free query/title, resets active, and retains/removes confirmation by name", () => {
    const { setTeams } = mount();
    input(title(), "Draft");
    input(search(), "  ALP ");
    key(search(), "ArrowDown");
    setTeams(teams(["alphabet", "Alpha", "Beta"]));
    expect(search().value).toBe("  ALP ");
    expect(title().value).toBe("Draft");
    expect(options()).toEqual(["alphabet", "Alpha"]);
    expect(list().hidden).toBe(false);
    expectActive(-1);
    expect(create().disabled).toBe(true);
    choose("Alpha");
    reopen();
    key(search(), "ArrowDown");
    setTeams(teams(["Alpha", "alphabet", "Beta"]));
    expect(confirmed().textContent).toBe("Selected team: Alpha");
    expect(search().value).toBe("Alpha");
    expect(options()).toEqual(["Alpha", "alphabet"]);
    expectActive(-1);
    expect(list().hidden).toBe(false);
    expect(create().disabled).toBe(false);
    setTeams(teams(["Beta"]));
    expect(confirmed().textContent).toBe("No team selected.");
    expect(search().value).toBe("");
    expect(options()).toEqual(["Beta"]);
    expectActive(-1);
    expect(list().hidden).toBe(false);
    expect(create().disabled).toBe(true);
    expect(title().value).toBe("Draft");
    expect(document.querySelector(".modal-overlay")).toBeTruthy();
  });

  it("D2-b bridge confirms the unique team before any key, with list/row present but hidden", async () => {
    stubAutomationGeometry();
    mount(["dev-alpha"]);
    const combo = await automate("query", "newRoom.teamSearch");
    expect(combo.role).toBe("combobox");
    expect(combo.expanded).toBe(false);
    expect(combo.state).toBe("confirmed");
    const confirmation = await automate("query", "newRoom.team.confirmed");
    expect(confirmation.text).toBe("Selected team: dev-alpha");
    expect(confirmation.state).toBe("confirmed");
    expect(JSON.parse(confirmation.metadata.detail)).toEqual({ selected: "dev-alpha" });
    await expectConfirmedListHidden();
  });

  it("exposes all unique R2 targets and confirms/invalidates teams through the real bridge", async () => {
    stubAutomationGeometry();
    const { setTeams } = mount(["dev-alpha", "dev-beta", "ops"]);
    expect((await automate("query", "newRoom.modal")).role).toBe("dialog");
    const combo = await automate("query", "newRoom.teamSearch");
    expect(combo.role).toBe("combobox");
    expect(combo.metadata.detail).toBe("Search teams...");
    expect(combo.state).toBe("unconfirmed");
    expect(combo.expanded).toBe(true);
    expect(search().getAttribute("aria-autocomplete")).toBe("list");
    expect(search().getAttribute("aria-controls")).toBe(list().id);
    expect(document.querySelector('label[for="new-room-team-search"]')?.textContent).toBe("Team");
    expect(document.querySelector('label[for="new-room-task-title"]')?.textContent).toBe("Task Title");
    expect((await automate("query", "newRoom.taskTitle")).metadata.detail).toBe("Task title (optional)");
    expect((await automate("query", "newRoom.taskTitle.hint")).role).toBe("text");
    expect((await automate("query", "newRoom.taskTitle.hint")).text).toBe("Leave empty to start with Clean.");
    expect((await automate("query", "newRoom.cancel")).text).toBe("Cancel");
    expect(document.querySelector("#task-keyhint")?.textContent).toBe("Enter to create");
    const retired = await request("query", "newRoom.team");
    expect(retired.ok).toBe(false);
    if (retired.ok) throw new Error("Retired native team target unexpectedly exists");
    expect(retired.error).toBe("missing_selector");
    const projection = async (names: string[], active: number) => {
      const target = await automate("query", "newRoom.team.list");
      expect(target.role).toBe("listbox");
      expect(list().getAttribute("aria-label")).toBe("Team");
      const detail = target.metadata.detail;
      expect(detail.length).toBeLessThanOrEqual(120);
      expect(JSON.parse(detail)).toEqual({ options: names, active });
      for (const [index, name] of names.entries()) {
        const row = await automate("query", `newRoom.team.option.${index}`);
        expect(rows()[index].getAttribute("role")).toBe("option");
        expect(rows()[index].hasAttribute("tabindex")).toBe(false);
        expect(row.role).toBe("text");
        expect(row.text).toBe(name);
        expect(row.metadata.detail).toBe(name);
        expect(row.state).toBe(index === active ? "active" : "inactive");
        expect(row.selected).toBe(index === active);
      }
    };
    const confirmation = async (name: string) => {
      const target = await automate("query", "newRoom.team.confirmed");
      expect(target.role).toBe("text");
      expect(target.state).toBe(name ? "confirmed" : "unconfirmed");
      expect(target.text).toBe(name ? `Selected team: ${name}` : "No team selected.");
      expect(target.metadata.detail.length).toBeLessThanOrEqual(120);
      expect(JSON.parse(target.metadata.detail)).toEqual({ selected: name });
    };
    await projection(["dev-alpha", "dev-beta", "ops"], -1);
    await confirmation("");
    expect((await automate("query", "newRoom.create")).disabled).toBe(true);
    await automate("key", "newRoom.teamSearch", "ArrowDown");
    await projection(["dev-alpha", "dev-beta", "ops"], 0);
    await automate("setValue", "newRoom.teamSearch", "  DEV ");
    await projection(["dev-alpha", "dev-beta"], -1);
    await confirmation("");
    expect((await automate("query", "newRoom.team.option.0")).text).toBe("dev-alpha");
    await automate("click", "newRoom.team.option.0");
    expect(document.activeElement).toBe(search());
    const selected = await automate("query", "newRoom.teamSearch");
    expect(selected.expanded).toBe(false);
    expect(selected.state).toBe("confirmed");
    await confirmation("dev-alpha");
    await expectConfirmedListHidden();
    await automate("key", "newRoom.teamSearch", "ArrowDown");
    await projection(["dev-alpha"], 0);
    await confirmation("dev-alpha");
    // A real focus transition makes bridge input-click reopen in jsdom.
    title().focus();
    await automate("click", "newRoom.teamSearch");
    await projection(["dev-alpha"], -1);
    await automate("setValue", "newRoom.taskTitle", "Draft");
    expect(title().value).toBe("Draft");
    await automate("setValue", "newRoom.teamSearch", "dev-alpha");
    await confirmation("");
    expect((await automate("query", "newRoom.create")).disabled).toBe(true);
    await automate("setValue", "newRoom.teamSearch", "missing");
    expect((await automate("query", "newRoom.team.empty")).text).toBe("No teams match your search.");
    expect(document.querySelector('[data-ac-testid="newRoom.team.empty"]')?.getAttribute("role")).toBe("status");
    expect((await automate("query", "newRoom.team.empty")).role).toBe("status");
    await projection([], -1);
    const missing = await request("query", "newRoom.team.option.0");
    expect(missing.ok).toBe(false);
    if (missing.ok) throw new Error("No-results row unexpectedly exists");
    expect(missing.error).toBe("missing_selector");
    expect((await automate("query", "newRoom.create")).disabled).toBe(true);
    await automate("setValue", "newRoom.teamSearch", "");
    expect(document.querySelector('[data-ac-testid="newRoom.team.empty"]')).toBeNull();
    await projection(["dev-alpha", "dev-beta", "ops"], -1);
    await confirmation("");
    setTeams([]);
    expect((await automate("query", "newRoom.team.empty")).text).toBe("No teams available.");
    await projection([], -1);
    expect((await automate("query", "newRoom.create")).disabled).toBe(true);
  });
  it("filters names with trimmed case-insensitive substrings, preserves order and requires selection after edits", () => {
    const { setTeams } = mount(["Beta", "Alpha", "alphabet"]);
    expect(document.activeElement).toBe(search());
    expect(create().disabled).toBe(true);
    input(search(), "  ALP ");
    expect(options()).toEqual(["Alpha", "alphabet"]);
    input(search(), "Alpha");
    expect(create().disabled).toBe(true);
    choose("Alpha");
    expect(search().value).toBe("Alpha");
    expect(list().hidden).toBe(true);
    expect(confirmed().textContent).toBe("Selected team: Alpha");
    expect(create().disabled).toBe(false);
    input(search(), "Alpha");
    expect(list().hidden).toBe(false);
    expectActive(-1);
    expect(confirmed().textContent).toBe("No team selected.");
    expect(create().disabled).toBe(true);
    input(search(), "missing");
    expect(document.querySelector('[role="status"]')?.textContent).toBe("No teams match your search.");
    input(search(), "");
    expect(options()).toEqual(["Beta", "Alpha", "alphabet"]);
    expect(create().disabled).toBe(true);
    setTeams([{ name: "Beta", agents: ["needle-agent"], coordinator: "needle-agent" }]);
    input(search(), "needle");
    expect(options()).toEqual([]);
    expect(create().disabled).toBe(true);
  });

  it("shows no teams accessibly and never auto-selects teams added by refresh", () => {
    const { setTeams } = mount([]);
    expect(document.querySelector('[role="status"]')?.textContent).toBe("No teams available.");
    expect(create().disabled).toBe(true);
    setTeams(teams(["Alpha"]));
    expect(confirmed().textContent).toBe("No team selected.");
    expect(create().disabled).toBe(true);
  });

  it("preselects only an initially unique team and invalidates removed teams while retaining drafts", () => {
    const { setTeams } = mount(["Alpha"]);
    expect(search().value).toBe("Alpha");
    expect(list().hidden).toBe(true);
    expect(create().disabled).toBe(false);
    input(search(), "alp");
    choose("Alpha");
    input(title(), "Draft");
    setTeams(teams(["Alpha", "alphabet", "Beta"]));
    expect(confirmed().textContent).toBe("Selected team: Alpha");
    expect(search().value).toBe("Alpha");
    expect(options()).toEqual(["Alpha", "alphabet"]);
    setTeams(teams(["alphabet", "Beta"]));
    expect(confirmed().textContent).toBe("No team selected.");
    expect(create().disabled).toBe(true);
    expect(search().value).toBe("");
    expect(options()).toEqual(["alphabet", "Beta"]);
    expect(title().value).toBe("Draft");
  });

  it("B2-A: open-list Enter with a confirmed team consumes; Enter in title creates once with empty title", async () => {
    const { fake, onClose } = mount();
    choose("Beta");
    expect(create().disabled).toBe(false);
    reopen();
    expectActive(-1);
    expect(key(search(), "Enter").defaultPrevented).toBe(true);
    expect(fake.callsFor("create_workgroup")).toHaveLength(0);
    expect(confirmed().textContent).toBe("Selected team: Beta");
    expect(list().hidden).toBe(false);
    expect(document.querySelector(".modal-overlay")).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    title().focus();
    key(title(), "Enter");
    key(title(), "Enter");
    expect(fake.callsFor("create_workgroup")).toHaveLength(1);
    expect(fake.lastCall("create_workgroup")?.args).toEqual({ projectPath, teamName: "Beta", taskTitle: "" });
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("B2-A: first Escape preserves confirmation and focus; second bubbling Escape closes exactly once", () => {
    const { onClose } = mount();
    choose("Alpha");
    expect(create().disabled).toBe(false);
    reopen();
    key(search(), "ArrowDown");
    expect(key(search(), "Escape").defaultPrevented).toBe(true);
    expect(onClose).not.toHaveBeenCalled();
    expect(document.querySelector(".modal-overlay")).toBeTruthy();
    expect(document.activeElement).toBe(search());
    expect(list().hidden).toBe(true);
    expectActive(-1);
    expect(confirmed().textContent).toBe("Selected team: Alpha");
    expect(create().disabled).toBe(false);
    key(search(), "Escape");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("ignores Shift+Enter and IME Enter/Escape", () => {
    const { fake, onClose } = mount(["Alpha"]);
    key(title(), "Enter", { shiftKey: true });
    key(search(), "Enter", { shiftKey: true });
    key(title(), "Enter", { isComposing: true });
    for (const eventKey of ["ArrowDown", "ArrowUp", "Enter", "Escape"]) {
      key(search(), eventKey, { isComposing: true });
    }
    key(title(), "Escape", { isComposing: true });
    expect(list().hidden).toBe(true);
    expectActive(-1);
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
    expect(create().textContent).toBe("Creating...");
    expect(document.querySelector(".wizard-loading")?.textContent).toBe("Creating room (cloning repos may take a moment)...");
    expect(document.querySelector<HTMLButtonElement>('[data-ac-testid="newRoom.cancel"]')?.disabled).toBe(true);
    reject("Clone failed");
    await waitFor(() => expect(document.querySelector(".new-agent-error")?.textContent).toBe("Clone failed"));
    expect(create().disabled).toBe(false);
    fake.resolve("create_workgroup", undefined);
    click(create());
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expect(fake.callsFor("create_workgroup")).toHaveLength(2);
  });
});
