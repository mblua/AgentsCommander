// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
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
import type { Session } from "../../shared/types";

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

function projectDiscovery() {
  return discovery({
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: workgroupPath,
        task: null,
        taskTitle: "Context menu states",
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

function findRow(root: ParentNode, testId: string): HTMLElement {
  const el = root.querySelector<HTMLElement>(`[data-ac-testid="${testId}"]`);
  if (!el) throw new Error(`Row not found: ${testId}`);
  return el;
}

describe("ProjectPanel replica context menu — gray/red (#545)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  async function setupPanel(sessions: Session[] = []): Promise<FakeTransport> {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", projectDiscovery());
    fake.resolve("task_clean", { workgroupRoot: workgroupPath, task: null });
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

  it("keeps the full menu on a green (running) replica", async () => {
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
    expect(menu.textContent).not.toContain("Clear task title");
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
});
