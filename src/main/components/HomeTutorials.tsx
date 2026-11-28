import { Component, Show } from "solid-js";
import { isBrowser } from "../../shared/platform";
import { sidebarCompact } from "../../shared/sidebar-compact";
import { sessionsStore } from "../../sidebar/stores/sessions";
import { rootMessageStore } from "../stores/root-message-tutorial";
import { tutorialStore } from "../stores/tutorial";
import { tutorialProgressStore } from "../stores/tutorial-progress";
import { TUTORIAL_LINE, TUTORIAL_TITLE } from "../tutorials/create-first-team";
import { RM_LINE, RM_TITLE } from "../tutorials/root-agent-message";

// #5228 - the Tutorials section at the top of Home. Desktop only.
// #5287 - second card: one message to the Root Agent.
function blockReason(): string | null {
  if (sidebarCompact()) return "Expand the sidebar to use this tutorial.";
  if (!sessionsStore.showCategories) return "Show category sections to use this tutorial.";
  return null;
}

const HomeTutorials: Component = () => {
  if (isBrowser) return null;
  const done = () => tutorialProgressStore.completed;
  const running = () => tutorialStore.active || rootMessageStore.active;
  return (
    <section class="home-tutorials" data-ac-testid="home.tutorials">
      <h3>Tutorials</h3>
      <div class="home-tutorial-cards">
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
            disabled={blockReason() !== null || running()}
            onClick={() => tutorialStore.start()}
          >
            {done() ? "Start again" : "Start"}
          </button>
        </div>
        <div class="home-tutorial-card" data-ac-testid="home.tutorial.rootMessage">
          <strong>{RM_TITLE}</strong>
          <p>{RM_LINE}</p>
          <button
            type="button"
            data-ac-testid="home.tutorial.rootMessage.start"
            disabled={running()}
            onClick={() => rootMessageStore.start()}
          >
            Start
          </button>
        </div>
      </div>
    </section>
  );
};

export default HomeTutorials;
