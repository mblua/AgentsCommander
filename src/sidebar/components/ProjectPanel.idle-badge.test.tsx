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
import { replicaVolatileStore } from "../stores/replica-volatile";
import { sessionsStore } from "../stores/sessions";
import { settingsStore } from "../../shared/stores/settings";
import { clockStore } from "../stores/clock";
import type { AcAgentReplica } from "../../shared/types";

// #580 — the coordinator idle counter and the AUTO-CLOSED pill are mutually
// exclusive (the XOR gate, Section 7.1), and the tooltip's auto-close clause is
// conditional on the setting (Section 7.2). These render the real ProjectPanel
// through the FakeTransport harness and assert the DOM, per the
// frontend-visual-verification discipline.

const projectPath = "C:\\Project";
const workgroupPath = `${projectPath}\\.ac\\wg-2-dev-team`;
const coordPath = `${workgroupPath}\\__agent_dev-webpage-ui`;

const IDLE_TOOLTIP_BASE =
  "Time this team has been idle. Resets when you message the coordinator or any member is active (persists across restarts).";
const IDLE_TOOLTIP_AUTOCLOSE = "The team auto-closes at the configured limit.";

/** An RFC3339 anchor `minutes` before the badge clock's "now". Anchored to
 *  `clockStore.nowMs` (the exact value the badge memo reads) so the rendered
 *  minutes label is deterministic regardless of wall-clock at test time. */
function isoMinutesAgo(minutes: number): string {
  return new Date(clockStore.nowMs - minutes * 60_000).toISOString();
}

/** Discovery with a single coordinator replica carrying the given idle fields. */
function coordDiscovery(coord: Partial<AcAgentReplica>) {
  return discovery({
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: workgroupPath,
        task: null,
        taskTitle: "Idle badge",
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

describe("ProjectPanel coordinator idle badge (#580 XOR gate + conditional tooltip)", () => {
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

  it("shows only the AUTO-CLOSED pill when auto-closed, then restores the red 90m counter on reopen", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("get_settings", baseSettings({ coordinatorAutoCloseEnabled: true }));
    // The coordinator has BOTH a 90-min idle anchor AND an auto-closed marker:
    // the XOR gate must suppress the counter while the marker is present.
    fake.resolve(
      "discover_project",
      coordDiscovery({ lastUserMessageAt: isoMinutesAgo(90), autoClosedAt: isoMinutesAgo(5) })
    );

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await settingsStore.load();
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      // Auto-closed → the gray pill renders and the minutes counter is gated off.
      await waitFor(() => {
        expect(rendered.root.querySelector(".coord-autoclosed")).not.toBeNull();
        expect(rendered.root.querySelector(".coord-idle")).toBeNull();
      });
      expect(rendered.root.querySelector(".coord-autoclosed")?.textContent).toBe("AUTO-CLOSED");
      expect(rendered.root.textContent).not.toContain("90m");

      // Reopen (clear the marker, as the auto-close event does) → the pill
      // disappears and the red 90m counter returns from the same anchor.
      // #748: the clear lands in the volatile store; the pill memo must react
      // WITHOUT the row being re-created (row identity is stable now).
      replicaVolatileStore.setAutoClosedAt(coordPath, null);
      await waitFor(() => {
        expect(rendered.root.querySelector(".coord-autoclosed")).toBeNull();
        const counter = rendered.root.querySelector(".coord-idle");
        expect(counter).not.toBeNull();
        expect(counter?.classList.contains("red")).toBe(true);
        expect(counter?.textContent).toBe("90m");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("#589: hides the AUTO-CLOSED pill once the coordinator has a live session, restoring the counter — even with a stale autoClosedAt", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("get_settings", baseSettings({ coordinatorAutoCloseEnabled: true }));
    // Auto-closed marker set AND a 90-min idle anchor. Critically, the marker is
    // NEVER cleared in this test (it simulates a discovery reload clobbering the
    // event clear, the #589 root cause): the liveness gate alone must drive the
    // pill, proving the fix self-heals without the projectStore marker changing.
    fake.resolve(
      "discover_project",
      coordDiscovery({ lastUserMessageAt: isoMinutesAgo(90), autoClosedAt: isoMinutesAgo(5) })
    );

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await settingsStore.load();
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      // (b) autoClosedAt set AND no live session → the gray pill shows, counter gated off.
      await waitFor(() => {
        expect(rendered.root.querySelector(".coord-autoclosed")).not.toBeNull();
        expect(rendered.root.querySelector(".coord-idle")).toBeNull();
      });

      // Raise: a live session lands in the sessionsStore — the SAME signal the
      // status dot reads — while the projectStore autoClosedAt stays stale.
      sessionsStore.setSessions([
        session({
          id: "coord-live",
          name: "wg-2-dev-team/dev-webpage-ui",
          workingDirectory: coordPath,
          isCoordinator: true,
          status: "running",
        }),
      ]);

      // (a) autoClosedAt still set BUT the session is live → pill ABSENT, the red
      // 90m counter returns automatically via the XOR gate.
      await waitFor(() => {
        expect(rendered.root.querySelector(".coord-autoclosed")).toBeNull();
        const counter = rendered.root.querySelector(".coord-idle");
        expect(counter).not.toBeNull();
        expect(counter?.classList.contains("red")).toBe(true);
        expect(counter?.textContent).toBe("90m");
      });

      // The marker was never cleared — the liveness gate, not the marker, hid the
      // pill. This is the self-heal that survives a discovery clobber.
      const coord = projectStore.projects[0]?.workgroups[0]?.agents[0];
      expect(coord?.autoClosedAt).toBeTruthy();
    } finally {
      rendered.cleanup();
    }
  });

  it("appends the auto-close clause to the idle tooltip when coordinatorAutoCloseEnabled is true", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("get_settings", baseSettings({ coordinatorAutoCloseEnabled: true }));
    fake.resolve("discover_project", coordDiscovery({ lastUserMessageAt: isoMinutesAgo(90) }));

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await settingsStore.load();
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.querySelector(".coord-idle")).not.toBeNull());

      const title = rendered.root.querySelector(".coord-idle")?.getAttribute("title") ?? "";
      expect(title).toContain(IDLE_TOOLTIP_BASE);
      expect(title).toContain(IDLE_TOOLTIP_AUTOCLOSE);
    } finally {
      rendered.cleanup();
    }
  });

  it("omits the auto-close clause from the idle tooltip when coordinatorAutoCloseEnabled is false (badge still shown)", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    // Decision 3: the badge stays visible when auto-close is OFF, but it will NOT
    // close, so the tooltip must not assert that it auto-closes.
    fake.resolve("get_settings", baseSettings({ coordinatorAutoCloseEnabled: false }));
    fake.resolve("discover_project", coordDiscovery({ lastUserMessageAt: isoMinutesAgo(90) }));

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await settingsStore.load();
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.querySelector(".coord-idle")).not.toBeNull());

      const title = rendered.root.querySelector(".coord-idle")?.getAttribute("title") ?? "";
      expect(title).toContain(IDLE_TOOLTIP_BASE);
      expect(title).not.toContain("auto-closes");
    } finally {
      rendered.cleanup();
    }
  });
});
