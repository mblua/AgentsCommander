import { Component, Show } from "solid-js";
import { isBrowser } from "../../shared/platform";
import { sidebarCompact } from "../../shared/sidebar-compact";
import { sessionsStore } from "../../sidebar/stores/sessions";
import { tutorialStore } from "../stores/tutorial";
import { tutorialProgressStore } from "../stores/tutorial-progress";
import { TUTORIAL_LINE, TUTORIAL_TITLE } from "../tutorials/create-first-team";

// #5228 - the Tutorials section at the top of Home. One card, desktop only.
function blockReason(): string | null {
  if (sidebarCompact()) return "Expand the sidebar to use this tutorial.";
  if (!sessionsStore.showCategories) return "Show category sections to use this tutorial.";
  return null;
}

const HomeTutorials: Component = () => {
  if (isBrowser) return null;
  const done = () => tutorialProgressStore.completed;
  return (
    <section class="home-tutorials" data-ac-testid="home.tutorials">
      <h3>Tutorials</h3>
      <div
        class="home-tutorial-card"
        data-ac-testid="home.tutorial.team"
        data-ac-state={done() ? "completed" : "new"}
      >
        <strong>
          {TUTORIAL_TITLE}
          <Show when={done()}>
            <span class="home-tutorial-done">Completed</span>
          </Show>
        </strong>
        <p>{TUTORIAL_LINE}</p>
        <Show when={blockReason()}>
          {(reason) => <p class="home-tutorial-note">{reason()}</p>}
        </Show>
        <button
          type="button"
          data-ac-testid="home.tutorial.team.start"
          disabled={blockReason() !== null || tutorialStore.active}
          onClick={() => tutorialStore.start()}
        >
          {done() ? "Start again" : "Start"}
        </button>
      </div>
    </section>
  );
};

export default HomeTutorials;
