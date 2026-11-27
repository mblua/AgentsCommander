import { Component, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";
import { isBrowser } from "../../shared/platform";
import { projectStore } from "../../sidebar/stores/project";
import { rootMessageStore } from "../stores/root-message-tutorial";
import { tutorialStore } from "../stores/tutorial";
import { tutorialProgressStore } from "../stores/tutorial-progress";
import type { Probe } from "../tutorials/create-first-team";
import RootMessageOverlay from "./RootMessageOverlay";
import TutorialHint from "./TutorialHint";
import TutorialOverlay from "./TutorialOverlay";

// #5228 - mounted once in the main window. Watches the DOM only while a run is
// active or the hint can show; every trigger coalesces into one frame.
const find = (id: string) => document.querySelector(`[data-ac-testid="${id}"]`);

const probe: Probe = {
  has: (id) => find(id) !== null,
  state: (id) => find(id)?.getAttribute("data-ac-state") ?? null,
  value: (id) => find(id)?.getAttribute("data-ac-value") ?? null,
  matches: (selector) => document.querySelector(selector) !== null,
  get projects() { return projectStore.projects; },
  get outcome() { return projectStore.newProjectOutcome; },
};

const OBSERVE: MutationObserverInit = {
  subtree: true, childList: true, attributes: true,
  attributeFilter: ["data-ac-state", "data-ac-testid", "data-ac-value", "title"],
};

const TutorialLayer: Component = () => {
  if (isBrowser) return null;
  const [tick, setTick] = createSignal(0);
  let frame = 0;
  const flush = () => {
    frame = 0;
    if (tutorialStore.active) tutorialStore.sync(probe);
    setTick((t) => t + 1);
  };
  const schedule = () => {
    if (!frame) frame = requestAnimationFrame(flush);
  };
  const watching = () => {
    const p = tutorialProgressStore;
    if (rootMessageStore.active) return true;
    return tutorialStore.active || (p.loaded && !p.hintDismissed && !p.completed);
  };
  onMount(() => void tutorialProgressStore.load());
  createEffect(() => {
    if (!watching()) return;
    const observer = new MutationObserver(schedule);
    observer.observe(document.body, OBSERVE);
    window.addEventListener("resize", schedule);
    window.addEventListener("scroll", schedule, true);
    schedule();
    onCleanup(() => {
      observer.disconnect();
      window.removeEventListener("resize", schedule);
      window.removeEventListener("scroll", schedule, true);
      if (frame) cancelAnimationFrame(frame);
      frame = 0;
    });
  });
  createEffect(() => {
    void projectStore.projects;
    void projectStore.newProjectOutcome;
    void tutorialStore.ctx;
    if (watching()) schedule();
  });
  return (
    <Portal>
      <TutorialHint tick={tick()} />
      <Show when={tutorialStore.active}>
        <TutorialOverlay tick={tick()} probe={probe} />
      </Show>
      <Show when={rootMessageStore.active}>
        <RootMessageOverlay tick={tick()} />
      </Show>
    </Portal>
  );
};

export default TutorialLayer;
