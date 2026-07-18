// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SidebarApp from "./App";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../shared/testing/ui-harness";
import { liveSelection, SESSION_A } from "../shared/testing/session-selection";

// #592: the profile-drift badge must surface from the NORMAL list_sessions load
// path, not only from a coding_agent_profiles_updated event. The canonical failure
// the user hit: a profile cell edited OUTSIDE the app (a hand edit to settings.json)
// emits no event, so the surgical event refresh never runs and the badge stays dark.
// This mounts the real SidebarApp and proves a window focus re-list lights the badge
// with NO event emitted.

const projectPath = "C:\\Project";
const agentPath = `${projectPath}\\.ac\\_agent_General`;

function setupTransport(fake: FakeTransport, isOutdated: () => boolean): void {
  fake.resolve(
    "get_settings",
    baseSettings({ projectPaths: [projectPath], projectPath }),
  );
  fake.resolve("open_project", { path: projectPath, registered: true, created: false });
  fake.resolve(
    "discover_project",
    discovery({
      agents: [{ name: "General", path: agentPath, roleExists: true }],
      teams: [],
      workgroups: [],
    }),
  );
  fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
  fake.resolve("search_repos", []);
  // Dynamic: the drift flag the backend would compute in list_sessions flips
  // between calls without any event being emitted.
  fake.onInvoke("list_sessions", () => [
    session({
      id: SESSION_A,
      name: "General",
      workingDirectory: agentPath,
      status: "active",
      agentId: "codex",
      agentLabel: "Codex",
      profileOutdated: isOutdated(),
    }),
  ]);
  fake.resolve("get_active_session", liveSelection(SESSION_A));
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
}

describe("SidebarApp profile-drift surfacing (#592)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
  });

  it("lights the outdated badge on window focus when list_sessions reports drift, with no event", async () => {
    let outdated = false;
    const fake = new FakeTransport();
    setupTransport(fake, () => outdated);

    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      // Initial load: the agent renders, no drift yet → no badge.
      await waitFor(() => expect(rendered.root.textContent).toContain("General"));
      expect(rendered.root.querySelector(".profile-outdated-badge")).toBeNull();

      // Backend now reports drift (e.g. settings.json hand-edited). NO
      // coding_agent_profiles_updated event is emitted; the only trigger is the
      // user returning to the window.
      outdated = true;
      window.dispatchEvent(new Event("focus"));

      // The debounced re-list (250ms) pulls the fresh profileOutdated and
      // surgically applies it; the badge appears without any event. Generous
      // timeout so the debounce + async list_sessions never flakes on slow CI.
      await waitFor(
        () => expect(rendered.root.querySelector(".profile-outdated-badge")).not.toBeNull(),
        2000,
      );
    } finally {
      rendered.cleanup();
    }
  });
});
