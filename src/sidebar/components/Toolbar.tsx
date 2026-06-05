import { Component, createEffect, createSignal, onCleanup, Show } from "solid-js";
import { projectStore } from "../stores/project";
import OpenAgentModal from "./OpenAgentModal";
import NewAgentModal from "./NewAgentModal";

const Toolbar: Component = () => {
  const [showOpenAgent, setShowOpenAgent] = createSignal(false);
  const [showNewAgent, setShowNewAgent] = createSignal(false);
  const [confirmPath, setConfirmPath] = createSignal<string | null>(null);

  const handleOpenProject = async () => {
    const { picked, hasWorkspace } = await projectStore.pickAndCheck();
    if (!picked) return;
    if (!hasWorkspace) {
      setConfirmPath(picked);
    }
  };

  const handleConfirmCreate = async () => {
    const path = confirmPath();
    if (path) {
      await projectStore.createAndLoad(path);
      setConfirmPath(null);
    }
  };

  const onConfirmKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") setConfirmPath(null);
  };

  createEffect(() => {
    if (confirmPath()) {
      window.addEventListener("keydown", onConfirmKeyDown);
    } else {
      window.removeEventListener("keydown", onConfirmKeyDown);
    }
  });

  onCleanup(() => window.removeEventListener("keydown", onConfirmKeyDown));

  return (
    <>
      <div class="toolbar">
        <button
          class="toolbar-btn toolbar-btn-project"
          onClick={handleOpenProject}
          title="Open an AC project folder"
        >
          &#x1F4C2; Open Project
        </button>
        <button
          class="toolbar-btn"
          onClick={() => setShowNewAgent(true)}
        >
          &#x2795; New Agent
        </button>
        <button
          class="toolbar-btn"
          onClick={() => setShowOpenAgent(true)}
        >
          &#x25B6; Open Agent
        </button>
      </div>
      {showOpenAgent() && (
        <OpenAgentModal onClose={() => setShowOpenAgent(false)} />
      )}
      {showNewAgent() && (
        <NewAgentModal onClose={() => setShowNewAgent(false)} />
      )}
      <Show when={confirmPath()}>
        <div class="confirm-overlay">
          <div class="confirm-dialog" onClick={(e) => e.stopPropagation()}>
            <p class="confirm-text">
              This folder does not have an AC project. Do you want to create a new project here?
            </p>
            <p class="confirm-path">{confirmPath()}</p>
            <div class="confirm-actions">
              <button class="confirm-btn confirm-btn-yes" onClick={handleConfirmCreate}>
                Yes, create project
              </button>
              <button class="confirm-btn confirm-btn-no" onClick={() => setConfirmPath(null)}>
                Cancel
              </button>
            </div>
          </div>
        </div>
      </Show>
    </>
  );
};

export default Toolbar;
