import type { AcWorkgroup, WorkgroupGroupsConfig } from "../../shared/types";
import type { ProjectState } from "../stores/project";
import { defaultGroupsConfig, exactGroupRegexForWorkgroup } from "../stores/workgroup-groups";

export function createRailProject(rooms: AcWorkgroup[]): ProjectState {
  return {
    path: "C:\\Project",
    folderName: "Project",
    workgroups: rooms,
    agents: [],
    teams: [],
    loops: [],
    contextTemplateUpdates: [],
  };
}

/** ui -> wg-1-dev-team, rust -> wg-2-rust-team; wg-3-docs-team stays ungrouped. */
export function createIndicatorGroupsConfig(overrides: Partial<WorkgroupGroupsConfig> = {}): WorkgroupGroupsConfig {
  return {
    ...defaultGroupsConfig(),
    ...overrides,
    groups:
      overrides.groups ?? [
        { id: "ui", name: "UI", regex: exactGroupRegexForWorkgroup("wg-1-dev-team") },
        { id: "rust", name: "Rust", regex: exactGroupRegexForWorkgroup("wg-2-rust-team") },
      ],
  };
}

export function defaultIndicatorRooms(createRoom: (name: string) => AcWorkgroup): AcWorkgroup[] {
  return [createRoom("wg-1-dev-team"), createRoom("wg-2-rust-team"), createRoom("wg-3-docs-team")];
}
