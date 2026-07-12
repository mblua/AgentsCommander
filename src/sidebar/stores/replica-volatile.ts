import { batch } from "solid-js";
import { createStore } from "solid-js/store";
import type { AcAgentReplica, RepoBranchByPath } from "../../shared/types";
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
  /** #943 B2 - per-repo branches keyed by repo source path. See RepoBranchByPath
   *  (shared/types.ts) for the missing-key vs explicit-null semantics. */
  repoBranchByPath?: RepoBranchByPath;
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

/**
 * #943 B2 - build the path -> branch map from the event's parallel arrays.
 *
 * A length mismatch could only come from a build that broke the backend's 1:1
 * invariant. Pairing them anyway would attach a branch to the wrong repo, so the
 * map is dropped instead and every repo falls back to "no branch": visible, inert,
 * and self-healing on the next tick. No branch beats a wrong branch.
 */
function buildRepoBranchByPath(
  repoPaths: string[] | undefined,
  repoBranches: (string | null)[] | undefined
): RepoBranchByPath {
  const paths = repoPaths ?? [];
  const branches = repoBranches ?? [];
  if (paths.length !== branches.length) return {};
  const map: RepoBranchByPath = {};
  for (let i = 0; i < paths.length; i += 1) {
    map[paths[i]] = branches[i] ?? null;
  }
  return map;
}

export const replicaVolatileStore = {
  setRepoBranch(replicaPath: string, branch: string | null) {
    setField(replicaPath, "repoBranch", branch);
  },

  /**
   * #943 B2 - apply one `ac_discovery_branch_updated` event.
   *
   * Atomic on purpose: the single-repo shorthand and the path-keyed per-repo map
   * always land together, so no reader can observe a half-updated pair (for a
   * single-repo replica that would paint the previous branch for a frame).
   *
   * Repo paths are used as keys verbatim - the backend sends the same strings
   * discovery already put on `AcAgentReplica.repoPaths` - unlike the REPLICA path,
   * which is normalized because branch/clock events carry the session
   * working_directory and can differ in slash/case (#552).
   */
  applyDiscoveryBranchUpdate(
    replicaPath: string,
    branch: string | null,
    repoPaths?: string[],
    repoBranches?: (string | null)[]
  ) {
    batch(() => {
      setField(replicaPath, "repoBranch", branch);
      setField(replicaPath, "repoBranchByPath", buildRepoBranchByPath(repoPaths, repoBranches));
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

  /**
   * Drop the live overrides for replica paths a fresh discovery snapshot now
   * covers — but only the fields that snapshot actually re-supplies.
   *
   * `repoBranch`, `lastUserMessageAt`, `autoClosedAt` and `manuallyClosedAt` each
   * have a counterpart on `AcAgentReplica` (ac_discovery.rs reads the branch and
   * the persisted CoordinatorClocks), so on load/reload the snapshot is the newer
   * truth and a pre-reload override must not mask it — the "discovery wins on
   * reload, events re-patch after" contract the old in-place patches had.
   *
   * `repoBranchByPath` (#943 B2) is deliberately PRESERVED, because it has no
   * counterpart: we did not widen `AcAgentReplica`, so wiping it installs nothing.
   * It would fall back to the single-repo `repoBranch` shorthand — `null` for a
   * multi-repo replica — and the backend would never re-send it: the discovery
   * branch watcher only emits when the payload CHANGES (`ac_discovery.rs` Gate A),
   * and an unchanged repo set with unchanged branches is an identical payload. So
   * the map would be gone until a branch changed on disk or the app restarted, and
   * reloads are routine (every loop tick, every CLI-driven refresh, every entity
   * creation). "Discovery wins on reload" is meaningless for a field discovery
   * never sends.
   *
   * Preserving it is safe precisely BECAUSE it is keyed by path: a repo dropped
   * from config.json simply never matches again, a repo added since the last tick
   * has no entry until the watcher emits one, and a real branch change re-emits.
   *
   * Use `clearAll` when the replicas themselves are gone.
   */
  clearForPaths(replicaPaths: Iterable<string>) {
    batch(() => {
      for (const path of replicaPaths) {
        const key = volatileKey(path);
        if (entries[key] === undefined) continue;
        // Delete the entry, then restore ONLY the B2 map. Returning a smaller
        // object instead would not clear anything: a Solid store setter MERGES a
        // wrappable value into the existing node (mergeStoreNode), so the snapshot-
        // backed fields would survive the reload and mask discovery forever - a
        // worse bug than the one this method is fixing. The function form is used
        // to read `prev` RAW (untracked); both writes sit inside the batch above,
        // so readers only ever observe the final state.
        let preserved: RepoBranchByPath | undefined;
        setEntries(key, (prev) => {
          preserved = prev?.repoBranchByPath;
          return undefined; // deleting the key notifies its readers
        });
        if (preserved !== undefined) {
          setEntries(key, { repoBranchByPath: preserved });
        }
      }
    });
  },

  /**
   * Full wipe, `repoBranchByPath` included. For when the replicas themselves go
   * away (`projectStore.clear`) and for test isolation — unlike `clearForPaths`,
   * there is no surviving replica whose per-repo branches would be worth keeping.
   */
  clearAll() {
    batch(() => {
      // Stored keys are already normalized, so volatileKey is a no-op on them.
      for (const key of Object.keys(entries)) {
        setEntries(key, undefined); // deleting the key notifies its readers
      }
    });
  },
};

type ReplicaVolatileBase = Pick<AcAgentReplica, "path"> &
  Partial<Pick<AcAgentReplica, "repoBranch" | "lastUserMessageAt" | "autoClosedAt" | "manuallyClosedAt">>;

/** Live branch when an event patched it (null = branch gone), else the discovery snapshot. */
export function effectiveRepoBranch(replica: ReplicaVolatileBase): string | undefined {
  const override = entries[volatileKey(replica.path)]?.repoBranch;
  return override === undefined ? replica.repoBranch : override ?? undefined;
}

/** #943 B2 - live per-repo branches for this replica, keyed by repo source path.
 *  `undefined` when no branch event has landed yet, in which case the caller keeps
 *  the pre-B2 single-repo `repoBranch` fallback. */
export function effectiveRepoBranchByPath(
  replica: ReplicaVolatileBase
): RepoBranchByPath | undefined {
  return entries[volatileKey(replica.path)]?.repoBranchByPath;
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
