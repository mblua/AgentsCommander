// @vitest-environment jsdom
import { vi } from "vitest";
// #944 - acceptance A and B, driven through the REAL automation bridge.
//
// The whole chain runs here: executeAutomationRequest -> synthetic pointer/mouse
// events -> Solid's directly-bound onMouseEnter/onMouseLeave -> the flyout. The
// #943 suite next door fakes the hover with a target-only `new MouseEvent(...)`,
// which is exactly the lie this action exists to stop telling.
//
// The platform mock is NOT decoration: `repoBrowseItems` is gated on
// `browseSupported() = isTauri` (ProjectPanel :861, :888), so without it every
// `browseItems()` is empty, no arrow ever renders, and every test below hangs on
// the arrow wait.
vi.mock("../../shared/platform", () => ({
  isTauri: true,
  isBrowser: false,
  isWindows: true,
}));

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import {
  executeAutomationRequest,
  resetAutomationBridgeForTests,
} from "../../shared/automation-bridge";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import type {
  AcDiscoveryResult,
  Session,
  SessionRepo,
  UiAutomationAction,
  UiAutomationResponse,
} from "../../shared/types";

const projectPath = "C:\\Project";

const wgAPath = `${projectPath}\\.ac\\wg-2-dev-team`;
const coordAPath = `${wgAPath}\\__agent_dev-webpage-ui`;
const repoA1 = `${wgAPath}\\repo-AgentsCommander`;
const repoA2 = `${wgAPath}\\repo-docs`;

const wgBPath = `${projectPath}\\.ac\\wg-3-other-team`;
const coordBPath = `${wgBPath}\\__agent_dev-rust`;

const rowA = "replica.row.quick.wg-2-dev-team.dev-webpage-ui";
const rowB = "replica.row.quick.wg-3-other-team.dev-rust";

const entry0 = "replica.coord-a.menu.repo.0";
const entry1 = "replica.coord-a.menu.repo.1";
const arrow0 = "replica.coord-a.menu.repo.0.browse.arrow";
const arrow1 = "replica.coord-a.menu.repo.1.browse.arrow";
const flyout0 = "replica.coord-a.menu.repo.0.browse.flyout";
const flyout1 = "replica.coord-a.menu.repo.1.browse.flyout";
const browseMain0 = "replica.coord-a.menu.repo.0.browse.main";
const browseBranch0 = "replica.coord-a.menu.repo.0.browse.branch";

const GITHUB_REMOTE = "https://github.com/mblua/AgentsCommander.git";
const GITHUB_MAIN_URL = "https://github.com/mblua/AgentsCommander";

/** The 180 ms `scheduleRepoFlyoutClose` timer (ProjectPanel :1439-1445) is REAL, and
 *  fake timers cannot be used in this file (see the header of the describe below).
 *  So a "the flyout is still there afterwards" assertion has to burn real wall clock:
 *  `waitFor` resolves on its FIRST passing poll, which is t=0, and would green-light
 *  an enter-before-leave bug that kills the flyout 180 ms later. */
const REPO_FLYOUT_CLOSE_MS = 180;
const sleepPastFlyoutClose = () =>
  new Promise((resolve) => setTimeout(resolve, REPO_FLYOUT_CLOSE_MS + 70));

function domRect(left: number, top: number, width: number, height: number): DOMRect {
  return {
    left,
    top,
    width,
    height,
    right: left + width,
    bottom: top + height,
    x: left,
    y: top,
    toJSON: () => ({}),
  } as DOMRect;
}

/** Verbatim from ProjectPanel.repo-browse.test.tsx:104-113. A detached node keeps
 *  its all-zero rect, which is what a real browser reports for one. */
function stubBoundingRects(): () => void {
  const spy = vi
    .spyOn(Element.prototype, "getBoundingClientRect")
    .mockImplementation(function (this: Element) {
      if (!this.isConnected) return domRect(0, 0, 0, 0);
      if (this.classList?.contains("session-context-flyout")) return domRect(204, 50, 100, 20);
      return domRect(100, 50, 100, 20); // a repo entry: right = 200, top = 50
    });
  return () => spy.mockRestore();
}

/** REQUIRED, not belt-and-braces: jsdom returns an empty DOMRectList for every
 *  element, and `isElementVisible` (automation-bridge :492) demands
 *  `getClientRects().length > 0`. Without this stub EVERY bridge call in this file
 *  answers `target_hidden` and the suite is uniformly, silently useless. */
function stubClientRects(): () => void {
  const spy = vi
    .spyOn(Element.prototype, "getClientRects")
    .mockImplementation(function (this: Element) {
      const rect = this.getBoundingClientRect(); // goes through stubBoundingRects
      const rects = rect.width > 0 && rect.height > 0 ? [rect] : [];
      const list = rects as unknown as DOMRectList;
      Object.defineProperty(list, "item", {
        configurable: true,
        value: (index: number) => list[index] ?? null,
      });
      return list;
    });
  return () => spy.mockRestore();
}

/** The production reclamp prefers requestAnimationFrame; jsdom fires it on a real
 *  ~16 ms frame. Route it through a macrotask so the queued clamp is deterministic. */
function stubAnimationFrame(): () => void {
  const previous = globalThis.requestAnimationFrame;
  Object.defineProperty(globalThis, "requestAnimationFrame", {
    configurable: true,
    writable: true,
    value: (cb: FrameRequestCallback) => window.setTimeout(() => cb(0), 0),
  });
  return () => {
    Object.defineProperty(globalThis, "requestAnimationFrame", {
      configurable: true,
      writable: true,
      value: previous,
    });
  };
}

function coordinatorAgent(name: string, path: string, repoPaths: string[]) {
  return {
    name,
    path,
    identityPath: `../../_agent_${name}`,
    repoPaths,
    isCoordinator: true,
  };
}

function discoveryA(repoPaths: string[] = [repoA1]): AcDiscoveryResult {
  return discovery({
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: wgAPath,
        task: null,
        taskTitle: "Repo browse",
        agents: [coordinatorAgent("dev-webpage-ui", coordAPath, repoPaths)],
      },
    ],
  });
}

/** Both coordinators point at the SAME repo path, so the identity-check effect
 *  (:1831-1839) re-derives a live entry and cannot fire. The only thing left that
 *  can close the flyout on an A -> B menu switch is the handler-level
 *  `closeRepoFlyout()` at :2276 / :2322 - the #943 fix that B3 pins. */
function discoveryShared(shared: string): AcDiscoveryResult {
  return discovery({
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
}

function repo(sourcePath: string, label: string, branch: string | null): SessionRepo {
  return { label, sourcePath, branch, dirty: null };
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

function target(testId: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(`[data-ac-testid="${testId}"]`);
}

/** Prefix-agnostic. A ghost flyout re-renders under the CURRENT menu's prefix
 *  (the testid is built from the live `testIdPrefix()` accessor, :1864 + :3768), so
 *  a testid query can only ever look for it under one prefix. This cannot miss. */
function flyoutNode(): HTMLElement | null {
  return document.querySelector<HTMLElement>(".session-context-flyout");
}

describe("ProjectPanel repo Browse submenu, driven through the automation bridge (#944)", () => {
  let cleanupDom: (() => void) | null = null;
  let restoreRects: (() => void) | null = null;
  let restoreClientRects: (() => void) | null = null;
  let restoreRaf: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  /** Every request is a real one: the bridge resolves the selector, runs the
   *  visible / disabled / obscured gates and dispatches for real.
   *
   *  There is deliberately NO `elementFromPoint` stub in this file. jsdom does not
   *  implement it, so `topmostElementAtCenter` (:562-565) short-circuits to ok and
   *  the obscured gate is inert here. Faking a hit-test would make the gate
   *  unfalsifiable AND look tested; it is pinned at the unit level instead
   *  (automation-bridge.test.ts, A10). */
  const act = async (
    action: Exclude<UiAutomationAction, "terminal">,
    selector: string,
    value?: string,
  ): Promise<UiAutomationResponse> =>
    executeAutomationRequest("main", {
      requestId: `r-${action}-${selector}`,
      token: "t",
      window: "main",
      action,
      selector,
      value,
      expiresAtUnixMs: Date.now() + 5_000,
    });

  const leave = () => act("hover", "", "leave");

  const ok = (response: UiAutomationResponse): Extract<UiAutomationResponse, { ok: true }> => {
    if (!response.ok) throw new Error(`${response.error}: ${response.message}`);
    return response;
  };

  const errorOf = (response: UiAutomationResponse): string => {
    if (response.ok) throw new Error(`expected an error, got ok for "${response.selector}"`);
    return response.error;
  };

  /** Wait on the DOM, never through the bridge: a `query` that misses burns the full
   *  250 ms MISSING_SELECTOR_RETRY_MS budget (bridge :305-341) on every poll. */
  const waitForTarget = (testId: string) =>
    waitFor(() => expect(target(testId)).not.toBeNull());

  async function setupPanel(
    sessions: Session[],
    discoveryResult: AcDiscoveryResult,
    remote: unknown | ((args: Record<string, unknown>) => unknown) = GITHUB_REMOTE,
  ): Promise<FakeTransport> {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", discoveryResult);
    fake.resolve("task_clean", { workgroupRoot: wgAPath, task: null });
    fake.resolve("task_clean_at", { workgroupRoot: wgAPath, task: null });
    fake.resolve("open_in_explorer", null);
    fake.resolve("open_external_url", null);
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

  /** Right-click through the bridge, then wait for the arrow.
   *
   *  The arrow wait is re-run after EVERY contextClick on purpose: `resolveRepoRemotes`
   *  clears `repoRemotes` to `{}` synchronously on every menu open (:865-866), so the
   *  previous menu's arrow vanishes the instant the next menu opens. */
  async function openMenuWithArrow(rowTestId: string, arrowTestId: string): Promise<void> {
    ok(await act("contextClick", rowTestId));
    await waitForTarget(arrowTestId);
  }

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    restoreRects = stubBoundingRects();
    restoreClientRects = stubClientRects();
    restoreRaf = stubAnimationFrame();
    resetUiStoresForTests();
    // The sticky pointer is module scope and survives a render teardown.
    resetAutomationBridgeForTests();
  });

  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    restoreRaf?.();
    restoreClientRects?.();
    restoreRects?.();
    cleanupDom?.();
    restoreRaf = null;
    restoreClientRects = null;
    restoreRects = null;
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  // B1 - acceptance A
  it("opens the Browse submenu on hover, with Browse Main and Browse Branch", async () => {
    await setupPanel([coordASession([repo(repoA1, "AgentsCommander", "feature/x")])], discoveryA());

    await openMenuWithArrow(rowA, arrow0);
    const hovered = ok(await act("hover", entry0));

    // Repo-derived selectors are addressable but private under #1539, so the
    // response must prove the transition without projecting either identifier.
    expect(hovered.diagnostics?.hover).toMatchObject({ from: null, to: null, changed: true });
    expect(ok(await act("query", flyout0)).target.testId).toBeNull();
    expect(ok(await act("query", browseMain0)).target.text).toBe("Browse Main");
    expect(ok(await act("query", browseBranch0)).target.text).toBe("Browse Branch");
  });

  // B2
  it("offers no Browse Branch on main, while the arrow still resolves", async () => {
    await setupPanel([coordASession([repo(repoA1, "AgentsCommander", "main")])], discoveryA());

    await openMenuWithArrow(rowA, arrow0);
    ok(await act("hover", entry0));

    expect(ok(await act("query", browseMain0)).target.text).toBe("Browse Main");
    expect(errorOf(await act("query", browseBranch0))).toBe("missing_selector");
  });

  // B3 - acceptance B
  it("leaves no ghost flyout on an A -> B menu switch, with an identical repo path", async () => {
    const shared = `${projectPath}\\.ac\\repo-shared`;
    await setupPanel(
      [
        coordASession([repo(shared, "shared", "feature/x")]),
        coordBSession([repo(shared, "shared", "feature/x")]),
      ],
      discoveryShared(shared),
    );

    await openMenuWithArrow(rowA, arrow0);
    ok(await act("hover", entry0));
    ok(await act("query", flyout0));

    ok(await act("contextClick", rowB));
    await waitForTarget("replica.coord-b.menu.repo.0");

    // (i) THE detector. A surviving flyout re-renders under B's prefix, because the
    //     testid is built from the live testIdPrefix() accessor. Revert :2276 / :2322
    //     and this line goes red (B4).
    expect(errorOf(await act("query", "replica.coord-b.menu.repo.0.browse.flyout"))).toBe(
      "missing_selector",
    );
    // (ii) the prefix-agnostic belt (same check as repo-browse.test.tsx:607).
    expect(flyoutNode()).toBeNull();
  });

  // B5
  it("opens the Add to Group flyout on hover", async () => {
    await setupPanel([coordASession([repo(repoA1, "AgentsCommander", "feature/x")])], discoveryA());

    ok(await act("contextClick", rowA));
    await waitForTarget("replica.wg-2-dev-team.groups.trigger");

    ok(await act("hover", "replica.wg-2-dev-team.groups.trigger"));

    expect(ok(await act("query", "replica.wg-2-dev-team.groups.flyout")).target.testId).toBeNull();
  });

  // B6
  it("re-anchors from one repo entry to the next, and the flyout survives the 180 ms window", async () => {
    await setupPanel(
      [
        coordASession([
          repo(repoA1, "AgentsCommander", "feature/x"),
          repo(repoA2, "docs", "feature/y"),
        ]),
      ],
      discoveryA([repoA1, repoA2]),
    );

    await openMenuWithArrow(rowA, arrow1);
    ok(await act("hover", entry0));
    ok(await act("query", flyout0));

    ok(await act("hover", entry1));
    ok(await act("query", flyout1));

    // The whole test. Hovering entry 1 fires mouseleave on entry 0, which arms
    // scheduleRepoFlyoutClose(180ms); the enter that follows cancels it inside
    // openRepoFlyout (:1493). Swap the two groups and the timer is armed AFTER the
    // cancel, so the flyout dies here - 180 ms after a `waitFor` would have passed.
    await sleepPastFlyoutClose();

    expect(ok(await act("query", flyout1)).target.testId).toBeNull();
  });

  // B7
  it("closes the flyout when the pointer is parked nowhere", async () => {
    await setupPanel([coordASession([repo(repoA1, "AgentsCommander", "feature/x")])], discoveryA());

    await openMenuWithArrow(rowA, arrow0);
    ok(await act("hover", entry0));
    expect(flyoutNode()).not.toBeNull();

    const left = ok(await leave());
    expect(left.diagnostics?.hover).toMatchObject({ from: null, to: null, changed: true });

    // Nothing re-opened it, so the 180 ms timer runs to completion.
    await waitFor(() => expect(flyoutNode()).toBeNull());
  });

  // B8
  it("opens nothing, and does not lie, when the remote has not resolved yet", async () => {
    await setupPanel(
      [coordASession([repo(repoA1, "AgentsCommander", "feature/x")])],
      discoveryA(),
      () => new Promise(() => {}), // git_remote_url never resolves
    );

    ok(await act("contextClick", rowA));
    await waitForTarget(entry0);
    expect(target(arrow0)).toBeNull(); // the gate is closed: no arrow

    const hovered = ok(await act("hover", entry0)); // ok, and honest about it

    expect(hovered.diagnostics?.hover?.changed).toBe(true); // the pointer DID move
    expect(errorOf(await act("query", flyout0))).toBe("missing_selector"); // nothing opened
  });

  // B9 - the chain test. Worth more than the rest of this file combined.
  it("hovers into the submenu item across the Portal boundary, survives, and clicks it", async () => {
    const fake = await setupPanel(
      [coordASession([repo(repoA1, "AgentsCommander", "feature/x")])],
      discoveryA(),
    );

    await openMenuWithArrow(rowA, arrow0);
    ok(await act("hover", entry0));
    ok(await act("query", browseMain0));

    // repo.0 lives in the context-menu Portal, browse.main in the flyout's OWN
    // Portal, so their common ancestor is <body>:
    //
    //   leaveChain = [button, div.session-context-menu, portal]   <- onMouseLeave
    //                                                                (:3727) arms 180ms
    //   enterChain = [portal, div.session-context-flyout, button] <- onMouseEnter
    //                                                                (:1849) cancels it
    //
    // Both handlers are non-bubbling and Solid binds them directly on the declaring
    // node, so a target-only dispatch reaches NEITHER: the menu's leave arms the
    // timer, nothing cancels it, and the flyout dies 180 ms later - mid-click.
    ok(await act("hover", browseMain0));
    await sleepPastFlyoutClose();

    expect(ok(await act("query", browseMain0)).target.testId).toBeNull();

    ok(await act("click", browseMain0));

    await waitFor(() =>
      expect(fake.lastCall("open_external_url")?.args).toEqual({ url: GITHUB_MAIN_URL }),
    );
  });
});
