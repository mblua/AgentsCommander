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
import { initialSelection } from "../shared/testing/session-selection";
import { toastStore } from "../shared/stores/toasts";
import type { Session, SessionCommunication } from "../shared/types";

const projectPath = "C:\\Project";
const wgName = "room-guard";
const replicaName = "orchestrator";
const workgroupPath = `${projectPath}\\.ac\\${wgName}`;
const replicaPath = `${workgroupPath}\\__agent_${replicaName}`;
const sessionId = "menu-session";
const updatedAt = "2026-08-31T06:00:00.000Z";
const menuMessage = "Choose an option in the interactive menu";

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

function setupMenuGuardTransport(fake: FakeTransport, sessions: Session[]): void {
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
          taskTitle: "Menu guard",
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
  fake.resolve("get_active_session", initialSelection());
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
  fake.resolve("resolve_blocking_menu", undefined);
  fake.resolve("switch_session", undefined);
}

function blockedMenu(message = menuMessage): SessionCommunication {
  return {
    kind: "blockedMenu",
    visible: true,
    updatedAt,
    message,
  };
}

describe("SidebarApp menu-guard workflow (#1649)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    toastStore.clear();
  });

  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    toastStore.clear();
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
  });

  async function mountSidebar(): Promise<ReturnType<typeof renderWithFakeTransport>> {
    const fake = new FakeTransport();
    setupMenuGuardTransport(fake, [coordSession()]);
    const next = renderWithFakeTransport(() => <SidebarApp />, fake);
    rendered = next;
    await waitFor(() => expect(fake.callsFor("list_sessions")).toHaveLength(1));
    return next;
  }

  it("shows a sticky toast with the resolution action for a blocked-menu event", async () => {
    const { fake } = await mountSidebar();

    fake.emitFromBackend("session_communication_changed", {
      sessionId,
      communication: blockedMenu(),
    });

    await waitFor(() => {
      expect(document.body.querySelector("[data-ac-testid='toast.item']")?.textContent ?? "")
        .toContain(menuMessage);
      expect(document.body.querySelector("[data-ac-testid='toast.item.action']")?.textContent)
        .toBe("Resolved by user");
    });
  });

  it("invokes resolve_blocking_menu with the blocked session id", async () => {
    const { fake } = await mountSidebar();
    fake.emitFromBackend("session_communication_changed", {
      sessionId,
      communication: blockedMenu(),
    });
    await waitFor(() =>
      expect(document.body.querySelector("[data-ac-testid='toast.item.action']")).not.toBeNull()
    );

    document.body.querySelector<HTMLButtonElement>("[data-ac-testid='toast.item.action']")!
      .dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));

    await waitFor(() => {
      expect(fake.callsFor("resolve_blocking_menu")).toEqual([
        { cmd: "resolve_blocking_menu", args: { id: sessionId } },
      ]);
    });
  });

  it("auto-dismisses the tagged toast when the backend clears communication", async () => {
    const { fake } = await mountSidebar();
    fake.emitFromBackend("session_communication_changed", {
      sessionId,
      communication: blockedMenu(),
    });
    await waitFor(() =>
      expect(document.body.querySelector("[data-ac-testid='toast.item']")).not.toBeNull()
    );

    fake.emitFromBackend("session_communication_changed", {
      sessionId,
      communication: null,
    });

    await waitFor(() =>
      expect(document.body.querySelector("[data-ac-testid='toast.item']")).toBeNull()
    );
  });

  it("shows See terminal and Resolved by user on the blocked-menu toast", async () => {
    const { fake } = await mountSidebar();

    fake.emitFromBackend("session_communication_changed", {
      sessionId,
      communication: blockedMenu(),
    });

    await waitFor(() => {
      expect(
        document.body.querySelector("[data-ac-testid='toast.item.action.secondary']")?.textContent
      ).toBe("See terminal");
      expect(document.body.querySelector("[data-ac-testid='toast.item.action']")?.textContent)
        .toBe("Resolved by user");
    });
  });

  it("See terminal invokes switch_session with the blocked session id and keeps the toast", async () => {
    const { fake } = await mountSidebar();
    fake.emitFromBackend("session_communication_changed", {
      sessionId,
      communication: blockedMenu(),
    });
    await waitFor(() =>
      expect(
        document.body.querySelector("[data-ac-testid='toast.item.action.secondary']")
      ).not.toBeNull()
    );

    document.body
      .querySelector<HTMLButtonElement>("[data-ac-testid='toast.item.action.secondary']")!
      .dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));

    await waitFor(() =>
      expect(fake.callsFor("switch_session")).toEqual([
        { cmd: "switch_session", args: { id: sessionId } },
      ])
    );

    expect(fake.callsFor("resolve_blocking_menu")).toEqual([]);
    const item = document.body.querySelector("[data-ac-testid='toast.item']");
    expect(item).not.toBeNull();
    expect(item?.classList.contains("toast-item--exiting")).toBe(false);
  });
});
