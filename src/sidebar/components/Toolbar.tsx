import { Component, createSignal } from "solid-js";
import OpenAgentModal from "./OpenAgentModal";
import NewAgentModal from "./NewAgentModal";

const Toolbar: Component = () => {
  const [showOpenAgent, setShowOpenAgent] = createSignal(false);
  const [showNewAgent, setShowNewAgent] = createSignal(false);

  return (
    <>
      <div class="toolbar">
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
    </>
  );
};

export default Toolbar;
