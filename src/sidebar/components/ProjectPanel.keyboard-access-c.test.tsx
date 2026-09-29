// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SidebarApp from "../App";
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
import { initialSelection } from "../../shared/testing/session-selection";
import { ROW_KEY_PROXY_CLASS } from "./RowKeyProxy";

vi.mock("../attention", () => ({ requestTaskbarAttention: vi.fn() }));

// #2659 - the replica row runs its click action (switch to its live session)
// by keyboard through its row key proxy; its nested buttons keep their own keys.
const projectPath = "C:\\Project";
const wgName = "room-kb";
const replicaPath = `${projectPath}\\.ac\\${wgName}\\__agent_worker`;
const sessionId = "kb-session";

const press = (el: Element, key: string) =>
  el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));

describe("ProjectPanel Phase C replica row keyboard access", () => {
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

  async function mount(): Promise<{ fake: FakeTransport; row: () => HTMLElement }> {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings({ projectPaths: [projectPath], projectPath }));
    fake.resolve("get_update_status", null);
    fake.resolve("open_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", discovery({
      workgroups: [{
        name: wgName,
        path: `${projectPath}\\.ac\\${wgName}`,
        task: null,
        taskTitle: null,
        agents: [{ name: "worker", path: replicaPath, repoPaths: [], isCoordinator: false }],
      }],
    }));
    fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
    fake.resolve("search_repos", []);
    fake.resolve("list_sessions", [session({
      id: sessionId,
      name: `${wgName}/worker`,
      workingDirectory: replicaPath,
      status: "running",
      // An adopted session renders the orphan notice, whose dismiss is a nested button.
      agentId: "codex",
      agentLabel: "Codex",
      requestedProfile: "B",
      effectiveProfile: "B",
      matchTier: "labelAndLetter",
    })]);
    fake.resolve("get_active_session", initialSelection());
    fake.resolve("list_detached_sessions", []);
    fake.resolve("telegram_list_bridges", []);
    fake.resolve("resolve_blocking_menu", undefined);
    fake.resolve("switch_session", undefined);
    rendered = renderWithFakeTransport(() => <SidebarApp />, fake);
    const row = () => document.body.querySelector<HTMLElement>(".replica-item")!;
    await waitFor(() => expect(row()).not.toBeNull());
    return { fake, row };
  }

  it("switches to the live session once with Enter, Space and click", async () => {
    const { fake, row } = await mount();
    const proxy = row().querySelector<HTMLElement>(`:scope > .${ROW_KEY_PROXY_CLASS}`)!;
    expect(row().firstElementChild).toBe(proxy);
    expect(proxy.getAttribute("aria-label")).toBe("worker");
    for (const act of [() => press(proxy, "Enter"), () => press(proxy, " "), () => click(row())]) {
      fake.clearCalls();
      act();
      await waitFor(() => expect(fake.callsFor("switch_session")).toHaveLength(1));
    }
  });

  it("ignores keys from nested buttons and keeps right-click", async () => {
    const { fake, row } = await mount();
    fake.clearCalls();
    const nested = Array.from(row().querySelectorAll<HTMLElement>("button"))
      .filter((b) => !b.classList.contains(ROW_KEY_PROXY_CLASS));
    expect(nested.length).toBeGreaterThan(0);
    for (const btn of nested) {
      press(btn, "Enter");
      press(btn, " ");
    }
    await Promise.resolve();
    expect(fake.callsFor("switch_session")).toHaveLength(0);
    contextMenu(row());
    await waitFor(() => expect(document.body.querySelector(".session-context-option")).not.toBeNull());
    expect(fake.callsFor("switch_session")).toHaveLength(0);
  });
});
