// @vitest-environment jsdom
// S6853 characterization: the Coding Agent filter label is bound to its
// search input through a literal `for`/`id` pair (P1, false positive).
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AppSettings } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  input,
  renderWithFakeTransport,
  waitFor,
} from "../../shared/testing/ui-harness";
import AgentPickerModal from "./AgentPickerModal";

/**
 * Asserts the filter label and its input point at each other: literal text,
 * `htmlFor`, `HTMLLabelElement.control`, and `labels` holding only this label.
 */
function expectLabelAssociation(text: string, id: string) {
  const el = document.getElementById(id);
  expect(el).toBeInstanceOf(HTMLInputElement);
  const control = el as HTMLInputElement;
  const all = control.labels ? [...control.labels] : [];
  expect(all).toHaveLength(1);
  const label = all[0];
  expect(label.htmlFor).toBe(id);
  expect(label.textContent?.trim()).toBe(text);
  expect(label.control).toBe(control);
  return { label, control };
}

/** Clicks label text from unrelated focus; returns [previous focus, reached]. */
function clickFilterLabel(label: HTMLLabelElement): { before: Element; hits: Element[] } {
  const before = document.createElement("a");
  before.href = "#unrelated";
  document.body.append(before);
  before.focus();
  expect(document.activeElement).toBe(before);
  const hits: Element[] = [];
  const onClick = (e: MouseEvent) => {
    if (e.target instanceof HTMLElement && e.target !== label) hits.push(e.target);
  };
  window.addEventListener("click", onClick, true);
  click(label);
  window.removeEventListener("click", onClick, true);
  // jsdom performs label activation but not the browser's focus step.
  for (const hit of hits) (hit as HTMLElement).focus();
  return { before, hits };
}

function expectLabelFocusesControl(label: HTMLLabelElement, control: Element) {
  const { hits } = clickFilterLabel(label);
  expect(hits).toStrictEqual([control]);
  expect(document.activeElement).toBe(control);
}

function expectMismatchBreaksAssociation(text: string, id: string) {
  const { label, control } = expectLabelAssociation(text, id);
  label.htmlFor = "agentPickerNoSuchControl";
  expect(label.control).toBeNull();
  expect(control.labels?.length ?? 0).toBe(0);
  expect(() => expectLabelAssociation(text, id)).toThrow();
  const { before, hits } = clickFilterLabel(label);
  expect(hits).toHaveLength(0);
  expect(document.activeElement).toBe(before);
}

const FILTER_TEXT = "Filter by name or start line";
const AGENT_PATH = String.raw`C:\repos\proj\.ac\_agent_architect`;

function settings(agents: AppSettings["agents"]): AppSettings {
  return baseSettings({ agents });
}

const agent = (id: string, label: string, color: string): AppSettings["agents"][number] => ({
  id,
  label,
  command: id,
  color,
  envs: [],
  isolatedHome: false,
});

const twoAgents = (): AppSettings["agents"] => [
  agent("codex", "Codex", "#10b981"),
  agent("claude", "Claude Code", "#d97706"),
];

let cleanup: (() => void) | null = null;

function open(agents: AppSettings["agents"]): FakeTransport {
  const fake = new FakeTransport();
  fake.resolve("get_settings", settings(agents));
  cleanup = renderWithFakeTransport(
    () => (
      <AgentPickerModal
        sessionName="architect"
        agentPath={AGENT_PATH}
        currentAgentId="codex"
        explicitCurrentAgentId="codex"
        onSelect={vi.fn()}
        onClose={vi.fn()}
      />
    ),
    fake,
  ).cleanup;
  return fake;
}

const filterInput = () => document.getElementById("agentPickerAgentFilter");

describe("AgentPickerModal S6853 label association (#4945)", () => {
  afterEach(() => {
    cleanup?.();
    cleanup = null;
    document.body.innerHTML = "";
  });

  it("Filter · AaHUB_5CkehT9D_3bUG8 label focuses the agent search input", async () => {
    open(twoAgents());
    await waitFor(() => expect(filterInput()).not.toBeNull());
    const { label, control } = expectLabelAssociation(FILTER_TEXT, "agentPickerAgentFilter");
    const filter = control as HTMLInputElement;
    expect(filter.tagName).toBe("INPUT");
    expect(filter.type).toBe("search");
    expect(filter.disabled).toBe(false);
    expect(filter.getAttribute("aria-controls")).toBe("agentPickerAgentList");
    expect(document.getElementById("agentPickerAgentList")).not.toBeNull();
    expect(filter.getAttribute("autocomplete")).toBe("off");
    expect(filter.placeholder).toBe("name or command + args");
    expect(filter.dataset.acTestid).toBe("agentPicker.agentFilter");
    expectLabelFocusesControl(label, filter);
    expect(filter.value).toBe("");
    input(filter, "claude");
    expect(filter.value).toBe("claude");
  });

  it("Filter · AaHUB_5CkehT9D_3bUG8 label and input are absent without agents", async () => {
    const fake = open([]);
    await waitFor(() => expect(fake.callsFor("get_settings").length).toBeGreaterThan(0));
    await new Promise<void>((done) => setTimeout(done, 20));
    expect(filterInput()).toBeNull();
    expect(
      Array.from(document.querySelectorAll("label"), (l) => l.textContent?.trim()),
    ).not.toContain(FILTER_TEXT);
  });

  it("positive control: an unmatched Filter label `for` breaks both directions", async () => {
    open(twoAgents());
    await waitFor(() => expect(filterInput()).not.toBeNull());
    expectMismatchBreaksAssociation(FILTER_TEXT, "agentPickerAgentFilter");
  });
});
