
import { Component, createSignal, createResource } from "solid-js";
import { specBoardStore, setSpecBoardStore } from "../stores/spec-board";
import { SessionAPI, PtyAPI } from "../../shared/ipc";
import { Session } from "../../shared/types";

const AskAgentPanel: Component = () => {
  const [instruction, setInstruction] = createSignal("");
  const [selectedSessionId, setSelectedSessionId] = createSignal("");
  
  const [sessions] = createResource<Session[]>(async () => {
    return await SessionAPI.list();
  });

  const handleSend = async () => {
    if (!selectedSessionId() || !specBoardStore.path || !instruction()) return;
    
    const prompt = `Please edit the spec board Mermaid file at:\n\n"${specBoardStore.path}"\n\nUser request:\n${instruction()}\n\nKeep exactly one Mermaid diagram in the file. Save the file when done. The board watches the file and will update automatically.\r`;
    
    try {
      await PtyAPI.write(selectedSessionId(), new TextEncoder().encode(prompt));
      setInstruction("");
      setSpecBoardStore("showAskAgent", false);
    } catch (err) {
      console.error(err);
      setSpecBoardStore("renderError", "Failed to send to agent: " + String(err));
    }
  };

  return (
    <div class="spec-board-ask-agent">
      <div style={{ display: 'flex', "justify-content": 'space-between' }}>
        <strong>Ask Agent</strong>
        <button onClick={() => setSpecBoardStore("showAskAgent", false)}>✕</button>
      </div>
      <select 
        value={selectedSessionId()} 
        onChange={(e) => setSelectedSessionId(e.currentTarget.value)}
      >
        <option value="">Select a session...</option>
        {sessions()?.map(s => (
          <option value={s.id}>{s.name || s.id}</option>
        ))}
      </select>
      <textarea 
        placeholder="Instructions..."
        value={instruction()}
        onInput={(e) => setInstruction(e.currentTarget.value)}
      />
      <button onClick={handleSend} disabled={!selectedSessionId() || !instruction()}>Send</button>
    </div>
  );
};

export default AskAgentPanel;
