// @vitest-environment jsdom
// #4779 P04 - the "Temporary Rooms" sidebar category, driven over FakeTransport.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TemporaryRoomsPanel from "./TemporaryRoomsPanel";
import SidebarApp from "../App";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  contextMenu,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { installReconcileIntervalSpy, setupAppTransport } from "../testing/app-harness";
import { sessionsStore } from "../stores/sessions";
import { toastStore } from "../../shared/stores/toasts";
import type { AgentConfig, Session } from "../../shared/types";
import type { TempRoomAgentChoice, TempRoomInfo } from "../../shared/temp-rooms";

const ROOM_1: TempRoomInfo = { name: "temp-room-tr01", path: "/cfg/temp-room-tr01" };
const ROOM_2: TempRoomInfo = { name: "temp-room-tr02", path: "/cfg/temp-room-tr02" };
const CONTAINER_ERR = "temp rooms cannot use container transport";
const LIVE_ERR = "temp room temp-room-tr01 has a live session";

const agent = (id: string): AgentConfig => ({
  id,
  label: id,
  command: id,
  color: "#d97706",
  envs: [],
  isolatedHome: false,
});

interface Setup {
  rooms?: TempRoomInfo[];
  saved?: TempRoomAgentChoice;
  sessions?: Session[];
}

function setup({ rooms = [ROOM_1], saved, sessions = [] }: Setup = {}) {
  const fake = new FakeTransport();
  let listed = [...rooms];
  const settings = { ...baseSettings({ agents: [agent("claude")] }), tempRoomAgent: saved };
  fake.onInvoke("get_settings", () => settings);
  fake.onInvoke("update_settings", (args) => {
    Object.assign(settings, args.newSettings as object);
    return null;
  });
  fake.onInvoke("list_temp_rooms", () => listed);
  fake.onInvoke("create_temp_room", () => {
    const n = listed.length + 1;
    const room = { name: `temp-room-tr0${n}`, path: `/cfg/temp-room-tr0${n}` };
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
  fake.resolve("switch_session", null);
  fake.resolve("destroy_session", null);
  fake.resolve("open_in_explorer", null);
  sessionsStore.setSessions(sessions);
  const rendered = renderWithFakeTransport(() => <TemporaryRoomsPanel />, fake);
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

describe("#4779 TemporaryRoomsPanel", () => {
  let cleanupDom: (() => void) | null = null;
  let cleanups: (() => void)[] = [];

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });
  afterEach(() => {
    for (const c of cleanups) c();
    cleanups = [];
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
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

  it("6. live agent-row menu: catalogue order, room folder, disabled items inert", async () => {
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
      `${p}.menu.addToGroup`,
      `${p}.menu.editTaskTitle`,
      `${p}.menu.clearTaskTitle`,
    ]);
    const text = menu.textContent ?? "";
    expect(text).toContain("Open Room's Folder");
    for (const absent of ["Open Replica's Folder", "Open Matrix folder", "Delete"]) {
      expect(text).not.toContain(absent);
    }
    expect(menu.querySelector(".session-context-repo-option")).toBeNull();
    const titles: Record<string, string> = {
      ".menu.unleash": "Not available in Temporary Rooms yet",
      ".menu.addToGroup": "Groups need a project room",
      ".menu.editTaskTitle": "Temp rooms have no TASK.md",
      ".menu.clearTaskTitle": "Temp rooms have no TASK.md",
    };
    const before = fake.calls.length;
    for (const [suffix, title] of Object.entries(titles)) {
      const item = byId(`${p}${suffix}`) as HTMLButtonElement;
      expect(item.disabled).toBe(true);
      expect(item.className).toContain("context-option-disabled");
      expect(item.title).toContain(title);
      item.click();
    }
    expect(fake.calls.length).toBe(before);
    expect(document.querySelector(".session-context-flyout")).toBeNull();
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
});
