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
import { automationIdPart } from "./replica-repo-badges";

// Issue #545: gray (never launched) and red (exited) replicas could not open a
// right-click menu, blocking the Coding Agent selector and the clear-task broom
// before/between launches.
//   - gray (no session): Coding Agent + Matrix folder + Replica folder + broom menu.
//   - red (exited): the FULL active-replica menu (Restart Session, Coding Agent,
//     Matrix/Replica folders, Open in new window) PLUS the clear-task broom (#545 rework).
//   - green (running): the full menu, including the broom.

const projectPath = "C:\\Project";
const workgroupPath = `${projectPath}\\.ac\\wg-2-dev-team`;
const coordPath = `${workgroupPath}\\__agent_dev-webpage-ui`;
const memberPath = `${workgroupPath}\\__agent_dev-rust`;
const memberMatrixPath = `${projectPath}\\.ac\\_agent_dev-rust`;
const originAgentPath = `${projectPath}\\.ac\\_agent_dev-docs`;
const otherProjectPath = "D:\\OtherProject";
// renderReplicaItem builds rowTestId() from rowContext + wg.name + replica.name
// via automationIdPart (which keeps these slugs verbatim). The non-coordinator
// member always renders in the "workgroups" section.
const memberRowTestId = "replica.row.workgroups.wg-2-dev-team.dev-rust";
const coordQuickRowTestId = "replica.row.quick.wg-2-dev-team.dev-webpage-ui";

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
          {
            name: "dev-webpage-ui",
            path: coordPath,
            identityPath: "../../_agent_dev-webpage-ui",
            repoPaths: [],
            isCoordinator: true,
          },
          {
            name: "dev-rust",
            path: memberPath,
            identityPath: "../../_agent_dev-rust",
            repoPaths: [],
            isCoordinator: false,
          },
        ],
      },
    ],
  });
}

function projectDiscoveryWithCoordinatorRepos(path: string, repoPaths: string[]): AcDiscoveryResult {
  const wgPath = `${path}\\.ac\\wg-2-dev-team`;
  const replicaPath = `${wgPath}\\__agent_dev-webpage-ui`;
  return discovery({
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: wgPath,
        task: null,
        taskTitle: "Context menu states",
        agents: [
          {
            name: "dev-webpage-ui",
            path: replicaPath,
            identityPath: "../../_agent_dev-webpage-ui",
            repoPaths,
            isCoordinator: true,
          },
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

function findMatrixFolderAction(menu: HTMLElement): HTMLButtonElement | null {
  return (
    Array.from(menu.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Open Matrix folder")
    ) ?? null
  );
}

function findReplicaFolderAction(menu: HTMLElement): HTMLButtonElement | null {
  return (
    Array.from(menu.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Open Replica's Folder")
    ) ?? null
  );
}

function repoFolderActions(menu: HTMLElement): HTMLButtonElement[] {
  return Array.from(menu.querySelectorAll<HTMLButtonElement>(".session-context-repo-option"));
}

function repoFolderLabel(item: HTMLElement): string | null {
  return item.querySelector<HTMLElement>(".session-context-repo-label")?.textContent ?? null;
}

function menuButtonLabels(menu: HTMLElement): string[] {
  return Array.from(menu.querySelectorAll<HTMLButtonElement>("button")).map((button) =>
    (button.textContent ?? "").trim().replace(/\u203a/g, "").replace(/^[^A-Za-z0-9]+/, "").trim()
  );
}

function findRow(root: ParentNode, testId: string): HTMLElement {
  const el = root.querySelector<HTMLElement>(`[data-ac-testid="${testId}"]`);
  if (!el) throw new Error(`Row not found: ${testId}`);
  return el;
}

function findProjectPanel(root: ParentNode, path: string): HTMLElement {
  const panel = Array.from(root.querySelectorAll<HTMLElement>(".project-panel")).find((candidate) =>
    candidate.querySelector<HTMLElement>(".project-header")?.getAttribute("title") === path
  );
  if (!panel) throw new Error(`Project panel not found: ${path}`);
  return panel;
}

function findAgentRow(root: ParentNode, label: string): HTMLElement {
  const el = Array.from(root.querySelectorAll<HTMLElement>(".replica-item")).find((row) =>
    row.textContent?.includes(label)
  );
  if (!el) throw new Error(`Agent row not found: ${label}`);
  return el;
}

function findAgentsHeader(root: ParentNode): HTMLElement {
  const el = Array.from(root.querySelectorAll<HTMLElement>(".ac-wg-header")).find((row) =>
    row.textContent?.includes("Agents")
  );
  if (!el) throw new Error("Agents header not found");
  return el;
}

function findExactMenuButton(menu: HTMLElement, label: string): HTMLButtonElement | null {
  return (
    Array.from(menu.querySelectorAll<HTMLButtonElement>("button")).find((button) =>
      (button.textContent ?? "").trim() === label
    ) ?? null
  );
}

describe("ProjectPanel replica context menu — gray/red (#545)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  async function setupPanel(
    sessions: Session[] = [],
    discoveryResult: AcDiscoveryResult = projectDiscovery(),
    expectedText = "dev-rust"
  ): Promise<FakeTransport> {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", discoveryResult);
    fake.resolve("task_clean", { workgroupRoot: workgroupPath, task: null });
    // #545: cold-workgroup broom routes here when no session resolves the root.
    fake.resolve("task_clean_at", { workgroupRoot: workgroupPath, task: null });
    fake.resolve("open_in_explorer", null);
    if (sessions.length > 0) sessionsStore.setSessions(sessions);
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    await projectStore.createAndLoad(projectPath);
    await waitFor(() => expect(rendered!.root.textContent).toContain(expectedText));
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

  it("opens a Coding Agent + Matrix folder + broom menu on a gray replica", async () => {
    await setupPanel([coordSession()]);

    contextMenu(findRow(rendered!.root, memberRowTestId));

    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      expect(menu!.textContent).toContain("Coding Agent");
      expect(menu!.textContent).toContain("Open Replica's Folder");
      expect(menu!.textContent).toContain("Clear task title");
    });
    const menu = replicaMenu()!;
    expect(menu.textContent).not.toContain("Restart Session");
    expect(menu.textContent).not.toContain("Open in new window");
  });

  it("preserves the native context menu inside the project regex filter row", async () => {
    await setupPanel([coordSession()]);

    const header = rendered!.root.querySelector<HTMLElement>(".project-header");
    const filterInput = header?.querySelector<HTMLInputElement>(".project-filter-input");
    expect(header).not.toBeNull();
    expect(filterInput).not.toBeNull();

    const inputContextMenu = new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
      clientX: 80,
      clientY: 96,
    });

    expect(filterInput!.dispatchEvent(inputContextMenu)).toBe(true);
    expect(inputContextMenu.defaultPrevented).toBe(false);
    expect(replicaMenu()).toBeNull();

    contextMenu(header!);

    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      expect(menu!.textContent).toContain("New Agent");
    });
  });

  it("opens the canonical Matrix folder from a gray workgroup replica", async () => {
    const fake = await setupPanel([coordSession()]);

    contextMenu(findRow(rendered!.root, memberRowTestId));

    let action: HTMLButtonElement | null = null;
    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      action = findMatrixFolderAction(menu!);
      expect(action).not.toBeNull();
      expect(action!.querySelector(".session-context-matrix-icon")).not.toBeNull();
    });

    click(action!);

    await waitFor(() => {
      const call = fake.lastCall("open_in_explorer");
      expect(call).toBeDefined();
      expect(call!.args.path).toBe(memberMatrixPath);
    });
    expect(fake.lastCall("open_in_explorer")!.args.path).not.toBe(memberPath);
  });

  it("opens the workgroup replica folder from a gray workgroup replica", async () => {
    const fake = await setupPanel([coordSession()]);

    contextMenu(findRow(rendered!.root, memberRowTestId));

    let action: HTMLButtonElement | null = null;
    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      action = findReplicaFolderAction(menu!);
      expect(action).not.toBeNull();
    });

    click(action!);

    await waitFor(() => {
      const call = fake.lastCall("open_in_explorer");
      expect(call).toBeDefined();
      expect(call!.args.path).toBe(memberPath);
    });
    expect(fake.lastCall("open_in_explorer")!.args.path).not.toBe(memberMatrixPath);
  });

  it("opens the canonical Matrix folder from a live workgroup replica", async () => {
    const fake = await setupPanel([coordSession(), memberSession()]);

    contextMenu(findRow(rendered!.root, memberRowTestId));

    let action: HTMLButtonElement | null = null;
    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      action = findMatrixFolderAction(menu!);
      expect(action).not.toBeNull();
    });

    click(action!);

    await waitFor(() => {
      const call = fake.lastCall("open_in_explorer");
      expect(call).toBeDefined();
      expect(call!.args.path).toBe(memberMatrixPath);
    });
    expect(fake.lastCall("open_in_explorer")!.args.path).not.toBe(memberPath);
  });

  it("opens the workgroup replica folder from a live workgroup replica", async () => {
    const fake = await setupPanel([coordSession(), memberSession()]);

    contextMenu(findRow(rendered!.root, memberRowTestId));

    let action: HTMLButtonElement | null = null;
    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      action = findReplicaFolderAction(menu!);
      expect(action).not.toBeNull();
    });

    click(action!);

    await waitFor(() => {
      const call = fake.lastCall("open_in_explorer");
      expect(call).toBeDefined();
      expect(call!.args.path).toBe(memberPath);
    });
    expect(fake.lastCall("open_in_explorer")!.args.path).not.toBe(memberMatrixPath);
  });

  it("renders coordinator repo folder actions from live gitRepos and opens duplicate labels by index", async () => {
    const repos = [
      { label: "AgentsCommander", sourcePath: "C:\\Project\\.ac\\wg-2-dev-team\\repo-AgentsCommander", branch: "feature/x" },
      { label: "empty", sourcePath: "", branch: null },
      { label: "missing", branch: null },
      { label: "number", sourcePath: 42, branch: null },
      { label: "spaces", sourcePath: "   ", branch: null },
      { label: "docs", sourcePath: "C:\\Project\\.ac\\wg-2-dev-team\\repo-docs-primary", branch: null },
      { label: "docs", sourcePath: "C:\\Project\\.ac\\wg-2-dev-team\\repo-docs-secondary", branch: "docs-alt" },
      { label: "cli", sourcePath: "C:\\Project\\.ac\\wg-2-dev-team\\repo-cli", branch: "main" },
    ] as unknown as Session["gitRepos"];
    const fake = await setupPanel([coordSession({ gitRepos: repos })], projectDiscovery(), "dev-webpage-ui");

    contextMenu(findRow(rendered!.root, coordQuickRowTestId));

    let items: HTMLButtonElement[] = [];
    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      items = repoFolderActions(menu!);
      expect(items).toHaveLength(4);
      expect(items.map(repoFolderLabel)).toEqual(["AgentsCommander", "docs", "docs", "cli"]);
    });

    expect(items.map((item) => item.textContent?.trim())).toEqual(["AgentsCommander", "docs", "docs", "cli"]);
    expect(items[0].textContent).not.toContain("Open");
    expect(items[0].textContent).not.toContain("feature/x");
    expect(items[2].textContent).not.toContain("docs-alt");
    expect(items[3].textContent).not.toContain("main");
    expect(menuButtonLabels(replicaMenu()!)).toEqual([
      "Restart Session",
      "Coding Agent",
      "Open Replica's Folder",
      "AgentsCommander",
      "docs",
      "docs",
      "cli",
      "Open Matrix folder",
      "Open in new window",
      "Add to Group",
      "Clear task title",
    ]);

    click(document.querySelector('[data-ac-testid="replica.coord-session.menu.repo.2"]')!);

    await waitFor(() =>
      expect(fake.lastCall("open_in_explorer")?.args).toEqual({
        path: "C:\\Project\\.ac\\wg-2-dev-team\\repo-docs-secondary",
      }),
    );
    expect(replicaMenu()).toBeNull();
  });

  it("falls back to the row's configured repos when a same-name live session belongs to another project", async () => {
    const wrongLiveRepo = `${projectPath}\\.ac\\wg-2-dev-team\\repo-wrong-live`;
    const otherConfiguredRepo = `${otherProjectPath}\\.ac\\wg-2-dev-team\\repo-correct-fallback`;
    const fake = new FakeTransport();
    fake.onInvoke("new_project", (args) => ({
      path: args.path as string,
      registered: true,
      created: false,
    }));
    fake.onInvoke("discover_project", (args) => {
      const path = args.path as string;
      if (path === projectPath) return projectDiscoveryWithCoordinatorRepos(projectPath, [wrongLiveRepo]);
      if (path === otherProjectPath) return projectDiscoveryWithCoordinatorRepos(otherProjectPath, [otherConfiguredRepo]);
      throw new Error(`unexpected discovery path: ${path}`);
    });
    fake.resolve("open_in_explorer", null);
    sessionsStore.setSessions([
      coordSession({
        id: "same-name-other-project",
        workingDirectory: coordPath,
        gitRepos: [{ label: "wrong-live", sourcePath: wrongLiveRepo, branch: "main" }],
      }),
    ]);
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    await projectStore.createAndLoad(projectPath);
    await projectStore.createAndLoad(otherProjectPath);
    await waitFor(() => expect(rendered!.root.textContent).toContain("OtherProject"));

    contextMenu(findRow(findProjectPanel(rendered!.root, otherProjectPath), coordQuickRowTestId));

    let items: HTMLButtonElement[] = [];
    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      expect(menu!.textContent).not.toContain("Restart Session");
      items = repoFolderActions(menu!);
      expect(items).toHaveLength(1);
      expect(repoFolderLabel(items[0])).toBe("correct-fallback");
      expect(items[0].title).toBe(otherConfiguredRepo);
    });

    click(items[0]);

    await waitFor(() =>
      expect(fake.lastCall("open_in_explorer")?.args).toEqual({
        path: otherConfiguredRepo,
      }),
    );
    expect(fake.lastCall("open_in_explorer")!.args.path).not.toBe(wrongLiveRepo);
  });

  it("renders repo folder actions for a gray inactive coordinator from configured repo paths", async () => {
    const repos = [
      `${projectPath}\\.ac\\wg-2-dev-team\\repo-AgentsCommander`,
      `${projectPath}\\.ac\\wg-2-dev-team\\repo-docs`,
    ];
    const fake = await setupPanel(
      [],
      projectDiscoveryWithCoordinatorRepos(projectPath, repos),
      "dev-webpage-ui"
    );

    contextMenu(findRow(rendered!.root, coordQuickRowTestId));

    let items: HTMLButtonElement[] = [];
    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      expect(menu!.textContent).not.toContain("Restart Session");
      items = repoFolderActions(menu!);
      expect(items).toHaveLength(2);
      expect(items.map(repoFolderLabel)).toEqual(["AgentsCommander", "docs"]);
      expect(items[0].title).toBe(repos[0]);
      expect(items[1].title).toBe(repos[1]);
      expect(menuButtonLabels(menu!)).toEqual([
        "Coding Agent",
        "Open Replica's Folder",
        "AgentsCommander",
        "docs",
        "Open Matrix folder",
        "Add to Group",
        "Clear task title",
      ]);
    });

    click(items[1]);

    await waitFor(() =>
      expect(fake.lastCall("open_in_explorer")?.args).toEqual({
        path: repos[1],
      }),
    );
  });

  it("hides the Matrix folder action for a workgroup replica without identityPath", async () => {
    const fake = await setupPanel(
      [coordSession()],
      discovery({
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
      })
    );

    contextMenu(findRow(rendered!.root, memberRowTestId));

    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      expect(findMatrixFolderAction(menu!)).toBeNull();
      expect(findReplicaFolderAction(menu!)).not.toBeNull();
    });
    expect(fake.lastCall("open_in_explorer")).toBeUndefined();
  });

  it("opens the Matrix folder from an origin agent row", async () => {
    const fake = await setupPanel(
      [],
      discovery({
        agents: [
          {
            name: "dev-docs",
            path: originAgentPath,
            roleExists: true,
          },
        ],
      }),
      "dev-docs"
    );

    contextMenu(findAgentRow(rendered!.root, "dev-docs"));

    let action: HTMLButtonElement | null = null;
    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      action = findMatrixFolderAction(menu!);
      expect(action).not.toBeNull();
    });

    click(action!);

    await waitFor(() => {
      const call = fake.lastCall("open_in_explorer");
      expect(call).toBeDefined();
      expect(call!.args.path).toBe(originAgentPath);
    });
  });

  it("keeps the AGENTS header menu category-only", async () => {
    await setupPanel(
      [],
      discovery({
        agents: [
          {
            name: "dev-docs",
            path: originAgentPath,
            roleExists: true,
          },
        ],
      }),
      "dev-docs"
    );

    contextMenu(findAgentsHeader(rendered!.root));

    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      expect(menu!.textContent).toContain("New Agent");
      expect(findExactMenuButton(menu!, "Delete")).toBeNull();
      expect(menu!.textContent).not.toContain("Delete Agent");
    });
  });

  it("shows a trash Delete action on an offline origin agent row", async () => {
    await setupPanel(
      [],
      discovery({
        agents: [
          {
            name: "dev-docs",
            path: originAgentPath,
            roleExists: true,
          },
        ],
      }),
      "dev-docs"
    );

    contextMenu(findAgentRow(rendered!.root, "dev-docs"));

    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      const action = findExactMenuButton(menu!, "Delete");
      expect(action).not.toBeNull();
      expect(action!.querySelector(".session-context-trash-icon")).not.toBeNull();
      expect(action!.getAttribute("data-ac-role")).toBe("menuitem");
      expect(action!.getAttribute("data-ac-testid")).toBe(
        `agent.action.delete.${automationIdPart(originAgentPath)}`
      );
      expect(menu!.textContent).not.toContain("Delete Agent");
    });
  });

  it("opens and cancels the offline origin agent delete modal without invoking delete", async () => {
    const fake = await setupPanel(
      [],
      discovery({
        agents: [
          {
            name: "dev-docs",
            path: originAgentPath,
            roleExists: true,
          },
        ],
      }),
      "dev-docs"
    );

    contextMenu(findAgentRow(rendered!.root, "dev-docs"));
    await waitFor(() => expect(replicaMenu()).not.toBeNull());
    click(findExactMenuButton(replicaMenu()!, "Delete")!);

    const dialogId = `agent.delete.dialog.${automationIdPart(originAgentPath)}`;
    const cancelId = `agent.delete.cancel.${automationIdPart(originAgentPath)}`;
    await waitFor(() => expect(document.querySelector(`[data-ac-testid="${dialogId}"]`)).not.toBeNull());
    expect(document.body.textContent).toContain("workgroup replicas");

    click(document.querySelector(`[data-ac-testid="${cancelId}"]`)!);

    await waitFor(() => expect(document.querySelector(`[data-ac-testid="${dialogId}"]`)).toBeNull());
    expect(fake.callsFor("delete_agent_matrix")).toHaveLength(0);
  });

  it("confirms offline origin agent delete with an agentPath payload and reloads the project", async () => {
    const fake = await setupPanel(
      [],
      discovery({
        agents: [
          {
            name: "dev-docs",
            path: originAgentPath,
            roleExists: true,
          },
        ],
      }),
      "dev-docs"
    );
    fake.resolve("delete_agent_matrix", null);
    const initialDiscoveryCalls = fake.callsFor("discover_project").length;

    contextMenu(findAgentRow(rendered!.root, "dev-docs"));
    await waitFor(() => expect(replicaMenu()).not.toBeNull());
    click(findExactMenuButton(replicaMenu()!, "Delete")!);

    const confirmId = `agent.delete.confirm.${automationIdPart(originAgentPath)}`;
    await waitFor(() => expect(document.querySelector(`[data-ac-testid="${confirmId}"]`)).not.toBeNull());
    click(document.querySelector(`[data-ac-testid="${confirmId}"]`)!);

    await waitFor(() => expect(fake.callsFor("delete_agent_matrix")).toHaveLength(1));
    expect(fake.callsFor("delete_agent_matrix")[0].args).toEqual({
      projectPath,
      agentPath: originAgentPath,
    });
    await waitFor(() => expect(fake.callsFor("discover_project").length).toBeGreaterThan(initialDiscoveryCalls));
  });

  it("surfaces parsed BLOCKERS details when offline origin agent delete is blocked", async () => {
    const fake = await setupPanel(
      [],
      discovery({
        agents: [
          {
            name: "dev-docs",
            path: originAgentPath,
            roleExists: true,
          },
        ],
      }),
      "dev-docs"
    );
    fake.onInvoke("delete_agent_matrix", () => {
      throw `BLOCKERS:${JSON.stringify({
        workgroup: "agent dev-docs",
        platform: "windows",
        diagnosticAvailable: true,
        rawOsError: "sharing violation",
        sessions: [],
        processes: [],
        liveSessions: [
          {
            sessionId: "agent-session",
            agentName: "dev-docs",
            cwd: originAgentPath,
          },
        ],
        externalProcesses: [
          {
            pid: 42,
            name: "Code.exe",
            cwd: projectPath,
            files: [originAgentPath],
          },
        ],
      })}`;
    });

    contextMenu(findAgentRow(rendered!.root, "dev-docs"));
    await waitFor(() => expect(replicaMenu()).not.toBeNull());
    click(findExactMenuButton(replicaMenu()!, "Delete")!);

    const confirmId = `agent.delete.confirm.${automationIdPart(originAgentPath)}`;
    const errorId = `agent.delete.error.${automationIdPart(originAgentPath)}`;
    await waitFor(() => expect(document.querySelector(`[data-ac-testid="${confirmId}"]`)).not.toBeNull());
    click(document.querySelector(`[data-ac-testid="${confirmId}"]`)!);

    await waitFor(() => {
      const error = document.querySelector<HTMLElement>(`[data-ac-testid="${errorId}"]`);
      expect(error).not.toBeNull();
      expect(error!.textContent).toContain("Live AC sessions");
      expect(error!.textContent).toContain("dev-docs");
      expect(error!.textContent).toContain("External processes");
      expect(error!.textContent).toContain("Code.exe");
    });
    expect((document.querySelector(`[data-ac-testid="${confirmId}"]`) as HTMLButtonElement).disabled).toBe(false);
  });

  it("exposes trash Delete on an active AGENTS row through SessionItem", async () => {
    await setupPanel(
      [
        session({
          id: "agent-session",
          name: "dev-docs",
          workingDirectory: originAgentPath,
          status: "running",
        }),
      ],
      discovery({
        agents: [
          {
            name: "dev-docs",
            path: originAgentPath,
            roleExists: true,
          },
        ],
      }),
      "dev-docs"
    );

    contextMenu(document.querySelector('[data-ac-testid="session.agent-session"]')!);

    await waitFor(() => {
      const menu = document.querySelector<HTMLElement>('[data-ac-testid="session.agent-session.menu"]');
      expect(menu).not.toBeNull();
      const action = findExactMenuButton(menu!, "Delete");
      expect(action).not.toBeNull();
      expect(action!.querySelector(".session-context-trash-icon")).not.toBeNull();
      expect(action!.getAttribute("data-ac-testid")).toBe(
        `agent.action.delete.${automationIdPart(originAgentPath)}`
      );
    });

    click(findExactMenuButton(document.querySelector<HTMLElement>('[data-ac-testid="session.agent-session.menu"]')!, "Delete")!);

    await waitFor(() =>
      expect(document.querySelector(`[data-ac-testid="agent.delete.dialog.${automationIdPart(originAgentPath)}"]`)).not.toBeNull()
    );
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
      expect(menu!.textContent).toContain("Open Replica's Folder");
      expect(menu!.textContent).toContain("Open in new window");
      // ...and gains the broom (#545 rework).
      expect(menu!.textContent).toContain("Clear task title");
    });
  });

  it("opens the workgroup replica folder from a red (exited) workgroup replica", async () => {
    const fake = await setupPanel([
      coordSession(),
      memberSession({ status: { exited: 0 } }),
    ]);

    contextMenu(findRow(rendered!.root, memberRowTestId));

    let action: HTMLButtonElement | null = null;
    await waitFor(() => {
      const menu = replicaMenu();
      expect(menu).not.toBeNull();
      action = findReplicaFolderAction(menu!);
      expect(action).not.toBeNull();
    });

    click(action!);

    await waitFor(() => {
      const call = fake.lastCall("open_in_explorer");
      expect(call).toBeDefined();
      expect(call!.args.path).toBe(memberPath);
    });
    expect(fake.lastCall("open_in_explorer")!.args.path).not.toBe(memberMatrixPath);
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
    expect(menu.textContent).toContain("Open Replica's Folder");
    expect(menu.textContent).toContain("Open in new window");
    // #545: the broom now renders in EVERY dot state, including green.
    expect(menu.textContent).toContain("Clear task title");
  });

  it("removes the ProjectPanel live replica hover folder button", async () => {
    await setupPanel([coordSession(), memberSession()]);

    const row = findRow(rendered!.root, memberRowTestId);

    expect(row.querySelector(".session-item-explorer")).toBeNull();
    expect(row.querySelector(".session-item-mic")).not.toBeNull();
    expect(row.querySelector(".session-item-detach")).not.toBeNull();
    expect(row.querySelector(".session-item-telegram")).not.toBeNull();
    expect(row.querySelector(".session-item-close")).not.toBeNull();
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
