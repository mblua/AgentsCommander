// @vitest-environment jsdom
// The "Temporary Rooms" sidebar category, driven over FakeTransport.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TemporaryRoomsPanel from "./TemporaryRoomsPanel";
import ProjectPanel from "./ProjectPanel";
import SidebarApp from "../App";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  contextMenu,
  discovery,
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { installReconcileIntervalSpy, setupAppTransport } from "../testing/app-harness";
import { sessionsStore } from "../stores/sessions";
import { projectStore } from "../stores/project";
import { toastStore } from "../../shared/stores/toasts";
import { unleashedReplicasStore } from "../stores/unleashed-replicas";
import { tempRoomsStore } from "../stores/temp-rooms";
import type { AgentConfig, Session } from "../../shared/types";
import type { TempRoomAgentChoice, TempRoomInfo } from "../../shared/temp-rooms";
import { tempRoomPlaceholderWarning } from "../../shared/temp-rooms";

const ROOM_1: TempRoomInfo = { name: "temp-room-tr01", path: "/cfg/temp-room-tr01", unleashed: false };
const ROOM_2: TempRoomInfo = { name: "temp-room-tr02", path: "/cfg/temp-room-tr02", unleashed: false };
const CONTAINER_ERR = "temp rooms cannot use container transport";
const LIVE_ERR = "temp room temp-room-tr01 has a live session";

const agent = (id: string, command = id): AgentConfig => ({
  id,
  label: id,
  command,
  color: "#d97706",
  envs: [],
  isolatedHome: false,
});

interface Setup {
  rooms?: TempRoomInfo[];
  saved?: TempRoomAgentChoice;
  sessions?: Session[];
  withProject?: boolean;
  agents?: AgentConfig[];
}

const PROJECT = "/proj";
const WG = `${PROJECT}/.ac/wg-2-dev-team`;
const wgAgent = (name: string, isCoordinator: boolean) =>
  ({ name, path: `${WG}/__agent_${name}`, repoPaths: [], isCoordinator });
const projectDiscovery = () =>
  discovery({
    agents: [],
    teams: [],
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: WG,
        task: null,
        agents: [wgAgent("dev-webpage-ui", true), wgAgent("dev-rust", false)],
      },
    ],
  });

function setup(opts: Setup = {}) {
  const { rooms = [ROOM_1], saved, sessions = [], withProject } = opts;
  const fake = new FakeTransport();
  let listed = [...rooms];
  const agents = opts.agents ?? [agent("claude")];
  const settings = { ...baseSettings({ agents }), tempRoomAgent: saved };
  fake.onInvoke("get_settings", () => settings);
  fake.onInvoke("update_settings", (args) => {
    Object.assign(settings, args.newSettings as object);
    return null;
  });
  fake.onInvoke("list_temp_rooms", () => listed);
  fake.onInvoke("create_temp_room", () => {
    const n = listed.length + 1;
    const room: TempRoomInfo = {
      name: `temp-room-tr0${n}`,
      path: `/cfg/temp-room-tr0${n}`,
      unleashed: false,
    };
    listed = [...listed, room];
    return room;
  });
  fake.onInvoke("delete_temp_room", (args) => {
    listed = listed.filter((r) => r.name !== args.name);
    return null;
  });
  fake.onInvoke("create_session", (args) =>
    session({ id: "new", name: String(args.sessionName), workingDirectory: String(args.cwd) }),
  );
  fake.onInvoke("restart_session", (args) => session({ id: String(args.id) }));
  fake.resolve("get_temp_room_groups", {
    groups: [],
    nonStop: null,
    showAll: true,
    showUngrouped: true,
  });
  fake.resolve("switch_session", null);
  fake.resolve("destroy_session", null);
  fake.resolve("open_in_explorer", null);
  fake.resolve("new_project", { path: PROJECT, registered: true, created: false });
  fake.resolve("discover_project", projectDiscovery());
  sessionsStore.setSessions(sessions);
  const view = () => (
    <>
      <TemporaryRoomsPanel />
      {withProject && <ProjectPanel />}
    </>
  );
  const rendered = renderWithFakeTransport(view, fake);
  return { fake, rendered };
}

const byId = (id: string) => document.querySelector<HTMLElement>(`[data-ac-testid="${id}"]`);
const roomRow = (room = ROOM_1) => byId(`tempRoom.${room.name}.row`);

async function openCategoryMenu() {
  await waitFor(() => expect(byId("tempRooms.header")).not.toBeNull());
  contextMenu(byId("tempRooms.header")!);
  await waitFor(() => expect(byId("tempRooms.menu.create")).not.toBeNull());
}

async function openAgentMenu(room = ROOM_1) {
  await waitFor(() => expect(roomRow(room)).not.toBeNull());
  contextMenu(roomRow(room)!);
  await waitFor(() => expect(byId(`tempRoom.${room.name}.menu`)).not.toBeNull());
  return byId(`tempRoom.${room.name}.menu`)!;
}

async function applyPicker() {
  const sel = '[data-ac-testid="agentPicker.apply"]';
  await waitFor(() => {
    const apply = document.querySelector<HTMLButtonElement>(sel);
    expect(apply).not.toBeNull();
    expect(apply!.disabled).toBe(false);
  });
  click(document.querySelector(sel)!);
}

const liveSession = (overrides: Partial<Session> = {}) =>
  session({
    id: "tr1",
    name: ROOM_1.name,
    workingDirectory: ROOM_1.path,
    status: "running",
    ...overrides,
  });

const SAVED: TempRoomAgentChoice = { agentId: "claude", requestedProfile: null };

describe("TemporaryRoomsPanel", () => {
  let cleanupDom: (() => void) | null = null;
  let cleanups: (() => void)[] = [];

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    tempRoomsStore.resetForTests();
  });
  afterEach(() => {
    for (const c of cleanups) c();
    cleanups = [];
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    unleashedReplicasStore.resetForTests();
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });
  const track = <T extends { rendered: { cleanup: () => void } }>(r: T): T => {
    cleanups.push(r.rendered.cleanup);
    return r;
  };

  it("1. sits after the Root banner and before the first project; shows the count", async () => {
    const intervals = installReconcileIntervalSpy({ periodMs: 5000 });
    const fake = new FakeTransport();
    setupAppTransport(fake, { projectPath: "/proj", agents: [], rows: () => [] });
    fake.resolve("list_temp_rooms", [ROOM_1, ROOM_2]);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => {
        const titles = Array.from(document.querySelectorAll(".project-title"));
        expect(titles.some((t) => t.textContent?.startsWith("Project: "))).toBe(true);
        expect(byId("tempRooms.count")?.textContent).toBe("2");
      });
      const banner = document.querySelector(".root-agent-banner")!;
      const header = byId("tempRooms.header")!;
      const project = Array.from(document.querySelectorAll(".project-title")).find((t) =>
        t.textContent?.startsWith("Project: "),
      )!;
      const FOLLOWING = Node.DOCUMENT_POSITION_FOLLOWING;
      expect(banner.compareDocumentPosition(header) & FOLLOWING).toBeTruthy();
      expect(header.compareDocumentPosition(project) & FOLLOWING).toBeTruthy();
    } finally {
      rendered.cleanup();
      intervals.clear();
      intervals.restore();
    }
  });

  it("1b. the category renders with zero temp rooms", async () => {
    track(setup({ rooms: [] }));
    await waitFor(() => expect(byId("tempRooms.count")?.textContent).toBe("0"));
    expect(byId("tempRooms.header")?.textContent).toContain("Temporary Rooms");
  });

  it("2. right-click on the category: first item is Create new Temp Room", async () => {
    track(setup());
    await openCategoryMenu();
    const first = byId("tempRooms.menu")!.querySelector("button")!;
    expect(first.textContent?.trim()).toBe("Create new Temp Room");
  });

  it("3. create, no saved choice: picker, save tempRoomAgent, then create_session", async () => {
    const { fake } = track(setup({ rooms: [] }));
    await openCategoryMenu();
    click(byId("tempRooms.menu.create")!);
    await waitFor(() => expect(fake.callsFor("create_temp_room")).toHaveLength(1));
    await applyPicker();
    await waitFor(() => expect(fake.callsFor("create_session")).toHaveLength(1));
    const saved = fake.lastCall("update_settings")!.args.newSettings as Record<string, unknown>;
    expect(saved.tempRoomAgent).toEqual({ agentId: "claude", requestedProfile: null });
    expect(saved.agents).toBeDefined(); // the full settings object, not a fragment
    expect(fake.lastCall("create_session")!.args).toMatchObject({
      cwd: ROOM_1.path,
      sessionName: "temp-room-tr01",
      agentId: "claude",
    });
    const order = fake.calls.map((call) => call.cmd);
    expect(order.indexOf("update_settings")).toBeLessThan(order.indexOf("create_session"));
    await waitFor(() => expect(roomRow()).not.toBeNull());
  });

  it("4. a saved valid choice skips the picker; an unknown agentId shows it", async () => {
    const { fake } = track(setup({ rooms: [], saved: SAVED }));
    await openCategoryMenu();
    click(byId("tempRooms.menu.create")!);
    await waitFor(() => expect(fake.callsFor("create_session")).toHaveLength(1));
    expect(byId("agentPicker.modal")).toBeNull();
    expect(fake.callsFor("update_settings")).toHaveLength(0);
    cleanups.pop()?.();
    document.body.replaceChildren();

    const second = track(setup({ rooms: [], saved: { agentId: "gone", requestedProfile: null } }));
    await openCategoryMenu();
    click(byId("tempRooms.menu.create")!);
    await waitFor(() => expect(byId("agentPicker.modal")).not.toBeNull());
    expect(second.fake.callsFor("create_session")).toHaveLength(0);
  });

  it("4c. a saved agent using %AC_MATRIX_ROOT% shows the red warning", async () => {
    track(setup({ saved: SAVED, agents: [agent("claude", "claude %AC_MATRIX_ROOT%")] }));
    await waitFor(() => expect(byId("tempRooms.placeholderWarning")).not.toBeNull());
    const warning = byId("tempRooms.placeholderWarning")!;
    expect(warning.className).toBe("new-agent-error");
    expect(warning.textContent).toBe(tempRoomPlaceholderWarning("%AC_MATRIX_ROOT%"));
  });

  it("4d. a clean saved agent shows no warning", async () => {
    const { fake } = track(setup({ saved: SAVED }));
    await waitFor(() => expect(fake.callsFor("get_settings").length).toBeGreaterThan(0));
    await waitFor(() => expect(roomRow()).not.toBeNull());
    expect(byId("tempRooms.placeholderWarning")).toBeNull();
  });

  it("4e. chosen agent with %AC_MATRIX_ROOT%: warning and exactly one toast", async () => {
    const matrixErr = "%AC_MATRIX_ROOT% does not apply to Temporary Rooms: they have no Agent Matrix";
    const { fake } = track(
      setup({ rooms: [], agents: [agent("claude", "claude %AC_MATRIX_ROOT%")] }),
    );
    fake.reject("create_session", matrixErr);
    const toast = vi.spyOn(toastStore, "error");
    await openCategoryMenu();
    click(byId("tempRooms.menu.create")!);
    await waitFor(() => expect(fake.callsFor("create_temp_room")).toHaveLength(1));
    await applyPicker();
    await waitFor(() => expect(byId("tempRooms.placeholderWarning")).not.toBeNull());
    expect(byId("tempRooms.placeholderWarning")!.textContent).toBe(
      tempRoomPlaceholderWarning("%AC_MATRIX_ROOT%"),
    );
    await waitFor(() => expect(toast).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 20));
    expect(toast).toHaveBeenCalledTimes(1);
    expect(toast.mock.calls[0]?.[0]).toContain(matrixErr);
  });

  it("4b. two quick creates give two rows", async () => {
    const { fake } = track(setup({ rooms: [], saved: SAVED }));
    await openCategoryMenu();
    click(byId("tempRooms.menu.create")!);
    await openCategoryMenu();
    click(byId("tempRooms.menu.create")!);
    await waitFor(() => expect(fake.callsFor("create_session")).toHaveLength(2));
    await waitFor(() => {
      expect(roomRow(ROOM_1)).not.toBeNull();
      expect(roomRow(ROOM_2)).not.toBeNull();
    });
  });

  it("5. create_session rejects with the container message: toast, row stays", async () => {
    const { fake } = track(setup({ rooms: [], saved: SAVED }));
    fake.reject("create_session", CONTAINER_ERR);
    const toast = vi.spyOn(toastStore, "error");
    await openCategoryMenu();
    click(byId("tempRooms.menu.create")!);
    await waitFor(() => expect(toast).toHaveBeenCalled());
    expect(toast.mock.calls[0]?.[0]).toContain(CONTAINER_ERR);
    expect(roomRow()).not.toBeNull();
  });

  it("6. live agent-row menu: catalogue order, room folder, items enabled", async () => {
    const { fake } = track(setup({ sessions: [liveSession()] }));
    const menu = await openAgentMenu();
    const p = `tempRoom.${ROOM_1.name}`;
    const ids = Array.from(menu.querySelectorAll("[data-ac-testid]")).map((e) =>
      e.getAttribute("data-ac-testid"),
    );
    expect(ids).toEqual([
      `${p}.restart`,
      `${p}.codingAgent`,
      `${p}.openFolder`,
      `${p}.menu.unleash`,
      `${p}.close`,
      `${p}.menu.detachToggle`,
      `${p}.menu.telegram`,
      `${p}.groups.trigger`,
      `${p}.menu.editTaskTitle`,
      `${p}.menu.clearTaskTitle`,
    ]);
    const text = menu.textContent ?? "";
    expect(text).toContain("Open Room's Folder");
    for (const absent of ["Open Replica's Folder", "Open Matrix folder", "Delete"]) {
      expect(text).not.toContain(absent);
    }
    expect(menu.querySelector(".session-context-repo-option")).toBeNull();
    // No dimmed class, no offline title.
    for (const suffix of [".groups.trigger", ".menu.editTaskTitle", ".menu.clearTaskTitle"]) {
      const item = byId(`${p}${suffix}`) as HTMLButtonElement;
      expect(item.className).not.toContain("context-option-disabled");
      for (const old of ["Groups need a project room", "Temp rooms have no TASK.md"]) {
        expect(item.title ?? "").not.toContain(old);
      }
    }
    expect(fake.calls.length).toBeGreaterThan(0);
  });

  const unleashItem = () => byId(`tempRoom.${ROOM_1.name}.menu.unleash`) as HTMLButtonElement;
  const confirmBtn = () => byId("unleashConfirm.confirm");

  it("unleash_item_enabled_and_labelled_unleash", async () => {
    const { fake } = track(setup({ sessions: [liveSession()] }));
    fake.resolve("set_replica_unleashed", undefined);
    await openAgentMenu();
    expect(unleashItem().disabled).toBe(false);
    expect(unleashItem().textContent).toContain("Unleash");
  });

  it("unleash_confirm_stores_flag_and_restarts", async () => {
    const { fake } = track(setup({ sessions: [liveSession()] }));
    fake.resolve("set_replica_unleashed", undefined);
    await openAgentMenu();
    click(unleashItem());
    await waitFor(() => expect(confirmBtn()).not.toBeNull());
    click(confirmBtn()!);
    await waitFor(() => expect(fake.callsFor("restart_session")).toHaveLength(1));
    expect(fake.lastCall("set_replica_unleashed")!.args).toEqual({
      replicaPath: ROOM_1.path,
      unleashed: true,
    });
    expect(confirmBtn()).toBeNull();
    await openAgentMenu();
    expect(unleashItem().textContent).toContain("Leash");
    expect(unleashItem().textContent).not.toContain("Unleash");
  });

  it("unleash_cancel_does_nothing", async () => {
    const { fake } = track(setup({ sessions: [liveSession()] }));
    fake.resolve("set_replica_unleashed", undefined);
    await openAgentMenu();
    click(unleashItem());
    await waitFor(() => expect(byId("unleashConfirm.cancel")).not.toBeNull());
    click(byId("unleashConfirm.cancel")!);
    await waitFor(() => expect(confirmBtn()).toBeNull());
    expect(fake.callsFor("set_replica_unleashed")).toHaveLength(0);
    expect(fake.callsFor("restart_session")).toHaveLength(0);
  });

  it("unleashed_room_shows_leash_and_leashes_without_modal", async () => {
    const room = { ...ROOM_1, unleashed: true };
    const { fake } = track(setup({ rooms: [room], sessions: [liveSession()] }));
    fake.resolve("set_replica_unleashed", undefined);
    await openAgentMenu(room);
    expect(unleashItem().textContent).toContain("Leash");
    expect(unleashItem().textContent).not.toContain("Unleash");
    click(unleashItem());
    await waitFor(() => expect(fake.callsFor("restart_session")).toHaveLength(1));
    expect(confirmBtn()).toBeNull();
    expect(fake.lastCall("set_replica_unleashed")!.args).toEqual({
      replicaPath: ROOM_1.path,
      unleashed: false,
    });
  });

  it("unleash_confirm_adds_halo_class_to_row", async () => {
    const { fake } = track(setup({ sessions: [liveSession()] }));
    fake.resolve("set_replica_unleashed", undefined);
    await openAgentMenu();
    expect(roomRow()!.classList.contains("unleashed")).toBe(false);
    click(unleashItem());
    await waitFor(() => expect(confirmBtn()).not.toBeNull());
    click(confirmBtn()!);
    await waitFor(() => expect(roomRow()!.classList.contains("unleashed")).toBe(true));
  });

  it("leash_removes_halo_class_from_row", async () => {
    const room = { ...ROOM_1, unleashed: true };
    const { fake } = track(setup({ rooms: [room], sessions: [liveSession()] }));
    fake.resolve("set_replica_unleashed", undefined);
    await waitFor(() => expect(roomRow(room)).not.toBeNull());
    expect(roomRow(room)!.classList.contains("unleashed")).toBe(true);
    await openAgentMenu(room);
    click(unleashItem());
    await waitFor(() => expect(roomRow(room)!.classList.contains("unleashed")).toBe(false));
  });

  it("unleash_store_failure_toasts_and_skips_restart", async () => {
    const { fake } = track(setup({ sessions: [liveSession()] }));
    fake.reject("set_replica_unleashed", "disk full");
    const toast = vi.spyOn(toastStore, "error");
    await openAgentMenu();
    click(unleashItem());
    await waitFor(() => expect(confirmBtn()).not.toBeNull());
    click(confirmBtn()!);
    await waitFor(() => expect(toast).toHaveBeenCalled());
    expect(toast.mock.calls[0]?.[0]).toContain("disk full");
    expect(fake.callsFor("restart_session")).toHaveLength(0);
    expect(unleashedReplicasStore.isUnleashed(ROOM_1)).toBe(false);
  });

  it("7. Open Room's Folder opens the room path", async () => {
    const { fake } = track(setup({ sessions: [liveSession()] }));
    await openAgentMenu();
    click(byId(`tempRoom.${ROOM_1.name}.openFolder`)!);
    await waitFor(() =>
      expect(fake.lastCall("open_in_explorer")?.args).toEqual({ path: ROOM_1.path }),
    );
  });

  it("7b. Open Room's Folder is present with no session", async () => {
    const { fake } = track(setup());
    await openAgentMenu();
    click(byId(`tempRoom.${ROOM_1.name}.openFolder`)!);
    await waitFor(() =>
      expect(fake.lastCall("open_in_explorer")?.args).toEqual({ path: ROOM_1.path }),
    );
  });

  it("8. Close Session keeps the room listed", async () => {
    const { fake } = track(setup({ sessions: [liveSession()] }));
    await openAgentMenu();
    click(byId(`tempRoom.${ROOM_1.name}.close`)!);
    await waitFor(() => expect(fake.callsFor("destroy_session")).toHaveLength(1));
    expect(fake.callsFor("delete_temp_room")).toHaveLength(0);
    expect(roomRow()).not.toBeNull();
  });

  it("8c. the task title label carries the full title as tooltip", async () => {
    const full = "A very long temporary room task title that the sidebar ellipsizes";
    const { fake } = track(setup({
      rooms: [{ ...ROOM_1, taskTitle: `  ${full}  ` }],
      sessions: [liveSession()],
    }));
    await waitFor(() => expect(byId("tempRoom.temp-room-tr01.taskTitle")).not.toBeNull());
    const label = byId("tempRoom.temp-room-tr01.taskTitle")!;
    expect(label.textContent).toBe(full);
    expect(label.getAttribute("title")).toBe(full);
    expect(label.getAttribute("data-ac-role")).toBe("text");
    expect(label.getAttribute("data-ac-state")).toBe("task");
    expect(label.closest(".ac-wg-header")!.getAttribute("title")).toBe(ROOM_1.path);
    click(label);
    label.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    label.dispatchEvent(new KeyboardEvent("keydown", { key: " ", bubbles: true }));
    expect(fake.callsFor("switch_session")).toHaveLength(0);
    expect(fake.callsFor("create_session")).toHaveLength(0);
    expect(fake.callsFor("restart_session")).toHaveLength(0);

    contextMenu(label);
    await waitFor(() => expect(byId("tempRoom.headerMenu.delete")).not.toBeNull());
  });

  it("keeps task title text and hooks outside the Tab order", async () => {
    const full = "A long temporary room task title that remains available as a tooltip";
    track(setup({ rooms: [{ ...ROOM_1, taskTitle: `  ${full}  ` }] }));
    const taskTitleId = "tempRoom.temp-room-tr01.taskTitle";
    await waitFor(() => expect(byId(taskTitleId)).not.toBeNull());
    const label = byId(taskTitleId)!;
    expect(label.hasAttribute("tabindex")).toBe(false);
    expect(label.textContent).toBe(full);
    expect(label.getAttribute("title")).toBe(full);
    expect(label.getAttribute("data-ac-role")).toBe("text");
    expect(label.getAttribute("data-ac-state")).toBe("task");
  });

  it("8b. the header collapse button carries tempRooms.header.toggle", async () => {
    track(setup());
    await waitFor(() => expect(byId("tempRooms.header.toggle")).not.toBeNull());
    expect(document.querySelectorAll('[data-ac-testid="tempRooms.header.toggle"]')).toHaveLength(1);
    const toggle = byId("tempRooms.header.toggle")!;
    const before = toggle.getAttribute("aria-expanded");
    click(toggle);
    await waitFor(() => expect(toggle.getAttribute("aria-expanded")).not.toBe(before));
  });

  it("9. no session: row click and Restart Session both launch with the saved choice", async () => {
    const { fake } = track(setup({ saved: SAVED }));
    await waitFor(() => expect(roomRow()).not.toBeNull());
    click(roomRow()!);
    await waitFor(() => expect(fake.callsFor("create_session")).toHaveLength(1));
    await openAgentMenu();
    click(byId(`tempRoom.${ROOM_1.name}.restart`)!);
    await waitFor(() => expect(fake.callsFor("create_session")).toHaveLength(2));
    for (const call of fake.callsFor("create_session")) {
      expect(call.args).toMatchObject({ cwd: ROOM_1.path, agentId: "claude" });
    }
    expect(fake.callsFor("restart_session")).toHaveLength(0);
  });

  it("9c. Enter on the row's key proxy does what a click does", async () => {
    const { fake } = track(setup({ saved: SAVED }));
    await waitFor(() => expect(roomRow()).not.toBeNull());
    const proxy = roomRow()!.querySelector(".ac-row-key-proxy")!;
    proxy.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await waitFor(() => expect(fake.callsFor("create_session")).toHaveLength(1));
    expect(fake.lastCall("create_session")!.args).toMatchObject({ cwd: ROOM_1.path });
  });

  it("switches a live room once per proxy activation and row click", async () => {
    const { fake } = track(setup({ sessions: [liveSession()] }));
    await waitFor(() => expect(roomRow()).not.toBeNull());
    const row = roomRow()!;
    const stops = Array.from(row.querySelectorAll<HTMLButtonElement>("button, [tabindex]"));
    expect(stops).toHaveLength(1);
    const [proxy] = stops;
    expect(proxy).toBeInstanceOf(HTMLButtonElement);
    expect(proxy.getAttribute("aria-label")).toBe(ROOM_1.name);
    expect(proxy.tabIndex).toBe(0);
    proxy.focus();
    expect(document.activeElement).toBe(proxy);

    const press = (key: string, repeat = false) => {
      proxy.dispatchEvent(new KeyboardEvent("keydown", { key, repeat, bubbles: true }));
    };
    press("Enter");
    await waitFor(() => expect(fake.callsFor("switch_session")).toHaveLength(1));
    press(" ");
    await waitFor(() => expect(fake.callsFor("switch_session")).toHaveLength(2));
    press("Enter", true);
    press(" ", true);
    press("ArrowDown");
    press("Escape");
    expect(fake.callsFor("switch_session")).toHaveLength(2);

    click(row.querySelector(".replica-item-name")!);
    await waitFor(() => expect(fake.callsFor("switch_session")).toHaveLength(3));
    expect(fake.callsFor("switch_session").map(({ args }) => args)).toEqual([
      { id: "tr1" }, { id: "tr1" }, { id: "tr1" },
    ]);
    await openAgentMenu();
    expect(fake.callsFor("switch_session")).toHaveLength(3);
  });

  it("9b. an exited session: row click restarts it", async () => {
    const { fake } = track(setup({ sessions: [liveSession({ status: { exited: 0 } })] }));
    await waitFor(() => expect(roomRow()).not.toBeNull());
    click(roomRow()!);
    await waitFor(() => expect(fake.lastCall("restart_session")?.args.id).toBe("tr1"));
  });

  async function openDeleteModal() {
    await waitFor(() => expect(byId(`tempRoom.${ROOM_1.name}.header`)).not.toBeNull());
    contextMenu(byId(`tempRoom.${ROOM_1.name}.header`)!);
    await waitFor(() => expect(byId("tempRoom.headerMenu.delete")).not.toBeNull());
    click(byId("tempRoom.headerMenu.delete")!);
    await waitFor(() => expect(byId("tempRoom.deleteModal")).not.toBeNull());
  }

  it("10. delete: Cancel invokes nothing; Delete deletes, then refreshes", async () => {
    const { fake } = track(setup());
    await openDeleteModal();
    expect(byId("tempRoom.deleteModal")!.textContent).toContain("Delete temp room temp-room-tr01?");
    click(byId("tempRoom.deleteModal.cancel")!);
    expect(byId("tempRoom.deleteModal")).toBeNull();
    expect(fake.callsFor("delete_temp_room")).toHaveLength(0);

    await openDeleteModal();
    const listsBefore = fake.callsFor("list_temp_rooms").length;
    click(byId("tempRoom.deleteModal.confirm")!);
    await waitFor(() => expect(byId("tempRoom.deleteModal")).toBeNull());
    expect(fake.lastCall("delete_temp_room")!.args).toEqual({ name: ROOM_1.name });
    expect(fake.callsFor("list_temp_rooms").length).toBeGreaterThan(listsBefore);
    await waitFor(() => expect(roomRow()).toBeNull());
  });

  it("10b. delete refused (live session): error shown, modal stays, room listed", async () => {
    const { fake } = track(setup({ sessions: [liveSession()] }));
    fake.reject("delete_temp_room", LIVE_ERR);
    await openDeleteModal();
    click(byId("tempRoom.deleteModal.confirm")!);
    await waitFor(() =>
      expect(byId("tempRoom.deleteModal.error")?.textContent).toContain(LIVE_ERR),
    );
    expect(byId("tempRoom.deleteModal")).not.toBeNull();
    expect(roomRow()).not.toBeNull();
  });

  const filterInput = () => byId("sidebar.regexFilter.input") as HTMLInputElement;
  const escape = () =>
    filterInput().dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
    );
  const openFilter = async () => {
    await waitFor(() => expect(byId("sidebar.regexFilter.toggle")).not.toBeNull());
    click(byId("sidebar.regexFilter.toggle")!);
    await waitFor(() =>
      expect(byId("sidebar.regexFilter.toggle")!.getAttribute("aria-expanded")).toBe("true"),
    );
  };
  const openHeaders = () =>
    Array.from(document.querySelectorAll(".project-header"))
      .filter((h) => h.querySelector(".project-filter-row.open"))
      .map((h) => h.getAttribute("data-ac-testid"));
  const filterRow = () => byId("sidebar.regexFilter.row")!;
  const filterError = () => byId("sidebar.regexFilter.error");

  it("11. regex filter on the header filters temp rooms", async () => {
    track(setup({ rooms: [ROOM_1, ROOM_2] }));
    await waitFor(() => expect(roomRow(ROOM_2)).not.toBeNull());
    const header = byId("tempRooms.header")!;
    expect(header.classList.contains("project-header--has-indicator")).toBe(true);
    expect(byId("tempRooms.count")!.classList.contains("project-header-indicator--last")).toBe(
      true,
    );
    expect(header.contains(byId("sidebar.regexFilter.toggle"))).toBe(true);
    await openFilter();
    expect(header.contains(filterInput())).toBe(true);
    expect(openHeaders()).toEqual(["tempRooms.header"]);

    input(filterInput(), "tr02");
    await waitFor(() => expect(roomRow(ROOM_1)).toBeNull());
    expect(roomRow(ROOM_2)).not.toBeNull();
    expect(byId("tempRooms.count")!.textContent).toBe("1");

    input(filterInput(), "Temporary");
    await waitFor(() => expect(roomRow(ROOM_1)).not.toBeNull());
    expect(roomRow(ROOM_2)).not.toBeNull();

    input(filterInput(), "(");
    await waitFor(() =>
      expect(byId("sidebar.regexFilter.error")?.textContent).toBe("Invalid regex"),
    );
    expect(filterError()).not.toBeNull();
    expect(roomRow(ROOM_1)).not.toBeNull();
    expect(roomRow(ROOM_2)).not.toBeNull();

    escape();
    await waitFor(() => expect(filterInput().value).toBe(""));
    expect(filterError()).toBeNull();
    escape();
    await waitFor(() =>
      expect(byId("sidebar.regexFilter.toggle")!.getAttribute("aria-expanded")).toBe("false"),
    );
    expect(filterRow().classList.contains("open")).toBe(false);
  });

  it("11b. collapsed and empty: the control stays in the header and toggles", async () => {
    track(setup({ rooms: [] }));
    await waitFor(() => expect(byId("tempRooms.count")?.textContent).toBe("0"));
    const header = byId("tempRooms.header")!;
    expect(header.contains(byId("sidebar.regexFilter.toggle"))).toBe(true);
    await openFilter();
    click(byId("sidebar.regexFilter.toggle")!);
    await waitFor(() => expect(filterRow().classList.contains("open")).toBe(false));
    click(header.querySelector(".project-header-main")!);
    await waitFor(() =>
      expect(header.querySelector(".project-header-main")!.getAttribute("aria-expanded")).toBe(
        "false",
      ),
    );
    expect(header.contains(byId("sidebar.regexFilter.toggle"))).toBe(true);
    await openFilter();
    expect(filterRow().classList.contains("open")).toBe(true);
    input(filterInput(), "(");
    await waitFor(() => expect(filterError()).not.toBeNull());
    expect(header.contains(filterError())).toBe(true);
    input(filterInput(), "");
    await waitFor(() => expect(filterError()).toBeNull());
  });

  it("11c. one filter for temp rooms and projects", async () => {
    const { rendered } = track(setup({ rooms: [ROOM_1], withProject: true }));
    await projectStore.createAndLoad(PROJECT);
    const text = () => rendered.root.textContent ?? "";
    await waitFor(() => expect(text()).toContain("dev-rust"));
    expect(text()).toContain("dev-webpage-ui");
    await waitFor(() => expect(roomRow()).not.toBeNull());

    await openFilter();
    expect(openHeaders()).toEqual(["tempRooms.header"]);
    input(filterInput(), "webpage");
    await waitFor(() => expect(text()).not.toContain("dev-rust"));
    expect(text()).toContain("dev-webpage-ui");
    expect(roomRow()).toBeNull();

    click(byId("sidebar.regexFilter.clear")!);
    await waitFor(() => expect(text()).toContain("dev-rust"));
    expect(roomRow()).not.toBeNull();

    const headers = Array.from(document.querySelectorAll(".project-header"));
    expect(headers.length).toBeGreaterThan(1);
    for (const h of headers) {
      if (h === byId("tempRooms.header")) continue;
      expect(h.querySelector(".project-filter-row")).toBeNull();
      expect(h.querySelector(".project-filter-error")).toBeNull();
    }
  });

  it("11d. right-click in the filter input keeps the native menu", async () => {
    track(setup());
    await openFilter();
    const ev = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    filterInput().dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(false);
    expect(byId("tempRooms.menu")).toBeNull();
    await openCategoryMenu();
  });

  // One prototype stub gives the scroll area (the render
  // container) and each group panel (told apart by its header testid) a rect.
  let layout: "top" | "project" = "top";
  const stubLayout = (scroller: HTMLElement) =>
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (
      this: Element,
    ) {
      const r = (top: number, bottom: number) =>
        ({ top, bottom, left: 0, right: 0, width: 0, height: bottom - top }) as DOMRect;
      if (this === scroller) return r(0, 300);
      if (!this.classList?.contains("project-panel")) return r(0, 0);
      const id = this.querySelector(":scope > .project-header")?.getAttribute("data-ac-testid");
      if (id === "tempRooms.header") return layout === "top" ? r(0, 100) : r(-100, -50);
      if (id?.startsWith("project.header.")) return layout === "top" ? r(100, 400) : r(-50, 400);
      return r(0, 0);
    });
  const projectHeader = () =>
    document.querySelector<HTMLElement>(
      '.project-panel > .project-header[data-ac-testid^="project.header."]',
    );

  async function hostInProject() {
    layout = "top";
    const { rendered } = track(setup({ rooms: [ROOM_1], withProject: true }));
    stubLayout(rendered.root);
    await projectStore.createAndLoad(PROJECT);
    await waitFor(() => expect(projectHeader()).not.toBeNull());
    await openFilter();
    input(filterInput(), "dev");
    filterInput().focus();
    layout = "project";
    rendered.root.dispatchEvent(new Event("scroll"));
    await waitFor(() => expect(projectHeader()!.contains(filterRow())).toBe(true));
    return rendered;
  }

  it("11e. scrolled into a project the one filter moves there and keeps working", async () => {
    await hostInProject();
    expect(document.querySelectorAll(".project-filter-row")).toHaveLength(1);
    expect(document.activeElement).toBe(filterInput());
    expect(filterInput().value).toBe("dev");
    const toggle = () => byId("sidebar.regexFilter.toggle")!;
    expect(toggle().getAttribute("aria-expanded")).toBe("true");
    expect(openHeaders()).toEqual([projectHeader()!.getAttribute("data-ac-testid")]);

    click(byId("sidebar.regexFilter.clear")!);
    await waitFor(() => expect(filterInput().value).toBe(""));
    escape();
    await waitFor(() => expect(toggle().getAttribute("aria-expanded")).toBe("false"));
    click(toggle());
    await waitFor(() => expect(toggle().getAttribute("aria-expanded")).toBe("true"));

    const ev = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    filterInput().dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(false);
    expect(document.querySelector(".session-context-menu")).toBeNull();

    const row = filterRow();
    click(toggle());
    await waitFor(() => expect(row.classList.contains("open")).toBe(false));
    click(toggle());
    await waitFor(() => expect(row.classList.contains("open")).toBe(true));
    expect(filterRow()).toBe(row);
    expect(projectHeader()!.contains(row)).toBe(true);
  });

  it("11g. the invalid-regex error follows the hosting header", async () => {
    const rendered = await hostInProject();
    input(filterInput(), "(");
    await waitFor(() => expect(filterError()?.textContent).toBe("Invalid regex"));
    const error = filterError()!;
    expect(projectHeader()!.contains(error)).toBe(true);
    expect(byId("sidebar.regexFilter.errorSlot")!.contains(error)).toBe(true);
    expect(document.querySelectorAll('[data-ac-testid="sidebar.regexFilter.error"]')).toHaveLength(
      1,
    );
    expect(byId("tempRooms.header")!.contains(error)).toBe(false);

    const ev = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    error.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(true);
    await waitFor(() =>
      expect(document.querySelector(".session-context-menu")?.textContent).toContain("New Agent"),
    );
    expect(byId("tempRooms.menu")).toBeNull();
    expect(byId(`tempRoom.${ROOM_1.name}.menu`)).toBeNull();

    layout = "top";
    rendered.root.dispatchEvent(new Event("scroll"));
    await waitFor(() => expect(byId("tempRooms.header")!.contains(filterRow())).toBe(true));
    expect(filterError()).toBe(error);
    expect(byId("tempRooms.header")!.contains(error)).toBe(true);
    input(filterInput(), "dev");
    await waitFor(() => expect(filterError()).toBeNull());
    expect(byId("tempRooms.header")!.contains(byId("sidebar.regexFilter.errorSlot"))).toBe(true);
  });

  it("11f. a re-created project panel re-hosts the row with focus and caret", async () => {
    await hostInProject();
    filterInput().setSelectionRange(1, 3);
    const oldHeader = projectHeader()!;
    projectStore.updateWorkgroupTask(WG, "x", "x");
    await waitFor(() => expect(oldHeader.isConnected).toBe(false));
    await waitFor(() => {
      expect(projectHeader()!.contains(filterRow())).toBe(true);
      expect(projectHeader()!.contains(byId("sidebar.regexFilter.errorSlot"))).toBe(true);
    });
    expect(document.querySelectorAll(".project-filter-row")).toHaveLength(1);
    expect(document.activeElement).toBe(filterInput());
    expect(filterInput().value).toBe("dev");
    expect([filterInput().selectionStart, filterInput().selectionEnd]).toEqual([1, 3]);

    input(filterInput(), "(");
    await waitFor(() => expect(filterError()).not.toBeNull());
    const before = projectHeader()!;
    projectStore.updateWorkgroupTask(WG, "y", "y");
    await waitFor(() => expect(before.isConnected).toBe(false));
    await waitFor(() => expect(projectHeader()!.contains(filterError())).toBe(true));
    expect(document.querySelectorAll('[data-ac-testid="sidebar.regexFilter.error"]')).toHaveLength(
      1,
    );
  });
});
