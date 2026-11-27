import { Component, Show, createMemo } from "solid-js";
import { homeStore } from "../stores/home";
import { tutorialStore } from "../stores/tutorial";
import { tutorialProgressStore } from "../stores/tutorial-progress";
import { IDS } from "../tutorials/create-first-team";

// #5228 - "Check the tutorials" bubble next to the Home button.
const TutorialHint: Component<{ tick: number }> = (props) => {
  const rect = createMemo(() => {
    void props.tick;
    const p = tutorialProgressStore;
    if (!p.loaded || p.hintDismissed || p.completed) return null;
    if (homeStore.visible || tutorialStore.active) return null;
    const el = document.querySelector(`[data-ac-testid="${IDS.home}"]`);
    return el ? el.getBoundingClientRect() : null;
  });
  return (
    <Show when={rect()}>
      {(r) => (
        <div
          class="tutorial-bubble tutorial-hint"
          data-ac-testid="tutorial.hint"
          style={{ top: `${r().bottom + 8}px`, left: `${Math.max(8, r().left)}px` }}
        >
          Check the tutorials
          <button
            type="button"
            data-ac-testid="tutorial.hint.close"
            aria-label="Close"
            onClick={() => void tutorialProgressStore.dismissHint()}
          >
            ×
          </button>
        </div>
      )}
    </Show>
  );
};

export default TutorialHint;
