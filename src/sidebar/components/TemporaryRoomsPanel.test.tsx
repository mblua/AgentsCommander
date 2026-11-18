// @vitest-environment jsdom
// #4779 P04 - the "Temporary Rooms" sidebar category, driven over FakeTransport.
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
  withProject?: boolean;
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

function setup({ rooms = [ROOM_1], saved, sessions = [], withProject }: Setup = {}) {
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

  it("9c. Enter on the row's key proxy does what a click does", async () => {
    const { fake } = track(setup({ saved: SAVED }));
    await waitFor(() => expect(roomRow()).not.toBeNull());
    const proxy = roomRow()!.querySelector(".ac-row-key-proxy")!;
    proxy.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await waitFor(() => expect(fake.callsFor("create_session")).toHaveLength(1));
    expect(fake.lastCall("create_session")!.args).toMatchObject({ cwd: ROOM_1.path });
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
    Array.from(document.querySelectorAll(".project-header.open")).map((h) =>
      h.getAttribute("data-ac-testid"),
    );

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
    expect(header.classList.contains("invalid")).toBe(true);
    expect(roomRow(ROOM_1)).not.toBeNull();
    expect(roomRow(ROOM_2)).not.toBeNull();

    escape();
    await waitFor(() => expect(filterInput().value).toBe(""));
    expect(header.classList.contains("invalid")).toBe(false);
    escape();
    await waitFor(() =>
      expect(byId("sidebar.regexFilter.toggle")!.getAttribute("aria-expanded")).toBe("false"),
    );
    expect(header.classList.contains("open")).toBe(false);
  });

  it("11b. collapsed and empty: the control stays in the header and toggles", async () => {
    track(setup({ rooms: [] }));
    await waitFor(() => expect(byId("tempRooms.count")?.textContent).toBe("0"));
    const header = byId("tempRooms.header")!;
    expect(header.contains(byId("sidebar.regexFilter.toggle"))).toBe(true);
    await openFilter();
    click(byId("sidebar.regexFilter.toggle")!);
    await waitFor(() => expect(header.classList.contains("open")).toBe(false));
    click(header.querySelector(".project-header-main")!);
    await waitFor(() =>
      expect(header.querySelector(".project-header-main")!.getAttribute("aria-expanded")).toBe(
        "false",
      ),
    );
    expect(header.contains(byId("sidebar.regexFilter.toggle"))).toBe(true);
    await openFilter();
    expect(header.classList.contains("open")).toBe(true);
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
      expect(h.classList.contains("open")).toBe(false);
      expect(h.classList.contains("invalid")).toBe(false);
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
});
