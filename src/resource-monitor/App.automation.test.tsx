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
  return snapshot;
};

function setupResourceMonitor(fake: FakeTransport, snapshot: ResourceSnapshot): void {
  fake.resolve("get_settings", baseSettings());
  fake.onInvoke("get_resource_snapshot", () => snapshot);
  fake.resolve("kill_resource_group", {
    sessionId: "session-1",
    state: "terminating",
    quarantined: false,
    message: "terminating",
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

  it("exposes a stable empty-state selector when no agent groups are active", async () => {
    const fake = new FakeTransport();
    setupResourceMonitor(fake, emptySnapshot());

    const rendered = renderWithFakeTransport(() => <ResourceMonitorApp />, fake);
    try {
      await waitFor(() => {
        const empty = rendered.root.querySelector('[data-ac-testid="resourceMonitor.empty"]');
        expect(empty?.getAttribute("data-ac-state")).toBe("empty");
        expect(empty?.textContent).toContain("No active agent groups");
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
});
