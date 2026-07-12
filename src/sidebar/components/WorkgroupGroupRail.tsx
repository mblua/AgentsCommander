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

/** #965 — the context menu and the groups modal live at the rail root (they are
 *  shared by the project sections and the cross-project Favorites section), so a
 *  target must be stored as IDENTITY, never as a captured `ProjectState` /
 *  `WorkgroupGroup` object: `projectStore` replaces those objects on every reload,
 *  and a capture goes stale the moment discovery refreshes. */
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
  // #965 — the rail is 68px and the project header ellipsizes, so the native
  // multi-line `title` is the only place the full project identity can live. It
  // is what identifies a Favorites entry's owning project without a visible label.
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

/** #965 — the rail button markup carries THREE testids, all keyed on the group id.
 *  A favorited group renders twice (Favorites + its project section), so all three
 *  would duplicate. That is a hard break, not a wart: `automation-bridge.ts` THROWS
 *  on a duplicate `data-ac-testid` rather than taking the first match, and the
 *  suites' `railButtonOrder()` / `railDots()` do document-wide PREFIX scans
 *  (`^workgroupGroups.button.` / `^workgroupGroups.dot.`) that a suffixed id would
 *  still double. Hence a DISTINCT prefix for Favorites, not a namespaced suffix. */
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

/** #965 — lifted out of the `buttons` memo so the cross-project Favorites section
 *  can build an identical button for a group it does not own. `reorderable` is a
 *  parameter, and Favorites MUST pass `false` (see the reorder invariants). */
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

/** #965 — was a closure inside `ProjectRailSection`; both sections need it now.
 *  Both copies of a favorited group highlight simultaneously, which is correct:
 *  it is one group. */
function isSelected(projectPath: string, button: GroupButton): boolean {
  if (!workgroupGroupsStore.isActiveProject(projectPath)) return false;
  const current = workgroupGroupsStore.selection(projectPath);
  if (current.kind !== button.selection.kind) return false;
  return current.kind !== "group" || button.selection.kind !== "group" || current.id === button.selection.id;
}

/** #965 — the existing group-button onClick body, parameterized by project so a
 *  Favorites click behaves identically (you clicked a group of project B, so the
 *  main panel focuses B).
 *
 *  RC-2: the two `projectCollapseStore` calls drive the **ProjectPanel** (#810
 *  auto-focus) and are unchanged. They must NEVER touch `railCollapseStore`: the
 *  rail folds only on an explicit header click. */
function selectFromRail(project: ProjectState, selection: WorkgroupGroupSelection): void {
  workgroupGroupsStore.select(project.path, selection);
  // Collapse every other loaded project and expand this owner on every click. Use
  // the live project list because a fresh session has no collapse-map entries.
  // Scrolling is owned separately by SidebarApp's primitive semantic-key effect.
  projectCollapseStore.collapseAllExceptKnown(
    project.path,
    projectStore.projects.map((p) => p.path)
  );
  projectCollapseStore.setProjectCollapsed(project.path, false);
}

/** #965 — the rail button markup, extracted verbatim so Favorites renders an
 *  identical button. The pointer handlers and `onRef` are OPTIONAL: Favorites
 *  passes none, so a favorite never enters a `groupButtonEls` map, never arms a
 *  drag, and never gets the `reorderable` class (and its `cursor: grab`). */
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
          // #775 — built-in/system groups (All, Ungrouped, …) render bold to stand
          // out from user-created groups. Gated on the selection kind, not the
          // display name, so a user group named "All" stays normal weight.
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

/** #965 — the cross-project Favorites section. Pinned OUTSIDE
 *  `.workgroup-group-rail-scroll` at the top, mirroring how #881 pinned Archive at
 *  the bottom: that is what makes it permanently visible. */
const FavoritesRailSection: Component<{
  projects: ProjectState[];
  onOpenContextMenu: (event: MouseEvent, target: RailContextTarget) => void;
}> = (props) => {
  const collapsed = () => railCollapseStore.isFavoritesCollapsed();

  // Iterate `props.projects`, never a global list: an archived/hidden project has
  // no rail section and must not contribute favorites. A project whose rail section
  // is COLLAPSED still contributes, because `ensureLoaded` lives outside the
  // collapse `<Show>` — that is what makes a collapsed project's favorites visible.
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
  // #965 — the RAIL's own collapse (RC-1), never the ProjectPanel's. The only
  // `projectCollapseStore` reference left in this file is the #810 auto-focus pair
  // inside `selectFromRail`, which drives the ProjectPanel and is unchanged.
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
        // #775 — the built-in "All" group never shows the raise-hand indicator,
        // under any condition (even when a member workgroup's coordinator has a
        // raised hand). Gated here on the statically-built "all" entry, not on
        // the display name, so a user group coincidentally named "All" is
        // unaffected. Ungrouped + dynamic groups keep their aggregation below.
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

  // #965 — this section owns the reorder gesture, so it cancels the drag before
  // delegating to the rail root (which owns the menu). preventDefault /
  // stopPropagation live in the root's handler.
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
    // #965 G2 — a collapsed section has no drop targets at all.
    //
    // UNREACHABLE today, deliberately: RC-1 makes the header's onClick the only
    // mutator of rail collapse, and G3 cancels the drag there BEFORE the toggle, so
    // `collapsed()` is never true while a reorder is in flight. Nor is there a
    // partially-unmounted frame in between — Solid disposes the <Show> synchronously
    // inside the click task. G1 below is the guard that actually fires in production.
    //
    // Kept as the backstop for a future mutator that folds outside the header path.
    // Deliberately pinned by NO test: reaching it requires manufacturing a state the
    // design forbids, and a test for that would calcify it.
    if (collapsed()) return null;
    const projectRect = projectEl.getBoundingClientRect();
    if (clientY < projectRect.top || clientY > projectRect.bottom) return null;

    const candidates = Array.from(groupButtonEls.entries())
      .filter(([id]) => id !== groupId)
      // Solid does not null out `ref` callbacks on disposal, so this map retains
      // detached elements whose getBoundingClientRect() is all zeros. Necessary,
      // but NOT sufficient — see G1. Do not remove it.
      .map(([id, el]) => ({ id, rect: el.getBoundingClientRect() }))
      .filter(({ rect }) => rect.height > 0)
      .sort((a, b) => a.rect.top - b.rect.top);

    // #965 G1 (root fix) — "no candidates" means "nothing to drop onto", NOT "drop
    // at index 0". Falling through to `return candidates.length` (= 0) here would
    // splice the dragged group to the front and PERSIST it: a drag in flight whose
    // buttons unmount mid-gesture (a header click) would silently reorder the
    // project. The only pre-existing empty case is a single-group project, where
    // sourceIndex is 0 anyway and the reorder was already a no-op, so null is
    // equally correct there.
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
            // #965 G3 — cancel any in-flight drag BEFORE the toggle unmounts the
            // buttons, so the trailing pointerup cannot resolve a drop against an
            // empty candidate list. Same one-liner the context menu already uses.
            // suppressClick=true is correct: the gesture was a drag, not a click.
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
                  // Pseudo-entries (All / Ungrouped / Alert me!) have `groupId: null`
                  // and cannot be favorited, so they open the project menu (Edit only).
                  // Gated on the id, never on the display name.
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
  // #965 — the menu and the groups modal are hoisted here from ProjectRailSection:
  // Favorites is cross-project and needs the same menu, and one <Portal> + one set
  // of window dismiss listeners beats N of them.
  const [contextTarget, setContextTarget] = createSignal<RailContextTarget | null>(null);
  const [contextMenuPos, setContextMenuPos] = createSignal({ x: 0, y: 0 });
  const [editingProjectPath, setEditingProjectPath] = createSignal<string | null>(null);
  let contextMenuEl: HTMLDivElement | undefined;
  let dismissContextMenu: ((ev?: Event) => void) | null = null;

  createEffect(() => {
    workgroupGroupsStore.reconcileActiveProject(props.projects.map((project) => project.path));
  });

  // Resolved LIVE from props.projects, never from a capture, so the modal header
  // cannot go stale after a discovery refresh (and it closes if the project dies).
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
    // Deferred so the opening `contextmenu` event does not dismiss its own menu.
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

  // #965 — the menu/modal now outlive the section that opened them (they live at the
  // rail root). Drop a target whose project has left `props.projects`, OR whose group
  // has left that project's config — a cross-window delete kills the group while the
  // project lives, and that is the commoner case. Without this the menu offers
  // "Favorite" on a dead group and the click is a silent no-op (the throw precedes
  // any setEntries, so no `!` indicator ever appears). Mirrors the groupButtonEls
  // prune effect in ProjectRailSection.
  //
  // NOT a write barrier: a `WorkgroupGroupsModal.save()` already in flight still
  // lands. That is harmless (the store never evicts entries, so the modal seeded from
  // the real config and rewrites the same groups), but nobody should believe otherwise.
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

  // Read from the store at render, never from a capture, so the label is correct
  // after an external update (another window favoriting the same group).
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
        // A save failure surfaces through workgroupGroupsStore.error(path), which the
        // project section renders as the `!` badge.
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
