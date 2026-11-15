import { createStore, reconcile } from "solid-js/store";
import { UnleashAPI } from "../../shared/ipc";
import type { AcAgentReplica } from "../../shared/types";
import { normalizeProjectPathForCompare } from "./project-refresh";

/**
 * #4752 - user overrides of the discovery `unleashed` flag, keyed by the
 * normalized replica path. An override is written only after the backend
 * accepted the toggle, so the menu label never runs ahead of the stored flag.
 */
const [overrides, setOverrides] = createStore<Record<string, boolean | undefined>>({});

export const unleashedReplicasStore = {
  isUnleashed(replica: Pick<AcAgentReplica, "path" | "unleashed">): boolean {
    const override = overrides[normalizeProjectPathForCompare(replica.path)];
    return override ?? replica.unleashed === true;
  },

  async set(path: string, unleashed: boolean): Promise<void> {
    await UnleashAPI.set(path, unleashed);
    setOverrides(normalizeProjectPathForCompare(path), unleashed);
  },

  resetForTests(): void {
    setOverrides(reconcile({}));
  },
};
