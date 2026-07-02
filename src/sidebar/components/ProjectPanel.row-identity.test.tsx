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
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { replicaVolatileStore } from "../stores/replica-volatile";
import { settingsStore } from "../../shared/stores/settings";
import { clockStore } from "../stores/clock";
import { automationIdPart } from "./replica-repo-badges";

// #748 — the lost-first-click root cause: volatile events (coordinator clock,
// close markers, repo branch) used to replace every project object reference,
// so the reference-keyed <For>s disposed and re-created the whole clickable
// panel DOM. A click whose press straddled that swap never fired (Chromium
// drops `click` when the mousedown target is detached, w3c/uievents#141).
// These tests pin the fix at the DOM level: after each volatile event the row
// must be the SAME element (identity stable ⇒ no click can be lost to it),
// while the badge/pill content still updates in place. An actually-changed
// discovery reload must still re-create the row (real data changes render).

const projectPath = "C:\\Project";
const workgroupName = "wg-2-dev-team";
const workgroupPath = `${projectPath}\\.ac\\${workgroupName}`;
const coordName = "dev-webpage-ui";
const coordPath = `${workgroupPath}\\__agent_${coordName}`;

const rowSelector = `[data-ac-testid="replica.row.quick.${automationIdPart(
  workgroupName,
)}.${automationIdPart(coordName)}"]`;

function isoMinutesAgo(minutes: number): string {
  return new Date(clockStore.nowMs - minutes * 60_000).toISOString();
}

function coordDiscovery(taskTitle = "Row identity") {
  return discovery({
    workgroups: [
      {
        name: workgroupName,
        path: workgroupPath,
        task: null,
        taskTitle,
        agents: [
          {
            name: coordName,
            path: coordPath,
            repoPaths: [],
            isCoordinator: true,
            lastUserMessageAt: isoMinutesAgo(90),
          },
        ],
      },
    ],
  });
}

describe("ProjectPanel row identity across volatile events (#748)", () => {
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

  it("keeps the same row DOM node across clock/close/branch events while the badge updates in place", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("get_settings", baseSettings({ coordinatorAutoCloseEnabled: true }));
    fake.resolve("discover_project", coordDiscovery());

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await settingsStore.load();
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.querySelector(rowSelector)).toBeTruthy());

      const row = rendered.root.querySelector(rowSelector);
      expect(rendered.root.querySelector(".coord-idle")?.textContent).toBe("90m");

      // Clock event → badge text moves, row node does not.
      replicaVolatileStore.setLastUserMessageAt(coordPath, isoMinutesAgo(0));
      await waitFor(() =>
        expect(rendered.root.querySelector(".coord-idle")?.textContent).toBe("0m"),
      );
      expect(rendered.root.querySelector(rowSelector)).toBe(row);

      // Close-marker event → pill appears (XOR gates the counter off), same row.
      replicaVolatileStore.setAutoClosedAt(coordPath, isoMinutesAgo(1));
      await waitFor(() =>
        expect(rendered.root.querySelector(".coord-autoclosed")).not.toBeNull(),
      );
      expect(rendered.root.querySelector(rowSelector)).toBe(row);

      // Reopen clear + branch event → still the same node.
      replicaVolatileStore.setAutoClosedAt(coordPath, null);
      replicaVolatileStore.setRepoBranch(coordPath, "feature/x");
      await waitFor(() =>
        expect(rendered.root.querySelector(".coord-autoclosed")).toBeNull(),
      );
      expect(rendered.root.querySelector(rowSelector)).toBe(row);
      expect(row?.isConnected).toBe(true);
    } finally {
      rendered.cleanup();
    }
  });

  it("an identical discovery reload keeps the row node; a changed one re-creates it", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("get_settings", baseSettings());
    const firstSnapshot = coordDiscovery();
    fake.resolve("discover_project", firstSnapshot);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await settingsStore.load();
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.querySelector(rowSelector)).toBeTruthy());
      const row = rendered.root.querySelector(rowSelector);

      // Identical snapshot → complete no-op. Serve a deep CLONE so the merge
      // sees fresh objects with equal data: FakeTransport re-serves the same
      // reference on every invoke, and without the clone the no-op verdict
      // would ride deepEqual's `a === b` fast-path instead of proving real
      // structural comparison.
      fake.resolve("discover_project", structuredClone(firstSnapshot));
      await projectStore.reloadProject(projectPath);
      await waitFor(() =>
        expect(fake.callsFor("discover_project").length).toBeGreaterThanOrEqual(2),
      );
      expect(rendered.root.querySelector(rowSelector)).toBe(row);

      // Changed workgroup data → the row legitimately re-renders.
      fake.resolve("discover_project", coordDiscovery("Row identity v2"));
      await projectStore.reloadProject(projectPath);
      await waitFor(() =>
        expect(rendered.root.textContent).toContain("Row identity v2"),
      );
      expect(rendered.root.querySelector(rowSelector)).not.toBe(row);
    } finally {
      rendered.cleanup();
    }
  });
});
