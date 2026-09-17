import { beforeEach, describe, expect, it, vi } from "vitest";
import type { RemoteActivityUpdate } from "../../shared/types";
import { remoteActivityByPath, remoteActivityStore } from "./remote-activity";

// #2064 — the repo-keyed remote-activity map, fed by `ac_remote_activity_updated`.
// Phase A emits the COMPLETE live set every round and garbage-collects departed
// paths, which is what makes the whole-map replacement below correct rather than
// lossy. These tests drive the store directly; the wiring is pinned in
// ProjectPanel.remote-activity.test.tsx, whose listener test is the only one that
// fails when nothing is registered.

const REPO_A = "C:\\Users\\Maria\\Project\\.ac\\wg-1-team\\repo-AgentsCommander";
// The same directory as REPO_A would be after normalization: a distinct key by
// design, because the chip looks up the exact string discovery handed it.
const REPO_A_FORWARD = "c:/users/maria/project/.ac/wg-1-team/repo-agentscommander";
const REPO_B = "C:\\Users\\Maria\\Project\\.ac\\wg-1-team\\repo-webpage";

function update(overrides: Partial<RemoteActivityUpdate> = {}): RemoteActivityUpdate {
  return {
    repoPaths: [REPO_A],
    ciStates: ["running"],
    stalenessStates: ["stale"],
    behindBy: [3],
    ...overrides,
  };
}

describe("remoteActivityStore (#2064)", () => {
  beforeEach(() => {
    remoteActivityStore.clearAll();
  });

  it("replaces_the_whole_map_per_event", () => {
    remoteActivityStore.applyRemoteActivityUpdate(update());
    expect(remoteActivityStore.forPath(REPO_A)).toEqual({
      ci: "running",
      staleness: "stale",
      behindBy: 3,
    });

    // The next round reports a different repo only: the previous entry is gone,
    // because a path absent from the payload is one the backend stopped tracking.
    remoteActivityStore.applyRemoteActivityUpdate(
      update({
        repoPaths: [REPO_B],
        ciStates: ["idle"],
        stalenessStates: ["current"],
        behindBy: [null],
      })
    );

    expect(remoteActivityStore.forPath(REPO_A)).toBeUndefined();
    expect(remoteActivityStore.forPath(REPO_B)).toEqual({
      ci: "idle",
      staleness: "current",
      behindBy: null,
    });
  });

  it("rejects_a_misaligned_payload_and_keeps_the_previous_map", () => {
    remoteActivityStore.applyRemoteActivityUpdate(update());
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    try {
      // Two CI states for one path: zipping these would paint the wrong repo, so
      // the payload is dropped instead.
      remoteActivityStore.applyRemoteActivityUpdate(update({ ciStates: ["idle", "running"] }));

      expect(warn).toHaveBeenCalledTimes(1);
      expect(remoteActivityStore.forPath(REPO_A)).toEqual({
        ci: "running",
        staleness: "stale",
        behindBy: 3,
      });
    } finally {
      warn.mockRestore();
    }
  });

  it("a_path_absent_from_the_next_payload_reads_unknown", () => {
    remoteActivityStore.applyRemoteActivityUpdate(
      update({
        repoPaths: [REPO_A, REPO_B],
        ciStates: ["running", "running"],
        stalenessStates: ["stale", "current"],
        behindBy: [1, null],
      })
    );
    expect(remoteActivityStore.forPath(REPO_B)).not.toBeUndefined();

    remoteActivityStore.applyRemoteActivityUpdate(update());

    expect(remoteActivityStore.forPath(REPO_B)).toBeUndefined();
    expect(Object.keys(remoteActivityByPath)).toEqual([REPO_A]);
  });

  it("keys_are_the_exact_unnormalized_path_strings", () => {
    remoteActivityStore.applyRemoteActivityUpdate(
      update({
        repoPaths: [REPO_A, REPO_A_FORWARD],
        ciStates: ["running", "idle"],
        stalenessStates: ["current", "current"],
        behindBy: [null, null],
      })
    );

    // Backslashes and case are preserved: two spellings of one directory stay two
    // keys, and neither is reachable by normalizing the lookup key.
    expect(Object.keys(remoteActivityByPath)).toEqual([REPO_A, REPO_A_FORWARD]);
    expect(remoteActivityStore.forPath(REPO_A)?.ci).toBe("running");
    expect(remoteActivityStore.forPath(REPO_A_FORWARD)?.ci).toBe("idle");
    expect(remoteActivityStore.forPath(REPO_A.toUpperCase())).toBeUndefined();
  });
});
