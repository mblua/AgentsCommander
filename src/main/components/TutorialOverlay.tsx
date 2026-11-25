import { Component, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import type { JSX } from "solid-js";
import { tutorialStore } from "../stores/tutorial";
import { HIDDEN, view, type Probe } from "../tutorials/create-first-team";

// #5228 - ring around the real control and the step bubble. The ring never
// takes clicks; there is no keyboard handler (Esc never ends the tutorial).
const MISSING_MS = 2000;
const GAP = 8;
const BUBBLE_W = 260;

const TutorialOverlay: Component<{ tick: number; probe: Probe }> = (props) => {
  let bubble: HTMLDivElement | undefined;
  const v = createMemo(() => {
    const ctx = tutorialStore.ctx;
    return ctx ? view(ctx, props.probe) : null;
  });
  const rect = createMemo(() => {
    void props.tick;
    const target = v()?.target;
    const el = target ? document.querySelector(target) : null;
    return el ? el.getBoundingClientRect() : null;
  });
  const [hidden, setHidden] = createSignal(false);
  // Same step and target, still missing: one key, one timer.
  const missingKey = createMemo(() => {
    const cur = v();
    if (!cur?.target || cur.final || rect() !== null) return "";
    return `${tutorialStore.ctx?.step}|${cur.target}`;
  });
  createEffect(() => {
    if (missingKey() === "") return setHidden(false);
    const timer = setTimeout(() => setHidden(true), MISSING_MS);
    onCleanup(() => clearTimeout(timer));
  });
  const bubbleStyle = (): JSX.CSSProperties => {
    const r = rect();
    if (!r) return { right: "24px", bottom: "24px" };
    const h = bubble?.offsetHeight ?? 0;
    const left = Math.max(GAP, Math.min(r.left, window.innerWidth - BUBBLE_W - GAP));
    const top = Math.max(GAP, Math.min(r.bottom + GAP, window.innerHeight - h - GAP));
    return { left: `${left}px`, top: `${top}px` };
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
              <Show
                when={cur().final}
                fallback={
                  <button type="button" data-ac-testid="tutorial.exit"
                    onClick={() => tutorialStore.exit()}>Exit tutorial</button>
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
