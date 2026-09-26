// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot } from "solid-js";
import { type AgentDragReorder, type AgentDragReorderOptions, createAgentDragReorder } from "./agentDragReorder";
import { reorderIndex } from "./agentReorderDnd";
import { dispatchPointer, stubAgentRowGeometry } from "./agentReorderDnd.testkit";

/** #2594 - the shared drag lifecycle over a hand-built list: a container with
 *  three 40 px rows, each holding one grip. The factory only returns handlers,
 *  so they are wired to the grip here the way both components' JSX does. */
describe("createAgentDragReorder", () => {
  let frames: FrameRequestCallback[];
  let container: HTMLElement;
  let rows: HTMLElement[];
  let grip: HTMLElement;
  let commit: ReturnType<typeof vi.fn<AgentDragReorderOptions["commit"]>>;
  let announce: ReturnType<typeof vi.fn<AgentDragReorderOptions["announce"]>>;
  let canDrag: boolean;
  let drag: AgentDragReorder;
  let dispose: () => void;

  beforeEach(() => {
    frames = [];
    vi.stubGlobal("requestAnimationFrame", vi.fn((cb: FrameRequestCallback) => frames.push(cb)));
    vi.stubGlobal("cancelAnimationFrame", vi.fn());
    container = document.createElement("div");
    rows = ["a", "b", "c"].map((id) => {
      const row = document.createElement("div");
      row.className = "row";
      row.id = `row-${id}`;
      row.setAttribute("data-ac-testid", `row.${id}`);
      row.setAttribute("data-ac-role", "row");
      const handle = document.createElement("button");
      handle.className = "grip";
      handle.setAttribute("data-ac-testid", `row.${id}.grip`);
      row.append(handle);
      container.append(row);
      return row;
    });
    document.body.append(container);
    grip = rows[1]!.querySelector<HTMLElement>(".grip")!;
    stubAgentRowGeometry(rows, grip);
    commit = vi.fn();
    announce = vi.fn();
    canDrag = true;
    createRoot((d) => {
      dispose = d;
      drag = createAgentDragReorder({
        container: () => container,
        rowSelector: ".row",
        handleSelector: ".grip",
        canDrag: () => canDrag,
        commit,
        announce,
      });
    });
    grip.addEventListener("pointerdown", (e) => drag.onPointerDown(e as PointerEvent, "b"));
    grip.addEventListener("pointermove", (e) => drag.onPointerMove(e as PointerEvent));
    grip.addEventListener("pointerup", (e) => drag.onPointerUp(e as PointerEvent));
    grip.addEventListener("pointercancel", (e) => drag.onPointerCancel(e as PointerEvent));
  });

  afterEach(() => {
    dispose();
    container.remove();
    vi.unstubAllGlobals();
  });

  const ghosts = () => document.querySelectorAll(".drag-ghost");
  const isDraggingClass = () => document.body.classList.contains("is-dragging");

  it("creates no ghost and no is-dragging below the threshold", () => {
    dispatchPointer(grip, "pointerdown", 5, 60);
    dispatchPointer(grip, "pointermove", 7, 62);
    expect(ghosts()).toHaveLength(0);
    expect(isDraggingClass()).toBe(false);
    expect(drag.dragSourceId()).toBeNull();
    expect(drag.isDragging()).toBe(true);
  });

  it("crossing the threshold creates one ghost and marks the source", () => {
    dispatchPointer(grip, "pointerdown", 5, 60);
    dispatchPointer(grip, "pointermove", 5, 140);
    expect(ghosts()).toHaveLength(1);
    expect(drag.dragSourceId()).toBe("b");
    expect(isDraggingClass()).toBe(true);
  });

  it("strips ids from the ghost and makes it inert", () => {
    dispatchPointer(grip, "pointerdown", 5, 60);
    dispatchPointer(grip, "pointermove", 5, 140);
    const ghost = ghosts()[0] as HTMLElement;
    for (const node of [ghost, ...ghost.querySelectorAll("*")]) {
      expect(node.hasAttribute("data-ac-testid")).toBe(false);
      expect(node.hasAttribute("data-ac-role")).toBe(false);
      expect(node.hasAttribute("id")).toBe(false);
    }
    expect(ghost.getAttribute("aria-hidden")).toBe("true");
    expect(ghost.getAttribute("tabindex")).toBe("-1");
    expect(ghost.hasAttribute("inert")).toBe(true);
  });

  it("commits once on a drop at a new slot", () => {
    dispatchPointer(grip, "pointerdown", 5, 60);
    dispatchPointer(grip, "pointermove", 5, 140);
    dispatchPointer(grip, "pointerup", 5, 140);
    expect(commit).toHaveBeenCalledTimes(1);
    expect(commit).toHaveBeenCalledWith("b", reorderIndex(1, 2));
    expect(ghosts()).toHaveLength(0);
  });

  it("does not commit on a drop at the same slot", () => {
    dispatchPointer(grip, "pointerdown", 5, 60);
    dispatchPointer(grip, "pointermove", 5, 140);
    dispatchPointer(grip, "pointermove", 5, 60);
    dispatchPointer(grip, "pointerup", 5, 60);
    expect(commit).not.toHaveBeenCalled();
    expect(drag.isDragging()).toBe(false);
  });

  it("Escape mid-drag cancels, prevents default and announces once", () => {
    dispatchPointer(grip, "pointerdown", 5, 60);
    dispatchPointer(grip, "pointermove", 5, 140);
    const escape = new KeyboardEvent("keydown", { key: "Escape", cancelable: true });
    window.dispatchEvent(escape);
    expect(escape.defaultPrevented).toBe(true);
    expect(announce).toHaveBeenCalledTimes(1);
    expect(announce).toHaveBeenCalledWith("Move cancelled.");
    expect(ghosts()).toHaveLength(0);
    expect(isDraggingClass()).toBe(false);
  });

  it("Escape before the threshold announces nothing", () => {
    dispatchPointer(grip, "pointerdown", 5, 60);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", cancelable: true }));
    expect(announce).not.toHaveBeenCalled();
  });

  it("pointercancel with the matching pointerId cancels the drag", () => {
    dispatchPointer(grip, "pointerdown", 5, 60);
    dispatchPointer(grip, "pointermove", 5, 140);
    dispatchPointer(grip, "pointercancel", 5, 140);
    expect(ghosts()).toHaveLength(0);
    expect(drag.dragSourceId()).toBeNull();
    expect(announce).toHaveBeenCalledWith("Move cancelled.");
    expect(commit).not.toHaveBeenCalled();
  });

  it("disposing the root mid-drag removes the ghost, the class and the Escape listener", () => {
    dispatchPointer(grip, "pointerdown", 5, 60);
    dispatchPointer(grip, "pointermove", 5, 140);
    dispose();
    expect(ghosts()).toHaveLength(0);
    expect(isDraggingClass()).toBe(false);
    const escape = new KeyboardEvent("keydown", { key: "Escape", cancelable: true });
    window.dispatchEvent(escape);
    expect(announce).not.toHaveBeenCalled();
    expect(escape.defaultPrevented).toBe(false);
  });

  it("canDrag() false makes pointerdown a no-op", () => {
    canDrag = false;
    const down = dispatchPointer(grip, "pointerdown", 5, 60);
    expect(grip.setPointerCapture).not.toHaveBeenCalled();
    expect(down.defaultPrevented).toBe(false);
    expect(drag.isDragging()).toBe(false);
  });

  it("ignores a pointer event whose pointerId does not match", () => {
    dispatchPointer(grip, "pointerdown", 5, 60);
    const foreign = new MouseEvent("pointermove", { bubbles: true, cancelable: true, clientX: 5, clientY: 140 });
    Object.defineProperty(foreign, "pointerId", { value: 2 });
    grip.dispatchEvent(foreign);
    expect(ghosts()).toHaveLength(0);
    expect(isDraggingClass()).toBe(false);
    expect(drag.dragSourceId()).toBeNull();
    dispatchPointer(grip, "pointermove", 5, 140);
    expect(ghosts()).toHaveLength(1);
  });

  it("auto-scrolls on a frame tick and stops after teardown", () => {
    container.getBoundingClientRect = () =>
      ({ top: 0, bottom: 120, height: 120, left: 0, right: 200, width: 200, x: 0, y: 0 }) as DOMRect;
    Object.defineProperty(container, "scrollTop", { configurable: true, writable: true, value: 0 });
    dispatchPointer(grip, "pointerdown", 5, 60);
    dispatchPointer(grip, "pointermove", 5, 115);
    expect(frames).toHaveLength(1);
    frames[0]!(0);
    const scrolled = container.scrollTop;
    expect(scrolled).toBeGreaterThan(0);
    dispose();
    frames[frames.length - 1]!(0);
    expect(container.scrollTop).toBe(scrolled);
  });
});
