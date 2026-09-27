// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { For } from "solid-js";
import { render } from "solid-js/web";
import { executeAutomationRequest, resetAutomationBridgeForTests } from "./automation-bridge";
import type { UiAutomationAction, UiAutomationRequest } from "./types";
import {
  type AgentDragReorder,
  createAgentDragReorder,
} from "../sidebar/components/settings/agentDragReorder";
import { stubAgentRowGeometry } from "../sidebar/components/settings/agentReorderDnd.testkit";

vi.mock("./ipc", () => ({
  AutomationAPI: {
    complete: vi.fn(() => Promise.resolve()),
    enabled: vi.fn(() => Promise.resolve(false)),
    frontendReady: vi.fn(() => Promise.resolve()),
    executeTerminalController: vi.fn(),
    resetTerminalControllerForTests: vi.fn(),
  },
  onUiAutomationRequest: vi.fn(() => Promise.resolve(() => {})),
}));

type Box = { left: number; top: number; width: number; height: number };

/** jsdom has no layout: give `el` a box and a one-item client-rect list. */
function visible(el: Element, box: Box | DOMRect): void {
  const rect = {
    ...box,
    x: box.left,
    y: box.top,
    right: box.left + box.width,
    bottom: box.top + box.height,
  } as DOMRect;
  el.getBoundingClientRect = () => rect;
  el.getClientRects = () => Object.assign([rect], { item: () => rect }) as unknown as DOMRectList;
}

function send(action: UiAutomationAction, selector: string, value?: string) {
  const request: UiAutomationRequest = {
    requestId: `request-${action}-${selector}-${value}`,
    token: "token",
    window: "main",
    action,
    selector,
    value,
    expiresAtUnixMs: null,
  } as UiAutomationRequest;
  return executeAutomationRequest("main", request) as Promise<any>;
}

const pointer = (value: string, selector = "") => send("pointer", selector, value);

let dispose: () => void = () => {};
const throwers: Array<[string, EventListener]> = [];

function throwOnWindow(type: string, message: string): void {
  const fn: EventListener = () => {
    throw new Error(message);
  };
  window.addEventListener(type, fn);
  throwers.push([type, fn]);
}

function plainTarget(testId: string, tag = "button"): HTMLElement {
  const el = document.createElement(tag);
  el.setAttribute("data-ac-testid", testId);
  document.body.append(el);
  visible(el, { left: 300, top: 300, width: 40, height: 20 });
  return el;
}

/** Fixture F: the real drag controller behind a SettingsModal-shaped list. */
function mountList() {
  const commit = vi.fn();
  const announce = vi.fn();
  let container!: HTMLDivElement;
  let drag!: AgentDragReorder;
  vi.stubGlobal("requestAnimationFrame", vi.fn(() => 1));
  vi.stubGlobal("cancelAnimationFrame", vi.fn());

  const List = () => {
    drag = createAgentDragReorder({
      container: () => container,
      rowSelector: ".settings-agent-row",
      handleSelector: ".settings-agent-drag-handle",
      canDrag: () => true,
      commit,
      announce,
    });
    return (
      <div ref={container}>
        <For each={["a0", "a1", "a2"]}>
          {(id, i) => (
            <div class="settings-agent-row" data-ac-testid={`t.row.${i()}`}>
              <button
                class="settings-agent-drag-handle"
                data-ac-testid={`t.grip.${i()}`}
                onPointerDown={(e) => drag.onPointerDown(e, id)}
                onPointerMove={drag.onPointerMove}
                onPointerUp={drag.onPointerUp}
                onPointerCancel={drag.onPointerCancel}
              />
            </div>
          )}
        </For>
      </div>
    );
  };
  dispose = render(() => <List />, document.body);

  const rows = [...container.querySelectorAll<HTMLElement>(".settings-agent-row")];
  const grips = [...container.querySelectorAll<HTMLElement>(".settings-agent-drag-handle")];
  stubAgentRowGeometry(rows, grips[0]);
  let held = false;
  const grip0 = grips[0];
  grip0.setPointerCapture = vi.fn((id: number) => {
    if (id !== 1) throw new DOMException("x", "NotFoundError");
    held = true;
  });
  grip0.releasePointerCapture = vi.fn(() => { held = false; });
  grip0.hasPointerCapture = vi.fn(() => held);
  container.getBoundingClientRect = () =>
    ({ top: 0, bottom: 120, height: 120, left: 0, right: 200, width: 200, x: 0, y: 0 }) as DOMRect;
  rows.forEach((row) => visible(row, row.getBoundingClientRect()));
  grips.forEach((grip, i) => visible(grip, { left: 10, top: 10 + i * 40, width: 20, height: 20 }));
  return { commit, announce, drag, grip0 };
}

beforeEach(() => {
  delete (document as any).elementFromPoint;
  resetAutomationBridgeForTests();
});

afterEach(() => {
  dispose();
  dispose = () => {};
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  resetAutomationBridgeForTests();
  for (const [type, fn] of throwers.splice(0)) window.removeEventListener(type, fn);
  delete (document as any).elementFromPoint;
});

describe("pointer and key actions", () => {
  it("T1 drags a row to the end and commits once", async () => {
    const f = mountList();
    const down = await pointer("down", "t.grip.0");
    expect(down.ok).toBe(true);
    expect(down.diagnostics.pointer).toMatchObject({
      operation: "down", position: null, pointerId: 1, source: "t.grip.0", at: null, defaultPrevented: true,
    });
    expect(f.grip0.setPointerCapture).toHaveBeenCalledWith(1);

    const move = await pointer("move:bottom", "t.row.2");
    expect(move.ok).toBe(true);
    expect(move.target.testId).toBe("t.row.2");
    expect(move.diagnostics.pointer).toMatchObject({ position: "bottom", at: "t.row.2", clientY: 119 });
    expect(f.drag.dropIndicatorTop()).not.toBeNull();

    const up = await pointer("up");
    expect(up.ok).toBe(true);
    expect(up.target.testId).toBe("t.grip.0");
    expect(f.commit).toHaveBeenCalledTimes(1);
    expect(f.commit).toHaveBeenCalledWith("a0", 2);
    expect(f.grip0.releasePointerCapture).toHaveBeenCalledWith(1);
  });

  it("T2 Escape cancels the drag", async () => {
    const f = mountList();
    await pointer("down", "t.grip.0");
    await pointer("move", "t.row.2");
    const key = await send("key", "t.grip.0", "Escape");
    expect(key.ok).toBe(true);
    expect(key.diagnostics.key.defaultPrevented).toBe(true);
    expect(f.announce).toHaveBeenCalledWith("Move cancelled.");
    expect((await pointer("up")).ok).toBe(true);
    expect(f.commit).not.toHaveBeenCalled();
  });

  it("T3 pointer cancel aborts and clears the button", async () => {
    const f = mountList();
    await pointer("down", "t.grip.0");
    await pointer("move", "t.row.2");
    const cancel = await pointer("cancel");
    expect(cancel.ok).toBe(true);
    expect(f.commit).not.toHaveBeenCalled();
    expect(f.drag.dropIndicatorTop()).toBeNull();
    expect((await pointer("down", "t.grip.0")).ok).toBe(true);
  });

  it("T4 dispatches key chords with focus and no default actions", async () => {
    const button = plainTarget("t.key");
    const seen: KeyboardEvent[] = [];
    button.onkeydown = (e) => { seen.push(e); };
    const res = await send("key", "t.key", "Alt+ArrowUp");
    expect(res.ok).toBe(true);
    expect(seen).toHaveLength(1);
    expect(seen[0]).toMatchObject({ altKey: true, ctrlKey: false, key: "ArrowUp", code: "ArrowUp", bubbles: true, composed: true });
    expect(document.activeElement).toBe(button);
    expect(res.diagnostics.key).toMatchObject({ focused: true, events: ["keydown", "keyup"], altKey: true });

    const space = await send("key", "t.key", "Space");
    expect(space.diagnostics.key).toMatchObject({ key: " ", code: "Space" });
    expect(seen[1]).toMatchObject({ key: " ", code: "Space" });
  });

  it("T5 reports pointer and key errors", async () => {
    const a = plainTarget("t.a");
    plainTarget("t.b");
    expect((await pointer("move", "t.b")).error).toBe("pointer_not_down");
    expect((await pointer("down", "t.a")).ok).toBe(true);
    expect((await pointer("down", "t.b")).error).toBe("pointer_already_down");
    expect((await pointer("up", "t.b")).error).toBe("value_not_supported");

    const overlay = plainTarget("t.overlay", "div");
    Object.defineProperty(document, "elementFromPoint", { configurable: true, value: () => overlay });
    expect((await pointer("move", "t.b")).ok).toBe(true);

    a.remove();
    expect((await pointer("up")).error).toBe("pointer_source_detached");
    expect((await pointer("up")).error).toBe("pointer_not_down");

    expect((await pointer("drag", "t.b")).error).toBe("value_not_supported");
    expect((await send("key", "t.b", "Shift+Ctrl+Escape")).error).toBe("value_not_supported");
    expect((await send("key", "t.b", "KeyA")).error).toBe("value_not_supported");

    delete (document as any).elementFromPoint;
    plainTarget("t.disabled").setAttribute("disabled", "");
    expect((await pointer("down", "t.disabled")).error).toBe("target_disabled");
  });

  it("T5x-a pointerdown listener throw still arms; cancel recovers", async () => {
    const f = mountList();
    throwOnWindow("pointerdown", "boom-down");
    const down = await pointer("down", "t.grip.0");
    expect(down.error).toBe("listener_exception");
    expect(down.message).toContain("boom-down");
    expect(down.diagnostics.pointer.defaultPrevented).toBe(true);
    throwers.splice(0).forEach(([type, fn]) => window.removeEventListener(type, fn));

    expect((await pointer("move", "t.row.2")).ok).toBe(true);
    expect(f.drag.dropIndicatorTop()).not.toBeNull();
    expect((await pointer("cancel")).ok).toBe(true);
    expect(f.drag.dropIndicatorTop()).toBeNull();
    expect(f.commit).not.toHaveBeenCalled();
    expect((await pointer("down", "t.grip.0")).ok).toBe(true);
  });

  it("T5x-b pointerup listener throw after commit clears the button", async () => {
    const f = mountList();
    await pointer("down", "t.grip.0");
    await pointer("move:bottom", "t.row.2");
    throwOnWindow("pointerup", "boom-up");
    expect((await pointer("up")).error).toBe("listener_exception");
    expect(f.commit).toHaveBeenCalledTimes(1);
    expect((await pointer("move", "t.row.1")).error).toBe("pointer_not_down");
    expect((await pointer("down", "t.grip.0")).ok).toBe(true);
  });

  it("T5x-c pointercancel listener throw still cancels and clears", async () => {
    const f = mountList();
    await pointer("down", "t.grip.0");
    await pointer("move", "t.row.2");
    throwOnWindow("pointercancel", "boom-cancel");
    expect((await pointer("cancel")).error).toBe("listener_exception");
    expect(f.commit).not.toHaveBeenCalled();
    expect(f.drag.dropIndicatorTop()).toBeNull();
    expect((await pointer("down", "t.grip.0")).ok).toBe(true);
  });

  it("T5x-d a capture thrower before the controller behaves like a)", async () => {
    const f = mountList();
    const fn = () => {
      throw new Error("boom-before");
    };
    f.grip0.addEventListener("pointerdown", fn, { capture: true });
    expect((await pointer("down", "t.grip.0")).error).toBe("listener_exception");
    f.grip0.removeEventListener("pointerdown", fn, { capture: true });
    expect((await pointer("move", "t.row.2")).ok).toBe(true);
    expect(f.drag.dropIndicatorTop()).not.toBeNull();
    expect((await pointer("cancel")).ok).toBe(true);
    expect(f.commit).not.toHaveBeenCalled();
  });

  it("T6 pointer events carry the mouse identity and go to the source", async () => {
    const source = plainTarget("t.src");
    plainTarget("t.dst");
    const events: PointerEvent[] = [];
    const record = (e: Event) => events.push(e as PointerEvent);
    const types = ["pointerdown", "pointermove", "pointerup", "pointercancel"];
    types.forEach((type) => window.addEventListener(type, record, true));
    try {
      await pointer("down", "t.src");
      await pointer("move", "t.dst");
      await pointer("up");
      await pointer("down", "t.src");
      await pointer("cancel");
    } finally {
      types.forEach((type) => window.removeEventListener(type, record, true));
    }
    expect(events.map((e) => [e.type, e.button, e.buttons, e.cancelable])).toEqual([
      ["pointerdown", 0, 1, true],
      ["pointermove", -1, 1, true],
      ["pointerup", 0, 0, true],
      ["pointerdown", 0, 1, true],
      ["pointercancel", -1, 0, false],
    ]);
    for (const e of events) {
      expect([e.pointerId, e.pointerType, e.isPrimary, e.target]).toEqual([1, "mouse", true, source]);
      expect(e.bubbles && e.composed).toBe(true);
    }
  });
});
