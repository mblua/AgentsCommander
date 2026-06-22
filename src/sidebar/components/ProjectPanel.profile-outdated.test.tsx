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
