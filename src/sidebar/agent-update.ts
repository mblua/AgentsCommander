import { createStore } from "solid-js/store";
import type { UnlistenFn } from "../shared/transport";
import {
  AgentUpdateAPI,
  onAgentUpdatePrompt,
  onAgentUpdatePromptClosed,
  onAgentUpdatesFinished,
  onAgentUpdatesStarted,
} from "../shared/ipc";
import type {
  AgentUpdatePrompt,
  AgentUpdateResult,
} from "../shared/types";
import { toastStore } from "../shared/stores/toasts";

/** #1327 sidebar state for the startup coding-agent update run. */
export const agentUpdateStore = createStore<{
  inProgress: boolean;
  prompt: AgentUpdatePrompt | null;
}>({ inProgress: false, prompt: null });

/**
 * #1327 - sticky red toast per failed update command, deduped per wiring
 * instance. The event and the snapshot can carry the same failure (the #609
 * subscribe-then-snapshot race), so each command toasts at most once; a fresh
 * mount starts clean (and unit tests stay isolated).
 */
export function showAgentUpdateFailures(
  results: AgentUpdateResult[],
  shownFailures: Set<string>
): void {
  for (const result of results) {
    if (result.ok) continue;
    if (shownFailures.has(result.command)) continue;
    shownFailures.add(result.command);
    toastStore.error(
      `Auto-update failed for ${result.label} (${result.command}): ${result.error ?? "unknown error"}`,
      { durationMs: null }
    );
  }
}

/**
 * #1327 - wire the sidebar's startup coding-agent update notifications.
 * Subscribe-then-snapshot (#609): the listeners register BEFORE the snapshot is
 * queried, so a startup emit fired during mount is never dropped, and the
 * snapshot restores a prompt that was emitted pre-wiring. Returns the listener
 * unsubscribers for the caller's cleanup list. Extracted into its own module so
 * the wiring is unit-testable without rendering the whole sidebar.
 */
export async function wireAgentUpdateListeners(): Promise<UnlistenFn[]> {
  const unlisteners: UnlistenFn[] = [];
  // Per-instance dedup: event + snapshot for the same command toast once.
  const shownFailures = new Set<string>();
  const [, setAgentUpdateState] = agentUpdateStore;

  unlisteners.push(
    await onAgentUpdatesStarted(() => {
      setAgentUpdateState("inProgress", true);
    })
  );

  unlisteners.push(
    await onAgentUpdatePrompt((prompt) => {
      setAgentUpdateState("prompt", prompt);
    })
  );

  // F4: the backend timed the prompt out (no answer within 60s).
  unlisteners.push(
    await onAgentUpdatePromptClosed(() => {
      setAgentUpdateState("prompt", null);
    })
  );

  unlisteners.push(
    await onAgentUpdatesFinished(({ results }) => {
      setAgentUpdateState({ inProgress: false, prompt: null });
      showAgentUpdateFailures(results, shownFailures);
    })
  );

  // THEN snapshot (F8: a getStatus failure never breaks the live listeners).
  AgentUpdateAPI.getStatus()
    .then((status) => {
      if (!status) return;
      if (status.inProgress) setAgentUpdateState("inProgress", true);
      if (status.prompt) setAgentUpdateState("prompt", status.prompt); // F3
      showAgentUpdateFailures(status.results, shownFailures);
    })
    .catch((err) => {
      console.error("[agent-update] getStatus failed:", err);
    });

  return unlisteners;
}
