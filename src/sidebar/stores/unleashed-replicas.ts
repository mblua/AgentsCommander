import { createSignal } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { AcDiscoveryAPI } from "../../shared/ipc";
import type { AcAgentReplica } from "../../shared/types";
import { normalizeProjectPathForCompare } from "./project-refresh";

/**
 * User overrides of the discovery `unleashed` flag, keyed by the
 * normalized replica path. An override is written only after the backend
 * accepted the toggle, so the menu label never runs ahead of the stored flag.
 * It also owns the open confirmation prompt and its busy state.
 */
export interface UnleashPrompt {
  sessionId: string;
  replicaPath: string;
  replicaName: string;
}

const [overrides, setOverrides] = createStore<Record<string, boolean | undefined>>({});
const [prompt, setPrompt] = createSignal<UnleashPrompt | null>(null);
const [busy, setBusy] = createSignal(false);

export const unleashedReplicasStore = {
  isUnleashed(replica: Pick<AcAgentReplica, "path" | "unleashed">): boolean {
    const override = overrides[normalizeProjectPathForCompare(replica.path)];
    return override ?? replica.unleashed === true;
  },

  async set(path: string, unleashed: boolean): Promise<void> {
    await AcDiscoveryAPI.setReplicaUnleashed(path, unleashed);
    setOverrides(normalizeProjectPathForCompare(path), unleashed);
  },

  prompt,
  openPrompt(p: UnleashPrompt): void {
    setPrompt(p);
  },
  closePrompt(): void {
    setPrompt(null);
  },
  busy,
  setBusy(b: boolean): void {
    setBusy(b);
  },

  resetForTests(): void {
    setOverrides(reconcile({}));
    setPrompt(null);
    setBusy(false);
  },
};
