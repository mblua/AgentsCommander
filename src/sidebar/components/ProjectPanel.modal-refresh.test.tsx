// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectPanel from "./ProjectPanel";
import type {
  AcLoopSummary,
  AgentConfig,
  AppSettings,
  ApplySelectionLockRemovalResult,
  CodingAgentProfileResolution,
  PreviewSelectionLockRemovalResult,
  ProfileAssignmentScope,
  ReplicaSelectionDefaultResult,
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

import { executeAutomationRequest, resetAutomationBridgeForTests } from "../../shared/automation-bridge";
import type { UiAutomationAction } from "../../shared/types";

// #710: modal-open state used to live on the per-project <For> row. A background
// discovery refresh replaces each project object reference, so SolidJS disposes
// and re-creates the row — tearing down any modal whose open-flag lived there.
// These tests drive the three reported flows to a modal and assert it survives a
// reloadProject (and, for the workgroup modal, that its live data re-resolves by
// stable identity). They mirror the restart-prompt (#537) / edit-team (#669)
// survival tests, the precedents for the same bug class.


// Geometry enables bridge dispatch in jsdom; it does not establish Windows
// visibility, hit-testing, physical keyboard/focus behavior or real IME coverage.
async function automate(action: UiAutomationAction, selector: string, value?: string) {
  expect(document.querySelectorAll('[data-ac-testid="' + selector + '"]')).toHaveLength(1);
  const response = await executeAutomationRequest("main", {
    requestId: action + selector, token: "test", window: "main", action, selector, value,
    expiresAtUnixMs: Date.now() + 5000,
  });
  if (!response.ok) throw new Error(response.error + ": " + response.message);
  return response.target;
}
function stubAutomationGeometry() {
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
    return { x: 100, y: 50, left: 100, top: 50, right: 200, bottom: 70,
      width: this.isConnected ? 100 : 0, height: this.isConnected ? 20 : 0, toJSON: () => ({}) } as DOMRect;
  });
  vi.spyOn(Element.prototype, "getClientRects").mockImplementation(function (this: Element) {
    const list = (this.isConnected ? [this.getBoundingClientRect()] : []) as unknown as DOMRectList;
    Object.defineProperty(list, "item", { value: (index: number) => list[index] ?? null });
    return list;
  });
}

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
            // #1943 - full persisted lock payload: unprotected, so the lock row
            // is actionable and no lock chip is drawn.
            savedPair: null,
            selectionState: "unlocked",
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
    sessionStart: "fresh",
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
  // #1943 - every command the lock bar drives, with fully populated payloads.
  fake.resolve("preview_coding_agent_profile_selection", {
    scope: "replica",
    targetCount: 1,
    liveSessionCount: 1,
    targetFingerprint: "fp-replica",
    requiresExplicitConfirmation: false,
    targets: [],
    warnings: [],
  });
  fake.resolve("apply_coding_agent_profile_selection", {
    scope: "replica",
    updatedCount: 1,
    restartedCount: 0,
    updatedReplicaPaths: [replicaPath],
    restartedSessionIds: [],
    destroyedButNotRecreatedSessionIds: [],
    targetFingerprint: "fp-replica",
    warnings: [],
    errors: [],
  });
  fake.onInvoke("preview_selection_lock_removal", (args) => {
    const scope = (args.request as { scope: ProfileAssignmentScope }).scope;
    return {
      scope,
      targetFingerprint: `fp-remove-${scope}`,
      candidateCount: 1,
      countsComplete: true,
      protectedCount: 0,
      alreadyUnlockedCount: 1,
      invalidCount: 0,
      targets: [],
      warnings: [],
    } satisfies PreviewSelectionLockRemovalResult;
  });
  fake.resolve("apply_selection_lock_removal", {
    scope: "replica",
    targetFingerprint: "fp-remove-replica",
    removedCount: 0,
    removedReplicaPaths: [],
    alreadyUnlockedPaths: [replicaPath],
    failedReplicaPaths: [],
    remainingProtectedCount: 0,
    candidateCount: 1,
    countsComplete: true,
    invalidCount: 0,
    errors: [],
    warnings: [],
  } satisfies ApplySelectionLockRemovalResult);
  const selectionDefault: ReplicaSelectionDefaultResult = {
    targetReplicaPath: replicaPath,
    matrixPath: `${projectPath}\\.ac\\_agent_${replicaName}`,
    default: null,
    defaultFingerprint: "fp-default-1",
    warnings: [],
  };
  fake.resolve("get_replica_selection_default", selectionDefault);
  fake.resolve("set_replica_selection_default", selectionDefault);
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
    'input[placeholder="Task title (optional)"]',
  );
}

function teamOptionValues(): string[] {
  return Array.from(document.body.querySelectorAll<HTMLElement>(".new-room-team-option")).map(
    (row) => row.textContent ?? "",
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
  const originalScrollIntoView = Object.getOwnPropertyDescriptor(Element.prototype, "scrollIntoView");

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    stubAutomationGeometry();
    resetAutomationBridgeForTests();
    resetUiStoresForTests();
    Object.defineProperty(Element.prototype, "scrollIntoView", { configurable: true, writable: true, value: vi.fn() });
  });

  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
    vi.restoreAllMocks();
    if (originalScrollIntoView) Object.defineProperty(Element.prototype, "scrollIntoView", originalScrollIntoView);
    else delete (Element.prototype as Partial<Element>).scrollIntoView;
  });

  it("keeps the New Workgroup modal + unsaved task title open across a refresh, and re-resolves its live teams", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);

    await projectStore.createAndLoad(projectPath);
    await waitFor(() => expect(rendered!.root.querySelector(".project-header")).toBeTruthy());

    await automate("contextClick", `project.header.${automationIdPart(projectPath)}`);
    const action = `project.action.newRoom.${automationIdPart(projectPath)}.projectMenu`;
    await waitFor(() => expect(q(action)).toBeTruthy());
    await automate("click", action);

    await waitFor(() => expect(newWorkgroupModalOpen()).toBe(true));
    const titleInput = workgroupTaskTitleInput();
    expect(titleInput).toBeTruthy();
    input(titleInput!, "Unsaved WG title");
    const search = document.querySelector<HTMLInputElement>("#new-room-team-search")!;
    // The initial unique team is confirmed and the input shows its exact name.
    expect(search.value).toBe(teamName);
    input(search, "");
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
    input(search, "  DEV ");
    expect((await automate("query", "newRoom.team.option.0")).text).toBe(teamName);
    await automate("click", "newRoom.team.option.0");
    expect(search.value).toBe(teamName);
    const create = document.querySelector<HTMLButtonElement>(".new-agent-create-btn")!;
    expect(create.disabled).toBe(false);
    fake.resolve("discover_project", discoveryResult(["dev-extra", "ops-other"]));
    await projectStore.reloadProject(projectPath);
    expect(search.value).toBe(teamName);
    expect(workgroupTaskTitleInput()?.value).toBe("Unsaved WG title");
    expect(teamOptionValues()).toEqual([teamName]);
    expect((await automate("query", "newRoom.team.confirmed")).text).toBe(`Selected team: ${teamName}`);
    const removed = discoveryResult(["dev-extra", "ops-other"]);
    removed.teams = removed.teams.filter(team => team.name !== teamName);
    fake.resolve("discover_project", removed);
    await projectStore.reloadProject(projectPath);
    expect(newWorkgroupModalOpen()).toBe(true);
    expect(search.value).toBe("");
    expect(workgroupTaskTitleInput()?.value).toBe("Unsaved WG title");
    expect(teamOptionValues()).toEqual(["dev-extra", "ops-other"]);
    expect((await automate("query", "newRoom.team.confirmed")).text).toBe("No team selected.");
    expect(create.disabled).toBe(true);

  });

  it("projects Clean and explicit task titles with distinct semantic states", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    fake.resolve("list_unresolved_loop_targets", []);
    const result = discoveryResult();
    const original = result.workgroups[0];
    result.workgroups = [
      { ...original, taskTitle: "Clean" },
      { ...original, name: "room-2-dev-team", path: projectPath + "\\.ac\\room-2-dev-team", taskTitle: "USER: Fixture title", agents: [] },
    ];
    fake.resolve("discover_project", result);
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    await projectStore.createAndLoad(projectPath);
    for (const [name, text, state] of [[workgroupName, "Clean", "clean"], ["room-2-dev-team", "USER: Fixture title", "task"]]) {
      const selector = `workgroup.taskTitle.${automationIdPart(projectPath)}.workgroups.${automationIdPart(name)}`;
      await waitFor(() => expect(q(selector)).toBeTruthy());
      const target = await automate("query", selector);
      expect(target.text).toBe(text);
      expect(target.role).toBe("text");
      expect(target.state).toBe(state);
    }
  });

  it("keeps an unconfirmed team query, open list and title draft across a discovery refresh", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    fake.resolve("list_unresolved_loop_targets", []);
    fake.resolve("discover_project", discoveryResult(["ops-team"]));
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    await projectStore.createAndLoad(projectPath);
    await waitFor(() => expect(rendered!.root.querySelector(".project-header")).toBeTruthy());
    await automate("contextClick", `project.header.${automationIdPart(projectPath)}`);
    const action = `project.action.newRoom.${automationIdPart(projectPath)}.projectMenu`;
    await waitFor(() => expect(q(action)).toBeTruthy());
    await automate("click", action);
    await waitFor(() => expect(newWorkgroupModalOpen()).toBe(true));
    const search = document.querySelector<HTMLInputElement>("#new-room-team-search")!;
    input(workgroupTaskTitleInput()!, "Free-query draft");
    input(search, "  DEV ");
    await automate("key", "newRoom.teamSearch", "ArrowDown");
    fake.resolve("discover_project", discoveryResult(["dev-extra", "ops-other"]));
    await projectStore.reloadProject(projectPath);
    await expectSecondDiscover(fake);
    expect(newWorkgroupModalOpen()).toBe(true);
    expect(document.querySelector("#new-room-team-search")).toBe(search);
    expect(search.value).toBe("  DEV ");
    expect(search.getAttribute("aria-expanded")).toBe("true");
    expect(search.hasAttribute("aria-activedescendant")).toBe(false);
    expect(workgroupTaskTitleInput()?.value).toBe("Free-query draft");
    expect(teamOptionValues()).toEqual([teamName, "dev-extra"]);
    const projection = await automate("query", "newRoom.team.list");
    expect(JSON.parse(projection.metadata.detail)).toEqual({ options: [teamName, "dev-extra"], active: -1 });
    expect((await automate("query", "newRoom.team.confirmed")).state).toBe("unconfirmed");
    expect((await automate("query", "newRoom.create")).disabled).toBe(true);
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
