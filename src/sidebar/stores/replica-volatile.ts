import { batch } from "solid-js";
import { createStore } from "solid-js/store";
import type { AcAgentReplica, RepoBranchByPath, RepoDirtyByPath } from "../../shared/types";
import { normalizeProjectPathForCompare } from "./project-refresh";

export interface ReplicaVolatileEntry {
  repoBranch?: string | null;
  repoBranchByPath?: RepoBranchByPath;
  repoDirtyByPath?: RepoDirtyByPath;
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
  setEntries(volatileKey(path), (prev) => ({ ...prev, [field]: value }));
}

function zipByPath<T>(
  repoPaths: string[] | undefined,
  values: (T | null)[] | undefined
): Record<string, T | null> {
  const paths = repoPaths ?? [];
  const vals = values ?? [];
  if (paths.length !== vals.length) return {};
  const map: Record<string, T | null> = {};
  for (let i = 0; i < paths.length; i += 1) {
    map[paths[i]] = vals[i] ?? null;
  }
  return map;
}

export const replicaVolatileStore = {
  setRepoBranch(replicaPath: string, branch: string | null) {
    setField(replicaPath, "repoBranch", branch);
  },

  applyDiscoveryBranchUpdate(
    replicaPath: string,
    branch: string | null,
    repoPaths?: string[],
    repoBranches?: (string | null)[],
    repoDirty?: (boolean | null)[]
  ) {
    batch(() => {
      setField(replicaPath, "repoBranch", branch);
      setField(replicaPath, "repoBranchByPath", zipByPath<string>(repoPaths, repoBranches));
      setField(replicaPath, "repoDirtyByPath", zipByPath<boolean>(repoPaths, repoDirty));
    });
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

  clearForPaths(replicaPaths: Iterable<string>) {
    batch(() => {
      for (const path of replicaPaths) {
        const key = volatileKey(path);
        if (entries[key] === undefined) continue;
        let preservedBranch: RepoBranchByPath | undefined;
        let preservedDirty: RepoDirtyByPath | undefined;
        setEntries(key, (prev) => {
          preservedBranch = prev?.repoBranchByPath;
          preservedDirty = prev?.repoDirtyByPath;
          return undefined; // deleting the key notifies its readers
        });
        if (preservedBranch !== undefined || preservedDirty !== undefined) {
          setEntries(key, { repoBranchByPath: preservedBranch, repoDirtyByPath: preservedDirty });
        }
      }
    });
  },

  clearAll() {
    batch(() => {
      for (const key of Object.keys(entries)) {
        setEntries(key, undefined); // deleting the key notifies its readers
      }
    });
  },
};

type ReplicaVolatileBase = Pick<AcAgentReplica, "path"> &
  Partial<Pick<AcAgentReplica, "repoBranch" | "lastUserMessageAt" | "autoClosedAt" | "manuallyClosedAt">>;

export function effectiveRepoBranch(replica: ReplicaVolatileBase): string | undefined {
  const override = entries[volatileKey(replica.path)]?.repoBranch;
  return override === undefined ? replica.repoBranch : override ?? undefined;
}

export function effectiveRepoBranchByPath(
  replica: ReplicaVolatileBase
): RepoBranchByPath | undefined {
  return entries[volatileKey(replica.path)]?.repoBranchByPath;
}

export function effectiveRepoDirtyByPath(
  replica: ReplicaVolatileBase
): RepoDirtyByPath | undefined {
  return entries[volatileKey(replica.path)]?.repoDirtyByPath;
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
