// Test-only SidebarApp harness (#2271 dup gate). The transport arrangement the
// sidebar App suites resolve before mount and the reconcile-timer spy they both
// need live here, so each suite file carries only the data that varies at the
// call site.
import { vi } from "vitest";
import type { FakeTransport } from "../../shared/testing/fake-transport";
import { baseSettings, discovery } from "../../shared/testing/ui-harness";
import { liveSelection, SESSION_A } from "../../shared/testing/session-selection";
import type { Session } from "../../shared/types";

export interface AppTransportAgent {
  name: string;
  path: string;
}

export interface AppTransportSetup {
  projectPath: string;
  agents: AppTransportAgent[];
  rows: () => Session[];
}

export function setupAppTransport(
  fake: FakeTransport,
  { projectPath, agents, rows }: AppTransportSetup,
): void {
  fake.resolve("get_settings", baseSettings({ projectPaths: [projectPath], projectPath }));
  fake.resolve("open_project", { path: projectPath, registered: true, created: false });
  fake.resolve(
    "discover_project",
    discovery({
      agents: agents.map((agent) => ({ name: agent.name, path: agent.path, roleExists: true })),
      teams: [],
      workgroups: [],
    }),
  );
  fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
  fake.resolve("search_repos", []);
  fake.onInvoke("list_sessions", rows);
  fake.resolve("get_active_session", liveSelection(SESSION_A));
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
}

export interface ReconcileIntervalSpy {
  /** Every captured setInterval call at the configured period, in call order. */
  ticks(): unknown[][];
  /** Clears the captured handles. MUST run before restore(). */
  clear(): void;
  restore(): void;
}

export interface ReconcileIntervalSpyOptions {
  periodMs: number;
}

/**
 * Spies on setInterval without replacing its implementation, so the app's real
 * reconcile timers still run and mock.results[i].value stays the real handle,
 * index-aligned with mock.calls[i]. The sweep MUST precede the restore:
 * mockRestore() discards mock.calls and mock.results and the handles become
 * unrecoverable.
 */
export function installReconcileIntervalSpy({
  periodMs,
}: ReconcileIntervalSpyOptions): ReconcileIntervalSpy {
  const spy = vi.spyOn(globalThis, "setInterval");
  return {
    ticks: () => spy.mock.calls.filter((call) => call[1] === periodMs),
    clear: () => {
      spy.mock.calls.forEach((call, i) => {
        if (call[1] !== periodMs) return;
        const handle = spy.mock.results[i]?.value as ReturnType<typeof setInterval> | undefined;
        if (handle !== undefined) clearInterval(handle);
      });
    },
    restore: () => spy.mockRestore(),
  };
}
