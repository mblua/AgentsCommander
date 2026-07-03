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
import {
  defaultGroupsConfig,
  exactGroupRegexForWorkgroup,
} from "../stores/workgroup-groups";
import WorkgroupGroupRail from "./WorkgroupGroupRail";
import { workgroupHasRaisedHand } from "./workgroup-session";

const projectPath = "C:\\Project";

/** A workgroup with one coordinator replica. `taskTitle` defaults to a real
 *  title so the #763 `!!wg.taskTitle` gate passes; pass null to exercise it. */
function wg(
  name: string,
  opts: { taskTitle?: string | null; coordinator?: boolean } = {}
): AcWorkgroup {
  const { taskTitle = "Ship the thing", coordinator = true } = opts;
  return {
    name,
    path: `${projectPath}\\.ac\\${name}`,
    task: null,
    taskTitle,
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

const defaultWorkgroups = () => [
  wg("wg-1-dev-team"),
  wg("wg-2-rust-team"),
  wg("wg-3-docs-team"),
];

/** A session for the workgroup's coordinator with a raised hand up. */
function raisedSession(wgName: string, overrides: Partial<Session> = {}): Session {
  return session({
    id: `session-${wgName}`,
    name: `${wgName}/dev-webpage-ui`,
    workingDirectory: `${projectPath}\\.ac\\${wgName}\\__agent_dev-webpage-ui`,
    status: "running",
    isCoordinator: true,
    communication: { kind: "raiseHand", visible: true, updatedAt: "2026-07-03T00:00:00Z" },
    ...overrides,
  });
}

function target<T extends Element>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing element ${testId}`);
  return element;
}

function railButtonOrder(): string[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>('[data-ac-testid^="workgroupGroups.button."]')
  ).map((button) => button.dataset.acTestid!.replace("workgroupGroups.button.", ""));
}

function railRaiseHands(): string[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>('[data-ac-testid^="workgroupGroups.raiseHand."]')
  ).map((el) => el.dataset.acTestid!.replace("workgroupGroups.raiseHand.", ""));
}

describe("workgroupHasRaisedHand (#763 predicate — mirrors the #747 row)", () => {
  beforeEach(() => resetUiStoresForTests());
  afterEach(() => resetUiStoresForTests());

  it("is true for a coordinator with a visible raised hand and a task title", () => {
    sessionsStore.setSessions([raisedSession("wg-1-dev-team")]);
    expect(workgroupHasRaisedHand(wg("wg-1-dev-team"))).toBe(true);
  });

  it("stays true when the coordinator session has EXITED (no live gate — #747/decision C)", () => {
    // The whole point of decision C: a restored/dormant coordinator keeps its
    // persisted hand, so the tab must light even though the PTY is gone.
    sessionsStore.setSessions([raisedSession("wg-1-dev-team", { status: { exited: 0 } })]);
    expect(workgroupHasRaisedHand(wg("wg-1-dev-team"))).toBe(true);
  });

  it("is false when the hand is not visible", () => {
    sessionsStore.setSessions([
      raisedSession("wg-1-dev-team", {
        communication: { kind: "raiseHand", visible: false, updatedAt: "2026-07-03T00:00:00Z" },
      }),
    ]);
    expect(workgroupHasRaisedHand(wg("wg-1-dev-team"))).toBe(false);
  });

  it("is false when there is no communication at all", () => {
    sessionsStore.setSessions([raisedSession("wg-1-dev-team", { communication: null })]);
    expect(workgroupHasRaisedHand(wg("wg-1-dev-team"))).toBe(false);
  });

  it("is false when the workgroup has no task title (mirrors the row's !!taskTitle gate)", () => {
    sessionsStore.setSessions([raisedSession("wg-1-dev-team")]);
    expect(workgroupHasRaisedHand(wg("wg-1-dev-team", { taskTitle: null }))).toBe(false);
  });

  it("is false when the raised replica is not a coordinator", () => {
    sessionsStore.setSessions([raisedSession("wg-1-dev-team", { isCoordinator: false })]);
    expect(workgroupHasRaisedHand(wg("wg-1-dev-team", { coordinator: false }))).toBe(false);
  });

  it("is false when no session matches the replica", () => {
    sessionsStore.setSessions([]);
    expect(workgroupHasRaisedHand(wg("wg-1-dev-team"))).toBe(false);
  });
});

describe("WorkgroupGroupRail raise-hand badge (#763 render + aggregation)", () => {
  beforeEach(() => resetUiStoresForTests());
  afterEach(() => {
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("lights the amber badge on All and on the group whose coordinator raised a hand", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_project_groups", groupsConfig());
    sessionsStore.setSessions([raisedSession("wg-1-dev-team")]);

    const rendered = renderWithFakeTransport(
      () => <WorkgroupGroupRail projects={[project(defaultWorkgroups())]} />,
      fake
    );
    try {
      await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust"]));
      // Aggregates up to All; lights the matching group; leaves the rest dark.
      await waitFor(() => expect(railRaiseHands()).toEqual(["all", "ui"]));

      const badge = target<HTMLElement>("workgroupGroups.raiseHand.ui");
      expect(badge.classList.contains("workgroup-group-rail-raise-hand")).toBe(true);
      expect(badge.textContent?.trim()).toBe("!");
      expect(badge.getAttribute("aria-label")).toBe("A coordinator raised its hand");
    } finally {
      rendered.cleanup();
    }
  });

  it("aggregates onto All and Ungrouped when an ungrouped workgroup's coordinator raised", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_project_groups", groupsConfig());
    sessionsStore.setSessions([raisedSession("wg-3-docs-team")]);

    const rendered = renderWithFakeTransport(
      () => <WorkgroupGroupRail projects={[project(defaultWorkgroups())]} />,
      fake
    );
    try {
      await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust"]));
      await waitFor(() => expect(railRaiseHands()).toEqual(["all", "ungrouped"]));
    } finally {
      rendered.cleanup();
    }
  });

  it("shows no badge anywhere when no coordinator has a hand up", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_project_groups", groupsConfig());
    sessionsStore.setSessions([raisedSession("wg-1-dev-team", { communication: null })]);

    const rendered = renderWithFakeTransport(
      () => <WorkgroupGroupRail projects={[project(defaultWorkgroups())]} />,
      fake
    );
    try {
      await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust"]));
      expect(railRaiseHands()).toEqual([]);
    } finally {
      rendered.cleanup();
    }
  });

  it("reactively lights and clears the badge as the coordinator's hand toggles", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_project_groups", groupsConfig());

    const rendered = renderWithFakeTransport(
      () => <WorkgroupGroupRail projects={[project(defaultWorkgroups())]} />,
      fake
    );
    try {
      await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust"]));
      expect(railRaiseHands()).toEqual([]);

      // Hand goes up -> badge appears on All + the group.
      sessionsStore.setSessions([raisedSession("wg-1-dev-team")]);
      await waitFor(() => expect(railRaiseHands()).toEqual(["all", "ui"]));

      // User attends (backend clears communication) -> badge disappears.
      sessionsStore.setCommunication("session-wg-1-dev-team", null);
      await waitFor(() => expect(railRaiseHands()).toEqual([]));
    } finally {
      rendered.cleanup();
    }
  });
});
