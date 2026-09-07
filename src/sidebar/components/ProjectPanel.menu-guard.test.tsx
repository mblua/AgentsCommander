// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
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
