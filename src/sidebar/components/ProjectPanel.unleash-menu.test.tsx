// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
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
import type { Session } from "../../shared/types";

// The Unleash/Leash item in the replica menu, its dialog and restart.

const projectPath = "C:\\Project";
const wgPath = `${projectPath}\\.ac\\wg-1-dev-team`;
const memberPath = `${wgPath}\\__agent_dev-rust`;
const memberRowTestId = "replica.row.workgroups.C-Project-.ac-wg-1-dev-team-__agent_dev-rust";
const itemTestId = "replica.member-session.menu.unleash";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const byTestId = (id: string) => document.querySelector<HTMLElement>(`[data-ac-testid="${id}"]`);
const menu = () => document.querySelector<HTMLElement>(".session-context-menu");

describe("ProjectPanel Unleash menu item", () => {
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

  async function setupPanel(opts: { member?: Partial<Session>; unleashed?: boolean } = {}) {
    const fake = new FakeTransport();
    const result = discovery({
      workgroups: [
        {
          name: "wg-1-dev-team",
          path: wgPath,
          task: null,
          taskTitle: "Clean",
          agents: [
            {
              name: "dev-rust",
              path: memberPath,
              identityPath: "../../_agent_dev-rust",
              repoPaths: [],
              isCoordinator: false,
              ...(opts.unleashed === undefined ? {} : { unleashed: opts.unleashed }),
            },
          ],
        },
      ],
    });
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", { ...result, teams: [] });
    fake.resolve("get_settings", baseSettings());
    fake.resolve("set_replica_unleashed", undefined);
    fake.onInvoke("restart_session", ({ id }) => session({ id: String(id) }));
    fake.onInvoke("task_get_snapshot_at", ({ workgroupRoot }) => ({
      workgroupRoot: String(workgroupRoot), task: null, taskTitle: "Clean", description: "",
      status: null, revision: "topic:0", statusRecord: null, tailIncomplete: false,
    }));
    sessionsStore.setSessions([
      session({
        id: "member-session",
        name: "wg-1-dev-team/dev-rust",
        workingDirectory: memberPath,
        status: "running",
        ...opts.member,
      }),
    ]);
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    await projectStore.createAndLoad(projectPath);
    await waitFor(() => expect(byTestId(memberRowTestId)).not.toBeNull());
    return fake;
  }

  async function openMenu() {
    contextMenu(byTestId(memberRowTestId)!);
    await waitFor(() => expect(menu()).not.toBeNull());
    await flush();
  }

  const item = () => byTestId(itemTestId) as HTMLButtonElement | null;
  const unleashCalls = (fake: FakeTransport) =>
    fake.callsFor("set_replica_unleashed").map((c) => c.args);
  const restartCalls = (fake: FakeTransport) => fake.callsFor("restart_session").map((c) => c.args);
  const orderOf = (fake: FakeTransport) =>
    fake.calls
      .map((c) => c.cmd)
      .filter((cmd) => cmd === "set_replica_unleashed" || cmd === "restart_session");

  it("puts Unleash directly before Close Session", async () => {
    await setupPanel();
    await openMenu();
    const el = item()!;
    expect(el.textContent).toContain("Unleash");
    expect(el.getAttribute("data-ac-state")).toBe("leashed");
    expect(el.nextElementSibling?.textContent).toContain("Close Session (Ctrl+Shift+W)");
    const buttons = Array.from(menu()!.querySelectorAll("button"));
    const matrix = buttons.find((b) => b.textContent?.trim().endsWith("Open Matrix folder"));
    const close = buttons.find((b) => b.textContent?.includes("Close Session (Ctrl+Shift+W)"));
    expect(matrix?.nextElementSibling).toBe(el);
    expect(el.nextElementSibling).toBe(close);
  });

  it("disables the item for a leashed container session, with a tooltip", async () => {
    const fake = await setupPanel({ member: { backendKind: "containerTransport" } });
    await openMenu();
    const el = item()!;
    expect(el.disabled).toBe(true);
    expect(el.classList.contains("context-option-disabled")).toBe(true);
    expect(el.title).toContain("container mounts still limit what the agent can see");
    click(el);
    await flush();
    expect(byTestId("unleashConfirm.modal")).toBeNull();
    expect(unleashCalls(fake)).toEqual([]);
  });

  it("shows an enabled Leash for a container session that is already unleashed", async () => {
    await setupPanel({ member: { backendKind: "containerTransport" }, unleashed: true });
    await openMenu();
    const el = item()!;
    expect(el.disabled).toBe(false);
    expect(el.textContent).toContain("Leash");
    expect(el.textContent).not.toContain("Unleash");
    expect(el.getAttribute("data-ac-state")).toBe("unleashed");
  });

  it("Cancel closes the dialog without any IPC call", async () => {
    const fake = await setupPanel();
    await openMenu();
    click(item()!);
    await waitFor(() => expect(byTestId("unleashConfirm.modal")).not.toBeNull());
    click(byTestId("unleashConfirm.cancel")!);
    await flush();
    expect(byTestId("unleashConfirm.modal")).toBeNull();
    expect(unleashCalls(fake)).toEqual([]);
    expect(restartCalls(fake)).toEqual([]);
  });

  it("Confirm stores the flag, then fresh-restarts the session", async () => {
    const fake = await setupPanel();
    await openMenu();
    click(item()!);
    await waitFor(() => expect(byTestId("unleashConfirm.modal")).not.toBeNull());
    click(byTestId("unleashConfirm.confirm")!);
    await waitFor(() => expect(restartCalls(fake)).toHaveLength(1));
    expect(unleashCalls(fake)).toEqual([{ replicaPath: memberPath, unleashed: true }]);
    expect(restartCalls(fake)).toEqual([
      { id: "member-session", agentId: null, requestedProfile: null, skipAutoResume: null },
    ]);
    expect(orderOf(fake)).toEqual(["set_replica_unleashed", "restart_session"]);
    expect(byTestId("unleashConfirm.modal")).toBeNull();
    await openMenu();
    expect(item()!.textContent).toContain("Leash");
    expect(item()!.getAttribute("data-ac-state")).toBe("unleashed");
  });

  it("Leash makes both calls without a dialog", async () => {
    const fake = await setupPanel({ unleashed: true });
    await openMenu();
    click(item()!);
    await waitFor(() => expect(restartCalls(fake)).toHaveLength(1));
    expect(byTestId("unleashConfirm.modal")).toBeNull();
    expect(unleashCalls(fake)).toEqual([{ replicaPath: memberPath, unleashed: false }]);
    expect(orderOf(fake)).toEqual(["set_replica_unleashed", "restart_session"]);
  });

  it("does not restart when storing the flag fails", async () => {
    const fake = await setupPanel();
    fake.reject("set_replica_unleashed", "disk full");
    await openMenu();
    click(item()!);
    await waitFor(() => expect(byTestId("unleashConfirm.modal")).not.toBeNull());
    click(byTestId("unleashConfirm.confirm")!);
    await waitFor(() => expect(unleashCalls(fake)).toHaveLength(1));
    await flush();
    await flush();
    expect(restartCalls(fake)).toEqual([]);
    await openMenu();
    expect(item()!.getAttribute("data-ac-state")).toBe("leashed");
  });
});
