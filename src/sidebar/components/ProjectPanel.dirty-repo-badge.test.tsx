// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { replicaVolatileStore } from "../stores/replica-volatile";

// #1028/#1078 - the repo badge's letters go red when that repo has local work not
// confirmed by cached origin tracking: a dirty worktree, but also a clean local-ahead
// commit, a missing/gone origin target, or a detached HEAD.
//
// These render the real ProjectPanel through the FakeTransport harness and assert
// the DOM, per the frontend-visual-verification discipline. They cover the CLASS and
// the TITLE; they deliberately do NOT claim to cover the COLOUR. jsdom does not
// evaluate the stylesheet, so a misspelled rule, a wrong property, or losing the
// specificity contest would leave every assertion in this file green and the badge
// violet on screen. AC 1 and AC 2 are closed by a real visual check, not by this file.
//
// The rows here are DORMANT - nothing in this file creates a session - which is AC 4:
// Gate A polls every replica whether or not a session exists, so a dormant row's
// dirtiness was already being computed and thrown away.

const projectPath = "C:\\Project";
const workgroupPath = `${projectPath}\\.ac\\wg-2-dev-team`;
const coordPath = `${workgroupPath}\\__agent_dev-webpage-ui`;
const memberPath = `${workgroupPath}\\__agent_dev-rust`;
const REPO_A = `${workgroupPath}\\repo-AgentsCommander`;
const REPO_B = `${workgroupPath}\\repo-webpage`;

/** A coordinator row renders TWICE: once in the workgroup tree (rowContext
 *  "workgroups") and once in the coord-quick-access strip (rowContext "quick"). Both
 *  go through the same renderReplicaItem, so tests target one context explicitly
 *  rather than counting badges document-wide. */
const TREE = "workgroups";
const QUICK = "quick";

/** The badge's own data-ac-testid: replica.repoBadge.<ctx>.<wg>.<replica>.<i>.<label> */
function badgeIn(
  root: Element,
  ctx: string,
  replicaName: string,
  index: number,
  label: string
): HTMLElement | null {
  return root.querySelector<HTMLElement>(
    `[data-ac-testid="replica.repoBadge.${ctx}.wg-2-dev-team.${replicaName}.${index}.${label}"]`
  );
}

/** A dormant coordinator plus a dormant non-coordinator, both carrying repo paths. */
function repoDiscovery(repoPaths: string[]) {
  return discovery({
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: workgroupPath,
        task: null,
        taskTitle: "Dirty badge",
        agents: [
          {
            name: "dev-webpage-ui",
            path: coordPath,
            repoPaths,
            isCoordinator: true,
          },
          {
            name: "dev-rust",
            path: memberPath,
            repoPaths,
            isCoordinator: false,
          },
        ],
      },
    ],
  });
}

async function renderDormantRepoRow(fake: FakeTransport, repoPaths: string[]) {
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("get_settings", baseSettings());
  fake.resolve("discover_project", repoDiscovery(repoPaths));

  const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
  await projectStore.createAndLoad(projectPath);
  await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));
  return rendered;
}

describe("ProjectPanel dirty repo badge (#1028)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("adds the dirty class to a DORMANT coordinator's badge when the repo is dirty (AC 4)", async () => {
    const fake = new FakeTransport();
    const rendered = await renderDormantRepoRow(fake, [REPO_A]);
    try {
      // Before any event lands: no dirty class, and the tooltip says why.
      await waitFor(() =>
        expect(badgeIn(rendered.root, TREE, "dev-webpage-ui", 0, "AgentsCommander")).not.toBeNull()
      );
      const before = badgeIn(rendered.root, TREE, "dev-webpage-ui", 0, "AgentsCommander")!;
      expect(before.classList.contains("dirty")).toBe(false);
      expect(before.title).toBe(`${REPO_A} (status unknown)`);

      // A Gate A tick for a replica with NO session: the badge still goes red.
      replicaVolatileStore.applyDiscoveryBranchUpdate(coordPath, "main", [REPO_A], ["main"], [
        true,
      ]);

      await waitFor(() => {
        const badge = badgeIn(rendered.root, TREE, "dev-webpage-ui", 0, "AgentsCommander")!;
        expect(badge.classList.contains("dirty")).toBe(true);
        // ADDITIVE variant: `.branch` must survive alongside it, or the badge loses
        // its background and the `.branch.dirty` rule stops matching at all.
        expect(badge.className).toBe("ac-discovery-badge branch dirty");
        expect(badge.title).toBe(`${REPO_A} (local work not confirmed by cached origin tracking)`);
        expect(badge.textContent).toBe("AgentsCommander/main");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("reddens the coordinator's quick-access copy from the same event", async () => {
    // The coordinator row renders in two places off one code path. Pinned so a future
    // refactor that splits them cannot redden one and leave the other violet.
    const fake = new FakeTransport();
    const rendered = await renderDormantRepoRow(fake, [REPO_A]);
    try {
      replicaVolatileStore.applyDiscoveryBranchUpdate(coordPath, "main", [REPO_A], ["main"], [
        true,
      ]);

      await waitFor(() => {
        const quick = badgeIn(rendered.root, QUICK, "dev-webpage-ui", 0, "AgentsCommander");
        expect(quick).not.toBeNull();
        expect(quick!.classList.contains("dirty")).toBe(true);
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("leaves a clean repo without the dirty class, and its title untouched (AC 2)", async () => {
    const fake = new FakeTransport();
    const rendered = await renderDormantRepoRow(fake, [REPO_A]);
    try {
      replicaVolatileStore.applyDiscoveryBranchUpdate(coordPath, "main", [REPO_A], ["main"], [
        false,
      ]);

      await waitFor(() => {
        const badge = badgeIn(rendered.root, TREE, "dev-webpage-ui", 0, "AgentsCommander")!;
        expect(badge.textContent).toBe("AgentsCommander/main");
        expect(badge.classList.contains("dirty")).toBe(false);
        expect(badge.className).toBe("ac-discovery-badge branch");
        // A detected-clean repo keeps the bare path, exactly as before #1028.
        expect(badge.title).toBe(REPO_A);
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("keys the class off `=== true`, so an unknown state stays violet", async () => {
    const fake = new FakeTransport();
    const rendered = await renderDormantRepoRow(fake, [REPO_A]);
    try {
      replicaVolatileStore.applyDiscoveryBranchUpdate(coordPath, "main", [REPO_A], ["main"], [
        null,
      ]);

      await waitFor(() => {
        const badge = badgeIn(rendered.root, TREE, "dev-webpage-ui", 0, "AgentsCommander")!;
        expect(badge.classList.contains("dirty")).toBe(false);
        expect(badge.title).toBe(`${REPO_A} (status unknown)`);
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("reddens only the dirty repo of a multi-repo row, keyed by path", async () => {
    const fake = new FakeTransport();
    const rendered = await renderDormantRepoRow(fake, [REPO_A, REPO_B]);
    try {
      await waitFor(() =>
        expect(badgeIn(rendered.root, TREE, "dev-webpage-ui", 1, "webpage")).not.toBeNull()
      );

      replicaVolatileStore.applyDiscoveryBranchUpdate(
        coordPath,
        null, // multi-repo: the single-repo shorthand stays null
        [REPO_A, REPO_B],
        ["main", "main"],
        [false, true]
      );

      await waitFor(() => {
        const a = badgeIn(rendered.root, TREE, "dev-webpage-ui", 0, "AgentsCommander")!;
        const b = badgeIn(rendered.root, TREE, "dev-webpage-ui", 1, "webpage")!;
        expect(a.classList.contains("dirty")).toBe(false);
        expect(b.classList.contains("dirty")).toBe(true);
        expect(a.title).toBe(REPO_A);
        expect(b.title).toBe(`${REPO_B} (local work not confirmed by cached origin tracking)`);
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps the red across a discovery reload (AC 6, end to end)", async () => {
    const fake = new FakeTransport();
    const rendered = await renderDormantRepoRow(fake, [REPO_A]);
    try {
      replicaVolatileStore.applyDiscoveryBranchUpdate(coordPath, "main", [REPO_A], ["main"], [
        true,
      ]);
      await waitFor(() =>
        expect(
          badgeIn(rendered.root, TREE, "dev-webpage-ui", 0, "AgentsCommander")!.classList.contains(
            "dirty"
          )
        ).toBe(true)
      );

      // What every reload does (loop tick, CLI refresh, entity creation). Gate A will
      // NOT re-emit afterwards, because the payload has not changed - so if the map
      // were wiped here the red would be gone until the worktree changed on disk.
      replicaVolatileStore.clearForPaths([coordPath]);

      await waitFor(() => {
        const badge = badgeIn(rendered.root, TREE, "dev-webpage-ui", 0, "AgentsCommander")!;
        expect(badge.classList.contains("dirty")).toBe(true);
        expect(badge.title).toBe(`${REPO_A} (local work not confirmed by cached origin tracking)`);
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("renders no repo badge at all on a non-coordinator row (G8)", async () => {
    const fake = new FakeTransport();
    const rendered = await renderDormantRepoRow(fake, [REPO_A]);
    try {
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-rust"));

      // The badge render is gated on isCoordinator, so the member row has no badge to
      // redden: the dirty feed reaches it and changes nothing on screen.
      replicaVolatileStore.applyDiscoveryBranchUpdate(memberPath, "main", [REPO_A], ["main"], [
        true,
      ]);

      expect(badgeIn(rendered.root, TREE, "dev-rust", 0, "AgentsCommander")).toBeNull();
      // ...while the coordinator in the same workgroup still has one.
      expect(badgeIn(rendered.root, TREE, "dev-webpage-ui", 0, "AgentsCommander")).not.toBeNull();
    } finally {
      rendered.cleanup();
    }
  });
});
