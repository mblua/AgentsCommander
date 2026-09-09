// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AcWorkgroup, Session, WorkgroupGroupsConfig } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import type { ProjectState } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import { defaultGroupsConfig, exactGroupRegexForWorkgroup } from "../stores/workgroup-groups";
import WorkgroupGroupRail from "./WorkgroupGroupRail";

// #1859 — the rail is the outermost level that can hide a blocked menu: a
// collapsed project rail tab shows no rows at all. This file mirrors
// WorkgroupGroupRail.raise-hand.test.tsx in structure and fixtures on purpose,
// because the two indicators share the render site and the same aggregation.

const projectPath = "C:\\Project";

/** A workgroup with one replica. Unlike the raise-hand rollup, the blocked-menu
 *  rollup has no coordinator and no task-title gate: any replica counts. */
function wg(name: string, opts: { coordinator?: boolean } = {}): AcWorkgroup {
  const { coordinator = true } = opts;
  return {
    name,
    path: `${projectPath}\\.ac\\${name}`,
    task: null,
    taskTitle: "Ship the thing",
    agents: [
      {
        name: "dev-webpage-ui",
        path: `${projectPath}\\.ac\\${name}\\__agent_dev-webpage-ui`,
        repoPaths: [],
        isCoordinator: coordinator,
      },
    ],
  };
}

function project(workgroups: AcWorkgroup[]): ProjectState {
  return {
    path: projectPath,
    folderName: "Project",
    workgroups,
    agents: [],
    teams: [],
    loops: [],
    contextTemplateUpdates: [],
  };
}

/** ui -> wg-1-dev-team, rust -> wg-2-rust-team; wg-3-docs-team stays ungrouped. */
function groupsConfig(overrides: Partial<WorkgroupGroupsConfig> = {}): WorkgroupGroupsConfig {
  return {
    ...defaultGroupsConfig(),
    ...overrides,
    groups:
      overrides.groups ?? [
        { id: "ui", name: "UI", regex: exactGroupRegexForWorkgroup("wg-1-dev-team") },
        { id: "rust", name: "Rust", regex: exactGroupRegexForWorkgroup("wg-2-rust-team") },
      ],
  };
}

const defaultWorkgroups = () => [wg("wg-1-dev-team"), wg("wg-2-rust-team"), wg("wg-3-docs-team")];

function sessionFor(wgName: string, overrides: Partial<Session> = {}): Session {
  return session({
    id: `session-${wgName}`,
    name: `${wgName}/dev-webpage-ui`,
    workingDirectory: `${projectPath}\\.ac\\${wgName}\\__agent_dev-webpage-ui`,
    status: "running",
    isCoordinator: true,
    ...overrides,
  });
}

/** A session whose replica is stuck on an interactive menu. */
function blockedSession(wgName: string): Session {
  return sessionFor(wgName, {
    communication: {
      kind: "blockedMenu",
      visible: true,
      updatedAt: "2026-09-07T00:00:00Z",
      message: "Choose the authentication method",
    },
  });
}

/** The same replica with a raised hand instead. */
function raisedSession(wgName: string): Session {
  return sessionFor(wgName, {
    communication: { kind: "raiseHand", visible: true, updatedAt: "2026-09-07T00:00:00Z" },
  });
}

function target<T extends Element>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing element ${testId}`);
  return element;
}

function keysFor(prefix: string): string[] {
  return Array.from(document.querySelectorAll<HTMLElement>(`[data-ac-testid^="${prefix}"]`)).map(
    (el) => el.dataset.acTestid!.replace(prefix, "")
  );
}

const railButtonOrder = () => keysFor("workgroupGroups.button.");
const railBlockedMenus = () => keysFor("workgroupGroups.blockedMenu.");
const railRaiseHands = () => keysFor("workgroupGroups.raiseHand.");

function mountRail(sessions: Session[]) {
  const fake = new FakeTransport();
  fake.resolve("get_project_groups", groupsConfig());
  sessionsStore.setSessions(sessions);
  return renderWithFakeTransport(
    () => <WorkgroupGroupRail projects={[project(defaultWorkgroups())]} />,
    fake
  );
}

describe("WorkgroupGroupRail blocked-menu rollup (#1859)", () => {
  beforeEach(() => resetUiStoresForTests());
  afterEach(() => {
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("lights the tab of the group holding a blocked replica, and never All", async () => {
    const rendered = mountRail([blockedSession("wg-1-dev-team")]);
    try {
      await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust"]));
      await waitFor(() => expect(railBlockedMenus()).toEqual(["ui"]));

      const badge = target<HTMLElement>("workgroupGroups.blockedMenu.ui");
      expect(badge.classList.contains("workgroup-group-rail-blocked-menu")).toBe(true);
      expect(badge.querySelector("svg.workgroup-group-rail-blocked-menu-icon")).not.toBeNull();
      expect(badge.getAttribute("aria-label")).toBe(
        "A session is waiting on an interactive menu"
      );
      // Same inline placement as the hand: first in the title line, ahead of
      // the title text.
      const titleLine = target<HTMLElement>("workgroupGroups.button.ui").querySelector(
        ".workgroup-group-rail-title-line"
      );
      expect(titleLine?.firstElementChild).toBe(badge);
    } finally {
      rendered.cleanup();
    }
  });

  it("shows no blocked-menu indicator anywhere when no replica is blocked", async () => {
    const rendered = mountRail([sessionFor("wg-1-dev-team")]);
    try {
      await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust"]));
      expect(railBlockedMenus()).toEqual([]);

      // Same mount, same selector, one blocked menu later: this half is what a
      // predicate hardcoded to `false` cannot survive.
      sessionsStore.setSessions([blockedSession("wg-1-dev-team")]);
      await waitFor(() => expect(railBlockedMenus()).toEqual(["ui"]));
    } finally {
      rendered.cleanup();
    }
  });

  // The two directions below are one test each on purpose: a single direction
  // cannot catch the two predicates being swapped for each other.
  it("a raised hand lights the hand and NOT the blocked-menu indicator", async () => {
    const rendered = mountRail([raisedSession("wg-1-dev-team")]);
    try {
      await waitFor(() => expect(railRaiseHands()).toEqual(["ui"]));
      expect(railBlockedMenus()).toEqual([]);
    } finally {
      rendered.cleanup();
    }
  });

  it("a blocked menu lights the blocked-menu indicator and NOT the hand", async () => {
    const rendered = mountRail([blockedSession("wg-1-dev-team")]);
    try {
      await waitFor(() => expect(railBlockedMenus()).toEqual(["ui"]));
      expect(railRaiseHands()).toEqual([]);
    } finally {
      rendered.cleanup();
    }
  });

  it("shows both indicators when a group has both, blocked menu first in DOM order", async () => {
    // Two workgroups in the same group would need a second group regex, so put
    // the two states on the two replicas of one group's single workgroup.
    const both: AcWorkgroup = {
      ...wg("wg-1-dev-team"),
      agents: [
        {
          name: "dev-webpage-ui",
          path: `${projectPath}\\.ac\\wg-1-dev-team\\__agent_dev-webpage-ui`,
          repoPaths: [],
          isCoordinator: true,
        },
        {
          name: "dev-rust",
          path: `${projectPath}\\.ac\\wg-1-dev-team\\__agent_dev-rust`,
          repoPaths: [],
          isCoordinator: false,
        },
      ],
    };
    const fake = new FakeTransport();
    fake.resolve("get_project_groups", groupsConfig());
    sessionsStore.setSessions([
      raisedSession("wg-1-dev-team"),
      session({
        id: "session-wg-1-dev-team-rust",
        name: "wg-1-dev-team/dev-rust",
        workingDirectory: `${projectPath}\\.ac\\wg-1-dev-team\\__agent_dev-rust`,
        status: "running",
        communication: {
          kind: "blockedMenu",
          visible: true,
          updatedAt: "2026-09-07T00:00:00Z",
          message: "Choose the authentication method",
        },
      }),
    ]);
    const rendered = renderWithFakeTransport(
      () => (
        <WorkgroupGroupRail
          projects={[project([both, wg("wg-2-rust-team"), wg("wg-3-docs-team")])]}
        />
      ),
      fake
    );
    try {
      await waitFor(() => expect(railBlockedMenus()).toEqual(["ui"]));
      expect(railRaiseHands()).toEqual(["ui"]);

      const blocked = target<HTMLElement>("workgroupGroups.blockedMenu.ui");
      const hand = target<HTMLElement>("workgroupGroups.raiseHand.ui");
      expect(
        Boolean(blocked.compareDocumentPosition(hand) & Node.DOCUMENT_POSITION_FOLLOWING)
      ).toBe(true);
    } finally {
      rendered.cleanup();
    }
  });
});
