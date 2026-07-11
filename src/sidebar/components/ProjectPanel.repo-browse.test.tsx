// @vitest-environment jsdom
import { vi } from "vitest";
// #943 - the Browse submenu is desktop-only (`open_external_url` and
// `git_remote_url` are both no-ops on the WS host), so it only renders when
// isTauri is true. jsdom has no __TAURI_INTERNALS__, so isTauri is false there.
// Mock the module for THIS FILE ONLY: vi.mock is hoisted file-wide, and flipping
// isTauri inside ProjectPanel.context-menu.test.tsx would break its raw
// textContent assertion (:401 would read "AgentsCommander›") and arm the
// four isTauri-gated dynamic @tauri-apps/api/webviewWindow imports.
// A module mock replaces the module wholesale, so all three exports are needed.
vi.mock("../../shared/platform", () => ({
  isTauri: true,
  isBrowser: false,
  isWindows: true,
}));

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
import type { AcDiscoveryResult, Session, SessionRepo } from "../../shared/types";

const projectPath = "C:\\Project";

const wgAPath = `${projectPath}\\.ac\\wg-2-dev-team`;
const coordAPath = `${wgAPath}\\__agent_dev-webpage-ui`;
const repoA1 = `${wgAPath}\\repo-AgentsCommander`;
const repoA2 = `${wgAPath}\\repo-docs`;

const wgBPath = `${projectPath}\\.ac\\wg-3-other-team`;
const coordBPath = `${wgBPath}\\__agent_dev-rust`;
const repoB1 = `${wgBPath}\\repo-webpage`;

const rowA = "replica.row.quick.wg-2-dev-team.dev-webpage-ui";
const rowB = "replica.row.quick.wg-3-other-team.dev-rust";

const GITHUB_REMOTE = "https://github.com/mblua/AgentsCommander.git";

// Solid does not delegate mouseenter/mouseleave (they do not bubble); it binds
// them directly, so a non-bubbling dispatch is exactly what the handler sees.
const mouseEnter = (el: Element) =>
  el.dispatchEvent(new MouseEvent("mouseenter", { bubbles: false, cancelable: true }));
const mouseLeave = (el: Element) =>
  el.dispatchEvent(new MouseEvent("mouseleave", { bubbles: false, cancelable: true }));
const keyDown = (el: Element, key: string) =>
  el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));

/** Drain the macrotask queue. The menu registers its window-level dismiss
 *  listeners inside a `setTimeout` (ProjectPanel :1918), so a test that dispatches
 *  a dismissing event without flushing first would hit a menu that is not yet
 *  listening. */
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

function coordinatorAgent(name: string, path: string, repoPaths: string[], repoBranch?: string) {
  return {
    name,
    path,
    identityPath: `../../_agent_${name}`,
    repoPaths,
    ...(repoBranch === undefined ? {} : { repoBranch }),
    isCoordinator: true,
  };
}

function discoveryA(repoPaths: string[] = [repoA1], repoBranch?: string): AcDiscoveryResult {
  return discovery({
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: wgAPath,
        task: null,
        taskTitle: "Repo browse",
        agents: [coordinatorAgent("dev-webpage-ui", coordAPath, repoPaths, repoBranch)],
      },
    ],
  });
}

function discoveryAB(): AcDiscoveryResult {
  return discovery({
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: wgAPath,
        task: null,
        taskTitle: "Repo browse",
        agents: [coordinatorAgent("dev-webpage-ui", coordAPath, [repoA1])],
      },
      {
        name: "wg-3-other-team",
        path: wgBPath,
        task: null,
        taskTitle: "Repo browse B",
        agents: [coordinatorAgent("dev-rust", coordBPath, [repoB1])],
      },
    ],
  });
}

function repo(sourcePath: string, label: string, branch: string | null): SessionRepo {
  return { label, sourcePath, branch };
}

function coordASession(gitRepos: SessionRepo[]): Session {
  return session({
    id: "coord-a",
    name: "wg-2-dev-team/dev-webpage-ui",
    workingDirectory: coordAPath,
    status: "running",
    isCoordinator: true,
    gitRepos,
  });
}

function coordBSession(gitRepos: SessionRepo[]): Session {
  return session({
    id: "coord-b",
    name: "wg-3-other-team/dev-rust",
    workingDirectory: coordBPath,
    status: "running",
    isCoordinator: true,
    gitRepos,
  });
}

function menu(): HTMLElement | null {
  return document.querySelector<HTMLElement>(".session-context-menu");
}

function flyout(): HTMLElement | null {
  return document.querySelector<HTMLElement>('[data-ac-testid$=".browse.flyout"]');
}

function target(testId: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(`[data-ac-testid="${testId}"]`);
}

function repoEntries(): HTMLButtonElement[] {
  const root = menu();
  if (!root) return [];
  return Array.from(root.querySelectorAll<HTMLButtonElement>(".session-context-repo-option"));
}

/** Arrows on REPO entries only. `Add to Group` is a submenu trigger too and
 *  carries the same arrow span, so an unscoped selector always finds one. */
function repoArrows(): HTMLElement[] {
  const root = menu();
  if (!root) return [];
  return Array.from(
    root.querySelectorAll<HTMLElement>(
      ".session-context-repo-option .session-context-submenu-arrow"
    )
  );
}

function flyoutLabels(): string[] {
  const panel = flyout();
  if (!panel) return [];
  return Array.from(panel.querySelectorAll("button")).map((b) => (b.textContent ?? "").trim());
}

describe("ProjectPanel coordinator repo Browse submenu (#943)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  async function setupPanel(
    sessions: Session[],
    discoveryResult: AcDiscoveryResult,
    remote: unknown | ((args: Record<string, unknown>) => unknown) = GITHUB_REMOTE
  ): Promise<FakeTransport> {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", discoveryResult);
    fake.resolve("task_clean", { workgroupRoot: wgAPath, task: null });
    fake.resolve("task_clean_at", { workgroupRoot: wgAPath, task: null });
    fake.resolve("open_in_explorer", null);
    fake.resolve("open_external_url", null);
    // FakeTransport throws on an unregistered invoke, so git_remote_url is always
    // registered. Args-aware when a function is passed: fake.resolve() keys by
    // command only and cannot vary the answer per repo path.
    if (typeof remote === "function") {
      fake.onInvoke("git_remote_url", remote as (args: Record<string, unknown>) => unknown);
    } else {
      fake.resolve("git_remote_url", remote);
    }
    if (sessions.length > 0) sessionsStore.setSessions(sessions);
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    await projectStore.createAndLoad(projectPath);
    await waitFor(() => expect(rendered!.root.textContent).toContain("dev-webpage-ui"));
    return fake;
  }

  /** Right-click a row, wait until the Browse arrows have resolved, and let the
   *  menu finish installing its window-level dismiss listeners. */
  async function openMenuWithArrow(testId: string, expectedArrows = 1): Promise<void> {
    contextMenu(target(testId)!);
    await waitFor(() => {
      expect(menu()).not.toBeNull();
      expect(repoArrows()).toHaveLength(expectedArrows);
    });
    await flush();
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

  // 1
  it("opens Browse Main + Browse Branch on hover and navigates to the branch URL", async () => {
    const fake = await setupPanel(
      [coordASession([repo(repoA1, "AgentsCommander", "feature/x")])],
      discoveryA()
    );

    await openMenuWithArrow(rowA);
    mouseEnter(repoEntries()[0]);

    await waitFor(() => expect(flyout()).not.toBeNull());
    expect(flyoutLabels()).toEqual(["Browse Main", "Browse Branch"]);

    click(target("replica.coord-a.menu.repo.0.browse.branch")!);

    await waitFor(() => {
      expect(fake.lastCall("open_external_url")?.args).toEqual({
        url: "https://github.com/mblua/AgentsCommander/tree/feature/x",
      });
      expect(menu()).toBeNull();
    });
  });

  // 2
  it("keeps the slashes in a branch name literal", async () => {
    const fake = await setupPanel(
      [coordASession([repo(repoA1, "AgentsCommander", "fix/909-agency-agents-roles-yaml-skip")])],
      discoveryA()
    );

    await openMenuWithArrow(rowA);
    mouseEnter(repoEntries()[0]);
    await waitFor(() => expect(flyout()).not.toBeNull());

    click(target("replica.coord-a.menu.repo.0.browse.branch")!);

    await waitFor(() =>
      expect(fake.lastCall("open_external_url")?.args).toEqual({
        url: "https://github.com/mblua/AgentsCommander/tree/fix/909-agency-agents-roles-yaml-skip",
      })
    );
  });

  // 3
  it("offers Browse Main only when the branch is main", async () => {
    await setupPanel([coordASession([repo(repoA1, "AgentsCommander", "main")])], discoveryA());

    await openMenuWithArrow(rowA);
    mouseEnter(repoEntries()[0]);

    await waitFor(() => expect(flyout()).not.toBeNull());
    expect(flyoutLabels()).toEqual(["Browse Main"]);
  });

  // 4
  it("offers Browse Main only when the branch is unknown", async () => {
    await setupPanel([coordASession([repo(repoA1, "AgentsCommander", null)])], discoveryA());

    await openMenuWithArrow(rowA);
    mouseEnter(repoEntries()[0]);

    await waitFor(() => expect(flyout()).not.toBeNull());
    expect(flyoutLabels()).toEqual(["Browse Main"]);
  });

  // 5
  it("renders no arrow and no flyout for a non-GitHub remote, and still opens the folder", async () => {
    const fake = await setupPanel(
      [coordASession([repo(repoA1, "AgentsCommander", "feature/x")])],
      discoveryA(),
      "https://gitlab.com/o/r.git"
    );

    contextMenu(target(rowA)!);
    await waitFor(() => expect(repoEntries()).toHaveLength(1));
    // The remote resolves asynchronously; give it every chance to paint an arrow.
    await waitFor(() => expect(fake.callsFor("git_remote_url")).toHaveLength(1));
    await flush();

    expect(repoArrows()).toHaveLength(0);
    mouseEnter(repoEntries()[0]);
    await flush();
    expect(flyout()).toBeNull();

    click(repoEntries()[0]);
    await waitFor(() =>
      expect(fake.lastCall("open_in_explorer")?.args).toEqual({ path: repoA1 })
    );
  });

  // 6
  it("renders no arrow when the repo has no origin", async () => {
    const fake = await setupPanel(
      [coordASession([repo(repoA1, "AgentsCommander", "feature/x")])],
      discoveryA(),
      null
    );

    contextMenu(target(rowA)!);
    await waitFor(() => expect(repoEntries()).toHaveLength(1));
    await waitFor(() => expect(fake.callsFor("git_remote_url")).toHaveLength(1));
    await flush();

    expect(repoArrows()).toHaveLength(0);
    mouseEnter(repoEntries()[0]);
    await flush();
    expect(flyout()).toBeNull();
  });

  // 7
  it("still opens the local folder when the entry itself is clicked (purely additive)", async () => {
    const fake = await setupPanel(
      [coordASession([repo(repoA1, "AgentsCommander", "feature/x")])],
      discoveryA()
    );

    await openMenuWithArrow(rowA);
    click(repoEntries()[0]);

    await waitFor(() => {
      expect(fake.lastCall("open_in_explorer")?.args).toEqual({ path: repoA1 });
      expect(menu()).toBeNull();
    });
    expect(fake.callsFor("open_external_url")).toHaveLength(0);
  });

  // 8
  it("renders the submenu on an inactive (gray) coordinator", async () => {
    // Cold single-repo replica: the branch comes from discovery's repoBranch via
    // configuredReplicaRepoBadges, not from a session.
    await setupPanel([], discoveryA([repoA1], "feature/y"));

    await openMenuWithArrow(rowA);
    mouseEnter(repoEntries()[0]);

    await waitFor(() => expect(target("replica.inactive.menu.repo.0.browse.flyout")).not.toBeNull());
    expect(flyoutLabels()).toEqual(["Browse Main", "Browse Branch"]);
    expect(target("replica.inactive.menu.repo.0.browse.branch")!.getAttribute("title")).toBe(
      "https://github.com/mblua/AgentsCommander/tree/feature/y"
    );
  });

  // 9
  it("is mutually exclusive with the Add to Group flyout", async () => {
    await setupPanel([coordASession([repo(repoA1, "AgentsCommander", "feature/x")])], discoveryA());

    await openMenuWithArrow(rowA);
    mouseEnter(repoEntries()[0]);
    await waitFor(() => expect(flyout()).not.toBeNull());

    mouseEnter(target("replica.wg-2-dev-team.groups.trigger")!);

    await waitFor(() => {
      expect(flyout()).toBeNull();
      expect(target("replica.wg-2-dev-team.groups.flyout")).not.toBeNull();
    });
  });

  // 10
  it("invokes git_remote_url once per distinct repo path per menu open, never on hover", async () => {
    const fake = await setupPanel(
      [
        coordASession([
          repo(repoA1, "AgentsCommander", "feature/x"),
          repo(repoA2, "docs", "main"),
        ]),
      ],
      discoveryA([repoA1, repoA2])
    );

    await openMenuWithArrow(rowA, 2);
    expect(fake.callsFor("git_remote_url")).toHaveLength(2);

    mouseEnter(repoEntries()[0]);
    mouseEnter(repoEntries()[0]);
    mouseEnter(repoEntries()[0]);
    await flush();

    expect(fake.callsFor("git_remote_url")).toHaveLength(2);
    expect(fake.callsFor("git_remote_url").map((c) => c.args.path)).toEqual([repoA1, repoA2]);
  });

  // 11
  it("does not inherit a flyout when the menu is closed and reopened", async () => {
    const fake = await setupPanel(
      [coordASession([repo(repoA1, "AgentsCommander", "feature/x")])],
      discoveryA()
    );

    await openMenuWithArrow(rowA);
    mouseEnter(repoEntries()[0]);
    await waitFor(() => expect(flyout()).not.toBeNull());

    // Escape at the window level is the menu's own dismiss path.
    keyDown(document.body, "Escape");
    await waitFor(() => expect(menu()).toBeNull());
    expect(flyout()).toBeNull();

    contextMenu(target(rowA)!);
    await waitFor(() => expect(menu()).not.toBeNull());
    // No stale panel before the fresh resolution lands, and the reopen re-resolved
    // (generation bumped) rather than reusing the previous menu's answers.
    expect(flyout()).toBeNull();
    await waitFor(() => expect(fake.callsFor("git_remote_url")).toHaveLength(2));
    expect(flyout()).toBeNull();
  });

  // 12 [S1 / BLOCKER 1]
  it("renders B's repos, titles and testids after right-clicking A then B", async () => {
    const fake = await setupPanel(
      [
        coordASession([repo(repoA1, "AgentsCommander", "feature/x")]),
        coordBSession([repo(repoB1, "webpage", "feature/y")]),
      ],
      discoveryAB()
    );

    await openMenuWithArrow(rowA);
    expect(repoEntries()[0].getAttribute("title")).toBe(repoA1);
    expect(target("replica.coord-a.menu.repo.0")).not.toBeNull();

    // No close in between: the handlers overwrite the signal non-null -> non-null,
    // so nothing here goes through a null transition.
    contextMenu(target(rowB)!);

    await waitFor(() => {
      expect(repoEntries()).toHaveLength(1);
      expect(repoEntries()[0].getAttribute("title")).toBe(repoB1);
    });
    expect(target("replica.coord-b.menu.repo.0")).not.toBeNull();
    expect(target("replica.coord-a.menu.repo.0")).toBeNull();
    expect(repoEntries()[0].textContent).toContain("webpage");

    click(repoEntries()[0]);
    await waitFor(() =>
      expect(fake.lastCall("open_in_explorer")?.args).toEqual({ path: repoB1 })
    );
  });

  // 13 [S1]
  it("leaves no stale flyout when the menu switches from A to B", async () => {
    await setupPanel(
      [
        coordASession([repo(repoA1, "AgentsCommander", "feature/x")]),
        coordBSession([repo(repoB1, "webpage", "feature/y")]),
      ],
      discoveryAB()
    );

    await openMenuWithArrow(rowA);
    mouseEnter(repoEntries()[0]);
    await waitFor(() => expect(flyout()).not.toBeNull());

    contextMenu(target(rowB)!);

    await waitFor(() => expect(target("replica.coord-b.menu.repo.0")).not.toBeNull());
    expect(flyout()).toBeNull();
  });

  // 13b [S1] - isolates the handler guard. In test 13 the flyout ALSO gets closed
  // by the identity-check effect (B's entry 0 has a different sourcePath, so the
  // live re-derive returns null). Here both coordinators point at the SAME repo
  // path, so the identity check passes and cannot fire: the only thing that can
  // close the flyout is `closeRepoFlyout()` in the context-menu handler. Delete
  // that line and this test goes red with an empty, stale, mis-anchored panel.
  it("closes the flyout on an A->B switch even when the repo path is identical", async () => {
    const shared = `${projectPath}\.ac\repo-shared`;
    const discoveryShared = discovery({
      workgroups: [
        {
          name: "wg-2-dev-team",
          path: wgAPath,
          task: null,
          taskTitle: "Repo browse",
          agents: [coordinatorAgent("dev-webpage-ui", coordAPath, [shared])],
        },
        {
          name: "wg-3-other-team",
          path: wgBPath,
          task: null,
          taskTitle: "Repo browse B",
          agents: [coordinatorAgent("dev-rust", coordBPath, [shared])],
        },
      ],
    });

    await setupPanel(
      [
        coordASession([repo(shared, "shared", "feature/x")]),
        coordBSession([repo(shared, "shared", "feature/x")]),
      ],
      discoveryShared
    );

    await openMenuWithArrow(rowA);
    mouseEnter(repoEntries()[0]);
    await waitFor(() => expect(flyout()).not.toBeNull());

    contextMenu(target(rowB)!);

    await waitFor(() => expect(target("replica.coord-b.menu.repo.0")).not.toBeNull());
    expect(flyout()).toBeNull();
  });

  // 14 [S2]
  it("closes the flyout on Escape without re-opening it, and keeps the menu", async () => {
    await setupPanel([coordASession([repo(repoA1, "AgentsCommander", "feature/x")])], discoveryA());

    await openMenuWithArrow(rowA);
    const entry = repoEntries()[0];

    keyDown(entry, "ArrowRight");
    await waitFor(() => expect(flyout()).not.toBeNull());

    keyDown(flyout()!, "Escape");
    expect(flyout()).toBeNull();

    // The refocus of the trigger fires onFocus synchronously; without the
    // suppress latch it would re-open the flyout on the next microtask.
    await flush();
    expect(flyout()).toBeNull();
    expect(menu()).not.toBeNull();
    expect(document.activeElement).toBe(entry);
  });

  // 15
  it("does not kill the menu when the flyout's padding is clicked", async () => {
    const fake = await setupPanel(
      [coordASession([repo(repoA1, "AgentsCommander", "feature/x")])],
      discoveryA()
    );

    await openMenuWithArrow(rowA);
    mouseEnter(repoEntries()[0]);
    await waitFor(() => expect(flyout()).not.toBeNull());

    click(flyout()!); // the panel itself, not a button
    await flush();

    expect(menu()).not.toBeNull();
    expect(flyout()).not.toBeNull();
    expect(fake.callsFor("open_external_url")).toHaveLength(0);
  });

  // 16 [S3]
  it("follows a branch change that lands while the flyout is open", async () => {
    const fake = await setupPanel(
      [coordASession([repo(repoA1, "AgentsCommander", "feature/x")])],
      discoveryA()
    );

    await openMenuWithArrow(rowA);
    mouseEnter(repoEntries()[0]);
    await waitFor(() => expect(flyout()).not.toBeNull());

    // This is exactly what App.tsx's onSessionGitRepos listener does with the
    // backend event (App.tsx:596-597). ProjectPanel does not own that listener,
    // so driving the store directly is the honest way to reproduce a GitWatcher
    // tick in a ProjectPanel-scoped test.
    sessionsStore.setGitRepos("coord-a", [repo(repoA1, "AgentsCommander", "feature/new")]);

    await waitFor(() =>
      expect(target("replica.coord-a.menu.repo.0.browse.branch")!.getAttribute("title")).toBe(
        "https://github.com/mblua/AgentsCommander/tree/feature/new"
      )
    );
    // The entry was re-derived by index + sourcePath, so the flyout survives.
    expect(flyout()).not.toBeNull();

    click(target("replica.coord-a.menu.repo.0.browse.branch")!);
    await waitFor(() =>
      expect(fake.lastCall("open_external_url")?.args).toEqual({
        url: "https://github.com/mblua/AgentsCommander/tree/feature/new",
      })
    );
  });

  // 17
  it("shows the arrow only on the repo that has a GitHub origin", async () => {
    await setupPanel(
      [
        coordASession([
          repo(repoA1, "AgentsCommander", "feature/x"),
          repo(repoA2, "docs", "feature/x"),
        ]),
      ],
      discoveryA([repoA1, repoA2]),
      (args: Record<string, unknown>) =>
        String(args.path).endsWith("repo-AgentsCommander")
          ? GITHUB_REMOTE
          : "https://gitlab.com/o/r.git"
    );

    await openMenuWithArrow(rowA, 1);
    expect(repoEntries()).toHaveLength(2);
    expect(repoEntries()[0].querySelector(".session-context-submenu-arrow")).not.toBeNull();
    expect(repoEntries()[1].querySelector(".session-context-submenu-arrow")).toBeNull();

    mouseEnter(repoEntries()[1]);
    await flush();
    expect(flyout()).toBeNull();

    mouseEnter(repoEntries()[0]);
    await waitFor(() => expect(flyout()).not.toBeNull());
    expect(flyoutLabels()).toEqual(["Browse Main", "Browse Branch"]);

    // Moving onto the non-browsable entry must not leave the flyout hanging.
    mouseLeave(repoEntries()[0]);
    mouseEnter(repoEntries()[1]);
    expect(flyout()).toBeNull();
  });
});
