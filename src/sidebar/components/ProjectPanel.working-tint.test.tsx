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
import { automationIdPart } from "./replica-repo-badges";

// #1755 leg 2 — ProjectPanel must put `working` on the right elements in the
// right states and take it off again. Every test below carries its idle control
// INSIDE the same run, so a detector that tints everything fails rather than
// passing on a lucky positive.
//
// What this leg deliberately does NOT assert is colour. jsdom does not implement
// specificity in the cascade (the boundary probe at the end of this file
// measures it, with a control), so a getComputedStyle assertion here would have
// returned the blue wash and PASSED on the round-2 defect that shipped a
// half-rendered light-theme card-sections room. The cascade is pinned from the
// bytes instead, in working-tint-css.test.ts assertion 7.

const projectPath = "C:\\Project";
const wgName = "wg-9-tint-team";
const workgroupPathOf = (wg: string): string => `${projectPath}\\.ac\\${wg}`;
const workgroupPath = workgroupPathOf(wgName);

const ORCHESTRATOR = "orchestrator";
const WORKER = "worker";
const BYSTANDER = "bystander";

// #1783 - the second room. It exists to separate "this room is working" from
// "something somewhere is working". With a single room those are the same
// proposition in every frame, so no single-room test can tell them apart. It is
// mutation M6 in the plan's section 8 that this room kills, and nothing else in
// the suite sees M6 at all.
const IDLE_ROOM = "wg-9-idle-team";
const IDLE_ORCHESTRATOR = "idle-orchestrator";
const IDLE_MEMBER = "idle-member";

const replicaPath = (name: string, wg = wgName): string =>
  `${workgroupPathOf(wg)}\\__agent_${name}`;
const sessionName = (name: string, wg = wgName): string => `${wg}/${name}`;
const sessionId = (name: string): string => `session-${name}`;

const rowTestId = (context: string, replica: string, wg = wgName): string =>
  `replica.row.${automationIdPart(context)}.${automationIdPart(wg)}.${automationIdPart(replica)}`;

function tintDiscovery() {
  return discovery({
    workgroups: [
      {
        name: wgName,
        path: workgroupPath,
        task: null,
        taskTitle: "Working tint",
        agents: [
          { name: ORCHESTRATOR, path: replicaPath(ORCHESTRATOR), repoPaths: [], isCoordinator: true },
          { name: WORKER, path: replicaPath(WORKER), repoPaths: [], isCoordinator: false },
          { name: BYSTANDER, path: replicaPath(BYSTANDER), repoPaths: [], isCoordinator: false },
        ],
      },
    ],
  });
}

/**
 * `status: "running"` makes a replica working; `status: "idle"` does not.
 * `waitingForInput` is carried separately because "the session exists" and "the
 * session is working" are different questions, which is assertion 5.
 */
function replicaSession(
  name: string,
  status: "running" | "idle",
  waitingForInput = false,
  wg = wgName
) {
  return session({
    id: sessionId(name),
    name: sessionName(name, wg),
    workingDirectory: replicaPath(name, wg),
    isCoordinator: name === ORCHESTRATOR || name === IDLE_ORCHESTRATOR,
    status,
    waitingForInput,
  });
}

async function mount() {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("get_settings", baseSettings());
  fake.resolve("discover_project", tintDiscovery());
  const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
  await settingsStore.load();
  await projectStore.createAndLoad(projectPath);
  await waitFor(() => expect(rendered.root.textContent).toContain(WORKER));
  return rendered;
}

function twoRoomDiscovery() {
  return discovery({
    workgroups: [
      {
        name: wgName,
        path: workgroupPathOf(wgName),
        task: null,
        taskTitle: "Working tint",
        agents: [
          { name: ORCHESTRATOR, path: replicaPath(ORCHESTRATOR), repoPaths: [], isCoordinator: true },
          { name: WORKER, path: replicaPath(WORKER), repoPaths: [], isCoordinator: false },
          { name: BYSTANDER, path: replicaPath(BYSTANDER), repoPaths: [], isCoordinator: false },
        ],
      },
      {
        name: IDLE_ROOM,
        path: workgroupPathOf(IDLE_ROOM),
        task: null,
        taskTitle: "Idle control room",
        agents: [
          {
            name: IDLE_ORCHESTRATOR,
            path: replicaPath(IDLE_ORCHESTRATOR, IDLE_ROOM),
            repoPaths: [],
            isCoordinator: true,
          },
          {
            name: IDLE_MEMBER,
            path: replicaPath(IDLE_MEMBER, IDLE_ROOM),
            repoPaths: [],
            isCoordinator: false,
          },
        ],
      },
    ],
  });
}

async function mountTwoRooms() {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("get_settings", baseSettings());
  fake.resolve("discover_project", twoRoomDiscovery());
  const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
  await settingsStore.load();
  await projectStore.createAndLoad(projectPath);
  await waitFor(() => expect(rendered.root.textContent).toContain(IDLE_MEMBER));
  return rendered;
}

const row = (root: HTMLElement, context: string, replica: string, wg = wgName): Element => {
  const el = root.querySelector(`[data-ac-testid="${rowTestId(context, replica, wg)}"]`);
  if (!el) throw new Error(`missing row: ${rowTestId(context, replica, wg)}`);
  return el;
};

const subgroups = (root: HTMLElement): Element[] => [
  ...root.querySelectorAll(".ac-wg-subgroup"),
];

const anySubgroupWorking = (root: HTMLElement): boolean =>
  subgroups(root).some((el) => el.classList.contains("working"));

describe("ProjectPanel working tint (#1755)", () => {
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

  it("1. tints the working row and leaves the idle row alone in the same render", async () => {
    const rendered = await mount();
    try {
      sessionsStore.setSessions([
        replicaSession(WORKER, "running"),
        replicaSession(BYSTANDER, "idle"),
        replicaSession(ORCHESTRATOR, "idle"),
      ]);
      await waitFor(() =>
        expect(row(rendered.root, "workgroups", WORKER).classList.contains("working")).toBe(true)
      );
      // The control, in the same run: a detector that tints every row fails here.
      expect(row(rendered.root, "workgroups", BYSTANDER).classList.contains("working")).toBe(false);
      expect(row(rendered.root, "workgroups", ORCHESTRATOR).classList.contains("working")).toBe(
        false
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("2. a selected working row carries working and active at once", async () => {
    const rendered = await mount();
    try {
      sessionsStore.setSessions([
        replicaSession(WORKER, "running"),
        replicaSession(BYSTANDER, "idle"),
        replicaSession(ORCHESTRATOR, "idle"),
      ]);
      sessionsStore.setVisibleActiveIdForTests(sessionId(WORKER));
      await waitFor(() => {
        const el = row(rendered.root, "workgroups", WORKER);
        expect(el.classList.contains("active")).toBe(true);
        expect(el.classList.contains("working")).toBe(true);
      });

      // The idle selected control: active WITHOUT working, so `working` is not
      // simply tracking selection.
      sessionsStore.setVisibleActiveIdForTests(sessionId(BYSTANDER));
      await waitFor(() => {
        const el = row(rendered.root, "workgroups", BYSTANDER);
        expect(el.classList.contains("active")).toBe(true);
        expect(el.classList.contains("working")).toBe(false);
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("3. a room where only the orchestrator works is a working room", async () => {
    // Required behaviour 4. workgroupIsWorking iterates wg.agents, which
    // contains the coordinator replica, so this needs no extra predicate.
    const rendered = await mount();
    try {
      sessionsStore.setSessions([
        replicaSession(ORCHESTRATOR, "running"),
        replicaSession(WORKER, "idle"),
        replicaSession(BYSTANDER, "idle"),
      ]);
      await waitFor(() => expect(anySubgroupWorking(rendered.root)).toBe(true));
      // Control inside the same run: the two idle member rows stay untinted, so
      // the group class is not leaking onto rows.
      expect(row(rendered.root, "workgroups", WORKER).classList.contains("working")).toBe(false);
      expect(row(rendered.root, "workgroups", BYSTANDER).classList.contains("working")).toBe(false);
      expect(row(rendered.root, "workgroups", ORCHESTRATOR).classList.contains("working")).toBe(
        true
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("4. a room where nobody works has no group class and no row class", async () => {
    const rendered = await mount();
    try {
      sessionsStore.setSessions([
        replicaSession(ORCHESTRATOR, "idle"),
        replicaSession(WORKER, "idle"),
        replicaSession(BYSTANDER, "idle"),
      ]);
      await waitFor(() => expect(subgroups(rendered.root).length).toBeGreaterThan(0));
      expect(anySubgroupWorking(rendered.root)).toBe(false);
      for (const name of [ORCHESTRATOR, WORKER, BYSTANDER]) {
        expect(row(rendered.root, "workgroups", name).classList.contains("working")).toBe(false);
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("5. waiting for input is not working", async () => {
    const rendered = await mount();
    try {
      sessionsStore.setSessions([
        // Same status as the positive control in test 1; only waitingForInput differs.
        replicaSession(WORKER, "running", true),
        replicaSession(BYSTANDER, "running"),
        replicaSession(ORCHESTRATOR, "idle"),
      ]);
      // The control is the other running row, which IS working, so this cannot
      // pass by tinting nothing at all.
      await waitFor(() =>
        expect(row(rendered.root, "workgroups", BYSTANDER).classList.contains("working")).toBe(true)
      );
      expect(row(rendered.root, "workgroups", WORKER).classList.contains("working")).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });

  it("6. the row and group classes clear when the last worker stops", async () => {
    // The deterministic observation of required behaviour 6. The fade itself is
    // a 150ms CSS transition and is not observable here.
    const rendered = await mount();
    try {
      sessionsStore.setSessions([
        replicaSession(WORKER, "running"),
        replicaSession(BYSTANDER, "idle"),
        replicaSession(ORCHESTRATOR, "idle"),
      ]);
      await waitFor(() => {
        expect(row(rendered.root, "workgroups", WORKER).classList.contains("working")).toBe(true);
        expect(anySubgroupWorking(rendered.root)).toBe(true);
      });

      sessionsStore.setSessions([
        replicaSession(WORKER, "idle"),
        replicaSession(BYSTANDER, "idle"),
        replicaSession(ORCHESTRATOR, "idle"),
      ]);
      await waitFor(() => {
        expect(row(rendered.root, "workgroups", WORKER).classList.contains("working")).toBe(false);
        expect(anySubgroupWorking(rendered.root)).toBe(false);
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("7. a quick-access row still tints when its own orchestrator session is the only one working", async () => {
    // Acceptance criterion 3. Since #1783 the quick-access predicate is
    // room-wide, so this test isolates the orchestrator: it is the ONLY working
    // agent in the room, which is the one case a peers-only implementation
    // (wiring the row to runningCoordinatorPeers) would get wrong. The two
    // structural expectations below still pin that the quick row lives in
    // .coord-quick-access and in no .ac-wg-subgroup.
    const rendered = await mount();
    try {
      sessionsStore.setSessions([
        replicaSession(ORCHESTRATOR, "running"),
        replicaSession(WORKER, "idle"),
        replicaSession(BYSTANDER, "idle"),
      ]);
      await waitFor(() =>
        expect(row(rendered.root, "workgroups", ORCHESTRATOR).classList.contains("working")).toBe(
          true
        )
      );

      const quick = row(rendered.root, "quick", ORCHESTRATOR);
      expect(quick.classList.contains("working")).toBe(true);
      expect(quick.closest(".coord-quick-access")).not.toBeNull();
      expect(quick.closest(".ac-wg-subgroup")).toBeNull();

      // Controls in the same render: the two idle members stay untinted in the
      // member list, so this cannot pass by tinting every row.
      expect(row(rendered.root, "workgroups", WORKER).classList.contains("working")).toBe(false);
      expect(row(rendered.root, "workgroups", BYSTANDER).classList.contains("working")).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });

  it("8. a quick-access row tints when a room member works and its own session is idle", async () => {
    // #1783 acceptance criteria 1 and 2. Three controls live in this one render:
    // the control room's quick row (kills a constant-true predicate, a
    // project-wide "anything is working" predicate, and a room-wide check
    // hand-rolled off raw session status), and the SAME orchestrator's
    // member-list row, which must stay untinted while its quick row tints.
    //
    // IDLE_MEMBER is deliberately "running" WITH waitingForInput rather than
    // idle. That makes the criterion-2 assertion carry two facts at once: the
    // control room has no WORKING agent, and a waiting agent is not a working
    // agent. Against a raw `status === "running"` check the room reads as busy,
    // which is mutation M7 and required behaviour 5.8. Do not simplify this
    // argument back to ("idle", false).
    const rendered = await mountTwoRooms();
    try {
      sessionsStore.setSessions([
        replicaSession(ORCHESTRATOR, "idle"),
        replicaSession(WORKER, "running"),
        replicaSession(BYSTANDER, "idle"),
        replicaSession(IDLE_ORCHESTRATOR, "idle", false, IDLE_ROOM),
        replicaSession(IDLE_MEMBER, "running", true, IDLE_ROOM),
      ]);
      // Gate only. True before and after #1783 and under every mutation probe,
      // so it settles the store without ever masking one.
      await waitFor(() =>
        expect(row(rendered.root, "workgroups", WORKER).classList.contains("working")).toBe(true)
      );

      // Criterion 1.
      expect(row(rendered.root, "quick", ORCHESTRATOR).classList.contains("working")).toBe(true);

      // Criterion 2, same render: a room with nobody working carries no tint.
      expect(
        row(rendered.root, "quick", IDLE_ORCHESTRATOR, IDLE_ROOM).classList.contains("working")
      ).toBe(false);

      // Same replica, same room, member list: still own-session only.
      expect(row(rendered.root, "workgroups", ORCHESTRATOR).classList.contains("working")).toBe(
        false
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("9. the room-wide rule does not leak into either .ac-wg-subgroup context", async () => {
    // #1783 acceptance criteria 4 and 5. renderReplicaItem is a shared factory,
    // so an edit that forgets to condition on rowContext tints every member row
    // of a working room. TWO rowContext values render inside .ac-wg-subgroup,
    // "workgroups" (ProjectPanel.tsx:2870) and "selected" (:2810), and this test
    // asserts both: a predicate branching on the complement of "workgroups"
    // passes the first half and fails the second. The positive control in each
    // half is the working member itself, so neither half can pass by tinting
    // nothing.
    const rendered = await mountTwoRooms();
    try {
      sessionsStore.setSessions([
        replicaSession(ORCHESTRATOR, "idle"),
        replicaSession(WORKER, "running"),
        replicaSession(BYSTANDER, "idle"),
        replicaSession(IDLE_ORCHESTRATOR, "idle", false, IDLE_ROOM),
        replicaSession(IDLE_MEMBER, "idle", false, IDLE_ROOM),
      ]);
      await waitFor(() =>
        expect(row(rendered.root, "workgroups", WORKER).classList.contains("working")).toBe(true)
      );

      // Criterion 4, context "workgroups": the two idle members of a WORKING
      // room stay untinted.
      expect(row(rendered.root, "workgroups", ORCHESTRATOR).classList.contains("working")).toBe(
        false
      );
      expect(row(rendered.root, "workgroups", BYSTANDER).classList.contains("working")).toBe(false);
      expect(
        row(rendered.root, "workgroups", IDLE_MEMBER, IDLE_ROOM).classList.contains("working")
      ).toBe(false);

      // Criterion 5: the room block itself still tints on room-wide work.
      expect(anySubgroupWorking(rendered.root)).toBe(true);

      // Criterion 4, context "selected". Selecting WORKER's session makes
      // selectedWorkgroup() resolve to the busy room, which renders a second
      // .ac-wg-subgroup through renderWorkgroupSubgroup(wg, "selected"). The
      // waitFor is a gate on the positive row only: it is true under the correct
      // implementation and under every mutation in section 8, and it also proves
      // the section actually rendered rather than the rows being absent.
      sessionsStore.setVisibleActiveIdForTests(sessionId(WORKER));
      await waitFor(() =>
        expect(row(rendered.root, "selected", WORKER).classList.contains("working")).toBe(true)
      );
      expect(row(rendered.root, "selected", ORCHESTRATOR).classList.contains("working")).toBe(
        false
      );
      expect(row(rendered.root, "selected", BYSTANDER).classList.contains("working")).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });

  it("boundary: jsdom resolves this cascade by order, not specificity, so leg 2 never asserts colour", () => {
    // Not one of leg 2's seven assertions. It pins the measured boundary the
    // plan relies on, so that the justification for pinning the cascade from the
    // bytes cannot go stale silently. The pair is the exact one from structural
    // fact 3: (0,3,1) declared first, (0,2,0) declared second.
    const style = document.createElement("style");
    style.textContent = [
      'html.light-theme[data-sidebar-style="card-sections"] .probe { background: rgb(1, 1, 1); }',
      ".probe.working { background: rgb(2, 2, 2); }",
    ].join("\n");
    document.head.appendChild(style);
    const host = document.createElement("div");
    host.className = "probe working";
    document.body.appendChild(host);
    document.documentElement.classList.add("light-theme");
    document.documentElement.setAttribute("data-sidebar-style", "card-sections");
    try {
      // Correct CSS gives rgb(1, 1, 1): (0,3,1) beats (0,2,0) and order never
      // breaks a specificity difference. jsdom gives the later rule.
      expect(getComputedStyle(host).backgroundColor).toBe("rgb(2, 2, 2)");

      // The control that rules out a blind probe: standing alone, the
      // high-specificity rule DOES apply, so the selector matches and only the
      // ordering is wrong.
      style.textContent =
        'html.light-theme[data-sidebar-style="card-sections"] .probe { background: rgb(1, 1, 1); }';
      expect(getComputedStyle(host).backgroundColor).toBe("rgb(1, 1, 1)");
    } finally {
      document.documentElement.classList.remove("light-theme");
      document.documentElement.removeAttribute("data-sidebar-style");
      host.remove();
      style.remove();
    }
  });
});
