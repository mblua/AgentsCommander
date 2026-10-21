import { Component, createMemo, onMount, onCleanup } from "solid-js";
import { createStore } from "solid-js/store";
import type { UnlistenFn } from "../../shared/transport";
import { onLastPrompt, onSessionCreated, SessionAPI } from "../../shared/ipc";
import { terminalStore } from "../stores/terminal";

interface LastPromptProps {
  sessionId?: string;
}

const LastPrompt: Component<LastPromptProps> = (props) => {
  const [prompts, setPrompts] = createStore<Record<string, string | undefined>>({});
  let unlistenLastPrompt: UnlistenFn | null = null;
  let unlistenSessionCreated: UnlistenFn | null = null;

  let requestSeq = 0;
  let disposed = false;
  const revisionPorId = new Map<string, number>();

  const getSessionId = () => props.sessionId ?? terminalStore.activeSessionId;

  const currentPrompt = createMemo(() => {
    const id = getSessionId();
    return id ? prompts[id] ?? "" : "";
  });

  const loadPrompts = async () => {
    if (disposed) return;
    const seq = ++requestSeq;
    const revisions = new Map(revisionPorId);
    try {
      const sessions = await SessionAPI.list();
      if (disposed || seq !== requestSeq) return;
      const snapshot = new Map(sessions.map((s) => [s.id, s.lastPrompt]));
      for (const id of new Set([...Object.keys(prompts), ...snapshot.keys()])) {
        // Events received after this request own their session's value.
        if ((revisionPorId.get(id) ?? 0) !== (revisions.get(id) ?? 0)) continue;
        setPrompts(id, snapshot.get(id) ?? undefined);
      }
    } catch {
      console.warn("LastPrompt: could not load prompts");
    }
  };

  const releaseListeners = () => {
    unlistenLastPrompt?.();
    unlistenLastPrompt = null;
    unlistenSessionCreated?.();
    unlistenSessionCreated = null;
  };

  onMount(async () => {
    try {
      const lastPromptListener = await onLastPrompt((data) => {
        if (disposed) return;
        revisionPorId.set(data.sessionId, (revisionPorId.get(data.sessionId) ?? 0) + 1);
        setPrompts(data.sessionId, data.text);
      });
      if (disposed) {
        lastPromptListener();
        return;
      }
      unlistenLastPrompt = lastPromptListener;
      const sessionCreatedListener = await onSessionCreated(() => { void loadPrompts(); });
      if (disposed) {
        sessionCreatedListener();
        return;
      }
      unlistenSessionCreated = sessionCreatedListener;
      await loadPrompts();
    } catch {
      releaseListeners();
      console.warn("LastPrompt: could not register listeners");
    }
  });

  onCleanup(() => {
    disposed = true;
    ++requestSeq;
    releaseListeners();
    revisionPorId.clear();
  });

  return (
    <div class="last-prompt-panel">
      <div class="last-prompt-label">LAST PROMPT</div>
      <div class="last-prompt-text">{currentPrompt() || "..."}</div>
    </div>
  );
};

export default LastPrompt;
