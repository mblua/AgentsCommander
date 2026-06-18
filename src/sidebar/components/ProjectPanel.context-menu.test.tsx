// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectPanel, { isTaskClean } from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  contextMenu,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import type { AcDiscoveryResult, Session } from "../../shared/types";

// Issue #545: gray (never launched) and red (exited) replicas could not open a
// right-click menu, blocking the Coding Agent selector and the clear-task broom
// before/between launches.
//   - gray (no session): minimal Coding Agent + broom menu.
//   - red (exited): the FULL active-replica menu (Restart Session, Coding Agent,
//     Open in new window) PLUS the clear-task broom (#545 rework).
//   - green (running): the full menu, unchanged (no broom).

const projectPath = "C:\\Project";
const workgroupPath = `${projectPath}\\.ac\\wg-2-dev-team`;
const coordPath = `${workgroupPath}\\__agent_dev-webpage-ui`;
const memberPath = `${workgroupPath}\\__agent_dev-rust`;
// renderReplicaItem builds rowTestId() from rowContext + wg.name + replica.name
// via automationIdPart (which keeps these slugs verbatim). The non-coordinator
// member always renders in the "workgroups" section.
const memberRowTestId = "replica.row.workgroups.wg-2-dev-team.dev-rust";

// #545: taskTitle/task are parametrized so tests can drive the broom's
// title-only disable predicate (isTaskClean) across Clean/empty/real titles.
function projectDiscovery(
  taskTitle: string | null = "Context menu states",
  task: string | null = null
) {
  return discovery({
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: workgroupPath,
        task,
        taskTitle,
        agents: [
          { name: "dev-webpage-ui", path: coordPath, repoPaths: [], isCoordinator: true },
          { name: "dev-rust", path: memberPath, repoPaths: [], isCoordinator: false },
        ],
      },
    ],
  });
}

function coordSession(overrides: Partial<Session> = {}): Session {
  return session({
    id: "coord-session",
    name: "wg-2-dev-team/dev-webpage-ui",
    workingDirectory: coordPath,
    status: "running",
    isCoordinator: true,
    ...overrides,
  });
}

function memberSession(overrides: Partial<Session> = {}): Session {
  return session({
    id: "member-session",
    name: "wg-2-dev-team/dev-rust",
    workingDirectory: memberPath,
    status: "running",
    ...overrides,
  });
}

function replicaMenu(): HTMLElement | null {
  // Context menus render through a Portal into document.body, not the panel root.
  return document.querySelector<HTMLElement>(".session-context-menu");
}

function findBroom(menu: HTMLElement): HTMLButtonElement | null {
  return (
    Array.from(menu.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Clear task title")
    ) ?? null
  );
}

function findRow(root: ParentNode, testId: string): HTMLElement {
  const el = root.querySelector<HTMLElement>(`[data-ac-testid="${testId}"]`);
  if (!el) throw new Error(`Row not found: ${testId}`);
  return el;
}

describe("ProjectPanel replica context menu — gray/red (#545)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  async function setupPanel(
    sessions: Session[] = [],
    discoveryResult: AcDiscoveryResult = projectDiscovery()
  ): Promise<FakeTransport> {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", discoveryResult);
    fake.resolve("task_clean", { workgroupRoot: workgroupPath, task: null });
    // #545: cold-workgroup broom routes here when no session resolves the root.
    fake.resolve("task_clean_at", { workgroupRoot: workgroupPath, task: null });
    if (sessions.length > 0) sessionsStore.setSessions(sessions);
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    await projectStore.createAndLoad(projectPath);
    await waitFor(() => expect(rendered!.root.textContent).toContain("dev-rust"));
    return fake;
  }

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("opens a Coding Agent + broom menu on a gray (never-launched) replica", async () => {
    await setupPanel([coordSession()]);

    contextMenu(findRow(rendered!.root, memberRowTestId));

    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      expect(menu!.textContent).toContain("Coding Agent");
      expect(menu!.textContent).toContain("Clear task title");
    });
    const menu = replicaMenu()!;
    expect(menu.textContent).not.toContain("Restart Session");
    expect(menu.textContent).not.toContain("Open in new window");
  });

  it("opens the full menu PLUS broom on a red (exited) replica", async () => {
    await setupPanel([coordSession(), memberSession({ status: { exited: 0 } })]);

    contextMenu(findRow(rendered!.root, memberRowTestId));

    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      // Red keeps the full active-replica menu...
      expect(menu!.textContent).toContain("Restart Session");
      expect(menu!.textContent).toContain("Coding Agent");
      expect(menu!.textContent).toContain("Open in new window");
      // ...and gains the broom (#545 rework).
      expect(menu!.textContent).toContain("Clear task title");
    });
  });

  it("broom on a red replica clears the task", async () => {
    const fake = await setupPanel([
      coordSession(),
      memberSession({ status: { exited: 0 } }),
    ]);

    contextMenu(findRow(rendered!.root, memberRowTestId));

    let broom: HTMLButtonElement | null = null;
    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      broom =
        Array.from(menu!.querySelectorAll("button")).find((b) =>
          b.textContent?.includes("Clear task title")
        ) ?? null;
      expect(broom).not.toBeNull();
      expect(broom!.disabled).toBe(false);
    });

    click(broom!);

    await waitFor(() => {
      const call = fake.lastCall("task_clean");
      expect(call).toBeDefined();
      // task_clean resolves the workgroup TASK.md from any session cwd under the
      // wg-* root; resolveWorkgroupSessionId returns the first non-placeholder
      // session (the coordinator) for the shared workgroup task.
      expect(call!.args.sessionId).toBe("coord-session");
    });
  });

  it("keeps the full menu PLUS broom on a green (running) replica", async () => {
    await setupPanel([coordSession(), memberSession()]);

    contextMenu(findRow(rendered!.root, memberRowTestId));

    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      expect(menu!.textContent).toContain("Restart Session");
    });
    const menu = replicaMenu()!;
    expect(menu.textContent).toContain("Coding Agent");
    expect(menu.textContent).toContain("Open in new window");
    // #545: the broom now renders in EVERY dot state, including green.
    expect(menu.textContent).toContain("Clear task title");
  });

  it("broom on a gray replica clears the task via a sibling workgroup session", async () => {
    const fake = await setupPanel([coordSession()]);

    contextMenu(findRow(rendered!.root, memberRowTestId));

    let broom: HTMLButtonElement | null = null;
    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      broom =
        Array.from(menu!.querySelectorAll("button")).find((b) =>
          b.textContent?.includes("Clear task title")
        ) ?? null;
      expect(broom).not.toBeNull();
      // A live coordinator session exists in the workgroup, so the broom is actionable.
      expect(broom!.disabled).toBe(false);
    });

    click(broom!);

    await waitFor(() => {
      const call = fake.lastCall("task_clean");
      expect(call).toBeDefined();
      // task_clean resolves the workgroup TASK.md from any session cwd under the
      // wg-* root; the gray member reuses the coordinator's session id.
      expect(call!.args.sessionId).toBe("coord-session");
    });
  });

  // ===== #545: broom-on-all-states + cold cleanAt fallback =====

  // The broom now renders in every active (live) dot state, not just red/exited.
  // One representative each for running / idle / waiting; they share the
  // activeReplicaMenu code path and the default title is real, so it stays enabled.
  for (const variant of [
    { label: "running", overrides: {} as Partial<Session> },
    { label: "idle", overrides: { status: "idle" } as Partial<Session> },
    { label: "waiting", overrides: { waitingForInput: true } as Partial<Session> },
  ]) {
    it(`shows an enabled broom on a ${variant.label} replica`, async () => {
      await setupPanel([coordSession(), memberSession(variant.overrides)]);

      contextMenu(findRow(rendered!.root, memberRowTestId));

      let broom: HTMLButtonElement | null = null;
      await waitFor(() => {
        const menu = replicaMenu();
        expect(menu).not.toBeNull();
        broom = findBroom(menu!);
        expect(broom).not.toBeNull();
      });
      expect(broom!.disabled).toBe(false);
    });
  }

  it("disables the broom when the task title is the Clean sentinel (active menu)", async () => {
    await setupPanel([coordSession(), memberSession()], projectDiscovery("Clean"));

    contextMenu(findRow(rendered!.root, memberRowTestId));

    let broom: HTMLButtonElement | null = null;
    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      broom = findBroom(menu!);
      expect(broom).not.toBeNull();
    });
    expect(broom!.disabled).toBe(true);
    expect(broom!.title).toBe("Nothing to clear");
  });

  it("disables the broom when the task title is empty/missing (gray menu)", async () => {
    await setupPanel([coordSession()], projectDiscovery(null));

    contextMenu(findRow(rendered!.root, memberRowTestId));

    let broom: HTMLButtonElement | null = null;
    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      broom = findBroom(menu!);
      expect(broom).not.toBeNull();
    });
    expect(broom!.disabled).toBe(true);
    expect(broom!.title).toBe("Nothing to clear");
  });

  // F1 (ties to G2): the disable predicate is TITLE-ONLY. A "Clean" title with a
  // non-empty body still disables the broom; the body bytes are never consulted.
  it("disables the broom on a Clean title even with a non-empty task body (F1)", async () => {
    await setupPanel(
      [coordSession(), memberSession()],
      projectDiscovery("Clean", "Clean\n\nstale body text")
    );

    contextMenu(findRow(rendered!.root, memberRowTestId));

    let broom: HTMLButtonElement | null = null;
    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      broom = findBroom(menu!);
      expect(broom).not.toBeNull();
    });
    expect(broom!.disabled).toBe(true);
  });

  it("clears a cold workgroup via task_clean_at when no session resolves the root", async () => {
    const fake = await setupPanel([]);

    contextMenu(findRow(rendered!.root, memberRowTestId));

    let broom: HTMLButtonElement | null = null;
    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      broom = findBroom(menu!);
      expect(broom).not.toBeNull();
      // Real title, and the cold path no longer disables on "no session".
      expect(broom!.disabled).toBe(false);
    });

    click(broom!);

    await waitFor(() => {
      const call = fake.lastCall("task_clean_at");
      expect(call).toBeDefined();
      expect(call!.args.workgroupRoot).toBe(workgroupPath);
    });
    // The cold path must NOT fall back to the session-based command.
    expect(fake.lastCall("task_clean")).toBeUndefined();
  });

  // F3 (error path): a rejected task_clean_at must be swallowed by the catch in
  // clearReplicaTaskTitle. We assert the catch actually RAN by spying on
  // console.error — the "Failed to clear task title:" log only fires from inside
  // the catch, so removing the try/catch turns this test red. (A plain
  // not.toThrow() would pass regardless: the onClick wraps an async call, so the
  // rejection surfaces as an unhandled promise, never a synchronous throw.)
  it("swallows a task_clean_at rejection via the catch (F3)", async () => {
    const fake = await setupPanel([]);
    fake.reject("task_clean_at", "boom");
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    try {
      contextMenu(findRow(rendered!.root, memberRowTestId));

      let broom: HTMLButtonElement | null = null;
      await waitFor(() => {
        const menu = replicaMenu();
        expect(menu).not.toBeNull();
        broom = findBroom(menu!);
        expect(broom).not.toBeNull();
      });

      click(broom!);

      // The catch logs exactly this with the rejection reason; if the catch were
      // gone the rejection would go unhandled and the spy would never see it.
      await waitFor(() => {
        expect(
          errorSpy.mock.calls.some(
            (args) => args[0] === "Failed to clear task title:" && args[1] === "boom"
          )
        ).toBe(true);
      });
      expect(fake.lastCall("task_clean_at")).toBeDefined();
      // Menu still dismisses (synchronously, before the rejected await).
      expect(replicaMenu()).toBeNull();
    } finally {
      errorSpy.mockRestore();
    }
  });
});

describe("isTaskClean (#545)", () => {
  it("treats empty/missing and the exact Clean sentinel as clean", () => {
    expect(isTaskClean("")).toBe(true);
    expect(isTaskClean(null)).toBe(true);
    expect(isTaskClean(undefined)).toBe(true);
    expect(isTaskClean("   ")).toBe(true);
    expect(isTaskClean(" Clean ")).toBe(true);
    expect(isTaskClean("Clean")).toBe(true);
  });

  it("is case-sensitive and treats real titles as not clean (G4)", () => {
    expect(isTaskClean("clean")).toBe(false);
    expect(isTaskClean("CLEAN")).toBe(false);
    expect(isTaskClean("Cleanup")).toBe(false);
    expect(isTaskClean("Real")).toBe(false);
  });
});
