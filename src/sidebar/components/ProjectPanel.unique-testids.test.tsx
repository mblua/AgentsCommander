// @vitest-environment jsdom
// #4827 - two projects with equal room and replica names: the replica menu,
// Add to Group flyout, Co-managed item and row ids stay unique in the window.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
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
import { settingsStore } from "../../shared/stores/settings";
import { defaultGroupsConfig, workgroupGroupsStore } from "../stores/workgroup-groups";
import type { AcDiscoveryResult } from "../../shared/types";
import { automationIdPart as AP } from "./replica-repo-badges";

const PROJECTS = ["/p/a", "/p/b"];
const WG = "wg-1-dev-team";
const GROUP = { id: "frontend", name: "Frontend", regex: "^wg-1-" };
const CONTEXTS = ["quick", "selected", "workgroups"];
const wgPath = (proj: string) => `${proj}/.ac/${WG}`;
const devPath = (proj: string) => `${wgPath(proj)}/__agent_dev`;
const workerPath = (proj: string) => `${wgPath(proj)}/__agent_worker`;
const workerSession = (proj: string) => `worker-${AP(proj)}`;
const IN_SCOPE = /^replica\.(inactive\.|row\.|coManaged\.|groups\.|[^.]+(\.[^.]+)*\.groups\.)/;

function projectDiscovery(proj: string): AcDiscoveryResult {
  return discovery({
    workgroups: [
      {
        name: WG,
        path: wgPath(proj),
        task: null,
        taskTitle: "Same room",
        agents: [
          { name: "dev", path: devPath(proj), repoPaths: [`${proj}/repo-x`], isCoordinator: true },
          { name: "worker", path: workerPath(proj), repoPaths: [], isCoordinator: false },
        ],
      },
    ],
  });
}

function inScopeDuplicates(): string[] {
  const seen = new Map<string, number>();
  for (const el of Array.from(document.querySelectorAll("[data-ac-testid]"))) {
    const id = el.getAttribute("data-ac-testid") ?? "";
    if (IN_SCOPE.test(id)) seen.set(id, (seen.get(id) ?? 0) + 1);
  }
  return Array.from(seen).filter(([, n]) => n > 1).map(([id]) => id).sort();
}

const count = (root: ParentNode, id: string) =>
  root.querySelectorAll(`[data-ac-testid="${id}"]`).length;

function only(root: ParentNode, id: string): HTMLElement {
  const all = root.querySelectorAll<HTMLElement>(`[data-ac-testid="${id}"]`);
  if (all.length !== 1) throw new Error(`${id}: expected 1 element, found ${all.length}`);
  return all[0];
}

function panels(): HTMLElement[] {
  return Array.from(document.querySelectorAll<HTMLElement>(".project-panel"));
}

function rowsOf(panel: HTMLElement, ctx: string): HTMLElement[] {
  return Array.from(panel.querySelectorAll<HTMLElement>(`[data-ac-testid^="replica.row.${ctx}."]`))
    .filter((el) => !el.getAttribute("data-ac-testid")!.endsWith("communicationSlot"));
}

const isDev = (el: HTMLElement) => el.getAttribute("data-ac-testid")!.endsWith("dev");

function textButton(root: ParentNode, label: string): HTMLButtonElement {
  const found = Array.from(root.querySelectorAll<HTMLButtonElement>("button")).find(
    (b) => b.textContent?.trim().replace(/^\p{Extended_Pictographic}/u, "") === label,
  );
  if (!found) throw new Error(`button not found: ${label}`);
  return found;
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("#4827 sidebar replica ids are unique per window", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  beforeEach(async () => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    const fake = new FakeTransport();
    fake.onInvoke("new_project", (args) => ({
      path: args.path as string,
      registered: true,
      created: false,
    }));
    fake.onInvoke("discover_project", (args) => projectDiscovery(args.path as string));
    fake.resolve("get_settings", { ...baseSettings(), coManagedEnabled: true });
    fake.resolve("co_managed_get", { enabled: false, catalogPath: null });
    fake.resolve("co_managed_effective_state", { Off: { reason: "RoomFlagOff" } });
    fake.resolve("task_get_title", "Same room");
    fake.resolve("list_unresolved_loop_targets", []);
    fake.onInvoke("update_project_groups", (args) => args.config);
    for (const proj of PROJECTS) {
      workgroupGroupsStore.applyExternalUpdate(proj, { ...defaultGroupsConfig(), groups: [GROUP] });
    }
    sessionsStore.setSessions(
      PROJECTS.map((proj) =>
        session({
          id: workerSession(proj),
          name: `${WG}/worker`,
          workingDirectory: workerPath(proj),
          status: "idle",
        }),
      ),
    );
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    await settingsStore.load();
    for (const proj of PROJECTS) await projectStore.createAndLoad(proj);
    await waitFor(() => expect(panels()).toHaveLength(2));
    sessionsStore.setVisibleActiveIdForTests(workerSession(PROJECTS[0]));
    await waitFor(() => expect(rowsOf(panels()[0], "selected").length).toBeGreaterThan(0));
    sessionsStore.setVisibleActiveIdForTests(workerSession(PROJECTS[1]));
    sessionsStore.setAlwaysShowSelectedWorkgroup(true);
    // Guard: both projects render dev rows in all three contexts, none open.
    await waitFor(() => {
      for (const panel of panels()) {
        for (const ctx of CONTEXTS) {
          const devRows = rowsOf(panel, ctx).filter(isDev);
          expect(devRows.length, `${ctx} rows`).toBeGreaterThan(0);
        }
      }
    });
    expect(document.querySelector(".session-context-menu")).toBeNull();
  });

  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    sessionsStore.setAlwaysShowSelectedWorkgroup(false);
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  function devQuickRow(panel: HTMLElement): HTMLElement {
    const row = rowsOf(panel, "quick").find(isDev);
    if (!row) throw new Error("dev quick row not found");
    return row;
  }

  async function openMenusAndFlyouts(): Promise<void> {
    for (const panel of panels()) {
      contextMenu(devQuickRow(panel));
      await tick();
    }
    await waitFor(() => expect(document.querySelectorAll(".session-context-menu")).toHaveLength(2));
    const triggers = Array.from(document.querySelectorAll<HTMLElement>(".session-context-menu"))
      .map((menu) => {
        const t = Array.from(menu.querySelectorAll<HTMLElement>("button")).find((b) =>
          b.textContent?.includes("Add to Group"),
        );
        if (!t) throw new Error("Add to Group trigger not found");
        return t;
      });
    for (const trigger of triggers) {
      click(trigger);
      await tick();
    }
    await waitFor(() =>
      expect(document.querySelectorAll(".session-context-flyout")).toHaveLength(2),
    );
  }

  async function blankCreateInEachFlyout(): Promise<void> {
    for (const flyout of Array.from(document.querySelectorAll(".session-context-flyout"))) {
      click(textButton(flyout, "Create new group"));
      await tick();
    }
    for (const flyout of Array.from(document.querySelectorAll(".session-context-flyout"))) {
      await waitFor(() => void textButton(flyout, "Create"));
      click(textButton(flyout, "Create"));
      await tick();
    }
    await waitFor(() =>
      expect(document.querySelectorAll(".session-context-flyout .session-context-error"))
        .toHaveLength(2),
    );
  }

  it("no in-scope duplicates: menus + flyouts open, before create", async () => {
    await openMenusAndFlyouts();
    expect(inScopeDuplicates()).toEqual([]);
  });

  it("no in-scope duplicates: after blank-name create", async () => {
    await openMenusAndFlyouts();
    await blankCreateInEachFlyout();
    expect(inScopeDuplicates()).toEqual([]);
  });

  it("each new id matches exactly one element per project", async () => {
    await openMenusAndFlyouts();
    for (const proj of PROJECTS) {
      const rp = AP(devPath(proj));
      const wp = AP(wgPath(proj));
      only(document, `replica.inactive.${rp}.menu.coding-agent`);
      only(document, `replica.inactive.${rp}.menu.repo.0`);
      only(document, `replica.coManaged.inactive.${rp}`);
      for (const leaf of ["trigger", "flyout", "nonstop", AP(GROUP.id), "create"]) {
        only(document, `replica.${wp}.groups.${leaf}`);
      }
    }
    await blankCreateInEachFlyout();
    for (const proj of PROJECTS) {
      const wp = AP(wgPath(proj));
      for (const leaf of ["error", "create.input", "create.save"]) {
        only(document, `replica.${wp}.groups.${leaf}`);
      }
    }
  });

  it("row ids unique across projects in all contexts", () => {
    expect(inScopeDuplicates()).toEqual([]);
    for (const proj of PROJECTS) {
      for (const ctx of CONTEXTS) {
        expect(count(document, `replica.row.${ctx}.${AP(devPath(proj))}`), ctx).toBe(1);
      }
    }
  });
});

// #4841 - row decorations and header blockedMenu badges, same two-project setup.
const DECORATION = new RegExp(
  "^(replica\\.(badges|repoBadge|tierBadge|lockChip|orphanNotice|outdated|contextBadge|micCancel)\\.|"
    + "(workgroup|project|coordinators)\\.header\\.([^.]+\\.)*blockedMenu)",
);
const FAMILIES = [
  "badges", "repoBadge", "tierBadge", "lockChip", "orphanNotice", "outdated", "contextBadge",
];
const devSession = (proj: string) => `dev-${AP(proj)}`;

function decorationDuplicates(): string[] {
  const seen = new Map<string, number>();
  for (const el of Array.from(document.querySelectorAll("[data-ac-testid]"))) {
    const id = el.getAttribute("data-ac-testid") ?? "";
    if (DECORATION.test(id)) seen.set(id, (seen.get(id) ?? 0) + 1);
  }
  return Array.from(seen).filter(([, n]) => n > 1).map(([id]) => id).sort();
}

function decorationDiscovery(proj: string): AcDiscoveryResult {
  const result = projectDiscovery(proj);
  result.workgroups[0].agents[0].selectionState = "locked";
  return result;
}

describe("#4841 row decorations and header badges are unique per window", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  beforeEach(async () => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    sessionsStore.resetOrphanNoticesForTests();
    const fake = new FakeTransport();
    fake.onInvoke("new_project", (args) => ({
      path: args.path as string,
      registered: true,
      created: false,
    }));
    fake.onInvoke("discover_project", (args) => decorationDiscovery(args.path as string));
    fake.resolve("get_settings", baseSettings({
      agents: [{
        id: "codex",
        label: "Codex",
        command: "codex",
        color: "#d97757",
        envs: [],
        isolatedHome: false,
        contextRegex: String.raw`Context (\d{1,3})%`,
      }],
    }));
    fake.resolve("task_get_title", "Same room");
    fake.resolve("list_unresolved_loop_targets", []);
    sessionsStore.setSessions(
      PROJECTS.map((proj) =>
        session({
          id: devSession(proj),
          name: `${WG}/dev`,
          workingDirectory: devPath(proj),
          isCoordinator: true,
          status: "running",
          agentId: "codex",
          agentLabel: "Codex",
          requestedProfile: "B",
          effectiveProfile: "B",
          profileFallbackApplied: false,
          matchTier: "labelAndLetter",
          profileOutdated: true,
          communication: {
            kind: "blockedMenu",
            visible: true,
            updatedAt: "2026-11-21T00:00:00Z",
            message: "Pick one",
          },
        }),
      ),
    );
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    await settingsStore.load();
    for (const proj of PROJECTS) await projectStore.createAndLoad(proj);
    await waitFor(() => expect(panels()).toHaveLength(2));
    sessionsStore.setVisibleActiveIdForTests(devSession(PROJECTS[0]));
    await waitFor(() => expect(rowsOf(panels()[0], "selected").length).toBeGreaterThan(0));
    sessionsStore.setVisibleActiveIdForTests(devSession(PROJECTS[1]));
    sessionsStore.setAlwaysShowSelectedWorkgroup(true);
    // Guard: every family renders in both panels, plus 4 header badges each.
    await waitFor(() => {
      for (const panel of panels()) {
        for (const fam of FAMILIES) {
          const found = panel.querySelectorAll(`[data-ac-testid^="replica.${fam}."]`);
          expect(found.length, fam).toBeGreaterThan(0);
        }
        expect(panel.querySelectorAll(".ac-wg-header-blocked-menu")).toHaveLength(4);
      }
    });
  });

  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    sessionsStore.setAlwaysShowSelectedWorkgroup(false);
    sessionsStore.resetOrphanNoticesForTests();
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  it("no duplicate decoration or header badge ids across projects", () => {
    expect(decorationDuplicates()).toEqual([]);
  });

  it("each new id matches one element per project", () => {
    const single = ["badges", "tierBadge", "lockChip", "orphanNotice", "outdated", "contextBadge"];
    for (const proj of PROJECTS) {
      const rp = AP(devPath(proj));
      for (const ctx of CONTEXTS) {
        for (const fam of single) only(document, `replica.${fam}.${ctx}.${rp}`);
        only(document, `replica.repoBadge.${ctx}.${rp}.0.x`);
        only(document, `replica.orphanNotice.${ctx}.${rp}.dismiss`);
      }
      for (const ctx of ["selected", "workgroups"]) {
        only(document, `workgroup.header.blockedMenu.${ctx}.${AP(wgPath(proj))}`);
      }
      only(document, `project.header.${AP(proj)}.blockedMenu`);
      only(document, `coordinators.header.${AP(proj)}.blockedMenu`);
    }
  });
});
