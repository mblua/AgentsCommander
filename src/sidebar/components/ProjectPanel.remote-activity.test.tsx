// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { baseSettings, discovery } from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { replicaVolatileStore } from "../stores/replica-volatile";
import { remoteActivityByPath, remoteActivityStore } from "../stores/remote-activity";
import type { CiState, StalenessState } from "../../shared/types";

// #2064 Phase C — the CI ring and the stale bar on the orchestrator repo chip.
//
// These render the real ProjectPanel through the FakeTransport harness and assert the
// DOM, per the frontend-visual-verification discipline. They cover the CLASS and the
// TITLE; the marker's PAINT is pinned as bytes in
// `src/sidebar/styles/remote-activity-css.test.ts`, because jsdom does not apply a
// stylesheet and a class-only assertion stays green against a rule that does not
// exist.
//
// The last case is the only one in the repo that fails when the feature is wired to
// NOTHING: the store cases above it set the store directly, so they would all stay
// green with no listener registered at all. It is not optional.
//
// The row is DORMANT — nothing here creates a session — which matches Phase A, which
// polls every replica whether or not a session exists.

/** One dormant coordinator row, described once. */
const ROW = {
  project: "C:\\Project",
  workgroup: "wg-2-dev-team",
  replica: "dev-webpage-ui",
  repo: "repo-AgentsCommander",
} as const;

const WG_DIR = `${ROW.project}\\.ac\\${ROW.workgroup}`;
const REPO_PATH = `${WG_DIR}\\${ROW.repo}`;
const REPLICA_PATH = `${WG_DIR}\\__agent_${ROW.replica}`;
const CHIP_LABEL = ROW.repo.replace(/^repo-/, "");

type Panel = ReturnType<typeof renderWithFakeTransport>;

let panel: Panel | null = null;

/** Releases the mounted panel without unmounting the DOM stubs: test 15 needs the
 *  unmount and the stub teardown as separate steps, and the teardown order below
 *  depends on the panel going first. */
function closePanel(): void {
  panel?.cleanup();
  panel = null;
}

function rowDiscovery() {
  return discovery({
    workgroups: [
      {
        name: ROW.workgroup,
        path: WG_DIR,
        task: null,
        taskTitle: "Remote activity",
        agents: [
          {
            name: ROW.replica,
            path: REPLICA_PATH,
            repoPaths: [REPO_PATH],
            isCoordinator: true,
          },
        ],
      },
    ],
  });
}

/** Mounts the real panel against the fake backend and waits for the chip. */
async function openPanel(): Promise<FakeTransport> {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: ROW.project, registered: true, created: false });
  fake.resolve("get_settings", baseSettings());
  fake.resolve("discover_project", rowDiscovery());
  panel = renderWithFakeTransport(() => <ProjectPanel />, fake);
  await projectStore.createAndLoad(ROW.project);
  await waitFor(() => expect(chipCopies().length).toBeGreaterThan(0));
  return fake;
}

/** Every rendered copy of the repo chip. A coordinator row is drawn twice — the
 *  workgroup tree and the quick-access strip — through one renderReplicaItem, so
 *  asserting on ALL copies is the stronger claim, not a wider net. */
function chipCopies(): HTMLElement[] {
  const root = panel?.root;
  if (!root) throw new Error("openPanel() has not run");
  return Array.from(root.querySelectorAll<HTMLElement>('[data-ac-testid*=".repoBadge."]')).filter(
    (el) => el.getAttribute("data-ac-testid")!.endsWith(`.${CHIP_LABEL}`)
  );
}

/** The chip's class and title, with the rendered copies cross-checked against each
 *  other: a refactor that repaints one copy and leaves the other stale fails here,
 *  and so does an assertion over an empty (never-rendered) set. */
function chipState(): { className: string; title: string } {
  const copies = chipCopies();
  const classes = new Set(copies.map((el) => el.className));
  const titles = new Set(copies.map((el) => el.title));
  if (copies.length === 0 || classes.size !== 1 || titles.size !== 1) {
    throw new Error(
      `unusable chip set: ${copies.length} copies, classes [${[...classes].join(" | ")}], titles [${[...titles].join(" | ")}]`
    );
  }
  return { className: [...classes][0], title: [...titles][0] };
}

/** One repo's published answer, exactly as the event carries it. */
function publish(ci: CiState, staleness: StalenessState, behindBy: number | null = null): void {
  remoteActivityStore.applyRemoteActivityUpdate({
    repoPaths: [REPO_PATH],
    ciStates: [ci],
    stalenessStates: [staleness],
    behindBy: [behindBy],
  });
}

/** Marks the repo dirty through the channel #1028 owns, which this phase must not
 *  disturb: the two signals have to be additive. */
function markDirty(): void {
  replicaVolatileStore.applyDiscoveryBranchUpdate(REPLICA_PATH, "main", [REPO_PATH], ["main"], [
    true,
  ]);
}

describe("ProjectPanel remote-activity chip (#2064 Phase C)", () => {
  let stubs: (() => void) | null = null;

  beforeEach(() => {
    stubs = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    closePanel();
    stubs?.();
    stubs = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("class_is_untouched_when_the_store_is_empty", async () => {
    await openPanel();

    // No event has landed: no class, no suffix, and no entry invented for it. This
    // is also the shape a user without `gh`, or with the feature off, keeps.
    expect(chipState().className).toBe("ac-discovery-badge branch");
    expect(chipState().title).toBe(`${REPO_PATH} (status unknown)`);
    expect(Object.keys(remoteActivityByPath)).toHaveLength(0);
  });

  it("ci_running_adds_only_the_ci_running_class", async () => {
    await openPanel();
    publish("running", "current");

    await waitFor(() =>
      expect(chipState().className).toBe("ac-discovery-badge branch ci-running")
    );
    expect(chipState().title).toBe(`${REPO_PATH} (status unknown) - CI running`);

    // The dirty channel keeps its own class: the combination is `dirty ci-running`,
    // in that order.
    markDirty();
    await waitFor(() =>
      expect(chipState().className).toBe("ac-discovery-badge branch dirty ci-running")
    );
    expect(chipState().title).toBe(
      `${REPO_PATH} (local work not confirmed by cached origin tracking) - CI running`
    );
  });

  it("stale_adds_only_the_stale_class", async () => {
    await openPanel();
    publish("unknown", "stale", 4);

    await waitFor(() => expect(chipState().className).toBe("ac-discovery-badge branch stale"));
    expect(chipState().title).toBe(`${REPO_PATH} (status unknown) - base is 4 commits ahead`);

    // Both markers at once, plus dirty: the fixed order is dirty, ci-running, stale.
    markDirty();
    publish("running", "stale", 4);
    await waitFor(() =>
      expect(chipState().className).toBe("ac-discovery-badge branch dirty ci-running stale")
    );
  });

  it("idle_and_current_and_unknown_add_no_class", async () => {
    await openPanel();

    // idle/current: no CI text on the chip. The gate is a real transition: `running`
    // must first tint the chip and print its title, so the neutral title below is
    // measured after a visible change, never against the initial DOM.
    publish("running", "current");
    await waitFor(() => expect(chipState().className).toBe("ac-discovery-badge branch ci-running"));
    expect(chipState().title).toBe(`${REPO_PATH} (status unknown) - CI running`);
    publish("idle", "current");
    await waitFor(() => expect(chipState().className).toBe("ac-discovery-badge branch"));
    expect(chipState().title).toBe(`${REPO_PATH} (status unknown)`);

    // unknown/unknown: no `gh`, feature off, or not swept yet. Silent on purpose,
    // proven after the same running-to-neutral transition.
    publish("running", "current");
    await waitFor(() => expect(chipState().className).toBe("ac-discovery-badge branch ci-running"));
    expect(chipState().title).toBe(`${REPO_PATH} (status unknown) - CI running`);
    publish("unknown", "unknown");
    await waitFor(() => expect(chipState().className).toBe("ac-discovery-badge branch"));
    expect(chipState().title).toBe(`${REPO_PATH} (status unknown)`);
  });

  it("the_listener_is_registered_and_released", async () => {
    const fake = await openPanel();
    await waitFor(() => expect(fake.listensFor("ac_remote_activity_updated")).toHaveLength(1));

    const payload = {
      repoPaths: [REPO_PATH],
      ciStates: ["running"] as CiState[],
      stalenessStates: ["current"] as StalenessState[],
      behindBy: [null],
    };

    // Repaint half: a listener that is registered but inert fails here.
    fake.emitFromBackend("ac_remote_activity_updated", payload);
    await waitFor(() =>
      expect(chipState().className).toBe("ac-discovery-badge branch ci-running")
    );

    // Release half. `fake.listens` records registrations and never releases, so the
    // unmount is pinned by BEHAVIOUR instead: with the component gone, the same
    // payload must reach nothing. A leaked listener repopulates the map here.
    closePanel();
    remoteActivityStore.clearAll();    fake.emitFromBackend("ac_remote_activity_updated", payload);
    expect(Object.keys(remoteActivityByPath)).toHaveLength(0);
  });
});
