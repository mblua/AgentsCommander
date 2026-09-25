// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SidebarApp from "../App";
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
import { sessionsStore } from "../stores/sessions";
import type { AgentConfig, Session } from "../../shared/types";

// #2482 p7 - the weekly-quota fill on the ROOM-REPLICA agent chip. Every case
// resolves the chip through its row and asserts the NodeList length first: a
// missing chip must fail, never pass a "no quota-fill" check.
const projectPath = "C:\\Project";
const wgName = "room-quota";
const workgroupPath = `${projectPath}\\.ac\\${wgName}`;
const coordName = "orchestrator";
const workerName = "worker";
const idleName = "idle-one";
const coordSessionId = "coord-session";
const workerSessionId = "worker-session";

const replicaChip = (root: Element, ctx: "quick" | "workgroups", wg: string, replica: string) =>
  root.querySelectorAll<HTMLElement>(`[data-ac-testid="replica.row.${ctx}.${wg}.${replica}"] .ac-discovery-badge.agent`);

function oneChip(ctx: "quick" | "workgroups", replica: string): HTMLElement {
  const all = replicaChip(document.body, ctx, wgName, replica);
  expect(all).toHaveLength(1);
  return all[0];
}

function replicaPath(name: string): string {
  return `${workgroupPath}\\__agent_${name}`;
}

function agentConfig(overrides: Partial<AgentConfig> = {}): AgentConfig {
  return {
    id: "claude",
    label: "Claude Code",
    command: "claude",
    color: "#d97757",
    envs: [],
    isolatedHome: false,
    ...overrides,
  };
}

function replicaSessionFor(id: string, name: string, isCoordinator: boolean): Session {
  return session({
    id,
    name: `${wgName}/${name}`,
    workingDirectory: replicaPath(name),
    status: "running",
    isCoordinator,
    agentId: "claude",
    agentLabel: "Claude Code",
  });
}

function setupTransport(fake: FakeTransport, idleAgentId: string): void {
  const agents = [agentConfig(), agentConfig({ id: "codex", label: "Codex", command: "codex" })];
  fake.resolve("get_settings", baseSettings({ projectPaths: [projectPath], projectPath, agents }));
  fake.resolve("get_update_status", null);
  fake.resolve("open_project", { path: projectPath, registered: true, created: false });
  fake.resolve(
    "discover_project",
    discovery({
      workgroups: [
        {
          name: wgName,
          path: workgroupPath,
          task: null,
          taskTitle: "Quota chip",
          agents: [
            { name: coordName, path: replicaPath(coordName), repoPaths: [], isCoordinator: true },
            { name: workerName, path: replicaPath(workerName), repoPaths: [], isCoordinator: false },
            // No session and no currentCodingAgentId: the label resolves from
            // preferredAgentId + settings, so the chip renders WITHOUT a session.
            {
              name: idleName,
              path: replicaPath(idleName),
              repoPaths: [],
              isCoordinator: false,
              preferredAgentId: idleAgentId,
            },
          ],
        },
      ],
    })
  );
  fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
  fake.resolve("search_repos", []);
  fake.resolve("list_sessions", [
    replicaSessionFor(coordSessionId, coordName, true),
    replicaSessionFor(workerSessionId, workerName, false),
  ]);
  fake.resolve("get_active_session", null);
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
  fake.resolve("get_session_context", null);
  fake.resolve("get_agent_quota_readings", {});
}

describe("ProjectPanel replica weekly-quota chip (#2482)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    sessionsStore.resetQuotaReadingsForTests();
  });

  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    sessionsStore.resetQuotaReadingsForTests();
    document.body.replaceChildren();
  });

  async function mount(idleAgentId = "claude"): Promise<FakeTransport> {
    const fake = new FakeTransport();
    setupTransport(fake, idleAgentId);
    rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    await waitFor(() => {
      expect(replicaChip(document.body, "quick", wgName, coordName)).toHaveLength(1);
      expect(replicaChip(document.body, "workgroups", wgName, workerName)).toHaveLength(1);
      expect(replicaChip(document.body, "workgroups", wgName, idleName)).toHaveLength(1);
    });
    await waitFor(() => expect(fake.callsFor("get_agent_quota_readings").length).toBeGreaterThan(0));
    return fake;
  }

  function reading(fake: FakeTransport, agentId: string, weeklyUsedPercent: number | null): void {
    fake.emitFromBackend("agent_quota", { agentId, weeklyUsedPercent });
  }

  // A real-timer flush, so an absence assertion is not satisfied before the
  // snapshot has had a chance to land.
  async function settle(): Promise<void> {
    await new Promise((resolve) => setTimeout(resolve, 20));
  }

  it("a_replica_chip_fills_from_a_reading_on_its_session", async () => {
    const fake = await mount();
    reading(fake, "claude", 28);
    await waitFor(() => {
      const el = oneChip("workgroups", workerName);
      expect(el.className).toContain("quota-fill");
      expect(el.style.getPropertyValue("--ac-quota-remaining")).toBe("72%");
    });
  });

  it("a_replica_with_no_reading_renders_the_plain_chip", async () => {
    await mount();
    await settle();
    const el = oneChip("workgroups", workerName);
    expect(el.className).not.toContain("quota-fill");
    expect(el.getAttribute("style")).toBeNull();
  });

  it("a_replica_with_no_live_session_is_filled_from_its_configured_agent", async () => {
    const fake = await mount();
    reading(fake, "claude", 28);
    await waitFor(() => expect(oneChip("workgroups", idleName).className).toContain("quota-fill"));
    const el = oneChip("workgroups", idleName);
    expect(el.textContent).toBe("Claude Code");
    expect(el.style.getPropertyValue("--ac-quota-remaining")).toBe("72%");
  });

  it("a_reading_on_one_session_fills_every_replica_of_the_same_agent", async () => {
    const fake = await mount();
    reading(fake, "claude", 28);
    await waitFor(() => {
      for (const ctx of ["quick", "workgroups"] as const) {
        const el = oneChip(ctx, coordName);
        expect(el.className).toContain("quota-fill");
        expect(el.style.getPropertyValue("--ac-quota-remaining")).toBe("72%");
      }
    });
  });

  it("a_reading_on_one_agent_does_not_fill_a_replica_of_a_DIFFERENT_agent", async () => {
    const fake = await mount("codex");
    reading(fake, "claude", 28);
    await waitFor(() => expect(oneChip("workgroups", workerName).className).toContain("quota-fill"));
    await settle();
    const el = oneChip("workgroups", idleName);
    expect(el.textContent).toBe("Codex");
    expect(el.className).not.toContain("quota-fill");
    expect(el.getAttribute("style")).toBeNull();
  });

  it("a_replica_whose_agent_has_no_reading_renders_the_plain_chip", async () => {
    const fake = await mount("codex");
    reading(fake, "claude", 28);
    await waitFor(() => expect(oneChip("workgroups", workerName).className).toContain("quota-fill"));
    await settle();
    const el = oneChip("workgroups", idleName);
    expect(el.getAttribute("class")).toBe("ac-discovery-badge agent");
    expect(el.getAttribute("style")).toBeNull();
    expect(el.getAttribute("role")).toBeNull();
    expect(el.getAttribute("aria-valuenow")).toBeNull();
  });

  it("the_same_reading_fills_the_row_in_both_row_contexts", async () => {
    const fake = await mount();
    reading(fake, "claude", 28);
    await waitFor(() => {
      for (const ctx of ["quick", "workgroups"] as const) {
        const el = oneChip(ctx, coordName);
        expect(el.className).toContain("quota-fill");
        expect(el.style.getPropertyValue("--ac-quota-remaining")).toBe("72%");
      }
    });
  });

  it("the_filled_replica_chip_keeps_the_agent_name_in_its_accessible_name", async () => {
    const fake = await mount();
    reading(fake, "claude", 28);
    await waitFor(() => expect(oneChip("workgroups", workerName).getAttribute("role")).toBe("meter"));
    const el = oneChip("workgroups", workerName);
    expect(el.getAttribute("aria-label")).toContain("Claude Code");
    expect(el.textContent).toBe("Claude Code");
  });
});
