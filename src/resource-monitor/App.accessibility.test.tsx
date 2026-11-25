// @vitest-environment jsdom
// S6853 characterization: the Resource Monitor filter labels are bound
// to their controls through literal `for`/`id` pairs (P1, false positive).
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ResourceMonitorApp from "./App";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  installBrowserDomStubs,
  renderWithFakeTransport,
  waitFor,
} from "../shared/testing/ui-harness";
import { resourceMonitorStore } from "../shared/stores/resourceMonitor";
import type { ResourceSnapshot } from "../shared/types";

type MonitorControl = HTMLInputElement | HTMLSelectElement;

// Label <-> control, both directions: text, `for`, `.control`, sole `.labels` entry.
function expectLabelAssociation(text: string, id: string) {
  const control = document.querySelector<MonitorControl>(`#${id}`);
  if (control === null) throw new Error(`no #${id}`);
  const [label, ...others] = Array.from(control.labels ?? []);
  expect(others).toEqual([]);
  expect([label?.textContent?.trim(), label?.htmlFor]).toEqual([text, id]);
  expect(label?.control).toBe(control);
  return { label, control };
}

// Focus an unrelated button, click the label text and report what jsdom's
// label activation reached; jsdom has no focus default, so focus that target.
function labelActivation(label: HTMLLabelElement): [HTMLButtonElement, Element[]] {
  const bystander = document.body.appendChild(document.createElement("button"));
  bystander.focus();
  expect(document.activeElement).toBe(bystander);
  const reached: Element[] = [];
  const spy = (e: Event) => void (e.target !== label && reached.push(e.target as Element));
  document.addEventListener("click", spy, { capture: true });
  click(label);
  document.removeEventListener("click", spy, { capture: true });
  reached.forEach((el) => (el as HTMLElement).focus());
  return [bystander, reached];
}

const expectLabelFocusesControl = (label: HTMLLabelElement, control: Element) => {
  expect(labelActivation(label)[1]).toEqual([control]);
  expect(document.activeElement).toBe(control);
};

const expectLabelLeavesFocus = (label: HTMLLabelElement) => {
  const [bystander, reached] = labelActivation(label);
  expect({ reached, focused: document.activeElement }).toEqual({ reached: [], focused: bystander });
};

// Positive control: rewiring `for` to an unknown id must break both directions.
const expectMismatchBreaksAssociation = (text: string, id: string) => {
  const { label, control } = expectLabelAssociation(text, id);
  label.setAttribute("for", "rm-unmatched-target");
  expect([label.control, control.labels?.length]).toEqual([null, 0]);
  expect(() => expectLabelAssociation(text, id)).toThrow();
  expectLabelLeavesFocus(label);
};

const PID_HELP = "Comma-separated PIDs from the observed process trees.";
const DISABLED_TITLE = "Resource monitoring is disabled, so there is nothing to filter.";
const SEARCH_TITLE = "Search agent, room, project, role and process names.";

const snapshot = (monitorEnabled: boolean): ResourceSnapshot => ({
  capturedAt: "2026-06-15T01:00:05.000Z",
  overallState: "ok",
  monitorEnabled,
  activeAgentGroups: 0,
  maxConcurrentAgentGroups: 1,
  appPrivateBytes: 1024 ** 3,
  appWorkingSetBytes: 2 * 1024 ** 3,
  warnings: [],
  groups: [],
});

async function renderMonitor(monitorEnabled: boolean) {
  const fake = new FakeTransport();
  fake.resolve("get_settings", baseSettings());
  fake.onInvoke("get_resource_snapshot", () => snapshot(monitorEnabled));
  const rendered = renderWithFakeTransport(() => <ResourceMonitorApp />, fake);
  await waitFor(() => expect(document.getElementById("rm-sort-field")).not.toBeNull());
  await waitFor(() => expect(fake.callsFor("get_resource_snapshot").length).toBeGreaterThan(0));
  return rendered;
}

const input = (id: string) => document.getElementById(id) as HTMLInputElement;

describe("ResourceMonitorApp S6853 label associations (#4945)", () => {
  let cleanupDom: (() => void) | null = null;
  let cleanupRender: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resourceMonitorStore.stopPolling();
  });

  afterEach(() => {
    cleanupRender?.();
    cleanupRender = null;
    resourceMonitorStore.stopPolling();
    cleanupDom?.();
    cleanupDom = null;
    document.body.innerHTML = "";
  });

  it("PID · AaHUB__CkehT9D_3bUKz label focuses the enabled PID input", async () => {
    cleanupRender = (await renderMonitor(true)).cleanup;
    const { label, control } = expectLabelAssociation("PID", "rm-filter-pid-input");
    const pid = control as HTMLInputElement;
    expect(pid.tagName).toBe("INPUT");
    expect(pid.type).toBe("text");
    expect(pid.getAttribute("inputmode")).toBe("numeric");
    expect(pid.getAttribute("aria-describedby")).toBe("rm-filter-pid-help");
    expect(document.getElementById("rm-filter-pid-help")?.textContent?.trim()).toBe(PID_HELP);
    expect(pid.hasAttribute("aria-invalid")).toBe(false);
    expect(pid.dataset.acTestid).toBe("resourceMonitor.filter.pid.input");
    expect(pid.dataset.acRole).toBe("input");
    await waitFor(() => expect(pid.disabled).toBe(false));
    expect(pid.title).toBe(PID_HELP);
    expectLabelFocusesControl(label, pid);
    expect(input("rm-filter-search-input").value).toBe("");
    expect(pid.value).toBe("");
  });

  it("PID · AaHUB__CkehT9D_3bUKz label does not focus the disabled PID input", async () => {
    cleanupRender = (await renderMonitor(false)).cleanup;
    const { label, control } = expectLabelAssociation("PID", "rm-filter-pid-input");
    await waitFor(() => expect(control.disabled).toBe(true));
    expect(control.title).toBe(DISABLED_TITLE);
    expectLabelLeavesFocus(label);
  });

  it("Search · AaHUB__CkehT9D_3bUK0 label focuses the enabled search input", async () => {
    cleanupRender = (await renderMonitor(true)).cleanup;
    const { label, control } = expectLabelAssociation("Search", "rm-filter-search-input");
    const search = control as HTMLInputElement;
    expect(search.tagName).toBe("INPUT");
    expect(search.type).toBe("text");
    expect(search.dataset.acTestid).toBe("resourceMonitor.filter.search.input");
    expect(search.dataset.acRole).toBe("input");
    await waitFor(() => expect(search.disabled).toBe(false));
    expect(search.title).toBe(SEARCH_TITLE);
    expectLabelFocusesControl(label, search);
    expect(input("rm-filter-pid-input").value).toBe("");
    expect(search.value).toBe("");
  });

  it("Search · AaHUB__CkehT9D_3bUK0 label does not focus the disabled input", async () => {
    cleanupRender = (await renderMonitor(false)).cleanup;
    const { label, control } = expectLabelAssociation("Search", "rm-filter-search-input");
    await waitFor(() => expect(control.disabled).toBe(true));
    expect(control.title).toBe(DISABLED_TITLE);
    expectLabelLeavesFocus(label);
  });

  it.each([true, false])(
    "Sort · AaHUB__CkehT9D_3bUK1 label focuses the select (monitorEnabled %s)",
    async (monitorEnabled) => {
      cleanupRender = (await renderMonitor(monitorEnabled)).cleanup;
      const { label, control } = expectLabelAssociation("Sort", "rm-sort-field");
      const sort = control as HTMLSelectElement;
      expect(sort.tagName).toBe("SELECT");
      expect(sort.disabled).toBe(false);
      expect(sort.dataset.acTestid).toBe("resourceMonitor.sort.field");
      expect(sort.dataset.acRole).toBe("input");
      expect(Array.from(sort.options, (o) => o.value)).toEqual([
        "default", "cpu", "private", "processes", "name",
      ]);
      expect(sort.value).toBe("default");
      const direction = document.querySelector<HTMLButtonElement>(
        '[data-ac-testid="resourceMonitor.sort.direction"]',
      );
      expect(direction?.disabled).toBe(true);
      expectLabelFocusesControl(label, sort);
      expect(sort.value).toBe("default");
      sort.value = "cpu";
      sort.dispatchEvent(new Event("change", { bubbles: true }));
      await waitFor(() => expect(direction?.disabled).toBe(false));
    },
  );

  it("positive control: an unmatched Sort label `for` breaks both directions", async () => {
    cleanupRender = (await renderMonitor(true)).cleanup;
    expectMismatchBreaksAssociation("Sort", "rm-sort-field");
  });
});
