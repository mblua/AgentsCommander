import { Component, createMemo, onMount, onCleanup } from "solid-js";
import { createStore } from "solid-js/store";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { SessionAPI } from "../../shared/ipc";
import { terminalStore } from "../stores/terminal";

interface LastPromptProps {
  sessionId?: string;
}

const LastPrompt: Component<LastPromptProps> = (props) => {
  const [prompts, setPrompts] = createStore<Record<string, string>>({});
  let unlisten: UnlistenFn | null = null;

  const getSessionId = () => props.sessionId ?? terminalStore.activeSessionId;

  const currentPrompt = createMemo(() => {
    const id = getSessionId();
    return id ? prompts[id] ?? "" : "";
  });

  onMount(async () => {
    // Load persisted last prompts from backend
    const sessions = await SessionAPI.list();
    for (const s of sessions) {
      if (s.lastPrompt) {
        setPrompts(s.id, s.lastPrompt);
      }
    }

    unlisten = await listen<{ text: string; sessionId: string }>(
      "last_prompt",
      (event) => {
        setPrompts(event.payload.sessionId, event.payload.text);
      }
    );
  });

  onCleanup(() => {
    unlisten?.();
  });

  return (
    <div class="last-prompt-panel">
      <div class="last-prompt-label">LAST PROMPT</div>
      <div class="last-prompt-text">{currentPrompt() || "..."}</div>
    </div>
  );
};

export default LastPrompt;
