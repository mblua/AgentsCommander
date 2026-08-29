// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import type {
  AcLoopSummary,
  AgentConfig,
  AppSettings,
  CodingAgentProfileResolution,
} from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  contextMenu,
  discovery,
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { replicaVolatileStore } from "../stores/replica-volatile";
import { sessionsStore } from "../stores/sessions";
import { automationIdPart } from "./replica-repo-badges";

// #710: modal-open state used to live on the per-project <For> row. A background
// discovery refresh replaces each project object reference, so SolidJS disposes
// and re-creates the row — tearing down any modal whose open-flag lived there.
// These tests drive the three reported flows to a modal and assert it survives a
// reloadProject (and, for the workgroup modal, that its live data re-resolves by
// stable identity). They mirror the restart-prompt (#537) / edit-team (#669)
// survival tests, the precedents for the same bug class.

const projectPath = "C:\\Project";
const teamName = "dev-team";
const workgroupName = "wg-1-dev-team";
const workgroupPath = `${projectPath}\\.ac\\${workgroupName}`;
const replicaName = "dev-webpage-ui";
const replicaPath = `${workgroupPath}\\__agent_${replicaName}`;
const sessionId = "sess-1";
const sessionName = `${workgroupName}/${replicaName}`;

const replicaRowSelector = `[data-ac-testid="replica.row.quick.${automationIdPart(
  workgroupName,
)}.${automationIdPart(replicaName)}"]`;

function q<T extends HTMLElement = HTMLElement>(testId: string): T | null {
  return document.body.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function codexAgent(): AgentConfig {
  return {
    id: "codex",
    label: "Codex",
    command: "codex",
    color: "#10b981",
    envs: [],
    isolatedHome: false,
  };
}

function claudeAgent(): AgentConfig {
  return {
    id: "claude",
    label: "Claude Code",
    command: "claude",
    color: "#d97706",
    envs: [],
    isolatedHome: false,
  };
}

function resolution(): CodingAgentProfileResolution {
  return {
    requestedProfile: "A",
    effectiveProfile: "A",
    fallbackChain: ["A"],
    fallbackApplied: false,
    requestedProfileInput: null,
    instanceProfileOverride: null,
    originDefaultProfile: null,
    agentDefaultProfile: null,
    warnings: [],
  };
}

/** Discovery payload with one workgroup + coordinator replica. `extraTeams`
 *  lets a test add teams on a later reload to prove the New Workgroup modal
 *  re-resolves its live `teams` prop by stable projectPath (#710); `loops` seeds
 *  Loop rows for the Edit Loop survival test. */
function discoveryResult(extraTeams: string[] = [], loops: AcLoopSummary[] = []) {
  return discovery({
    teams: [teamName, ...extraTeams].map((name) => ({
      name,
      agents: [replicaName],
      coordinator: replicaName,
    })),
    workgroups: [
      {
        name: workgroupName,
        path: workgroupPath,
        task: null,
        taskTitle: "Modal refresh",
        teamName,
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
    loops,
  });
}

const loopId = "weekday-standup";

function loopFixture(): AcLoopSummary {
  return {
    id: loopId,
    name: "Weekday standup",
    enabled: false,
    expr: "0 9 * * 1-5",
    timezone: "local",
    targetKind: "workgroupCoordinator",
    workgroup: workgroupName,
    promptPreview: "Short preview",
    busyCoordinator: "skip",
    path: `${projectPath}\\.ac\\_loop_${loopId}`,
    configPath: `${projectPath}\\.ac\\_loop_${loopId}\\config.toml`,
    lastCheckedAt: null,
    lastDueAt: null,
    lastDeliveredAt: null,
    lastResult: null,
    pendingDueAt: null,
    lastMissedClosedAt: null,
    nextDueAt: null,
  };
}

function setupTransport(fake: FakeTransport): void {
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("discover_project", discoveryResult());
  fake.resolve(
    "get_settings",
    baseSettings({ agents: [codexAgent(), claudeAgent()] }) satisfies AppSettings,
  );
  fake.resolve("resolve_coding_agent_profile", resolution());
}

/** Seed a live PTY session for the replica so its row routes to the active
 *  (running) context menu — the live-replica Coding Agent picker path. */
function seedLiveSession(): void {
  sessionsStore.setSessions([
    session({
      id: sessionId,
      name: sessionName,
      workingDirectory: replicaPath,
      status: "running",
      agentId: "codex",
      agentLabel: "Codex",
      isCoordinator: true,
    }),
  ]);
}

/** #977: replica-menu items now lead with an icon (Restart Session and Coding
 *  Agent joined the folder/broom items), so match on the label with any leading
 *  icon stripped - the same normalization menuButtonLabels uses in
 *  ProjectPanel.context-menu.test.tsx. */
function menuLabel(text: string): string {
  return text.trim().replace(/^[^A-Za-z0-9]+/, "").trim();
}

function findButtonByText(label: string): HTMLButtonElement {
  const match = Array.from(document.body.querySelectorAll("button")).find(
    (b) => menuLabel(b.textContent ?? "") === menuLabel(label),
  );
  if (!(match instanceof HTMLButtonElement)) throw new Error(`Button not found: ${label}`);
  return match;
}

/** True when the New Workgroup modal overlay is mounted (distinct from the
 *  context-menu button of the same label, which lives in a .session-context-menu). */
function newWorkgroupModalOpen(): boolean {
  return Array.from(document.querySelectorAll<HTMLElement>(".modal-overlay")).some(
    (el) => el.querySelector(".agent-modal-title")?.textContent?.trim() === "New Room",
  );
}

function workgroupTaskTitleInput(): HTMLInputElement | null {
  return document.body.querySelector<HTMLInputElement>(
    'input[placeholder="Task title (required)"]',
  );
}

function teamOptionValues(): string[] {
  return Array.from(document.body.querySelectorAll<HTMLOptionElement>(".entity-select option")).map(
    (o) => o.value,
  );
}

async function expectSecondDiscover(fake: FakeTransport): Promise<void> {
  await waitFor(() =>
    expect(fake.callsFor("discover_project").length).toBeGreaterThanOrEqual(2),
  );
}

describe("ProjectPanel modal survival across project refresh (#710)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

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

  it("keeps the New Workgroup modal + unsaved task title open across a refresh, and re-resolves its live teams", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);

    await projectStore.createAndLoad(projectPath);
    await waitFor(() => expect(rendered!.root.querySelector(".project-header")).toBeTruthy());

    contextMenu(rendered.root.querySelector(".project-header")!);
    await waitFor(() => expect(findButtonByText("New Room")).toBeTruthy());
    click(findButtonByText("New Room"));

    await waitFor(() => expect(newWorkgroupModalOpen()).toBe(true));
    const titleInput = workgroupTaskTitleInput();
    expect(titleInput).toBeTruthy();
    input(titleInput!, "Unsaved WG title");
    expect(teamOptionValues()).not.toContain("ops-team");

    // The next discovery reload returns an extra team — the modal must both
    // survive AND show the freshly discovered team (live data resolved by the
    // stable projectPath, not the disposed row object).
    fake.resolve("discover_project", discoveryResult(["ops-team"]));
    await projectStore.reloadProject(projectPath);
    await expectSecondDiscover(fake);

    expect(newWorkgroupModalOpen()).toBe(true);
    expect(workgroupTaskTitleInput()?.value).toBe("Unsaved WG title");
    expect(teamOptionValues()).toContain("ops-team");
  });

  it("keeps the live-replica Coding Agent picker open across a refresh", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);

    seedLiveSession();
    await projectStore.createAndLoad(projectPath);
    await waitFor(() => expect(rendered!.root.querySelector(replicaRowSelector)).toBeTruthy());

    contextMenu(rendered.root.querySelector(replicaRowSelector)!);
    await waitFor(() => expect(findButtonByText("Coding Agent")).toBeTruthy());
    click(findButtonByText("Coding Agent"));

    await waitFor(() => expect(q("agentPicker.modal")).toBeTruthy());

    // Background discovery refresh WITH changed data: rebuilds the project
    // object → re-creates the <For> row. (#748 made an identical snapshot a
    // no-op, so the refresh must carry a real change to exercise re-creation.)
    // Before #710 this disposed the per-row signal and the picker vanished;
    // hoisted to the stable root and resolved by sessionId, it survives.
    fake.resolve("discover_project", discoveryResult(["ops-team"]));
    await projectStore.reloadProject(projectPath);
    await expectSecondDiscover(fake);
    expect(q("agentPicker.modal")).toBeTruthy();

    // A replica-branch event lands in the volatile store (#748) and must not
    // disturb the modal either.
    replicaVolatileStore.setRepoBranch(replicaPath, "feature/x");
    expect(q("agentPicker.modal")).toBeTruthy();
  });

  it("keeps the inactive-replica Coding Agent picker open across a refresh", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);

    // No live session: the coordinator row is gray and right-clicks into the
    // inactive (not-running) context menu (#545), whose target previously held
    // disposable wg/replica object refs (#710).
    await projectStore.createAndLoad(projectPath);
    await waitFor(() => expect(rendered!.root.querySelector(replicaRowSelector)).toBeTruthy());

    contextMenu(rendered.root.querySelector(replicaRowSelector)!);
    await waitFor(() => expect(findButtonByText("Coding Agent")).toBeTruthy());
    click(findButtonByText("Coding Agent"));

    await waitFor(() => expect(q("agentPicker.modal")).toBeTruthy());

    // Changed snapshot so the reload still re-creates the row (#748).
    fake.resolve("discover_project", discoveryResult(["ops-team"]));
    await projectStore.reloadProject(projectPath);
    await expectSecondDiscover(fake);
    // Re-resolved by stable project/wg/replica paths, so the picker stays open
    // with fresh data instead of being disposed with the row.
    expect(q("agentPicker.modal")).toBeTruthy();

    replicaVolatileStore.setRepoBranch(replicaPath, "feature/y");
    expect(q("agentPicker.modal")).toBeTruthy();
  });

  it("keeps the Edit Loop modal + unsaved name edit open across a refresh", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", discoveryResult([], [loopFixture()]));
    // EditLoopModal.onMount loads config and re-seeds the form once. Use a loaded
    // name distinct from the summary so the test can wait for that async re-seed
    // to settle before typing — otherwise it could clobber the typed value.
    fake.resolve("get_loop_config", {
      summary: { ...loopFixture(), name: "Loaded standup name" },
      promptBody: "Short preview",
    });
    fake.resolve("preview_cron", { nextDueAt: null });
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);

    await projectStore.createAndLoad(projectPath);

    const loopRowSelector = `[data-ac-testid="loop.row.${automationIdPart(
      projectPath,
    )}.${automationIdPart(loopId)}"]`;
    await waitFor(() => expect(rendered!.root.querySelector(loopRowSelector)).toBeTruthy());
    click(rendered.root.querySelector(loopRowSelector)!);

    // Wait until the modal's onMount config load has applied (name shows the
    // loaded value) before editing, so the edit can't be clobbered by the load.
    await waitFor(() =>
      expect(q<HTMLInputElement>("loop.edit.name")?.value).toBe("Loaded standup name"),
    );
    input(q<HTMLInputElement>("loop.edit.name")!, "Unsaved loop name");

    // The loop is re-resolved by stable id (editingLoopResolved). Before #710 the
    // refresh disposed the row's editingLoop signal and the modal vanished.
    // #748: the snapshot carries a changed loop so the reload still re-creates
    // the project row (an identical snapshot is a no-op now).
    fake.resolve(
      "discover_project",
      discoveryResult([], [{ ...loopFixture(), promptPreview: "Changed preview" }]),
    );
    await projectStore.reloadProject(projectPath);
    await expectSecondDiscover(fake);

    expect(q("loop.edit.name")).toBeTruthy();
    expect(q<HTMLInputElement>("loop.edit.name")?.value).toBe("Unsaved loop name");
  });
});
