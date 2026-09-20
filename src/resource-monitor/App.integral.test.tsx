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
  ResourceGroupState,
  ResourceProcessSnapshot,
  ResourceSnapshot,
} from "../shared/types";

const MIB = 1024 ** 2;
const GIB = 1024 ** 3;
const DEBOUNCE_WAIT_MS = FILTER_DEBOUNCE_MS + 120;

const fakeProcess = (
  pid: number,
  name: string,
  overrides: Partial<ResourceProcessSnapshot> = {}
): ResourceProcessSnapshot => ({
  pid,
  name,
  killAllowed: true,
  ...overrides,
});

interface GroupSpec {
  sessionId: string;
  name: string;
  workgroup?: string | null;
  agent?: string | null;
  project?: string | null;
  rootPid?: number | null;
  state?: ResourceGroupState;
  descendantsObserved?: boolean;
  processCount?: number;
  privateBytes?: number | null;
  workingSetBytes?: number | null;
  cpuPercent?: number | null;
  processes?: ResourceProcessSnapshot[];
}

const fakeGroup = (spec: GroupSpec): ResourceAgentGroupSnapshot => ({
  sessionId: spec.sessionId,
  name: spec.name,
  workgroup: spec.workgroup === undefined ? "wg-1-ui" : spec.workgroup,
  agent: spec.agent === undefined ? "dev-web" : spec.agent,
  project: spec.project === undefined ? "ProjAlpha" : spec.project,
  rootPid: spec.rootPid === undefined ? 100 : spec.rootPid,
  state: spec.state ?? "running",
  descendantsObserved: spec.descendantsObserved ?? true,
  processCount: spec.processCount ?? 1,
  privateBytes: spec.privateBytes === undefined ? 300 * MIB : spec.privateBytes,
  workingSetBytes:
    spec.workingSetBytes === undefined ? 400 * MIB : spec.workingSetBytes,
  cpuPercent: spec.cpuPercent === undefined ? 10 : spec.cpuPercent,
  networkState: "observed",
  networkSummary: "Observed",
  killAllowed: true,
  processes: spec.processes ?? [],
});

const fakeSnapshot = (
  groups: ResourceAgentGroupSnapshot[]
): ResourceSnapshot => ({
  capturedAt: "2026-07-01T10:00:00.000Z",
  overallState: "ok",
  monitorEnabled: true,
  activeAgentGroups: groups.length,
  maxConcurrentAgentGroups: 6,
  appPrivateBytes: 3 * GIB,
  appWorkingSetBytes: 4 * GIB,
  networkState: "observed",
  networkSummary: "Observed",
  warnings: [],
  groups,
});

const alphaGroup = (): ResourceAgentGroupSnapshot =>
  fakeGroup({
    sessionId: "session-1",
    name: "alpha",
    workgroup: "wg-1-ui",
    agent: "dev-web",
    project: "ProjAlpha",
    rootPid: 100,
    processCount: 2,
    privateBytes: 100 * MIB,
    workingSetBytes: 200 * MIB,
    cpuPercent: 10,
    processes: [
      fakeProcess(100, "node.exe", { parentPid: null, depth: 0 }),
      fakeProcess(4242, "pwsh.exe", { parentPid: 100, depth: 1, cpuPercent: 2.5 }),
    ],
  });

const bravoGroup = (): ResourceAgentGroupSnapshot =>
  fakeGroup({
    sessionId: "session-2",
    name: "bravo",
    workgroup: "wg-2-core",
    agent: "tech-lead",
    project: "ProjBeta",
    rootPid: 200,
    processCount: 5,
    privateBytes: 300 * MIB,
    workingSetBytes: 500 * MIB,
    cpuPercent: 30,
    processes: [
      fakeProcess(200, "cargo.exe", { parentPid: null, depth: 0 }),
      fakeProcess(42, "helper.exe", { parentPid: 200, depth: 1 }),
      fakeProcess(5120, "rustc.exe", { parentPid: 200, depth: 1 }),
    ],
  });

const charlieGroup = (): ResourceAgentGroupSnapshot =>
  fakeGroup({
    sessionId: "session-3",
    name: "charlie",
    workgroup: "wg-3-ops",
    agent: "shipper",
    project: "ProjGamma",
    rootPid: 300,
    state: "terminated",
    processCount: 1,
    privateBytes: null,
    workingSetBytes: null,
    cpuPercent: null,
    processes: [fakeProcess(300, "python.exe", { parentPid: null, depth: 0 })],
  });

const deltaGroup = (): ResourceAgentGroupSnapshot =>
  fakeGroup({
    sessionId: "session-4",
    name: "delta",
    workgroup: "wg-4-build",
    agent: "shipper",
    project: "ProjDelta",
    rootPid: 400,
    processCount: 3,
    privateBytes: 250 * MIB,
    workingSetBytes: 350 * MIB,
    cpuPercent: 20,
    processes: [fakeProcess(400, "dotnet.exe", { parentPid: null, depth: 0 })],
  });

const echoGroup = (): ResourceAgentGroupSnapshot =>
  fakeGroup({
    sessionId: "session-5",
    name: "echo",
    workgroup: "wg-5-labs",
    agent: "dev-web",
    project: "ProjEcho",
    rootPid: 500,
    processCount: 4,
    privateBytes: 150 * MIB,
    workingSetBytes: 260 * MIB,
    cpuPercent: 40,
    processes: [fakeProcess(500, "code.exe", { parentPid: null, depth: 0 })],
  });

const baseSnapshot = (): ResourceSnapshot =>
  fakeSnapshot([alphaGroup(), bravoGroup(), charlieGroup()]);

const quadSnapshot = (): ResourceSnapshot =>
  fakeSnapshot([alphaGroup(), bravoGroup(), charlieGroup(), deltaGroup()]);

const pentaSnapshot = (): ResourceSnapshot =>
  fakeSnapshot([
    alphaGroup(),
    bravoGroup(),
    charlieGroup(),
    deltaGroup(),
    echoGroup(),
  ]);

const must = (root: ParentNode, testid: string): HTMLElement => {
  const el = root.querySelector(`[data-ac-testid="${testid}"]`);
  if (!el) throw new Error(`missing ${testid}`);
  return el as HTMLElement;
};

const pidInput = (root: ParentNode): HTMLInputElement =>
  must(root, "resourceMonitor.filter.pid.input") as HTMLInputElement;

const searchInput = (root: ParentNode): HTMLInputElement =>
  must(root, "resourceMonitor.filter.search.input") as HTMLInputElement;

const sortField = (root: ParentNode): HTMLSelectElement =>
  must(root, "resourceMonitor.sort.field") as HTMLSelectElement;

const sortDirection = (root: ParentNode): HTMLElement =>
  must(root, "resourceMonitor.sort.direction");

const countText = (root: ParentNode): string =>
  must(root, "resourceMonitor.filter.count").textContent ?? "";

const chips = (root: ParentNode): HTMLElement[] =>
  Array.from(
    root.querySelectorAll<HTMLElement>(
      '[data-ac-testid^="resourceMonitor.filter.pid.chip."]'
    )
  );

const renderedOrder = (root: ParentNode): string[] =>
  Array.from(root.querySelectorAll(".rm-group-row")).map((row) =>
    (row.getAttribute("data-ac-testid") ?? "").replace(
      "resourceMonitor.group.",
      ""
    )
  );

const selectOption = (el: HTMLSelectElement, value: string): void => {
  el.value = value;
  el.dispatchEvent(new Event("change", { bubbles: true }));
};

const keydown = (target: Element, key: string, shiftKey = false): KeyboardEvent => {
  const event = new KeyboardEvent("keydown", {
    key,
    shiftKey,
    bubbles: true,
    cancelable: true,
  });
  target.dispatchEvent(event);
  return event;
};

interface Harness {
  fake: FakeTransport;
  root: HTMLDivElement;
  cleanup: () => void;
  ready: Promise<void>;
  advance: (mutate?: (snapshot: ResourceSnapshot) => void) => Promise<void>;
  settle: () => Promise<void>;
  snapshotHandler: () => ResourceSnapshot;
  setSnapshot: (next: ResourceSnapshot) => void;
}

const createHarness = (initial: ResourceSnapshot): Harness => {
  const fake = new FakeTransport();
  fake.resolve("get_settings", baseSettings());
  let current = initial;
  const snapshotHandler = () => structuredClone(current);
  fake.onInvoke("get_resource_snapshot", snapshotHandler);
  const rendered = renderWithFakeTransport(() => <ResourceMonitorApp />, fake);

  // Stop the component's poll timer so every refresh below is one this test
  // starts, and settle the mount refresh so the store carries THIS harness's
  // snapshot before a test mutates it.
  const ready = (async () => {
    resourceMonitorStore.stopPolling();
    await resourceMonitorStore.refresh();
  })();

  const advance = async (mutate?: (snapshot: ResourceSnapshot) => void) => {
    if (mutate) mutate(current);
    await resourceMonitorStore.refresh();
  };

  const settle = async () => {
    await new Promise((resolve) => setTimeout(resolve, DEBOUNCE_WAIT_MS));
    await Promise.resolve();
  };

  return {
    fake,
    root: rendered.root,
    cleanup: rendered.cleanup,
    ready,
    advance,
    settle,
    snapshotHandler,
    setSnapshot: (next) => {
      current = next;
    },
  };
};

const waitForRows = async (harness: Harness, count: number): Promise<void> => {
  await harness.ready;
  await waitFor(() => {
    expect(harness.root.querySelectorAll(".rm-group-row").length).toBe(count);
  });
};

const filteredEmptyText = (harness: Harness): string =>
  must(harness.root, "resourceMonitor.empty").textContent ?? "";

describe("ResourceMonitorApp integral view", () => {
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

  it("0: deep-clones every poll and replaces row nodes", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      const first = harness.snapshotHandler();
      const second = harness.snapshotHandler();
      expect(first).not.toBe(second);
      expect(first).toEqual(second);

      const before = must(harness.root, "resourceMonitor.group.session-1");
      await harness.advance();
      const after = must(harness.root, "resourceMonitor.group.session-1");
      expect(after).not.toBe(before);
    } finally {
      harness.cleanup();
    }
  });

  it("7: narrows to the group holding a matched process", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      input(pidInput(harness.root), "4242");
      await harness.settle();
      expect(renderedOrder(harness.root)).toEqual(["session-1"]);
      expect(countText(harness.root)).toBe(
        "Showing 1 of 3 agents - 1 matching process"
      );
    } finally {
      harness.cleanup();
    }
  });

  it("7c: renders the counter without any filter and reads totals from the groups", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      expect(must(harness.root, "resourceMonitor.filter.count")).not.toBeNull();
      expect(countText(harness.root)).toBe("Showing 3 of 3 agents");

      click(must(harness.root, "resourceMonitor.filter.status.inactive"));
      await waitFor(() => {
        expect(countText(harness.root)).toBe("Showing 1 of 3 agents");
      });

      click(must(harness.root, "resourceMonitor.filter.clear"));
      await waitFor(() => {
        expect(countText(harness.root)).toBe("Showing 3 of 3 agents");
      });
    } finally {
      harness.cleanup();
    }
  });

  it("8: counts two matched PIDs in one group with a plural label", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      input(pidInput(harness.root), "100, 4242");
      await harness.settle();
      expect(renderedOrder(harness.root)).toEqual(["session-1"]);
      expect(countText(harness.root)).toBe(
        "Showing 1 of 3 agents - 2 matching processes"
      );
    } finally {
      harness.cleanup();
    }
  });

  it("9: reports a group matched only by its root PID", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      await harness.advance((snapshot) => {
        snapshot.groups[0].processes = [
          fakeProcess(4242, "pwsh.exe", { parentPid: 100, depth: 1 }),
        ];
      });
      input(pidInput(harness.root), "100");
      await harness.settle();
      expect(renderedOrder(harness.root)).toEqual(["session-1"]);
      expect(countText(harness.root)).toContain("0 matching processes");
      expect(countText(harness.root)).toContain("1 matched by root PID only");
    } finally {
      harness.cleanup();
    }
  });

  it("9b: a process match suppresses the root-PID-only clause", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      await harness.advance((snapshot) => {
        snapshot.groups[0].processes = [
          fakeProcess(4242, "pwsh.exe", { parentPid: 100, depth: 1 }),
        ];
      });
      input(pidInput(harness.root), "100, 4242");
      await harness.settle();
      expect(renderedOrder(harness.root)).toEqual(["session-1"]);
      expect(countText(harness.root)).toBe(
        "Showing 1 of 3 agents - 1 matching process"
      );
      expect(countText(harness.root)).not.toContain("root PID only");
      expect(
        must(harness.root, "resourceMonitor.filter.pid.chip.100").getAttribute(
          "data-ac-state"
        )
      ).toBe("matched");
      expect(
        must(harness.root, "resourceMonitor.filter.pid.chip.4242").getAttribute(
          "data-ac-state"
        )
      ).toBe("matched");
    } finally {
      harness.cleanup();
    }
  });

  it("10: shows no chip before the debounce window elapses", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      input(pidInput(harness.root), "4242");
      expect(chips(harness.root)).toHaveLength(0);
    } finally {
      harness.cleanup();
    }
  });

  it("10b: cancels the pending keystroke and never expands the superseded PID", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      input(pidInput(harness.root), "42");
      input(pidInput(harness.root), "4242");
      await harness.settle();
      expect(
        must(harness.root, "resourceMonitor.filter.pid.chip.4242")
      ).not.toBeNull();
      expect(
        harness.root.querySelector(
          '[data-ac-testid="resourceMonitor.filter.pid.chip.42"]'
        )
      ).toBeNull();

      click(must(harness.root, "resourceMonitor.filter.pid.clear"));
      await harness.settle();
      expect(
        must(harness.root, "resourceMonitor.group.session-2.toggle").getAttribute(
          "aria-expanded"
        )
      ).toBe("false");
    } finally {
      harness.cleanup();
    }
  });

  it("10c: a clear inside the window cancels the pending apply", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);

      input(pidInput(harness.root), "4242");
      click(must(harness.root, "resourceMonitor.filter.pid.clear"));
      await harness.settle();
      expect(chips(harness.root)).toHaveLength(0);
      expect(renderedOrder(harness.root)).toEqual([
        "session-1",
        "session-2",
        "session-3",
      ]);

      input(pidInput(harness.root), "4242");
      await harness.settle();
      expect(renderedOrder(harness.root)).toEqual(["session-1"]);
      input(pidInput(harness.root), "100");
      click(must(harness.root, "resourceMonitor.filter.clear"));
      await harness.settle();
      expect(chips(harness.root)).toHaveLength(0);
      expect(renderedOrder(harness.root)).toEqual([
        "session-1",
        "session-2",
        "session-3",
      ]);
    } finally {
      harness.cleanup();
    }
  });

  it("11: applies valid PIDs and names the rejected tokens", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      input(pidInput(harness.root), "4242, abc, 42a");
      await harness.settle();
      expect(renderedOrder(harness.root)).toEqual(["session-1"]);
      const notice = must(harness.root, "resourceMonitor.filter.pid.error");
      expect(notice.textContent).toContain("abc");
      expect(notice.textContent).toContain("42a");
    } finally {
      harness.cleanup();
    }
  });

  it("11b: truncates after 32 PIDs and says so", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      const text = Array.from({ length: 40 }, (_, index) => 6000 + index).join(
        ","
      );
      input(pidInput(harness.root), text);
      await harness.settle();
      expect(
        must(harness.root, "resourceMonitor.filter.pid.error").textContent
      ).toContain("Only the first 32 PIDs are applied.");
      expect(chips(harness.root)).toHaveLength(32);
    } finally {
      harness.cleanup();
    }
  });

  it("12: an all-invalid input applies no PID filter", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      click(must(harness.root, "resourceMonitor.filter.status.active"));
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual(["session-1", "session-2"]);
      });
      input(pidInput(harness.root), "abc, -5");
      await harness.settle();
      expect(
        must(harness.root, "resourceMonitor.filter.pid.error")
      ).not.toBeNull();
      expect(chips(harness.root)).toHaveLength(0);
      expect(renderedOrder(harness.root)).toEqual(["session-1", "session-2"]);
    } finally {
      harness.cleanup();
    }
  });

  it("13: an absent PID gives the PID-only empty state and an unmatched chip", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      input(pidInput(harness.root), "987654");
      await harness.settle();
      expect(renderedOrder(harness.root)).toEqual([]);
      expect(filteredEmptyText(harness)).toContain(
        "No process in this snapshot matches PID 987654"
      );
      expect(
        must(harness.root, "resourceMonitor.filter.pid.chip.987654").getAttribute(
          "data-ac-state"
        )
      ).toBe("unmatched");
    } finally {
      harness.cleanup();
    }
  });

  it("13b: coverage is evaluated over the non-PID-filtered groups", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      await harness.advance((snapshot) => {
        snapshot.groups[2].descendantsObserved = false;
      });
      input(pidInput(harness.root), "987654");
      await harness.settle();
      expect(renderedOrder(harness.root)).toEqual([]);
      expect(
        must(harness.root, "resourceMonitor.filter.coverage")
      ).not.toBeNull();
    } finally {
      harness.cleanup();
    }
  });

  it("14a: a partial visible group shows the pill and the coverage notice with a PID applied", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      await harness.advance((snapshot) => {
        snapshot.groups[0].descendantsObserved = false;
      });
      input(pidInput(harness.root), "4242");
      await harness.settle();
      expect(
        must(harness.root, "resourceMonitor.group.session-1.partial")
      ).not.toBeNull();
      expect(
        must(harness.root, "resourceMonitor.filter.coverage")
      ).not.toBeNull();
    } finally {
      harness.cleanup();
    }
  });

  it("14b: without a PID filter the pill stays and the coverage notice does not", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      await harness.advance((snapshot) => {
        snapshot.groups[0].descendantsObserved = false;
      });
      expect(
        must(harness.root, "resourceMonitor.group.session-1.partial")
      ).not.toBeNull();
      expect(
        harness.root.querySelector(
          '[data-ac-testid="resourceMonitor.filter.coverage"]'
        )
      ).toBeNull();
    } finally {
      harness.cleanup();
    }
  });

  it("14c: a fully observed group shows no pill", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      expect(
        harness.root.querySelector(
          '[data-ac-testid="resourceMonitor.group.session-1.partial"]'
        )
      ).toBeNull();
    } finally {
      harness.cleanup();
    }
  });

  it("15: composes status and PID with AND and keeps the chip matched", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      click(must(harness.root, "resourceMonitor.filter.status.inactive"));
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual(["session-3"]);
      });
      input(pidInput(harness.root), "4242");
      await harness.settle();
      expect(renderedOrder(harness.root)).toEqual([]);
      expect(filteredEmptyText(harness)).toContain("No agents match the filters");
      expect(
        must(harness.root, "resourceMonitor.filter.pid.chip.4242").getAttribute(
          "data-ac-state"
        )
      ).toBe("matched");
    } finally {
      harness.cleanup();
    }
  });

  it("16: clear controls reset only what they own", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      input(searchInput(harness.root), "alpha");
      await harness.settle();
      input(pidInput(harness.root), "4242");
      await harness.settle();
      expect(renderedOrder(harness.root)).toEqual(["session-1"]);

      click(must(harness.root, "resourceMonitor.filter.pid.clear"));
      await harness.settle();
      expect(searchInput(harness.root).value).toBe("alpha");
      expect(pidInput(harness.root).value).toBe("");
      expect(renderedOrder(harness.root)).toEqual(["session-1"]);

      click(must(harness.root, "resourceMonitor.filter.clear"));
      await harness.settle();
      expect(searchInput(harness.root).value).toBe("");
      expect(pidInput(harness.root).value).toBe("");
      expect(renderedOrder(harness.root)).toEqual([
        "session-1",
        "session-2",
        "session-3",
      ]);
    } finally {
      harness.cleanup();
    }
  });

  it("16b: search matches every field, ignores case and shows the generic empty state", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      const cases: Array<{ query: string; expected: string[] }> = [
        { query: "alpha", expected: ["session-1"] },
        { query: "tech-lead", expected: ["session-2"] },
        { query: "wg-3-ops", expected: ["session-3"] },
        { query: "ProjBeta", expected: ["session-2"] },
        { query: "rustc", expected: ["session-2"] },
      ];
      for (const { query, expected } of cases) {
        input(searchInput(harness.root), query);
        await harness.settle();
        expect(renderedOrder(harness.root)).toEqual(expected);
      }

      input(searchInput(harness.root), "ALPHA");
      await harness.settle();
      expect(renderedOrder(harness.root)).toEqual(["session-1"]);

      input(searchInput(harness.root), "zzz-nothing");
      await harness.settle();
      expect(renderedOrder(harness.root)).toEqual([]);
      expect(filteredEmptyText(harness)).toContain("No agents match the filters");
    } finally {
      harness.cleanup();
    }
  });

  it("17: expands every matching group at once and keeps a manual collapse", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      await harness.advance((snapshot) => {
        snapshot.groups[1].rootPid = 900;
        snapshot.groups[1].processes = [
          fakeProcess(4242, "twin.exe", { parentPid: 900, depth: 1 }),
        ];
      });
      input(pidInput(harness.root), "4242");
      await harness.settle();
      expect(
        must(harness.root, "resourceMonitor.group.session-1.toggle").getAttribute(
          "aria-expanded"
        )
      ).toBe("true");
      expect(
        must(harness.root, "resourceMonitor.group.session-2.toggle").getAttribute(
          "aria-expanded"
        )
      ).toBe("true");

      click(must(harness.root, "resourceMonitor.group.session-2.toggle"));
      await harness.advance();
      expect(
        must(harness.root, "resourceMonitor.group.session-2.toggle").getAttribute(
          "aria-expanded"
        )
      ).toBe("false");
      expect(
        must(harness.root, "resourceMonitor.group.session-1.toggle").getAttribute(
          "aria-expanded"
        )
      ).toBe("true");
    } finally {
      harness.cleanup();
    }
  });

  it("18: CPU sort orders both directions with unknown last", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      expect(sortDirection(harness.root).getAttribute("data-ac-state")).toBe(
        "disabled"
      );
      selectOption(sortField(harness.root), "cpu");
      expect(sortDirection(harness.root).getAttribute("data-ac-state")).toBe(
        "desc"
      );
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual([
          "session-2",
          "session-1",
          "session-3",
        ]);
      });
      click(sortDirection(harness.root));
      expect(sortDirection(harness.root).getAttribute("data-ac-state")).toBe(
        "asc"
      );
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual([
          "session-1",
          "session-2",
          "session-3",
        ]);
      });
    } finally {
      harness.cleanup();
    }
  });

  it("18b: private, processes and name sorts each order the whole list", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);

      selectOption(sortField(harness.root), "private");
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual([
          "session-2",
          "session-1",
          "session-3",
        ]);
      });
      click(sortDirection(harness.root));
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual([
          "session-1",
          "session-2",
          "session-3",
        ]);
      });

      selectOption(sortField(harness.root), "processes");
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual([
          "session-2",
          "session-1",
          "session-3",
        ]);
      });
      click(sortDirection(harness.root));
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual([
          "session-3",
          "session-1",
          "session-2",
        ]);
      });

      selectOption(sortField(harness.root), "name");
      expect(sortDirection(harness.root).getAttribute("data-ac-state")).toBe(
        "asc"
      );
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual([
          "session-1",
          "session-2",
          "session-3",
        ]);
      });
      click(sortDirection(harness.root));
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual([
          "session-3",
          "session-2",
          "session-1",
        ]);
      });
    } finally {
      harness.cleanup();
    }
  });

  it("18c: ties break by sessionId ascending in both directions", async () => {
    const harness = createHarness(
      fakeSnapshot([charlieGroup(), alphaGroup(), bravoGroup()])
    );
    try {
      await waitForRows(harness, 3);
      await harness.advance((snapshot) => {
        for (const group of snapshot.groups) group.cpuPercent = 12;
      });
      selectOption(sortField(harness.root), "cpu");
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual([
          "session-1",
          "session-2",
          "session-3",
        ]);
      });
      click(sortDirection(harness.root));
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual([
          "session-1",
          "session-2",
          "session-3",
        ]);
      });
      await harness.advance();
      expect(renderedOrder(harness.root)).toEqual([
        "session-1",
        "session-2",
        "session-3",
      ]);
    } finally {
      harness.cleanup();
    }
  });

  it("19: a pinned row keeps its index and a collapse releases it", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      selectOption(sortField(harness.root), "cpu");
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual([
          "session-2",
          "session-1",
          "session-3",
        ]);
      });
      click(must(harness.root, "resourceMonitor.group.session-2.toggle"));
      await harness.advance((snapshot) => {
        snapshot.groups[0].cpuPercent = 99;
        snapshot.groups[1].cpuPercent = 1;
      });
      expect(renderedOrder(harness.root)).toEqual([
        "session-2",
        "session-1",
        "session-3",
      ]);

      click(must(harness.root, "resourceMonitor.group.session-2.toggle"));
      expect(renderedOrder(harness.root)).toEqual([
        "session-1",
        "session-2",
        "session-3",
      ]);
    } finally {
      harness.cleanup();
    }
  });

  it("19b: two pins hold their recorded indices through a reorder", async () => {
    const harness = createHarness(quadSnapshot());
    try {
      await waitForRows(harness, 4);
      selectOption(sortField(harness.root), "cpu");
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual([
          "session-2",
          "session-4",
          "session-1",
          "session-3",
        ]);
      });
      click(must(harness.root, "resourceMonitor.group.session-1.toggle"));
      click(must(harness.root, "resourceMonitor.group.session-2.toggle"));

      await harness.advance((snapshot) => {
        snapshot.groups[0].cpuPercent = 50;
        snapshot.groups[1].cpuPercent = 5;
      });
      expect(renderedOrder(harness.root)).toEqual([
        "session-2",
        "session-4",
        "session-1",
        "session-3",
      ]);

      click(must(harness.root, "resourceMonitor.group.session-2.toggle"));
      expect(renderedOrder(harness.root)).toEqual([
        "session-4",
        "session-2",
        "session-1",
        "session-3",
      ]);
    } finally {
      harness.cleanup();
    }
  });

  it("19c: clamps two pins to the end in recorded order when the list shrinks", async () => {
    const harness = createHarness(pentaSnapshot());
    try {
      await waitForRows(harness, 5);
      selectOption(sortField(harness.root), "cpu");
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual([
          "session-5",
          "session-2",
          "session-4",
          "session-1",
          "session-3",
        ]);
      });
      click(must(harness.root, "resourceMonitor.group.session-1.toggle"));
      click(must(harness.root, "resourceMonitor.group.session-3.toggle"));

      await harness.advance((snapshot) => {
        snapshot.groups = snapshot.groups.filter(
          (group) =>
            group.sessionId !== "session-4" && group.sessionId !== "session-5"
        );
      });
      expect(renderedOrder(harness.root)).toEqual([
        "session-2",
        "session-1",
        "session-3",
      ]);
    } finally {
      harness.cleanup();
    }
  });

  it("19d: a pinned group that stops matching is not reinserted", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      selectOption(sortField(harness.root), "cpu");
      input(pidInput(harness.root), "4242");
      await harness.settle();
      expect(renderedOrder(harness.root)).toEqual(["session-1"]);

      await harness.advance((snapshot) => {
        snapshot.groups[0].rootPid = 777;
        snapshot.groups[0].processes = [
          fakeProcess(777, "other.exe", { parentPid: null, depth: 0 }),
        ];
      });
      expect(renderedOrder(harness.root)).toEqual([]);
    } finally {
      harness.cleanup();
    }
  });

  it("19e: re-registers pins on a direction change and keeps them when the metric moves", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      selectOption(sortField(harness.root), "cpu");
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual([
          "session-2",
          "session-1",
          "session-3",
        ]);
      });
      click(must(harness.root, "resourceMonitor.group.session-1.toggle"));
      click(must(harness.root, "resourceMonitor.group.session-2.toggle"));

      click(sortDirection(harness.root));
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual([
          "session-1",
          "session-2",
          "session-3",
        ]);
      });

      await harness.advance((snapshot) => {
        snapshot.groups[0].cpuPercent = 30;
        snapshot.groups[1].cpuPercent = 10;
      });
      expect(renderedOrder(harness.root)).toEqual([
        "session-1",
        "session-2",
        "session-3",
      ]);
    } finally {
      harness.cleanup();
    }
  });

  it("20: the Processes tile sums all groups and follows the tile order", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      const tile = must(harness.root, "resourceMonitor.summary.processCount");
      expect(tile.querySelector("strong")?.textContent).toBe("8");

      const tileIds = Array.from(
        must(harness.root, "resourceMonitor.summary").children
      ).map((child) => child.getAttribute("data-ac-testid"));
      expect(tileIds).toEqual([
        "resourceMonitor.summary.state",
        "resourceMonitor.summary.activeGroups",
        "resourceMonitor.summary.processCount",
        "resourceMonitor.summary.appPrivateBytes",
        "resourceMonitor.summary.network",
      ]);

      click(must(harness.root, "resourceMonitor.filter.status.inactive"));
      await waitFor(() => {
        expect(renderedOrder(harness.root)).toEqual(["session-3"]);
      });
      expect(
        must(
          harness.root,
          "resourceMonitor.summary.processCount"
        ).querySelector("strong")?.textContent
      ).toBe("8");
    } finally {
      harness.cleanup();
    }
  });

  it("21: indents the process tree by depth with no calc", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      await harness.advance((snapshot) => {
        snapshot.groups[0].processes = [
          fakeProcess(100, "node.exe", { parentPid: null, depth: 2 }),
          fakeProcess(101, "deep.exe", { parentPid: 100, depth: 9 }),
          fakeProcess(102, "flat.exe", { parentPid: 100, depth: 0 }),
        ];
      });
      input(pidInput(harness.root), "100");
      await harness.settle();
      await waitFor(() => {
        expect(
          must(
            harness.root,
            "resourceMonitor.group.session-1.process.100.name"
          )
        ).not.toBeNull();
      });
      expect(
        must(
          harness.root,
          "resourceMonitor.group.session-1.process.100.name"
        ).style.paddingLeft
      ).toBe("24px");
      expect(
        must(
          harness.root,
          "resourceMonitor.group.session-1.process.101.name"
        ).style.paddingLeft
      ).toBe("72px");
      expect(
        must(
          harness.root,
          "resourceMonitor.group.session-1.process.102.name"
        ).style.paddingLeft
      ).toBe("0px");
    } finally {
      harness.cleanup();
    }
  });

  it("22: the counter's process count equals the pid-match rows", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      input(pidInput(harness.root), "100, 4242");
      await harness.settle();
      const match = / - (\d+) matching/.exec(countText(harness.root));
      expect(match).not.toBeNull();
      const matched = Number(match![1]);
      expect(
        harness.root.querySelectorAll('[data-ac-state="pid-match"]').length
      ).toBe(matched);
    } finally {
      harness.cleanup();
    }
  });

  it("23: focuses Cancel, labels the dialog and traps Tab from both ends", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      click(must(harness.root, "resourceMonitor.group.session-1.kill"));
      await waitFor(() => {
        expect(must(harness.root, "resourceMonitor.killConfirm.cancel")).toBe(
          document.activeElement
        );
      });
      const dialog = must(harness.root, "resourceMonitor.killConfirm.dialog");
      expect(dialog.getAttribute("role")).toBe("dialog");
      expect(dialog.getAttribute("aria-modal")).toBe("true");
      expect(dialog.getAttribute("tabindex")).toBe("-1");
      const labelId = dialog.getAttribute("aria-labelledby");
      expect(labelId).toBeTruthy();
      expect(dialog.querySelector(`#${labelId}`)?.tagName).toBe("H2");

      const confirm = must(harness.root, "resourceMonitor.killConfirm.confirm");
      confirm.focus();
      const tab = keydown(confirm, "Tab");
      expect(tab.defaultPrevented).toBe(true);
      expect(document.activeElement).toBe(
        must(harness.root, "resourceMonitor.killConfirm.cancel")
      );

      const shiftTab = keydown(document.activeElement!, "Tab", true);
      expect(shiftTab.defaultPrevented).toBe(true);
      expect(document.activeElement).toBe(confirm);
    } finally {
      harness.cleanup();
    }
  });

  it("23b: keeps focus and both Tab directions inside the in-flight dialog", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      let resolveKill: ((value: unknown) => void) | undefined;
      harness.fake.onInvoke(
        "kill_resource_group",
        () =>
          new Promise((resolve) => {
            resolveKill = resolve;
          })
      );
      await waitFor(() => {
        expect(resourceMonitorStore.loading).toBe(false);
      });

      click(must(harness.root, "resourceMonitor.group.session-1.kill"));
      await waitFor(() => {
        expect(
          must(harness.root, "resourceMonitor.killConfirm.confirm")
        ).not.toBeNull();
      });
      click(must(harness.root, "resourceMonitor.killConfirm.confirm"));
      await waitFor(() => {
        expect(document.activeElement).toBe(
          must(harness.root, "resourceMonitor.killConfirm.dialog")
        );
      });
      const dialog = must(harness.root, "resourceMonitor.killConfirm.dialog");

      const escape = keydown(dialog, "Escape");
      expect(escape.defaultPrevented).toBe(true);
      expect(
        must(harness.root, "resourceMonitor.killConfirm")
      ).not.toBeNull();
      expect(document.activeElement).toBe(dialog);

      const tab = keydown(dialog, "Tab");
      expect(tab.defaultPrevented).toBe(true);
      expect(document.activeElement).toBe(dialog);
      const shiftTab = keydown(dialog, "Tab", true);
      expect(shiftTab.defaultPrevented).toBe(true);
      expect(document.activeElement).toBe(dialog);

      resolveKill?.({
        sessionId: "session-1",
        state: "terminated",
        quarantined: false,
        message: "resource group terminated and verified",
        blockedBySecurity: false,
        finalized: true,
      });
      await waitFor(() => {
        expect(
          harness.root.querySelector(
            '[data-ac-testid="resourceMonitor.killConfirm"]'
          )
        ).toBeNull();
      });
    } finally {
      harness.cleanup();
    }
  });

  it("23c: restores focus to the heading after a finalized kill removes the row", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      await waitFor(() => {
        expect(resourceMonitorStore.loading).toBe(false);
      });
      resourceMonitorStore.stopPolling();
      const before = harness.fake.calls.filter(
        (call) => call.cmd === "get_resource_snapshot"
      ).length;
      harness.fake.resolve("kill_resource_group", {
        sessionId: "session-1",
        state: "terminated",
        quarantined: false,
        message: "resource group terminated and verified",
        blockedBySecurity: false,
        finalized: true,
      });

      click(must(harness.root, "resourceMonitor.group.session-1.kill"));
      await waitFor(() => {
        expect(
          must(harness.root, "resourceMonitor.killConfirm.cancel")
        ).not.toBeNull();
      });
      harness.setSnapshot(fakeSnapshot([bravoGroup(), charlieGroup()]));
      click(must(harness.root, "resourceMonitor.killConfirm.confirm"));

      await waitFor(() => {
        expect(document.activeElement).toBe(
          must(harness.root, "resourceMonitor.groups.heading")
        );
      });
      expect(document.activeElement).not.toBe(document.body);
      const after = harness.fake.calls.filter(
        (call) => call.cmd === "get_resource_snapshot"
      ).length;
      expect(after).toBe(before + 1);
    } finally {
      harness.cleanup();
    }
  });

  it("23d: returns focus to the re-queried row after a poll replaced it", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      click(must(harness.root, "resourceMonitor.group.session-1.kill"));
      await waitFor(() => {
        expect(
          must(harness.root, "resourceMonitor.killConfirm.cancel")
        ).not.toBeNull();
      });
      const rowBefore = must(harness.root, "resourceMonitor.group.session-1");
      await harness.advance();
      expect(must(harness.root, "resourceMonitor.group.session-1")).not.toBe(
        rowBefore
      );

      keydown(must(harness.root, "resourceMonitor.killConfirm.dialog"), "Escape");
      await waitFor(() => {
        expect(document.activeElement).toBe(
          must(harness.root, "resourceMonitor.group.session-1.kill")
        );
      });
      expect(document.activeElement).not.toBe(document.body);
    } finally {
      harness.cleanup();
    }
  });

  it("24: disables both filter inputs when monitoring is off", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      await harness.advance((snapshot) => {
        snapshot.monitorEnabled = false;
      });
      await waitFor(() => {
        expect(pidInput(harness.root).disabled).toBe(true);
      });
      expect(searchInput(harness.root).disabled).toBe(true);
    } finally {
      harness.cleanup();
    }
  });

  it("25: applies the PID filter to the retained snapshot after a failed refresh", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      input(pidInput(harness.root), "4242");
      await harness.settle();
      expect(renderedOrder(harness.root)).toEqual(["session-1"]);

      harness.fake.onInvoke("get_resource_snapshot", () => {
        throw new Error("snapshot boom");
      });
      await harness.advance();
      await waitFor(() => {
        expect(resourceMonitorStore.error).toBeTruthy();
      });
      expect(renderedOrder(harness.root)).toEqual(["session-1"]);
      expect(countText(harness.root)).toBe(
        "Showing 1 of 3 agents - 1 matching process"
      );
    } finally {
      harness.cleanup();
    }
  });

  it("25b: the group working set is visible and one automation metric remains", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      const workingSet = must(
        harness.root,
        "resourceMonitor.group.session-1.workingSetBytes"
      );
      expect(workingSet.className).not.toContain("rm-automation-metric");

      const hidden = harness.root.querySelectorAll(".rm-automation-metric");
      expect(hidden).toHaveLength(1);
      expect(hidden[0].getAttribute("data-ac-testid")).toBe(
        "resourceMonitor.summary.appWorkingSetBytes"
      );
    } finally {
      harness.cleanup();
    }
  });

  it("25c: every group row renders all five metric cells", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      for (const sessionId of ["session-1", "session-2", "session-3"]) {
        for (const cell of [
          "processCount",
          "privateBytes",
          "workingSetBytes",
          "cpu",
          "network",
        ]) {
          expect(
            must(harness.root, `resourceMonitor.group.${sessionId}.${cell}`)
          ).not.toBeNull();
        }
      }
    } finally {
      harness.cleanup();
    }
  });

  it("25d: the process header keeps exactly six cells in order", async () => {
    const harness = createHarness(baseSnapshot());
    try {
      await waitForRows(harness, 3);
      click(must(harness.root, "resourceMonitor.group.session-1.toggle"));
      await waitFor(() => {
        expect(
          must(
            harness.root,
            "resourceMonitor.group.session-1.processList"
          ).querySelector(".rm-process-header")
        ).not.toBeNull();
      });
      const header = must(
        harness.root,
        "resourceMonitor.group.session-1.processList"
      ).querySelector(".rm-process-header") as HTMLElement;
      expect(
        Array.from(header.children).map((cell) => cell.textContent)
      ).toEqual(["Process", "PID", "Private", "Working Set", "CPU", "Kill Scope"]);
    } finally {
      harness.cleanup();
    }
  });
});
