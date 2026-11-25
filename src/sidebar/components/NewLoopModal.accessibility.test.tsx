// @vitest-environment jsdom
// S6853 characterization: the New Loop labels are bound to their
// controls through literal `for`/`id` pairs (P1, false positive).
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AcWorkgroup } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { click, input, renderWithFakeTransport, waitFor } from "../../shared/testing/ui-harness";
import NewLoopModal from "./NewLoopModal";

import { projectStore } from "../stores/project";

const KIND_BY_ID: Record<string, string> = {
  "new-loop-name": "INPUT",
  "new-loop-cron": "INPUT",
  "new-loop-workgroup": "SELECT",
  "new-loop-prompt": "TEXTAREA",
};

// Checks text, `for`, `.control` and the one-entry `.labels` list for a field.
function expectLabelAssociation(text: string, id: string) {
  const control = document.getElementById(id) as HTMLInputElement | HTMLSelectElement | null;
  if (!control) throw new Error(`#${id} absent`);
  if (KIND_BY_ID[id]) expect(control.tagName).toBe(KIND_BY_ID[id]);
  const labels = control.labels ? Array.from(control.labels) : [];
  expect(labels.map((l) => [l.textContent?.trim(), l.htmlFor])).toEqual([[text, id]]);
  const label = labels[0];
  expect(label.control).toBe(control);
  return { label, control };
}

// Clicking label text after focusing something unrelated. jsdom forwards the
// activation click (enabled controls only) without focusing; mimic the focus.
function clickLabelText(label: HTMLLabelElement) {
  const elsewhere = document.createElement("textarea");
  document.body.insertBefore(elsewhere, document.body.firstChild);
  elsewhere.focus();
  expect(document.activeElement).toBe(elsewhere);
  const forwarded = new Set<HTMLElement>();
  const watch = (evt: Event) => {
    const t = evt.target as HTMLElement;
    if (t !== label) forwarded.add(t);
  };
  document.documentElement.addEventListener("click", watch, true);
  click(label);
  document.documentElement.removeEventListener("click", watch, true);
  forwarded.forEach((t) => t.focus());
  return { elsewhere, forwarded: [...forwarded] };
}

const expectLabelFocusesControl = (label: HTMLLabelElement, control: Element) => {
  const { forwarded } = clickLabelText(label);
  expect(forwarded).toEqual([control]);
  expect(document.activeElement).toBe(control);
};

const expectLabelLeavesFocus = (label: HTMLLabelElement) => {
  const { elsewhere, forwarded } = clickLabelText(label);
  expect(forwarded).toHaveLength(0);
  expect(document.activeElement).toBe(elsewhere);
};

// Positive control: an unknown `for` target empties both directions.
function expectMismatchBreaksAssociation(text: string, id: string) {
  const found = expectLabelAssociation(text, id);
  found.label.htmlFor = "new-loop-nowhere";
  expect(found.label.control).toBeNull();
  expect(found.control.labels?.length).toBe(0);
  expect(() => expectLabelAssociation(text, id)).toThrow();
  expectLabelLeavesFocus(found.label);
}

function pressEnter(target: HTMLElement, init: KeyboardEventInit = {}): KeyboardEvent {
  const keydown = new KeyboardEvent("keydown", {
    bubbles: true,
    cancelable: true,
    key: "Enter",
    ...init,
  });
  target.dispatchEvent(keydown);
  return keydown;
}

const m = { reloadProject: vi.spyOn(projectStore, "reloadProject") };

const PROJECT = String.raw`C:\Project`;

function coordinatorRoom(name: string, agents: AcWorkgroup["agents"]): AcWorkgroup {
  return { name, path: String.raw`C:\Project\.ac\${name}`, task: null, agents };
}

const withOrchestrator = (): AcWorkgroup[] => [
  coordinatorRoom("wg-10-dev-team", [
    {
      name: "tech-lead",
      path: String.raw`C:\Project\.ac\wg-10-dev-team\__agent_tech-lead`,
      repoPaths: [],
      isCoordinator: true,
    },
  ]),
];

const withoutOrchestrator = (): AcWorkgroup[] => [coordinatorRoom("wg-10-dev-team", [])];

let cleanup: (() => void) | null = null;
let fake: FakeTransport;

function open(workgroups: AcWorkgroup[]): void {
  fake = new FakeTransport();
  fake.resolve("preview_loop_cron", {
    nextDueAt: "2026-06-14T12:00:00Z",
    upcoming: ["2026-06-14T12:00:00Z"],
  });
  fake.resolve("create_loop", { summary: null, promptBody: "" });
  cleanup = renderWithFakeTransport(
    () => <NewLoopModal projectPath={PROJECT} workgroups={workgroups} onClose={() => {}} />,
    fake,
  ).cleanup;
}

const control = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const createButton = () =>
  document.querySelector<HTMLButtonElement>('[data-ac-testid="loop.new.create"]');

async function seedValidForm(): Promise<HTMLTextAreaElement> {
  input(control<HTMLInputElement>("new-loop-name"), "Weekday standup");
  input(control<HTMLInputElement>("new-loop-cron"), "0 9 * * 1-5");
  const prompt = control<HTMLTextAreaElement>("new-loop-prompt");
  prompt.value = "Summarize blockers";
  prompt.dispatchEvent(new Event("input", { bubbles: true }));
  await waitFor(() =>
    expect(
      document.querySelector('[data-ac-testid="loop.new.cronPreview"]')?.textContent,
    ).toMatch(/^Next run: /),
  );
  expect(createButton()?.disabled).toBe(false);
  return prompt;
}

describe("NewLoopModal S6853 label associations (#4945)", () => {
  beforeEach(() => {
    m.reloadProject.mockReset();
    m.reloadProject.mockResolvedValue(undefined);
  });

  afterEach(() => {
    cleanup?.();
    cleanup = null;
    document.body.innerHTML = "";
  });

  it("Name · AaHUB_4zkehT9D_3bUGy label focuses the editable name input", () => {
    open(withOrchestrator());
    const { label, control: name } = expectLabelAssociation("Name", "new-loop-name");
    expect(name.tagName).toBe("INPUT");
    expect((name as HTMLInputElement).type).toBe("text");
    expect(name.hasAttribute("autofocus")).toBe(true);
    expect(name.disabled).toBe(false);
    expect(name.dataset.acTestid).toBe("loop.new.name");
    expectLabelFocusesControl(label, name);
    input(name as HTMLInputElement, "Weekday standup");
    expect(name.value).toBe("Weekday standup");
    expect(control<HTMLInputElement>("new-loop-cron").value).toBe("");
  });

  it("Cron · AaHUB_4zkehT9D_3bUGz label focuses the cron input with preview", async () => {
    open(withOrchestrator());
    const { label, control: cron } = expectLabelAssociation("Cron", "new-loop-cron");
    expect(cron.tagName).toBe("INPUT");
    expect(cron.disabled).toBe(false);
    expect(cron.dataset.acTestid).toBe("loop.new.cron");
    expectLabelFocusesControl(label, cron);
    expect(control<HTMLInputElement>("new-loop-name").value).toBe("");
    input(cron as HTMLInputElement, "0 9 * * 1-5");
    await waitFor(() => expect(fake.callsFor("preview_loop_cron")).toHaveLength(1));
    expect(fake.lastCall("preview_loop_cron")?.args).toEqual({ expr: "0 9 * * 1-5" });
  });

  it("Orchestrator · AaHUB_4zkehT9D_3bUG0 label focuses the enabled select", () => {
    open(withOrchestrator());
    const { label, control: select } = expectLabelAssociation(
      "Room Orchestrator",
      "new-loop-workgroup",
    );
    expect(select.tagName).toBe("SELECT");
    expect(select.disabled).toBe(false);
    expect(select.value).toBe("wg-10-dev-team");
    expect(select.dataset.acTestid).toBe("loop.new.workgroup");
    expectLabelFocusesControl(label, select);
    expect(select.value).toBe("wg-10-dev-team");
  });

  it("Orchestrator · AaHUB_4zkehT9D_3bUG0 label skips the select without options", () => {
    open(withoutOrchestrator());
    const { label, control: select } = expectLabelAssociation(
      "Room Orchestrator",
      "new-loop-workgroup",
    );
    expect(select.disabled).toBe(true);
    expect(Array.from((select as HTMLSelectElement).options, (o) => o.value)).toEqual([""]);
    expect(document.body.textContent).toContain(
      "A room with a verified orchestrator is required.",
    );
    expect(createButton()?.disabled).toBe(true);
    expectLabelLeavesFocus(label);
  });

  it("Prompt · AaHUB_4zkehT9D_3bUG1 label focuses the prompt textarea", () => {
    open(withOrchestrator());
    const { label, control: prompt } = expectLabelAssociation("Prompt", "new-loop-prompt");
    expect(prompt.tagName).toBe("TEXTAREA");
    expect(prompt.disabled).toBe(false);
    expect(prompt.dataset.acTestid).toBe("loop.new.prompt");
    expect(document.body.textContent).toContain("Ctrl+Enter to create");
    expectLabelFocusesControl(label, prompt);
  });

  it.each([
    ["Ctrl+Enter", { ctrlKey: true }],
    ["Meta+Enter", { metaKey: true }],
  ])("Prompt · AaHUB_4zkehT9D_3bUG1 %s creates exactly once", async (_name, mods) => {
    open(withOrchestrator());
    const prompt = await seedValidForm();
    expect(pressEnter(prompt, mods).defaultPrevented).toBe(true);
    await waitFor(() => expect(m.reloadProject).toHaveBeenCalledTimes(1));
    expect(fake.callsFor("create_loop")).toHaveLength(1);
    expect(fake.lastCall("create_loop")?.args).toMatchObject({
      request: {
        name: "Weekday standup",
        expr: "0 9 * * 1-5",
        workgroup: "wg-10-dev-team",
        promptBody: "Summarize blockers",
      },
    });
  });

  it("Prompt · AaHUB_4zkehT9D_3bUG1 plain Enter does not create", async () => {
    open(withOrchestrator());
    const prompt = await seedValidForm();
    expect(pressEnter(prompt).defaultPrevented).toBe(false);
    await new Promise<void>((done) => setTimeout(done, 20));
    expect(fake.callsFor("create_loop")).toHaveLength(0);
  });

  it("positive control: an unmatched Prompt label `for` breaks both directions", () => {
    open(withOrchestrator());
    expectMismatchBreaksAssociation("Prompt", "new-loop-prompt");
  });
});
