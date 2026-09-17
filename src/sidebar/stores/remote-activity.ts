import { batch } from "solid-js";
import { createStore } from "solid-js/store";
import type { CiState, RemoteActivityUpdate, StalenessState } from "../../shared/types";

/** #2064 — one repo's published remote-activity answer. */
export interface RemoteActivityEntry {
  ci: CiState;
  staleness: StalenessState;
  behindBy: number | null;
}

/** Keyed by the EXACT `sourcePath` string the backend emitted, with no
 *  normalization: the payload's `repoPaths` and `SessionRepo.sourcePath` are the
 *  same strings, Windows backslashes included. A normalized key would simply
 *  never match the chip.
 *
 *  Keyed by REPO path, not replica path, which is why this map lives beside
 *  `replica-volatile.ts` instead of inside `ReplicaVolatileEntry`: that entry is
 *  keyed per replica, and a fourth parallel per-replica vector would be N copies
 *  of one fact. */
const [remoteActivityByPath, setRemoteActivityByPath] = createStore<
  Record<string, RemoteActivityEntry | undefined>
>({});

export { remoteActivityByPath };

export const remoteActivityStore = {
  forPath(sourcePath: string): RemoteActivityEntry | undefined {
    return remoteActivityByPath[sourcePath];
  },

  /** Replaces the WHOLE map, which is correct rather than lazy: Phase A emits the
   *  complete live set every round and garbage-collects departed paths, so a path
   *  absent from this payload is one we stopped tracking and reads as unknown. */
  applyRemoteActivityUpdate(update: RemoteActivityUpdate): void {
    const { repoPaths, ciStates, stalenessStates, behindBy } = update;
    if (
      repoPaths.length !== ciStates.length ||
      repoPaths.length !== stalenessStates.length ||
      repoPaths.length !== behindBy.length
    ) {
      // Misaligned vectors mean the producer disagrees with itself, and zipping
      // them would paint the WRONG repo — worse than painting nothing. Keep the
      // previous map; one warning per rejected payload.
      console.warn(
        "[remote-activity] rejected a misaligned payload (lengths " +
          `${repoPaths.length}/${ciStates.length}/${stalenessStates.length}/${behindBy.length})`
      );
      return;
    }

    const next: Record<string, RemoteActivityEntry> = {};
    for (let i = 0; i < repoPaths.length; i += 1) {
      next[repoPaths[i]] = {
        ci: ciStates[i],
        staleness: stalenessStates[i],
        behindBy: behindBy[i] ?? null,
      };
    }

    batch(() => {
      for (const key of Object.keys(remoteActivityByPath)) {
        // Deleting the key notifies its readers, exactly as `replica-volatile` does.
        if (next[key] === undefined) setRemoteActivityByPath(key, undefined);
      }
      for (const key of Object.keys(next)) setRemoteActivityByPath(key, next[key]);
    });
  },

  clearAll(): void {
    batch(() => {
      for (const key of Object.keys(remoteActivityByPath)) {
        setRemoteActivityByPath(key, undefined); // deleting the key notifies its readers
      }
    });
  },
};
