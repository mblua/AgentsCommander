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
import { remoteActivityByPath, remoteActivityStore } from "../stores/remote-activity";
import type { CiState, RemoteActivityUpdate, StalenessState } from "../../shared/types";

// #2064 Phase C — the CI ring and the stale bar on the orchestrator repo chip.
//
// These render the real ProjectPanel through the FakeTransport harness and assert the
// DOM, per the frontend-visual-verification discipline. They cover the CLASS and the
// TITLE; the marker's PAINT is pinned as bytes in
// `src/sidebar/styles/remote-activity-css.test.ts`, because jsdom does not apply a
// stylesheet and a class-only assertion stays green against a rule that does not
// exist.
//
// Test 15 at the bottom is the only one in this repo that fails when the feature is
// wired to NOTHING: tests 8-14 set the store directly, so they would all stay green
// with no listener registered at all.
//
// The rows are DORMANT — nothing here creates a session — which matches Phase A,
// which polls every replica whether or not a session exists.

const projectPath = "C:\\Project";
const workgroupPath = `${projectPath}\\.ac\\wg-2-dev-team`;
const coordPath = `${workgroupPath}\\__agent_dev-webpage-ui`;
const REPO_A = `${workgroupPath}\\repo-AgentsCommander`;

/** The coordinator row also renders in the quick-access strip; tests target the
 *  workgroup-tree copy explicitly rather than counting badges document-wide. */
const TREE = "workgroups";

/** The badge's own data-ac-testid: replica.repoBadge.<ctx>.<wg>.<replica>.<i>.<label> */
function badgeIn(root: Element, ctx: string, index: number, label: string): HTMLElement | null {
  return root.querySelector<HTMLElement>(
    `[data-ac-testid="replica.repoBadge.${ctx}.wg-2-dev-team.dev-webpage-ui.${index}.${label}"]`
  );
}

function repoDiscovery(repoPaths: string[]) {
  return discovery({
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: workgroupPath,
        task: null,
        taskTitle: "Remote activity",
        agents: [
          {
            name: "dev-webpage-ui",
            path: coordPath,
            repoPaths,
            isCoordinator: true,
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

/** Exactly what the event carries: four parallel vectors, one entry per repo. */
function applyActivity(
  repoPaths: string[],
  ciStates: CiState[],
  stalenessStates: StalenessState[],
  behindBy: (number | null)[]
): void {
  const update: RemoteActivityUpdate = { repoPaths, ciStates, stalenessStates, behindBy };
  remoteActivityStore.applyRemoteActivityUpdate(update);
}

describe("ProjectPanel remote-activity chip (#2064 Phase C)", () => {
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

  it("class_is_untouched_when_the_store_is_empty", async () => {
    const fake = new FakeTransport();
    const rendered = await renderDormantRepoRow(fake, [REPO_A]);
    try {
      await waitFor(() =>
        expect(badgeIn(rendered.root, TREE, 0, "AgentsCommander")).not.toBeNull()
      );
      const badge = badgeIn(rendered.root, TREE, 0, "AgentsCommander")!;
      // No event has landed: no class, no suffix, and no entry invented for it. This
      // is also the shape a user without `gh`, or with the feature off, keeps.
      expect(badge.className).toBe("ac-discovery-badge branch");
      expect(badge.title).toBe(`${REPO_A} (status unknown)`);
      expect(Object.keys(remoteActivityByPath)).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("ci_running_adds_only_the_ci_running_class", async () => {
    const fake = new FakeTransport();
    const rendered = await renderDormantRepoRow(fake, [REPO_A]);
    try {
      await waitFor(() =>
        expect(badgeIn(rendered.root, TREE, 0, "AgentsCommander")).not.toBeNull()
      );

      applyActivity([REPO_A], ["running"], ["current"], [null]);

      await waitFor(() => {
        const badge = badgeIn(rendered.root, TREE, 0, "AgentsCommander")!;
        expect(badge.className).toBe("ac-discovery-badge branch ci-running");
        expect(badge.title).toBe(`${REPO_A} (status unknown) - CI running`);
      });

      // The dirty channel is untouched by this phase and keeps its own class: the
      // combination has to be `dirty ci-running`, in that order.
      replicaVolatileStore.applyDiscoveryBranchUpdate(coordPath, "main", [REPO_A], ["main"], [
        true,
      ]);

      await waitFor(() => {
        const badge = badgeIn(rendered.root, TREE, 0, "AgentsCommander")!;
        expect(badge.className).toBe("ac-discovery-badge branch dirty ci-running");
        expect(badge.title).toBe(
          `${REPO_A} (local work not confirmed by cached origin tracking) - CI running`
        );
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("stale_adds_only_the_stale_class", async () => {
    const fake = new FakeTransport();
    const rendered = await renderDormantRepoRow(fake, [REPO_A]);
    try {
      await waitFor(() =>
        expect(badgeIn(rendered.root, TREE, 0, "AgentsCommander")).not.toBeNull()
      );

      applyActivity([REPO_A], ["unknown"], ["stale"], [4]);

      await waitFor(() => {
        const badge = badgeIn(rendered.root, TREE, 0, "AgentsCommander")!;
        expect(badge.className).toBe("ac-discovery-badge branch stale");
        expect(badge.title).toBe(`${REPO_A} (status unknown) - base is 4 commits ahead`);
      });

      // Both markers at once, plus dirty: the fixed order is dirty, ci-running, stale.
      replicaVolatileStore.applyDiscoveryBranchUpdate(coordPath, "main", [REPO_A], ["main"], [
        true,
      ]);
      applyActivity([REPO_A], ["running"], ["stale"], [4]);

      await waitFor(() => {
        const badge = badgeIn(rendered.root, TREE, 0, "AgentsCommander")!;
        expect(badge.className).toBe("ac-discovery-badge branch dirty ci-running stale");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("idle_and_current_and_unknown_add_no_class", async () => {
    const fake = new FakeTransport();
    const rendered = await renderDormantRepoRow(fake, [REPO_A]);
    try {
      await waitFor(() =>
        expect(badgeIn(rendered.root, TREE, 0, "AgentsCommander")).not.toBeNull()
      );

      // idle/current: the honest "checked, nothing happening" state.
      applyActivity([REPO_A], ["idle"], ["current"], [null]);
      await waitFor(() => {
        const badge = badgeIn(rendered.root, TREE, 0, "AgentsCommander")!;
        expect(badge.className).toBe("ac-discovery-badge branch");
        expect(badge.title).toBe(`${REPO_A} (status unknown) - no CI activity for this commit`);
      });

      // unknown/unknown: no `gh`, feature off, or not swept yet. Silent on purpose.
      applyActivity([REPO_A], ["unknown"], ["unknown"], [null]);
      await waitFor(() => {
        const badge = badgeIn(rendered.root, TREE, 0, "AgentsCommander")!;
        expect(badge.className).toBe("ac-discovery-badge branch");
        expect(badge.title).toBe(`${REPO_A} (status unknown)`);
      });
    } finally {
      rendered.cleanup();
    }
  });

  // The wiring test. Tests 8-14 above set the store directly and would ALL stay green
  // with no listener at all; this is the one that fails when the feature is connected
  // to nothing. FakeTransport, not an ipc spy, because `fake.listens` records the
  // registration and `emitFromBackend` drives the real callback.
  it("the_listener_is_registered_and_released", async () => {
    const fake = new FakeTransport();
    const rendered = await renderDormantRepoRow(fake, [REPO_A]);
    try {
      await waitFor(() =>
        expect(fake.listensFor("ac_remote_activity_updated")).toHaveLength(1)
      );
      expect(badgeIn(rendered.root, TREE, 0, "AgentsCommander")).not.toBeNull();

      fake.emitFromBackend("ac_remote_activity_updated", {
        repoPaths: [REPO_A],
        ciStates: ["running"],
        stalenessStates: ["current"],
        behindBy: [null],
      });

      // Repaint half: a listener that is registered but inert fails here.
      await waitFor(() =>
        expect(badgeIn(rendered.root, TREE, 0, "AgentsCommander")!.className).toBe(
          "ac-discovery-badge branch ci-running"
        )
      );
    } finally {
      rendered.cleanup();
    }

    // Release half. `fake.listens` records registrations and never releases, so the
    // unmount is pinned by BEHAVIOUR instead: with the component gone, the same
    // payload must reach nothing. A leaked listener repopulates the map here.
    remoteActivityStore.clearAll();
    fake.emitFromBackend("ac_remote_activity_updated", {
      repoPaths: [REPO_A],
      ciStates: ["running"],
      stalenessStates: ["current"],
      behindBy: [null],
    });
    expect(Object.keys(remoteActivityByPath)).toHaveLength(0);
  });
});
