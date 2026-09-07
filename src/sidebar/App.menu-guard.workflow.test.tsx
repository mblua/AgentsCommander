// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SidebarApp, { BLOCKED_MENU_TAG } from "./App";
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
import { requestTaskbarAttention } from "./attention";

vi.mock("./attention", () => ({ requestTaskbarAttention: vi.fn() }));

const flashMock = vi.mocked(requestTaskbarAttention);

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

// #1857: ALWAYS a fresh object. `sessionsStore.setCommunication` writes through
// a Solid store, which merges key by key when both the old and the new value are
// objects, so a reused fixture can leave a stale key behind (issue 1863).
function blockedMenu(message = menuMessage, at = updatedAt): SessionCommunication {
  return {
    kind: "blockedMenu",
    visible: true,
    updatedAt: at,
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
    flashMock.mockClear();
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

  // #1857: N blocked sessions, ONE derived toast. These helpers mount an
  // arbitrary session list so the aggregate can be exercised past one session.
  function blockedRow(index: number, communication: SessionCommunication | null): Session {
    return session({
      id: `blocked-${index}`,
      name: `agent-${index}`,
      workingDirectory: replicaPath,
      status: "running",
      communication,
    });
  }

  async function mountWithSessions(
    sessions: Session[],
  ): Promise<ReturnType<typeof renderWithFakeTransport>> {
    const fake = new FakeTransport();
    setupMenuGuardTransport(fake, sessions);
    const next = renderWithFakeTransport(() => <SidebarApp />, fake);
    rendered = next;
    await waitFor(() => expect(fake.callsFor("list_sessions")).toHaveLength(1));
    return next;
  }

  function aggregateToast() {
    return toastStore.items.find((toast) => toast.tag === BLOCKED_MENU_TAG);
  }

  function visibleToastText(): string {
    return document.body.querySelector("[data-ac-testid='toast.item']")?.textContent ?? "";
  }

  function visibleToastCount(): number {
    return document.body.querySelectorAll("[data-ac-testid='toast.item']").length;
  }

  // A real-timer flush. `waitFor` returns the FIRST time its assertion holds, so
  // it cannot show that nothing further arrives; this gives a queued second
  // effect run a real chance to land before a count is asserted.
  async function settle(): Promise<void> {
    await new Promise((resolve) => setTimeout(resolve, 20));
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
  // ---- #1857: the aggregated notice ----------------------------------------

  it("12. one blocked session: the toast carries that session's verbatim text, no suffix", async () => {
    const { fake } = await mountSidebar();

    fake.emitFromBackend("session_communication_changed", {
      sessionId,
      communication: blockedMenu(),
    });

    await waitFor(() => expect(aggregateToast()).toBeDefined());
    expect(aggregateToast()?.message).toBe(`${wgName}/${replicaName}: ${menuMessage}`);
    expect(aggregateToast()?.message).not.toContain("more waiting");
    expect(visibleToastCount()).toBe(1);
  });

  it("13. ten blocked sessions: one toast, the NEWEST text, and (and 9 more waiting)", async () => {
    // The newest by `updatedAt` is deliberately index 3, not the last element,
    // so a test that passes by accident of array order cannot.
    const minutes = [10, 11, 12, 59, 13, 14, 15, 16, 17, 18];
    const rows = minutes.map((minute, index) =>
      blockedRow(
        index,
        blockedMenu(`menu option ${index}`, `2026-08-31T06:${minute}:00.000Z`),
      ),
    );

    await mountWithSessions(rows);

    await waitFor(() => expect(aggregateToast()).toBeDefined());
    expect(aggregateToast()?.message).toBe("agent-3: menu option 3 (and 9 more waiting)");
    expect(visibleToastText()).toContain("agent-3: menu option 3");
    expect(visibleToastCount()).toBe(1);
  });

  it("14. resolving the one ON SCREEN repaints the toast with the second-newest", async () => {
    const minutes = [10, 11, 12, 59, 13, 14, 15, 16, 17, 18];
    const rows = minutes.map((minute, index) =>
      blockedRow(
        index,
        blockedMenu(`menu option ${index}`, `2026-08-31T06:${minute}:00.000Z`),
      ),
    );

    const { fake } = await mountWithSessions(rows);
    await waitFor(() => expect(aggregateToast()).toBeDefined());

    // Resolve the one ON SCREEN through its own button: the actions must target
    // the session whose text is showing (agent-3), never the oldest (agent-0).
    document.body
      .querySelector<HTMLButtonElement>("[data-ac-testid='toast.item.action']")!
      .dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    await waitFor(() =>
      expect(fake.callsFor("resolve_blocking_menu")).toEqual([
        { cmd: "resolve_blocking_menu", args: { id: "blocked-3" } },
      ]),
    );

    // The backend clearing it is what changes the set. The second-newest is
    // agent-9, at minute 18.
    fake.emitFromBackend("session_communication_changed", {
      sessionId: "blocked-3",
      communication: null,
    });

    await waitFor(() =>
      expect(aggregateToast()?.message).toBe("agent-9: menu option 9 (and 8 more waiting)"),
    );
    expect(visibleToastCount()).toBe(1);
  });

  it("15. clearing the LAST blocked session dismisses the aggregate", async () => {
    const rows = [0, 1].map((index) =>
      blockedRow(
        index,
        blockedMenu(`menu option ${index}`, `2026-08-31T06:1${index}:00.000Z`),
      ),
    );

    const { fake } = await mountWithSessions(rows);
    await waitFor(() => expect(aggregateToast()).toBeDefined());

    for (const index of [0, 1]) {
      fake.emitFromBackend("session_communication_changed", {
        sessionId: `blocked-${index}`,
        communication: null,
      });
    }

    await waitFor(() =>
      expect(document.body.querySelector("[data-ac-testid='toast.item']")).toBeNull(),
    );
    expect(aggregateToast()).toBeUndefined();
  });

  it("16. the flash fires ONCE per session entering the set, not per event", async () => {
    const { fake } = await mountSidebar();

    fake.emitFromBackend("session_communication_changed", {
      sessionId,
      communication: blockedMenu(),
    });
    await waitFor(() => expect(flashMock).toHaveBeenCalledTimes(1));

    // The #1856 poll re-applying an IDENTICAL communication must not re-flash.
    // A fresh object with the same content: the store merges key by key and
    // notifies nothing, so the effect does not even run.
    fake.emitFromBackend("session_communication_changed", {
      sessionId,
      communication: blockedMenu(),
    });
    await settle();
    expect(flashMock).toHaveBeenCalledTimes(1);
  });

  it("17. leaving and re-entering the blocked set re-arms the flash", async () => {
    const { fake } = await mountSidebar();

    fake.emitFromBackend("session_communication_changed", {
      sessionId,
      communication: blockedMenu(),
    });
    await waitFor(() => expect(flashMock).toHaveBeenCalledTimes(1));

    fake.emitFromBackend("session_communication_changed", {
      sessionId,
      communication: null,
    });
    await waitFor(() => expect(aggregateToast()).toBeUndefined());

    fake.emitFromBackend("session_communication_changed", {
      sessionId,
      communication: blockedMenu("a second blocking menu", "2026-08-31T07:00:00.000Z"),
    });
    await waitFor(() => expect(flashMock).toHaveBeenCalledTimes(2));
  });

  it("18. an empty blocked set never flashes", async () => {
    const { fake } = await mountSidebar();

    fake.emitFromBackend("session_communication_changed", {
      sessionId,
      communication: null,
    });

    await settle();
    expect(flashMock).not.toHaveBeenCalled();
    expect(aggregateToast()).toBeUndefined();
  });

  it("19. the effect runs exactly ONCE for one arriving blocked session", async () => {
    // Filtering by the tag is mandatory, not stylistic. `vi.spyOn` replaces the
    // `push` property on the shared `toastStore`, and `.error`, `.info` and
    // `.success` call back THROUGH that property, so an unfiltered counter
    // counts every toast source reachable from a mounted sidebar (eighteen of
    // them) and its 1-versus-2 discrimination is a flake waiting to happen.
    const pushSpy = vi.spyOn(toastStore, "push");
    try {
      await mountWithSessions([blockedRow(0, blockedMenu("only one"))]);
      await waitFor(() => expect(aggregateToast()).toBeDefined());
      await settle();

      // Without `untrack` this is 2, measured. Do NOT relax it to "the array
      // length stays 1": Solid's reactive cycle is synchronous, so flushing
      // microtasks cannot observe re-entry and that assertion is decorative.
      expect(
        pushSpy.mock.calls.filter((c) => c[0]?.tag === BLOCKED_MENU_TAG),
      ).toHaveLength(1);
    } finally {
      pushSpy.mockRestore();
    }
  });

  it("21. the SAME session moving to another menu repaints the toast", async () => {
    const { fake } = await mountSidebar();

    fake.emitFromBackend("session_communication_changed", {
      sessionId,
      communication: blockedMenu("first menu text"),
    });
    await waitFor(() =>
      expect(aggregateToast()?.message).toBe(`${wgName}/${replicaName}: first menu text`),
    );

    // Same set, same `kind`, same `visible`: only `message` and `updatedAt` move.
    // That is what `set_blocked_menu` does when a session goes from one blocking
    // menu to another, and after change 4 the memo is the only thing that can
    // notice. A memo filtering only on `kind` and `visible` leaves the old text.
    fake.emitFromBackend("session_communication_changed", {
      sessionId,
      communication: blockedMenu("second menu text", "2026-08-31T07:30:00.000Z"),
    });

    await waitFor(() =>
      expect(aggregateToast()?.message).toBe(`${wgName}/${replicaName}: second menu text`),
    );
    expect(visibleToastText()).toContain("second menu text");
    expect(visibleToastText()).not.toContain("first menu text");
    expect(visibleToastCount()).toBe(1);
  });
});
