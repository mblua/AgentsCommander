// @vitest-environment jsdom
// #4810 - the one sidebar filter row follows the stuck group header.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot } from "solid-js";
import { createSidebarFilterHost, pickHostIndex, placeFilterRow } from "./sidebar-filter-host";
import { scanSheet, soleRuleBody } from "../styles/css-test-helpers";

const ROW = "sidebar.regexFilter.row";
const SLOT = "sidebar.regexFilter.errorSlot";

// Rect per `data-test-box`: scroller top is the edge (0); panels get [top, bottom].
let boxes: Record<string, [number, number]> = {};
const rect = (top: number, bottom: number) =>
  ({ top, bottom, left: 0, right: 0, width: 0, height: bottom - top, x: 0, y: top }) as DOMRect;

function panel(box: string): HTMLElement {
  const p = document.createElement("div");
  p.className = "project-panel";
  p.dataset.testBox = box;
  const h = document.createElement("div");
  h.className = "project-header";
  p.appendChild(h);
  return p;
}

function build(withSlot = true) {
  const scroller = document.createElement("div");
  scroller.dataset.testBox = "scroller";
  const panels = ["p0", "p1", "p2"].map(panel);
  scroller.append(...panels);
  const row = document.createElement("div");
  row.setAttribute("data-ac-testid", ROW);
  const input = document.createElement("input");
  row.appendChild(input);
  const header = (i: number) => panels[i]!.firstElementChild as HTMLElement;
  header(0).appendChild(row);
  const slot = document.createElement("div");
  slot.setAttribute("data-ac-testid", SLOT);
  if (withSlot) header(0).appendChild(slot);
  const outside = document.createElement("button");
  document.body.append(scroller, outside);
  input.value = "room-2";
  return { scroller, panels, row, input, slot, header, outside };
}

const SPANS: Record<number, Record<string, [number, number]>> = {
  0: { p0: [0, 100], p1: [100, 200], p2: [200, 300] },
  1: { p0: [-100, -50], p1: [-50, 50], p2: [50, 150] },
};

describe("#4810 sidebar filter host", () => {
  beforeEach(() => {
    boxes = { scroller: [0, 300], ...SPANS[0] };
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (
      this: Element,
    ) {
      const box = (this as HTMLElement).dataset?.testBox;
      const b = box === undefined ? undefined : boxes[box];
      return b ? rect(b[0], b[1]) : rect(0, 0);
    });
  });
  afterEach(() => {
    vi.restoreAllMocks();
    document.body.replaceChildren();
  });

  it("H1 pickHostIndex picks the panel spanning the edge", () => {
    const p = [
      { top: 0, bottom: 100 },
      { top: 100, bottom: 200 },
      { top: 200, bottom: 300 },
    ];
    expect(pickHostIndex(p, 50)).toBe(0);
    expect(pickHostIndex(p, 250)).toBe(2);
    expect(pickHostIndex(p, 99)).toBe(1); // edge + 1 = panel 1's top
    expect(pickHostIndex(p, 100)).toBe(1);
    expect(pickHostIndex([{ top: 0, bottom: 10 }, { top: 20, bottom: 30 }], 14)).toBe(0);
    expect(pickHostIndex(p, 500)).toBe(0);
    expect(pickHostIndex([], 0)).toBe(0);
  });

  it("H2 placeFilterRow moves row and slot, keeps focus, value and selection", () => {
    const { scroller, row, input, slot, header } = build();
    input.focus();
    input.setSelectionRange(2, 4);
    boxes = { ...boxes, ...SPANS[1] };
    placeFilterRow(scroller);
    expect(row.parentElement).toBe(header(1));
    expect(slot.parentElement).toBe(header(1));
    expect(row.nextElementSibling).toBe(slot);
    expect(document.activeElement).toBe(input);
    expect(input.value).toBe("room-2");
    expect([input.selectionStart, input.selectionEnd]).toEqual([2, 4]);
  });

  it("H2b unfocused input moves without stealing focus; no write when already hosted", () => {
    const { scroller, row, header, outside } = build();
    outside.focus();
    boxes = { ...boxes, ...SPANS[1] };
    placeFilterRow(scroller);
    expect(row.parentElement).toBe(header(1));
    expect(document.activeElement).toBe(outside);
    const append = vi.spyOn(Element.prototype, "append");
    const appendChild = vi.spyOn(Node.prototype, "appendChild");
    placeFilterRow(scroller);
    expect(append).not.toHaveBeenCalled();
    expect(appendChild).not.toHaveBeenCalled();
  });

  it("H2c without a slot only the row moves", () => {
    const { scroller, row, header } = build(false);
    boxes = { ...boxes, ...SPANS[1] };
    placeFilterRow(scroller);
    expect(row.parentElement).toBe(header(1));
  });

  it("H3 a detached row is re-hosted in the header picked by the rects", () => {
    const { scroller, panels, row, slot, header } = build();
    boxes = { ...boxes, ...SPANS[1] };
    placeFilterRow(scroller);
    expect(row.parentElement).toBe(header(1));
    panels[1]!.remove();
    boxes = { ...boxes, ...SPANS[0] };
    placeFilterRow(scroller);
    expect(row.parentElement).toBe(header(0));
    expect(slot.parentElement).toBe(header(0));
    expect(row.nextElementSibling).toBe(slot);
  });

  it("H5 a re-created host keeps focus and selection; a real blur does not", async () => {
    const { scroller, panels, row, input, slot, outside } = build();
    boxes = { ...boxes, ...SPANS[1] };
    const removeSpy = vi.spyOn(input, "removeEventListener");
    let dispose!: () => void;
    createRoot((d) => {
      dispose = d;
      createSidebarFilterHost(() => scroller);
    });
    await Promise.resolve();
    expect(row.parentElement).toBe(panels[1]!.firstElementChild);
    input.focus();
    input.setSelectionRange(2, 4);

    const fresh = panel("p1");
    scroller.replaceChild(fresh, panels[1]!);
    await vi.waitFor(() => expect(row.parentElement).toBe(fresh.firstElementChild));
    expect(row.nextElementSibling).toBe(slot);
    expect(document.activeElement).toBe(input);
    expect(input.value).toBe("room-2");
    expect([input.selectionStart, input.selectionEnd]).toEqual([2, 4]);

    outside.focus();
    await Promise.resolve();
    const again = panel("p1");
    scroller.replaceChild(again, fresh);
    await vi.waitFor(() => expect(row.parentElement).toBe(again.firstElementChild));
    expect(document.activeElement).toBe(outside);

    dispose();
    const types = removeSpy.mock.calls.map((c) => c[0]);
    expect(types).toContain("focusin");
    expect(types).toContain("focusout");
  });

  // Vite rewrites the literal `new URL(..., import.meta.url)` form; keep it indirect.
  const moduleUrl = import.meta.url;
  const sheet = () => scanSheet(new URL("../styles/sidebar.css", moduleUrl));

  it("H4 the title fade follows the hosted open row", () => {
    const rules = sheet();
    const body = soleRuleBody(rules, ".project-header:has(> .project-filter-row.open) .project-title");
    expect(body).toMatch(/opacity:\s*0/);
    expect(rules.some((r) => r.selectors.includes(".project-header.open .project-title"))).toBe(
      false,
    );
  });

  it("H6 the error slot hangs under the host, opaque, and stays clickable", () => {
    const rules = sheet();
    const slot = soleRuleBody(rules, ".project-header > .project-filter-error-slot");
    expect(slot).toMatch(/position:\s*absolute/);
    expect(slot).toMatch(/top:\s*100%/);
    const inherit = slot.search(/background:\s*inherit/);
    const color = slot.search(/background-color:\s*var\(--sidebar-bg\)/);
    expect(inherit).toBeGreaterThanOrEqual(0);
    expect(color).toBeGreaterThan(inherit);
    expect(slot).not.toMatch(/pointer-events/);
    expect(soleRuleBody(rules, ".project-filter-error")).not.toMatch(/pointer-events/);
    const lift = soleRuleBody(
      rules,
      ".project-panel:has(> .project-header > .project-filter-error-slot > .project-filter-error)",
    );
    expect(lift).toMatch(/position:\s*relative/);
    expect(lift).toMatch(/z-index:\s*22/);
  });
});
