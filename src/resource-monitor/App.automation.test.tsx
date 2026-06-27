// @vitest-environment jsdom
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

const activeSnapshot = (): ResourceSnapshot => ({
  capturedAt: "2026-06-15T01:00:00.000Z",
  overallState: "warn",
  monitorEnabled: true,
  activeAgentGroups: 1,
  maxConcurrentAgentGroups: 1,
  appPrivateBytes: 2 * 1024 ** 3,
  appWorkingSetBytes: 3 * 1024 ** 3,
  networkState: "unknown",
  networkSummary: "Unknown",
  warnings: ["Resource Monitor cap reached"],
  groups: [
    {
      sessionId: "session-1",
      name: "cap-one",
      workgroup: "wg-5-dev-team",
      agent: "dev-rust",
      project: "AgentsCommander_ac",
      rootPid: 100,
      state: "running",
      descendantsObserved: true,
      processCount: 1,
      privateBytes: 512 * 1024 ** 2,
      workingSetBytes: 768 * 1024 ** 2,
      cpuPercent: 1.2,
      networkState: "unknown",
      networkSummary: "Unknown",
      killAllowed: true,
      processes: [
        {
          pid: 4242,
          name: "powershell.exe",
          privateBytes: 256 * 1024 ** 2,
          workingSetBytes: 300 * 1024 ** 2,
          cpuPercent: 0.4,
          killAllowed: true,
        },
      ],
    },
  ],
});

const emptySnapshot = (): ResourceSnapshot => ({
  capturedAt: "2026-06-15T01:00:05.000Z",
  overallState: "ok",
  monitorEnabled: true,
  activeAgentGroups: 0,
  maxConcurrentAgentGroups: 1,
  appPrivateBytes: 1024 ** 3,
  appWorkingSetBytes: 2 * 1024 ** 3,
  networkState: "observed",
  networkSummary: "Observed",
  warnings: [],
  groups: [],
});

const nullIdentitySnapshot = (): ResourceSnapshot => {
  const snapshot = activeSnapshot();
  snapshot.groups[0].workgroup = null;
  snapshot.groups[0].agent = null;
  snapshot.groups[0].project = null;
  return snapshot;
};

// #647 C/D: a quarantined group — the kill was not verified dead (an AV is
// stripping PROCESS_TERMINATE). It must stay killable (Force-kill) and carry the
// per-PID detail in lastError.
const quarantinedSnapshot = (): ResourceSnapshot => {
  const snapshot = activeSnapshot();
  snapshot.overallState = "critical";
  snapshot.groups[0].state = "quarantined";
  snapshot.groups[0].lastError =
    "resource group cleanup incomplete: pid 4242: win32 error 5";
  return snapshot;
};

// #647 C: a group the backend is actively terminating (a kill in flight). Its row
// shows the "Verifying..." indicator and its kill button is disabled.
const terminatingSnapshot = (): ResourceSnapshot => {
  const snapshot = activeSnapshot();
  snapshot.overallState = "enforcing";
  snapshot.groups[0].state = "terminating";
  return snapshot;
};

// #566 - multiple groups spanning projects / workgroups / roles / states so the
// filter dimensions visibly change the rendered row count.
const multiGroupSnapshot = (): ResourceSnapshot => {
  const snapshot = activeSnapshot();
  snapshot.activeAgentGroups = 2;
  snapshot.maxConcurrentAgentGroups = 3;
  snapshot.groups = [
    {
      ...snapshot.groups[0],
      sessionId: "session-1",
      name: "cap-one",
      workgroup: "wg-5-dev-team",
      agent: "dev-rust",
      project: "AgentsCommander_ac",
      state: "running",
    },
    {
      ...snapshot.groups[0],
      sessionId: "session-2",
      name: "cap-two",
      workgroup: "wg-9-other-team",
      agent: "tech-lead",
      project: "OtherProject_ac",
      state: "running",
    },
    {
      ...snapshot.groups[0],
      sessionId: "session-3",
      name: "cap-dead",
      workgroup: "wg-5-dev-team",
      agent: "shipper",
      project: "AgentsCommander_ac",
      state: "terminated",
    },
  ];
  return snapshot;
};

const groupRowCount = (root: ParentNode): number =>
  root.querySelectorAll(".rm-group-row").length;

function setupResourceMonitor(fake: FakeTransport, snapshot: ResourceSnapshot): void {
  fake.resolve("get_settings", baseSettings());
  fake.onInvoke("get_resource_snapshot", () => snapshot);
  fake.resolve("kill_resource_group", {
    sessionId: "session-1",
    state: "terminated",
    quarantined: false,
    message: "resource group terminated and verified",
    blockedBySecurity: false,
    finalized: true,
  });
}

describe("ResourceMonitorApp automation hooks", () => {
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

  it("exposes stable summary, group, process, warning, and kill-cancel selectors", async () => {
    const fake = new FakeTransport();
    setupResourceMonitor(fake, activeSnapshot());

    const rendered = renderWithFakeTransport(() => <ResourceMonitorApp />, fake);
    try {
      await waitFor(() => {
        expect(
          rendered.root.querySelector('[data-ac-testid="resourceMonitor.summary.activeGroups.count"]')
            ?.textContent
        ).toBe("1");
      });

      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.summary.network"]')
          ?.getAttribute("data-ac-state")
      ).toBe("unknown");
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.summary.activeGroups.count"]')
          ?.textContent
      ).toBe("1");
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.summary.activeGroups.limit"]')
          ?.textContent
      ).toBe("1");
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.summary.appPrivateBytes"]')
          ?.textContent
      ).toContain("2.0 GB");
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.summary.appWorkingSetBytes"]')
          ?.textContent
      ).toContain("3.0 GB");
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.summary.timestamp"]')
      ).not.toBeNull();
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.warning.0"]')?.textContent
      ).toContain("cap reached");

      expect(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.group.session-1.origin"]'
        )?.textContent
      ).toBe("wg-5-dev-team / dev-rust");

      const toggle = rendered.root.querySelector(
        '[data-ac-testid="resourceMonitor.group.session-1.toggle"]'
      );
      expect(toggle).not.toBeNull();
      click(toggle!);

      await waitFor(() => {
        expect(
          rendered.root.querySelector(
            '[data-ac-testid="resourceMonitor.group.session-1.process.4242"]'
          )
        ).not.toBeNull();
      });

      expect(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.group.session-1.processCount"]'
        )?.textContent
      ).toContain("1 proc");
      expect(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.group.session-1.process.4242.killAllowed"]'
        )?.getAttribute("data-ac-state")
      ).toBe("allowed");

      click(rendered.root.querySelector('[data-ac-testid="resourceMonitor.group.session-1.kill"]')!);
      await waitFor(() => {
        expect(
          rendered.root.querySelector('[data-ac-testid="resourceMonitor.killConfirm.cancel"]')
        ).not.toBeNull();
      });

      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.killConfirm.origin"]')
          ?.textContent
      ).toBe("wg-5-dev-team / dev-rust");
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.killConfirm.name"]')
          ?.textContent
      ).toBe("cap-one");

      click(rendered.root.querySelector('[data-ac-testid="resourceMonitor.killConfirm.cancel"]')!);
      await waitFor(() => {
        expect(
          rendered.root.querySelector('[data-ac-testid="resourceMonitor.killConfirm"]')
        ).toBeNull();
      });
    } finally {
      rendered.cleanup();
    }
  });

  // #587 — embedded mode (mounted in MainApp's central pane) drops the window
  // titlebar and exposes a Detach control instead.
  it("in embedded mode hides the window titlebar and shows a Detach button", async () => {
    const fake = new FakeTransport();
    setupResourceMonitor(fake, activeSnapshot());

    const rendered = renderWithFakeTransport(
      () => <ResourceMonitorApp embedded />,
      fake
    );
    try {
      await waitFor(() => {
        expect(
          rendered.root.querySelector('[data-ac-testid="resourceMonitor.window"]')
        ).not.toBeNull();
      });

      // No window titlebar (main owns the window chrome).
      expect(rendered.root.querySelector(".rm-titlebar")).toBeNull();
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.titlebar.attach"]')
      ).toBeNull();
      // Detach button present; the body + root still render.
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.detach"]')
      ).not.toBeNull();
      expect(rendered.root.querySelector(".rm-root")).not.toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  // #587 — windowed (default) mode keeps the window titlebar and shows no
  // Detach button. (The titlebar's controls — including the renamed Attach
  // button — are gated behind isTauri, which is false under jsdom, so only the
  // titlebar shell is observable here; the Detach-vs-titlebar split is the
  // meaningful embedded/windowed distinction.)
  it("in windowed mode keeps the titlebar and shows no Detach button", async () => {
    const fake = new FakeTransport();
    setupResourceMonitor(fake, activeSnapshot());

    const rendered = renderWithFakeTransport(() => <ResourceMonitorApp />, fake);
    try {
      await waitFor(() => {
        expect(
          rendered.root.querySelector('[data-ac-testid="resourceMonitor.window"]')
        ).not.toBeNull();
      });

      expect(rendered.root.querySelector(".rm-titlebar")).not.toBeNull();
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.detach"]')
      ).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("exposes a stable empty-state selector when no agents are active", async () => {
    const fake = new FakeTransport();
    setupResourceMonitor(fake, emptySnapshot());

    const rendered = renderWithFakeTransport(() => <ResourceMonitorApp />, fake);
    try {
      await waitFor(() => {
        const empty = rendered.root.querySelector('[data-ac-testid="resourceMonitor.empty"]');
        expect(empty?.getAttribute("data-ac-state")).toBe("empty");
        expect(empty?.textContent).toContain("No active agents");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("falls back to the group name in the origin label when WG/agent are null", async () => {
    const fake = new FakeTransport();
    setupResourceMonitor(fake, nullIdentitySnapshot());

    const rendered = renderWithFakeTransport(() => <ResourceMonitorApp />, fake);
    try {
      await waitFor(() => {
        expect(
          rendered.root.querySelector(
            '[data-ac-testid="resourceMonitor.group.session-1.origin"]'
          )?.textContent
        ).toBe("- / cap-one");
      });

      click(rendered.root.querySelector('[data-ac-testid="resourceMonitor.group.session-1.kill"]')!);
      await waitFor(() => {
        expect(
          rendered.root.querySelector('[data-ac-testid="resourceMonitor.killConfirm.origin"]')
            ?.textContent
        ).toBe("- / cap-one");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("filters rows by status, workgroup, and project, and re-runs when a chip is toggled off (G3)", async () => {
    const fake = new FakeTransport();
    setupResourceMonitor(fake, multiGroupSnapshot());

    const rendered = renderWithFakeTransport(() => <ResourceMonitorApp />, fake);
    try {
      // All three rows visible once the snapshot loads.
      await waitFor(() => {
        expect(groupRowCount(rendered.root)).toBe(3);
      });

      // Status: Active hides the single terminated row.
      click(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.filter.status.active"]'
        )!
      );
      await waitFor(() => {
        expect(groupRowCount(rendered.root)).toBe(2);
      });
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.group.session-3"]')
      ).toBeNull();

      // Status: Inactive shows only the terminated row.
      click(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.filter.status.inactive"]'
        )!
      );
      await waitFor(() => {
        expect(groupRowCount(rendered.root)).toBe(1);
      });
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.group.session-3"]')
      ).not.toBeNull();

      // Reset status back to All.
      click(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.filter.status.all"]'
        )!
      );
      await waitFor(() => {
        expect(groupRowCount(rendered.root)).toBe(3);
      });

      // Workgroup chip (multi-select Set) narrows to its single row. This is the
      // G3 assertion: in-place Set mutation would leave the count unchanged here.
      click(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.filter.workgroup.wg-9-other-team"]'
        )!
      );
      await waitFor(() => {
        expect(groupRowCount(rendered.root)).toBe(1);
      });
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.group.session-2"]')
      ).not.toBeNull();

      // "Showing X of Y" indicator appears only while a filter is active.
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.filter.count"]')
          ?.textContent
      ).toContain("Showing 1 of 3");

      // Toggling the SAME chip off restores every row — proves the toggle yields
      // a fresh Set in both directions (the core G3 regression guard).
      click(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.filter.workgroup.wg-9-other-team"]'
        )!
      );
      await waitFor(() => {
        expect(groupRowCount(rendered.root)).toBe(3);
      });

      // Project chip narrows to the matching project (new #566 dimension).
      click(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.filter.project.OtherProject_ac"]'
        )!
      );
      await waitFor(() => {
        expect(groupRowCount(rendered.root)).toBe(1);
      });
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.group.session-2"]')
      ).not.toBeNull();

      // Clear filters resets everything and hides the indicator.
      click(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.filter.clear"]')!
      );
      await waitFor(() => {
        expect(groupRowCount(rendered.root)).toBe(3);
      });
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.filter.count"]')
      ).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("shows a distinct filtered-empty state when filters exclude every row", async () => {
    const fake = new FakeTransport();
    setupResourceMonitor(fake, multiGroupSnapshot());

    const rendered = renderWithFakeTransport(() => <ResourceMonitorApp />, fake);
    try {
      await waitFor(() => {
        expect(groupRowCount(rendered.root)).toBe(3);
      });

      // Inactive + a workgroup with no terminated row => zero matches, but the
      // snapshot still has rows, so this is filtered-empty, not truly-empty.
      click(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.filter.status.inactive"]'
        )!
      );
      click(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.filter.workgroup.wg-9-other-team"]'
        )!
      );

      await waitFor(() => {
        const empty = rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.empty"]'
        );
        expect(empty?.getAttribute("data-ac-state")).toBe("filtered-empty");
        expect(empty?.textContent).toContain("No agents match the filters");
      });
      expect(groupRowCount(rendered.root)).toBe(0);
    } finally {
      rendered.cleanup();
    }
  });

  // #566 N1 - regression guard: with a snapshot already loaded but the active
  // filter matching zero rows, an in-flight poll (loading=true, kept snapshot)
  // must NOT flash the "loading" empty-state over the stable filtered-empty one.
  it("keeps the filtered-empty state during an in-flight poll instead of flashing loading (N1)", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.resolve("kill_resource_group", {
      sessionId: "session-1",
      state: "terminated",
      quarantined: false,
      message: "resource group terminated and verified",
      blockedBySecurity: false,
      finalized: true,
    });
    // A controllable snapshot handler: returns rows normally, but once `hang` is
    // set the next call stays pending so the store's `loading` flag stays true.
    let hang = false;
    const pending: { resolve: (snapshot: ResourceSnapshot) => void } = {
      resolve: () => {},
    };
    fake.onInvoke("get_resource_snapshot", () => {
      if (!hang) return multiGroupSnapshot();
      return new Promise<ResourceSnapshot>((resolve) => {
        pending.resolve = resolve;
      });
    });

    const rendered = renderWithFakeTransport(() => <ResourceMonitorApp />, fake);
    try {
      await waitFor(() => {
        expect(groupRowCount(rendered.root)).toBe(3);
      });

      // Exclude every row: Inactive + a workgroup that has no terminated row.
      click(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.filter.status.inactive"]'
        )!
      );
      click(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.filter.workgroup.wg-9-other-team"]'
        )!
      );
      await waitFor(() => {
        expect(
          rendered.root
            .querySelector('[data-ac-testid="resourceMonitor.empty"]')
            ?.getAttribute("data-ac-state")
        ).toBe("filtered-empty");
      });

      // Drive a poll in-flight while filtered-empty. Settle any prior poll first,
      // stop the timer so only our hanging refresh flips `loading`, then make the
      // next snapshot call hang so `loading` stays true deterministically.
      await waitFor(() => {
        expect(resourceMonitorStore.loading).toBe(false);
      });
      resourceMonitorStore.stopPolling();
      hang = true;
      void resourceMonitorStore.refresh();

      await waitFor(() => {
        expect(resourceMonitorStore.loading).toBe(true);
        const empty = rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.empty"]'
        );
        // The N1 assertion: still filtered-empty, NOT "loading".
        expect(empty?.getAttribute("data-ac-state")).toBe("filtered-empty");
        expect(empty?.textContent).toContain("No agents match the filters");
      });
    } finally {
      // Release the pending snapshot so the store settles back to loading=false
      // and does not leak an in-flight promise into later tests. cleanup() must
      // run even if the settle wait throws, so guard it in its own finally.
      pending.resolve(emptySnapshot());
      try {
        await waitFor(() => {
          expect(resourceMonitorStore.loading).toBe(false);
        });
      } finally {
        rendered.cleanup();
      }
    }
  });

  // #647 (test 6): a quarantined group is killable again (Force-kill), and a
  // non-finalized quarantined result keeps the confirm modal open showing the
  // per-PID detail AND the English security guidance (the latter ADDS to, never
  // replaces, the former). The narrowed "Verifying..." indicator does NOT show on
  // an idle quarantined row.
  it("exposes Force-kill on a quarantined group and surfaces the per-PID detail + security hint without auto-closing", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.onInvoke("get_resource_snapshot", () => quarantinedSnapshot());
    fake.resolve("kill_resource_group", {
      sessionId: "session-1",
      state: "quarantined",
      quarantined: true,
      message: "resource group cleanup incomplete: pid 4242: win32 error 5",
      blockedBySecurity: true,
      finalized: false,
    });

    const rendered = renderWithFakeTransport(() => <ResourceMonitorApp />, fake);
    try {
      // The quarantined row's kill button is enabled and reads "Force-kill".
      await waitFor(() => {
        const killBtn = rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.group.session-1.kill"]'
        ) as HTMLButtonElement | null;
        expect(killBtn).not.toBeNull();
        expect(killBtn!.disabled).toBe(false);
        expect(killBtn!.textContent).toContain("Force-kill");
      });

      // Narrowing: an idle quarantined row (no kill in flight) does NOT show the
      // "Verifying..." indicator.
      expect(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.group.session-1.verifying"]'
        )
      ).toBeNull();

      // Open the confirm modal and fire the force-kill.
      click(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.group.session-1.kill"]'
        )!
      );
      await waitFor(() => {
        expect(
          rendered.root.querySelector(
            '[data-ac-testid="resourceMonitor.killConfirm.confirm"]'
          )
        ).not.toBeNull();
      });
      click(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.killConfirm.confirm"]'
        )!
      );

      // The non-finalized quarantined result surfaces the per-PID message AND the
      // security hint, and the modal stays open (not auto-closed) for a Retry.
      await waitFor(() => {
        expect(
          rendered.root.querySelector(
            '[data-ac-testid="resourceMonitor.killConfirm.quarantined"]'
          )
        ).not.toBeNull();
        expect(
          rendered.root.querySelector(
            '[data-ac-testid="resourceMonitor.killConfirm.message"]'
          )?.textContent
        ).toContain("win32 error 5");
        expect(
          rendered.root.querySelector(
            '[data-ac-testid="resourceMonitor.killConfirm.securityHint"]'
          )?.textContent
        ).toContain("security software");
      });

      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.killConfirm"]')
      ).not.toBeNull();
      expect(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.killConfirm.confirm"]'
        )?.textContent
      ).toContain("Retry");
    } finally {
      rendered.cleanup();
    }
  });

  // #647 (Step 7): success keys off `finalized`. A finalized kill closes the modal
  // (the tile flips to Exited via the backend session_created emit, E5).
  it("closes the kill modal when the result is finalized (verified success)", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.onInvoke("get_resource_snapshot", () => activeSnapshot());
    fake.resolve("kill_resource_group", {
      sessionId: "session-1",
      state: "terminated",
      quarantined: false,
      message: "resource group terminated and verified",
      blockedBySecurity: false,
      finalized: true,
    });

    const rendered = renderWithFakeTransport(() => <ResourceMonitorApp />, fake);
    try {
      // Wait until THIS test's (running) snapshot has loaded — the store is a
      // module singleton, so a prior test's snapshot can linger for a tick.
      await waitFor(() => {
        const killBtn = rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.group.session-1.kill"]'
        ) as HTMLButtonElement | null;
        expect(killBtn).not.toBeNull();
        expect(killBtn!.disabled).toBe(false);
        expect(killBtn!.textContent?.trim()).toBe("Kill");
      });
      click(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.group.session-1.kill"]'
        )!
      );
      await waitFor(() => {
        expect(
          rendered.root.querySelector(
            '[data-ac-testid="resourceMonitor.killConfirm.confirm"]'
          )
        ).not.toBeNull();
      });
      click(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.killConfirm.confirm"]'
        )!
      );

      // Finalized => the modal closes as success.
      await waitFor(() => {
        expect(
          rendered.root.querySelector('[data-ac-testid="resourceMonitor.killConfirm"]')
        ).toBeNull();
      });
    } finally {
      rendered.cleanup();
    }
  });

  // #647 C: a group the backend is actively terminating shows the "Verifying..."
  // indicator and its kill button stays disabled (a kill is already in flight).
  it("shows the Verifying indicator and disables Kill on a terminating group", async () => {
    const fake = new FakeTransport();
    setupResourceMonitor(fake, terminatingSnapshot());

    const rendered = renderWithFakeTransport(() => <ResourceMonitorApp />, fake);
    try {
      await waitFor(() => {
        expect(
          rendered.root.querySelector(
            '[data-ac-testid="resourceMonitor.group.session-1.verifying"]'
          )
        ).not.toBeNull();
      });
      const killBtn = rendered.root.querySelector(
        '[data-ac-testid="resourceMonitor.group.session-1.kill"]'
      ) as HTMLButtonElement | null;
      expect(killBtn).not.toBeNull();
      expect(killBtn!.disabled).toBe(true);
    } finally {
      rendered.cleanup();
    }
  });

  // #647 (Step 7): a non-finalized `terminating` result (a concurrent kill still
  // settling) must keep the modal open with a "Verifying..." banner and a Retry,
  // NOT close as a false success — the zombie-tile race grinch found.
  it("keeps the modal open with a Verifying banner when the result is terminating and not finalized", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.onInvoke("get_resource_snapshot", () => quarantinedSnapshot());
    fake.resolve("kill_resource_group", {
      sessionId: "session-1",
      state: "terminating",
      quarantined: false,
      message: "resource group cleanup pending",
      blockedBySecurity: false,
      finalized: false,
    });

    const rendered = renderWithFakeTransport(() => <ResourceMonitorApp />, fake);
    try {
      // Wait until THIS test's quarantined snapshot has loaded (enabled Force-kill),
      // not a prior test's lingering snapshot (singleton store).
      await waitFor(() => {
        const killBtn = rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.group.session-1.kill"]'
        ) as HTMLButtonElement | null;
        expect(killBtn).not.toBeNull();
        expect(killBtn!.disabled).toBe(false);
        expect(killBtn!.textContent).toContain("Force-kill");
      });
      click(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.group.session-1.kill"]'
        )!
      );
      await waitFor(() => {
        expect(
          rendered.root.querySelector(
            '[data-ac-testid="resourceMonitor.killConfirm.confirm"]'
          )
        ).not.toBeNull();
      });
      click(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.killConfirm.confirm"]'
        )!
      );

      // NOT finalized => modal stays open with the Verifying banner + Retry.
      await waitFor(() => {
        expect(
          rendered.root.querySelector(
            '[data-ac-testid="resourceMonitor.killConfirm.verifying"]'
          )
        ).not.toBeNull();
      });
      expect(
        rendered.root.querySelector('[data-ac-testid="resourceMonitor.killConfirm"]')
      ).not.toBeNull();
      expect(
        rendered.root.querySelector(
          '[data-ac-testid="resourceMonitor.killConfirm.confirm"]'
        )?.textContent
      ).toContain("Retry");
    } finally {
      rendered.cleanup();
    }
  });
});
