// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import type {
  AgentConfig,
  ApplyCodingAgentProfileSelectionResult,
  AppSettings,
  CodingAgentProfileResolution,
  PreviewCodingAgentProfileSelectionResult,
} from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  contextMenu,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import { automationIdPart } from "./replica-repo-badges";

const projectPath = "C:\\Project";
const workgroupName = "wg-1-dev-team";
const workgroupPath = `${projectPath}\\.ac\\${workgroupName}`;
const replicaName = "dev-webpage-ui";
const replicaPath = `${workgroupPath}\\__agent_${replicaName}`;
const sessionId = "sess-1";
const sessionName = `${workgroupName}/${replicaName}`;

const rowSelector = `[data-ac-testid="replica.row.quick.${automationIdPart(workgroupName)}.${automationIdPart(replicaName)}"]`;

function q<T extends HTMLElement = HTMLElement>(testId: string): T | null {
  return document.body.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function codexAgent(): AgentConfig {
  return {
    id: "codex",
    label: "Codex",
    command: "codex",
    color: "#10b981",
    gitPullBefore: false,
    excludeGlobalClaudeMd: true,
    envs: [],
    isolatedHome: false,
  };
}

// #551: a second coding agent so the test can make a *real* re-assignment.
// Re-assigning the live session's current agent (codex) is now disabled, so the
// restart prompt is reached by switching to a different coding agent.
function claudeAgent(): AgentConfig {
  return {
    id: "claude",
    label: "Claude Code",
    command: "claude",
    color: "#d97706",
    gitPullBefore: false,
    excludeGlobalClaudeMd: false,
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

function previewResult(): PreviewCodingAgentProfileSelectionResult {
  return {
    scope: "replica",
    targetCount: 1,
    liveSessionCount: 1,
    targetFingerprint: "fp-replica",
    requiresExplicitConfirmation: false,
    targets: [
      {
        workgroupName,
        workgroupPath,
        replicaName,
        replicaPath,
        identityPath: `${replicaPath}\\identity.json`,
        originProject: "Project",
        liveSessionIds: [sessionId],
      },
    ],
    warnings: [],
  };
}

function applyResult(): ApplyCodingAgentProfileSelectionResult {
  return {
    scope: "replica",
    updatedCount: 1,
    restartedCount: 0,
    updatedReplicaPaths: [replicaPath],
    restartedSessionIds: [],
    destroyedButNotRecreatedSessionIds: [],
    targetFingerprint: "fp-replica",
    warnings: [],
    errors: [],
  };
}

function discoveryResult() {
  return discovery({
    teams: [{ name: "dev-team", agents: [replicaName], coordinator: replicaName }],
    workgroups: [
      {
        name: workgroupName,
        path: workgroupPath,
        task: null,
        taskTitle: "Restart prompt",
        teamName: "dev-team",
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
  });
}

function setupTransport(fake: FakeTransport): void {
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("discover_project", discoveryResult());
  fake.resolve(
    "get_settings",
    baseSettings({ agents: [codexAgent(), claudeAgent()] }) satisfies AppSettings,
  );
  fake.resolve("resolve_coding_agent_profile", resolution());
  fake.resolve("preview_coding_agent_profile_selection", previewResult());
  fake.resolve("apply_coding_agent_profile_selection", applyResult());
  fake.resolve(
    "restart_session",
    session({ id: sessionId, name: sessionName, workingDirectory: replicaPath }),
  );
}

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

function findButtonByText(label: string): HTMLButtonElement {
  const match = Array.from(document.body.querySelectorAll("button")).find(
    (b) => b.textContent?.trim() === label,
  );
  if (!(match instanceof HTMLButtonElement)) throw new Error(`Button not found: ${label}`);
  return match;
}

/** Drive the real UI to the post-assign "Restart now?" prompt: load the project,
 *  right-click the live replica, pick Coding Agent, then apply a replica-scope
 *  selection. Leaves the RestartPromptModal open in document.body. */
async function driveToRestartPrompt(root: HTMLElement): Promise<void> {
  seedLiveSession();
  await projectStore.createAndLoad(projectPath);

  await waitFor(() => expect(root.querySelector(rowSelector)).toBeTruthy());
  const row = root.querySelector(rowSelector);
  if (!(row instanceof HTMLElement)) throw new Error("Replica row not found");

  contextMenu(row);
  await waitFor(() => expect(findButtonByText("Coding Agent")).toBeTruthy());
  click(findButtonByText("Coding Agent"));

  await waitFor(() => expect(q("agentPicker.modal")).toBeTruthy());
  // #551: the picker opens pre-selected to the live session's current Coding Agent
  // (codex), so "Assign to this replica" starts disabled (re-assigning the same
  // pair is a no-op). Switch to a different coding agent to make it a real
  // re-assignment that enables apply and pops the post-assign restart prompt.
  await waitFor(() => expect(q("agentPicker.provider.claude")).toBeTruthy());
  click(q<HTMLButtonElement>("agentPicker.provider.claude")!);
  await waitFor(() => {
    const apply = q<HTMLButtonElement>("agentPicker.apply");
    expect(apply && apply.disabled).toBe(false);
  });
  click(q<HTMLButtonElement>("agentPicker.apply")!);

  await waitFor(() => expect(q("restartPrompt.modal")).toBeTruthy());
}

describe("ProjectPanel post-assign restart prompt (#537)", () => {
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

  it("keeps the prompt open across background replica-list refreshes and treats Later as a no-op", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await driveToRestartPrompt(rendered.root);

      // A discovery poll re-discovers the project and replaces its object
      // reference (projectStore.reloadProject), re-creating the projects <For>
      // row. Before the fix this disposed the per-row signal and the modal
      // vanished; hoisted to the stable root, it must survive.
      await projectStore.reloadProject(projectPath);
      await waitFor(() => expect(fake.callsFor("discover_project").length).toBeGreaterThanOrEqual(2));
      expect(q("restartPrompt.modal")).toBeTruthy();

      // The same resilience must hold for a replica-branch update, which rebuilds
      // every project object in the store.
      projectStore.updateReplicaBranch(replicaPath, "feature/x");
      expect(q("restartPrompt.modal")).toBeTruthy();

      // Later dismisses without restarting the live session.
      click(q<HTMLButtonElement>("restartPrompt.later")!);
      await waitFor(() => expect(q("restartPrompt.modal")).toBeNull());
      expect(fake.callsFor("restart_session")).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("restarts the live session exactly once on Restart now after a refresh", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await driveToRestartPrompt(rendered.root);

      await projectStore.reloadProject(projectPath);
      await waitFor(() => expect(fake.callsFor("discover_project").length).toBeGreaterThanOrEqual(2));
      expect(q("restartPrompt.modal")).toBeTruthy();

      click(q<HTMLButtonElement>("restartPrompt.restart")!);

      await waitFor(() => expect(fake.callsFor("restart_session")).toHaveLength(1));
      expect(fake.lastCall("restart_session")?.args).toEqual(
        expect.objectContaining({
          id: sessionId,
          agentId: "claude",
          requestedProfile: "A",
        }),
      );
      await waitFor(() => expect(q("restartPrompt.modal")).toBeNull());
      // A single click must not fire a second restart.
      expect(fake.callsFor("restart_session")).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });
});
