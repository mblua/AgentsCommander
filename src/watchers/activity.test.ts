import { describe, expect, it } from "vitest";
import type {
  Session,
  WatcherActivitySnapshot,
  WatcherMatchPayload,
} from "../shared/types";
import {
  type ActivityRow,
  anyTruncated,
  capPerSession,
  degradedWatchers,
  filterRows,
  freezeRow,
  keepSessions,
  mergeActiveWatchers,
  mergeRows,
  resolveView,
  rowKey,
  totalPossiblyMissedFrames,
} from "./activity";

function match(overrides: Partial<WatcherMatchPayload> = {}): WatcherMatchPayload {
  return {
    sessionId: "s1",
    seq: 1,
    watcherId: "reads",
    mode: "occurrence",
    at: "2026-07-30T22:31:05Z",
    captures: ["C:/repo/main.rs"],
    row: "Read (C:/repo/main.rs)",
    rowTruncated: false,
    ...overrides,
  };
}

// Only the fields these tests read; the cast is the fixture admitting it is partial.
function session(overrides: Partial<Session> = {}): Session {
  return {
    id: "s1",
    name: "claude@repo",
    workingDirectory: "C:/proj/.ac/wg-19-dev-v4-team/__agent_dev-rust",
    agentId: "agent_1",
    agentLabel: "Claude Sandbox",
    ...overrides,
  } as Session;
}

function snapshot(
  overrides: Partial<WatcherActivitySnapshot> = {}
): WatcherActivitySnapshot {
  return {
    matches: [],
    lastSeq: 0,
    truncated: false,
    possiblyMissedFrames: 0,
    warmedUp: true,
    activeWatchers: [],
    ...overrides,
  };
}

const row = (overrides: Partial<ActivityRow> = {}): ActivityRow => ({
  ...freezeRow(match(), session()),
  ...overrides,
});

describe("freezing a row (#1171)", () => {
  it("takes the workgroup from the working directory, not from the session name", () => {
    const frozen = freezeRow(
      match(),
      session({ name: "renamed by the user", workingDirectory: "C:/p/.ac/wg-7-team/x" })
    );
    expect(frozen.workgroup).toBe("WG-7-TEAM");
  });

  it("keeps the session id as the name when the session is already gone", () => {
    const frozen = freezeRow(match({ sessionId: "ghost" }), undefined);
    expect(frozen.sessionName).toBe("ghost");
    expect(frozen.agentId).toBeNull();
    expect(frozen.workgroup).toBeNull();
  });
});

/**
 * #1171 test 77. The window subscribes before it fetches, so a match arriving during the
 * fetch is in BOTH the stream and the snapshot. It must be shown exactly once.
 */
describe("merging the snapshot with the stream (#1171)", () => {
  it("neither duplicates nor loses a match that arrives during the fetch", () => {
    const streamed = freezeRow(match({ seq: 7 }), session());
    const fromSnapshot = [
      freezeRow(match({ seq: 6 }), session()),
      freezeRow(match({ seq: 7 }), session()), // the overlap
    ];

    const merged = mergeRows([streamed], fromSnapshot);

    expect(merged).toHaveLength(2);
    expect(merged.map((r) => r.seq).sort()).toEqual([6, 7]);
  });

  it("keeps two matches of the same tick apart, which only `seq` can do", () => {
    // Same `at`, same watcher, same row text: legal under `dedupe: "none"`.
    const a = freezeRow(match({ seq: 1 }), session());
    const b = freezeRow(match({ seq: 2 }), session());
    expect(rowKey(a)).not.toBe(rowKey(b));
    expect(mergeRows([], [a, b])).toHaveLength(2);
  });

  it("does not collapse the same seq coming from two different sessions", () => {
    const a = freezeRow(match({ sessionId: "s1", seq: 1 }), session({ id: "s1" }));
    const b = freezeRow(match({ sessionId: "s2", seq: 1 }), session({ id: "s2" }));
    expect(mergeRows([], [a, b])).toHaveLength(2);
  });

  it("orders newest first, breaking a tie on seq", () => {
    const older = freezeRow(match({ seq: 1, at: "2026-07-30T22:00:00Z" }), session());
    const newer = freezeRow(match({ seq: 2, at: "2026-07-30T23:00:00Z" }), session());
    const sameTick = freezeRow(match({ seq: 3, at: "2026-07-30T23:00:00Z" }), session());
    const merged = mergeRows([], [older, newer, sameTick]);
    expect(merged.map((r) => r.seq)).toEqual([3, 2, 1]);
  });

  it("is idempotent, so a poll that returns the same ring changes nothing", () => {
    const rows = [freezeRow(match({ seq: 1 }), session())];
    expect(mergeRows(rows, rows)).toHaveLength(1);
  });

  it("drops rows whose session left the scope", () => {
    const kept = freezeRow(match({ sessionId: "s1" }), session({ id: "s1" }));
    const gone = freezeRow(match({ sessionId: "s2" }), session({ id: "s2" }));
    expect(keepSessions([kept, gone], ["s1"]).map((r) => r.sessionId)).toEqual(["s1"]);
  });
});

/**
 * #1171 - the 500/100 limits are not only fetch parameters.
 *
 * The ring caps what a SNAPSHOT returns; the event stream keeps appending for as long as the
 * window is open, so without a structural bound after every merge the frontend grows without
 * one. A single session can deliver dozens of rows a second.
 */
describe("bounding what the window holds (#1171)", () => {
  const burst = (sessionId: string, count: number, from = 0) =>
    Array.from({ length: count }, (_, i) =>
      freezeRow(match({ sessionId, seq: from + i }), session({ id: sessionId }))
    );

  it("keeps at most the limit per session and drops the oldest", () => {
    const capped = capPerSession(mergeRows([], burst("s1", 250)), 100);
    expect(capped).toHaveLength(100);
    // Newest first, so the survivors are the highest seqs.
    expect(capped[0].seq).toBe(249);
    expect(Math.min(...capped.map((r) => r.seq))).toBe(150);
  });

  it("bounds each session on its own, not the table as a whole", () => {
    const both = mergeRows(burst("s1", 30), burst("s2", 30));
    const capped = capPerSession(both, 10);
    expect(capped.filter((r) => r.sessionId === "s1")).toHaveLength(10);
    expect(capped.filter((r) => r.sessionId === "s2")).toHaveLength(10);
  });

  /** Hours of streaming, one small batch at a time, is the shape that actually happens. */
  it("stays bounded across many merges rather than only at the end", () => {
    let held: ActivityRow[] = [];
    for (let round = 0; round < 200; round += 1) {
      held = capPerSession(mergeRows(held, burst("s1", 20, round * 20)), 100);
      expect(held.length).toBeLessThanOrEqual(100);
    }
    expect(held[0].seq).toBe(200 * 20 - 1);
  });

  it("leaves a table under the limit untouched", () => {
    const rows = mergeRows([], burst("s1", 5));
    expect(capPerSession(rows, 100).map((r) => r.seq)).toEqual(rows.map((r) => r.seq));
  });
});

/**
 * #1171 - Solid's `<For>` keys on object identity, not on the logical key, so a poll that
 * rebuilds the whole ring would rebuild and re-animate every row on screen every ten seconds.
 */
describe("row identity across merges (#1171)", () => {
  it("keeps the object it already held for a key it already has", () => {
    const held = freezeRow(match({ seq: 1 }), session());
    const identicalSnapshot = freezeRow(match({ seq: 1 }), session());
    expect(identicalSnapshot).not.toBe(held);

    const merged = mergeRows([held], [identicalSnapshot]);
    expect(merged[0]).toBe(held);
  });

  it("takes the newer object when the older one was frozen before its session was known", () => {
    const orphan = freezeRow(match({ seq: 1 }), undefined);
    const resolved = freezeRow(match({ seq: 1 }), session());
    const merged = mergeRows([orphan], [resolved]);
    expect(merged[0]).toBe(resolved);
    expect(merged[0].agentId).toBe("agent_1");
  });
});

describe("surfacing degraded watchers (#1171)", () => {
  /**
   * A degraded watcher has BY DEFINITION already emitted matches, so a marker that only
   * renders inside the "configured and waiting" branch is exactly where it can never be
   * reached: one visible row sends the window to the table instead.
   */
  it("reports a degraded watcher that has already matched", () => {
    const degraded = degradedWatchers([
      snapshot({
        activeWatchers: [
          { watcherId: "reads", mode: "occurrence", count: 4000, degraded: true },
          { watcherId: "quiet", mode: "state", count: 1, degraded: false },
        ],
      }),
    ]);
    expect(degraded.map((c) => c.watcherId)).toEqual(["reads"]);
  });

  it("reports nothing when no watcher is capped", () => {
    expect(degradedWatchers([snapshot()])).toEqual([]);
  });
});

describe("filtering (#1171)", () => {
  const rows = [
    row({ sessionId: "s1", seq: 1, watcherId: "reads", agentId: "a1", workgroup: "WG-1" }),
    row({ sessionId: "s2", seq: 2, watcherId: "errors", agentId: "a2", workgroup: "WG-2" }),
  ];
  const none = {
    watchers: new Set<string>(),
    agents: new Set<string>(),
    workgroups: new Set<string>(),
    text: "",
  };

  it("passes everything through when no filter is set", () => {
    expect(filterRows(rows, none)).toHaveLength(2);
  });

  it("ORs within one dimension", () => {
    const filtered = filterRows(rows, { ...none, watchers: new Set(["reads", "errors"]) });
    expect(filtered).toHaveLength(2);
  });

  it("ANDs between dimensions", () => {
    const filtered = filterRows(rows, {
      ...none,
      watchers: new Set(["reads"]),
      agents: new Set(["a2"]),
    });
    expect(filtered).toHaveLength(0);
  });

  it("matches free text against the captures, case-insensitively", () => {
    expect(filterRows(rows, { ...none, text: "MAIN.RS" })).toHaveLength(2);
    expect(filterRows(rows, { ...none, text: "nothing here" })).toHaveLength(0);
  });

  it("excludes a row with no workgroup once a workgroup filter is on", () => {
    const orphan = row({ sessionId: "s3", seq: 3, workgroup: null });
    expect(filterRows([orphan], { ...none, workgroups: new Set(["WG-1"]) })).toHaveLength(0);
  });
});

/**
 * #1171 test 81. The empty states, told apart from snapshot VALUES with no nullability --
 * and `warming` is its own state so the day-one message never flickers before the first tick.
 */
describe("resolving which state the window shows (#1171)", () => {
  it("shows warming until the engine has ticked, even with no watchers", () => {
    expect(resolveView([snapshot({ warmedUp: false })], 0, 0)).toBe("warming");
    expect(resolveView([], 0, 0)).toBe("warming");
  });

  it("shows the unconfigured state only once warmed up with nothing reaching", () => {
    expect(resolveView([snapshot({ warmedUp: true, activeWatchers: [] })], 0, 0)).toBe(
      "unconfigured"
    );
  });

  it("shows waiting when watchers reach but nothing has matched", () => {
    const waiting = snapshot({
      activeWatchers: [{ watcherId: "reads", mode: "occurrence", count: 0, degraded: false }],
    });
    expect(resolveView([waiting], 0, 0)).toBe("waiting");
  });

  it("shows the table as soon as a row is visible", () => {
    expect(resolveView([snapshot({ warmedUp: false })], 1, 1)).toBe("rows");
  });

  it("counts a scope as warmed when any of its sessions has ticked", () => {
    const view = resolveView(
      [snapshot({ warmedUp: false }), snapshot({ warmedUp: true })],
      0,
      0
    );
    expect(view).toBe("unconfigured");
  });

  /**
   * The empty states are statements about the ACTIVATIONS. Deciding them from filtered rows
   * makes a filter that happens to hide everything say "nothing has matched yet", which is
   * false and which the user cannot attribute to the filter they just set.
   */
  it("blames the filters, not the watchers, when a filter hides every activation", () => {
    const waiting = snapshot({
      activeWatchers: [{ watcherId: "reads", mode: "occurrence", count: 4, degraded: false }],
    });
    expect(resolveView([waiting], 4, 0)).toBe("filtered");
  });

  it("still says filtered when nothing reaches but activations are held", () => {
    // Rows outlive the watcher that produced them: deleting a watcher empties
    // `activeWatchers` while the ring keeps its activations, and a filter over those must
    // not be reported as "no configured watcher reaches this agent".
    expect(resolveView([snapshot({ warmedUp: true, activeWatchers: [] })], 3, 0)).toBe(
      "filtered"
    );
  });
});

describe("aggregating snapshots across a scope (#1171)", () => {
  it("sums a watcher's counts and keeps degraded sticky across sessions", () => {
    const merged = mergeActiveWatchers([
      snapshot({
        activeWatchers: [{ watcherId: "reads", mode: "occurrence", count: 2, degraded: false }],
      }),
      snapshot({
        activeWatchers: [{ watcherId: "reads", mode: "occurrence", count: 3, degraded: true }],
      }),
    ]);
    expect(merged).toEqual([
      { watcherId: "reads", mode: "occurrence", count: 5, degraded: true },
    ]);
  });

  it("reports truncation if any session dropped entries", () => {
    expect(anyTruncated([snapshot(), snapshot({ truncated: true })])).toBe(true);
    expect(anyTruncated([snapshot(), snapshot()])).toBe(false);
  });

  it("sums the unaligned frames across the scope", () => {
    expect(
      totalPossiblyMissedFrames([
        snapshot({ possiblyMissedFrames: 2 }),
        snapshot({ possiblyMissedFrames: 5 }),
      ])
    ).toBe(7);
  });
});
