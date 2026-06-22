// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import { settingsStore } from "../../shared/stores/settings";
import { clockStore } from "../stores/clock";
import type { AcAgentReplica } from "../../shared/types";

// #588 — the MANUALLY-CLOSED pill mirrors AUTO-CLOSED (same `coord-autoclosed`
// class, only the label differs), shows ONLY on a manually-closed *dormant*
// coordinator, wins the XOR over both the idle badge and AUTO-CLOSED, and — like
// #589's AUTO-CLOSED gate — hides the instant the coordinator has a live session
// again (the `!isSessionLive(session())` gate, the stale-on-raise guard).

const projectPath = "C:\\Project";
const workgroupPath = `${projectPath}\\.ac\\wg-2-dev-team`;
const coordPath = `${workgroupPath}\\__agent_dev-webpage-ui`;
const coordSessionName = "wg-2-dev-team/dev-webpage-ui";

/** RFC3339 anchor `minutes` before the badge clock's "now" (deterministic). */
function isoMinutesAgo(minutes: number): string {
  return new Date(clockStore.nowMs - minutes * 60_000).toISOString();
}

/** Discovery with a single coordinator replica carrying the given fields. */
function coordDiscovery(coord: Partial<AcAgentReplica>) {
  return discovery({
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: workgroupPath,
        task: null,
        taskTitle: "Manual close",
        agents: [
          {
            name: "dev-webpage-ui",
            path: coordPath,
            repoPaths: [],
            isCoordinator: true,
            ...coord,
          },
        ],
      },
    ],
  });
}

/** All pills that reuse the shared coord-autoclosed style (AUTO/MANUALLY). */
function closedPills(root: HTMLElement): Element[] {
  return Array.from(root.querySelectorAll(".coord-autoclosed"));
}

describe("ProjectPanel MANUALLY-CLOSED pill (#588)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("renders the MANUALLY-CLOSED pill and suppresses the idle counter (XOR) for a dormant coordinator", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("get_settings", baseSettings({ coordinatorAutoCloseEnabled: true }));
    // 90-min idle anchor AND a manual-close marker, NO live session: the manual
    // pill must show and the minutes counter must be gated off.
    fake.resolve(
      "discover_project",
      coordDiscovery({ lastUserMessageAt: isoMinutesAgo(90), manuallyClosedAt: isoMinutesAgo(2) })
    );

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await settingsStore.load();
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      // The coordinator is rendered in both the pinned coordinators overview and
      // the workgroup replica list, so the pill legitimately appears more than
      // once; assert EVERY closed-pill is the manual one (the AUTO-CLOSED label
      // never leaks through).
      await waitFor(() => {
        const pills = closedPills(rendered.root);
        expect(pills.length).toBeGreaterThan(0);
        expect(pills.every((p) => p.textContent === "MANUALLY-CLOSED")).toBe(true);
      });
      // XOR: the idle minutes counter is suppressed while the manual marker is set.
      expect(rendered.root.querySelector(".coord-idle")).toBeNull();
      expect(rendered.root.textContent).not.toContain("90m");
    } finally {
      rendered.cleanup();
    }
  });

  it("lets MANUALLY-CLOSED win when both markers are set (no AUTO-CLOSED, no double pill)", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("get_settings", baseSettings({ coordinatorAutoCloseEnabled: true }));
    fake.resolve(
      "discover_project",
      coordDiscovery({ autoClosedAt: isoMinutesAgo(5), manuallyClosedAt: isoMinutesAgo(2) })
    );

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await settingsStore.load();
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      await waitFor(() => {
        const pills = closedPills(rendered.root);
        expect(pills.length).toBeGreaterThan(0);
        expect(pills.every((p) => p.textContent === "MANUALLY-CLOSED")).toBe(true);
      });
      // Manual wins the XOR: the AUTO-CLOSED label is suppressed everywhere.
      expect(rendered.root.textContent).not.toContain("AUTO-CLOSED");
    } finally {
      rendered.cleanup();
    }
  });

  it("suppresses the pill the moment the coordinator has a live session again (#589-parity !isSessionLive gate)", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("get_settings", baseSettings({ coordinatorAutoCloseEnabled: true }));
    fake.resolve("discover_project", coordDiscovery({ manuallyClosedAt: isoMinutesAgo(2) }));

    // A live (running) session matching the replica name -> isSessionLive() true,
    // so the manual pill must NOT render even though the marker is present. This
    // is the exact stale-on-raise trap #589 fixes for AUTO-CLOSED.
    sessionsStore.setSessions([
      session({ id: "live-coord", name: coordSessionName, isCoordinator: true, status: "running" }),
    ]);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await settingsStore.load();
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      // Give any async badge patching a beat, then assert the pill stayed hidden.
      await waitFor(() => expect(rendered.root.textContent).not.toContain("MANUALLY-CLOSED"));
      expect(closedPills(rendered.root)).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });
});
