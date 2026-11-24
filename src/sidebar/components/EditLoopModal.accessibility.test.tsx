// @vitest-environment jsdom
// #4945 S6853 characterization: the Edit Loop labels are bound to their
// controls through literal `for`/`id` pairs (P1, false positive).
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AcWorkgroup, LoopConfigDetails } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { click, input, renderWithFakeTransport, waitFor } from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import EditLoopModal from "./EditLoopModal";
import { loopSummaryFixture } from "./loop-summary-fixture";

type EditControl = HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement;

interface Pair {
  label: HTMLLabelElement;
  control: EditControl;
}

/** Bidirectional `for`/`id` association with exact visible label text. */
function expectLabelAssociation(text: string, id: string): Pair {
  const control = document.getElementById(id) as EditControl;
  expect(control, `#${id}`).toBeTruthy();
  expect(control.labels).toHaveLength(1);
  const label = control.labels!.item(0);
  expect(label).toMatchObject({ htmlFor: id, control });
  expect(label.textContent?.trim()).toBe(text);
  return { label, control };
}

/**
 * Moves focus to an unrelated input, then clicks the label. Returns the
 * elements jsdom's label activation re-dispatched the click to, focused the
 * way a browser would (jsdom skips that step).
 */
function activateFrom(label: HTMLLabelElement): { start: HTMLElement; targets: HTMLElement[] } {
  const start = document.createElement("input");
  start.setAttribute("aria-label", "unrelated");
  document.body.prepend(start);
  start.focus();
  expect(document.activeElement).toBe(start);
  const targets: HTMLElement[] = [];
  const listener = ({ target }: Event) => {
    if (target !== label) targets.push(target as HTMLElement);
  };
  document.body.addEventListener("click", listener, true);
  click(label);
  document.body.removeEventListener("click", listener, true);
  targets.forEach((target) => target.focus());
  expect(start.value).toBe("");
  return { start, targets };
}

function expectLabelFocusesControl(label: HTMLLabelElement, control: Element): void {
  const { targets } = activateFrom(label);
  expect(targets.length === 1 && targets[0] === control).toBe(true);
  expect(document.activeElement === control).toBe(true);
}

function expectLabelLeavesFocus(label: HTMLLabelElement): void {
  const { start, targets } = activateFrom(label);
  expect(targets).toEqual([]);
  expect(document.activeElement === start).toBe(true);
}

/** Positive control: proves the association checks above can fail. */
function expectMismatchBreaksAssociation(text: string, id: string): void {
  const pair = expectLabelAssociation(text, id);
  pair.label.htmlFor = "edit-loop-missing";
  expect(pair.label.control).toBe(null);
  expect(pair.control.labels).toHaveLength(0);
  expect(() => expectLabelAssociation(text, id)).toThrow();
  expectLabelLeavesFocus(pair.label);
}

const pressEnter = (target: HTMLElement, mods: KeyboardEventInit = {}): KeyboardEvent => {
  const options = Object.assign({ key: "Enter", bubbles: true, cancelable: true }, mods);
  const ev = new KeyboardEvent("keydown", options);
  target.dispatchEvent(ev);
  return ev;
};

const PROJECT = String.raw`C:\Project`;
const loop = loopSummaryFixture({ workgroup: "wg-10-dev-team" });
const details = (): LoopConfigDetails => ({ summary: loop, promptBody: "Summarize blockers" });

const withOrchestrator = (): AcWorkgroup[] => [
  {
    name: "wg-10-dev-team",
    path: String.raw`C:\Project\.ac\wg-10-dev-team`,
    task: null,
    agents: [
      {
        name: "tech-lead",
        path: String.raw`C:\Project\.ac\wg-10-dev-team\__agent_tech-lead`,
        repoPaths: [],
        isCoordinator: true,
      },
    ],
  },
];

let cleanup: (() => void) | null = null;
let fake: FakeTransport;
let reload: ReturnType<typeof vi.spyOn>;

/** Opens the modal; `loaded: false` keeps `get_loop_config` pending (loading). */
function open(workgroups: AcWorkgroup[], loaded = true): void {
  fake = new FakeTransport();
  fake.onInvoke("get_loop_config", () =>
    loaded ? details() : new Promise<LoopConfigDetails>(() => {}),
  );
  fake.resolve("preview_loop_cron", {
    nextDueAt: "2026-06-14T12:00:00Z",
    upcoming: ["2026-06-14T12:00:00Z"],
  });
  fake.resolve("update_loop", details());
  reload = vi.spyOn(projectStore, "reloadProject").mockResolvedValue(undefined);
  cleanup = renderWithFakeTransport(
    () => (
      <EditLoopModal projectPath={PROJECT} workgroups={workgroups} loop={loop} onClose={() => {}} />
    ),
    fake,
  ).cleanup;
}

const byId = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const saveButton = () =>
  document.querySelector<HTMLButtonElement>('[data-ac-testid="loop.edit.save"]');

async function waitLoaded(): Promise<void> {
  await waitFor(() => expect(byId<HTMLInputElement>("edit-loop-name").disabled).toBe(false));
}

/** Loaded form with an edited name and a ready cron preview. */
async function seedValidForm(): Promise<HTMLTextAreaElement> {
  await waitLoaded();
  input(byId<HTMLInputElement>("edit-loop-name"), "Weekday sync");
  await waitFor(() =>
    expect(
      document.querySelector('[data-ac-testid="loop.edit.cronPreview"]')?.textContent,
    ).toMatch(/^Next run: /),
  );
  expect(saveButton()?.disabled).toBe(false);
  return byId<HTMLTextAreaElement>("edit-loop-prompt");
}

describe("EditLoopModal S6853 label associations (#4945)", () => {
  afterEach(() => {
    cleanup?.();
    cleanup = null;
    reload?.mockRestore();
    document.body.innerHTML = "";
  });

  it("ID · AaHUB_7GkehT9D_3bUHh label skips the immutable disabled ID input", async () => {
    open(withOrchestrator());
    await waitLoaded();
    const { label, control: id } = expectLabelAssociation("ID", "edit-loop-id");
    expect(id.tagName).toBe("INPUT");
    expect((id as HTMLInputElement).type).toBe("text");
    expect(id.value).toBe("loop-standup");
    expect(id.disabled).toBe(true);
    expect(id.dataset.acTestid).toBe("loop.edit.id");
    expectLabelLeavesFocus(label);
  });

  it.each([
    ["Name · AaHUB_7GkehT9D_3bUHi", "Name", "edit-loop-name", "loop.edit.name", "Weekday standup"],
    ["Cron · AaHUB_7GkehT9D_3bUHj", "Cron", "edit-loop-cron", "loop.edit.cron", "0 9 * * 1-5"],
    [
      "Prompt · AaHUB_7GkehT9D_3bUHl",
      "Prompt",
      "edit-loop-prompt",
      "loop.edit.prompt",
      "Summarize blockers",
    ],
  ])("%s label focuses the loaded editable control", async (_case, text, id, testId, value) => {
    open(withOrchestrator());
    await waitLoaded();
    const { label, control } = expectLabelAssociation(text, id);
    expect(control.disabled).toBe(false);
    expect(control.dataset.acTestid).toBe(testId);
    expect(control.value).toBe(value);
    expectLabelFocusesControl(label, control);
    expect(control.value).toBe(value);
  });

  it.each([
    ["Name · AaHUB_7GkehT9D_3bUHi", "Name", "edit-loop-name"],
    ["Cron · AaHUB_7GkehT9D_3bUHj", "Cron", "edit-loop-cron"],
    ["Prompt · AaHUB_7GkehT9D_3bUHl", "Prompt", "edit-loop-prompt"],
    ["Orchestrator · AaHUB_7GkehT9D_3bUHk", "Room Orchestrator", "edit-loop-workgroup"],
  ])("%s label skips its control disabled while loading", (_case, text, id) => {
    open(withOrchestrator(), false);
    expect(document.body.textContent).toContain("Loading Loop...");
    const { label, control } = expectLabelAssociation(text, id);
    expect(control.disabled).toBe(true);
    expectLabelLeavesFocus(label);
  });

  it("Name · AaHUB_7GkehT9D_3bUHi label focuses the input that accepts edits", async () => {
    open(withOrchestrator());
    await waitLoaded();
    const { label, control } = expectLabelAssociation("Name", "edit-loop-name");
    expect(control.tagName).toBe("INPUT");
    expectLabelFocusesControl(label, control);
    input(control as HTMLInputElement, "Weekday sync");
    expect(control.value).toBe("Weekday sync");
    expect(byId<HTMLInputElement>("edit-loop-cron").value).toBe("0 9 * * 1-5");
  });

  it("Cron · AaHUB_7GkehT9D_3bUHj label focuses the input that drives preview", async () => {
    open(withOrchestrator());
    await waitLoaded();
    const { label, control } = expectLabelAssociation("Cron", "edit-loop-cron");
    expectLabelFocusesControl(label, control);
    fake.clearCalls();
    input(control as HTMLInputElement, "0 10 * * 1-5");
    await waitFor(() => expect(fake.callsFor("preview_loop_cron")).toHaveLength(1));
    expect(fake.lastCall("preview_loop_cron")?.args).toEqual({ expr: "0 10 * * 1-5" });
  });

  it("Orchestrator · AaHUB_7GkehT9D_3bUHk label focuses the loaded select", async () => {
    open(withOrchestrator());
    await waitLoaded();
    const { label, control: select } = expectLabelAssociation(
      "Room Orchestrator",
      "edit-loop-workgroup",
    );
    expect(select.tagName).toBe("SELECT");
    expect(select.disabled).toBe(false);
    expect(select.value).toBe("wg-10-dev-team");
    expect(select.dataset.acTestid).toBe("loop.edit.workgroup");
    expectLabelFocusesControl(label, select);
  });

  it("Orchestrator · AaHUB_7GkehT9D_3bUHk label skips the select without options", async () => {
    open([{ ...withOrchestrator()[0], agents: [] }]);
    await waitLoaded();
    const { label, control: select } = expectLabelAssociation(
      "Room Orchestrator",
      "edit-loop-workgroup",
    );
    expect(select.disabled).toBe(true);
    expect(Array.from((select as HTMLSelectElement).options, (o) => o.value)).toEqual([""]);
    expect(document.body.textContent).toContain(
      "A room with a verified orchestrator is required.",
    );
    expect(saveButton()?.disabled).toBe(true);
    expectLabelLeavesFocus(label);
  });

  it("Prompt · AaHUB_7GkehT9D_3bUHl label focuses the textarea with save hint", async () => {
    open(withOrchestrator());
    await waitLoaded();
    const { label, control } = expectLabelAssociation("Prompt", "edit-loop-prompt");
    expect(control.tagName).toBe("TEXTAREA");
    expect(document.body.textContent).toContain("Ctrl+Enter to save");
    expectLabelFocusesControl(label, control);
  });

  it.each([
    ["Ctrl+Enter", { ctrlKey: true }],
    ["Meta+Enter", { metaKey: true }],
  ])("Prompt · AaHUB_7GkehT9D_3bUHl %s updates exactly once", async (_name, mods) => {
    open(withOrchestrator());
    const prompt = await seedValidForm();
    expect(pressEnter(prompt, mods).defaultPrevented).toBe(true);
    await waitFor(() => expect(reload).toHaveBeenCalledTimes(1));
    expect(fake.callsFor("update_loop")).toHaveLength(1);
    expect(fake.lastCall("update_loop")?.args).toMatchObject({
      request: { projectPath: PROJECT, id: "loop-standup", name: "Weekday sync" },
    });
  });

  it("Prompt · AaHUB_7GkehT9D_3bUHl plain Enter does not update", async () => {
    open(withOrchestrator());
    const prompt = await seedValidForm();
    expect(pressEnter(prompt).defaultPrevented).toBe(false);
    await new Promise<void>((done) => setTimeout(done, 20));
    expect(fake.callsFor("update_loop")).toHaveLength(0);
  });

  it("positive control: an unmatched Name label `for` breaks both directions", async () => {
    open(withOrchestrator());
    await waitLoaded();
    expectMismatchBreaksAssociation("Name", "edit-loop-name");
  });
});
