// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  click,
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import { settingsStore } from "../../shared/stores/settings";
import type { AcAgentReplica, AcWorkgroup, Session } from "../../shared/types";

const projectPath = "C:\\Project";
const updatedAt = "2026-08-31T06:00:00.000Z";
const menuMessage = "Choose the authentication method";

function wgPath(wgName: string): string {
  return `${projectPath}\\.ac\\${wgName}`;
}

function replicaPath(wgName: string, replicaName: string): string {
  return `${wgPath(wgName)}\\__agent_${replicaName}`;
}

function rowSlotTestId(
  wgName: string,
  replicaName: string,
  rowContext = "quick"
): string {
  return `replica.row.${rowContext}.${wgName}.${replicaName}.communicationSlot`;
}

function replica(wgName: string, replicaName: string, isCoordinator: boolean): AcAgentReplica {
  return {
    name: replicaName,
    path: replicaPath(wgName, replicaName),
    repoPaths: [],
    isCoordinator,
  };
}

function workgroup(
  wgName: string,
  replicaName: string,
  taskTitle: string | null,
  isCoordinator: boolean
): AcWorkgroup {
  return {
    name: wgName,
    path: wgPath(wgName),
    task: null,
    taskTitle,
    agents: [replica(wgName, replicaName, isCoordinator)],
  };
}

function blockedMenuSession(
  wgName: string,
  replicaName: string,
  isCoordinator: boolean,
  message = menuMessage
): Session {
  return session({
    id: `${wgName}-${replicaName}`,
    name: `${wgName}/${replicaName}`,
    workingDirectory: replicaPath(wgName, replicaName),
    isCoordinator,
    status: "running",
    communication: {
      kind: "blockedMenu",
      visible: true,
      updatedAt,
      message,
    },
  });
}

// #1858 — one blocked-menu row and one raise-hand row in the same mount. The
// raise-hand slot only renders for a coordinator that has a task title (it
// lives inside .coord-task-line), so the two rows differ in row context too.
const blockedWg = "room-blocked";
const blockedReplica = "dev-rust";
const handWg = "room-hand";
const handReplica = "orchestrator";
const handTaskTitle = "Coordinate";
const blockedSlotSelector = `[data-ac-testid="${rowSlotTestId(blockedWg, blockedReplica, "workgroups")}"]`;
const handSlotSelector = `[data-ac-testid="${rowSlotTestId(handWg, handReplica)}"]`;

function raiseHandSession(wgName: string, replicaName: string): Session {
  return session({
    id: `${wgName}-${replicaName}`,
    name: `${wgName}/${replicaName}`,
    workingDirectory: replicaPath(wgName, replicaName),
    isCoordinator: true,
    status: "running",
    communication: {
      kind: "raiseHand",
      visible: true,
      updatedAt,
    },
  });
}

function slotGlyphPath(root: ParentNode, selector: string): string | null {
  return root.querySelector(`${selector} svg path`)?.getAttribute("d") ?? null;
}

async function mountProject(workgroups: AcWorkgroup[], sessions: Session[]) {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("get_settings", baseSettings());
  fake.resolve("discover_project", discovery({ workgroups }));
  sessionsStore.setSessions(sessions);
  const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
  await settingsStore.load();
  await projectStore.createAndLoad(projectPath);
  await waitFor(() => expect(rendered.root.querySelector(".replica-item")).not.toBeNull());
  return rendered;
}

describe("ProjectPanel blocked-menu communication slot (#1649)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: Awaited<ReturnType<typeof mountProject>> | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
  });

  it("renders the blocked-menu slot for a coordinator replica", async () => {
    const wgName = "room-coord";
    const replicaName = "orchestrator";
    rendered = await mountProject(
      [workgroup(wgName, replicaName, "Coordinate", true)],
      [blockedMenuSession(wgName, replicaName, true)]
    );

    const slot = rendered.root.querySelector(
      `[data-ac-testid="${rowSlotTestId(wgName, replicaName)}"]`
    );
    expect(slot?.getAttribute("data-kind")).toBe("blockedMenu");
    const strip = slot?.parentElement;
    expect(strip?.classList.contains("ac-discovery-badges")).toBe(true);
    expect(strip?.firstElementChild).toBe(slot);
    expect(rendered.root.querySelector(".replica-item-name-row")).toBeNull();
    expect(rendered.root.querySelector(".coord-task-line")).not.toBeNull();
  });

  it("renders the blocked-menu slot for a worker without a task title", async () => {
    const wgName = "room-worker";
    const replicaName = "dev-rust";
    rendered = await mountProject(
      [workgroup(wgName, replicaName, null, false)],
      [blockedMenuSession(wgName, replicaName, false)]
    );

    const slot = rendered.root.querySelector(
      `[data-ac-testid="${rowSlotTestId(wgName, replicaName, "workgroups")}"]`
    );
    expect(slot?.getAttribute("data-kind")).toBe("blockedMenu");
    const strip = slot?.parentElement;
    expect(strip?.classList.contains("ac-discovery-badges")).toBe(true);
    expect(strip?.firstElementChild).toBe(slot);
    const chip = strip?.querySelector<HTMLElement>(".agent-name-chip");
    expect(chip?.textContent).toBe(replicaName);
    expect(chip?.getAttribute("title")).toBe(replicaName);
    expect(rendered.root.querySelector(".replica-item-name-row")).toBeNull();
  });

  it("uses the backend message for the tooltip and exposes an accessible label", async () => {
    const wgName = "room-message";
    const replicaName = "architect";
    rendered = await mountProject(
      [workgroup(wgName, replicaName, null, false)],
      [blockedMenuSession(wgName, replicaName, false)]
    );

    const slot = rendered.root.querySelector(
      `[data-ac-testid="${rowSlotTestId(wgName, replicaName, "workgroups")}"]`
    );
    expect(slot?.getAttribute("title")).toBe(menuMessage);
    expect(slot?.getAttribute("aria-label")).toBe("Interactive menu requires user input");
  });

  // #1858 — a blocked menu and a raised hand are two different states, and until
  // this phase both rendered RaiseHandIcon inside the same amber chip. These
  // three assert on what a sighted user actually sees (the rendered `d`, the
  // modifier class) plus the automation contract that must NOT move with it.
  it("renders a different glyph for a blocked menu than for a raised hand", async () => {
    rendered = await mountProject(
      [
        workgroup(blockedWg, blockedReplica, null, false),
        workgroup(handWg, handReplica, handTaskTitle, true),
      ],
      [
        blockedMenuSession(blockedWg, blockedReplica, false),
        raiseHandSession(handWg, handReplica),
      ]
    );

    const blockedPath = slotGlyphPath(rendered.root, blockedSlotSelector);
    const handPath = slotGlyphPath(rendered.root, handSlotSelector);

    // Comparing the rendered paths, not the component names, is what makes this
    // fail if someone puts RaiseHandIcon back in the blocked-menu branch.
    expect(blockedPath).toBeTruthy();
    expect(handPath).toBeTruthy();
    expect(blockedPath).not.toBe(handPath);
  });

  it("gives the blocked-menu chip its own modifier class and leaves the raise-hand chip alone", async () => {
    rendered = await mountProject(
      [
        workgroup(blockedWg, blockedReplica, null, false),
        workgroup(handWg, handReplica, handTaskTitle, true),
      ],
      [
        blockedMenuSession(blockedWg, blockedReplica, false),
        raiseHandSession(handWg, handReplica),
      ]
    );

    const blockedSlot = rendered.root.querySelector(blockedSlotSelector);
    const handSlot = rendered.root.querySelector(handSlotSelector);

    expect(blockedSlot?.classList.contains("coord-communication-slot--blocked-menu")).toBe(true);
    expect(handSlot?.classList.contains("coord-communication-slot--blocked-menu")).toBe(false);
    expect(handSlot?.classList.contains("coord-communication-slot")).toBe(true);
  });

  it("keeps data-kind on both slots after the glyph swap", async () => {
    rendered = await mountProject(
      [
        workgroup(blockedWg, blockedReplica, null, false),
        workgroup(handWg, handReplica, handTaskTitle, true),
      ],
      [
        blockedMenuSession(blockedWg, blockedReplica, false),
        raiseHandSession(handWg, handReplica),
      ]
    );

    expect(rendered.root.querySelector(blockedSlotSelector)?.getAttribute("data-kind")).toBe(
      "blockedMenu"
    );
    expect(rendered.root.querySelector(handSlotSelector)?.getAttribute("data-kind")).toBe(
      "raiseHand"
    );
  });
});


// #1859 - the collapse-proof rollup. Three collapses and one regex filter can
// each hide the row that carries the #1858 chip, so every level that can hide a
// row now summarises what it hides. These drive the real UI (clicking the real
// headers, typing in the real filter input) rather than poking the collapse
// stores, so they fail if a badge is placed under something a collapse removes.
describe("ProjectPanel blocked-menu rollup badges (#1859)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: Awaited<ReturnType<typeof mountProject>> | null = null;

  const rollupWg = "room-rollup";
  const coordName = "orchestrator";
  const workerName = "dev-rust";
  const coordSessionId = `${rollupWg}-${coordName}`;

  const subgroupBadge = `[data-ac-testid="workgroup.header.blockedMenu.${rollupWg}"]`;
  const coordinatorsBadge = '[data-ac-testid="coordinators.header.blockedMenu"]';
  const projectBadge = '[data-ac-testid="project.header.blockedMenu"]';
  const allBadges = [subgroupBadge, coordinatorsBadge, projectBadge];

  // A blocked coordinator draws a row in BOTH sections: the orchestrators quick
  // group ("quick") and the Rooms subgroup ("workgroups").
  const quickRow = `[data-ac-testid="${rowSlotTestId(rollupWg, coordName, "quick")}"]`;
  const subgroupRow = `[data-ac-testid="${rowSlotTestId(rollupWg, coordName, "workgroups")}"]`;

  /** One workgroup, one coordinator, plus any extra replicas. */
  function rollupWorkgroup(extra: AcAgentReplica[] = []): AcWorkgroup {
    return {
      name: rollupWg,
      path: wgPath(rollupWg),
      task: null,
      taskTitle: "Coordinate",
      agents: [replica(rollupWg, coordName, true), ...extra],
    };
  }

  const present = (selector: string) => rendered!.root.querySelector(selector) !== null;
  const badgePresence = () => allBadges.map(present);

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
  });

  it("shows the row chip and all three enclosing badges with everything expanded", async () => {
    rendered = await mountProject(
      [rollupWorkgroup()],
      [blockedMenuSession(rollupWg, coordName, true)]
    );

    expect(present(quickRow)).toBe(true);
    expect(present(subgroupRow)).toBe(true);
    expect(badgePresence()).toEqual([true, true, true]);

    // The project badge is a SIBLING of the collapse button, never a child:
    // inside it, it would join the toggle's hit area and its accessible name.
    const badge = rendered.root.querySelector(projectBadge)!;
    const header = rendered.root.querySelector(".project-header")!;
    const toggle = rendered.root.querySelector(".project-header-main")!;
    expect(badge.parentElement).toBe(header);
    expect(toggle.contains(badge)).toBe(false);
    expect(badge.getAttribute("aria-label")).toBe("A session is waiting on an interactive menu");
  });

  it("keeps all three badges when the workgroup subgroup is collapsed away", async () => {
    rendered = await mountProject(
      [rollupWorkgroup()],
      [blockedMenuSession(rollupWg, coordName, true)]
    );
    expect(present(subgroupRow)).toBe(true);

    click(rendered.root.querySelector(".ac-wg-subgroup > .ac-wg-header--collapsible")!);
    await waitFor(() => expect(present(subgroupRow)).toBe(false));

    expect(badgePresence()).toEqual([true, true, true]);
  });

  it("keeps the project badge when the whole project panel is collapsed away", async () => {
    rendered = await mountProject(
      [rollupWorkgroup()],
      [blockedMenuSession(rollupWg, coordName, true)]
    );
    expect(present(quickRow)).toBe(true);

    click(rendered.root.querySelector(".project-header-main")!);
    await waitFor(() => expect(present(quickRow)).toBe(false));

    // Every row and both inner headers go with .project-content; this badge is
    // the only thing left, which is the whole point of the phase.
    expect(present(subgroupRow)).toBe(false);
    expect(badgePresence()).toEqual([false, false, true]);
  });

  it("keeps all three badges when the regex filter hides the blocked row", async () => {
    // The worker matches the pattern and is working, so (a) the subgroup still
    // renders and (b) the coordinator survives filteredCoordinatorItems via its
    // running-peer text. The coordinator matches nothing in "dev-rust", so the
    // only blocked row in the Rooms subgroup is filtered out while the subgroup
    // badge stays lit: it is lit by a row that is no longer on screen.
    rendered = await mountProject(
      [rollupWorkgroup([replica(rollupWg, workerName, false)])],
      [
        blockedMenuSession(rollupWg, coordName, true),
        session({
          id: `${rollupWg}-${workerName}`,
          name: `${rollupWg}/${workerName}`,
          workingDirectory: replicaPath(rollupWg, workerName),
          status: "running",
        }),
      ]
    );
    expect(present(subgroupRow)).toBe(true);

    input(
      rendered.root.querySelector<HTMLInputElement>(
        '[data-ac-testid="project.regexFilter.input"]'
      )!,
      workerName
    );
    await waitFor(() => expect(present(subgroupRow)).toBe(false));

    expect(badgePresence()).toEqual([true, true, true]);
  });

  it("shows no badge anywhere with nothing blocked, then lights all three when one blocks", async () => {
    rendered = await mountProject(
      [rollupWorkgroup()],
      [
        session({
          id: coordSessionId,
          name: `${rollupWg}/${coordName}`,
          workingDirectory: replicaPath(rollupWg, coordName),
          isCoordinator: true,
          status: "running",
        }),
      ]
    );

    // Positive control first: the two headers this phase adds a badge to really
    // did render, so the absences below are absences of a badge, not of a mount.
    expect(rendered.root.querySelector(".ac-wg-subgroup")).not.toBeNull();
    expect(rendered.root.querySelector(".coord-quick-access-group")).not.toBeNull();
    expect(present(quickRow)).toBe(false);
    expect(badgePresence()).toEqual([false, false, false]);

    // The half a hardcoded `false` predicate cannot survive: same mount, same
    // selectors, one blocked menu later.
    sessionsStore.setCommunication(coordSessionId, {
      kind: "blockedMenu",
      visible: true,
      updatedAt,
      message: menuMessage,
    });
    await waitFor(() => expect(badgePresence()).toEqual([true, true, true]));
  });

  it("never lights a blocked-menu badge for a raised hand, and lights all three when that session blocks", async () => {
    rendered = await mountProject([rollupWorkgroup()], [raiseHandSession(rollupWg, coordName)]);

    // The hand is up and visible on the row...
    await waitFor(() =>
      expect(rendered!.root.querySelector(quickRow)?.getAttribute("data-kind")).toBe("raiseHand")
    );
    // ...and it lights none of the three blocked-menu badges.
    expect(badgePresence()).toEqual([false, false, false]);

    // Swapping only the communication kind on that same session lights all
    // three, so the badges track `blockedMenu` and not "has any communication".
    sessionsStore.setCommunication(coordSessionId, {
      kind: "blockedMenu",
      visible: true,
      updatedAt,
      message: menuMessage,
    });
    await waitFor(() => expect(badgePresence()).toEqual([true, true, true]));
  });
});

// #1858 — these two assert on the stylesheet source, not on the DOM, and they
// have to: the test renderer never attaches sidebar.css, variables.css or
// toast.css, so getComputedStyle in jsdom cannot see any of these rules and a
// DOM test would pass against a stylesheet that does not define them at all.
describe("ProjectPanel blocked-menu chip and toast action stylesheets (#1858)", () => {
  // The sibling stylesheet guards in src/sidebar/styles read their file with
  // `new URL(relative, import.meta.url)`, which does NOT work here: this file
  // runs under `@vitest-environment jsdom`, and jsdom's URL resolves the
  // two-argument form against the document base, so that call yields
  // `http://localhost:3000/...` and readFileSync rejects it with "The URL must
  // be of scheme file". import.meta.url itself is still a file: URL, so build an
  // absolute one-argument URL from it instead. It stays a URL on purpose:
  // @types/node is deliberately not a dependency, and the node:fs declaration in
  // src/vite-env.d.ts is narrowed to a URL argument.
  const marker = "/src/sidebar/components/";
  const markerAt = import.meta.url.indexOf(marker);
  if (markerAt < 0) {
    throw new Error(`cannot locate the repo root in import.meta.url: ${import.meta.url}`);
  }
  const repoRootUrl = import.meta.url.slice(0, markerAt);
  const readSource = (relative: string): string =>
    readFileSync(new URL(`${repoRootUrl}/${relative}`), "utf8");
  const sidebarCss = readSource("src/sidebar/styles/sidebar.css");
  const variablesCss = readSource("src/sidebar/styles/variables.css");
  const toastCss = readSource("src/shared/styles/toast.css");

  it("defines the blocked-menu chip rule and a --status-blocked that is not the raise-hand amber", () => {
    expect(sidebarCss).toContain(".coord-communication-slot--blocked-menu {");

    const lightIndex = variablesCss.indexOf("html.light-theme {");
    expect(lightIndex).toBeGreaterThan(-1);
    const darkBlock = variablesCss.slice(0, lightIndex);
    const lightBlock = variablesCss.slice(lightIndex);

    const readToken = (block: string): string | null => {
      const match = /--status-blocked:\s*([^;]+);/.exec(block);
      return match ? match[1].trim() : null;
    };
    const darkValue = readToken(darkBlock);
    const lightValue = readToken(lightBlock);

    expect(darkValue).not.toBeNull();
    expect(lightValue).not.toBeNull();
    // The raise-hand chip's amber. Reusing it would defeat the whole phase.
    expect(darkValue?.toLowerCase()).not.toBe("#eab308");
    expect(lightValue?.toLowerCase()).not.toBe("#eab308");
  });

  it("gives the toast action button a rule that stops it wrapping to two lines", () => {
    const ruleStart = toastCss.indexOf(".toast-item__action {");
    expect(ruleStart).toBeGreaterThan(-1);
    const rule = toastCss.slice(ruleStart, toastCss.indexOf("}", ruleStart));
    expect(rule).toContain("white-space");
    expect(rule).toContain("nowrap");
  });
});
