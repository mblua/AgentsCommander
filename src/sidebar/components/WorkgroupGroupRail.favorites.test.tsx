// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AcWorkgroup, Session, WorkgroupGroupsConfig } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  contextMenu,
  discovery,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore, type ProjectState } from "../stores/project";
import { projectCollapseStore } from "../stores/project-collapse";
import { sessionsStore } from "../stores/sessions";
import {
  defaultGroupsConfig,
  defaultNonStop,
  exactGroupRegexForWorkgroup,
} from "../stores/workgroup-groups";
import WorkgroupGroupRail from "./WorkgroupGroupRail";

const projectPath = "C:\\Project";
const otherProjectPath = "C:\\OtherProject";

function wg(name: string, root = projectPath): AcWorkgroup {
  return {
    name,
    path: `${root}\\.ac\\${name}`,
    task: null,
    taskTitle: null,
    agents: [
      {
        name: "dev-webpage-ui",
        path: `${root}\\.ac\\${name}\\__agent_dev-webpage-ui`,
        repoPaths: [],
        isCoordinator: true,
      },
    ],
  };
}

function project(): ProjectState {
  return {
    path: projectPath,
    folderName: "Project",
    workgroups: [wg("wg-1-dev-team"), wg("wg-2-rust-team"), wg("wg-3-docs-team")],
    agents: [],
    teams: [],
    loops: [],
    contextTemplateUpdates: [],
  };
}

/** A session that makes the given workgroup "working" (running dot). */
function replicaSession(wgName: string): Session {
  return session({
    id: `session-${wgName}`,
    name: `${wgName}/dev-webpage-ui`,
    workingDirectory: `${projectPath}\\.ac\\${wgName}\\__agent_dev-webpage-ui`,
    status: "running",
  });
}

function groupsConfig(overrides: Partial<WorkgroupGroupsConfig> = {}): WorkgroupGroupsConfig {
  return {
    ...defaultGroupsConfig(),
    ...overrides,
    groups: overrides.groups ?? [
      { id: "ui", name: "UI", regex: exactGroupRegexForWorkgroup("wg-1-dev-team") },
      { id: "rust", name: "Rust", regex: exactGroupRegexForWorkgroup("wg-2-rust-team") },
      { id: "docs", name: "Docs", regex: exactGroupRegexForWorkgroup("wg-3-docs-team") },
    ],
  };
}

function target<T extends Element>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing element ${testId}`);
  return element;
}

function maybeTarget<T extends Element>(testId: string): T | null {
  return document.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function railButtonOrder(): string[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>('[data-ac-testid^="workgroupGroups.button."]')
  ).map((button) => button.dataset.acTestid!.replace("workgroupGroups.button.", ""));
}

function railDotKeys(): string[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>('[data-ac-testid^="workgroupGroups.dot."]')
  ).map((dot) => dot.dataset.acTestid!.replace("workgroupGroups.dot.", ""));
}

function favoriteButtonKeys(): string[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>('[data-ac-testid^="workgroupGroups.favoriteButton."]')
  ).map((button) => button.dataset.acTestid!.replace("workgroupGroups.favoriteButton.", ""));
}

function lastUpdatedGroupIds(fake: FakeTransport): string[] | undefined {
  const config = fake.lastCall("update_project_groups")?.args.config as WorkgroupGroupsConfig | undefined;
  return config?.groups.map((group) => group.id);
}

function pointer(
  el: Element | Window,
  type: "pointerdown" | "pointermove" | "pointerup",
  init: { clientY?: number; pointerId?: number } = {}
): void {
  const event = new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    clientX: 10,
    clientY: init.clientY ?? 10,
    button: 0,
  });
  Object.defineProperty(event, "pointerId", { value: init.pointerId ?? 1 });
  Object.defineProperty(event, "pointerType", { value: "mouse" });
  Object.defineProperty(event, "isPrimary", { value: true });
  el.dispatchEvent(event);
}

function mockRect(el: HTMLElement, top: number, height = 30): void {
  vi.spyOn(el, "getBoundingClientRect").mockReturnValue({
    x: 0,
    y: top,
    top,
    left: 0,
    right: 68,
    bottom: top + height,
    width: 68,
    height,
    toJSON: () => ({}),
  } as DOMRect);
}

/** Right-click `el` and wait for the hoisted rail context menu to render. */
async function openMenuOn(el: Element): Promise<void> {
  contextMenu(el);
  await waitFor(() => expect(target("workgroupGroups.contextMenu")).toBeTruthy());
}

/** `get_project_groups` is invoked per project path, so a flat `resolve()` would
 *  hand OtherProject the SAME groups (and the same favorites) as Project. Key the
 *  handler on `args.path`; every project other than the main one gets the empty
 *  default config. */
function railFake(mainConfig: WorkgroupGroupsConfig = groupsConfig()): FakeTransport {
  const fake = new FakeTransport();
  fake.onInvoke("get_project_groups", (args) =>
    args.path === projectPath ? mainConfig : defaultGroupsConfig()
  );
  fake.onInvoke("update_project_groups", (args) => args.config);
  fake.resolve("set_rail_collapse", null);
  fake.resolve("list_archived_projects", []);
  return fake;
}

/** Seed `projectStore`, which is what `selectFromRail` reads for the #810
 *  auto-focus (`collapseAllExceptKnown(owner, projectStore.projects…)`) — NOT the
 *  `projects` prop. Without this the auto-focus list is empty and collapse-others
 *  silently no-ops. */
async function seedProjectStore(fake: FakeTransport, paths: string[]): Promise<void> {
  fake.onInvoke("new_project", (args) => ({ path: args.path as string }));
  fake.onInvoke("discover_project", (args) => {
    const path = args.path as string;
    return discovery({
      workgroups:
        path === projectPath
          ? [wg("wg-1-dev-team"), wg("wg-2-rust-team"), wg("wg-3-docs-team")]
          : [wg("wg-9-ops-team", path)],
    });
  });
  for (const path of paths) await projectStore.createAndLoad(path);
}

describe("WorkgroupGroupRail favorites + collapsible rail (#965)", () => {
  beforeEach(() => {
    resetUiStoresForTests();
  });

  afterEach(() => {
    resetUiStoresForTests();
    document.body.replaceChildren();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  describe("Option B: a group click never touches rail collapse (RC-2)", () => {
    // THE B REGRESSION TEST. Never delete it. This is the user's ruling, encoded:
    // the rail folds ONLY on an explicit header click, while the #810 auto-focus
    // keeps driving the ProjectPanel exactly as before.
    it("issues no set_rail_collapse and leaves every header expanded, while the ProjectPanel auto-focus still fires", async () => {
      const fake = railFake();
      const rendered = renderWithFakeTransport(
        () => <WorkgroupGroupRail projects={projectStore.projects} />,
        fake
      );
      try {
        await seedProjectStore(fake, [projectPath, otherProjectPath]);
        await waitFor(() => expect(target("workgroupGroups.button.ui")).toBeTruthy());

        click(target("workgroupGroups.button.ui"));

        // The rail did not fold, and nothing was persisted.
        expect(fake.callsFor("set_rail_collapse")).toEqual([]);
        expect(target("workgroupGroups.projectLabel.Project").getAttribute("aria-expanded")).toBe("true");
        expect(target("workgroupGroups.projectLabel.OtherProject").getAttribute("aria-expanded")).toBe(
          "true"
        );
        expect(target("workgroupGroups.button.ui")).toBeTruthy();

        // ...but the ProjectPanel's #810 auto-focus is untouched: it still collapses
        // the other project's CARD and expands the owner's.
        expect(projectCollapseStore.isProjectCollapsed(otherProjectPath)).toBe(true);
        expect(projectCollapseStore.isProjectCollapsed(projectPath)).toBe(false);
      } finally {
        rendered.cleanup();
      }
    });
  });

  describe("collapsible headers", () => {
    it("folds a project section on header click, persists the snapshot, and restores aria-expanded", async () => {
      const fake = railFake();
      const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
      try {
        await waitFor(() => expect(target("workgroupGroups.button.ui")).toBeTruthy());
        const header = target<HTMLElement>("workgroupGroups.projectLabel.Project");
        expect(header.getAttribute("aria-expanded")).toBe("true");

        click(header);

        await waitFor(() => expect(maybeTarget("workgroupGroups.button.ui")).toBeNull());
        expect(header.getAttribute("aria-expanded")).toBe("false");
        expect(fake.lastCall("set_rail_collapse")?.args).toEqual({
          collapsedProjects: ["c:/project"],
          favoritesCollapsed: false,
        });

        click(header);

        await waitFor(() => expect(maybeTarget("workgroupGroups.button.ui")).toBeTruthy());
        expect(header.getAttribute("aria-expanded")).toBe("true");
        expect(fake.lastCall("set_rail_collapse")?.args).toEqual({
          collapsedProjects: [],
          favoritesCollapsed: false,
        });
      } finally {
        rendered.cleanup();
      }
    });

    it("keeps the config error badge visible while the section is collapsed", async () => {
      const fake = railFake();
      fake.reject("get_project_groups", "disk denied");
      const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
      try {
        await waitFor(() =>
          expect(document.querySelector(".workgroup-group-rail-error")).toBeTruthy()
        );

        click(target("workgroupGroups.projectLabel.Project"));

        // An error badge that hides itself is a bug.
        await waitFor(() =>
          expect(target("workgroupGroups.projectLabel.Project").getAttribute("aria-expanded")).toBe(
            "false"
          )
        );
        expect(document.querySelector(".workgroup-group-rail-error")).toBeTruthy();
      } finally {
        rendered.cleanup();
      }
    });

    it("folds the Favorites section on its header click and persists the bit", async () => {
      const fake = railFake(
        groupsConfig({
          groups: [
            { id: "ui", name: "UI", regex: exactGroupRegexForWorkgroup("wg-1-dev-team"), favorite: true },
            { id: "rust", name: "Rust", regex: exactGroupRegexForWorkgroup("wg-2-rust-team") },
          ],
        })
      );
      const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
      try {
        await waitFor(() => expect(favoriteButtonKeys()).toEqual(["Project.ui"]));
        const header = target<HTMLElement>("workgroupGroups.favorites.header");
        expect(header.getAttribute("aria-expanded")).toBe("true");

        click(header);

        await waitFor(() => expect(favoriteButtonKeys()).toEqual([]));
        expect(header.getAttribute("aria-expanded")).toBe("false");
        // The section header stays, so the user can unfold it again.
        expect(target("workgroupGroups.favorites")).toBeTruthy();
        expect(fake.lastCall("set_rail_collapse")?.args).toEqual({
          collapsedProjects: [],
          favoritesCollapsed: true,
        });
      } finally {
        rendered.cleanup();
      }
    });
  });

  describe("favorite / unfavorite via the context menu", () => {
    it("offers Favorite on a user group, renders it in Favorites, and KEEPS it in its project section", async () => {
      const fake = railFake();
      const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
      try {
        await waitFor(() => expect(target("workgroupGroups.button.rust")).toBeTruthy());
        expect(maybeTarget("workgroupGroups.favorites")).toBeNull();

        await openMenuOn(target("workgroupGroups.button.rust"));
        expect(target("workgroupGroups.contextMenu.favorite").textContent).toContain("Favorite");

        click(target("workgroupGroups.contextMenu.favorite"));

        await waitFor(() => expect(favoriteButtonKeys()).toEqual(["Project.rust"]));
        // The duplication is the point: a favorite shows in BOTH places. Filtering it
        // out of the project section would desync the reorder index math (§4.7).
        expect(target("workgroupGroups.button.rust")).toBeTruthy();
        expect(lastUpdatedGroupIds(fake)).toEqual(["ui", "rust", "docs"]);
        const saved = fake.lastCall("update_project_groups")?.args.config as WorkgroupGroupsConfig;
        expect(saved.groups.find((group) => group.id === "rust")?.favorite).toBe(true);
      } finally {
        rendered.cleanup();
      }
    });

    it("offers Unfavorite on a favorited group and removes it from Favorites", async () => {
      const fake = railFake(
        groupsConfig({
          groups: [
            { id: "ui", name: "UI", regex: exactGroupRegexForWorkgroup("wg-1-dev-team"), favorite: true },
            { id: "rust", name: "Rust", regex: exactGroupRegexForWorkgroup("wg-2-rust-team") },
          ],
        })
      );
      const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
      try {
        await waitFor(() => expect(favoriteButtonKeys()).toEqual(["Project.ui"]));

        await openMenuOn(target("workgroupGroups.button.ui"));
        expect(target("workgroupGroups.contextMenu.favorite").textContent).toContain("Unfavorite");

        click(target("workgroupGroups.contextMenu.favorite"));

        await waitFor(() => expect(maybeTarget("workgroupGroups.favorites")).toBeNull());
        const saved = fake.lastCall("update_project_groups")?.args.config as WorkgroupGroupsConfig;
        expect(saved.groups.find((group) => group.id === "ui")?.favorite).toBe(false);
      } finally {
        rendered.cleanup();
      }
    });

    it("can unfavorite from the Favorites entry itself", async () => {
      const fake = railFake(
        groupsConfig({
          groups: [
            { id: "ui", name: "UI", regex: exactGroupRegexForWorkgroup("wg-1-dev-team"), favorite: true },
          ],
        })
      );
      const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
      try {
        await waitFor(() => expect(favoriteButtonKeys()).toEqual(["Project.ui"]));

        await openMenuOn(target("workgroupGroups.favoriteButton.Project.ui"));
        expect(target("workgroupGroups.contextMenu.favorite").textContent).toContain("Unfavorite");

        click(target("workgroupGroups.contextMenu.favorite"));

        await waitFor(() => expect(maybeTarget("workgroupGroups.favorites")).toBeNull());
      } finally {
        rendered.cleanup();
      }
    });

    it("exposes no favorite option on the pseudo-entries (All / Ungrouped / Alert me!)", async () => {
      const fake = railFake(groupsConfig({ nonStop: { ...defaultNonStop(), show: true } }));
      const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
      try {
        await waitFor(() => expect(target("workgroupGroups.button.nonstop")).toBeTruthy());

        for (const key of ["all", "ungrouped", "nonstop"]) {
          await openMenuOn(target(`workgroupGroups.button.${key}`));
          expect(target("workgroupGroups.contextMenu.edit")).toBeTruthy();
          expect(maybeTarget("workgroupGroups.contextMenu.favorite")).toBeNull();
        }

        // ...and the project header is a project target too: Edit only.
        await openMenuOn(target("workgroupGroups.projectLabel.Project"));
        expect(maybeTarget("workgroupGroups.contextMenu.favorite")).toBeNull();
      } finally {
        rendered.cleanup();
      }
    });
  });

  describe("a favorite of a collapsed project", () => {
    it("stays visible and stays clickable while its project section is folded", async () => {
      const fake = railFake(
        groupsConfig({
          groups: [
            { id: "ui", name: "UI", regex: exactGroupRegexForWorkgroup("wg-1-dev-team"), favorite: true },
            { id: "rust", name: "Rust", regex: exactGroupRegexForWorkgroup("wg-2-rust-team") },
          ],
        })
      );
      const rendered = renderWithFakeTransport(
        () => <WorkgroupGroupRail projects={projectStore.projects} />,
        fake
      );
      try {
        await seedProjectStore(fake, [projectPath, otherProjectPath]);
        await waitFor(() => expect(favoriteButtonKeys()).toEqual(["Project.ui"]));

        click(target("workgroupGroups.projectLabel.Project"));
        await waitFor(() => expect(maybeTarget("workgroupGroups.button.ui")).toBeNull());

        // This is the whole point of Option B plus Favorites: the group is reachable
        // in one click even though its project section is folded.
        const favorite = target<HTMLElement>("workgroupGroups.favoriteButton.Project.ui");
        expect(favorite).toBeTruthy();

        click(favorite);

        expect(projectCollapseStore.isProjectCollapsed(otherProjectPath)).toBe(true);
        expect(favorite.getAttribute("aria-pressed")).toBe("true");
        // Clicking a favorite does NOT unfold the rail section (RC-2).
        expect(maybeTarget("workgroupGroups.button.ui")).toBeNull();
        expect(target("workgroupGroups.projectLabel.Project").getAttribute("aria-expanded")).toBe(
          "false"
        );
      } finally {
        rendered.cleanup();
      }
    });
  });

  describe("testid namespaces (the automation bridge THROWS on duplicates)", () => {
    it("keeps the project-section testid namespaces byte-identical when a group is favorited", async () => {
      const fake = railFake();
      sessionsStore.setSessions([replicaSession("wg-1-dev-team")]);
      const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
      try {
        await waitFor(() => expect(target("workgroupGroups.button.ui")).toBeTruthy());
        const orderBefore = railButtonOrder();
        const dotsBefore = railDotKeys();
        expect(orderBefore).toEqual(["all", "ungrouped", "ui", "rust", "docs"]);
        expect(dotsBefore).toContain("ui");

        await openMenuOn(target("workgroupGroups.button.ui"));
        click(target("workgroupGroups.contextMenu.favorite"));
        await waitFor(() => expect(favoriteButtonKeys()).toEqual(["Project.ui"]));

        // The favorited group now renders TWICE, but the project-section prefixes are
        // untouched: no duplicate ids leak into `^workgroupGroups.button.` or
        // `^workgroupGroups.dot.`, which is what `railButtonOrder()` / `railDots()`
        // scan and what the automation bridge resolves against.
        expect(railButtonOrder()).toEqual(orderBefore);
        expect(railDotKeys()).toEqual(dotsBefore);
        expect(document.querySelectorAll('[data-ac-testid="workgroupGroups.button.ui"]')).toHaveLength(1);
        expect(
          document.querySelectorAll('[data-ac-testid="workgroupGroups.favoriteDot.Project.ui"]')
        ).toHaveLength(1);
      } finally {
        rendered.cleanup();
      }
    });
  });

  describe("reorder safety", () => {
    // THE F2 CORRUPTION TEST. Never delete it. `targetIndexForPointer` ends
    // `return candidates.length`, so with every candidate gone it returns 0 — "drop
    // at the top", not "nowhere". A drag in flight whose buttons unmount would splice
    // the group to the front and PERSIST it.
    it("does not reorder when the header is clicked mid-drag (G3 cancels the gesture)", async () => {
      const fake = railFake();
      const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
      try {
        await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust", "docs"]));
        const rail = target<HTMLElement>("workgroupGroups.rail.Project");
        const ui = target<HTMLElement>("workgroupGroups.button.ui");
        const rust = target<HTMLElement>("workgroupGroups.button.rust");
        const docs = target<HTMLElement>("workgroupGroups.button.docs");
        mockRect(rail, 0, 200);
        mockRect(ui, 80);
        mockRect(rust, 114);
        mockRect(docs, 148);

        vi.useFakeTimers();
        pointer(docs, "pointerdown", { clientY: 158 });
        vi.advanceTimersByTime(2000); // past REORDER_HOLD_MS -> dragging
        expect(rail.classList.contains("reorder-active")).toBe(true);

        click(target("workgroupGroups.projectLabel.Project")); // fold mid-gesture

        // ISOLATES G3, and it has to be asserted HERE, before the pointerup.
        // `reorder-active` is bound to phase === "dragging" || "saving", so it is
        // false only if cancelReorder() actually ran in the header's onClick.
        //
        // Without this line the test does NOT pin G3: `mockRect` installs its spy on
        // the element OBJECT, so the folded-away siblings keep reporting height 30,
        // the candidate list never empties, G1 cannot fire, and G2 silently catches
        // the drop instead — the test stays green with G3 deleted. Same mirage the
        // G1 test below sidesteps by mocking the siblings to zero height.
        expect(rail.classList.contains("reorder-active")).toBe(false);

        pointer(window, "pointerup", { clientY: 84 });
        vi.useRealTimers();

        await waitFor(() =>
          expect(target("workgroupGroups.projectLabel.Project").getAttribute("aria-expanded")).toBe(
            "false"
          )
        );
        expect(fake.callsFor("update_project_groups")).toEqual([]);
      } finally {
        rendered.cleanup();
      }
    });

    // G1 in isolation: candidates empty while the section is still expanded (every
    // sibling button measures zero-height, exactly as a detached ref does). Without
    // `if (candidates.length === 0) return null`, this reorders `docs` to index 0.
    it("does not reorder when every drop candidate has collapsed away (G1)", async () => {
      const fake = railFake();
      const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
      try {
        await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust", "docs"]));
        const rail = target<HTMLElement>("workgroupGroups.rail.Project");
        const ui = target<HTMLElement>("workgroupGroups.button.ui");
        const rust = target<HTMLElement>("workgroupGroups.button.rust");
        const docs = target<HTMLElement>("workgroupGroups.button.docs");
        mockRect(rail, 0, 200);
        mockRect(ui, 80, 0); // zero-height == detached, filtered out of the candidates
        mockRect(rust, 114, 0);
        mockRect(docs, 148);

        vi.useFakeTimers();
        pointer(docs, "pointerdown", { clientY: 158 });
        vi.advanceTimersByTime(2000);
        pointer(window, "pointermove", { clientY: 84 });
        pointer(window, "pointerup", { clientY: 84 });
        vi.useRealTimers();

        expect(fake.callsFor("update_project_groups")).toEqual([]);
        expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust", "docs"]);
      } finally {
        rendered.cleanup();
      }
    });

    // F9: the project section must NOT filter favorites out. The drop-target math
    // counts RENDERED buttons while `reorderGroup` writes into the UNFILTERED
    // `config.groups`; filtering would desync them and scramble the order.
    it("persists the rendered order when a middle group is favorited", async () => {
      const fake = railFake(
        groupsConfig({
          groups: [
            { id: "ui", name: "UI", regex: exactGroupRegexForWorkgroup("wg-1-dev-team") },
            { id: "rust", name: "Rust", regex: exactGroupRegexForWorkgroup("wg-2-rust-team"), favorite: true },
            { id: "docs", name: "Docs", regex: exactGroupRegexForWorkgroup("wg-3-docs-team") },
          ],
        })
      );
      const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
      try {
        await waitFor(() => expect(favoriteButtonKeys()).toEqual(["Project.rust"]));
        expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust", "docs"]);

        const rail = target<HTMLElement>("workgroupGroups.rail.Project");
        const ui = target<HTMLElement>("workgroupGroups.button.ui");
        const rust = target<HTMLElement>("workgroupGroups.button.rust");
        const docs = target<HTMLElement>("workgroupGroups.button.docs");
        mockRect(rail, 0, 200);
        mockRect(ui, 80);
        mockRect(rust, 114);
        mockRect(docs, 148);

        vi.useFakeTimers();
        pointer(docs, "pointerdown", { clientY: 158 });
        vi.advanceTimersByTime(2000);
        pointer(window, "pointermove", { clientY: 84 });
        pointer(window, "pointerup", { clientY: 84 });
        vi.useRealTimers();

        // Rendered order and persisted order agree, despite `rust` also rendering
        // up in Favorites.
        await waitFor(() => expect(lastUpdatedGroupIds(fake)).toEqual(["docs", "ui", "rust"]));
        await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "docs", "ui", "rust"]));
      } finally {
        rendered.cleanup();
      }
    });

    it("never arms a drag on a Favorites entry", async () => {
      const fake = railFake(
        groupsConfig({
          groups: [
            { id: "ui", name: "UI", regex: exactGroupRegexForWorkgroup("wg-1-dev-team"), favorite: true },
            { id: "rust", name: "Rust", regex: exactGroupRegexForWorkgroup("wg-2-rust-team") },
          ],
        })
      );
      const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
      try {
        await waitFor(() => expect(favoriteButtonKeys()).toEqual(["Project.ui"]));
        const favorite = target<HTMLElement>("workgroupGroups.favoriteButton.Project.ui");
        expect(favorite.classList.contains("reorderable")).toBe(false);

        vi.useFakeTimers();
        pointer(favorite, "pointerdown", { clientY: 20 });
        vi.advanceTimersByTime(2000);
        vi.useRealTimers();

        // No pointer handlers are bound at all, so the hold cannot arm a gesture.
        expect(favorite.classList.contains("reorder-arming")).toBe(false);
        expect(favorite.classList.contains("reorder-dragging")).toBe(false);
        expect(fake.callsFor("update_project_groups")).toEqual([]);
      } finally {
        rendered.cleanup();
      }
    });
  });

  describe("tooltip", () => {
    it("puts the owning project's folderName on the first line of every rail button", async () => {
      const fake = railFake(
        groupsConfig({
          groups: [
            { id: "ui", name: "UI", regex: exactGroupRegexForWorkgroup("wg-1-dev-team"), favorite: true },
          ],
        })
      );
      sessionsStore.setSessions([replicaSession("wg-1-dev-team")]);
      const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
      try {
        await waitFor(() => expect(favoriteButtonKeys()).toEqual(["Project.ui"]));

        const projectButton = target<HTMLElement>("workgroupGroups.button.ui");
        const favoriteButton = target<HTMLElement>("workgroupGroups.favoriteButton.Project.ui");

        // The rail is 68px and the header ellipsizes, so the tooltip is the only place
        // the full project identity can live — and it is what identifies a favorite's
        // owning project without a visible label.
        expect(projectButton.title.split("\n")[0]).toBe("Project");
        expect(favoriteButton.title.split("\n")[0]).toBe("Project");
        // The running-agent lines are preserved beneath it.
        expect(favoriteButton.title).toContain("WG1:(dev-webpage-ui)");
        expect(target<HTMLElement>("workgroupGroups.button.all").title).toBe(
          "Project\nWG1:(dev-webpage-ui)"
        );
      } finally {
        rendered.cleanup();
      }
    });
  });
});
