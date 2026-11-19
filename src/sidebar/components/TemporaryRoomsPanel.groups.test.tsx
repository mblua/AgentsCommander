// @vitest-environment jsdom
// #4805 P03 - temp-room groups, Alert me! and TASK title from the agent-row menu.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TemporaryRoomsPanel from "./TemporaryRoomsPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  contextMenu,
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { TEMP_ROOMS_GROUPS_KEY as KEY, type TempRoomInfo } from "../../shared/temp-rooms";
import type { WorkgroupGroupsConfig } from "../../shared/types";
import { tempRoomsStore } from "../stores/temp-rooms";
import { toastStore } from "../../shared/stores/toasts";
import {
  defaultGroupsConfig,
  defaultNonStop,
  exactGroupRegexForWorkgroup,
  workgroupGroupsStore,
} from "../stores/workgroup-groups";

const ROOM_1: TempRoomInfo = {
  name: "temp-room-tr01",
  path: "/cfg/temp-room-tr01",
  unleashed: false,
  taskTitle: null,
};
const ROOM_2: TempRoomInfo = {
  name: "temp-room-tr02",
  path: "/cfg/temp-room-tr02",
  unleashed: false,
  taskTitle: "Other",
};
const TOKEN = exactGroupRegexForWorkgroup(ROOM_1.name);
const P = `tempRoom.${ROOM_1.name}`;

interface Setup {
  rooms?: TempRoomInfo[];
  config?: WorkgroupGroupsConfig;
  failGroupUpdate?: boolean;
}

function setup(opts: Setup = {}) {
  const fake = new FakeTransport();
  let listed = [...(opts.rooms ?? [ROOM_1])];
  const state = { config: opts.config ?? defaultGroupsConfig() };
  fake.onInvoke("get_settings", () => ({ ...baseSettings(), tempRoomAgent: undefined }));
  fake.onInvoke("list_temp_rooms", () => listed);
  fake.onInvoke("get_temp_room_groups", () => state.config);
  fake.onInvoke("update_temp_room_groups", (args) => {
    if (opts.failGroupUpdate) throw new Error("disk denied");
    state.config = args.config as WorkgroupGroupsConfig;
    return state.config;
  });
  fake.onInvoke("task_set_title_at", (args) => {
    listed = listed.map((r) =>
      r.path === args.workgroupRoot ? { ...r, taskTitle: String(args.title) } : r,
    );
    return { workgroupRoot: String(args.workgroupRoot), task: null, taskTitle: args.title };
  });
  fake.onInvoke("task_clean_at", (args) => {
    listed = listed.map((r) => (r.path === args.workgroupRoot ? { ...r, taskTitle: "Clean" } : r));
    return { workgroupRoot: String(args.workgroupRoot), task: null, taskTitle: "Clean" };
  });
  fake.onInvoke("create_temp_room", () => {
    const room = { ...ROOM_1 };
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
  fake.resolve("switch_session", null);
  const rendered = renderWithFakeTransport(() => <TemporaryRoomsPanel />, fake);
  return { fake, rendered, state };
}

const byId = (id: string) => document.querySelector<HTMLElement>(`[data-ac-testid="${id}"]`);
const order = (fake: FakeTransport) => fake.calls.map((c) => c.cmd);
const lastGroups = (fake: FakeTransport) =>
  fake.lastCall("update_temp_room_groups")!.args.config as WorkgroupGroupsConfig;

async function openAgentMenu(room = ROOM_1) {
  await waitFor(() => expect(byId(`tempRoom.${room.name}.row`)).not.toBeNull());
  contextMenu(byId(`tempRoom.${room.name}.row`)!);
  await waitFor(() => expect(byId(`tempRoom.${room.name}.menu`)).not.toBeNull());
}

async function openGroups() {
  await openAgentMenu();
  click(byId(`${P}.groups.trigger`)!);
  await waitFor(() => expect(byId(`${P}.groups.flyout`)).not.toBeNull());
  return byId(`${P}.groups.flyout`)!;
}

async function openDeleteModal() {
  await waitFor(() => expect(byId(`${P}.header`)).not.toBeNull());
  contextMenu(byId(`${P}.header`)!);
  await waitFor(() => expect(byId("tempRoom.headerMenu.delete")).not.toBeNull());
  click(byId("tempRoom.headerMenu.delete")!);
  await waitFor(() => expect(byId("tempRoom.deleteModal.confirm")).not.toBeNull());
}

const enter = (el: HTMLElement) =>
  el.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));

describe("#4805 TemporaryRoomsPanel groups and TASK title", () => {
  let cleanupDom: (() => void) | null = null;
  let cleanups: (() => void)[] = [];
  const track = <T extends { rendered: { cleanup: () => void } }>(r: T): T => {
    cleanups.push(r.rendered.cleanup);
    return r;
  };
  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    tempRoomsStore.resetForTests();
    resetUiStoresForTests();
  });
  afterEach(() => {
    for (const c of cleanups) c();
    cleanups = [];
    cleanupDom?.();
    cleanupDom = null;
    tempRoomsStore.resetForTests();
    resetUiStoresForTests();
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  const withGroup = (regex = "(?!)"): WorkgroupGroupsConfig => ({
    ...defaultGroupsConfig(),
    groups: [{ id: "g", name: "G", regex }],
  });

  it("Add to Group lists Alert me! then the groups; toggling adds and removes the token", async () => {
    const { fake } = track(setup({ config: withGroup() }));
    const flyout = await openGroups();
    const names = Array.from(flyout.querySelectorAll("button")).map((b) => b.textContent ?? "");
    expect(names[0]).toContain("Alert me!");
    expect(names[1]).toContain("G");
    click(byId(`${P}.groups.g`)!);
    await waitFor(() => expect(fake.callsFor("update_temp_room_groups")).toHaveLength(1));
    expect(lastGroups(fake).groups[0]!.regex).toBe(TOKEN);
    await waitFor(() => expect(byId(`${P}.groups.g`)!.textContent).toContain("✓"));
    click(byId(`${P}.groups.g`)!);
    await waitFor(() => expect(fake.callsFor("update_temp_room_groups")).toHaveLength(2));
    expect(lastGroups(fake).groups[0]!.regex).not.toContain(ROOM_1.name);
  });

  it("Alert me! adds the room to a shown Non-stop group", async () => {
    const { fake } = track(setup());
    await openGroups();
    click(byId(`${P}.groups.nonstop`)!);
    await waitFor(() => expect(fake.callsFor("update_temp_room_groups")).toHaveLength(1));
    expect(lastGroups(fake).nonStop).toMatchObject({ show: true, regex: TOKEN });
  });

  it("Create new group sends a group named as typed; a blank name shows the error", async () => {
    const { fake } = track(setup());
    await openGroups();
    click(byId(`${P}.groups.create`)!);
    await waitFor(() => expect(byId("tempRoom.groups.create.input")).not.toBeNull());
    click(byId("tempRoom.groups.create.save")!);
    await waitFor(() =>
      expect(byId("tempRoom.groups.error")!.textContent).toContain("Group name cannot be blank."),
    );
    expect(fake.callsFor("update_temp_room_groups")).toHaveLength(0);
    input(byId("tempRoom.groups.create.input") as HTMLInputElement, "Mine");
    click(byId("tempRoom.groups.create.save")!);
    await waitFor(() => expect(fake.callsFor("update_temp_room_groups")).toHaveLength(1));
    expect(lastGroups(fake).groups[0]).toMatchObject({ name: "Mine", regex: TOKEN });
  });

  it("Edit TASK title saves T, lists again and shows it in the header; blank is refused", async () => {
    const { fake } = track(setup());
    await openAgentMenu();
    click(byId(`${P}.menu.editTaskTitle`)!);
    const field = () => document.querySelector<HTMLInputElement>(".session-context-title-input")!;
    await waitFor(() => expect(field()).not.toBeNull());
    enter(field());
    await waitFor(() =>
      expect(document.body.textContent).toContain("Title cannot be empty."),
    );
    expect(fake.callsFor("task_set_title_at")).toHaveLength(0);
    input(field(), "T");
    enter(field());
    await waitFor(() => expect(byId(`${P}.taskTitle`)?.textContent).toBe("T"));
    expect(fake.lastCall("task_set_title_at")!.args).toEqual({
      workgroupRoot: ROOM_1.path,
      title: "T",
    });
    const cmds = order(fake);
    expect(cmds.lastIndexOf("list_temp_rooms")).toBeGreaterThan(cmds.indexOf("task_set_title_at"));
  });

  it("Clear task title sends task_clean_at, lists again and shows Clean", async () => {
    const { fake } = track(setup());
    await openAgentMenu();
    click(byId(`${P}.menu.clearTaskTitle`)!);
    await waitFor(() => expect(byId(`${P}.taskTitle`)?.textContent).toBe("Clean"));
    expect(byId(`${P}.taskTitle`)!.getAttribute("data-ac-state")).toBe("clean");
    expect(fake.lastCall("task_clean_at")!.args).toEqual({ workgroupRoot: ROOM_1.path });
    const cmds = order(fake);
    expect(cmds.lastIndexOf("list_temp_rooms")).toBeGreaterThan(cmds.indexOf("task_clean_at"));
  });

  it("delete drops the room from its group and Alert me!", async () => {
    const config: WorkgroupGroupsConfig = {
      ...withGroup(TOKEN),
      nonStop: { ...defaultNonStop(), show: true, regex: TOKEN },
    };
    const { fake } = track(setup({ config }));
    await openDeleteModal();
    click(byId("tempRoom.deleteModal.confirm")!);
    await waitFor(() => expect(fake.callsFor("update_temp_room_groups")).toHaveLength(1));
    const saved = lastGroups(fake);
    expect(saved.groups[0]!.regex).not.toContain(ROOM_1.name);
    expect(saved.nonStop!.regex).not.toContain(ROOM_1.name);
    const cmds = order(fake);
    expect(cmds.indexOf("update_temp_room_groups")).toBeGreaterThan(cmds.indexOf("delete_temp_room"));
  });

  it("delete of a room in no group sends no update", async () => {
    const { fake } = track(setup({ config: withGroup() }));
    await openDeleteModal();
    click(byId("tempRoom.deleteModal.confirm")!);
    await waitFor(() => expect(byId(`${P}.row`)).toBeNull());
    expect(fake.callsFor("update_temp_room_groups")).toHaveLength(0);
  });

  it("create reusing tr01 removes its stored token before create_session", async () => {
    const { fake } = track(
      setup({ rooms: [], config: withGroup(TOKEN) }),
    );
    await waitFor(() => expect(byId("tempRooms.header")).not.toBeNull());
    contextMenu(byId("tempRooms.header")!);
    await waitFor(() => expect(byId("tempRooms.menu.create")).not.toBeNull());
    click(byId("tempRooms.menu.create")!);
    await waitFor(() => expect(fake.callsFor("update_temp_room_groups")).toHaveLength(1));
    expect(lastGroups(fake).groups[0]!.regex).not.toContain(ROOM_1.name);
    const cmds = order(fake);
    const create = cmds.indexOf("create_session");
    expect(create === -1 || cmds.indexOf("update_temp_room_groups") < create).toBe(true);
  });

  it("a failed cleanup still deletes the row and shows a toast", async () => {
    const error = vi.spyOn(toastStore, "error");
    const { fake } = track(setup({ config: withGroup(TOKEN), failGroupUpdate: true }));
    await openDeleteModal();
    click(byId("tempRoom.deleteModal.confirm")!);
    await waitFor(() => expect(byId(`${P}.row`)).toBeNull());
    expect(fake.callsFor("delete_temp_room")).toHaveLength(1);
    expect(error).toHaveBeenCalledWith(
      expect.stringContaining("Group entries of temp-room-tr01 could not be removed: "),
    );
  });

  it("the rail selection filters the rows", async () => {
    track(setup({ rooms: [ROOM_1, ROOM_2], config: withGroup(TOKEN) }));
    await waitFor(() => expect(byId(`tempRoom.${ROOM_2.name}.row`)).not.toBeNull());
    await workgroupGroupsStore.ensureLoaded(KEY);
    workgroupGroupsStore.select(KEY, { kind: "group", id: "g" });
    await waitFor(() => expect(byId(`tempRoom.${ROOM_2.name}.row`)).toBeNull());
    expect(byId(`${P}.row`)).not.toBeNull();
    workgroupGroupsStore.select(KEY, { kind: "all" });
    await waitFor(() => expect(byId(`tempRoom.${ROOM_2.name}.row`)).not.toBeNull());
  });
});
