import { describe, expect, it } from "vitest";
import type {
  Session,
  WatcherActivitySnapshot,
  WatcherMatchPayload,
} from "../shared/types";
import {
  type ActivityRow,
  anyTruncated,
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
 * #1171 test 81. Four states, told apart from snapshot VALUES with no nullability -- and
 * `warming` is its own state so the day-one message never flickers before the first tick.
 */
describe("resolving which state the window shows (#1171)", () => {
  it("shows warming until the engine has ticked, even with no watchers", () => {
    expect(resolveView([snapshot({ warmedUp: false })], 0)).toBe("warming");
    expect(resolveView([], 0)).toBe("warming");
  });

  it("shows the unconfigured state only once warmed up with nothing reaching", () => {
    expect(resolveView([snapshot({ warmedUp: true, activeWatchers: [] })], 0)).toBe(
      "unconfigured"
    );
  });

  it("shows waiting when watchers reach but nothing has matched", () => {
    const waiting = snapshot({
      activeWatchers: [{ watcherId: "reads", mode: "occurrence", count: 0, degraded: false }],
    });
    expect(resolveView([waiting], 0)).toBe("waiting");
  });

  it("shows the table as soon as a row is visible", () => {
    expect(resolveView([snapshot({ warmedUp: false })], 1)).toBe("rows");
  });

  it("counts a scope as warmed when any of its sessions has ticked", () => {
    const view = resolveView([snapshot({ warmedUp: false }), snapshot({ warmedUp: true })], 0);
    expect(view).toBe("unconfigured");
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
