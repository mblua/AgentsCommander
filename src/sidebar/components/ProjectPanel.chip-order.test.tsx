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
import { replicaVolatileStore } from "../stores/replica-volatile";
import { settingsStore } from "../../shared/stores/settings";
import { automationIdPart } from "./replica-repo-badges";
import type { AcWorkgroup, AppSettings, Session } from "../../shared/types";

// #1730 - the replica row's chip strip. Nothing in the suite reads the child
// list of .ac-discovery-badges, so without this file an implementation that
// adds agent-name-chip and leaves the other eleven chips in today's order
// passes typecheck, the whole test run and every CI job. The order itself is
// the subject here; the chip's text and title are pinned as well because they
// are the row's only identity after the name line is deleted.

const projectPath = "C:\\Project";
const wgName = "room-chip";
const wgPath = `${projectPath}\\.ac\\${wgName}`;
const coordName = "orchestrator";
const coordPath = `${wgPath}\\__agent_${coordName}`;
const peerName = "dev-rust";
const peerPath = `${wgPath}\\__agent_${peerName}`;
const originProject = "AcmeProject";
const coordSessionId = "coord-session";
const CTX_PATTERN = "Context left until auto-compact: (\\d+)%";

function rowTestId(rowContext: string, workgroup: string, replica: string): string {
  return `replica.row.${automationIdPart(rowContext)}.${automationIdPart(
    workgroup,
  )}.${automationIdPart(replica)}`;
}

function isoMinutesAgo(minutes: number): string {
  return new Date(Date.now() - minutes * 60_000).toISOString();
}

function chipSettings(): AppSettings {
  return baseSettings({
    agents: [
      {
        id: "codex",
        label: "Codex",
        command: "codex",
        color: "#888888",
        envs: [],
        isolatedHome: false,
        contextRegex: CTX_PATTERN,
      },
    ],
  });
}

// The maximal row: one workgroup with a task title, a coordinator that lights
// every gate of the strip, and one working peer so the running-peer chip
// renders. Of renderReplicaItem's two call sites only the .coord-quick-access
// one, the call that passes "quick" as its row context, passes extraBadge,
// runningPeers and taskTitle, so it is the only row that can light them all.
function maximalWorkgroup(): AcWorkgroup {
  return {
    name: wgName,
    path: wgPath,
    task: null,
    taskTitle: "Coordinate",
    agents: [
      {
        name: coordName,
        path: coordPath,
        originProject,
        repoPaths: [],
        isCoordinator: true,
        lastUserMessageAt: isoMinutesAgo(90),
      },
      {
        name: peerName,
        path: peerPath,
        repoPaths: [],
        isCoordinator: false,
      },
    ],
  };
}

function coordSession(overrides: Partial<Session> = {}): Session {
  return session({
    id: coordSessionId,
    name: `${wgName}/${coordName}`,
    workingDirectory: coordPath,
    isCoordinator: true,
    status: "running",
    agentId: "codex",
    agentLabel: "Codex",
    effectiveProfile: "B",
    profileOutdated: true,
    communication: {
      kind: "blockedMenu",
      visible: true,
      updatedAt: "2026-09-01T06:00:00.000Z",
      message: "Interactive menu requires user input",
    },
    gitRepos: [
      { label: "alpha", sourcePath: "C:\\repos\\alpha", branch: "main", dirty: false },
      { label: "beta", sourcePath: "C:\\repos\\beta", branch: "dev", dirty: false },
    ],
    ...overrides,
  });
}

function peerSession(): Session {
  return session({
    id: "peer-session",
    name: `${wgName}/${peerName}`,
    workingDirectory: peerPath,
    status: "running",
  });
}

async function mountProject(workgroups: AcWorkgroup[], sessions: Session[]) {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("get_settings", chipSettings());
  fake.resolve("discover_project", discovery({ workgroups }));
  sessionsStore.setSessions(sessions);
  const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
  await settingsStore.load();
  await projectStore.createAndLoad(projectPath);
  await waitFor(() => expect(rendered.root.querySelector(".replica-item")).not.toBeNull());
  return rendered;
}

function stripOf(root: ParentNode, rowContext: string, replicaName: string): HTMLElement {
  const row = root.querySelector<HTMLElement>(
    `[data-ac-testid="${rowTestId(rowContext, wgName, replicaName)}"]`,
  );
  if (!row) throw new Error(`row not rendered: ${replicaName}`);
  const strip = row.querySelector<HTMLElement>(".ac-discovery-badges");
  if (!strip) throw new Error(`badge strip not rendered: ${replicaName}`);
  return strip;
}

describe("ProjectPanel replica chip strip order (#1730)", () => {
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
    document.body.replaceChildren();
  });

  it("orders every chip of a maximal quick-access coordinator row", async () => {
    sessionsStore.setSessionContext(coordSessionId, 42);
    rendered = await mountProject(
      [maximalWorkgroup()],
      [coordSession(), peerSession()],
    );

    const row = rendered.root.querySelector<HTMLElement>(
      `[data-ac-testid="${rowTestId("quick", wgName, coordName)}"]`,
    )!;
    const strip = stripOf(rendered.root, "quick", coordName);
    const children = Array.from(strip.children) as HTMLElement[];
    const classNames = children.map((el) => el.className);

    expect(classNames).toHaveLength(12);
    expect(classNames[0]).toBe(
      "coord-communication-slot coord-communication-slot--blocked-menu",
    );
    expect(classNames[1]).toBe("profile-outdated-badge");
    // The idle pill appends a level token (COORD_IDLE_CLASS), so match by token.
    expect(children[2].classList.contains("ac-discovery-badge")).toBe(true);
    expect(children[2].classList.contains("coord-idle")).toBe(true);
    expect(classNames[3]).toBe("agent-name-chip");
    expect(classNames[4]).toBe("ac-discovery-badge coord");
    expect(classNames[5]).toBe("ac-discovery-badge agent");
    expect(classNames[6]).toBe("profile-badge");
    // ctxVisible() does not imply a reading; the seeded 42 keeps this off the
    // "ctx-badge unavailable" shape.
    expect(classNames[7]).toBe("ctx-badge");
    expect(classNames[8]).toBe("ac-discovery-badge team");
    expect(classNames[9]).toBe("ac-discovery-badge running-peer");
    // Neither fixture repo is dirty, so both are the bare branch class.
    expect(classNames[10]).toBe("ac-discovery-badge branch");
    expect(classNames[11]).toBe("ac-discovery-badge branch");

    // Nothing added and nothing removed: the eleven chips of today plus the
    // moved slot and the new chip. Enumerated, NOT compared against a second
    // render: on main the slot is not a child of this strip at all.
    const tokens = new Set(
      children.flatMap((el) => Array.from(el.classList)).filter(
        (token) => token !== "agent-name-chip" && token !== "red" &&
          token !== "coord-communication-slot--blocked-menu",
      ),
    );
    expect(tokens).toEqual(
      new Set([
        "coord-communication-slot",
        "profile-outdated-badge",
        "ac-discovery-badge",
        "coord-idle",
        "coord",
        "agent",
        "profile-badge",
        "ctx-badge",
        "team",
        "running-peer",
        "branch",
      ]),
    );

    const chips = strip.querySelectorAll<HTMLElement>(".agent-name-chip");
    expect(chips).toHaveLength(1);
    expect(chips[0].textContent).toBe(coordName);
    expect(chips[0].getAttribute("title")).toBe(`${coordName}@${originProject}`);

    expect(row.querySelector(".replica-item-name-row")).toBeNull();
    expect(row.querySelector(".replica-item-name")).toBeNull();
    expect(row.textContent).not.toContain("@");
    // This fixture attaches no bridge, the negative half of AC 2.
    expect(row.querySelector(".session-item-bridge-icon")).toBeNull();
  });

  it("lands the auto-closed pill at index 2 with the chip still at index 3", async () => {
    sessionsStore.setSessionContext(coordSessionId, 42);
    rendered = await mountProject(
      [maximalWorkgroup()],
      [coordSession({ status: { exited: 0 } }), peerSession()],
    );
    replicaVolatileStore.setAutoClosedAt(coordPath, isoMinutesAgo(1));

    await waitFor(() =>
      expect(rendered!.root.querySelector(".coord-autoclosed")).not.toBeNull(),
    );

    const strip = stripOf(rendered.root, "quick", coordName);
    const children = Array.from(strip.children) as HTMLElement[];
    expect(children[2].className).toBe("ac-discovery-badge coord-autoclosed");
    expect(children[2].textContent).toBe("AUTO-CLOSED");
    expect(children[3].className).toBe("agent-name-chip");
    expect(strip.querySelector(".coord-idle")).toBeNull();
    // Exactly one member of the XOR trio renders. Its contiguity is vacuous
    // (the gates let at most one through), so nothing here asserts it.
    expect(
      strip.querySelectorAll(".coord-idle, .coord-autoclosed"),
    ).toHaveLength(1);
  });

  it("renders the chip as the only child on a worker row from the other call site", async () => {
    rendered = await mountProject(
      [
        {
          name: wgName,
          path: wgPath,
          task: null,
          taskTitle: null,
          agents: [
            { name: peerName, path: peerPath, repoPaths: [], isCoordinator: false },
          ],
        },
      ],
      [],
    );

    const row = rendered.root.querySelector<HTMLElement>(
      `[data-ac-testid="${rowTestId("workgroups", wgName, peerName)}"]`,
    )!;
    expect(row).not.toBeNull();
    const strip = stripOf(rendered.root, "workgroups", peerName);
    const classNames = Array.from(strip.children).map((el) => el.className);
    expect(classNames).toEqual(["agent-name-chip"]);
    expect(strip.querySelector(".agent-name-chip")?.textContent).toBe(peerName);
    expect(row.querySelector(".replica-item-name-row")).toBeNull();
  });
});
