// @vitest-environment jsdom
// The Temporary Rooms group set as its own rail section.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkgroupGroupsConfig } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  contextMenu,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { TEMP_ROOMS_GROUPS_KEY as KEY, type TempRoomInfo } from "../../shared/temp-rooms";
import { projectCollapseStore } from "../stores/project-collapse";
import { tempRoomsStore } from "../stores/temp-rooms";
import {
  defaultGroupsConfig,
  defaultNonStop,
  exactGroupRegexForWorkgroup,
  workgroupGroupsStore,
} from "../stores/workgroup-groups";
import {
  createIndicatorGroupsConfig,
  createRailProject,
  createRailRoom,
  defaultIndicatorRooms,
  railButtonOf,
} from "./room-group-rail-fixtures";
import RoomGroupRail from "./RoomGroupRail";

const TEMP = "Temporary Rooms";
const ROOM_1: TempRoomInfo = { name: "temp-room-tr01", path: "/cfg/temp-room-tr01", unleashed: false };

function tempConfig(overrides: Partial<WorkgroupGroupsConfig> = {}): WorkgroupGroupsConfig {
  return {
    ...defaultGroupsConfig(),
    groups: [{ id: "scratch", name: "Scratch", regex: exactGroupRegexForWorkgroup(ROOM_1.name) }],
    ...overrides,
  };
}

const target = (id: string): HTMLElement => {
  const el = document.querySelector<HTMLElement>(`[data-ac-testid="${id}"]`);
  if (!el) throw new Error(`Missing element ${id}`);
  return el;
};

async function openMenuOn(el: Element): Promise<void> {
  contextMenu(el);
  await waitFor(() => expect(target("workgroupGroups.contextMenu")).toBeTruthy());
}

async function mount(temp: WorkgroupGroupsConfig, rooms: TempRoomInfo[] = [ROOM_1]) {
  const fake = new FakeTransport();
  fake.resolve("get_project_groups", createIndicatorGroupsConfig());
  fake.onInvoke("update_project_groups", (args) => args.config);
  fake.resolve("get_temp_room_groups", temp);
  fake.onInvoke("update_temp_room_groups", (args) => args.config);
  fake.resolve("list_temp_rooms", rooms);
  fake.resolve("set_rail_collapse", null);
  fake.resolve("list_archived_projects", []);
  fake.resolve("get_settings", baseSettings());
  const projectA = createRailProject(defaultIndicatorRooms(createRailRoom));
  const rendered = renderWithFakeTransport(
    () => <RoomGroupRail projects={[projectA, ...tempRoomsStore.railProjects()]} />,
    fake,
  );
  await tempRoomsStore.refresh();
  return { fake, rendered };
}

describe("RoomGroupRail - Temporary Rooms section", () => {
  let cleanup: (() => void) | null = null;
  beforeEach(() => {
    resetUiStoresForTests();
    tempRoomsStore.resetForTests();
  });
  afterEach(() => {
    cleanup?.();
    cleanup = null;
    tempRoomsStore.resetForTests();
    resetUiStoresForTests();
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  it("renders the temp section after the project's, with All, its groups and Alert me!", async () => {
    const { rendered } = await mount(tempConfig({ nonStop: { ...defaultNonStop(), show: true } }));
    cleanup = rendered.cleanup;
    await waitFor(() => expect(railButtonOf(TEMP, "scratch")).toBeTruthy());
    const sections = Array.from(
      document.querySelectorAll<HTMLElement>('[data-ac-testid^="workgroupGroups.rail."]'),
    ).map((el) => el.dataset.acTestid);
    const project = sections.indexOf("workgroupGroups.rail.Project");
    const temp = sections.indexOf(`workgroupGroups.rail.${TEMP}`);
    expect(project).toBeGreaterThanOrEqual(0);
    expect(temp).toBeGreaterThan(project);
    expect(railButtonOf(TEMP, "all")).toBeTruthy();
    expect(railButtonOf(TEMP, "nonstop")).toBeTruthy();
  });

  it("no temp rooms: no temp section", async () => {
    const { rendered } = await mount(tempConfig(), []);
    cleanup = rendered.cleanup;
    await waitFor(() => expect(target("workgroupGroups.rail.Project")).toBeTruthy());
    expect(document.querySelector(`[data-ac-testid="workgroupGroups.rail.${TEMP}"]`)).toBeNull();
  });

  it("a favorite temp group shows in Favorites; Unfavorite saves the temp set only", async () => {
    const fav = tempConfig({
      groups: [
        {
          id: "scratch",
          name: "Scratch",
          regex: exactGroupRegexForWorkgroup(ROOM_1.name),
          favorite: true,
        },
      ],
    });
    const { fake, rendered } = await mount(fav);
    cleanup = rendered.cleanup;
    await waitFor(() =>
      expect(target("workgroupGroups.favorites").textContent).toContain("Scratch"),
    );
    await openMenuOn(railButtonOf(TEMP, "scratch"));
    click(target("workgroupGroups.contextMenu.favorite"));
    await waitFor(() => expect(fake.callsFor("update_temp_room_groups")).toHaveLength(1));
    const saved = fake.lastCall("update_temp_room_groups")!.args.config as WorkgroupGroupsConfig;
    expect(saved.groups[0]!.favorite).toBe(false);
    expect(fake.callsFor("update_project_groups")).toHaveLength(0);
  });

  it("a temp group click selects it and expands the panel", async () => {
    const { rendered } = await mount(tempConfig());
    cleanup = rendered.cleanup;
    await waitFor(() => expect(railButtonOf(TEMP, "scratch")).toBeTruthy());
    projectCollapseStore.setProjectCollapsed(KEY, true);
    click(railButtonOf(TEMP, "scratch"));
    expect(workgroupGroupsStore.selection(KEY)).toEqual({ kind: "group", id: "scratch" });
    expect(projectCollapseStore.isProjectCollapsed(KEY)).toBe(false);
  });

  it("Edit opens the modal for Temporary Rooms; Save sends update_temp_room_groups", async () => {
    const { fake, rendered } = await mount(tempConfig());
    cleanup = rendered.cleanup;
    await waitFor(() => expect(railButtonOf(TEMP, "scratch")).toBeTruthy());
    await openMenuOn(railButtonOf(TEMP, "scratch"));
    click(target("workgroupGroups.contextMenu.edit"));
    await waitFor(() => expect(target("workgroupGroups.modal").textContent).toContain(TEMP));
    click(target("workgroupGroups.save"));
    await waitFor(() => expect(fake.callsFor("update_temp_room_groups")).toHaveLength(1));
    expect(fake.callsFor("update_project_groups")).toHaveLength(0);
  });
});
