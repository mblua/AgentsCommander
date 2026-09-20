// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ResourceMonitorApp from "./App";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  waitFor,
} from "../shared/testing/ui-harness";
import { resourceMonitorStore } from "../shared/stores/resourceMonitor";
import { FILTER_DEBOUNCE_MS } from "../shared/pid-filter";
import type {
  ResourceAgentGroupSnapshot,
  ResourceProcessSnapshot,
  ResourceSnapshot,
} from "../shared/types";

// #2245 - the integral Resource Monitor view: PID filter, search, sort with
// pinned expansions, the single counter, the process tree, and the kill
// dialog's keyboard and focus behaviour.
//
// Three helpers below carry most of the weight, and each exists against a
// specific way this suite could pass while proving nothing:
//   * the transport returns a DEEP CLONE every call, because setSnapshot
//     compares by reference — hand back the same object and every "survives the
//     next poll" assertion is vacuous. Test 0 guards that.
//   * settle() is a real wait past the debounce window. waitFor returns on its
//     FIRST passing sample, so against a condition that is already true before
//     the timer fires it returns immediately and proves nothing. No debounce
//     assertion here uses waitFor.
//   * must() throws on an absent node, so an assertion can never be satisfied
//     by querying nothing.

const MB = 1024 ** 2;

const process = (
  pid: number,
  overrides: Partial<ResourceProcessSnapshot> = {}
): ResourceProcessSnapshot => ({
  pid,
  name: `proc-${pid}.exe`,
  privateBytes: 32 * MB,
  workingSetBytes: 48 * MB,
  cpuPercent: 0.5,
  killAllowed: true,
  ...overrides,
});

/**
 * Three groups, each carrying a distinct shape this suite needs:
 *   session-a  a normal tree, fully observed, with real depths
 *   session-b  partially observed (descendantsObserved: false)
 *   session-c  terminated, unknown metrics, and a rootPid that is ABSENT from
 *              its own processes[] — the root-only match of 3.3
 */
const baseSnapshot = (): ResourceSnapshot => ({
  capturedAt: "2026-09-19T08:30:00.000Z",
  overallState: "warn",
  monitorEnabled: true,
  activeAgentGroups: 2,
  maxConcurrentAgentGroups: 4,
  appPrivateBytes: 2 * 1024 ** 3,
  appWorkingSetBytes: 3 * 1024 ** 3,
  networkState: "observed",
  networkSummary: "Observed",
  warnings: [],
  groups: [
    {
      sessionId: "session-a",
      name: "alpha-agent",
      workgroup: "wg-1-alpha",
      agent: "dev-rust",
      project: "ProjA",
      rootPid: 100,
      state: "running",
      descendantsObserved: true,
      processCount: 3,
      privateBytes: 300 * MB,
      workingSetBytes: 400 * MB,
      cpuPercent: 5,
      networkState: "observed",
      networkSummary: "Observed",
      killAllowed: true,
      processes: [
        process(4242, { name: "node.exe", depth: 0 }),
        process(4243, { name: "child.exe", parentPid: 4242, depth: 2 }),
        process(4244, { name: "deep.exe", parentPid: 4243, depth: 9 }),
      ],
    },
    {
      sessionId: "session-b",
      name: "beta-agent",
      workgroup: "wg-2-beta",
      agent: "dev-ui",
      project: "ProjB",
      rootPid: 200,
      state: "running",
      descendantsObserved: false,
      processCount: 1,
      privateBytes: 100 * MB,
      workingSetBytes: 150 * MB,
      cpuPercent: 1,
      networkState: "observed",
      networkSummary: "Observed",
      killAllowed: true,
      processes: [process(5120, { name: "python.exe" })],
    },
    {
      sessionId: "session-c",
      name: "gamma-agent",
      workgroup: "wg-3-gamma",
      agent: "dev-docs",
      project: "ProjC",
      rootPid: 300,
      state: "terminated",
      descendantsObserved: true,
      processCount: 1,
      privateBytes: null,
      workingSetBytes: null,
      cpuPercent: null,
      networkState: "unknown",
      networkSummary: "Unknown",
      killAllowed: true,
      processes: [process(42, { name: "old.exe" })],
    },
  ],
});

/** Five evenly separated groups, for the pin tests only. Three rows cannot
 *  express "pinned at index 3 and 4, then the list shrinks to three". */
const pinSnapshot = (): ResourceSnapshot => {
  const snapshot = baseSnapshot();
  snapshot.groups = [50, 40, 30, 20, 10].map((cpu, index) => ({
    sessionId: `p-${index + 1}`,
    name: `pin-${index + 1}`,
    workgroup: "wg-pin",
    agent: "dev-pin",
    project: "ProjPin",
    rootPid: 900 + index,
    state: "running",
    descendantsObserved: true,
    processCount: 1,
    privateBytes: cpu * MB,
    workingSetBytes: cpu * MB,
    cpuPercent: cpu,
    networkState: "observed",
    networkSummary: "Observed",
    killAllowed: true,
    processes: [process(9000 + index)],
  })) satisfies ResourceAgentGroupSnapshot[];
  return snapshot;
};

/** Two groups tied on the metric, listed in DESCENDING sessionId order, so
 *  "kept snapshot order" and "sessionId ascending" are distinguishable. */
const tieSnapshot = (): ResourceSnapshot => {
  const snapshot = pinSnapshot();
  snapshot.groups = snapshot.groups.slice(0, 2).map((group, index) => ({
    ...group,
    sessionId: index === 0 ? "tie-z" : "tie-a",
    name: index === 0 ? "tie-z" : "tie-a",
    cpuPercent: 7,
  }));
  return snapshot;
};

const DEFAULT_KILL_RESULT = {
  sessionId: "session-a",
  state: "terminated",
  quarantined: false,
  message: "resource group terminated and verified",
  blockedBySecurity: false,
  finalized: true,
};

interface Harness {
  fake: FakeTransport;
  state: { current: ResourceSnapshot; fail: boolean };
}

function makeHarness(
  initial: ResourceSnapshot = baseSnapshot(),
  killHandler?: () => unknown
): Harness {
  const fake = new FakeTransport();
  const state = { current: initial, fail: false };
  fake.resolve("get_settings", baseSettings());
  fake.onInvoke("get_resource_snapshot", () => {
    if (state.fail) throw new Error("snapshot failed");
    // A DEEP CLONE, never the same object: setSnapshot compares by reference.
    return structuredClone(state.current);
  });
  fake.onInvoke("kill_resource_group", () =>
    killHandler ? killHandler() : DEFAULT_KILL_RESULT
  );
  return { fake, state };
}

/** Renders, then takes the polling timer out of the picture so every later
 *  snapshot change in a test is one the test asked for. */
async function renderApp(
  harness: Harness,
  props: { embedded?: boolean } = {}
): Promise<{ root: HTMLElement; cleanup: () => void }> {
  const rendered = renderWithFakeTransport(
    () => <ResourceMonitorApp {...props} />,
    harness.fake
  );
  await waitFor(() => {
    expect(resourceMonitorStore.polling).toBe(true);
  });
  resourceMonitorStore.stopPolling();
  await resourceMonitorStore.refresh();
  return rendered;
}

const advance = async (
  harness: Harness,
  mutate?: (snapshot: ResourceSnapshot) => void
): Promise<void> => {
  if (mutate) mutate(harness.state.current);
  await resourceMonitorStore.refresh();
};

/** A real wait, past the debounce window. Never a waitFor. */
const settle = async (): Promise<void> => {
  await new Promise((resolve) => setTimeout(resolve, FILTER_DEBOUNCE_MS + 120));
  await Promise.resolve();
};

const must = (root: HTMLElement, testid: string): HTMLElement => {
  const el = root.querySelector(`[data-ac-testid="${testid}"]`);
  if (!el) throw new Error(`missing ${testid}`);
  return el as HTMLElement;
};

const maybe = (root: HTMLElement, testid: string): Element | null =>
  root.querySelector(`[data-ac-testid="${testid}"]`);

const counter = (root: HTMLElement): string =>
  must(root, "resourceMonitor.filter.count").textContent ?? "";

const order = (root: HTMLElement): string[] =>
  Array.from(root.querySelectorAll(".rm-group-row")).map((el) =>
    (el.getAttribute("data-ac-testid") ?? "").replace("resourceMonitor.group.", "")
  );

const chips = (root: HTMLElement): HTMLElement[] =>
  Array.from(root.querySelectorAll<HTMLElement>(".rm-pid-chip"));

const pidInput = (root: HTMLElement): HTMLInputElement =>
  must(root, "resourceMonitor.filter.pid.input") as HTMLInputElement;

const searchInput = (root: HTMLElement): HTMLInputElement =>
  must(root, "resourceMonitor.filter.search.input") as HTMLInputElement;

/** `input()` from the shared harness is typed HTMLInputElement and dispatches
 *  an InputEvent, which a <select> onChange handler never receives. The sort
 *  FIELD is a select and the sort DIRECTION is a button, so they are driven
 *  differently on purpose. */
const selectOption = (el: HTMLSelectElement, value: string): void => {
  el.value = value;
  el.dispatchEvent(new Event("change", { bubbles: true }));
};

const sortBy = async (root: HTMLElement, value: string): Promise<void> => {
  selectOption(must(root, "resourceMonitor.sort.field") as HTMLSelectElement, value);
  await Promise.resolve();
};

const flipDirection = async (root: HTMLElement): Promise<void> => {
  click(must(root, "resourceMonitor.sort.direction"));
  await Promise.resolve();
};

const typePid = async (root: HTMLElement, value: string): Promise<void> => {
  input(pidInput(root), value);
  await settle();
};

const typeSearch = async (root: HTMLElement, value: string): Promise<void> => {
  input(searchInput(root), value);
  await settle();
};

const key = (el: HTMLElement, init: KeyboardEventInit): KeyboardEvent => {
  const event = new KeyboardEvent("keydown", {
    bubbles: true,
    cancelable: true,
    ...init,
  });
  el.dispatchEvent(event);
  return event;
};

describe("#2245 Resource Monitor integral view", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resourceMonitorStore.stopPolling();
  });

  afterEach(() => {
    resourceMonitorStore.stopPolling();
    cleanupDom?.();
    cleanupDom = null;
  });

  // 0
  it("guards the fixture: every call returns a fresh equal snapshot", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      const first = await harness.fake.invoke("get_resource_snapshot");
      const second = await harness.fake.invoke("get_resource_snapshot");
      // If these were the same object, setSnapshot would emit nothing and every
      // "survives a poll" assertion below would pass vacuously.
      expect(first).not.toBe(second);
      expect(first).toEqual(second);

      const before = must(rendered.root, "resourceMonitor.group.session-a");
      await advance(harness);
      const after = must(rendered.root, "resourceMonitor.group.session-a");
      expect(after).not.toBe(before);
    } finally {
      rendered.cleanup();
    }
  });

  // 7
  it("filters to the group holding the PID and counts the matching process", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      await typePid(rendered.root, "5120");
      expect(order(rendered.root)).toEqual(["session-b"]);
      expect(counter(rendered.root)).toBe(
        "Showing 1 of 3 agents - 1 matching process"
      );
      expect(counter(rendered.root)).not.toContain("matched by root PID only");
    } finally {
      rendered.cleanup();
    }
  });

  // 7c
  it("renders the counter with no filter active and never gates it", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      // must() throws if a mutant restores <Show when={filtersActive()}>.
      expect(counter(rendered.root)).toBe("Showing 3 of 3 agents");

      click(must(rendered.root, "resourceMonitor.filter.status.inactive"));
      await Promise.resolve();
      // The total stays 3: a mutant reading filteredGroups().length for T
      // changes the second number.
      expect(counter(rendered.root)).toBe("Showing 1 of 3 agents");

      click(must(rendered.root, "resourceMonitor.filter.status.all"));
      await Promise.resolve();
      expect(counter(rendered.root)).toBe("Showing 3 of 3 agents");
    } finally {
      rendered.cleanup();
    }
  });

  // 8
  it("counts two PIDs in one group once as a group and twice as processes", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      await typePid(rendered.root, "4242, 4243");
      expect(order(rendered.root)).toEqual(["session-a"]);
      expect(counter(rendered.root)).toBe(
        "Showing 1 of 3 agents - 2 matching processes"
      );
    } finally {
      rendered.cleanup();
    }
  });

  // 9
  it("reports a root-only match separately from a process match", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      // 300 is session-c's rootPid and is absent from its processes[].
      await typePid(rendered.root, "300");
      expect(order(rendered.root)).toEqual(["session-c"]);
      expect(counter(rendered.root)).toBe(
        "Showing 1 of 3 agents - 0 matching processes, 1 matched by root PID only"
      );
    } finally {
      rendered.cleanup();
    }
  });

  // 9b
  it("counts a group matching both ways as a process match and not a root-only one", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      // 100 is session-a's rootPid (absent from its processes[]); 4242 is one
      // of its processes. One group, matched both ways.
      await typePid(rendered.root, "100, 4242");
      expect(order(rendered.root)).toEqual(["session-a"]);
      expect(counter(rendered.root)).toBe(
        "Showing 1 of 3 agents - 1 matching process"
      );
      expect(counter(rendered.root)).not.toContain("matched by root PID only");
      for (const pid of [100, 4242]) {
        expect(
          must(rendered.root, `resourceMonitor.filter.pid.chip.${pid}`).getAttribute(
            "data-ac-state"
          )
        ).toBe("matched");
      }
    } finally {
      rendered.cleanup();
    }
  });

  // 10
  it("shows no chip until the debounce window has elapsed", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      input(pidInput(rendered.root), "5120");
      expect(chips(rendered.root)).toHaveLength(0);
      await settle();
      expect(chips(rendered.root)).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });

  // 10b
  it("cancels the pending apply on every keystroke", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      input(pidInput(rendered.root), "42");
      input(pidInput(rendered.root), "4242");
      await settle();

      expect(chips(rendered.root).map((c) => c.getAttribute("data-ac-testid"))).toEqual([
        "resourceMonitor.filter.pid.chip.4242",
      ]);

      // The clear is required, not tidying: under the set {4242} the group of
      // PID 42 is filtered out, and "not expanded" on an absent row passes
      // under the mutant. Clearing the PID filter collapses nothing, so the row
      // comes back carrying whatever expansion state it really had.
      click(must(rendered.root, "resourceMonitor.filter.pid.clear"));
      await settle();
      expect(
        must(rendered.root, "resourceMonitor.group.session-c.toggle").getAttribute(
          "aria-expanded"
        )
      ).toBe("false");
      // ...while the group the set really did match is open.
      expect(
        must(rendered.root, "resourceMonitor.group.session-a.toggle").getAttribute(
          "aria-expanded"
        )
      ).toBe("true");
    } finally {
      rendered.cleanup();
    }
  });

  // 10c
  it("cancels a pending apply when either clear button is used", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      await typePid(rendered.root, "5120");
      input(pidInput(rendered.root), "4242");
      click(must(rendered.root, "resourceMonitor.filter.pid.clear"));
      // A real wait: "zero chips" is already true the instant after the clear,
      // so a waitFor would return before the timer could have fired and the
      // "flush does not cancel" mutant would survive.
      await settle();
      expect(chips(rendered.root)).toHaveLength(0);
      expect(order(rendered.root)).toHaveLength(3);

      // Same for Clear filters. A status filter first, so the button is there.
      click(must(rendered.root, "resourceMonitor.filter.status.active"));
      await Promise.resolve();
      input(pidInput(rendered.root), "4242");
      click(must(rendered.root, "resourceMonitor.filter.clear"));
      await settle();
      expect(chips(rendered.root)).toHaveLength(0);
      expect(order(rendered.root)).toHaveLength(3);
    } finally {
      rendered.cleanup();
    }
  });

  // 11
  it("applies the valid PIDs of a mixed input and names the rejected tokens", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      await typePid(rendered.root, "5120, abc, 42a, -5");
      // The list is never blanked for a typo in the fourth PID.
      expect(order(rendered.root)).toEqual(["session-b"]);
      const notice = must(rendered.root, "resourceMonitor.filter.pid.error");
      expect(notice.textContent).toContain("abc");
      expect(notice.textContent).toContain("42a");
      expect(notice.textContent).toContain("-5");
      expect(notice.getAttribute("data-ac-state")).toBe("warn");
      expect(pidInput(rendered.root).getAttribute("aria-invalid")).toBe("true");
    } finally {
      rendered.cleanup();
    }
  });

  // 11b
  it("truncates beyond 32 PIDs and says so", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      await typePid(
        rendered.root,
        Array.from({ length: 40 }, (_, i) => 1000 + i).join(",")
      );
      expect(
        must(rendered.root, "resourceMonitor.filter.pid.error").textContent
      ).toContain("Only the first 32 PIDs are applied.");
      expect(chips(rendered.root)).toHaveLength(32);
    } finally {
      rendered.cleanup();
    }
  });

  // 12
  it("applies no PID filter at all when every token is invalid", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      await typePid(rendered.root, "abc, xyz");
      expect(order(rendered.root)).toHaveLength(3);
      expect(chips(rendered.root)).toHaveLength(0);
      expect(
        must(rendered.root, "resourceMonitor.filter.pid.error").getAttribute(
          "data-ac-state"
        )
      ).toBe("error");
    } finally {
      rendered.cleanup();
    }
  });

  // 13 + 13b
  it("states only that a PID is absent from the snapshot, and flags partial coverage", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      await typePid(rendered.root, "9999");
      expect(order(rendered.root)).toHaveLength(0);
      const empty = must(rendered.root, "resourceMonitor.empty");
      // Never "does not exist", "died", or "is not an agent process".
      expect(empty.textContent).toContain(
        "No process in this snapshot matches PID 9999"
      );
      expect(
        must(rendered.root, "resourceMonitor.filter.pid.chip.9999").getAttribute(
          "data-ac-state"
        )
      ).toBe("unmatched");

      // 13b: session-b is partially observed and does NOT match 9999, so it is
      // not visible; a visible-only rule would suppress the one notice that
      // explains the empty screen.
      expect(maybe(rendered.root, "resourceMonitor.filter.coverage")).not.toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  // 14a, 14b, 14c
  it("shows the partial pill on its own cause and the coverage notice on another", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      // 14b: no PID applied — pill yes, coverage no. They share a cause, not a
      // condition.
      expect(maybe(rendered.root, "resourceMonitor.group.session-b.partial")).not.toBeNull();
      expect(maybe(rendered.root, "resourceMonitor.filter.coverage")).toBeNull();
      // 14c: a fully observed group carries no pill.
      expect(maybe(rendered.root, "resourceMonitor.group.session-a.partial")).toBeNull();

      // 14a: the partial group visible AND a PID applied — both present.
      await typePid(rendered.root, "5120");
      expect(order(rendered.root)).toEqual(["session-b"]);
      expect(maybe(rendered.root, "resourceMonitor.group.session-b.partial")).not.toBeNull();
      expect(maybe(rendered.root, "resourceMonitor.filter.coverage")).not.toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  // 15
  it("composes filters with AND and still calls a present PID matched", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      click(must(rendered.root, "resourceMonitor.filter.status.inactive"));
      await Promise.resolve();
      await typePid(rendered.root, "5120");

      expect(order(rendered.root)).toHaveLength(0);
      // The GENERIC text, because the PID set is not the only active filter.
      expect(must(rendered.root, "resourceMonitor.empty").textContent).toContain(
        "No agents match the filters"
      );
      // Chip state is computed against the whole snapshot: 5120 IS in it, so
      // calling it unmatched would be a false claim.
      expect(
        must(rendered.root, "resourceMonitor.filter.pid.chip.5120").getAttribute(
          "data-ac-state"
        )
      ).toBe("matched");
    } finally {
      rendered.cleanup();
    }
  });

  // 16
  it("clears both inputs with Clear filters and one input with its own clear", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      await typePid(rendered.root, "5120");
      await typeSearch(rendered.root, "beta");
      expect(order(rendered.root)).toEqual(["session-b"]);

      click(must(rendered.root, "resourceMonitor.filter.pid.clear"));
      await settle();
      expect(chips(rendered.root)).toHaveLength(0);
      // Search survives its neighbour's clear.
      expect(searchInput(rendered.root).value).toBe("beta");
      expect(order(rendered.root)).toEqual(["session-b"]);

      await typePid(rendered.root, "5120");
      click(must(rendered.root, "resourceMonitor.filter.clear"));
      await settle();
      expect(pidInput(rendered.root).value).toBe("");
      expect(searchInput(rendered.root).value).toBe("");
      expect(order(rendered.root)).toHaveLength(3);
      expect(counter(rendered.root)).toBe("Showing 3 of 3 agents");
    } finally {
      rendered.cleanup();
    }
  });

  // 16b
  it("searches name, agent, room, project and process names, case-insensitively", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      const cases: ReadonlyArray<[string, string[]]> = [
        ["beta-agent", ["session-b"]], // group.name
        ["dev-docs", ["session-c"]], // group.agent
        ["wg-1-alpha", ["session-a"]], // group.workgroup
        ["ProjB", ["session-b"]], // group.project
        ["python", ["session-b"]], // a process name
        ["ALPHA-AGENT", ["session-a"]], // case-insensitive
      ];
      for (const [query, expected] of cases) {
        await typeSearch(rendered.root, query);
        expect(order(rendered.root), `search ${query}`).toEqual(expected);
      }

      await typeSearch(rendered.root, "zzz-nothing");
      expect(order(rendered.root)).toHaveLength(0);
      expect(must(rendered.root, "resourceMonitor.empty").textContent).toContain(
        "No agents match the filters"
      );
    } finally {
      rendered.cleanup();
    }
  });

  // 17
  it("expands every matching group at once and keeps a manual collapse", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      await typePid(rendered.root, "4242, 5120");
      expect(order(rendered.root)).toEqual(["session-a", "session-b"]);
      for (const sessionId of ["session-a", "session-b"]) {
        expect(
          must(rendered.root, `resourceMonitor.group.${sessionId}.toggle`).getAttribute(
            "aria-expanded"
          )
        ).toBe("true");
      }

      click(must(rendered.root, "resourceMonitor.group.session-a.toggle"));
      await advance(harness);
      // A manual collapse wins until the applied PID set changes again.
      expect(
        must(rendered.root, "resourceMonitor.group.session-a.toggle").getAttribute(
          "aria-expanded"
        )
      ).toBe("false");
      expect(
        must(rendered.root, "resourceMonitor.group.session-b.toggle").getAttribute(
          "aria-expanded"
        )
      ).toBe("true");
    } finally {
      rendered.cleanup();
    }
  });

  // 18
  it("sorts by CPU and keeps an unknown last in both directions", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      await sortBy(rendered.root, "cpu");
      expect(
        must(rendered.root, "resourceMonitor.sort.direction").getAttribute("data-ac-state")
      ).toBe("desc");
      expect(order(rendered.root)).toEqual(["session-a", "session-b", "session-c"]);

      await flipDirection(rendered.root);
      expect(
        must(rendered.root, "resourceMonitor.sort.direction").getAttribute("data-ac-state")
      ).toBe("asc");
      // session-c has cpuPercent: null. An Unknown must never head the list of
      // biggest consumers, and reversing must not promote it either.
      expect(order(rendered.root)).toEqual(["session-b", "session-a", "session-c"]);
    } finally {
      rendered.cleanup();
    }
  });

  // 18b
  it("sorts by private bytes, process count and name", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      await sortBy(rendered.root, "private");
      expect(order(rendered.root)).toEqual(["session-a", "session-b", "session-c"]);
      await flipDirection(rendered.root);
      expect(order(rendered.root)).toEqual(["session-b", "session-a", "session-c"]);

      await sortBy(rendered.root, "processes");
      // 3, 1, 1 — the tie between b and c breaks by sessionId ascending.
      expect(order(rendered.root)).toEqual(["session-a", "session-b", "session-c"]);
      await flipDirection(rendered.root);
      expect(order(rendered.root)).toEqual(["session-b", "session-c", "session-a"]);

      await sortBy(rendered.root, "name");
      // Name opens ascending, unlike the three metrics.
      expect(
        must(rendered.root, "resourceMonitor.sort.direction").getAttribute("data-ac-state")
      ).toBe("asc");
      expect(order(rendered.root)).toEqual(["session-a", "session-b", "session-c"]);
      await flipDirection(rendered.root);
      expect(order(rendered.root)).toEqual(["session-c", "session-b", "session-a"]);
    } finally {
      rendered.cleanup();
    }
  });

  // 18c
  it("breaks ties by sessionId ascending, in both directions and across a poll", async () => {
    const harness = makeHarness(tieSnapshot());
    const rendered = await renderApp(harness);
    try {
      await sortBy(rendered.root, "cpu");
      // Snapshot order is tie-z, tie-a. A mutant that drops the tie-break
      // leaves them that way.
      expect(order(rendered.root)).toEqual(["tie-a", "tie-z"]);
      await flipDirection(rendered.root);
      expect(order(rendered.root)).toEqual(["tie-a", "tie-z"]);
      await advance(harness);
      expect(order(rendered.root)).toEqual(["tie-a", "tie-z"]);
    } finally {
      rendered.cleanup();
    }
  });

  // 19
  it("pins one expanded row at its index across a poll, and releases it on collapse", async () => {
    const harness = makeHarness(pinSnapshot());
    const rendered = await renderApp(harness);
    try {
      await sortBy(rendered.root, "cpu");
      expect(order(rendered.root)).toEqual(["p-1", "p-2", "p-3", "p-4", "p-5"]);

      click(must(rendered.root, "resourceMonitor.group.p-3.toggle"));
      await Promise.resolve();

      // p-5 jumps to the front, which would push p-3 from index 2 to index 3.
      await advance(harness, (s) => {
        s.groups[4].cpuPercent = 100;
      });
      expect(order(rendered.root)).toEqual(["p-5", "p-1", "p-3", "p-2", "p-4"]);

      click(must(rendered.root, "resourceMonitor.group.p-3.toggle"));
      await Promise.resolve();
      expect(order(rendered.root)).toEqual(["p-5", "p-1", "p-2", "p-3", "p-4"]);
    } finally {
      rendered.cleanup();
    }
  });

  // 19b
  it("pins several expanded rows and releases only the one collapsed", async () => {
    const harness = makeHarness(pinSnapshot());
    const rendered = await renderApp(harness);
    try {
      await sortBy(rendered.root, "cpu");
      click(must(rendered.root, "resourceMonitor.group.p-1.toggle"));
      click(must(rendered.root, "resourceMonitor.group.p-2.toggle"));
      await Promise.resolve();

      // Reverse the metric outright: without the pins the order would invert.
      await advance(harness, (s) => {
        [50, 40, 30, 20, 10].forEach((_, i) => {
          s.groups[i].cpuPercent = 10 * (i + 1);
        });
      });
      expect(order(rendered.root)).toEqual(["p-1", "p-2", "p-5", "p-4", "p-3"]);

      click(must(rendered.root, "resourceMonitor.group.p-2.toggle"));
      await Promise.resolve();
      // p-2 falls back to its sorted position (last); p-1 is still pinned at 0.
      expect(order(rendered.root)).toEqual(["p-1", "p-5", "p-4", "p-3", "p-2"]);
    } finally {
      rendered.cleanup();
    }
  });

  // 19c
  it("clamps pins past the end of a shrunken list, deterministically", async () => {
    const harness = makeHarness(pinSnapshot());
    const rendered = await renderApp(harness);
    try {
      await sortBy(rendered.root, "cpu");
      click(must(rendered.root, "resourceMonitor.group.p-4.toggle"));
      click(must(rendered.root, "resourceMonitor.group.p-5.toggle"));
      await Promise.resolve();
      expect(order(rendered.root)).toEqual(["p-1", "p-2", "p-3", "p-4", "p-5"]);

      // Three entries left, and the two pinned rows now top the metric, so a
      // missing clamp or a lost order would be visible.
      await advance(harness, (s) => {
        s.groups = s.groups.filter((g) => g.sessionId !== "p-2" && g.sessionId !== "p-3");
        s.groups[1].cpuPercent = 90;
        s.groups[2].cpuPercent = 80;
      });
      // Sorted would be p-4, p-5, p-1. Pinned at 3 and 4, both clamp to the end
      // in ascending recorded-index order.
      expect(order(rendered.root)).toEqual(["p-1", "p-4", "p-5"]);
    } finally {
      rendered.cleanup();
    }
  });

  // 19d
  it("never resurrects a pinned row that stopped matching the filter", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      await sortBy(rendered.root, "cpu");
      await typePid(rendered.root, "4242");
      expect(order(rendered.root)).toEqual(["session-a"]);
      expect(
        must(rendered.root, "resourceMonitor.group.session-a.toggle").getAttribute(
          "aria-expanded"
        )
      ).toBe("true");

      await advance(harness, (s) => {
        s.groups[0].processes = s.groups[0].processes.filter((p) => p.pid !== 4242);
      });
      // Reinsertion only ever repositions a row already in the sorted list.
      expect(order(rendered.root)).toHaveLength(0);
      expect(maybe(rendered.root, "resourceMonitor.group.session-a")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  // 19e
  it("re-registers the pins at their new positions when the direction changes", async () => {
    const harness = makeHarness(pinSnapshot());
    const rendered = await renderApp(harness);
    try {
      await sortBy(rendered.root, "cpu");
      click(must(rendered.root, "resourceMonitor.group.p-1.toggle"));
      click(must(rendered.root, "resourceMonitor.group.p-2.toggle"));
      await Promise.resolve();

      await flipDirection(rendered.root);
      // The direction change empties the pin map; the open rows are re-pinned
      // at their NEW sorted positions in the same pass.
      expect(order(rendered.root)).toEqual(["p-5", "p-4", "p-3", "p-2", "p-1"]);

      // The mutation is what makes this non-vacuous: with the metric moved, an
      // unregistered pin lets the two rows swap.
      await advance(harness, (s) => {
        s.groups[0].cpuPercent = 40;
        s.groups[1].cpuPercent = 50;
      });
      expect(order(rendered.root)).toEqual(["p-5", "p-4", "p-3", "p-2", "p-1"]);
    } finally {
      rendered.cleanup();
    }
  });

  // 20
  it("sums every group's process count in the tile, unaffected by filters", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      const expected = baseSnapshot().groups.reduce((n, g) => n + g.processCount, 0);
      expect(expected).toBe(5);
      expect(
        must(rendered.root, "resourceMonitor.summary.processCount").textContent
      ).toContain(String(expected));

      // Pins the tile order that #2246 documents in prose.
      const tiles = Array.from(
        must(rendered.root, "resourceMonitor.summary").querySelectorAll(".rm-status-tile")
      ).map((el) => el.getAttribute("data-ac-testid"));
      expect(tiles).toEqual([
        "resourceMonitor.summary.state",
        "resourceMonitor.summary.activeGroups",
        "resourceMonitor.summary.processCount",
        "resourceMonitor.summary.appPrivateBytes",
        "resourceMonitor.summary.network",
      ]);

      await typePid(rendered.root, "5120");
      expect(order(rendered.root)).toEqual(["session-b"]);
      // No other tile reacts to filtering, and neither does this one.
      expect(
        must(rendered.root, "resourceMonitor.summary.processCount").textContent
      ).toContain(String(expected));
    } finally {
      rendered.cleanup();
    }
  });

  // 21
  it("indents the process tree by depth, clamped, with no calc()", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      click(must(rendered.root, "resourceMonitor.group.session-a.toggle"));
      await Promise.resolve();

      const cell = (pid: number): HTMLElement =>
        must(rendered.root, `resourceMonitor.group.session-a.process.${pid}.name`);
      expect(cell(4242).style.paddingLeft).toBe("0px");
      expect(cell(4243).style.paddingLeft).toBe("24px");
      // Clamped at 6 so a deep tree cannot push the name out of its column.
      expect(cell(4244).style.paddingLeft).toBe("72px");

      expect(
        must(rendered.root, "resourceMonitor.group.session-a.process.4243").getAttribute(
          "title"
        )
      ).toBe("PID 4243 - parent 4242");

      // A group whose depths are all absent renders exactly as it does today.
      click(must(rendered.root, "resourceMonitor.group.session-c.toggle"));
      await Promise.resolve();
      expect(
        must(rendered.root, "resourceMonitor.group.session-c.process.42.name").style
          .paddingLeft
      ).toBe("0px");
      expect(
        must(rendered.root, "resourceMonitor.group.session-c.process.42").getAttribute(
          "title"
        )
      ).toBe("PID 42");
    } finally {
      rendered.cleanup();
    }
  });

  // 22
  it("keeps the counter's process total equal to the marked rows", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      await typePid(rendered.root, "4242, 5120");
      expect(order(rendered.root)).toEqual(["session-a", "session-b"]);
      expect(counter(rendered.root)).toContain("2 matching processes");

      const marked = rendered.root.querySelectorAll('[data-ac-state="pid-match"]');
      expect(marked).toHaveLength(2);
    } finally {
      rendered.cleanup();
    }
  });

  // 23
  it("labels the kill dialog, focuses Cancel and cycles Tab inside it", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      click(must(rendered.root, "resourceMonitor.group.session-a.kill"));
      await waitFor(() => {
        expect(maybe(rendered.root, "resourceMonitor.killConfirm.dialog")).not.toBeNull();
      });

      const dialog = must(rendered.root, "resourceMonitor.killConfirm.dialog");
      expect(dialog.getAttribute("role")).toBe("dialog");
      expect(dialog.getAttribute("aria-modal")).toBe("true");
      expect(dialog.getAttribute("tabindex")).toBe("-1");
      const labelledBy = dialog.getAttribute("aria-labelledby");
      expect(labelledBy).toBeTruthy();
      expect(dialog.querySelector(`#${labelledBy}`)?.tagName).toBe("H2");

      const cancel = must(rendered.root, "resourceMonitor.killConfirm.cancel");
      const confirm = must(rendered.root, "resourceMonitor.killConfirm.confirm");
      // Never the destructive button.
      expect(document.activeElement).toBe(cancel);

      confirm.focus();
      expect(key(dialog, { key: "Tab" }).defaultPrevented).toBe(true);
      expect(document.activeElement).toBe(cancel);

      expect(key(dialog, { key: "Tab", shiftKey: true }).defaultPrevented).toBe(true);
      expect(document.activeElement).toBe(confirm);
    } finally {
      rendered.cleanup();
    }
  });

  // 23b
  it("holds focus on the dialog itself while a kill is in flight", async () => {
    // A holder rather than a bare `let`: TypeScript narrows a closure-assigned
    // local to `never` at the call site below.
    const release: { fn: (() => void) | null } = { fn: null };
    const harness = makeHarness(
      baseSnapshot(),
      () =>
        new Promise((resolve) => {
          release.fn = () => resolve(DEFAULT_KILL_RESULT);
        })
    );
    const rendered = await renderApp(harness);
    try {
      click(must(rendered.root, "resourceMonitor.group.session-a.kill"));
      await waitFor(() => {
        expect(maybe(rendered.root, "resourceMonitor.killConfirm.dialog")).not.toBeNull();
      });
      click(must(rendered.root, "resourceMonitor.killConfirm.confirm"));

      const dialog = must(rendered.root, "resourceMonitor.killConfirm.dialog");
      await waitFor(() => {
        expect(document.activeElement).toBe(dialog);
      });

      // Both buttons are disabled, so the enabled list is empty. A handler that
      // only special-cases the two buttons lets both keys escape from here.
      expect(key(dialog, { key: "Escape" }).defaultPrevented).toBe(false);
      expect(maybe(rendered.root, "resourceMonitor.killConfirm.dialog")).not.toBeNull();

      for (const shiftKey of [false, true]) {
        expect(key(dialog, { key: "Tab", shiftKey }).defaultPrevented).toBe(true);
        expect(document.activeElement).toBe(dialog);
      }
    } finally {
      release.fn?.();
      await waitFor(() => {
        expect(maybe(rendered.root, "resourceMonitor.killConfirm.dialog")).toBeNull();
      });
      rendered.cleanup();
    }
  });

  // 23c
  it("returns focus to the Agents heading when a finalized kill removes the group", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      click(must(rendered.root, "resourceMonitor.group.session-a.kill"));
      await waitFor(() => {
        expect(maybe(rendered.root, "resourceMonitor.killConfirm.dialog")).not.toBeNull();
      });

      // The precondition of this path, established rather than assumed: with a
      // poll in flight, refresh() returns the earlier request through
      // `if (inFlight) return inFlight;` and could still carry the group.
      expect(resourceMonitorStore.loading).toBe(false);
      const before = harness.fake.callsFor("get_resource_snapshot").length;

      // The refresh that follows the kill drops the group.
      harness.state.current.groups = harness.state.current.groups.filter(
        (g) => g.sessionId !== "session-a"
      );

      click(must(rendered.root, "resourceMonitor.killConfirm.confirm"));
      await waitFor(() => {
        expect(maybe(rendered.root, "resourceMonitor.group.session-a")).toBeNull();
      });
      expect(harness.fake.callsFor("get_resource_snapshot").length).toBe(before + 1);

      const heading = must(rendered.root, "resourceMonitor.groups.heading");
      expect(document.activeElement).toBe(heading);
      expect(document.activeElement).not.toBe(document.body);
    } finally {
      rendered.cleanup();
    }
  });

  // 23d
  it("returns focus to a re-queried Kill button after a poll replaced every row", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      click(must(rendered.root, "resourceMonitor.group.session-a.kill"));
      await waitFor(() => {
        expect(maybe(rendered.root, "resourceMonitor.killConfirm.dialog")).not.toBeNull();
      });

      // Every row node is replaced underneath the open modal, so a DOM
      // reference stored at open time would now be detached.
      await advance(harness);

      key(must(rendered.root, "resourceMonitor.killConfirm.dialog"), { key: "Escape" });
      await waitFor(() => {
        expect(maybe(rendered.root, "resourceMonitor.killConfirm.dialog")).toBeNull();
      });

      expect(document.activeElement).toBe(
        must(rendered.root, "resourceMonitor.group.session-a.kill")
      );
      expect(document.activeElement).not.toBe(document.body);
    } finally {
      rendered.cleanup();
    }
  });

  // 24
  it("disables both filter inputs when monitoring is off", async () => {
    const disabled = baseSnapshot();
    disabled.monitorEnabled = false;
    const harness = makeHarness(disabled);
    const rendered = await renderApp(harness);
    try {
      expect(pidInput(rendered.root).disabled).toBe(true);
      expect(searchInput(rendered.root).disabled).toBe(true);
      expect(pidInput(rendered.root).getAttribute("title")).toContain("disabled");
    } finally {
      rendered.cleanup();
    }
  });

  // 25
  it("keeps filtering the retained snapshot after a failed refresh", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      await typePid(rendered.root, "5120");
      expect(order(rendered.root)).toEqual(["session-b"]);

      harness.state.fail = true;
      await advance(harness);
      expect(resourceMonitorStore.error).not.toBeNull();
      // keepLastSnapshot is on, so the rows are still there and still filtered.
      expect(order(rendered.root)).toEqual(["session-b"]);
      expect(counter(rendered.root)).toBe(
        "Showing 1 of 3 agents - 1 matching process"
      );
    } finally {
      harness.state.fail = false;
      rendered.cleanup();
    }
  });

  // 25b, 25c, 25d
  it("renders every metric cell, hides exactly one element, and leaves the process header at six", async () => {
    const harness = makeHarness();
    const rendered = await renderApp(harness);
    try {
      for (const sessionId of ["session-a", "session-b", "session-c"]) {
        for (const metric of [
          "processCount",
          "privateBytes",
          "workingSetBytes",
          "cpu",
          "network",
        ]) {
          // Presence, not visibility: jsdom lays nothing out (11.6).
          expect(
            maybe(rendered.root, `resourceMonitor.group.${sessionId}.${metric}`),
            `${sessionId}.${metric}`
          ).not.toBeNull();
        }
        expect(
          must(rendered.root, `resourceMonitor.group.${sessionId}.workingSetBytes`).className
        ).not.toContain("rm-automation-metric");
      }

      const hidden = Array.from(
        rendered.root.querySelectorAll(".rm-automation-metric")
      );
      expect(hidden).toHaveLength(1);
      expect(hidden[0].getAttribute("data-ac-testid")).toBe(
        "resourceMonitor.summary.appWorkingSetBytes"
      );

      click(must(rendered.root, "resourceMonitor.group.session-a.toggle"));
      await Promise.resolve();
      const header = rendered.root.querySelector(".rm-process-header");
      if (!header) throw new Error("missing .rm-process-header");
      // A seventh cell would desynchronize the header from its rows, which
      // share a six-track template. No test would otherwise catch that.
      expect(Array.from(header.querySelectorAll("span")).map((s) => s.textContent)).toEqual([
        "Process",
        "PID",
        "Private",
        "Working Set",
        "CPU",
        "Kill Scope",
      ]);
    } finally {
      rendered.cleanup();
    }
  });
});
