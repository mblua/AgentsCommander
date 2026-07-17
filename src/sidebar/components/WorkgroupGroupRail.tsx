import { Component, For, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import type { AcWorkgroup, WorkgroupGroup } from "../../shared/types";
import type { ProjectState } from "../stores/project";
import { projectStore } from "../stores/project";
import { projectCollapseStore } from "../stores/project-collapse";
import { railCollapseStore } from "../stores/rail-collapse";
import {
  MAX_GROUP_MATCH_ID_LENGTH,
  compileGroupRegex,
  groupMatchId,
  nonStopDisplayName,
  nonStopMatchesWorkgroup,
  workgroupGroupsStore,
  type WorkgroupGroupSelection,
} from "../stores/workgroup-groups";
import {
  isReplicaWorking,
  splitWorkgroupsByWorking,
  workgroupHasRaisedHand,
} from "./workgroup-session";
import WorkgroupGroupsModal from "./WorkgroupGroupsModal";
import RaiseHandIcon from "./RaiseHandIcon";
import ArchiveIcon from "./ArchiveIcon";
import ArchivedProjectsModal from "./ArchivedProjectsModal";

interface WorkgroupGroupRailProps {
  projects: ProjectState[];
}

interface GroupButton {
  key: string;
  name: string;
  counter: string;
  working: boolean;
  raiseHand: boolean;
  selection: WorkgroupGroupSelection;
  workgroups: AcWorkgroup[];
  title: string;
  reorderable: boolean;
  groupId: string | null;
  groupIndex: number | null;
}

type RailContextTarget =
  | { kind: "project"; projectPath: string }
  | { kind: "group"; projectPath: string; groupId: string };

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

function wgNumber(name: string): number {
  const match = name.match(/^wg-(\d+)/i);
  return match ? Number.parseInt(match[1], 10) : Number.MAX_SAFE_INTEGER;
}

function wgTooltipLabel(wgName: string): string {
  const match = wgName.match(/^wg-(\d+)/i);
  return match ? `WG${match[1]}` : wgName;
}

function groupMatches(group: WorkgroupGroup, wg: AcWorkgroup): boolean {
  const id = groupMatchId(wg);
  if (id.length > MAX_GROUP_MATCH_ID_LENGTH) return false;
  const regex = compileGroupRegex(group);
  return !!regex?.test(id);
}

function tooltipFor(folderName: string, workgroups: AcWorkgroup[]): string {
  const rows = workgroups
    .flatMap((wg) =>
      wg.agents
        .filter((replica) => isReplicaWorking(wg, replica))
        .map((replica) => ({ wg, replica }))
    )
    .sort((a, b) => {
      const wgDelta = wgNumber(a.wg.name) - wgNumber(b.wg.name);
      if (wgDelta !== 0) return wgDelta;
      return a.replica.name.localeCompare(b.replica.name, "en", { sensitivity: "base", numeric: true });
    })
    .map(({ wg, replica }) => `${wgTooltipLabel(wg.name)}:(${replica.name})`);
  const body = rows.length > 0 ? rows.join("\n") : "No running agents";
  return `${folderName}\n${body}`;
}

function buttonContent(
  name: string,
  workgroups: AcWorkgroup[]
): Pick<GroupButton, "name" | "counter" | "working" | "raiseHand"> {
  const working = splitWorkgroupsByWorking(workgroups).working.length;
  return {
    name,
    counter: `${working}/${workgroups.length}`,
    working: working > 0,
    raiseHand: workgroups.some(workgroupHasRaisedHand),
  };
}

type RailButtonTestIds = { button: string; raiseHand: string; dot: string };

function projectRailTestIds(key: string): RailButtonTestIds {
  return {
    button: `workgroupGroups.button.${key}`,
    raiseHand: `workgroupGroups.raiseHand.${key}`,
    dot: `workgroupGroups.dot.${key}`,
  };
}

function favoriteRailTestIds(folderName: string, groupId: string): RailButtonTestIds {
  const key = `${folderName}.${groupId}`;
  return {
    button: `workgroupGroups.favoriteButton.${key}`,
    raiseHand: `workgroupGroups.favoriteRaiseHand.${key}`,
    dot: `workgroupGroups.favoriteDot.${key}`,
  };
}

function groupButtonFor(
  project: ProjectState,
  group: WorkgroupGroup,
  groupIndex: number | null,
  reorderable: boolean
): GroupButton {
  const workgroups = project.workgroups.filter((wg) => groupMatches(group, wg));
  return {
    key: group.id,
    ...buttonContent(group.name, workgroups),
    selection: { kind: "group", id: group.id },
    workgroups,
    title: tooltipFor(project.folderName, workgroups),
    reorderable,
    groupId: group.id,
    groupIndex,
  };
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
}

const RailButton: Component<{
  button: GroupButton;
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
}> = (props) => (
  <button
    ref={(el) => props.onRef?.(el)}
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
      <Show when={props.button.raiseHand}>
        <span
          class="workgroup-group-rail-raise-hand"
          data-ac-testid={props.testIds.raiseHand}
          title="A coordinator raised its hand"
          aria-label="A coordinator raised its hand"
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
    <span class="workgroup-group-rail-counter-line">
      <Show when={props.button.working}>
        <span
          class="session-item-status running workgroup-group-rail-dot"
          data-ac-testid={props.testIds.dot}
        />
      </Show>
      {props.button.counter}
    </span>
  </button>
);

const FavoritesRailSection: Component<{
  projects: ProjectState[];
  onOpenContextMenu: (event: MouseEvent, target: RailContextTarget) => void;
}> = (props) => {
  const collapsed = () => railCollapseStore.isFavoritesCollapsed();

  const entries = createMemo(() =>
    props.projects.flatMap((project) =>
      workgroupGroupsStore
        .config(project.path)
        .groups.filter((group) => group.favorite)
        .map((group) => ({ project, group, button: groupButtonFor(project, group, null, false) }))
    )
  );

  return (
    <Show when={entries().length > 0}>
      <div class="workgroup-group-rail-favorites" data-ac-testid="workgroupGroups.favorites">
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
            <For each={entries()}>
              {(entry) => (
                <RailButton
                  button={entry.button}
                  testIds={favoriteRailTestIds(entry.project.folderName, entry.group.id)}
                  selected={isSelected(entry.project.path, entry.button)}
                  onContextMenu={(event) =>
                    props.onOpenContextMenu(event, {
                      kind: "group",
                      projectPath: entry.project.path,
                      groupId: entry.group.id,
                    })
                  }
                  onClick={() => selectFromRail(entry.project, entry.button.selection)}
                />
              )}
            </For>
          </div>
        </Show>
      </div>
    </Show>
  );
};

const ProjectRailSection: Component<{
  project: ProjectState;
  showProjectLabel: boolean;
  onOpenContextMenu: (event: MouseEvent, target: RailContextTarget) => void;
}> = (props) => {
  createEffect(() => {
    void workgroupGroupsStore.ensureLoaded(props.project.path);
  });

  const config = () => workgroupGroupsStore.config(props.project.path);
  const collapsed = () => railCollapseStore.isProjectCollapsed(props.project.path);
  const ungroupedWorkgroups = createMemo(() =>
    props.project.workgroups.filter(
      (wg) => !config().groups.some((group) => groupMatches(group, wg))
    )
  );
  const buttons = createMemo<GroupButton[]>(() => {
    const result: GroupButton[] = [];
    if (config().showAll) {
      result.push({
        key: "all",
        ...buttonContent("All", props.project.workgroups),
        raiseHand: false,
        selection: { kind: "all" },
        workgroups: props.project.workgroups,
        title: tooltipFor(props.project.folderName, props.project.workgroups),
        reorderable: false,
        groupId: null,
        groupIndex: null,
      });
    }
    if (config().showUngrouped) {
      const workgroups = ungroupedWorkgroups();
      result.push({
        key: "ungrouped",
        ...buttonContent("Ungrouped", workgroups),
        selection: { kind: "ungrouped" },
        workgroups,
        title: tooltipFor(props.project.folderName, workgroups),
        reorderable: false,
        groupId: null,
        groupIndex: null,
      });
    }
    const nonStop = config().nonStop;
    if (nonStop?.show) {
      const workgroups = props.project.workgroups.filter((wg) =>
        nonStopMatchesWorkgroup(nonStop, wg)
      );
      result.push({
        key: "nonstop",
        ...buttonContent(nonStopDisplayName(nonStop.name), workgroups),
        selection: { kind: "nonstop" },
        workgroups,
        title: tooltipFor(props.project.folderName, workgroups),
        reorderable: false,
        groupId: null,
        groupIndex: null,
      });
    }
    for (const [groupIndex, group] of config().groups.entries()) {
      result.push(groupButtonFor(props.project, group, groupIndex, true));
    }
    return result;
  });
  const [reorderState, setReorderState] = createSignal<ReorderState | null>(null);
  let holdTimer: number | null = null;
  let suppressClickGroupId: string | null = null;
  let projectEl: HTMLDivElement | undefined;
  const groupButtonEls = new Map<string, HTMLButtonElement>();

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
        <For each={previewButtons()}>
          {(button) => (
            <RailButton
              button={button}
              testIds={projectRailTestIds(button.key)}
              selected={isSelected(props.project.path, button)}
              reorderState={reorderState()}
              onRef={(el) => {
                if (button.groupId) groupButtonEls.set(button.groupId, el);
              }}
              onPointerDown={(event) => startPress(event, button)}
              onPointerMove={movePress}
              onPointerUp={finishPress}
              onPointerCancel={cancelPress}
              onContextMenu={(event) =>
                openContextMenu(
                  event,
                  button.groupId
                    ? { kind: "group", projectPath: props.project.path, groupId: button.groupId }
                    : { kind: "project", projectPath: props.project.path }
                )
              }
              onClick={(event) => {
                if (button.groupId && suppressClickGroupId === button.groupId) {
                  suppressClickGroupId = null;
                  event.preventDefault();
                  event.stopPropagation();
                  return;
                }
                selectFromRail(props.project, button.selection);
              }}
            />
          )}
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

const WorkgroupGroupRail: Component<WorkgroupGroupRailProps> = (props) => {
  const [showArchived, setShowArchived] = createSignal(false);
  const [contextTarget, setContextTarget] = createSignal<RailContextTarget | null>(null);
  const [contextMenuPos, setContextMenuPos] = createSignal({ x: 0, y: 0 });
  const [editingProjectPath, setEditingProjectPath] = createSignal<string | null>(null);
  let contextMenuEl: HTMLDivElement | undefined;
  let dismissContextMenu: ((ev?: Event) => void) | null = null;

  createEffect(() => {
    workgroupGroupsStore.reconcileActiveProject(props.projects.map((project) => project.path));
  });

  const editingProject = createMemo(() => {
    const path = editingProjectPath();
    return path ? (props.projects.find((project) => project.path === path) ?? null) : null;
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
      if (projectGone || groupGone) closeContextMenu();
    }
    const editing = editingProjectPath();
    if (editing && !live.has(editing)) setEditingProjectPath(null);
  });

  onCleanup(cleanupContextMenu);

  const favoriteTargetIsFavorited = () => {
    const target = contextTarget();
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
    if (target?.kind !== "group") return;
    const next = !favoriteTargetIsFavorited();
    closeContextMenu();
    void workgroupGroupsStore
      .setGroupFavorite(target.projectPath, target.groupId, next)
      .catch(() => {
      });
  };

  return (
    <aside class="workgroup-group-rail" data-ac-testid="workgroupGroups.rail">
      <FavoritesRailSection projects={props.projects} onOpenContextMenu={openContextMenu} />

      <div class="workgroup-group-rail-scroll">
        <For each={props.projects}>
          {(project) => (
            <ProjectRailSection
              project={project}
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
            ref={contextMenuEl}
            class="session-context-menu"
            style={{ left: `${contextMenuPos().x}px`, top: `${contextMenuPos().y}px` }}
            onClick={(event) => event.stopPropagation()}
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
            <Show when={contextTarget()?.kind === "group"}>
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

export default WorkgroupGroupRail;
