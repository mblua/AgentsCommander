// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SidebarHistoryState } from "../context-history";

const history = vi.hoisted(() => ({ acquire: vi.fn(), release: vi.fn() }));
vi.mock("../context-history", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../context-history")>();
  const ready: SidebarHistoryState = {
    status: "ready",
    scope: { kind: "known", agentKey: "dev", teamIds: ["t1"] },
    generatedAtUtcMs: 1,
    sessionWindows: [
      { requestedSessions: 25, recordsCount: 1, positiveSum: 40, mean: 40,
        sessionsCount: 1, recordsPerSession: 1, incomplete: false },
      { requestedSessions: 100, recordsCount: 2, positiveSum: 110, mean: 55,
        sessionsCount: 2, recordsPerSession: 1, incomplete: false },
    ],
  };
  history.acquire.mockImplementation(() => ({ state: () => ready, release: history.release }));
  return { ...actual, acquireContextHistory: history.acquire };
});

import ProjectPanel from "./ProjectPanel";
import SessionItem from "./SessionItem";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
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
import { rampColor } from "./ContextBadge";
import type { AppSettings } from "../../shared/types";

// #3532 P11 - rows hand the live runtime UUID and their project to the badge;
// nothing on the badge may switch or restart the session.

const root = "C:\\Project";
const room = `${root}\\.ac\\wg-2-dev-team`;
const replicaPath = `${room}\\__agent_dev-webpage-ui`;
const ID = "00000000-0000-4000-8000-0000000000aa";

const settings = (): AppSettings =>
  baseSettings({
    agents: [{
      id: "claude", label: "Claude Code", command: "claude", color: "#d97757",
      envs: [], isolatedHome: false, contextRegex: String.raw`(\d+)%`,
    }],
    contextHistoryColorSessions: 25,
  });

const badges = () =>
  Array.from(document.querySelectorAll<HTMLElement>(".ctx-badge.ctx-history"));

describe("ProjectPanel CTX history scope (#3532 P11)", () => {
  let cleanupDom: (() => void) | null = null;
  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    history.acquire.mockClear();
    history.release.mockClear();
  });
  afterEach(() => {
    cleanupDom?.();
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("replica rows pass the project and runtime UUID; badge events never switch", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", settings());
    fake.resolve("new_project", { path: root, registered: true, created: false });
    fake.resolve("discover_project", discovery({
      workgroups: [{
        name: "wg-2-dev-team", path: room, task: null, taskTitle: "Task",
        agents: [
          { name: "dev-webpage-ui", path: replicaPath, repoPaths: [], isCoordinator: false },
        ],
      }],
    }));
    sessionsStore.setSessions([session({
      id: ID, name: "wg-2-dev-team/dev-webpage-ui", workingDirectory: replicaPath,
      status: "running", agentId: "claude",
    })]);
    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await settingsStore.load();
      await projectStore.createAndLoad(root);
      await waitFor(() => expect(badges().length).toBeGreaterThan(0));
      expect(history.acquire).toHaveBeenCalledWith({ projectPath: root, runtimeSessionId: ID });
      const badge = badges()[0];
      expect(badge.classList.contains("toned")).toBe(true);
      // The saved colour count 25 selects the 40% bucket, not the last one (55%).
      expect(badge.style.getPropertyValue("--ctx-history-tone-dark")).toBe(rampColor(40, "dark"));
      fake.clearCalls();
      click(badge);
      badge.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
      badge.dispatchEvent(new KeyboardEvent("keydown", { key: " ", bubbles: true }));
      await new Promise((r) => setTimeout(r, 0));
      expect(fake.callsFor("switch_session")).toEqual([]);
      expect(fake.callsFor("restart_session")).toEqual([]);
      // The row itself keeps working: its click and the badge contextmenu reach it.
      const row = badge.closest<HTMLElement>(".replica-item")!;
      fake.resolve("switch_session", undefined);
      fake.resolve("list_unresolved_loop_targets", []);
      click(row);
      await waitFor(() => expect(fake.callsFor("switch_session").length).toBeGreaterThan(0));
      badge.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
      await waitFor(() => expect(document.querySelector(".session-context-menu")).not.toBeNull());
    } finally {
      rendered.cleanup();
    }
    expect(history.release).toHaveBeenCalled();
  });

  it("SessionItem passes its scope; a Root session never does", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", settings());
    const rendered = renderWithFakeTransport(
      () => (
        <>
          <SessionItem
            session={session({ id: ID, agentId: "claude" })}
            isActive={false}
            historyProjectPath={root}
          />
          <SessionItem
            session={session({ id: "00000000-0000-4000-8000-0000000000bb", agentId: "claude",
              isRootAgent: true })}
            isActive={false}
            historyProjectPath={root}
          />
          <SessionItem session={session({ id: "s3", agentId: "claude" })} isActive={false} />
        </>
      ),
      fake,
    );
    try {
      await settingsStore.load();
      await waitFor(() => expect(badges().length).toBe(1));
      expect(badges()[0].style.getPropertyValue("--ctx-history-tone-dark"))
        .toBe(rampColor(40, "dark"));
      expect(history.acquire).toHaveBeenCalledTimes(1);
      expect(history.acquire).toHaveBeenCalledWith({ projectPath: root, runtimeSessionId: ID });
      const plain = document.querySelector<HTMLElement>(
        '[data-ac-testid="session.s3.contextBadge"]',
      );
      expect(plain?.hasAttribute("title")).toBe(true);
      expect(plain?.hasAttribute("tabindex")).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });
});
