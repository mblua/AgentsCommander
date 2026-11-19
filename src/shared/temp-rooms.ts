// #4779 - Temporary Rooms: the IPC contract and the saved temp-room agent choice.
import {
  AC_MATRIX_ROOT_PLACEHOLDER,
  AC_WORKSPACE_ROOT_PLACEHOLDER,
  resolveProfilePreview,
} from "./profile-utils";
import type { Transport } from "./transport";
import type { SettingsSnapshot } from "./types";

/** One `temp-room-trNN` folder in the config dir, as `list_temp_rooms` returns it. */
export interface TempRoomInfo {
  name: string;
  path: string;
  /** #4780: the stored Unleash flag; the store override wins. */
  unleashed: boolean;
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

/** #4786 - placeholders that do not exist for a temp room (no workspace, no matrix). */
const TEMP_ROOM_UNAVAILABLE_PLACEHOLDERS = [
  AC_WORKSPACE_ROOT_PLACEHOLDER,
  AC_MATRIX_ROOT_PLACEHOLDER,
] as const;

/** #4786 - unavailable placeholders the chosen agent's launch values use, fixed order. */
export function tempRoomUnavailablePlaceholders(
  settings: SettingsSnapshot,
  choice: TempRoomAgentChoice | undefined,
): string[] {
  if (!choice) return [];
  const agent = settings.agents.find((a) => a.id === choice.agentId);
  if (!agent) return [];
  const profiles = settings.codingAgentProfiles;
  const letter = resolveProfilePreview(profiles, agent.id, choice.requestedProfile)
    .effectiveProfile;
  const cell = profiles.profilesByAgent[agent.id]?.[letter];
  const values = [
    agent.command,
    agent.preflight ?? "",
    ...agent.envs.filter((e) => e.enabled).map((e) => e.value),
  ];
  if (cell?.enabled) {
    values.push(cell.command, cell.preflight ?? "", ...Object.values(cell.env));
  }
  return TEMP_ROOM_UNAVAILABLE_PLACEHOLDERS.filter((t) => values.some((v) => v.includes(t)));
}

/** #4786 - the red warning line for one unavailable placeholder. */
export function tempRoomPlaceholderWarning(token: string): string {
  return `${token} does not apply to Temporary Rooms: it does not exist for them.`;
}
