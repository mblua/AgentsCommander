// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SidebarApp from "./App";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../shared/testing/ui-harness";
import { sessionsStore } from "./stores/sessions";
import type { Session } from "../shared/types";

const projectPath = "C:\\Project";
const wgName = "wg-2-dev-team";
const replicaName = "dev-webpage-ui";
const workgroupPath = `${projectPath}\\.ac\\${wgName}`;
const replicaPath = `${workgroupPath}\\__agent_${replicaName}`;
const sessionId = "coord-session";
const updatedAt = "2026-06-28T17:00:00.000Z";
const slotSelector =
  `[data-ac-testid="replica.row.quick.${wgName}.${replicaName}.communicationSlot"]`;

function coordSession(overrides: Partial<Session> = {}): Session {
  return session({
    id: sessionId,
    name: `${wgName}/${replicaName}`,
    workingDirectory: replicaPath,
    status: "running",
    isCoordinator: true,
    communication: null,
    ...overrides,
  });
}

function setupRaiseHandTransport(fake: FakeTransport, sessions: Session[]): void {
  fake.resolve(
    "get_settings",
    baseSettings({
      projectPaths: [projectPath],
      projectPath,
    })
  );
  fake.resolve("get_update_status", null);
  fake.resolve("open_project", {
    path: projectPath,
    registered: true,
    created: false,
  });
  fake.resolve(
    "discover_project",
    discovery({
      workgroups: [
        {
          name: wgName,
          path: workgroupPath,
          task: null,
          taskTitle: "Raise event",
          agents: [
            {
              name: replicaName,
              path: replicaPath,
              repoPaths: [],
              isCoordinator: true,
            },
          ],
        },
      ],
    })
  );
  fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
  fake.resolve("search_repos", []);
  fake.resolve("list_sessions", sessions);
  fake.resolve("get_active_session", null);
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
}

describe("SidebarApp raise-hand communication workflow (#676)", () => {
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

  it("updates and clears the coordinator quick-access slot from backend events", async () => {
    const fake = new FakeTransport();
    setupRaiseHandTransport(fake, [coordSession()]);

    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => {
        expect(fake.callsFor("list_sessions").length).toBeGreaterThan(0);
        expect(sessionsStore.sessions.find((s) => s.id === sessionId)).toBeTruthy();
      });
      expect(rendered.root.querySelector(slotSelector)).toBeNull();

      fake.emitFromBackend("session_communication_changed", {
        sessionId,
        communication: {
          kind: "raiseHand",
          visible: true,
          updatedAt,
        },
      });

      await waitFor(() => expect(rendered.root.querySelector(slotSelector)).not.toBeNull());

      fake.emitFromBackend("session_communication_changed", {
        sessionId,
        communication: null,
      });

      await waitFor(() => expect(rendered.root.querySelector(slotSelector)).toBeNull());
    } finally {
      rendered.cleanup();
    }
  });

  it("renders a hydrated raise-hand slot from the initial list_sessions snapshot", async () => {
    const fake = new FakeTransport();
    setupRaiseHandTransport(fake, [
      coordSession({
        communication: {
          kind: "raiseHand",
          visible: true,
          updatedAt,
        },
      }),
    ]);

    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(rendered.root.querySelector(slotSelector)).not.toBeNull());
    } finally {
      rendered.cleanup();
    }
  });
});
