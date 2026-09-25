import { type Accessor, createSignal, onCleanup } from "solid-js";
import { DRAG_THRESHOLD, autoScrollDelta, insertionSlot, reorderIndex } from "./agentReorderDnd";

/** #2594 - the one pointer drag-reorder lifecycle shared by Settings > Coding
 *  Agents (#2544) and the agent picker (#2577). */
export type AgentDragReorderOptions = {
  /** The scrolling element that is also the rows' offset parent. Read lazily:
   *  both call sites assign it through a Solid `ref` after this runs. */
  container: () => HTMLElement | undefined;
  /** Row element selector, queried inside `container()`. */
  rowSelector: string;
  /** Drag handle selector, matched with `closest()` from the event target. */
  handleSelector: string;
  /** The one gate for starting a gesture. False = pointerdown is ignored. */
  canDrag: () => boolean;
  /** Called once, on a drop that actually moves the row. */
  commit: (agentId: string, targetIndex: number) => void;
  /** Live-region text. Called only with "Move cancelled." by this module. */
  announce: (message: string) => void;
};

export type AgentDragReorder = {
  dragSourceId: Accessor<string | null>;
  dropIndicatorTop: Accessor<number | null>;
  /** True while a gesture is armed, started or not. */
  isDragging: () => boolean;
  onPointerDown: (e: PointerEvent, agentId: string) => void;
  onPointerMove: (e: PointerEvent) => void;
  onPointerUp: (e: PointerEvent) => void;
  onPointerCancel: (e: PointerEvent) => void;
};

/** One in-flight pointer drag. Not render state: only `dragSourceId` and
 *  `dropIndicatorTop` are signals the view reads. */
type PointerDrag = {
  agentId: string;
  handle: HTMLElement;
  row: HTMLElement;
  pointerId: number;
  startX: number;
  startY: number;
  lastY: number;
  rowTop: number;
  slot: number;
  started: boolean;
  ghost: HTMLElement | null;
  raf: number | null;
  onEscape: (e: KeyboardEvent) => void;
};

/** Call in a component body: it registers its own `onCleanup`. */
export function createAgentDragReorder(options: AgentDragReorderOptions): AgentDragReorder {
  const { container, rowSelector, handleSelector, canDrag, commit, announce } = options;
  const [dragSourceId, setDragSourceId] = createSignal<string | null>(null);
  const [dropIndicatorTop, setDropIndicatorTop] = createSignal<number | null>(null);
  let pointerDrag: PointerDrag | null = null;

  const rows = (): HTMLElement[] => {
    const root = container();
    return root ? [...root.querySelectorAll<HTMLElement>(rowSelector)] : [];
  };

  /** Ghost position, insertion slot and drop line, in the container's offset space. */
  const updatePointerDrag = (drag: PointerDrag) => {
    if (drag.ghost) drag.ghost.style.top = `${drag.lastY - (drag.startY - drag.rowTop)}px`;
    const all = rows();
    const from = all.indexOf(drag.row);
    const others = all.filter((row) => row !== drag.row);
    drag.slot = insertionSlot(drag.lastY, others.map((row) => row.getBoundingClientRect()));
    const atSlot = others[drag.slot];
    const last = others[others.length - 1];
    if (from < 0 || drag.slot === from || !last) {
      setDropIndicatorTop(null);
    } else {
      setDropIndicatorTop(atSlot ? atSlot.offsetTop - 3 : last.offsetTop + last.offsetHeight + 1);
    }
  };

  /** Runs on every exit path, including onCleanup: the ghost lives on
   *  document.body and a dead webview never runs a framework unmount. */
  const teardownPointerDrag = () => {
    const drag = pointerDrag;
    if (!drag) return;
    pointerDrag = null;
    if (drag.raf !== null) cancelAnimationFrame(drag.raf);
    drag.ghost?.remove();
    setDropIndicatorTop(null);
    setDragSourceId(null);
    document.body.classList.remove("is-dragging");
    window.removeEventListener("keydown", drag.onEscape, true);
    if (drag.handle.hasPointerCapture(drag.pointerId)) {
      drag.handle.releasePointerCapture(drag.pointerId);
    }
  };

  const cancelPointerDrag = () => {
    const started = pointerDrag?.started ?? false;
    teardownPointerDrag();
    if (started) announce("Move cancelled.");
  };

  const startPointerDrag = (drag: PointerDrag) => {
    const rect = drag.row.getBoundingClientRect();
    drag.rowTop = rect.top;
    const ghost = drag.row.cloneNode(true) as HTMLElement;
    // The clone must not double any test id, id or focusable control.
    for (const node of [ghost, ...ghost.querySelectorAll("*")]) {
      node.removeAttribute("data-ac-testid");
      node.removeAttribute("data-ac-role");
      node.removeAttribute("id");
    }
    ghost.setAttribute("aria-hidden", "true");
    ghost.setAttribute("tabindex", "-1");
    ghost.setAttribute("inert", "");
    ghost.classList.add("drag-ghost");
    ghost.style.width = `${rect.width}px`;
    ghost.style.left = `${rect.left}px`;
    document.body.append(ghost);
    drag.ghost = ghost;
    drag.started = true;
    setDragSourceId(drag.agentId);
    document.body.classList.add("is-dragging");
    // Capture phase: Escape cancels the drag and never reaches the modal close.
    window.addEventListener("keydown", drag.onEscape, true);
    const tick = () => {
      if (pointerDrag !== drag) return;
      const root = container();
      if (root) {
        const list = root.getBoundingClientRect();
        const delta = autoScrollDelta(drag.lastY, list.top, list.bottom);
        if (delta !== 0) {
          root.scrollTop += delta;
          updatePointerDrag(drag);
        }
      }
      drag.raf = requestAnimationFrame(tick);
    };
    drag.raf = requestAnimationFrame(tick);
  };

  const onPointerDown = (e: PointerEvent, agentId: string) => {
    if (e.button !== 0 || !canDrag() || pointerDrag) return;
    const handle = (e.target as Element).closest<HTMLElement>(handleSelector);
    const row = handle?.closest<HTMLElement>(rowSelector);
    if (!handle || !row) return;
    e.preventDefault();
    handle.setPointerCapture(e.pointerId);
    pointerDrag = {
      agentId,
      handle,
      row,
      pointerId: e.pointerId,
      startX: e.clientX,
      startY: e.clientY,
      lastY: e.clientY,
      rowTop: 0,
      slot: -1,
      started: false,
      ghost: null,
      raf: null,
      onEscape: (key: KeyboardEvent) => {
        if (key.key !== "Escape") return;
        key.preventDefault();
        key.stopPropagation();
        cancelPointerDrag();
      },
    };
  };

  const onPointerMove = (e: PointerEvent) => {
    const drag = pointerDrag;
    if (!drag || e.pointerId !== drag.pointerId) return;
    drag.lastY = e.clientY;
    if (!drag.started) {
      if (Math.hypot(e.clientX - drag.startX, e.clientY - drag.startY) < DRAG_THRESHOLD) return;
      startPointerDrag(drag);
    }
    updatePointerDrag(drag);
  };

  const onPointerUp = (e: PointerEvent) => {
    const drag = pointerDrag;
    if (!drag || e.pointerId !== drag.pointerId) return;
    // A refetch that re-rendered mid-drag detaches drag.row: from < 0, no-op.
    const from = rows().indexOf(drag.row);
    teardownPointerDrag();
    if (!drag.started || from < 0 || drag.slot === from) return;
    commit(drag.agentId, reorderIndex(from, drag.slot));
  };

  const onPointerCancel = (e: PointerEvent) => {
    if (pointerDrag && e.pointerId === pointerDrag.pointerId) cancelPointerDrag();
  };

  onCleanup(teardownPointerDrag);

  return {
    dragSourceId,
    dropIndicatorTop,
    isDragging: () => pointerDrag !== null,
    onPointerDown,
    onPointerMove,
    onPointerUp,
    onPointerCancel,
  };
}
