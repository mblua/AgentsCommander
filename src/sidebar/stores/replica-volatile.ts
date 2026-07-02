import { batch } from "solid-js";
import { createStore } from "solid-js/store";
import type { AcAgentReplica } from "../../shared/types";
import { normalizeProjectPathForCompare } from "./project-refresh";

/**
 * #748 — live event-driven layer for the volatile per-replica fields that used
 * to be patched INTO projectStore's replica objects (repoBranch,
 * lastUserMessageAt, autoClosedAt, manuallyClosedAt). Patching them in place
 * replaced every project/workgroup object reference, and ProjectPanel's
 * reference-keyed <For> then disposed and re-created the entire clickable
 * sidebar DOM on every event — a click whose press straddled that swap never
 * produced a `click` (detached mousedown target → no common inclusive
 * ancestor; Chromium fires nothing, w3c/uievents#141). Keeping the volatile
 * fields in a separate fine-grained store keyed by NORMALIZED replica path
 * (events carry the session working_directory, which can differ in slash/case
 * from the discovery path on Windows) leaves row identity stable: an event
 * re-runs only the badge/pill memo that reads it.
 *
 * Entry-field semantics: `undefined` = no live override (fall back to the
 * discovery snapshot on the replica object); `null` = an event explicitly
 * cleared the value (masks a stale discovery marker until the next snapshot).
 */
export interface ReplicaVolatileEntry {
  repoBranch?: string | null;
  lastUserMessageAt?: string;
  autoClosedAt?: string | null;
  manuallyClosedAt?: string | null;
}

const [entries, setEntries] = createStore<Record<string, ReplicaVolatileEntry | undefined>>({});

function volatileKey(path: string): string {
  return normalizeProjectPathForCompare(path);
}

function setField<K extends keyof ReplicaVolatileEntry>(
  path: string,
  field: K,
  value: ReplicaVolatileEntry[K]
) {
  // Merge-function form so a first write creates the entry ({...undefined} = {}).
  setEntries(volatileKey(path), (prev) => ({ ...prev, [field]: value }));
}

export const replicaVolatileStore = {
  setRepoBranch(replicaPath: string, branch: string | null) {
    setField(replicaPath, "repoBranch", branch);
  },

  setLastUserMessageAt(replicaPath: string, lastUserMessageAt: string) {
    setField(replicaPath, "lastUserMessageAt", lastUserMessageAt);
  },

  setAutoClosedAt(replicaPath: string, autoClosedAt: string | null) {
    setField(replicaPath, "autoClosedAt", autoClosedAt);
  },

  setManuallyClosedAt(replicaPath: string, manuallyClosedAt: string | null) {
    setField(replicaPath, "manuallyClosedAt", manuallyClosedAt);
  },

  /**
   * Drop overrides for replica paths a fresh discovery snapshot now covers.
   * Discovery fills all four fields (ac_discovery.rs reads the branch and the
   * persisted CoordinatorClocks), so on load/reload the snapshot is the newer
   * truth and a pre-reload live override must not mask it — the same
   * "discovery wins on reload, events re-patch after" contract the old
   * in-place patches had.
   */
  clearForPaths(replicaPaths: Iterable<string>) {
    batch(() => {
      for (const path of replicaPaths) {
        const key = volatileKey(path);
        if (entries[key] !== undefined) {
          setEntries(key, undefined); // deleting the key notifies its readers
        }
      }
    });
  },

  clearAll() {
    // Stored keys are already normalized, so volatileKey is a no-op on them.
    replicaVolatileStore.clearForPaths(Object.keys(entries));
  },
};

type ReplicaVolatileBase = Pick<AcAgentReplica, "path"> &
  Partial<Pick<AcAgentReplica, "repoBranch" | "lastUserMessageAt" | "autoClosedAt" | "manuallyClosedAt">>;

/** Live branch when an event patched it (null = branch gone), else the discovery snapshot. */
export function effectiveRepoBranch(replica: ReplicaVolatileBase): string | undefined {
  const override = entries[volatileKey(replica.path)]?.repoBranch;
  return override === undefined ? replica.repoBranch : override ?? undefined;
}

export function effectiveLastUserMessageAt(replica: ReplicaVolatileBase): string | undefined {
  return entries[volatileKey(replica.path)]?.lastUserMessageAt ?? replica.lastUserMessageAt;
}

export function effectiveAutoClosedAt(replica: ReplicaVolatileBase): string | undefined {
  const override = entries[volatileKey(replica.path)]?.autoClosedAt;
  return override === undefined ? replica.autoClosedAt : override ?? undefined;
}

export function effectiveManuallyClosedAt(replica: ReplicaVolatileBase): string | undefined {
  const override = entries[volatileKey(replica.path)]?.manuallyClosedAt;
  return override === undefined ? replica.manuallyClosedAt : override ?? undefined;
}
