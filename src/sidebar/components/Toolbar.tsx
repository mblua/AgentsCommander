import { Component, createSignal, Show } from "solid-js";
import { projectStore } from "../stores/project";
import OpenAgentModal from "./OpenAgentModal";
import NewAgentModal from "./NewAgentModal";

const Toolbar: Component = () => {
  const [showOpenAgent, setShowOpenAgent] = createSignal(false);
  const [showNewAgent, setShowNewAgent] = createSignal(false);
  const [confirmPath, setConfirmPath] = createSignal<string | null>(null);

  const handleOpenProject = async () => {
    const { picked, hasAcNew } = await projectStore.pickAndCheck();
    if (!picked) return;
    if (!hasAcNew) {
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
        <div class="confirm-overlay" onClick={() => setConfirmPath(null)}>
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
