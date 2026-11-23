// The temp-room list, shared by the panel, the group rail and the Non-stop
// watchdog. `railProjects()` is the Temporary Rooms group set seen as one project.
import { createSignal } from "solid-js";
import { TempRoomAPI } from "../../shared/ipc";
import { TEMP_ROOMS_GROUPS_KEY, type TempRoomInfo } from "../../shared/temp-rooms";
import type { AcWorkgroup } from "../../shared/types";
import type { ProjectState } from "./project-merge";

export const TEMP_ROOMS_LABEL = "Temporary Rooms";

/** A temp room as a one-agent room; its only session is named after the folder. */
export function tempRoomWorkgroup(room: TempRoomInfo): AcWorkgroup {
  return {
    name: room.name,
    path: room.path,
    task: null,
    taskTitle: room.taskTitle,
    agents: [
      {
        name: room.name,
        path: room.path,
        repoPaths: [],
        isCoordinator: false,
        sessionName: room.name,
      },
    ],
  };
}

function railProjectOf(rooms: TempRoomInfo[]): ProjectState[] {
  if (rooms.length === 0) return [];
  return [
    {
      path: TEMP_ROOMS_GROUPS_KEY,
      folderName: TEMP_ROOMS_LABEL,
      workgroups: rooms.map(tempRoomWorkgroup),
      agents: [],
      teams: [],
      loops: [],
      contextTemplateUpdates: [],
    },
  ];
}

const [rooms, setRooms] = createSignal<TempRoomInfo[]>([]);
const [railProjects, setRailProjects] = createSignal<ProjectState[]>([]);
let loaded: Promise<void> | null = null;

export const tempRoomsStore = {
  rooms,
  /** Zero or one entry; built once per refresh so its identity is stable. */
  railProjects,
  async refresh(): Promise<void> {
    const next = await TempRoomAPI.list();
    setRooms(next);
    setRailProjects(railProjectOf(next));
  },
  /** First load only, never retried (a failure logs once); the panel's `refresh()`
   *  always lists again. */
  ensureLoaded(): Promise<void> {
    loaded ??= this.refresh().catch((e) => {
      console.error("[temp-rooms] list_temp_rooms failed:", e);
    });
    return loaded;
  },
  resetForTests(): void {
    setRooms([]);
    setRailProjects([]);
    loaded = null;
  },
};
