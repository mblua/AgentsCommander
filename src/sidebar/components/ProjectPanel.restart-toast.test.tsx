// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectPanel, { RESTART_TIMEOUT_MS } from "./ProjectPanel";
import ToastHost from "../../shared/components/ToastHost";
import type { AgentConfig, AppSettings } from "../../shared/types";
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

function discoveryResult() {
  return discovery({
    teams: [{ name: "dev-team", agents: [replicaName], coordinator: replicaName }],
    workgroups: [
      {
        name: workgroupName,
        path: workgroupPath,
        task: null,
        taskTitle: "Restart toast",
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
  fake.resolve("get_settings", baseSettings({ agents: [codexAgent()] }) satisfies AppSettings);
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

/** Load the project, right-click the live replica row, and wait for its active
 *  context menu's "Restart Session" button. Leaves the menu open. */
async function driveToReplicaContextMenu(root: HTMLElement): Promise<void> {
  seedLiveSession();
  await projectStore.createAndLoad(projectPath);

  await waitFor(() => expect(root.querySelector(rowSelector)).toBeTruthy());
  const row = root.querySelector(rowSelector);
  if (!(row instanceof HTMLElement)) throw new Error("Replica row not found");

  contextMenu(row);
  await waitFor(() => expect(findButtonByText("Restart Session")).toBeTruthy());
}

describe("ProjectPanel restart-failure toast (#574)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    // Defensive: the wedge test installs fake timers. Restore real timers here
    // too (no-op otherwise) so a throw inside that window can't leak frozen
    // timers into the next test.
    vi.useRealTimers();
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("surfaces a friendly error toast when the context-menu restart rejects", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    // The Resource Monitor cap is the most likely relaunch failure; the toast
    // must show the launchErrorMessage-normalized copy, proving the catch funnels
    // through the same normalization #573 uses.
    fake.reject("restart_session", "Resource Monitor cap reached: 16/16 agent groups are active");

    const rendered = renderWithFakeTransport(
      () => (
        <>
          <ProjectPanel />
          <ToastHost />
        </>
      ),
      fake,
    );
    try {
      await driveToReplicaContextMenu(rendered.root);

      click(findButtonByText("Restart Session"));

      await waitFor(() => expect(q("toast.item")).toBeTruthy());
      expect(q("toast.item")?.getAttribute("data-ac-kind")).toBe("error");
      expect(q("toast.item")?.textContent ?? "").toContain("Resource Monitor cap reached");
      expect(fake.callsFor("restart_session")).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });

  it("shows no toast when the restart succeeds", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    fake.resolve(
      "restart_session",
      session({ id: sessionId, name: sessionName, workingDirectory: replicaPath }),
    );

    const rendered = renderWithFakeTransport(
      () => (
        <>
          <ProjectPanel />
          <ToastHost />
        </>
      ),
      fake,
    );
    try {
      await driveToReplicaContextMenu(rendered.root);

      click(findButtonByText("Restart Session"));

      await waitFor(() => expect(fake.callsFor("restart_session")).toHaveLength(1));
      // The success path has no toast code path, so none must appear.
      expect(q("toast.item")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  // §15.1 REQUIRED regression guard: a wedged desktop backend (restart_session
  // never settles) must still toast after RESTART_TIMEOUT_MS. Without the
  // Promise.race timeout the Tauri IPC await would hang forever, the catch would
  // never run, and no toast would fire - the exact #574 silent-failure class.
  it("toasts a timeout error when a desktop restart never settles (§15.1)", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    // Never settles - emulates a wedged backend / dropped IPC reply on desktop.
    fake.onInvoke("restart_session", () => new Promise(() => {}));

    const rendered = renderWithFakeTransport(
      () => (
        <>
          <ProjectPanel />
          <ToastHost />
        </>
      ),
      fake,
    );
    try {
      await driveToReplicaContextMenu(rendered.root);

      // Fake timers must own the production window.setTimeout BEFORE the restart
      // fires, so advanceTimers controls the RESTART_TIMEOUT_MS bound. Setup above
      // ran on real timers (waitFor polls real time); the synchronous click +
      // advance below need no waitFor, so the switch is safe from here on.
      vi.useFakeTimers();
      try {
        click(findButtonByText("Restart Session"));

        // The invoke fired and the timeout is armed; no toast yet.
        expect(fake.callsFor("restart_session")).toHaveLength(1);
        expect(q("toast.item")).toBeNull();

        // Advance past the bound: the timeout wins the race and rejects.
        await vi.advanceTimersByTimeAsync(RESTART_TIMEOUT_MS);

        expect(q("toast.item")).toBeTruthy();
        expect(q("toast.item")?.getAttribute("data-ac-kind")).toBe("error");
        // launchErrorMessage passes the bare timeout string through verbatim.
        expect(q("toast.item")?.textContent ?? "").toContain("Command timeout: restart_session");
      } finally {
        vi.useRealTimers();
      }
    } finally {
      rendered.cleanup();
    }
  });
});
