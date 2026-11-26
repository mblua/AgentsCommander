import { Component, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import type { JSX } from "solid-js";
import { tutorialStore } from "../stores/tutorial";
import { requestExternalLinkConfirmation } from "../../shared/external-links";
import {
  AGENCY_REPO_URL, HIDDEN, LINK_LABEL, view, type Probe,
} from "../tutorials/create-first-team";

// #5228 - ring around the real control and the step bubble. The ring never
// takes clicks; there is no keyboard handler (Esc never ends the tutorial).
const MISSING_MS = 2000;
const GAP = 8;
const BUBBLE_W = 260;

export type Box = { left: number; top: number; right: number; bottom: number };
/** Bubble position that never overlaps the target box r (when the window allows it). */
export function bubblePos(r: Box, h: number, vw: number, vh: number): { left: number; top: number } {
  const clampX = (x: number) => Math.max(GAP, Math.min(x, vw - BUBBLE_W - GAP));
  const clampY = (y: number) => Math.max(GAP, Math.min(y, vh - h - GAP));
  const below = r.bottom + GAP;
  if (below + h <= vh - GAP) return { left: clampX(r.left), top: below };
  const above = r.top - GAP - h;
  if (above >= GAP) return { left: clampX(r.left), top: above };
  const right = r.right + GAP;
  if (right + BUBBLE_W <= vw - GAP) return { left: right, top: clampY(r.top) };
  const left = r.left - GAP - BUBBLE_W;
  if (left >= GAP) return { left, top: clampY(r.top) };
  return { left: clampX(r.left), top: clampY(below) };
}

const AREA = '[data-ac-role="dialog"], [data-ac-role="menu"], .session-context-menu';
export type Area = { dialog: boolean; box: Box };
export const union = (a: Box, b: Box): Box => ({
  left: Math.min(a.left, b.left), top: Math.min(a.top, b.top),
  right: Math.max(a.right, b.right), bottom: Math.max(a.bottom, b.bottom),
});
/** Rule A (owner): above the dialog; else outside right; else outside left;
 *  else inside, below the field group g; else bubblePos around g. */
export function dialogPos(t: Box, d: Box, g: Box, h: number, vw: number, vh: number) {
  const clampX = (x: number) => Math.max(GAP, Math.min(x, vw - BUBBLE_W - GAP));
  const clampY = (y: number) => Math.max(GAP, Math.min(y, vh - h - GAP));
  const above = d.top - GAP - h;
  if (above >= GAP) return { left: clampX(t.left), top: above };
  const right = d.right + GAP;
  if (right + BUBBLE_W <= vw - GAP) return { left: right, top: clampY(t.top) };
  const left = d.left - GAP - BUBBLE_W;
  if (left >= GAP) return { left, top: clampY(t.top) };
  const below = g.bottom + GAP;
  if (below + h <= vh - GAP) return { left: clampX(t.left), top: below };
  return bubblePos(g, h, vw, vh);
}
/** g = target, joined with its field group when the step has one. */
export function placeBubble(t: Box, g: Box, area: Area | null, h: number, vw: number, vh: number) {
  if (area?.dialog) return dialogPos(t, area.box, g, h, vw, vh);
  return bubblePos(area ? union(g, area.box) : g, h, vw, vh);
}

const TutorialOverlay: Component<{ tick: number; probe: Probe }> = (props) => {
  let bubble: HTMLDivElement | undefined;
  const v = createMemo(() => {
    void props.tick; // view() reads the DOM (e.g. team wizard step), so re-run per frame
    const ctx = tutorialStore.ctx;
    return ctx ? view(ctx, props.probe) : null;
  });
  const rect = createMemo(() => {
    void props.tick;
    const target = v()?.target;
    const el = target ? document.querySelector(target) : null;
    return el ? el.getBoundingClientRect() : null;
  });
  const around = createMemo(() => {
    void props.tick;
    const cur = v();
    const el = cur?.target ? document.querySelector(cur.target) : null;
    if (!el) return null;
    let g: Box = el.getBoundingClientRect();
    const gr = cur?.group ? document.querySelector(cur.group)?.getBoundingClientRect() : null;
    if (gr && gr.width > 0 && gr.height > 0) g = union(g, gr);
    const a = el.closest(AREA);
    const area = a
      ? { dialog: a.matches('[data-ac-role="dialog"]'), box: a.getBoundingClientRect() }
      : null;
    return { g, area };
  });
  const [hidden, setHidden] = createSignal(false);
  // Same step and target, still missing: one key, one timer.
  const missingKey = createMemo(() => {
    const cur = v();
    if (!cur?.target || cur.final || cur.next || rect() !== null) return "";
    return `${tutorialStore.ctx?.step}|${cur.target}`;
  });
  createEffect(() => {
    if (missingKey() === "") return setHidden(false);
    const timer = setTimeout(() => setHidden(true), MISSING_MS);
    onCleanup(() => clearTimeout(timer));
  });
  // Height is re-read each tick; flush() runs sync() before setTick().
  const bubbleStyle = (): JSX.CSSProperties => {
    const r = rect();
    if (!r) return { right: "24px", bottom: "24px" };
    const h = bubble?.offsetHeight ?? 0;
    const ar = around();
    const { innerWidth: vw, innerHeight: vh } = window;
    const p = placeBubble(r, ar?.g ?? r, ar?.area ?? null, h, vw, vh);
    return { left: `${p.left}px`, top: `${p.top}px` };
  };
  return (
    <Show when={v()}>
      {(cur) => (
        <>
          <Show when={rect()}>
            {(r) => (
              <div
                class="tutorial-ring"
                data-ac-testid="tutorial.ring"
                style={{
                  top: `${r().top - 4}px`, left: `${r().left - 4}px`,
                  width: `${r().width + 8}px`, height: `${r().height + 8}px`,
                }}
              />
            )}
          </Show>
          <div
            ref={bubble}
            class="tutorial-bubble"
            classList={{ "tutorial-bubble--warn": cur().warn }}
            data-ac-testid="tutorial.bubble"
            data-ac-state={cur().warn ? "warn" : undefined}
            style={bubbleStyle()}
          >
            <p>{hidden() ? HIDDEN : cur().text}</p>
            <div class="tutorial-bubble-actions">
              <Show when={cur().link}>
                <button type="button" data-ac-testid="tutorial.link"
                  onClick={() => requestExternalLinkConfirmation(AGENCY_REPO_URL)}>
                  {LINK_LABEL}
                </button>
              </Show>
              <Show when={cur().skip}>
                <button type="button" data-ac-testid="tutorial.skip"
                  onClick={() => tutorialStore.skipTemplates()}>Skip download</button>
              </Show>
              <Show
                when={cur().final}
                fallback={
                  <Show
                    when={cur().next}
                    fallback={
                      <button type="button" data-ac-testid="tutorial.exit"
                        onClick={() => tutorialStore.exit()}>Exit tutorial</button>
                    }
                  >
                    <button type="button" data-ac-testid="tutorial.next"
                      onClick={() => tutorialStore.showOrchestrators()}>Next</button>
                  </Show>
                }
              >
                <button type="button" data-ac-testid="tutorial.done"
                  onClick={() => void tutorialStore.finish()}>Done</button>
              </Show>
            </div>
          </div>
        </>
      )}
    </Show>
  );
};

export default TutorialOverlay;
