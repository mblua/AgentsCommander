// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
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

// #2046 — an offline Agent Matrix row in the sidebar Agents section is NOT a
// launchable unit: a left click must explain the row (matrix vs replica), never
// run handleAgentClick's old launch flow (setPendingLaunch -> AgentPickerModal ->
// SessionAPI.create/switch). Only a live matrix session row (SessionItem) keeps
// switching. These four tests drive the real ProjectPanel -> Portal mount path
// through the FakeTransport and pin the notice, its copy, its three dismissal
// paths and the untouched live-session boundary.

const projectPath = "C:\\Project";
const originAgentPath = `${projectPath}\\.ac\\_agent_dev-docs`;
const agentName = "dev-docs";

/** Discovery with a single offline (no session) origin Agent Matrix row. */
function matrixDiscovery() {
  return discovery({
    agents: [
      {
        name: agentName,
        path: originAgentPath,
        roleExists: true,
      },
    ],
  });
}

function q(testId: string): HTMLElement | null {
  return document.body.querySelector<HTMLElement>(`[data-ac-testid="${testId}"]`);
}

/** The offline matrix row: the `.replica-item` whose text carries the agent name. */
function agentRow(root: ParentNode): HTMLElement {
  const row = Array.from(root.querySelectorAll<HTMLElement>(".replica-item")).find((candidate) =>
    candidate.textContent?.includes(agentName),
  );
  if (!row) throw new Error(`Agent Matrix row not found: ${agentName}`);
  return row;
}

async function mount() {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("get_settings", baseSettings());
  fake.resolve("discover_project", matrixDiscovery());
  fake.resolve("switch_session", null);
  const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
  await settingsStore.load();
  await projectStore.createAndLoad(projectPath);
  await waitFor(() => expect(rendered.root.textContent).toContain(agentName));
  return { fake, rendered };
}

describe("ProjectPanel Agent Matrix row notice (#2046)", () => {
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

  it("left click on an Agent Matrix row shows the notice and never mounts the Coding Agent picker", async () => {
    const { fake, rendered } = await mount();
    try {
      click(agentRow(rendered.root));
      await Promise.resolve();
      // Red on base: the picker is mounted here.
      expect(q("agentPicker.modal")).toBeNull();
      await waitFor(() => expect(q("agentMatrixNotice.modal")).not.toBeNull());
      expect(fake.callsFor("create_session")).toHaveLength(0);
      expect(fake.callsFor("switch_session")).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("states what a matrix is, what a replica is, and where the matrix folder lives", async () => {
    const { fake, rendered } = await mount();
    try {
      click(agentRow(rendered.root));
      await waitFor(() => expect(q("agentMatrixNotice.modal")).not.toBeNull());
      const text = q("agentMatrixNotice.modal")!.textContent ?? "";
      expect(text).toContain("Agent Matrix");
      expect(text).toContain(agentName);
      expect(text).toContain("replica");
      expect(text).toContain("assigned to a room");
      expect(text).toContain("Open Matrix folder");
      expect(text).toContain(originAgentPath);
      expect(q("agentPicker.modal")).toBeNull();
      expect(fake.callsFor("create_session")).toHaveLength(0);
      expect(fake.callsFor("switch_session")).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("closes on Got it, the overlay and Escape, and reopens on the next click", async () => {
    const { fake, rendered } = await mount();
    try {
      click(agentRow(rendered.root));
      await waitFor(() => expect(q("agentMatrixNotice.modal")).not.toBeNull());
      // The centering surface every project modal shares.
      expect(q("agentMatrixNotice.overlay")!.classList.contains("modal-overlay")).toBe(true);

      click(q("agentMatrixNotice.close")!);
      await waitFor(() => expect(q("agentMatrixNotice.modal")).toBeNull());

      click(agentRow(rendered.root));
      await waitFor(() => expect(q("agentMatrixNotice.modal")).not.toBeNull());
      click(q("agentMatrixNotice.overlay")!);
      await waitFor(() => expect(q("agentMatrixNotice.modal")).toBeNull());

      click(agentRow(rendered.root));
      await waitFor(() => expect(q("agentMatrixNotice.modal")).not.toBeNull());
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await waitFor(() => expect(q("agentMatrixNotice.modal")).toBeNull());

      expect(q("agentPicker.modal")).toBeNull();
      expect(fake.callsFor("create_session")).toHaveLength(0);
      expect(fake.callsFor("switch_session")).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps the live matrix session row switching to the session", async () => {
    sessionsStore.setSessions([
      session({
        id: "matrix-session",
        name: agentName,
        workingDirectory: originAgentPath,
        status: "running",
      }),
    ]);
    const { fake, rendered } = await mount();
    try {
      const row = q("session.matrix-session");
      expect(row).not.toBeNull();
      click(row!);
      await waitFor(() => expect(fake.callsFor("switch_session")).toHaveLength(1));
      expect(fake.lastCall("switch_session")!.args).toEqual({ id: "matrix-session" });
      expect(q("agentMatrixNotice.modal")).toBeNull();
      expect(q("agentPicker.modal")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });
});
