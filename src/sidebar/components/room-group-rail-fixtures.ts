import type { AcWorkgroup, WorkgroupGroupsConfig } from "../../shared/types";
import { projectStore, type ProjectState } from "../stores/project";
import {
  defaultGroupsConfig, defaultNonStop, exactGroupRegexForWorkgroup,
} from "../stores/workgroup-groups";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { resetUiStoresForTests } from "../../shared/testing/ui-harness";
import { __setTransportForTests, type RailDiagnosticRecord } from "../../shared/ipc";
import { setLogLevel } from "../../shared/console-capture";

export function createRailProject(rooms: AcWorkgroup[]): ProjectState {
  return {
    path: String.raw`C:\Project`,
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

// #4695 — gesture diagnostics support for RoomGroupRail.gestures.test.tsx.
export function createRailRoom(name: string): AcWorkgroup {
  const roomPath = `${String.raw`C:\Project\.ac`}\\${name}`;
  const agent = {
    name: "dev-webpage-ui",
    path: `${roomPath}\\__agent_dev-webpage-ui`,
    repoPaths: [],
    isCoordinator: true,
  };
  return { name, path: roomPath, task: null, taskTitle: null, agents: [agent] };
}

export function railDiagnostics(fake: FakeTransport): RailDiagnosticRecord[] {
  return fake
    .callsFor("append_rail_diagnostic")
    .map(call => call.args.record as RailDiagnosticRecord);
}

export function railButtonOf(projectFolder: string, buttonKey: string): HTMLElement {
  const rail = document.querySelector(`[data-ac-testid="workgroupGroups.rail.${projectFolder}"]`);
  const button = rail?.querySelector<HTMLElement>(
    `[data-ac-testid="workgroupGroups.button.${buttonKey}"]`,
  );
  if (!button) throw new Error(`Missing rail button ${projectFolder}/${buttonKey}`);
  return button;
}

type RailPointerType =
  | "pointerdown" | "pointermove" | "pointerup" | "pointercancel" | "lostpointercapture"
  | "click" | "mousedown";

export function dispatchRailPointer(
  el: Element | Window,
  type: RailPointerType,
  init: { clientX?: number; clientY?: number; button?: number; pointerId?: number } = {},
): void {
  const { clientX = 10, clientY = 10, button = 0, pointerId = 1 } = init;
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, clientX, clientY, button });
  Object.defineProperty(event, "pointerId", { value: pointerId });
  Object.defineProperty(event, "pointerType", { value: "mouse" });
  Object.defineProperty(event, "isPrimary", { value: true });
  el.dispatchEvent(event);
}

export async function prepareRailDiagnostics(): Promise<{
  fake: FakeTransport; restore: () => void;
}> {
  resetUiStoresForTests();
  const fake = new FakeTransport();
  const ui = { id: "ui", name: "PRIVATE title", regex: ".*", favorite: true };
  const nonStop = { ...defaultNonStop(), show: true, favorite: true };
  fake.resolve("get_project_groups", createIndicatorGroupsConfig({ groups: [ui], nonStop }));
  fake.resolve("open_project", { path: String.raw`C:\Project` });
  const discovered = createRailProject(defaultIndicatorRooms(createRailRoom));
  fake.resolve("discover_project", { ...discovered, workgroups: [] });
  fake.resolve("append_rail_diagnostic", null);
  const restore = __setTransportForTests(fake);
  setLogLevel("debug");
  await projectStore.loadProject(String.raw`C:\Project`);
  return { fake, restore };
}
