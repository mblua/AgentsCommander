import { batch } from "solid-js";
import { createStore } from "solid-js/store";
import type { AcAgentReplica, RepoBranchByPath, RepoDirtyByPath } from "../../shared/types";
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
  /** #1028 - per-repo worktree-dirty keyed by repo source path. Same missing-key vs
   *  explicit-null semantics as `repoBranchByPath` above; see RepoDirtyByPath
   *  (shared/types.ts). Like that map, this one is PRESERVED by `clearForPaths`. */
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
  // Merge-function form so a first write creates the entry ({...undefined} = {}).
  setEntries(volatileKey(path), (prev) => ({ ...prev, [field]: value }));
}

/**
 * #943 B2 - build a path -> value map from the event's parallel arrays.
 * #1028 - generalised over the value type: `T = string` gives RepoBranchByPath,
 * `T = boolean` gives RepoDirtyByPath.
 *
 * A length mismatch could only come from a build that broke the backend's 1:1
 * invariant. Pairing them anyway would attach a value to the wrong repo, so the
 * map is dropped instead and every repo falls back to "unknown": visible, inert,
 * and self-healing on the next tick. No branch beats a wrong branch, and an
 * unknown-dirty (violet) beats a red badge on the wrong repo.
 *
 * The guard is applied PER CALL, not once for both arrays: a malformed `repoDirty`
 * must not be able to drop the branch map with it.
 */
function zipByPath<T>(
  repoPaths: string[] | undefined,
  values: (T | null)[] | undefined
): Record<string, T | null> {
  const paths = repoPaths ?? [];
  const vals = values ?? [];
  if (paths.length !== vals.length) return {};
  const map: Record<string, T | null> = {};
  for (let i = 0; i < paths.length; i += 1) {
    // `?? null`, NEVER `|| null`. With T = string this looks like dead syntax an
    // implementer would "simplify". With T = boolean it is load-bearing and the
    // difference is INVISIBLE on the badge: `false ?? null` is `false` (detected
    // clean, correct), but `false || null` is `null` ("never detected"), and both
    // render violet. Only the badge title exposes the slip, which is why
    // `repoDirty: [false]` has its own test.
    map[paths[i]] = vals[i] ?? null;
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
    repoBranches?: (string | null)[],
    repoDirty?: (boolean | null)[]
  ) {
    batch(() => {
      setField(replicaPath, "repoBranch", branch);
      setField(replicaPath, "repoBranchByPath", zipByPath<string>(repoPaths, repoBranches));
      // #1028 - inside the SAME batch: dirty and branch come from one payload and
      // must land together, or a reader observes a row whose branch has ticked and
      // whose colour has not.
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
   * `repoBranchByPath` (#943 B2) and `repoDirtyByPath` (#1028) are deliberately
   * PRESERVED, because neither has a counterpart: we did not widen
   * `AcAgentReplica`, so wiping either installs nothing. The backend would never
   * re-send it either: the discovery branch watcher only emits when the payload
   * CHANGES (`ac_discovery.rs` Gate A), and an unchanged repo set with unchanged
   * branches and unchanged dirty is an identical payload. So a wiped map would be
   * gone until something changed on disk or the app restarted, and reloads are
   * routine (every loop tick, every CLI-driven refresh, every entity creation).
   * "Discovery wins on reload" is meaningless for a field discovery never sends.
   *
   * What each map falls back to when wiped is NOT the same, and the dirty half is
   * the worse one:
   *   - `repoBranchByPath` degrades to the single-repo `repoBranch` shorthand —
   *     `null` for a multi-repo replica — which is how Browse Branch died on the
   *     first reload and stayed dead, "not for 15s, forever" (H1, a HIGH bug).
   *   - `repoDirtyByPath` has NO shorthand to degrade to (discovery never sent a
   *     scalar dirty), so every repo silently reverts to `null` = violet = "clean,
   *     as far as you can see" — a false CLEAN on a passively-read surface, which is
   *     the one direction #1028 is built never to assert.
   *
   * Preserving them is safe precisely BECAUSE they are keyed by path: a repo
   * dropped from config.json simply never matches again, a repo added since the last
   * tick has no entry until the watcher emits one, and a real change re-emits.
   *
   * Use `clearAll` when the replicas themselves are gone.
   */
  clearForPaths(replicaPaths: Iterable<string>) {
    batch(() => {
      for (const path of replicaPaths) {
        const key = volatileKey(path);
        if (entries[key] === undefined) continue;
        // Delete the entry, then restore ONLY the two by-path maps. Returning a smaller
        // object instead would not clear anything: a Solid store setter MERGES a
        // wrappable value into the existing node (mergeStoreNode), so the snapshot-
        // backed fields would survive the reload and mask discovery forever - a
        // worse bug than the one this method is fixing. The function form is used
        // to read `prev` RAW (untracked); both writes sit inside the batch above,
        // so readers only ever observe the final state.
        let preservedBranch: RepoBranchByPath | undefined;
        let preservedDirty: RepoDirtyByPath | undefined;
        setEntries(key, (prev) => {
          preservedBranch = prev?.repoBranchByPath;
          preservedDirty = prev?.repoDirtyByPath;
          return undefined; // deleting the key notifies its readers
        });
        // `||`, not `&&`. Today the two are always written together (the only
        // writer is applyDiscoveryBranchUpdate, whose batch sets both, and a
        // length-mismatch stores `{}` rather than leaving the field undefined), so
        // `&&` would behave identically and no test can tell them apart. It is `||`
        // because it is the condition that stays CORRECT if that ever stops holding:
        // with `&&`, a single one-sided entry silently drops the map that IS there,
        // and the symptom would be a badge quietly losing its red on reload - the
        // exact failure this preserve exists to prevent, re-entering by the back
        // door. The cost of the safe operator here is zero.
        if (preservedBranch !== undefined || preservedDirty !== undefined) {
          setEntries(key, { repoBranchByPath: preservedBranch, repoDirtyByPath: preservedDirty });
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

/** #1028 - live per-repo worktree-dirty for this replica, keyed by repo source path.
 *  `undefined` until the first discovery branch event lands, which the caller maps to
 *  `dirty: null` (violet, "status unknown"): the normal state for the first <=15s
 *  after launch, not an error. There is no single-repo shorthand to fall back to. */
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
