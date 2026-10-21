import { Component, For, Show, createEffect, createMemo, createSignal, onCleanup, onMount, untrack, createComputed } from "solid-js";
import { Portal } from "solid-js/web";
import type { AcAgentReplica, AcWorkgroup, NonStopGroupConfig, WorkgroupGroup } from "../../shared/types";
import type { ProjectState } from "../stores/project";
import { entityDirNumber, entityShortLabel } from "../../shared/entity-prefix";
import { setSidebarCompactMode } from "../../shared/sidebar-compact";
import { projectStore } from "../stores/project";
import { projectCollapseStore } from "../stores/project-collapse";
import { railCollapseStore } from "../stores/rail-collapse";
import {
  compileWorkgroupGroups,
  groupMatchesWorkgroup,
  isUngroupedWorkgroup,
  nonStopDisplayName,
  nonStopMatchesWorkgroup,
  workgroupGroupsStore,
  type CompiledWorkgroupGroup,
  type WorkgroupGroupSelection,
} from "../stores/workgroup-groups";
import {
  isReplicaWorking,
  splitRoomsByActive,
  roomCiRunning,
  roomHasBlockedMenu,
  roomHasRaisedHand,
} from "./room-session";
import WorkgroupGroupsModal from "./WorkgroupGroupsModal";
import RaiseHandIcon from "./RaiseHandIcon";
import BlockedMenuIcon from "./BlockedMenuIcon";
import ArchiveIcon from "./ArchiveIcon";
import ArchivedProjectsModal from "./ArchivedProjectsModal";

import { emitRailDiagnostic, railToken, railObjectId, railEntity, railTag, reconcileRailGroups, type RailFields } from "../rail-diagnostics";
import { isDebugEnabled } from "../../shared/console-capture";

type RailSurface = "embedded" | "sidebar";
interface RailObservation {
  emit: (event: Parameters<typeof emitRailDiagnostic>[1], fields?: RailFields) => void;
  entity: (path: string, selection?: WorkgroupGroupSelection) => RailFields;
  node: (node: HTMLElement, fields: () => RailFields, section?: boolean) => void;
  selection: () => RailFields;
}

function selectionKind(selection: WorkgroupGroupSelection): NonNullable<RailFields["kind"]> {
  return selection.kind === "group" ? "custom" : selection.kind === "all" ? "system" : selection.kind;
}

function sameSelection(a: RailFields, b: RailFields): boolean {
  return a.selectionProjectId === b.selectionProjectId && a.selectionGroupId === b.selectionGroupId && a.selectionKind === b.selectionKind;
}

interface RoomGroupRailProps {
  projects: ProjectState[];
  surface?: RailSurface;
}

interface GroupButton {
  key: string;
  name: string;
  counter: string;
  working: boolean;
  raiseHand: boolean;
  blockedMenu: boolean;
  selection: WorkgroupGroupSelection;
  rooms: AcWorkgroup[];
  title: string;
  reorderable: boolean;
  groupId: string | null;
  groupIndex: number | null;
}

type RailContextTarget =
  | { kind: "project"; projectPath: string }
  | { kind: "group"; projectPath: string; groupId: string }
  | { kind: "nonstop"; projectPath: string };

const REORDER_HOLD_MS = 2000;
const REORDER_MOVE_CANCEL_PX = 6;
const CONTEXT_MENU_VIEWPORT_MARGIN = 8;

type ReorderPhase = "arming" | "dragging" | "saving";

interface ReorderState {
  phase: ReorderPhase;
  pointerId: number;
  groupId: string;
  sourceIndex: number;
  targetIndex: number | null;
  startX: number;
  startY: number;
}

function roomNumber(name: string): number {
  return entityDirNumber(name) ?? Number.MAX_SAFE_INTEGER;
}

function roomTooltipLabel(roomName: string): string {
  return entityShortLabel(roomName) ?? roomName;
}

function tooltipFor(folderName: string, rooms: AcWorkgroup[]): string {
  // #2202 - ONE list and ONE sort over both row kinds: a working replica entry
  // (`replica` set) and a room's CI entry (`replica` null). Room number is the
  // primary key for both, so a CI-only wg1 sorts above a working wg2; an
  // "append CI after the replicas" rule would invert that.
  const entries: { room: AcWorkgroup; replica: AcAgentReplica | null }[] = [];
  for (const room of rooms) {
    for (const replica of room.agents) {
      if (isReplicaWorking(room, replica)) entries.push({ room, replica });
    }
    if (roomCiRunning(room)) entries.push({ room, replica: null });
  }
  const rows = entries
    .sort((a, b) => {
      const roomDelta = roomNumber(a.room.name) - roomNumber(b.room.name);
      if (roomDelta !== 0) return roomDelta;
      if (!a.replica || !b.replica) return (a.replica ? 0 : 1) - (b.replica ? 0 : 1);
      return a.replica.name.localeCompare(b.replica.name, "en", { sensitivity: "base", numeric: true });
    })
    .map(({ room, replica }) => `${roomTooltipLabel(room.name)}:(${replica ? replica.name : "CI"})`);
  const body = rows.length > 0 ? rows.join("\n") : "No running agents";
  return `${folderName}\n${body}`;
}

function buttonContent(
  projectPath: string,
  name: string,
  rooms: AcWorkgroup[]
): Pick<GroupButton, "name" | "counter" | "working" | "raiseHand" | "blockedMenu"> {
  // #2202 - the counter and the dot read the active meaning (session working OR
  // room CI running). The `working` field name is kept: renaming it would touch
  // RailButton and the dot testids for no behavior gain.
  const active = splitRoomsByActive(projectPath, rooms).active.length;
  return {
    name,
    counter: `${active}/${rooms.length}`,
    working: active > 0,
    raiseHand: rooms.some(roomHasRaisedHand),
    blockedMenu: rooms.some(roomHasBlockedMenu),
  };
}

type RailButtonTestIds = {
  button: string;
  raiseHand: string;
  blockedMenu: string;
  dot: string;
};

function projectRailTestIds(key: string): RailButtonTestIds {
  return {
    button: `workgroupGroups.button.${key}`,
    raiseHand: `workgroupGroups.raiseHand.${key}`,
    blockedMenu: `workgroupGroups.blockedMenu.${key}`,
    dot: `workgroupGroups.dot.${key}`,
  };
}

function favoriteRailTestIds(folderName: string, groupId: string): RailButtonTestIds {
  const key = `${folderName}.${groupId}`;
  return {
    button: `workgroupGroups.favoriteButton.${key}`,
    raiseHand: `workgroupGroups.favoriteRaiseHand.${key}`,
    blockedMenu: `workgroupGroups.favoriteBlockedMenu.${key}`,
    dot: `workgroupGroups.favoriteDot.${key}`,
  };
}

// (#1257) A DISJOINT prefix on purpose, not `favoriteRailTestIds(folderName, "nonstop")`.
// That helper keys on the group id, and nothing reserves the string "nonstop":
// `createGroupId` falls back to `group-N` (:373-382) and settings.50.personal.no-git.json is
// hand-editable. A real group with id "nonstop" would then emit a duplicate
// data-ac-testid, which the automation bridge rejects as `duplicate_selector`.
function nonStopFavoriteRailTestIds(folderName: string): RailButtonTestIds {
  return {
    button: `workgroupGroups.favoriteNonStopButton.${folderName}`,
    raiseHand: `workgroupGroups.favoriteNonStopRaiseHand.${folderName}`,
    blockedMenu: `workgroupGroups.favoriteNonStopBlockedMenu.${folderName}`,
    dot: `workgroupGroups.favoriteNonStopDot.${folderName}`,
  };
}

function groupButtonFor(
  project: ProjectState,
  group: WorkgroupGroup,
  groupIndex: number | null,
  reorderable: boolean,
  compiled: readonly CompiledWorkgroupGroup[]
): GroupButton {
  const rooms = project.workgroups.filter((room) => groupMatchesWorkgroup(compiled, group.id, room));
  return {
    key: group.id,
    ...buttonContent(project.path, group.name, rooms),
    selection: { kind: "group", id: group.id },
    rooms,
    title: tooltipFor(project.folderName, rooms),
    reorderable,
    groupId: group.id,
    groupIndex,
  };
}

// (#1257) Built here, not inline in ProjectRailSection, because the Favorites
// section renders the SAME entry. Two copies would drift on display name,
// counter, running dot, raise-hand or tooltip.
function nonStopButtonFor(project: ProjectState, nonStop: NonStopGroupConfig): GroupButton {
  const rooms = project.workgroups.filter((room) => nonStopMatchesWorkgroup(nonStop, room));
  return {
    key: "nonstop",
    ...buttonContent(project.path, nonStopDisplayName(nonStop.name), rooms),
    selection: { kind: "nonstop" },
    rooms,
    title: tooltipFor(project.folderName, rooms),
    reorderable: false,
    groupId: null,
    groupIndex: null,
  };
}

// (#1257) Keyed on the selection, not on `groupId`. The Non-stop button carries
// `groupId: null` because it is not a `groups[]` record, so the previous
// `button.groupId ? group : project` ternary routed it to the project target and
// the Favorite item never rendered.
function railContextTargetFor(projectPath: string, button: GroupButton): RailContextTarget {
  if (button.selection.kind === "nonstop") return { kind: "nonstop", projectPath };
  if (button.groupId) return { kind: "group", projectPath, groupId: button.groupId };
  return { kind: "project", projectPath };
}

function isSelected(projectPath: string, button: GroupButton): boolean {
  if (!workgroupGroupsStore.isActiveProject(projectPath)) return false;
  const current = workgroupGroupsStore.selection(projectPath);
  if (current.kind !== button.selection.kind) return false;
  return current.kind !== "group" || button.selection.kind !== "group" || current.id === button.selection.id;
}

function selectFromRail(project: ProjectState, selection: WorkgroupGroupSelection): void {
  workgroupGroupsStore.select(project.path, selection);
  projectCollapseStore.collapseAllExceptKnown(
    project.path,
    projectStore.projects.map((p) => p.path)
  );
  projectCollapseStore.setProjectCollapsed(project.path, false);
  setSidebarCompactMode(false);
}

const RailButton: Component<{
  button: GroupButton;
  observation: RailObservation;
  projectPath: string;
  testIds: RailButtonTestIds;
  selected: boolean;
  reorderState?: ReorderState | null;
  onRef?: (el: HTMLButtonElement) => void;
  onPointerDown?: (event: PointerEvent & { currentTarget: HTMLButtonElement }) => void;
  onPointerMove?: (event: PointerEvent) => void;
  onPointerUp?: (event: PointerEvent) => void;
  onPointerCancel?: (event: PointerEvent) => void;
  onContextMenu: (event: MouseEvent) => void;
  onClick: (event: MouseEvent) => void;
}> = (props) => {
  let node!: HTMLButtonElement;
  onMount(() => {
    // Logical identity is fixed for this row, including disposal diagnostics.
    const path = props.projectPath;
    const selection = props.button.selection;
    props.observation.node(node, () => props.observation.entity(path, selection));
  });
  return (<button
    ref={(el) => { node = el; props.onRef?.(el); }}
    class="workgroup-group-rail-button"
    classList={{
      selected: props.selected,
      reorderable: props.button.reorderable,
      "reorder-arming":
        props.reorderState?.phase === "arming" && props.reorderState?.groupId === props.button.groupId,
      "reorder-dragging":
        (props.reorderState?.phase === "dragging" || props.reorderState?.phase === "saving") &&
        props.reorderState?.groupId === props.button.groupId,
      "reorder-invalid":
        props.reorderState?.phase === "dragging" &&
        props.reorderState?.groupId === props.button.groupId &&
        props.reorderState?.targetIndex === null,
    }}
    aria-pressed={props.selected}
    aria-grabbed={
      props.button.reorderable ? props.reorderState?.groupId === props.button.groupId : undefined
    }
    title={props.button.title}
    onPointerDown={props.onPointerDown}
    onPointerMove={props.onPointerMove}
    onPointerUp={props.onPointerUp}
    onPointerCancel={props.onPointerCancel}
    onContextMenu={props.onContextMenu}
    onClick={props.onClick}
    data-ac-testid={props.testIds.button}
  >
    <span class="workgroup-group-rail-title-line">
      {/* #1859 - the blocked-menu rollup, mirroring the raise-hand indicator
          right below it. Blocked menu first, then raised hand, in every render
          site. Like the hand it does NOT check collapse: a group holding a
          blocked menu lights whether or not the tab is collapsed, which also
          covers the sidebar regex filter for free (a filtered-out row is
          hidden while its section is still expanded). */}
      <Show when={props.button.blockedMenu}>
        <span
          class="workgroup-group-rail-blocked-menu"
          data-ac-testid={props.testIds.blockedMenu}
          title="A session is waiting on an interactive menu"
          aria-label="A session is waiting on an interactive menu"
        >
          <BlockedMenuIcon class="workgroup-group-rail-blocked-menu-icon" />
        </span>
      </Show>
      <Show when={props.button.raiseHand}>
        <span
          class="workgroup-group-rail-raise-hand"
          data-ac-testid={props.testIds.raiseHand}
          title="An orchestrator raised its hand"
          aria-label="An orchestrator raised its hand"
        >
          <RaiseHandIcon class="workgroup-group-rail-raise-hand-icon" />
        </span>
      </Show>
      <span
        class="workgroup-group-rail-title"
        classList={{
          "workgroup-group-rail-title-system": props.button.selection.kind !== "group",
        }}
      >
        {props.button.name}
      </span>
    </span>
    <span>
      <Show when={props.button.working}>
        <span
          class="session-item-status running workgroup-group-rail-dot"
          data-ac-testid={props.testIds.dot}
        />
      </Show>
      {props.button.counter}
    </span>
  </button>);
};

function buttonIdentity(button: GroupButton): string {
  return JSON.stringify([button.selection.kind, button.selection.kind === "group" ? button.selection.id : null]);
}

function favoriteIdentity(entry: FavoriteEntry): string {
  return JSON.stringify([entry.project.path, entry.button.selection.kind, entry.button.selection.kind === "group" ? entry.button.selection.id : null]);
}

type FavoriteEntry =
  | { kind: "group"; project: ProjectState; group: WorkgroupGroup; button: GroupButton }
  | { kind: "nonstop"; project: ProjectState; button: GroupButton };

const FavoritesRailSection: Component<{
  projects: ProjectState[];
  observation: RailObservation;
  onOpenContextMenu: (event: MouseEvent, target: RailContextTarget) => void;
}> = (props) => {
  const collapsed = () => railCollapseStore.isFavoritesCollapsed();

  let previousEntries: FavoriteEntry[] = [];
  const entries = createMemo<FavoriteEntry[]>(() => {
    const entries = props.projects.flatMap((project) => {
      const config = workgroupGroupsStore.config(project.path);
      const compiled = compileWorkgroupGroups(config.groups);
      const result: FavoriteEntry[] = [];
      const nonStop = config.nonStop;
      // (#1257 D3) `show` is part of the condition on purpose. The rail only draws
      // the Non-stop button when `show` (:338) and `normalizeSelection` bounces a
      // {kind:"nonstop"} selection to All/Ungrouped when it is off
      // (workgroup-groups.ts:191-193), so a favorite rendered while Non-stop is off
      // would be a ghost button that jumps elsewhere on click. The flag is kept,
      // not cleared: turning Non-stop back on brings the entry back.
      // (#1257 D4) Pushed BEFORE the groups so the block mirrors the project
      // section's order and the list is deterministic.
      if (nonStop?.show && nonStop.favorite) {
        result.push({ kind: "nonstop", project, button: nonStopButtonFor(project, nonStop) });
      }
      for (const group of config.groups) {
        if (group.favorite) {
          result.push({
            kind: "group",
            project,
            group,
            button: groupButtonFor(project, group, null, false, compiled),
          });
        }
      }
      return result;
    });
    untrack(() => props.observation.emit("favorites-entries", { objectId: railObjectId(entries), count: entries.length,
      changedCount: entries.filter((entry, index) => entry !== previousEntries[index]).length }));
    previousEntries = entries;
    return entries;
  });

  return (
    <Show when={entries().length > 0}>
      <div ref={(node) => { onMount(() => props.observation.node(node, () => ({ kind: "favorites" }), true)); }} class="workgroup-group-rail-favorites" data-ac-testid="workgroupGroups.favorites">
        <button
          type="button"
          class="workgroup-group-rail-project-label workgroup-group-rail-header"
          aria-expanded={!collapsed()}
          onClick={() => railCollapseStore.toggleFavoritesCollapsed()}
          data-ac-testid="workgroupGroups.favorites.header"
        >
          Favorites
        </button>
        <Show when={!collapsed()}>
          <div class="workgroup-group-rail-favorites-scroll">
            <For each={entries().map(favoriteIdentity)}>
              {(key) => {
                const current = () => entries().find(entry => favoriteIdentity(entry) === key);
                let row: HTMLButtonElement | undefined;
                return <Show when={current()}>{(entry) => (
                  <RailButton
                    onRef={el => { row = el; }}
                    observation={props.observation}
                    projectPath={entry().project.path}
                    button={entry().button}
                    testIds={
                      entry().kind === "nonstop"
                        ? nonStopFavoriteRailTestIds(entry().project.folderName)
                        : favoriteRailTestIds(entry().project.folderName, entry().button.groupId!)
                    }
                    selected={isSelected(entry().project.path, entry().button)}
                    onContextMenu={(event) =>
                      row?.isConnected && current() && props.onOpenContextMenu(
                        event,
                        entry().kind === "nonstop"
                          ? { kind: "nonstop", projectPath: entry().project.path }
                          : {
                              kind: "group",
                              projectPath: entry().project.path,
                              groupId: entry().button.groupId!,
                            }
                      )
                    }
                    onClick={(event) => {
                      if (!row?.isConnected || !current()) return;
                      const before = props.observation.selection();
                      const fields = props.observation.entity(entry().project.path, entry().button.selection);
                      props.observation.emit("handler-enter", { ...fields, ...before, trusted: event.isTrusted, phase: "idle" });
                      selectFromRail(entry().project, entry().button.selection);
                      const after = props.observation.selection();
                      props.observation.emit("handler-end", { ...fields, ...after, trusted: event.isTrusted, phase: "idle",
                        outcome: sameSelection(before, after) ? "unchanged" : "applied" });
                    }}
                  />
              )}</Show>;
              }}
            </For>
          </div>
        </Show>
      </div>
    </Show>
  );
};

const ProjectRailSection: Component<{
  project: ProjectState;
  observation: RailObservation;
  showProjectLabel: boolean;
  onOpenContextMenu: (event: MouseEvent, target: RailContextTarget) => void;
}> = (props) => {
  createEffect(() => {
    void workgroupGroupsStore.ensureLoaded(props.project.path);
  });

  const config = () => workgroupGroupsStore.config(props.project.path);
  const collapsed = () => railCollapseStore.isProjectCollapsed(props.project.path);
  const compiledGroups = createMemo(() => compileWorkgroupGroups(config().groups));
  const ungroupedWorkgroups = createMemo(() =>
    props.project.workgroups.filter((room) =>
      isUngroupedWorkgroup(compiledGroups(), config().nonStop, room)
    )
  );
  let previousButtons: GroupButton[] = [];
  const buttons = createMemo<GroupButton[]>(() => {
    const result: GroupButton[] = [];
    if (config().showAll) {
      result.push({
        key: "all",
        ...buttonContent(props.project.path, "All", props.project.workgroups),
        raiseHand: false,
        blockedMenu: false,
        selection: { kind: "all" },
        rooms: props.project.workgroups,
        title: tooltipFor(props.project.folderName, props.project.workgroups),
        reorderable: false,
        groupId: null,
        groupIndex: null,
      });
    }
    if (config().showUngrouped) {
      const rooms = ungroupedWorkgroups();
      result.push({
        key: "ungrouped",
        ...buttonContent(props.project.path, "Ungrouped", rooms),
        selection: { kind: "ungrouped" },
        rooms,
        title: tooltipFor(props.project.folderName, rooms),
        reorderable: false,
        groupId: null,
        groupIndex: null,
      });
    }
    const nonStop = config().nonStop;
    if (nonStop?.show) {
      result.push(nonStopButtonFor(props.project, nonStop));
    }
    for (const [groupIndex, group] of config().groups.entries()) {
      result.push(groupButtonFor(props.project, group, groupIndex, true, compiledGroups()));
    }
    untrack(() => props.observation.emit("project-buttons", { ...props.observation.entity(props.project.path),
      objectId: railObjectId(result), count: result.length, changedCount: result.filter((button, index) => button !== previousButtons[index]).length }));
    previousButtons = result;
    return result;
  });
  const [reorderState, setReorderState] = createSignal<ReorderState | null>(null);
  let holdTimer: number | null = null;
  let suppressClickGroupId: string | null = null;
  let projectEl: HTMLDivElement | undefined;
  const groupButtonEls = new Map<string, HTMLButtonElement>();
  onMount(() => {
    const path = props.project.path;
    if (projectEl) props.observation.node(projectEl, () => props.observation.entity(path), true);
  });

  const clearHoldTimer = () => {
    if (holdTimer !== null) {
      window.clearTimeout(holdTimer);
      holdTimer = null;
    }
  };

  const cancelReorder = (suppressClick = false) => {
    const current = reorderState();
    clearHoldTimer();
    if (suppressClick && current) {
      suppressClickGroupId = current.groupId;
    } else {
      suppressClickGroupId = null;
    }
    setReorderState(null);
  };

  const openContextMenu = (event: MouseEvent, target: RailContextTarget) => {
    cancelReorder(true);
    props.onOpenContextMenu(event, target);
  };

  createEffect(() => {
    const validIds = new Set(config().groups.map((group) => group.id));
    const current = reorderState();
    if (current && !validIds.has(current.groupId)) cancelReorder(false);
    for (const id of groupButtonEls.keys()) {
      if (!validIds.has(id)) groupButtonEls.delete(id);
    }
  });

  const targetIndexForPointer = (clientY: number, groupId: string): number | null => {
    if (!projectEl) return null;
    if (collapsed()) return null;
    const projectRect = projectEl.getBoundingClientRect();
    if (clientY < projectRect.top || clientY > projectRect.bottom) return null;

    const candidates = Array.from(groupButtonEls.entries())
      .filter(([id]) => id !== groupId)
      .map(([id, el]) => ({ id, rect: el.getBoundingClientRect() }))
      .filter(({ rect }) => rect.height > 0)
      .sort((a, b) => a.rect.top - b.rect.top);

    if (candidates.length === 0) return null;

    for (let index = 0; index < candidates.length; index++) {
      const rect = candidates[index].rect;
      if (clientY < rect.top + rect.height / 2) return index;
    }
    return candidates.length;
  };

  const startPress = (event: PointerEvent & { currentTarget: HTMLButtonElement }, button: GroupButton) => {
    suppressClickGroupId = null;
    if (
      event.button !== 0 ||
      event.isPrimary === false ||
      !button.reorderable ||
      !button.groupId ||
      button.groupIndex === null
    ) {
      return;
    }
    if (workgroupGroupsStore.saving(props.project.path)) return;

    try {
      event.currentTarget.setPointerCapture?.(event.pointerId);
    } catch {
    }

    setReorderState({
      phase: "arming",
      pointerId: event.pointerId,
      groupId: button.groupId,
      sourceIndex: button.groupIndex,
      targetIndex: button.groupIndex,
      startX: event.clientX,
      startY: event.clientY,
    });
    clearHoldTimer();
    holdTimer = window.setTimeout(() => {
      holdTimer = null;
      setReorderState((current) =>
        current?.pointerId === event.pointerId && current.groupId === button.groupId
          ? { ...current, phase: "dragging", targetIndex: current.sourceIndex }
          : current
      );
      suppressClickGroupId = button.groupId;
    }, REORDER_HOLD_MS);
  };

  const movePress = (event: PointerEvent) => {
    const current = reorderState();
    if (!current || current.pointerId !== event.pointerId) return;

    if (current.phase === "arming") {
      const dx = event.clientX - current.startX;
      const dy = event.clientY - current.startY;
      if (Math.hypot(dx, dy) > REORDER_MOVE_CANCEL_PX) cancelReorder(false);
      return;
    }

    if (current.phase === "dragging") {
      const targetIndex = targetIndexForPointer(event.clientY, current.groupId);
      setReorderState({ ...current, targetIndex });
    }
  };

  const finishPress = (event: PointerEvent) => {
    const current = reorderState();
    if (!current || current.pointerId !== event.pointerId) return;

    if (current.phase === "arming") {
      cancelReorder(false);
      return;
    }
    if (current.phase === "saving") return;

    event.preventDefault();
    event.stopPropagation();
    suppressClickGroupId = current.groupId;

    const dropTargetIndex = targetIndexForPointer(event.clientY, current.groupId);
    if (dropTargetIndex === null || dropTargetIndex === current.sourceIndex) {
      cancelReorder(true);
      return;
    }

    setReorderState({ ...current, phase: "saving", targetIndex: dropTargetIndex });
    void (async () => {
      try {
        await workgroupGroupsStore.reorderGroup(props.project.path, current.groupId, dropTargetIndex);
      } catch {
      } finally {
        setReorderState(null);
      }
    })();
  };

  const cancelPress = (event: PointerEvent) => {
    const current = reorderState();
    if (!current || current.pointerId !== event.pointerId || current.phase === "saving") return;
    cancelReorder(current.phase !== "arming");
  };

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key !== "Escape" || !reorderState()) return;
    event.preventDefault();
    cancelReorder(true);
  };

  const onWindowPointerUp = (event: PointerEvent) => {
    const current = reorderState();
    if (!current || current.pointerId !== event.pointerId) return;
    finishPress(event);
  };

  const onWindowPointerMove = (event: PointerEvent) => {
    movePress(event);
  };

  const onWindowPointerCancel = (event: PointerEvent) => {
    cancelPress(event);
  };

  window.addEventListener("keydown", onKeyDown);
  window.addEventListener("pointermove", onWindowPointerMove);
  window.addEventListener("pointerup", onWindowPointerUp);
  window.addEventListener("pointercancel", onWindowPointerCancel);
  onCleanup(() => {
    window.removeEventListener("keydown", onKeyDown);
    window.removeEventListener("pointermove", onWindowPointerMove);
    window.removeEventListener("pointerup", onWindowPointerUp);
    window.removeEventListener("pointercancel", onWindowPointerCancel);
    clearHoldTimer();
    suppressClickGroupId = null;
    groupButtonEls.clear();
  });

  const previewButtons = createMemo(() => {
    const current = reorderState();
    const base = buttons();
    if (!current || current.targetIndex === null || current.phase === "arming") return base;

    const systemButtons = base.filter((button) => !button.reorderable);
    const groupButtons = base.filter((button) => button.reorderable);
    const sourceIndex = groupButtons.findIndex((button) => button.groupId === current.groupId);
    if (sourceIndex < 0) return base;

    const nextGroups = groupButtons.slice();
    const [moved] = nextGroups.splice(sourceIndex, 1);
    const clampedTarget = Math.max(0, Math.min(current.targetIndex, nextGroups.length));
    nextGroups.splice(clampedTarget, 0, moved);
    return [...systemButtons, ...nextGroups];
  });

  return (
    <div
      ref={(el) => {
        projectEl = el;
      }}
      class="workgroup-group-rail-project"
      classList={{ "reorder-active": reorderState()?.phase === "dragging" || reorderState()?.phase === "saving" }}
      data-ac-testid={`workgroupGroups.rail.${props.project.folderName}`}
    >
      <Show when={props.showProjectLabel}>
        <button
          type="button"
          class="workgroup-group-rail-project-label workgroup-group-rail-header"
          title={props.project.path}
          aria-expanded={!collapsed()}
          onClick={() => {
            cancelReorder(true);
            railCollapseStore.toggleProjectCollapsed(props.project.path);
          }}
          onContextMenu={(event) =>
            openContextMenu(event, { kind: "project", projectPath: props.project.path })
          }
          data-ac-testid={`workgroupGroups.projectLabel.${props.project.folderName}`}
        >
          {props.project.folderName}
        </button>
      </Show>

      <Show when={!collapsed()}>
        <For each={previewButtons().map(buttonIdentity)}>
          {(key) => {
            const current = () => previewButtons().find(button => buttonIdentity(button) === key);
            let row: HTMLButtonElement | undefined;
            let groupId: string | null = null;
            onCleanup(() => {
              if (groupId && groupButtonEls.get(groupId) === row) groupButtonEls.delete(groupId);
            });
            return <Show when={current()}>{(button) => (
              <RailButton
                observation={props.observation}
                projectPath={props.project.path}
                button={button()}
                testIds={projectRailTestIds(button().key)}
                selected={isSelected(props.project.path, button())}
                reorderState={reorderState()}
                onRef={(el) => {
                  row = el;
                  groupId = button().groupId;
                  if (groupId) groupButtonEls.set(groupId, el);
                }}
                onPointerDown={(event) => row?.isConnected && current() && startPress(event, button())}
                onPointerMove={movePress}
                onPointerUp={finishPress}
                onPointerCancel={cancelPress}
                onContextMenu={(event) =>
                  row?.isConnected && current() && openContextMenu(event, railContextTargetFor(props.project.path, button()))
                }
                onClick={(event) => {
                  if (!row?.isConnected || !current()) return;
                  const before = props.observation.selection();
                  const fields = props.observation.entity(props.project.path, button().selection);
                  props.observation.emit("handler-enter", { ...fields, ...before, phase: reorderState()?.phase ?? "idle", trusted: event.isTrusted });
                  if (button().groupId && suppressClickGroupId === button().groupId) {
                    suppressClickGroupId = null;
                    event.preventDefault();
                    event.stopPropagation();
                    props.observation.emit("handler-end", { ...fields, ...props.observation.selection(), phase: reorderState()?.phase ?? "idle", trusted: event.isTrusted, outcome: "suppressed" });
                    return;
                  }
                  selectFromRail(props.project, button().selection);
                  const after = props.observation.selection();
                  props.observation.emit("handler-end", { ...fields, ...after, phase: reorderState()?.phase ?? "idle", trusted: event.isTrusted, outcome: sameSelection(before, after) ? "unchanged" : "applied" });
                }}
              />
          )}</Show>;
          }}
        </For>
      </Show>

      {/* Deliberately OUTSIDE the collapse Show: a collapsed project must still
          surface its config error. An error badge that hides itself is a bug. */}
      <Show when={workgroupGroupsStore.error(props.project.path)}>
        {(error) => (
          <div class="workgroup-group-rail-error" title={error()}>
            !
          </div>
        )}
      </Show>
    </div>
  );
};

const RoomGroupRail: Component<RoomGroupRailProps> = (props) => {
  let aside!: HTMLElement;
  let railId: number | undefined;
  const metadata = new WeakMap<Element, () => RailFields>();
  const reconcile = (project: ProjectState) => {
    const config = workgroupGroupsStore.config(project.path);
    reconcileRailGroups(project.path, config.groups.map(group => group.id), config.nonStop !== null);
  };
  const selection = (): RailFields => untrack(() => {
    if (!isDebugEnabled()) return {};
    const project = props.projects.find(project => workgroupGroupsStore.isActiveProject(project.path));
    if (!project) return {};
    const current = workgroupGroupsStore.selection(project.path);
    const entity = railEntity(project.path, selectionKind(current), current.kind === "group" ? current.id : null);
    return { selectionProjectId: entity.projectId, selectionGroupId: entity.groupId, selectionKind: entity.kind };
  });
  const observation: RailObservation = {
    emit(event, fields = {}) { untrack(() => {
      if (!isDebugEnabled()) return;
      railId ??= railToken();
      emitRailDiagnostic(props.surface ?? "sidebar", event, { railId, ...fields });
    }); },
    selection,
    entity(path, current) { return untrack(() => {
      const project = props.projects.find(project => project.path === path);
      if (project) reconcile(project);
      return railEntity(path, current ? selectionKind(current) : "project", current?.kind === "group" ? current.id : null);
    }); },
    node(node, fields, section = false) {
      metadata.set(node, fields);
      const snapshot = () => untrack(() => ({ ...fields(), nodeId: railObjectId(node), connected: node.isConnected }));
      observation.emit(section ? "section-mount" : "node-mount", snapshot());
      onCleanup(() => {
        observation.emit(section ? "section-dispose" : "node-dispose", snapshot());
        metadata.delete(node);
      });
    },
  };
  createComputed(() => {
    for (const project of props.projects) {
      const config = workgroupGroupsStore.config(project.path);
      const groups = config.groups.map(group => group.id);
      const nonstop = config.nonStop !== null;
      void config.showAll;
      untrack(() => reconcileRailGroups(project.path, groups, nonstop));
    }
  });
  let lastProject: string | undefined;
  let lastKind: string | undefined;
  let lastGroup: string | undefined;
  createEffect(() => {
    const active = props.projects.find(project => workgroupGroupsStore.isActiveProject(project.path));
    const current = active ? workgroupGroupsStore.selection(active.path) : undefined;
    const group = current?.kind === "group" ? current.id : undefined;
    if (active?.path !== lastProject || current?.kind !== lastKind || group !== lastGroup) observation.emit("selection-change", selection());
    lastProject = active?.path;
    lastKind = current?.kind;
    lastGroup = group;
  });
  onMount(() => {
    untrack(() => props.projects.forEach(reconcile));
    observation.emit("rail-mount", { nodeId: railObjectId(aside), connected: aside.isConnected });
    type Gesture = { node: Element; fields: RailFields; pointerId?: number };
    const gestures = new Map<number, Gesture>();
    const known = (target: EventTarget | null): Element | undefined => {
      let node = target instanceof Element ? target : null;
      while (node && node !== aside) {
        if (metadata.has(node)) return node;
        node = node.parentElement;
      }
      return undefined;
    };
    const observe = (event: Event) => untrack(() => {
      if (!isDebugEnabled()) { gestures.clear(); return; }
      const pointer = event as PointerEvent;
      const pointerId = typeof pointer.pointerId === "number" ? pointer.pointerId : undefined;
      const key = pointerId ?? -1;
      const inside = event.target instanceof Node && aside.contains(event.target);
      let gesture = gestures.get(key);
      if (!gesture && pointerId === undefined) { const values = [...gestures.values()]; gesture = values[values.length - 1]; }
      if (!inside && (event.type === "pointerdown" || event.type === "mousedown")) {
        // A new outside press ends attribution to the prior rail gesture.
        for (const [id, candidate] of gestures) if (id === key || candidate === gesture) gestures.delete(id);
        return;
      }
      if (!inside && !gesture) return;
      const node = known(event.target);
      if (event.type === "pointerdown" || (event.type === "mousedown" && !gesture)) {
        if (!inside) return;
        if (node) {
          const fields = { ...metadata.get(node)?.(), nodeId: railObjectId(node) };
          if (gestures.size >= 8 && !gestures.has(key)) {
            const oldest = gestures.keys().next().value!;
            gestures.delete(oldest);
            observation.emit("pointercancel", { outcome: "overflow" });
          }
          gesture = { node, fields, pointerId };
          gestures.set(key, gesture);
        }
      }
      let captured = false;
      try { captured = gesture?.pointerId !== undefined && !!gesture.node.hasPointerCapture?.(gesture.pointerId); } catch {}
      observation.emit(event.type as Parameters<typeof emitRailDiagnostic>[1], {
        ...(gesture?.fields ?? (node ? metadata.get(node)?.() : {})), ...selection(),
        pointerId, button: typeof pointer.button === "number" ? pointer.button : undefined,
        primary: typeof pointer.isPrimary === "boolean" ? pointer.isPrimary : undefined,
        trusted: event.isTrusted, targetTag: railTag(event.target), activeTag: railTag(document.activeElement),
        nodeId: railObjectId(node, false), originalNodeId: gesture?.fields.nodeId,
        connected: node?.isConnected, originalConnected: gesture?.node.isConnected, captured,
        documentFocused: document.hasFocus(), outcome: event.type === "click" && !gesture ? "no-press" : undefined,
      });
      if (event.type === "click") {
        for (const [id, candidate] of gestures) if (candidate === gesture) gestures.delete(id);
      }
    });
    const types = "pointerdown pointerup pointercancel mousedown mouseup click gotpointercapture lostpointercapture focusin focusout".split(" ");
    const options = { capture: true, passive: true };
    for (const type of types) document.addEventListener(type, observe, options);
    const windowFocus = (event: Event) => {
      if (gestures.size) observation.emit(event.type === "focus" ? "window-focus" : "window-blur", {
        ...selection(), documentFocused: document.hasFocus(), activeTag: railTag(document.activeElement),
      });
    };
    window.addEventListener("focus", windowFocus, options);
    window.addEventListener("blur", windowFocus, options);
    onCleanup(() => {
      for (const type of types) document.removeEventListener(type, observe, options);
      window.removeEventListener("focus", windowFocus, options);
      window.removeEventListener("blur", windowFocus, options);
      gestures.clear();
      observation.emit("rail-dispose", { nodeId: railObjectId(aside, false), connected: aside.isConnected });
    });
  });
  const [showArchived, setShowArchived] = createSignal(false);
  const [contextTarget, setContextTarget] = createSignal<RailContextTarget | null>(null);
  const [contextMenuPos, setContextMenuPos] = createSignal({ x: 0, y: 0 });
  const [editingProjectPath, setEditingProjectPath] = createSignal<string | null>(null);
  let contextMenuEl: HTMLDivElement | undefined;
  const contextMenuNodes = new WeakSet<HTMLDivElement>();
  onMount(() => {
    const containContextMenuClick = (event: MouseEvent) => {
      if (event.composedPath().some(node => node instanceof HTMLDivElement && contextMenuNodes.has(node))) {
        event.stopPropagation();
      }
    };
    document.addEventListener("click", containContextMenuClick, false);
    onCleanup(() => document.removeEventListener("click", containContextMenuClick, false));
  });
  let dismissContextMenu: ((ev?: Event) => void) | null = null;

  createEffect(() => {
    workgroupGroupsStore.reconcileActiveProject(props.projects.map((project) => project.path));
  });

  const editingProject = createMemo(() => {
    const path = editingProjectPath();
    return path ? (props.projects.find((project) => project.path === path) ?? null) : null;
  });

  let previousProjects: ProjectState[] = [];
  createComputed(() => {
    const projects = props.projects;
    untrack(() => observation.emit("project-list", { objectId: railObjectId(projects), count: projects.length,
      changedCount: projects.filter((project, index) => project !== previousProjects[index]).length }));
    previousProjects = projects;
  });

  const cleanupContextMenu = () => {
    if (!dismissContextMenu) return;
    window.removeEventListener("click", dismissContextMenu);
    window.removeEventListener("contextmenu", dismissContextMenu);
    window.removeEventListener("keydown", dismissContextMenu as EventListener);
    dismissContextMenu = null;
  };

  const positionContextMenu = (x: number, y: number) => {
    if (!contextMenuEl) return;
    const { width, height } = contextMenuEl.getBoundingClientRect();
    const maxX = Math.max(
      CONTEXT_MENU_VIEWPORT_MARGIN,
      window.innerWidth - width - CONTEXT_MENU_VIEWPORT_MARGIN
    );
    const maxY = Math.max(
      CONTEXT_MENU_VIEWPORT_MARGIN,
      window.innerHeight - height - CONTEXT_MENU_VIEWPORT_MARGIN
    );
    setContextMenuPos({
      x: Math.min(Math.max(CONTEXT_MENU_VIEWPORT_MARGIN, x), maxX),
      y: Math.min(Math.max(CONTEXT_MENU_VIEWPORT_MARGIN, y), maxY),
    });
  };

  const openContextMenu = (event: MouseEvent, target: RailContextTarget) => {
    event.preventDefault();
    event.stopPropagation();
    cleanupContextMenu();
    setContextMenuPos({ x: event.clientX, y: event.clientY });
    setContextTarget(target);
    const dismiss = (ev?: Event) => {
      if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
      setContextTarget(null);
      cleanupContextMenu();
    };
    dismissContextMenu = dismiss;
    setTimeout(() => {
      positionContextMenu(event.clientX, event.clientY);
      window.addEventListener("click", dismiss);
      window.addEventListener("contextmenu", dismiss);
      window.addEventListener("keydown", dismiss as EventListener);
    });
  };

  const closeContextMenu = () => {
    setContextTarget(null);
    cleanupContextMenu();
  };

  createEffect(() => {
    const live = new Set(props.projects.map((project) => project.path));
    const target = contextTarget();
    if (target) {
      const projectGone = !live.has(target.projectPath);
      const groupGone =
        target.kind === "group" &&
        !workgroupGroupsStore
          .config(target.projectPath)
          .groups.some((group) => group.id === target.groupId);
      const nonStopGone =
        target.kind === "nonstop" &&
        !workgroupGroupsStore.config(target.projectPath).nonStop?.show;
      if (projectGone || groupGone || nonStopGone) closeContextMenu();
    }
    const editing = editingProjectPath();
    if (editing && !live.has(editing)) setEditingProjectPath(null);
  });

  onCleanup(cleanupContextMenu);

  const favoriteTargetIsFavorited = () => {
    const target = contextTarget();
    if (target?.kind === "nonstop") {
      return !!workgroupGroupsStore.config(target.projectPath).nonStop?.favorite;
    }
    if (target?.kind !== "group") return false;
    return !!workgroupGroupsStore
      .config(target.projectPath)
      .groups.find((group) => group.id === target.groupId)?.favorite;
  };

  const openEditorFromContextMenu = () => {
    const target = contextTarget();
    if (!target) return;
    closeContextMenu();
    setEditingProjectPath(target.projectPath);
  };

  const toggleFavoriteFromContextMenu = () => {
    const target = contextTarget();
    if (!target || target.kind === "project") return;
    const next = !favoriteTargetIsFavorited();
    closeContextMenu();
    const write =
      target.kind === "nonstop"
        ? workgroupGroupsStore.setNonStopFavorite(target.projectPath, next)
        : workgroupGroupsStore.setGroupFavorite(target.projectPath, target.groupId, next);
    void write.catch(() => {
    });
  };

  return (
    <aside ref={aside} class="workgroup-group-rail" data-ac-testid="workgroupGroups.rail">
      <FavoritesRailSection observation={observation} projects={props.projects} onOpenContextMenu={openContextMenu} />

      <div class="workgroup-group-rail-scroll">
        <For each={props.projects.map(project => project.path)}>
          {(path) => (
            <ProjectRailSection
              observation={observation}
              project={props.projects.find(project => project.path === path)!}
              showProjectLabel={true}
              onOpenContextMenu={openContextMenu}
            />
          )}
        </For>
      </div>

      <button
        class="workgroup-group-rail-archive"
        onClick={() => setShowArchived(true)}
        title="Archived projects"
        aria-label="Archived projects"
        aria-haspopup="dialog"
        data-ac-testid="workgroupGroups.rail.archive"
      >
        <ArchiveIcon class="workgroup-group-rail-archive-icon" />
        <span class="workgroup-group-rail-archive-label">Archive</span>
      </button>

      <Show when={showArchived()}>
        <ArchivedProjectsModal onClose={() => setShowArchived(false)} />
      </Show>

      <Show when={contextTarget()}>
        <Portal>
          <div
            ref={element => {
              contextMenuEl = element;
              contextMenuNodes.add(element);
            }}
            class="session-context-menu"
            style={{ left: `${contextMenuPos().x}px`, top: `${contextMenuPos().y}px` }}
            data-ac-testid="workgroupGroups.contextMenu"
            data-ac-role="menu"
          >
            <button
              class="session-context-option"
              onClick={openEditorFromContextMenu}
              data-ac-testid="workgroupGroups.contextMenu.edit"
              data-ac-role="menuitem"
            >
              Edit
            </button>
            <Show
              when={
                contextTarget()?.kind === "group" || contextTarget()?.kind === "nonstop"
              }
            >
              <button
                class="session-context-option"
                onClick={toggleFavoriteFromContextMenu}
                data-ac-testid="workgroupGroups.contextMenu.favorite"
                data-ac-role="menuitem"
              >
                {favoriteTargetIsFavorited() ? "Unfavorite" : "Favorite"}
              </button>
            </Show>
          </div>
        </Portal>
      </Show>

      <Show when={editingProject()}>
        {(project) => (
          <WorkgroupGroupsModal
            projectPath={project().path}
            projectName={project().folderName}
            onClose={() => setEditingProjectPath(null)}
          />
        )}
      </Show>
    </aside>
  );
};

export default RoomGroupRail;
