// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SidebarApp from "./App";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../shared/testing/ui-harness";
import { liveSelection, SESSION_A } from "../shared/testing/session-selection";
import {
  defaultGroupsConfig,
  defaultNonStop,
  exactGroupRegexForWorkgroup,
} from "./stores/workgroup-groups";

// #2036: the rail counter and the panel Rooms count must derive Ungrouped from
// ONE rule. This mounts the real SidebarApp so the two sides agree in the same
// DOM: wg-1 is in UI, wg-2 is Alert me!-only, wg-3 is the only ungrouped room.

const projectPath = "C:\\Project";

function projectDiscovery() {
  return discovery({
    workgroups: ["wg-1-dev-team", "wg-2-rust-team", "wg-3-docs-team"].map((name) => ({
      name,
      path: `${projectPath}\\.ac\\${name}`,
      task: null,
      taskTitle: null,
      agents: [],
    })),
  });
}

function groupsConfig() {
  return {
    ...defaultGroupsConfig(),
    groups: [{ id: "ui", name: "UI", regex: exactGroupRegexForWorkgroup("wg-1-dev-team") }],
    nonStop: {
      ...defaultNonStop(),
      show: true,
      regex: exactGroupRegexForWorkgroup("wg-2-rust-team"),
    },
  };
}

function setupTransport(fake: FakeTransport): void {
  fake.resolve("get_settings", baseSettings({ projectPaths: [projectPath], projectPath }));
  fake.resolve("open_project", { path: projectPath, registered: true, created: false });
  fake.resolve("discover_project", projectDiscovery());
  fake.resolve("get_project_groups", groupsConfig());
  fake.resolve("search_repos", []);
  fake.resolve("list_sessions", []);
  fake.resolve("get_active_session", liveSelection(SESSION_A));
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
}

function railButton(root: ParentNode): HTMLElement | null {
  return root.querySelector<HTMLElement>('[data-ac-testid="workgroupGroups.button.ungrouped"]');
}

/** Same locator as ProjectPanel.regex-filter.test.tsx:387-392. */
function roomsCount(root: HTMLElement): string | null {
  const header = Array.from(root.querySelectorAll(".ac-wg-header")).find(
    (h) => h.querySelector(".ac-wg-name")?.textContent === "Rooms"
  );
  return header?.querySelector(".ac-team-count")?.textContent ?? null;
}

describe("SidebarApp Ungrouped counter (#2036)", () => {
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

  it("rail counter and panel Rooms count agree when a room is Alert me!-only", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);

    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(railButton(rendered.root)).not.toBeNull());
      click(railButton(rendered.root)!);

      await waitFor(() => expect(roomsCount(rendered.root)).toBe("1"));
      const rooms = roomsCount(rendered.root)!;
      const counter = railButton(rendered.root)!.textContent ?? "";
      expect(counter).toContain("0/1");
      // State agreement, not two independent constants.
      expect(counter).toContain(`/${rooms}`);
    } finally {
      rendered.cleanup();
    }
  });
});
