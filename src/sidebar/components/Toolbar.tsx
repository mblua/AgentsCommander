import { Component, createSignal } from "solid-js";
import { SessionAPI } from "../../shared/ipc";
import OpenAgentModal from "./OpenAgentModal";

const Toolbar: Component = () => {
  const [showOpenAgent, setShowOpenAgent] = createSignal(false);

  const handleNewSession = () => {
    SessionAPI.create();
  };

  return (
    <>
      <div class="toolbar">
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
    </>
  );
};

export default Toolbar;
