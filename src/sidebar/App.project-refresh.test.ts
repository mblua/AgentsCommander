// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";
import { createComponent } from "solid-js";
import type {
  AcProjectRefreshReason,
  CatalogReport,
  CodingAgentDefinition,
} from "../shared/types";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../shared/testing/ui-harness";
import { noneSelection } from "../shared/testing/session-selection";
import SidebarApp from "./App";
import { handleProjectRefreshRequested } from "./project-refresh-handler";
import { codingAgentsStore } from "./stores/coding-agents";
import { projectStore } from "./stores/project";

function refreshPayload(reason: AcProjectRefreshReason) {
  return {
    id: "request-1",
    projectPath: "C:\\Users\\Maria\\Project",
    changedPath: null,
    changedName: null,
    reason,
  };
}

describe("handleProjectRefreshRequested", () => {
  it("loads registered projects", () => {
    const store = {
      loadProject: vi.fn(),
      reloadProjectIfLoaded: vi.fn(),
    };

    handleProjectRefreshRequested(refreshPayload("projectRegistered"), store);

    expect(store.loadProject).toHaveBeenCalledWith("C:\\Users\\Maria\\Project");
    expect(store.reloadProjectIfLoaded).not.toHaveBeenCalled();
  });

  it.each([
    "createAgentMatrix",
    "workgroupCreated",
    "workgroupRemoved",
    "teamMembershipChanged",
    "teamMembershipRemoved",
    "futureReason",
  ] as const)("reloads loaded projects for %s", (reason) => {
    const store = {
      loadProject: vi.fn(),
      reloadProjectIfLoaded: vi.fn(),
    };

    handleProjectRefreshRequested(refreshPayload(reason), store);

    expect(store.reloadProjectIfLoaded).toHaveBeenCalledWith("C:\\Users\\Maria\\Project");
    expect(store.loadProject).not.toHaveBeenCalled();
  });
});

// #1966 — App owns the observer that binds the served catalog to the FIRST
// persisted project (the backend's primary). The store already owns generation
// safety, identity comparison and the visible source-changed state, so these
// tests mount the real SidebarApp with the real stores and FakeTransport and
// prove the EFFECT: which head reaches setPrimaryProject after startup, and what
// a head change does to the selectable catalog. No module mock: the previous
// file-level `vi.mock("./stores/project")` is gone, and the handler cases above
// still pass their explicit two-method stub stores.

const REPORT_CMD = "get_coding_agent_catalog_report";
const LIST_CMD = "list_reseedable_agent_commands";

const PROJECT_A = "C:\\repos\\alpha";
const PROJECT_B = "C:\\repos\\bravo";

const catalogKeys = () => codingAgentsStore.catalog().map((definition) => definition.key);

function definition(key: string): CodingAgentDefinition {
  return {
    key,
    label: key,
    description: `preset ${key}`,
    command: key,
    color: "#123456",
    isolatedHome: false,
    removable: true,
    autoUpdate: false,
    envs: [],
    updateCommands: [],
  };
}

function catalogReport(primary: string | null, keys: string[]): CatalogReport {
  return {
    primaryProjectRoot: primary,
    sourcePath: primary === null ? null : `${primary}\\.ac\\coding-agents\\agents.json`,
    catalog: keys.map((key) => definition(key)),
    warnings: [],
    unavailable: null,
  };
}

function tick(): Promise<void> {
  return new Promise<void>((resolve) => setTimeout(resolve, 0));
}

/**
 * The minimum a mounted SidebarApp needs to finish startup: settings, one
 * successful project open per path, discovery, groups, repos, sessions and the
 * project mutations these cases drive.
 */
function resolveSidebarTransport(fake: FakeTransport, projectPaths: string[]): void {
  fake.resolve(
    "get_settings",
    baseSettings({ projectPaths, projectPath: projectPaths[0] ?? null }),
  );
  fake.onInvoke("open_project", (args) => ({
    path: args.path as string,
    registered: true,
    created: false,
  }));
  fake.resolve("discover_project", discovery());
  fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
  fake.resolve("search_repos", []);
  fake.resolve("list_sessions", []);
  fake.resolve("get_active_session", noneSelection());
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
  fake.resolve("remove_project", null);
  fake.resolve("archive_project", null);
  fake.onInvoke("unarchive_project", (args) => ({
    path: args.path as string,
    registered: true,
    created: false,
  }));
}

describe("SidebarApp primary-project catalog invalidation (#1966)", () => {
  let cleanupDom: (() => void) | null = null;
  let fake!: FakeTransport;
  let setPrimary!: MockInstance<typeof codingAgentsStore.setPrimaryProject>;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    // Per-case suite fixtures: the observer is transport-driven, so every case
    // gets a fresh fake and spy. Cases that need another head or ordering
    // override handlers BEFORE mounting; this hook never mounts an App.
    fake = new FakeTransport();
    resolveSidebarTransport(fake, [PROJECT_A, PROJECT_B]);
    fake.resolve(LIST_CMD, []);
    fake.onInvoke(REPORT_CMD, () => catalogReport(PROJECT_A, ["alpha"]));
    setPrimary = vi.spyOn(codingAgentsStore, "setPrimaryProject");
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    vi.restoreAllMocks();
    resetUiStoresForTests();
  });

  /** Mounted App whose cleanup is idempotent, so a failing case still tears it down. */
  function renderSidebar(fake: FakeTransport) {
    const rendered = renderWithFakeTransport(
      () => createComponent(SidebarApp, { embedded: true }),
      fake,
    );
    let disposed = false;
    return {
      root: rendered.root,
      cleanup(): void {
        if (disposed) return;
        disposed = true;
        rendered.cleanup();
      },
    };
  }

  it("follows the authoritative head through remove, unarchive and an archive event", async () => {
    let served: string | null = PROJECT_A;
    // Case-specific override: the report follows the head the test moves.
    fake.onInvoke(REPORT_CMD, () =>
      served === PROJECT_A
        ? catalogReport(PROJECT_A, ["alpha"])
        : served === PROJECT_B
          ? catalogReport(PROJECT_B, ["bravo"])
          : catalogReport(null, []),
    );

    const rendered = renderSidebar(fake);
    try {
      // Startup adopts the head project's catalog.
      await waitFor(() => expect(catalogKeys()).toEqual(["alpha"]));
      expect(codingAgentsStore.loaded()).toBe(true);
      expect(setPrimary.mock.calls.map(([root]) => root)).toEqual([PROJECT_A]);

      // Removing the head promotes B, and the observed identity follows.
      served = PROJECT_B;
      await projectStore.removeProject(PROJECT_A);
      await waitFor(() => expect(catalogKeys()).toEqual(["bravo"]));
      expect(codingAgentsStore.error()).toBeNull();

      // Unarchiving appends BEHIND the head: the observer runs again, the store
      // sees the unchanged identity and does NOT reload the catalog.
      const reportCalls = fake.callsFor(REPORT_CMD).length;
      const generation = codingAgentsStore.generation();
      await projectStore.unarchiveProject(PROJECT_A);
      await waitFor(() => expect(projectStore.projects).toHaveLength(2));
      expect(projectStore.projects[0]?.path).toBe(PROJECT_B);
      await waitFor(() => expect(setPrimary).toHaveBeenCalledTimes(3));
      expect(fake.callsFor(REPORT_CMD)).toHaveLength(reportCalls);
      expect(codingAgentsStore.generation()).toBe(generation);
      expect(catalogKeys()).toEqual(["bravo"]);

      // Archiving B (the App's own backend event) makes A primary again.
      served = PROJECT_A;
      fake.emitFromBackend("project_archive_changed", {
        path: PROJECT_B,
        folderName: "bravo",
        archived: true,
        reason: "archive",
      });
      await waitFor(() => expect(catalogKeys()).toEqual(["alpha"]));
      expect(setPrimary.mock.calls.map(([root]) => root)).toEqual([
        PROJECT_A,
        PROJECT_B,
        PROJECT_B,
        PROJECT_A,
      ]);
    } finally {
      rendered.cleanup();
    }
  });

  it("does not invalidate an early adopted report while startup is still loading projects", async () => {
    let served = PROJECT_B;
    // Case-specific: only A is registered and the report/list follow `served`.
    resolveSidebarTransport(fake, [PROJECT_A]);
    fake.onInvoke(LIST_CMD, () =>
      served === PROJECT_A ? ["alpha"] : ["bravo"],
    );
    fake.onInvoke(REPORT_CMD, () =>
      served === PROJECT_A
        ? catalogReport(PROJECT_A, ["alpha"])
        : catalogReport(PROJECT_B, ["bravo"]),
    );
    // Hold the first project open so App's own initFromSettings cannot complete.
    let releaseOpenProject!: () => void;
    const openProjectGate = new Promise<void>((resolve) => {
      releaseOpenProject = resolve;
    });
    fake.onInvoke("open_project", async (args) => {
      await openProjectGate;
      return { path: args.path as string, registered: true, created: false };
    });

    const rendered = renderSidebar(fake);
    try {
      // Startup is provably mid-flight: the project open is awaited on the gate.
      await waitFor(() => expect(fake.callsFor("open_project")).toHaveLength(1));

      // An early Settings/Onboarding read adopts the catalog it can see.
      await codingAgentsStore.ensureLoaded();
      expect(catalogKeys()).toEqual(["bravo"]);
      expect(codingAgentsStore.loaded()).toBe(true);

      // Startup is still loading: the observer must not treat the empty project
      // list as the no-project identity and reject the adopted report.
      expect(projectStore.projects).toHaveLength(0);
      expect(projectStore.isLoading).toBe(true);
      expect(setPrimary).not.toHaveBeenCalled();
      expect(codingAgentsStore.error()).toBeNull();
      expect(catalogKeys()).toEqual(["bravo"]);

      // The backend settles on A; startup completes and the identity reconciles,
      // clearing the stale selectable state.
      served = PROJECT_A;
      releaseOpenProject();
      await waitFor(() => expect(codingAgentsStore.loaded()).toBe(true));
      await waitFor(() => expect(catalogKeys()).toEqual(["alpha"]));
      expect(setPrimary).toHaveBeenCalledWith(PROJECT_A);
      expect(codingAgentsStore.reseedableCommands()).toEqual(["alpha"]);
      expect(codingAgentsStore.error()).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("claims the no-project identity when the last project is removed", async () => {
    let head: string | null = PROJECT_A;
    // Case-specific: one registered project whose report follows `head`.
    resolveSidebarTransport(fake, [PROJECT_A]);
    fake.onInvoke(REPORT_CMD, () => catalogReport(head, head ? ["alpha"] : []));

    const rendered = renderSidebar(fake);
    try {
      await waitFor(() => expect(catalogKeys()).toEqual(["alpha"]));
      const generation = codingAgentsStore.generation();

      head = null;
      await projectStore.removeProject(PROJECT_A);

      await waitFor(() => expect(codingAgentsStore.generation()).toBeGreaterThan(generation));
      expect(catalogKeys()).toEqual([]);
      expect(codingAgentsStore.loaded()).toBe(true);
      expect(codingAgentsStore.error()).toBeNull();
      expect(codingAgentsStore.sourcePath()).toBeNull();
      expect(setPrimary).toHaveBeenLastCalledWith(null);
    } finally {
      rendered.cleanup();
    }
  });

  it("re-observes an unchanged head without reloading on an unrelated project edit", async () => {
    // Suite defaults: two projects, A serves alpha and never changes.
    const rendered = renderSidebar(fake);
    try {
      await waitFor(() => expect(catalogKeys()).toEqual(["alpha"]));
      const reportCalls = fake.callsFor(REPORT_CMD).length;
      const generation = codingAgentsStore.generation();

      // A discovery-only change on the NON-primary project: the project list
      // moves, the head does not.
      fake.resolve(
        "discover_project",
        discovery({
          agents: [
            { name: "General", path: `${PROJECT_B}\\.ac\\_agent_General`, roleExists: true },
          ],
        }),
      );
      await projectStore.reloadProject(PROJECT_B);

      await waitFor(() => expect(setPrimary).toHaveBeenCalledTimes(2));
      expect(projectStore.projects[0]?.path).toBe(PROJECT_A);
      expect(setPrimary.mock.calls.map(([root]) => root)).toEqual([
        PROJECT_A,
        PROJECT_A,
      ]);
      expect(fake.callsFor(REPORT_CMD)).toHaveLength(reportCalls);
      expect(codingAgentsStore.generation()).toBe(generation);
      expect(catalogKeys()).toEqual(["alpha"]);
    } finally {
      rendered.cleanup();
    }
  });

  it("uses the newest authoritative head when two rapid switches settle out of order", async () => {
    // Case-specific: each report request registers its own resolver, so the test
    // settles them in any order and a fifth request fails the tripwire.
    const resolvers: ((report: CatalogReport) => void)[] = [];
    fake.onInvoke(REPORT_CMD, () =>
      new Promise<CatalogReport>((resolve) => {
        if (resolvers.length >= 4) throw new Error("unexpected extra request");
        resolvers.push(resolve);
      }),
    );

    const rendered = renderSidebar(fake);
    try {
      await waitFor(() => expect(resolvers).toHaveLength(1));
      resolvers[0](catalogReport(PROJECT_A, ["alpha"]));
      await waitFor(() => expect(catalogKeys()).toEqual(["alpha"]));

      // Head A -> B -> (briefly no project) -> A again, each one its own request.
      await projectStore.removeProject(PROJECT_A);
      await projectStore.removeProject(PROJECT_B);
      await projectStore.loadProject(PROJECT_A);
      await waitFor(() => expect(fake.callsFor(REPORT_CMD)).toHaveLength(4));
      expect(resolvers).toHaveLength(4);
      expect(setPrimary.mock.calls.map(([root]) => root)).toEqual([
        PROJECT_A,
        PROJECT_B,
        null,
        PROJECT_A,
      ]);

      // The newest request settles first; the two superseded ones settle after
      // it and must not republish their stale catalogs.
      resolvers[3](catalogReport(PROJECT_A, ["alpha"]));
      await waitFor(() => expect(catalogKeys()).toEqual(["alpha"]));
      resolvers[1](catalogReport(PROJECT_B, ["bravo"]));
      resolvers[2](catalogReport(null, []));
      await tick();
      await tick();

      expect(catalogKeys()).toEqual(["alpha"]);
      expect(codingAgentsStore.loaded()).toBe(true);
      expect(codingAgentsStore.error()).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("surfaces a rejected switch as a diagnostic and recovers through Reload", async () => {
    // Suite defaults until the transport is switched off below.
    const rendered = renderSidebar(fake);
    try {
      await waitFor(() => expect(catalogKeys()).toEqual(["alpha"]));

      // The head changes while the catalog transport is down.
      fake.reject(REPORT_CMD, "catalog transport down");
      await projectStore.removeProject(PROJECT_A);
      await waitFor(() => expect(codingAgentsStore.error()?.code).toBe("transport-error"));
      expect(catalogKeys()).toEqual([]);
      expect(codingAgentsStore.loaded()).toBe(false);

      // User Reload is the bounded recovery: it retries against the recovered
      // backend and adopts the new head's catalog.
      fake.onInvoke(REPORT_CMD, () => catalogReport(PROJECT_B, ["bravo"]));
      await codingAgentsStore.refresh();

      expect(catalogKeys()).toEqual(["bravo"]);
      expect(codingAgentsStore.loaded()).toBe(true);
      expect(codingAgentsStore.error()).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("registers the first project from a project-refresh event and follows it", async () => {
    let head: string | null = null;
    // Case-specific: no projects yet; the report follows the nullable head.
    resolveSidebarTransport(fake, []);
    fake.onInvoke(REPORT_CMD, () => catalogReport(head, head ? ["alpha"] : []));

    const rendered = renderSidebar(fake);
    try {
      // No projects: the no-project identity is real, not an error.
      await waitFor(() => expect(codingAgentsStore.loaded()).toBe(true));
      expect(catalogKeys()).toEqual([]);
      expect(setPrimary.mock.calls.map(([root]) => root)).toEqual([null]);

      head = PROJECT_A;
      fake.emitFromBackend("ac_project_refresh_requested", {
        id: "request-1",
        projectPath: PROJECT_A,
        changedPath: null,
        changedName: null,
        reason: "projectRegistered",
      });

      await waitFor(() => expect(catalogKeys()).toEqual(["alpha"]));
      expect(setPrimary.mock.calls.map(([root]) => root)).toEqual([null, PROJECT_A]);
      expect(codingAgentsStore.error()).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("disposes the observer and its listeners with App", async () => {
    // Suite defaults: this case only needs a mounted, settled App.
    const rendered = renderSidebar(fake);
    try {
      await waitFor(() => expect(catalogKeys()).toEqual(["alpha"]));
      await waitFor(() => expect(projectStore.projects).toHaveLength(2));
      const reportCalls = fake.callsFor(REPORT_CMD).length;
      const generation = codingAgentsStore.generation();

      rendered.cleanup();

      // After disposal the App's archive listener and its effect are gone: the
      // project list cannot move and no catalog invalidation may start.
      fake.emitFromBackend("project_archive_changed", {
        path: PROJECT_B,
        folderName: "bravo",
        archived: true,
        reason: "archive",
      });
      await tick();

      expect(projectStore.projects.map((project) => project.path)).toEqual([
        PROJECT_A,
        PROJECT_B,
      ]);
      expect(fake.callsFor(REPORT_CMD)).toHaveLength(reportCalls);
      expect(codingAgentsStore.generation()).toBe(generation);
      expect(catalogKeys()).toEqual(["alpha"]);
      expect(setPrimary).toHaveBeenCalledTimes(1);
    } finally {
      rendered.cleanup();
    }
  });
});
