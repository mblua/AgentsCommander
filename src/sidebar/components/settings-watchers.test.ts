import { describe, expect, it } from "vitest";
import type {
  AgentConfig,
  WatcherConfig,
  WatcherEntry,
  WatcherReachEntry,
} from "../../shared/types";
import {
  canEnableWatcher,
  distinctCommandStems,
  isWatcherConfig,
  newWatcherConfig,
  nextWatcherId,
  reachRequestFingerprint,
  renameWatcherEntry,
  selectorMode,
  sortedWatcherIds,
  toggleCommandStem,
  validateWatcherId,
  watcherBudgetNotice,
  watcherReachRequest,
  watcherReachSummary,
  withSelectorMode,
} from "./settings-watchers";

function agent(id: string, command: string): AgentConfig {
  return {
    id,
    label: id,
    command,
    color: "#6366f1",
    envs: [],
    isolatedHome: false,
  };
}

function config(overrides: Partial<WatcherConfig> = {}): WatcherConfig {
  return { ...newWatcherConfig(), ...overrides };
}

describe("isWatcherConfig (#1171)", () => {
  it("accepts an entry carrying every field the Rust serializer always writes", () => {
    expect(isWatcherConfig(config())).toBe(true);
  });

  it("accepts commands absent, null or empty, which are three different meanings", () => {
    expect(isWatcherConfig(config({ commands: undefined }))).toBe(true);
    expect(isWatcherConfig(config({ commands: null }))).toBe(true);
    expect(isWatcherConfig(config({ commands: [] }))).toBe(true);
  });

  // The exact hand-written mistakes the Rust wrapper exists to survive. Written as plain
  // JSON, with no cast, because that is what the file holds and what `WatcherEntry` now says.
  it('rejects a capitalized mode, which is what "mode": "State" deserializes to', () => {
    const entry: WatcherEntry = {
      enabled: true,
      mode: "State",
      pattern: "x",
      dedupe: "row",
      dedupeWindowMs: 2000,
    };
    expect(isWatcherConfig(entry)).toBe(false);
  });

  it("rejects a string where commands must be a list", () => {
    const entry: WatcherEntry = {
      enabled: true,
      mode: "state",
      pattern: "x",
      commands: "claude",
      dedupe: "row",
      dedupeWindowMs: 2000,
    };
    expect(isWatcherConfig(entry)).toBe(false);
  });

  it("rejects a stringly-typed dedupe window", () => {
    const entry: WatcherEntry = {
      enabled: true,
      mode: "state",
      pattern: "x",
      dedupe: "row",
      dedupeWindowMs: "2000",
    };
    expect(isWatcherConfig(entry)).toBe(false);
  });

  it("rejects a missing pattern and a missing mode", () => {
    const { pattern: _pattern, ...noPattern } = config();
    const { mode: _mode, ...noMode } = config();
    expect(isWatcherConfig(noPattern)).toBe(false);
    expect(isWatcherConfig(noMode)).toBe(false);
  });

  /**
   * `WatcherEntry::Invalid` holds a `serde_json::Value`, so the entry Rust preserved and
   * handed back can be any JSON value at all. A predicate that only ever sees objects is one
   * whose callers had to lie to it with a cast.
   */
  it("rejects every non-object JSON value Rust can just as easily preserve", () => {
    const entries: WatcherEntry[] = [null, 7, -1.5, "claude", true, false, ["claude"], []];
    for (const entry of entries) {
      expect(isWatcherConfig(entry)).toBe(false);
    }
  });

  /**
   * #1171 test 58g, the whole numeric contract and not only the obvious member of it.
   *
   * `typeof n === "number" && n >= 0` is the naive correction and it must FAIL this: it still
   * admits `1.5` and `1e30`, both of which serde rejects for a `u64`, so both would be sent
   * to `preview_watcher_reach`, counted against an agent's budget, possibly push a real
   * watcher out of it, and then be skipped by the engine after Save. `-1` alone does not
   * exercise the gap.
   */
  it("rejects every dedupe window the Rust decoder rejects, not only the negative one", () => {
    for (const dedupeWindowMs of [-1, 1.5, 1e30, Number.MAX_SAFE_INTEGER + 2, NaN]) {
      expect(isWatcherConfig(config({ dedupeWindowMs }))).toBe(false);
    }
  });

  it("accepts the boundary it can still hold exactly", () => {
    expect(isWatcherConfig(config({ dedupeWindowMs: 0 }))).toBe(true);
    expect(isWatcherConfig(config({ dedupeWindowMs: Number.MAX_SAFE_INTEGER }))).toBe(true);
  });

  // Deliberately narrower than `u64`: JavaScript cannot hold a value above 2^53 exactly, and
  // rounding one silently would be worse than declining to edit it. The engine still runs
  // such a row, clamped; the editor lists it as unrecognised.
  it("classifies a hand-written window above 2^53 as unrecognised rather than rounding it", () => {
    expect(isWatcherConfig(config({ dedupeWindowMs: 2 ** 53 + 2 }))).toBe(false);
  });

  it("requires every commands entry to be a string, not merely an array", () => {
    expect(isWatcherConfig(config({ commands: [1] as unknown as string[] }))).toBe(false);
    expect(isWatcherConfig(config({ commands: ["claude", "codex"] }))).toBe(true);
  });
});

/**
 * #1171 - the birth state of a new row is a decision, not a preference.
 *
 * An empty pattern is a valid regex that matches every row, so a row born enabled turns Add
 * plus an accidental Save into a watcher that matches everything on every agent.
 */
describe("a brand-new watcher row (#1171 test 58k)", () => {
  it("is born disabled", () => {
    expect(newWatcherConfig().enabled).toBe(false);
  });

  it("cannot be enabled until it has a pattern", () => {
    expect(canEnableWatcher(newWatcherConfig())).toBe(false);
    expect(canEnableWatcher(config({ pattern: "Read" }))).toBe(true);
    // A single space is a pattern: it is an anchor, and the engine takes it verbatim.
    expect(canEnableWatcher(config({ pattern: " " }))).toBe(true);
  });
});

/**
 * #1171 test 58g and 58h, the request half.
 *
 * Both halves of the draft travel and nothing comes from disk, because running is a property
 * of the whole set and the modal edits agents and watchers in one store that one Save writes
 * together.
 */
describe("the reach request (#1171)", () => {
  const agents = [agent("a1", "claude"), agent("a2", "codex")];

  it("carries every valid row in key order, plus the draft agents", () => {
    const request = watcherReachRequest(
      { b: config({ pattern: "b" }), a: config({ pattern: "a", enabled: true }) },
      agents
    );
    expect(request.watchers.map((row) => row.id)).toEqual(["a", "b"]);
    expect(request.watchers[0]).toEqual({ id: "a", enabled: true, commands: null });
    expect(request.agents).toEqual([
      { id: "a1", label: "a1", command: "claude" },
      { id: "a2", label: "a2", command: "codex" },
    ]);
  });

  // A row this predicate calls valid and serde does not would be counted against the budget
  // and then skipped by the engine, which is the same false positive the draft shape exists
  // to remove.
  it("leaves out every row the Rust decoder would reject", () => {
    const request = watcherReachRequest(
      {
        good: config({ pattern: "ok" }),
        badCommands: config({ commands: [1] as unknown as string[] }),
        negative: config({ dedupeWindowMs: -1 }),
        fractional: config({ dedupeWindowMs: 1.5 }),
        huge: config({ dedupeWindowMs: 1e30 }),
        unsafe: config({ dedupeWindowMs: Number.MAX_SAFE_INTEGER + 2 }),
        capitalized: {
          enabled: true,
          mode: "State",
          pattern: "x",
          dedupe: "row",
          dedupeWindowMs: 2000,
        },
        scalar: "claude",
        nothing: null,
      },
      agents
    );
    expect(request.watchers.map((row) => row.id)).toEqual(["good"]);
  });

  it("normalizes absent and null commands to one request, because they mean one thing", () => {
    const absent = watcherReachRequest({ a: config({ commands: undefined }) }, agents);
    const explicit = watcherReachRequest({ a: config({ commands: null }) }, agents);
    expect(reachRequestFingerprint(absent)).toBe(reachRequestFingerprint(explicit));
    // And `[]` stays the opposite of both.
    const none = watcherReachRequest({ a: config({ commands: [] }) }, agents);
    expect(reachRequestFingerprint(none)).not.toBe(reachRequestFingerprint(absent));
  });

  /**
   * The fingerprint is what makes the guard consistent with itself: keying on "any change to
   * the draft" clears the answer on a `pattern` keystroke and then issues no call to replace
   * it, leaving the row pending forever.
   */
  it("is unchanged by a pattern keystroke and changed by everything that resolves", () => {
    const base = { a: config({ pattern: "Read" }) };
    const fingerprint = reachRequestFingerprint(watcherReachRequest(base, agents));

    const typed = { a: config({ pattern: "Read (" }) };
    expect(reachRequestFingerprint(watcherReachRequest(typed, agents))).toBe(fingerprint);

    const changes: Record<string, WatcherEntry>[] = [
      { a: config({ pattern: "Read", enabled: true }) },
      { a: config({ pattern: "Read", commands: ["claude"] }) },
      { a: config({ pattern: "Read" }), b: config({ pattern: "x" }) },
      {},
    ];
    for (const changed of changes) {
      expect(reachRequestFingerprint(watcherReachRequest(changed, agents))).not.toBe(
        fingerprint
      );
    }

    // An agent edit resolves too: deleting one and changing one's command both over-report
    // if the saved list is used instead.
    expect(
      reachRequestFingerprint(watcherReachRequest(base, [agent("a1", "codex"), agents[1]]))
    ).not.toBe(fingerprint);
    expect(reachRequestFingerprint(watcherReachRequest(base, [agents[0]]))).not.toBe(
      fingerprint
    );
  });
});

/** #1171 test 58i, the three wordings, which answer three different questions. */
describe("how a row states its reach (#1171 test 58i)", () => {
  const entry = (agentId: string, allocated: boolean): WatcherReachEntry => ({
    agentId,
    agentLabel: agentId.toUpperCase(),
    commandStem: "claude",
    allocated,
  });

  it("uses the present tense and the budget badge only for an enabled row", () => {
    const enabled = config({ enabled: true, pattern: "Read" });
    const entries = [entry("a1", true), entry("a2", false)];
    expect(watcherReachSummary(enabled, entries)).toBe("Reaches 2 agents.");
    expect(watcherBudgetNotice(enabled, entries)).toBe(" Not running on A2 (budget).");
  });

  // A disabled row holds no slot BECAUSE it is disabled, so naming that a budget outcome
  // would name the wrong cause.
  it("uses the conditional and shows no badge for a disabled row with a pattern", () => {
    const disabled = config({ enabled: false, pattern: "Read" });
    const entries = [entry("a1", false)];
    expect(watcherReachSummary(disabled, entries)).toBe(
      "Would reach 1 agent when enabled."
    );
    expect(watcherBudgetNotice(disabled, entries)).toBe("");
  });

  // The first state anyone sees after Add Watcher. "When enabled" alone offers a condition
  // the editor refuses to let them meet, so the missing one is named first.
  it("names the missing pattern first for a disabled row that has none", () => {
    expect(watcherReachSummary(newWatcherConfig(), [entry("a1", false)])).toBe(
      "Would reach 1 agent when enabled. Add a pattern to enable it."
    );
  });

  it("reports reaching nobody without pretending it is an error", () => {
    expect(watcherReachSummary(config({ enabled: true, pattern: "x" }), [])).toBe(
      "Reaches 0 agents."
    );
  });
});

describe("the commands selector (#1171)", () => {
  // The whole point of the two-state control: an empty multiselect must not become "[]".
  it("reports absent and null as All agents, and any list as Selected", () => {
    expect(selectorMode(config({ commands: undefined }))).toBe("all");
    expect(selectorMode(config({ commands: null }))).toBe("all");
    expect(selectorMode(config({ commands: [] }))).toBe("selected");
    expect(selectorMode(config({ commands: ["claude"] }))).toBe("selected");
  });

  it("writes null for All agents and a list for Selected, keeping the two distinct", () => {
    expect(withSelectorMode(config({ commands: ["claude"] }), "all").commands).toBeNull();
    expect(withSelectorMode(config({ commands: null }), "selected").commands).toEqual([]);
  });

  it("restores the list the user had picked when switching back to Selected", () => {
    const picked = config({ commands: ["claude", "codex"] });
    const all = withSelectorMode(picked, "all");
    expect(withSelectorMode(all, "selected").commands).toEqual([]);
  });

  it("toggles one stem without leaving the Selected state", () => {
    const start = config({ commands: [] });
    const added = toggleCommandStem(start, "claude");
    expect(added.commands).toEqual(["claude"]);
    expect(toggleCommandStem(added, "claude").commands).toEqual([]);
    expect(selectorMode(toggleCommandStem(added, "claude"))).toBe("selected");
  });
});

describe("watcher ids (#1171)", () => {
  it("accepts the documented shape and rejects what it does not match", () => {
    expect(validateWatcherId("permission-prompt", [])).toBeNull();
    expect(validateWatcherId("w1", [])).toBeNull();
    expect(validateWatcherId("", [])).not.toBeNull();
    expect(validateWatcherId("Permission", [])).not.toBeNull();
    expect(validateWatcherId("-leading", [])).not.toBeNull();
    expect(validateWatcherId("has space", [])).not.toBeNull();
    expect(validateWatcherId("a".repeat(41), [])).not.toBeNull();
    expect(validateWatcherId("a".repeat(40), [])).toBeNull();
  });

  it("rejects an id another watcher already holds", () => {
    expect(validateWatcherId("taken", ["taken"])).not.toBeNull();
    expect(validateWatcherId("free", ["taken"])).toBeNull();
  });

  it("never proposes an id that already exists", () => {
    expect(nextWatcherId([])).toBe("watcher-1");
    expect(nextWatcherId(["watcher-1", "watcher-2"])).toBe("watcher-3");
  });
});

describe("renaming a watcher (#1171)", () => {
  // Renaming is delete plus create, so what must be proven is that it takes nothing else
  // with it -- above all the entry this build could not read.
  it("moves one key and preserves every other entry, unreadable ones included", () => {
    const unreadable: WatcherEntry = { mode: "State", pattern: 7 };
    const watchers: Record<string, WatcherEntry> = {
      keep: config({ pattern: "keep" }),
      old: config({ pattern: "moved" }),
      broken: unreadable,
      // The shapes a hand-written file can hold that are not objects at all.
      scalar: "claude",
      nothing: null,
    };

    const renamed = renameWatcherEntry(watchers, "old", "new");

    expect(Object.keys(renamed).sort()).toEqual([
      "broken",
      "keep",
      "new",
      "nothing",
      "scalar",
    ]);
    expect(renamed["scalar"]).toBe("claude");
    expect(renamed["nothing"]).toBeNull();
    expect(renamed["new"]).toEqual(config({ pattern: "moved" }));
    expect(renamed["keep"]).toEqual(config({ pattern: "keep" }));
    expect(renamed["broken"]).toBe(unreadable);
  });

  it("leaves the map alone when the id is not there", () => {
    const watchers: Record<string, WatcherEntry> = { a: config() };
    expect(renameWatcherEntry(watchers, "missing", "new")).toEqual(watchers);
  });
});

describe("command stems for the Selected options (#1171)", () => {
  it("collapses five entries sharing one executable into one option", () => {
    const stems = distinctCommandStems([
      agent("a", "claude"),
      agent("b", String.raw`C:\tools\claude-sandbox-runtime\claude.cmd`),
      agent("c", "CLAUDE.EXE"),
      agent("d", "codex --sandbox workspace-write"),
      agent("e", "claude"),
    ]);
    expect(stems).toEqual(["claude", "codex"]);
  });

  it("ignores an agent with no command", () => {
    expect(distinctCommandStems([agent("a", "")])).toEqual([]);
  });
});

describe("watcher map ordering (#1171)", () => {
  it("lists ids in key order, which is the order the budget resolves in", () => {
    expect(sortedWatcherIds({ b: config(), a: config(), c: config() })).toEqual([
      "a",
      "b",
      "c",
    ]);
  });

  it("treats an absent map as empty", () => {
    expect(sortedWatcherIds(undefined)).toEqual([]);
  });
});
