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
import type { AcAgentReplica, AgentConfig } from "../../shared/types";

// #733 — a shut-down replica (gray, no live session) must still advertise its
// last Coding Agent + Profile. The row's three badge helpers now fall back to the
// persisted replica config (currentCodingAgentId/currentProfile, filled at
// discovery from disk) when session() is undefined, mirroring repoBadges(). Per
// Maria's scope decision this applies to ALL dormant replicas — coordinators AND
// members (no isCoord() gate) — and the live-session branch stays byte-identical.

const projectPath = "C:\\Project";
const workgroupPath = `${projectPath}\\.ac\\wg-3-dev-team`;
const replicaPath = `${workgroupPath}\\__agent_dev-webpage-ui`;
const replicaSessionName = "wg-3-dev-team/dev-webpage-ui";

const codexAgent: AgentConfig = {
  id: "codex",
  label: "Codex",
  command: "codex",
  color: "#4ade80",
  envs: [],
  isolatedHome: false,
};
const claudeAgent: AgentConfig = {
  id: "claude",
  label: "Claude",
  command: "claude",
  color: "#c084fc",
  envs: [],
  isolatedHome: false,
};

/** Settings whose profiles resolve `codex`/`A` to the human label "Fast" so the
 *  persisted tooltip (A-FAST) is meaningful, and which know the codex + claude
 *  agents so the agent-id → label lookup resolves. */
function settingsWithAgents() {
  return baseSettings({
    agents: [codexAgent, claudeAgent],
    codingAgentProfiles: {
      schemaVersion: 2,
      profileSlots: { A: { label: "" }, B: { label: "" } },
      defaultProfileByAgent: {},
      profilesByAgent: {},
      profileLabelsByAgent: { codex: { A: "Fast" } },
    },
  });
}

/** Discovery with a single replica (member or coordinator) carrying `fields`. */
function replicaDiscovery(fields: Partial<AcAgentReplica>) {
  return discovery({
    workgroups: [
      {
        name: "wg-3-dev-team",
        path: workgroupPath,
        task: null,
        taskTitle: "Offline badges",
        agents: [
          {
            name: "dev-webpage-ui",
            path: replicaPath,
            repoPaths: [],
            isCoordinator: false,
            ...fields,
          },
        ],
      },
    ],
  });
}

const agentBadges = (root: HTMLElement): Element[] =>
  Array.from(root.querySelectorAll(".ac-discovery-badge.agent"));
const profileBadges = (root: HTMLElement): Element[] =>
  Array.from(root.querySelectorAll(".profile-badge"));
const closedPills = (root: HTMLElement): Element[] =>
  Array.from(root.querySelectorAll(".coord-autoclosed"));

async function mount(discoveryResult: ReturnType<typeof discovery>, settings = settingsWithAgents()) {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("get_settings", settings);
  fake.resolve("discover_project", discoveryResult);
  const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
  await settingsStore.load();
  await projectStore.createAndLoad(projectPath);
  await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));
  return rendered;
}

describe("ProjectPanel offline Coding Agent + Profile badges (#733)", () => {
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

  it("shows both the Coding Agent and Profile badges for a shut-down NON-coordinator replica from persisted config", async () => {
    // Member replica (isCoordinator: false), no live session, but persisted
    // currentCodingAgentId + currentProfile — proves the fallback fires for ALL
    // replicas, not just coordinators (no isCoord() gate).
    const rendered = await mount(
      replicaDiscovery({ currentCodingAgentId: "codex", currentProfile: "A" })
    );
    try {
      await waitFor(() => {
        const agents = agentBadges(rendered.root);
        const profiles = profileBadges(rendered.root);
        expect(agents.length).toBeGreaterThan(0);
        expect(agents.every((b) => b.textContent === "Codex")).toBe(true);
        expect(profiles.length).toBeGreaterThan(0);
        expect(profiles.every((b) => b.textContent === "A")).toBe(true);
      });
      // The tooltip resolves from the persisted agent id + profile letter.
      expect(profileBadges(rendered.root)[0]?.getAttribute("title")).toBe("A-FAST");
    } finally {
      rendered.cleanup();
    }
  });

  it("shows NO Coding Agent or Profile badge for a shut-down replica with no persisted config (never invents)", async () => {
    // Gray replica that was never assigned an agent/profile: no
    // currentCodingAgentId, no currentProfile, no preferredAgentId hint.
    const rendered = await mount(replicaDiscovery({}));
    try {
      // The row itself renders (name is visible) so the absence of badges is a
      // real "no badge", not a missing row.
      expect(rendered.root.textContent).toContain("dev-webpage-ui");
      expect(agentBadges(rendered.root)).toHaveLength(0);
      expect(profileBadges(rendered.root)).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("shows the AUTO-CLOSED pill together with the persisted Coding Agent + Profile badges", async () => {
    // Auto-closed coordinators are DESTROYED (no live session), so before #733
    // their agent/profile badges vanished with the session. The pill and the
    // persisted badges must now coexist.
    const rendered = await mount(
      replicaDiscovery({
        isCoordinator: true,
        autoClosedAt: "2026-06-30T00:00:00.000Z",
        currentCodingAgentId: "codex",
        currentProfile: "A",
      })
    );
    try {
      await waitFor(() => {
        const pills = closedPills(rendered.root);
        expect(pills.length).toBeGreaterThan(0);
        expect(pills.every((p) => p.textContent === "AUTO-CLOSED")).toBe(true);
        const agents = agentBadges(rendered.root);
        const profiles = profileBadges(rendered.root);
        expect(agents.length).toBeGreaterThan(0);
        expect(agents.every((b) => b.textContent === "Codex")).toBe(true);
        expect(profiles.length).toBeGreaterThan(0);
        expect(profiles.every((b) => b.textContent === "A")).toBe(true);
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("shows the MANUALLY-CLOSED pill together with the persisted Coding Agent + Profile badges", async () => {
    const rendered = await mount(
      replicaDiscovery({
        isCoordinator: true,
        manuallyClosedAt: "2026-06-30T00:00:00.000Z",
        currentCodingAgentId: "codex",
        currentProfile: "A",
      })
    );
    try {
      await waitFor(() => {
        const pills = closedPills(rendered.root);
        expect(pills.length).toBeGreaterThan(0);
        expect(pills.every((p) => p.textContent === "MANUALLY-CLOSED")).toBe(true);
        const agents = agentBadges(rendered.root);
        const profiles = profileBadges(rendered.root);
        expect(agents.length).toBeGreaterThan(0);
        expect(agents.every((b) => b.textContent === "Codex")).toBe(true);
        expect(profiles.length).toBeGreaterThan(0);
        expect(profiles.every((b) => b.textContent === "A")).toBe(true);
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps the live session as the source of truth — persisted replica config does not override a live session's badges", async () => {
    // Live coordinator running claude/B, while the persisted replica config says
    // codex/A. The live-session branch must win (byte-identical to today): badges
    // read claude/B and the persisted codex/A must NOT leak through.
    sessionsStore.setSessions([
      session({
        id: "live-coord",
        name: replicaSessionName,
        workingDirectory: replicaPath,
        isCoordinator: true,
        status: "running",
        agentId: "claude",
        agentLabel: "Claude",
        requestedProfile: "B",
        effectiveProfile: "B",
      }),
    ]);

    const rendered = await mount(
      replicaDiscovery({
        isCoordinator: true,
        currentCodingAgentId: "codex",
        currentProfile: "A",
      })
    );
    try {
      await waitFor(() => {
        const agents = agentBadges(rendered.root);
        expect(agents.length).toBeGreaterThan(0);
        expect(agents.every((b) => b.textContent === "Claude")).toBe(true);
      });
      const profiles = profileBadges(rendered.root);
      expect(profiles.length).toBeGreaterThan(0);
      expect(profiles.every((b) => b.textContent === "B")).toBe(true);
      // The persisted codex/A fallback must not surface while the session is live.
      expect(agentBadges(rendered.root).some((b) => b.textContent === "Codex")).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });
});
