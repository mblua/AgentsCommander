import { describe, expect, it } from "vitest";
import type { AgentConfig, WatcherConfig, WatcherEntry } from "../../shared/types";
import {
  distinctCommandStems,
  isWatcherConfig,
  newWatcherConfig,
  nextWatcherId,
  renameWatcherEntry,
  selectorMode,
  sortedWatcherIds,
  toggleCommandStem,
  validateWatcherId,
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

  // The exact hand-written mistakes the Rust wrapper exists to survive.
  it('rejects a capitalized mode, which is what "mode": "State" deserializes to', () => {
    expect(isWatcherConfig({ ...config(), mode: "State" } as WatcherEntry)).toBe(false);
  });

  it("rejects a string where commands must be a list", () => {
    expect(isWatcherConfig({ ...config(), commands: "claude" } as WatcherEntry)).toBe(false);
  });

  it("rejects a stringly-typed dedupe window", () => {
    expect(
      isWatcherConfig({ ...config(), dedupeWindowMs: "2000" } as WatcherEntry)
    ).toBe(false);
  });

  it("rejects a missing pattern and a missing mode", () => {
    const { pattern: _pattern, ...noPattern } = config();
    const { mode: _mode, ...noMode } = config();
    expect(isWatcherConfig(noPattern as WatcherEntry)).toBe(false);
    expect(isWatcherConfig(noMode as WatcherEntry)).toBe(false);
  });

  it("rejects a non-object entry", () => {
    expect(isWatcherConfig("claude" as unknown as WatcherEntry)).toBe(false);
    expect(isWatcherConfig(null as unknown as WatcherEntry)).toBe(false);
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
    const unreadable = { mode: "State", pattern: 7 } as WatcherEntry;
    const watchers: Record<string, WatcherEntry> = {
      keep: config({ pattern: "keep" }),
      old: config({ pattern: "moved" }),
      broken: unreadable,
    };

    const renamed = renameWatcherEntry(watchers, "old", "new");

    expect(Object.keys(renamed).sort()).toEqual(["broken", "keep", "new"]);
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
