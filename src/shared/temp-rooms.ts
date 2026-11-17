// #4779 - Temporary Rooms: the IPC contract and the saved temp-room agent choice.
import type { Transport } from "./transport";
import type { SettingsSnapshot } from "./types";

/** One `temp-room-trNN` folder in the config dir, as `list_temp_rooms` returns it. */
export interface TempRoomInfo {
  name: string;
  path: string;
}

/** The agent every new temp room launches with; absent until the first choice. */
export interface TempRoomAgentChoice {
  agentId: string;
  requestedProfile: string | null;
}

/** Settings as the renderer reads and writes them, plus the temp-room choice. */
export type SettingsWithTempRoom = SettingsSnapshot & { tempRoomAgent?: TempRoomAgentChoice };

/** #4779 - temp-room commands over an injected transport (Tauri or WS). */
export function createTempRoomAPI(transport: Pick<Transport, "invoke">) {
  return {
    list: () => transport.invoke<TempRoomInfo[]>("list_temp_rooms"),
    create: () => transport.invoke<TempRoomInfo>("create_temp_room"),
    delete: (name: string) => transport.invoke<void>("delete_temp_room", { name }),
  };
}
