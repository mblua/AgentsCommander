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
const workgroupPath = `${projectPath}\\.ac\\${wgName}`;

const ORCHESTRATOR = "orchestrator";
const WORKER = "worker";
const BYSTANDER = "bystander";

const replicaPath = (name: string): string => `${workgroupPath}\\__agent_${name}`;
const sessionName = (name: string): string => `${wgName}/${name}`;
const sessionId = (name: string): string => `session-${name}`;

const rowTestId = (context: string, replica: string): string =>
  `replica.row.${automationIdPart(context)}.${automationIdPart(wgName)}.${automationIdPart(replica)}`;

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
  waitingForInput = false
) {
  return session({
    id: sessionId(name),
    name: sessionName(name),
    workingDirectory: replicaPath(name),
    isCoordinator: name === ORCHESTRATOR,
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

const row = (root: HTMLElement, context: string, replica: string): Element => {
  const el = root.querySelector(`[data-ac-testid="${rowTestId(context, replica)}"]`);
  if (!el) throw new Error(`missing row: ${rowTestId(context, replica)}`);
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

  it("7. both render sites are covered: quick access and the room member list", async () => {
    // One is inside .coord-quick-access and the other inside .ac-wg-subgroup, and
    // one class toggle in renderReplicaItem has to cover both.
    const rendered = await mount();
    try {
      sessionsStore.setSessions([
        replicaSession(ORCHESTRATOR, "running"),
        replicaSession(WORKER, "running"),
        replicaSession(BYSTANDER, "idle"),
      ]);
      await waitFor(() =>
        expect(row(rendered.root, "quick", ORCHESTRATOR).classList.contains("working")).toBe(true)
      );
      expect(row(rendered.root, "workgroups", WORKER).classList.contains("working")).toBe(true);

      // Controls in the same run: the quick-access row is inside
      // .coord-quick-access and NOT inside any .ac-wg-subgroup, which is the
      // structural fact that makes ground 1 real; and the idle member stays
      // untinted in both sections.
      const quick = row(rendered.root, "quick", ORCHESTRATOR);
      expect(quick.closest(".coord-quick-access")).not.toBeNull();
      expect(quick.closest(".ac-wg-subgroup")).toBeNull();
      expect(row(rendered.root, "workgroups", BYSTANDER).classList.contains("working")).toBe(false);
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
