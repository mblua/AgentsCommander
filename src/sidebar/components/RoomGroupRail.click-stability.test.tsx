// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { afterEach, expect, it, vi } from "vitest";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { click, renderWithFakeTransport, resetUiStoresForTests, waitFor } from "../../shared/testing/ui-harness";
import { setLogLevel } from "../../shared/console-capture";
import type { RailDiagnosticRecord } from "../../shared/ipc";
import { defaultNonStop, workgroupGroupsStore } from "../stores/workgroup-groups";
import { railCollapseStore } from "../stores/rail-collapse";
import { createIndicatorGroupsConfig, createRailProject } from "./room-group-rail-fixtures";
import { remoteActivityStore } from "../stores/remote-activity";
import RoomGroupRail from "./RoomGroupRail";

afterEach(() => { setLogLevel("info"); resetUiStoresForTests(); vi.restoreAllMocks(); });
function pointer(node: Element | Window, type: string) {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, clientY: 10 });
  Object.defineProperties(event, { pointerId: { value: 1 }, isPrimary: { value: true } });
  node.dispatchEvent(event);
}
function target(root: ParentNode, id: string): HTMLElement {
  const node = root.querySelector<HTMLElement>(`[data-ac-testid="${id}"]`);
  expect(node, id).toBeTruthy();
  return node!;
}
function config() {
  return createIndicatorGroupsConfig({ groups: [
    { id: "ui", name: "UI", regex: ".*", favorite: true },
    { id: "nonstop", name: "Custom nonstop", regex: ".*", favorite: true },
  ], nonStop: { ...defaultNonStop(), show: true, favorite: true, regex: ".*" } });
}
async function setup() {
  resetUiStoresForTests();
  const first = createRailProject([]);
  const second = { ...createRailProject([]), path: "C:\Other", folderName: "Other" };
  const fake = new FakeTransport();
  fake.resolve("get_project_groups", config());
  fake.resolve("append_rail_diagnostic", null);
  workgroupGroupsStore.applyExternalUpdate(first.path, config());
  workgroupGroupsStore.applyExternalUpdate(second.path, config());
  setLogLevel("debug");
  const [projects, setProjects] = createSignal([first, second]);
  const rendered = renderWithFakeTransport(() => <RoomGroupRail projects={projects()} />, fake);
  await waitFor(() => expect(target(rendered.root, "workgroupGroups.button.ui")).toBeTruthy());
  const records = () => fake.callsFor("append_rail_diagnostic").map(call => call.args.record as RailDiagnosticRecord);
  return { ...rendered, first, second, projects, setProjects, records };
}

it.each(["favorite", "project"] as const)("preserves %s custom and Non-stop press nodes with current presentation and one synthetic selection", async site => {
  const s = await setup();
  try {
    for (const kind of ["group", "nonstop"] as const) {
      workgroupGroupsStore.select(s.first.path, { kind: "all" });
      const section = target(s.root, "workgroupGroups.rail.Project");
      const favoriteId = kind === "group" ? "workgroupGroups.favoriteButton.Project.ui" : "workgroupGroups.favoriteNonStopButton.Project";
      const projectId = kind === "group" ? "workgroupGroups.button.ui" : "workgroupGroups.button.nonstop";
      // The custom 'nonstop' intentionally shares the historical project testid;
      // actual Non-stop comes first. Identity must use selection kind as well.
      const node = site === "favorite" ? target(s.root, favoriteId) : target(section, projectId);
      const otherCopy = site === "favorite" ? target(section, projectId) : target(s.root, favoriteId);
      const otherProject = target(target(s.root, "workgroupGroups.rail.Other"), "workgroupGroups.button.ui");
      const select = vi.spyOn(workgroupGroupsStore, "select");
      s.fake.clearCalls();
      pointer(node, "pointerdown");
      node.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
      const press = s.records().find(record => record.event === "pointerdown")!;
      s.setProjects(s.projects().map(project => ({ ...project })));
      workgroupGroupsStore.applyExternalUpdate(s.first.path, structuredClone(config()));
      await waitFor(() => expect(s.records().some(record => record.event === "project-buttons")).toBe(true));
      // This assertion must fail on unfixed production BEFORE any click.
      expect(node.isConnected, "pressed DOM node survives equivalent refresh before synthetic click").toBe(true);
      expect(site === "favorite" ? target(s.root, favoriteId) : target(section, projectId)).toBe(node);
      expect(target(s.root, "workgroupGroups.rail.Project")).toBe(section);
      expect(otherCopy.isConnected).toBe(true);
      expect(otherCopy).not.toBe(node);
      expect(target(target(s.root, "workgroupGroups.rail.Other"), "workgroupGroups.button.ui")).toBe(otherProject);
      const changed = config();
      changed.groups[0].name = "Renamed UI";
      changed.nonStop!.name = "Renamed nonstop";
      s.setProjects(s.projects().map(project => project.path === s.first.path ? {
        ...project, folderName: "Current", workgroups: [{ name: "wg-1-team", path: "C:\room", task: null, taskTitle: null, agents: [{ name: "dev", path: "C:\replica", repoPaths: ["C:\repo"], isCoordinator: true }] }],
      } : project));
      remoteActivityStore.applyRemoteActivityUpdate({ repoPaths: ["C:\repo"], ciStates: ["running"], stalenessStates: ["current"], behindBy: [null] });
      workgroupGroupsStore.applyExternalUpdate(s.first.path, changed);
      await waitFor(() => expect(node.textContent).toContain(kind === "group" ? "Renamed UI" : "Renamed nonstop"));
      expect(node.isConnected).toBe(true);
      expect(node.textContent).toContain("1/1");
      expect(node.title).toContain("Current");
      expect(node.title).toContain("CI");
      expect(node.querySelector(".workgroup-group-rail-dot")).toBeTruthy();
      if (site === "favorite") expect(node.dataset.acTestid).toContain("Current");
      pointer(node, "pointerup");
      const release = s.records().find(record => record.event === "pointerup")!;
      expect(release).toMatchObject({ originalNodeId: press.nodeId, originalConnected: true });
      expect(s.records().some(record => record.event === "node-dispose" && record.nodeId === press.nodeId)).toBe(false);
      click(node); // Synthetic handler proof, never a native Windows click claim.
      expect(select).toHaveBeenCalledExactlyOnceWith(s.first.path, kind === "group" ? { kind, id: "ui" } : { kind });
      expect(node.getAttribute("aria-pressed")).toBe("true");
      expect(otherCopy.getAttribute("aria-pressed")).toBe("true");
      select.mockRestore();
      s.setProjects([s.first, s.second]);
      workgroupGroupsStore.applyExternalUpdate(s.first.path, config());
    }
  } finally { s.cleanup(); }
});

it("moves surviving rows, separates colliding IDs and cancels removal while armed without stale selection or save", async () => {
  const s = await setup();
  try {
    const section = target(s.root, "workgroupGroups.rail.Project");
    const ui = target(section, "workgroupGroups.button.ui");
    const colliding = [...section.querySelectorAll<HTMLElement>('[data-ac-testid="workgroupGroups.button.nonstop"]')];
    expect(colliding).toHaveLength(2);
    const favorite = target(s.root, "workgroupGroups.favoriteButton.Project.ui");
    const other = target(target(s.root, "workgroupGroups.rail.Other"), "workgroupGroups.button.ui");
    const reordered = config();
    reordered.groups.reverse();
    reordered.groups.push({ id: "extra", name: "Extra", regex: "^$", favorite: false });
    workgroupGroupsStore.applyExternalUpdate(s.first.path, reordered);
    expect(target(section, "workgroupGroups.button.ui")).toBe(ui);
    expect([...section.querySelectorAll('[data-ac-testid="workgroupGroups.button.nonstop"]')]).toEqual(colliding);
    const names = [...section.querySelectorAll(".workgroup-group-rail-title")].map(n => n.textContent);
    expect(names.indexOf("Custom nonstop")).toBeLessThan(names.indexOf("UI"));
    pointer(ui, "pointerdown");
    expect(ui.classList.contains("reorder-arming")).toBe(true);
    const removed = { ...reordered, groups: reordered.groups.filter(group => group.id !== "ui") };
    workgroupGroupsStore.applyExternalUpdate(s.first.path, removed);
    expect(ui.isConnected).toBe(false);
    workgroupGroupsStore.applyExternalUpdate(s.first.path, reordered);
    const replacement = target(section, "workgroupGroups.button.ui");
    expect(replacement).not.toBe(ui);
    expect(replacement.classList.contains("reorder-arming")).toBe(false);
    const select = vi.spyOn(workgroupGroupsStore, "select");
    s.fake.clearCalls();
    pointer(window, "pointerup");
    click(ui); // Detached old handler must not select the re-added logical ID.
    expect(select).not.toHaveBeenCalled();
    expect(s.fake.callsFor("update_project_groups")).toHaveLength(0);
    expect(other.isConnected).toBe(true);
    expect(favorite.isConnected).toBe(false);
    const freshFavorite = target(s.root, "workgroupGroups.favoriteButton.Project.ui");
    railCollapseStore.toggleProjectCollapsed(s.first.path);
    expect(replacement.isConnected).toBe(false);
    expect(freshFavorite.isConnected).toBe(true);
    expect(other.isConnected).toBe(true);
    s.setProjects([s.second]);
    expect(other.isConnected).toBe(true);
    expect(freshFavorite.isConnected).toBe(false);
  } finally { s.cleanup(); }
});
