// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
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

// #592: the profile-drift "outdated" badge must surface on a WG replica row, not
// only on the sidebar SessionItem. The backend marks profileOutdated in
// list_sessions (loaded cell hash != current config); the row reads the SAME store
// value the status dot reads. Rendered through the real ProjectPanel + FakeTransport
// per the frontend-visual-verification discipline.

const projectPath = "C:\\Project";
const workgroupPath = `${projectPath}\\.ac\\wg-2-dev-team`;
const replicaPath = `${workgroupPath}\\__agent_dev-webpage-ui`;
const replicaName = "dev-webpage-ui";
const sessionName = `wg-2-dev-team/${replicaName}`;
// #2435 - a second replica with no session: the dormant row.
const dormantName = "dormant-ui";

function replicaDiscovery() {
  return discovery({
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: workgroupPath,
        task: null,
        taskTitle: "Drift badge",
        agents: [
          {
            name: replicaName,
            path: replicaPath,
            repoPaths: [],
            isCoordinator: true,
            currentProfile: "B",
          },
          {
            name: dormantName,
            path: `${workgroupPath}\\__agent_${dormantName}`,
            repoPaths: [],
            isCoordinator: false,
            currentProfile: "B",
          },
        ],
      },
    ],
  });
}

/** A live session bound to the replica row (matched by name, like the status dot). */
function replicaSession(profileOutdated: boolean | undefined) {
  return session({
    id: "replica-live",
    name: sessionName,
    workingDirectory: replicaPath,
    isCoordinator: true,
    status: "running",
    agentId: "codex",
    agentLabel: "Codex",
    profileOutdated,
  });
}

async function mount() {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("get_settings", baseSettings());
  fake.resolve("discover_project", replicaDiscovery());
  const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
  await settingsStore.load();
  await projectStore.createAndLoad(projectPath);
  await waitFor(() => expect(rendered.root.textContent).toContain(replicaName));
  return rendered;
}

describe("ProjectPanel replica profile-outdated badge (#592)", () => {
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

  it("renders the reload badge on a replica row whose session is profileOutdated", async () => {
    const rendered = await mount();
    try {
      // No drift yet → no badge even though the live session renders.
      sessionsStore.setSessions([replicaSession(false)]);
      await waitFor(() => expect(rendered.root.querySelector(".replica-item")).not.toBeNull());
      expect(rendered.root.querySelector(".profile-outdated-badge")).toBeNull();

      // Backend marks drift (surgical setProfileOutdated, the same path App.tsx uses
      // on load / focus / events) → the badge appears with no full re-list.
      sessionsStore.setProfileOutdated("replica-live", true);
      await waitFor(() =>
        expect(rendered.root.querySelector(".profile-outdated-badge")).not.toBeNull(),
      );

      // Clearing the drift (e.g. after a reload re-stamps the hash) hides it again.
      sessionsStore.setProfileOutdated("replica-live", false);
      await waitFor(() =>
        expect(rendered.root.querySelector(".profile-outdated-badge")).toBeNull(),
      );
    } finally {
      rendered.cleanup();
    }
  });
});

describe("ProjectPanel replica tier badge (#2435)", () => {
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

  const rowOf = (root: ParentNode, name: string) =>
    Array.from(root.querySelectorAll<HTMLElement>(".replica-item")).find((row) =>
      row.textContent?.includes(name),
    );

  it("shows the tier badge on a row with a tier-carrying session and none on a dormant row", async () => {
    const rendered = await mount();
    try {
      sessionsStore.setSessions([
        session({
          id: "replica-live",
          name: sessionName,
          workingDirectory: replicaPath,
          isCoordinator: true,
          status: "running",
          agentId: "codex",
          agentLabel: "Codex",
          requestedProfile: "B",
          effectiveProfile: "B",
          profileFallbackApplied: false,
          matchTier: "commandAndLetter",
        }),
      ]);
      const tierSelector =
        '[data-ac-testid="replica.tierBadge.workgroups.wg-2-dev-team.dev-webpage-ui"]';
      await waitFor(() => expect(rendered.root.querySelector(tierSelector)).not.toBeNull());
      const tier = rendered.root.querySelector<HTMLElement>(tierSelector)!;
      expect(tier.tagName).toBe("SPAN");
      expect(tier.textContent).toBe("B·cmd");
      expect(tier.getAttribute("title")).toBe(
        "Matched by command, not by name or configuration. Effective profile: B.",
      );
      const liveBadges = rowOf(rendered.root, replicaName)!.querySelectorAll(".profile-badge");
      expect(Array.from(liveBadges, (el) => el.textContent)).toEqual(["B", "B·cmd"]);

      // Dormant row: today's bare-letter badge, and no tier badge.
      const dormant = rowOf(rendered.root, dormantName)!;
      expect(dormant).toBeDefined();
      const dormantBadges = dormant.querySelectorAll(".profile-badge");
      expect(Array.from(dormantBadges, (el) => el.textContent)).toEqual(["B"]);
      expect(dormant.querySelector('[data-ac-testid^="replica.tierBadge."]')).toBeNull();
      expect(dormant.querySelector(".profile-badge--tier")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });
});

describe("ProjectPanel replica orphan notice (#2568)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    sessionsStore.resetOrphanNoticesForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    sessionsStore.resetOrphanNoticesForTests();
    document.body.replaceChildren();
  });

  const liveSelector =
    '[data-ac-testid="replica.orphanNotice.workgroups.wg-2-dev-team.dev-webpage-ui"]';
  const dormantSelector =
    '[data-ac-testid="replica.orphanNotice.workgroups.wg-2-dev-team.dormant-ui"]';
  // The coordinator row also renders in other panel sections; count the
  // workgroups surface only.
  const WG_NOTICES = '[data-ac-testid^="replica.orphanNotice.workgroups."]:not([data-ac-testid$=".dismiss"])';
  const TEXT = "Saved coding agent not found. Using Codex B, same name.";

  /** An adopted session on the named replica row; the agent key comes from `dir` + `agentId`. */
  const adoptedOn = (id: string, name: string, dir: string, agentId: string) =>
    session({
      id,
      name: `wg-2-dev-team/${name}`,
      workingDirectory: dir,
      status: "running",
      agentId,
      agentLabel: "Codex",
      requestedProfile: "B",
      effectiveProfile: "B",
      profileFallbackApplied: false,
      matchTier: "labelAndLetter",
    });

  it("shows the notice on an adopted replica row and none on a dormant row", async () => {
    const rendered = await mount();
    try {
      sessionsStore.setSessions([
        { ...adoptedOn("replica-live", replicaName, replicaPath, "codex"), isCoordinator: true },
      ]);
      await waitFor(() => expect(rendered.root.querySelector(liveSelector)).not.toBeNull());
      const notice = rendered.root.querySelector<HTMLElement>(liveSelector)!;
      expect(notice.querySelector(".orphan-notice-text")!.textContent).toBe(TEXT);
      expect(notice.parentElement!.classList.contains("replica-item-info")).toBe(true);
      expect(notice.parentElement!.lastElementChild).toBe(notice);
      expect(rendered.root.querySelector(dormantSelector)).toBeNull();
      expect(rendered.root.querySelectorAll(WG_NOTICES)).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });

  it("dismissing one agent's notice leaves a different agent's notice standing", async () => {
    const rendered = await mount();
    try {
      sessionsStore.setSessions([
        adoptedOn("live-a", replicaName, replicaPath, "codex"),
        adoptedOn("live-b", dormantName, `${workgroupPath}\\__agent_${dormantName}`, "claude"),
      ]);
      await waitFor(() => expect(rendered.root.querySelectorAll(WG_NOTICES)).toHaveLength(2));
      click(rendered.root.querySelector<HTMLElement>(`${liveSelector.slice(0, -2)}.dismiss"]`)!);
      await Promise.resolve();
      expect(rendered.root.querySelector(liveSelector)).toBeNull();
      expect(rendered.root.querySelector(dormantSelector)).not.toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  // A replica row binds its session by name AND path, so two workgroup rows can
  // never share one working directory. The coordinator row is the real case of
  // one agent on two rows: it renders in the workgroups section and again in a
  // second panel section, both reading the same session, so the same key.
  it("two rows of the same agent clear together on one dismissal", async () => {
    const rendered = await mount();
    try {
      sessionsStore.setSessions([
        { ...adoptedOn("replica-live", replicaName, replicaPath, "codex"), isCoordinator: true },
      ]);
      const allNotices = '.orphan-notice[data-ac-testid^="replica.orphanNotice."]';
      await waitFor(() => expect(rendered.root.querySelectorAll(allNotices)).toHaveLength(2));
      const ids = Array.from(rendered.root.querySelectorAll(allNotices), (el) => el.getAttribute("data-ac-testid"));
      expect(new Set(ids).size).toBe(2);
      click(rendered.root.querySelector<HTMLElement>(`${liveSelector.slice(0, -2)}.dismiss"]`)!);
      await Promise.resolve();
      expect(rendered.root.querySelectorAll(allNotices)).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });
});
