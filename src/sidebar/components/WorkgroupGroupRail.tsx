import { Component, For, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import type { AcWorkgroup, WorkgroupGroup } from "../../shared/types";
import type { ProjectState } from "../stores/project";
import { projectStore } from "../stores/project";
import { projectCollapseStore } from "../stores/project-collapse";
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
  /** #746/#882: true when the button's workgroup set has >=1 working agent
   *  (isSessionWorking: live + active/running, not waiting/pending). */
  working: boolean;
  /** #763 — true when >=1 coordinator in the button's workgroup set has a
   *  raised hand (mirrors ProjectPanel's per-row `showRaiseHand`). */
  raiseHand: boolean;
  selection: WorkgroupGroupSelection;
  workgroups: AcWorkgroup[];
  title: string;
  reorderable: boolean;
  groupId: string | null;
  groupIndex: number | null;
}

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

function tooltipFor(workgroups: AcWorkgroup[]): string {
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
  return rows.length > 0 ? rows.join("\n") : "No running agents";
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

const ProjectRailSection: Component<{
  project: ProjectState;
  showProjectLabel: boolean;
}> = (props) => {
  const [editing, setEditing] = createSignal(false);

  createEffect(() => {
    void workgroupGroupsStore.ensureLoaded(props.project.path);
  });

  const config = () => workgroupGroupsStore.config(props.project.path);
  const selection = () => workgroupGroupsStore.selection(props.project.path);
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
        // #775 — the built-in "All" group never shows the raise-hand indicator,
        // under any condition (even when a member workgroup's coordinator has a
        // raised hand). Gated here on the statically-built "all" entry, not on
        // the display name, so a user group coincidentally named "All" is
        // unaffected. Ungrouped + dynamic groups keep their aggregation below.
        raiseHand: false,
        selection: { kind: "all" },
        workgroups: props.project.workgroups,
        title: tooltipFor(props.project.workgroups),
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
        title: tooltipFor(workgroups),
        reorderable: false,
        groupId: null,
        groupIndex: null,
      });
    }
    // #777: the built-in Non-stop group pins directly after Ungrouped (or after
    // All when Ungrouped is hidden, or first when both are hidden), before the
    // user groups. Reuses buttonContent/tooltipFor so its counter + running dot
    // render identically to every other button.
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
        title: tooltipFor(workgroups),
        reorderable: false,
        groupId: null,
        groupIndex: null,
      });
    }
    for (const [groupIndex, group] of config().groups.entries()) {
      const workgroups = props.project.workgroups.filter((wg) => groupMatches(group, wg));
      result.push({
        key: group.id,
        ...buttonContent(group.name, workgroups),
        selection: { kind: "group", id: group.id },
        workgroups,
        title: tooltipFor(workgroups),
        reorderable: true,
        groupId: group.id,
        groupIndex,
      });
    }
    return result;
  });
  const selected = (button: GroupButton) => {
    if (!workgroupGroupsStore.isActiveProject(props.project.path)) return false;
    const current = selection();
    if (current.kind !== button.selection.kind) return false;
    return current.kind !== "group" || button.selection.kind !== "group" || current.id === button.selection.id;
  };
  const [reorderState, setReorderState] = createSignal<ReorderState | null>(null);
  const [showContextMenu, setShowContextMenu] = createSignal(false);
  const [contextMenuPos, setContextMenuPos] = createSignal({ x: 0, y: 0 });
  let holdTimer: number | null = null;
  let suppressClickGroupId: string | null = null;
  let projectEl: HTMLDivElement | undefined;
  let contextMenuEl: HTMLDivElement | undefined;
  let dismissContextMenu: ((ev?: Event) => void) | null = null;
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

  const openContextMenu = (event: MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    cancelReorder(true);
    cleanupContextMenu();
    setContextMenuPos({ x: event.clientX, y: event.clientY });
    setShowContextMenu(true);
    const dismiss = (ev?: Event) => {
      if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
      setShowContextMenu(false);
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

  const openEditorFromContextMenu = () => {
    setShowContextMenu(false);
    cleanupContextMenu();
    setEditing(true);
  };

  createEffect(() => {
    const validIds = new Set(config().groups.map((group) => group.id));
    for (const id of groupButtonEls.keys()) {
      if (!validIds.has(id)) groupButtonEls.delete(id);
    }
  });

  const targetIndexForPointer = (clientY: number, groupId: string): number | null => {
    if (!projectEl) return null;
    const projectRect = projectEl.getBoundingClientRect();
    if (clientY < projectRect.top || clientY > projectRect.bottom) return null;

    const candidates = Array.from(groupButtonEls.entries())
      .filter(([id]) => id !== groupId)
      .map(([id, el]) => ({ id, rect: el.getBoundingClientRect() }))
      .filter(({ rect }) => rect.height > 0)
      .sort((a, b) => a.rect.top - b.rect.top);

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
      // Window-level pointerup/pointercancel fallback still unwinds the gesture.
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
        // The store already exposes the save error.
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
    cleanupContextMenu();
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
        <div
          class="workgroup-group-rail-project-label"
          title={props.project.path}
          onContextMenu={openContextMenu}
          data-ac-testid={`workgroupGroups.projectLabel.${props.project.folderName}`}
        >
          {props.project.folderName}
        </div>
      </Show>

      <For each={previewButtons()}>
        {(button) => (
          <button
            ref={(el) => {
              if (button.groupId) groupButtonEls.set(button.groupId, el);
            }}
            class="workgroup-group-rail-button"
            classList={{
              selected: selected(button),
              reorderable: button.reorderable,
              "reorder-arming": reorderState()?.phase === "arming" && reorderState()?.groupId === button.groupId,
              "reorder-dragging":
                (reorderState()?.phase === "dragging" || reorderState()?.phase === "saving") &&
                reorderState()?.groupId === button.groupId,
              "reorder-invalid":
                reorderState()?.phase === "dragging" &&
                reorderState()?.groupId === button.groupId &&
                reorderState()?.targetIndex === null,
            }}
            aria-pressed={selected(button)}
            aria-grabbed={button.reorderable ? reorderState()?.groupId === button.groupId : undefined}
            title={button.title}
            onPointerDown={(event) => startPress(event, button)}
            onPointerMove={movePress}
            onPointerUp={finishPress}
            onPointerCancel={cancelPress}
            onContextMenu={openContextMenu}
            onClick={(event) => {
              if (button.groupId && suppressClickGroupId === button.groupId) {
                suppressClickGroupId = null;
                event.preventDefault();
                event.stopPropagation();
                return;
              }
              workgroupGroupsStore.select(props.project.path, button.selection);
              // #810 - auto-focus: collapse other projects, expand owner,
              // scroll owner into view. One-shot at click time; we do NOT
              // re-collapse projects the user re-expands afterwards manually.
              // Grinch F2: feed the live projectStore.projects list to the
              // explicit-list overload so collapse-others works on a fresh
              // session where the collapse map is still empty.
              projectCollapseStore.collapseAllExceptKnown(
                props.project.path,
                projectStore.projects.map((p) => p.path)
              );
              projectCollapseStore.setProjectCollapsed(props.project.path, false);
              projectCollapseStore.requestProjectFocus(props.project.path);
            }}
            data-ac-testid={`workgroupGroups.button.${button.key}`}
          >
            <span class="workgroup-group-rail-title-line">
              <Show when={button.raiseHand}>
                <span
                  class="workgroup-group-rail-raise-hand"
                  data-ac-testid={`workgroupGroups.raiseHand.${button.key}`}
                  title="A coordinator raised its hand"
                  aria-label="A coordinator raised its hand"
                >
                  <RaiseHandIcon class="workgroup-group-rail-raise-hand-icon" />
                </span>
              </Show>
              <span
                class="workgroup-group-rail-title"
                classList={{
                  // #775 — built-in/system groups (All, Ungrouped, …) render bold
                  // to stand out from user-created groups. Gated on the selection
                  // kind, not the display name, so a user group named "All" stays
                  // normal weight.
                  "workgroup-group-rail-title-system": button.selection.kind !== "group",
                }}
              >
                {button.name}
              </span>
            </span>
            <span class="workgroup-group-rail-counter-line">
              <Show when={button.working}>
                <span
                  class="session-item-status running workgroup-group-rail-dot"
                  data-ac-testid={`workgroupGroups.dot.${button.key}`}
                />
              </Show>
              {button.counter}
            </span>
          </button>
        )}
      </For>

      <Show when={workgroupGroupsStore.error(props.project.path)}>
        {(error) => (
          <div class="workgroup-group-rail-error" title={error()}>
            !
          </div>
        )}
      </Show>

      <Show when={showContextMenu()}>
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
          </div>
        </Portal>
      </Show>

      <Show when={editing()}>
        <WorkgroupGroupsModal
          projectPath={props.project.path}
          projectName={props.project.folderName}
          onClose={() => setEditing(false)}
        />
      </Show>
    </div>
  );
};

const WorkgroupGroupRail: Component<WorkgroupGroupRailProps> = (props) => {
  const [showArchived, setShowArchived] = createSignal(false);

  createEffect(() => {
    workgroupGroupsStore.reconcileActiveProject(props.projects.map((project) => project.path));
  });

  return (
    <aside class="workgroup-group-rail" data-ac-testid="workgroupGroups.rail">
      <div class="workgroup-group-rail-scroll">
        <For each={props.projects}>
          {(project) => (
            <ProjectRailSection project={project} showProjectLabel={true} />
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
    </aside>
  );
};

export default WorkgroupGroupRail;
