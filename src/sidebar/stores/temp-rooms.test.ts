// #4805 - the temp-room list store and its one-project rail projection.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { __setTransportForTests } from "../../shared/ipc";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { resetUiStoresForTests, session } from "../../shared/testing/ui-harness";
import type { TempRoomInfo } from "../../shared/temp-rooms";
import type { AcWorkgroup } from "../../shared/types";
import { findReplicaSession } from "../components/room-session";
import { sessionsStore } from "./sessions";
import { tempRoomsStore } from "./temp-rooms";

const room = (n: string, taskTitle: string | null = null): TempRoomInfo => ({
  name: `temp-room-tr${n}`,
  path: `/cfg/temp-rooms/temp-room-tr${n}`,
  unleashed: false,
  taskTitle,
});

describe("#4805 tempRoomsStore", () => {
  let fake: FakeTransport;
  let restore: () => void;

  beforeEach(() => {
    fake = new FakeTransport();
    restore = __setTransportForTests(fake);
    tempRoomsStore.resetForTests();
  });

  afterEach(() => {
    tempRoomsStore.resetForTests();
    resetUiStoresForTests();
    restore();
    vi.restoreAllMocks();
  });

  it("refresh() sets rooms and one rail project", async () => {
    const list = [room("01", "Fix login"), room("02")];
    fake.resolve("list_temp_rooms", list);
    await tempRoomsStore.refresh();
    expect(tempRoomsStore.rooms()).toEqual(list);
    const projects = tempRoomsStore.railProjects();
    expect(projects).toHaveLength(1);
    expect(projects[0].path).toBe("agentscommander://temp-rooms");
    expect(projects[0].folderName).toBe("Temporary Rooms");
    expect(projects[0].workgroups).toHaveLength(2);
    projects[0].workgroups.forEach((wg, i) => {
      expect(wg.agents).toHaveLength(1);
      expect(wg.agents[0].sessionName).toBe(list[i].name);
      expect(wg.taskTitle).toBe(list[i].taskTitle);
    });
  });

  it("zero rooms gives no rail project", async () => {
    fake.resolve("list_temp_rooms", []);
    await tempRoomsStore.refresh();
    expect(tempRoomsStore.railProjects()).toEqual([]);
  });

  it("railProjects() keeps its identity until the next refresh()", async () => {
    fake.resolve("list_temp_rooms", [room("01")]);
    await tempRoomsStore.refresh();
    const first = tempRoomsStore.railProjects();
    expect(tempRoomsStore.railProjects()).toBe(first);
    await tempRoomsStore.refresh();
    expect(tempRoomsStore.railProjects()).not.toBe(first);
  });

  it("findReplicaSession uses sessionName for temp rooms only", async () => {
    fake.resolve("list_temp_rooms", [room("01")]);
    await tempRoomsStore.refresh();
    const wg = tempRoomsStore.railProjects()[0].workgroups[0];
    sessionsStore.setSessions([
      session({ id: "t", name: "temp-room-tr01", workingDirectory: wg.path }),
    ]);
    expect(findReplicaSession(wg, wg.agents[0])?.id).toBe("t");

    const projectRoom: AcWorkgroup = {
      name: "wg-1-dev-team",
      path: "/p/.ac/wg-1-dev-team",
      task: null,
      agents: [{ name: "architect", path: "/p/.ac/wg-1-dev-team/__agent_architect",
        repoPaths: [], isCoordinator: false }],
    };
    const replicaPath = projectRoom.agents[0].path;
    sessionsStore.setSessions([session({ id: "bare", name: "architect", workingDirectory: replicaPath })]);
    expect(findReplicaSession(projectRoom, projectRoom.agents[0])).toBeUndefined();
    sessionsStore.setSessions([
      session({ id: "full", name: "wg-1-dev-team/architect", workingDirectory: replicaPath }),
    ]);
    expect(findReplicaSession(projectRoom, projectRoom.agents[0])?.id).toBe("full");
  });

  it("ensureLoaded() lists once and never retries after a failure", async () => {
    fake.resolve("list_temp_rooms", []);
    await tempRoomsStore.ensureLoaded();
    await tempRoomsStore.ensureLoaded();
    expect(fake.callsFor("list_temp_rooms")).toHaveLength(1);

    tempRoomsStore.resetForTests();
    vi.spyOn(console, "error").mockImplementation(() => {});
    fake.reject("list_temp_rooms", "boom");
    await tempRoomsStore.ensureLoaded();
    await tempRoomsStore.ensureLoaded();
    expect(fake.callsFor("list_temp_rooms")).toHaveLength(2);
    fake.resolve("list_temp_rooms", [room("01")]);
    await tempRoomsStore.refresh();
    expect(fake.callsFor("list_temp_rooms")).toHaveLength(3);
  });
});
