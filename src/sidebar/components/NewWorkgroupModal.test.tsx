// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createSignal } from "solid-js";
import NewWorkgroupModal from "./NewWorkgroupModal";
import type { AcTeam } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { click, discovery, input, renderWithFakeTransport, resetUiStoresForTests, waitFor } from "../../shared/testing/ui-harness";

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
beforeEach(() => resetUiStoresForTests());
afterEach(() => {
  rendered?.cleanup();
  rendered = undefined;
  resetUiStoresForTests();
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

describe("New Room team search and optional title (#2788)", () => {
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
