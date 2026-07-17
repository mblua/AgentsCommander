// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SidebarApp from "./App";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../shared/testing/ui-harness";
import { sessionsStore } from "./stores/sessions";
import type { AgentConfig, Session } from "../shared/types";

// #1033 - the wiring, end to end, through the REAL App listener and the REAL
// mount-time hydration: backend event / snapshot -> ipc -> store -> projection -> DOM.
//
// This mounts SidebarApp rather than re-declaring the listener inside the test,
// because the listener and its ordering ARE the thing under test. #1032's engine
// emits ONLY on change, so a listener alone leaves a reloaded sidebar reading N/A
// forever against a session that is plainly sitting at 42%.

const projectPath = "C:\\Project";
const wgName = "wg-2-dev-team";
const replicaName = "dev-webpage-ui";
const otherReplicaName = "dev-rust";
const workgroupPath = `${projectPath}\\.ac\\${wgName}`;
const replicaPath = `${workgroupPath}\\__agent_${replicaName}`;
const otherReplicaPath = `${workgroupPath}\\__agent_${otherReplicaName}`;
const sessionId = "coord-session";
const otherSessionId = "member-session";

const CLAUDE_PATTERN = String.raw`^ {2}Context [░█]+ (\d{1,3})%`;

/**
 * The replica row renders once per row context: `quick` is the coordinator
 * quick-access panel (coordinators only), `workgroups` is the workgroup tree
 * (every replica). Both render the same badge from the same shared resolver.
 */
function badgeSelector(replica: string, rowContext: "quick" | "workgroups" = "quick"): string {
  return `[data-ac-testid="replica.contextBadge.${rowContext}.${wgName}.${replica}"]`;
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

function agentSession(overrides: Partial<Session> = {}): Session {
  return session({
    id: sessionId,
    name: `${wgName}/${replicaName}`,
    workingDirectory: replicaPath,
    status: "running",
    isCoordinator: true,
    agentId: "claude",
    agentLabel: "Claude Code",
    ...overrides,
  });
}

function setupTransport(
  fake: FakeTransport,
  opts: { agents: AgentConfig[]; sessions: Session[]; replicas?: string[] },
): void {
  const replicas = opts.replicas ?? [replicaName];
  fake.resolve(
    "get_settings",
    baseSettings({ projectPaths: [projectPath], projectPath, agents: opts.agents }),
  );
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
          taskTitle: "Ctx badge",
          agents: replicas.map((name) => ({
            name,
            path: `${workgroupPath}\\__agent_${name}`,
            repoPaths: [],
            isCoordinator: name === replicaName,
          })),
        },
      ],
    }),
  );
  fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
  fake.resolve("search_repos", []);
  fake.resolve("list_sessions", opts.sessions);
  fake.resolve("get_active_session", null);
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
  // Default: the engine has no reading for anyone.
  fake.resolve("get_session_context", null);
}

async function mounted(fake: FakeTransport) {
  const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
  await waitFor(() => {
    expect(sessionsStore.sessions.find((s) => s.id === sessionId)).toBeTruthy();
    expect(rendered.root.querySelector(`[data-ac-testid="replica.row.quick.${wgName}.${replicaName}"]`)).not.toBeNull();
  });
  return rendered;
}

describe("SidebarApp CTX badge workflow (#1033)", () => {
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

  // The criterion the issue does not have and the feature is broken without.
  // NO event is emitted here: the session is ALREADY at 42%, which is exactly when
  // the emit-on-change engine stays silent forever. Only the snapshot can paint it.
  it("shows a session already sitting at a value after a reload (a_reloaded_sidebar_hydrates_a_session_already_at_a_value)", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, {
      agents: [agentConfig({ contextRegex: CLAUDE_PATTERN })],
      sessions: [agentSession()],
    });
    fake.onInvoke("get_session_context", (args) => (args.sessionId === sessionId ? 42 : null));

    const rendered = await mounted(fake);
    try {
      await waitFor(() => {
        expect(rendered.root.querySelector(badgeSelector(replicaName))?.textContent).toBe("CTX 42%");
      });
      // ...and it really was the snapshot: no session_context event was ever sent.
      expect(fake.callsFor("get_session_context").length).toBeGreaterThan(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("paints the badge from a backend event, and accepts a decrease (an_event_paints_the_badge)", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, {
      agents: [agentConfig({ contextRegex: CLAUDE_PATTERN })],
      sessions: [agentSession()],
    });

    const rendered = await mounted(fake);
    try {
      await waitFor(() => {
        expect(rendered.root.querySelector(badgeSelector(replicaName))?.textContent).toBe("CTX N/A");
      });

      fake.emitFromBackend("session_context", { sessionId, percent: 42 });
      await waitFor(() => {
        expect(rendered.root.querySelector(badgeSelector(replicaName))?.textContent).toBe("CTX 42%");
      });

      // Decreases are accepted as-is: no monotonicity, no smoothing, and nothing
      // infers "Compacting" from a fall.
      fake.emitFromBackend("session_context", { sessionId, percent: 12 });
      await waitFor(() => {
        expect(rendered.root.querySelector(badgeSelector(replicaName))?.textContent).toBe("CTX 12%");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("paints a real zero as a reading, never as unknown (an_event_with_percent_zero_paints_zero)", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, {
      agents: [agentConfig({ contextRegex: CLAUDE_PATTERN })],
      sessions: [agentSession()],
    });

    const rendered = await mounted(fake);
    try {
      fake.emitFromBackend("session_context", { sessionId, percent: 0 });

      await waitFor(() => {
        const badge = rendered.root.querySelector(badgeSelector(replicaName));
        expect(badge?.textContent).toBe("CTX 0%");
        expect(badge?.getAttribute("role")).toBe("meter");
        expect(badge?.getAttribute("aria-valuenow")).toBe("0");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("paints an explicit null as N/A and never as 0% (an_event_with_percent_null_paints_N_A)", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, {
      agents: [agentConfig({ contextRegex: CLAUDE_PATTERN })],
      sessions: [agentSession()],
    });

    const rendered = await mounted(fake);
    try {
      fake.emitFromBackend("session_context", { sessionId, percent: 42 });
      await waitFor(() => {
        expect(rendered.root.querySelector(badgeSelector(replicaName))?.textContent).toBe("CTX 42%");
      });

      // The engine lost the reading (cleared pattern, suppressed statusline, ...).
      fake.emitFromBackend("session_context", { sessionId, percent: null });

      await waitFor(() => {
        const badge = rendered.root.querySelector(badgeSelector(replicaName));
        expect(badge?.textContent).toBe("CTX N/A");
        expect(badge?.hasAttribute("role")).toBe(false);
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("renders no badge at all for an agent with no pattern (no_regex_configured_renders_no_badge)", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, { agents: [agentConfig()], sessions: [agentSession()] });

    const rendered = await mounted(fake);
    try {
      // Not N/A, not an empty chip: absent entirely.
      expect(rendered.root.querySelector(badgeSelector(replicaName))).toBeNull();
      expect(rendered.root.querySelector(".ctx-badge")).toBeNull();
      expect(rendered.root.textContent).not.toContain("CTX");
    } finally {
      rendered.cleanup();
    }
  });

  it("makes the badge appear as soon as a pattern is saved, with no reload (the_badge_appears_when_a_regex_is_saved)", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, { agents: [agentConfig()], sessions: [agentSession()] });

    const rendered = await mounted(fake);
    try {
      expect(rendered.root.querySelector(badgeSelector(replicaName))).toBeNull();

      // What SettingsModal's save does: re-resolve settings into the signal.
      fake.resolve(
        "get_settings",
        baseSettings({
          projectPaths: [projectPath],
          projectPath,
          agents: [agentConfig({ contextRegex: CLAUDE_PATTERN })],
        }),
      );
      const { settingsStore } = await import("../shared/stores/settings");
      await settingsStore.load();

      await waitFor(() => {
        expect(rendered.root.querySelector(badgeSelector(replicaName))?.textContent).toBe("CTX N/A");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps two concurrent sessions independent (two_concurrent_sessions_show_independent_values)", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, {
      agents: [agentConfig({ contextRegex: CLAUDE_PATTERN })],
      replicas: [replicaName, otherReplicaName],
      sessions: [
        agentSession(),
        agentSession({
          id: otherSessionId,
          name: `${wgName}/${otherReplicaName}`,
          workingDirectory: otherReplicaPath,
          isCoordinator: false,
        }),
      ],
    });

    const rendered = await mounted(fake);
    try {
      fake.emitFromBackend("session_context", { sessionId, percent: 42 });
      fake.emitFromBackend("session_context", { sessionId: otherSessionId, percent: 7 });

      // Read both from the workgroup tree: the quick panel holds coordinators only.
      await waitFor(() => {
        expect(
          rendered.root.querySelector(badgeSelector(replicaName, "workgroups"))?.textContent,
        ).toBe("CTX 42%");
        expect(
          rendered.root.querySelector(badgeSelector(otherReplicaName, "workgroups"))?.textContent,
        ).toBe("CTX 7%");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("never asks the engine about a plain shell (a_plain_shell_is_not_hydrated)", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, {
      agents: [agentConfig({ contextRegex: CLAUDE_PATTERN })],
      sessions: [agentSession(), session({ id: "plain-shell", name: "pwsh", agentId: null })],
    });

    const rendered = await mounted(fake);
    try {
      await waitFor(() => {
        expect(fake.callsFor("get_session_context").length).toBeGreaterThan(0);
      });
      const asked = fake.callsFor("get_session_context").map((c) => c.args.sessionId);
      expect(asked).toContain(sessionId);
      // Plain shells are never registered by the backend, so the fan-out skips them.
      expect(asked).not.toContain("plain-shell");
    } finally {
      rendered.cleanup();
    }
  });

  // A rejected snapshot must cost its own badge and nothing else: the session still
  // shows N/A and self-corrects on its next change, and onMount is not aborted.
  it("survives a rejected snapshot (a_failed_snapshot_is_harmless)", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, {
      agents: [agentConfig({ contextRegex: CLAUDE_PATTERN })],
      sessions: [agentSession()],
    });
    fake.reject("get_session_context", "scraper is gone");

    const rendered = await mounted(fake);
    try {
      await waitFor(() => {
        expect(rendered.root.querySelector(badgeSelector(replicaName))?.textContent).toBe("CTX N/A");
      });

      fake.emitFromBackend("session_context", { sessionId, percent: 42 });
      await waitFor(() => {
        expect(rendered.root.querySelector(badgeSelector(replicaName))?.textContent).toBe("CTX 42%");
      });
    } finally {
      rendered.cleanup();
    }
  });
});
