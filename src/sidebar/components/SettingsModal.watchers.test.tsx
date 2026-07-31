// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  AgentConfig,
  AppSettings,
  JsonValue,
  WatcherConfig,
  WatcherEntry,
  WatcherReachRow,
} from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import SettingsModal, { resolveSettingsSection } from "./SettingsModal";
import { mergeSettingsForSavePreservingProjects } from "./settings-save";
import { newWatcherConfig } from "./settings-watchers";

/**
 * #1171 test 59.
 *
 * `resolveSettingsSection` falls back to `"general"` for anything it does not know, with no
 * error and no log. That silence is why this is pinned: the watcher activity window's
 * day-one empty state offers a "Configure watchers" button, and an unwired section would
 * land the user on General with nothing to explain it.
 *
 * The same gap is why `"resources"` is worth noticing: it is a member of `SettingsTab` and
 * an entry in `TABS`, the Resource Monitor asks for it by name, and it resolves to
 * `"general"` all the same. That is the broken precedent this test exists to not repeat.
 */
describe("resolveSettingsSection (#1171)", () => {
  it('maps "watchers" to the Watchers tab and not to General', () => {
    expect(resolveSettingsSection("watchers")).toBe("watchers");
  });

  it("keeps every section that already resolved", () => {
    expect(resolveSettingsSection("agents")).toBe("agents");
    expect(resolveSettingsSection("profiles")).toBe("agents");
    expect(resolveSettingsSection("integrations")).toBe("integrations");
  });

  it("still falls back to General for an unknown or absent section", () => {
    expect(resolveSettingsSection(undefined)).toBe("general");
    expect(resolveSettingsSection("nope")).toBe("general");
  });
});

/**
 * #1171 - the editor must not delete what it could not read.
 *
 * The Rust `WatcherEntry` is untagged so a hand-written `"mode": "State"` costs one skipped
 * watcher instead of the whole `AppSettings` parse, and the invalid value is kept verbatim
 * so a save round-trips the user's bytes. That guarantee only survives if the frontend's
 * save path carries the entry through untouched, which is what this pins.
 */
describe("saving a settings draft that holds an unreadable watcher (#1171)", () => {
  const unreadable = { mode: "State", commands: "claude" };

  it("carries the unreadable entry and the valid one through the save merge", () => {
    const draft = baseSettings({
      watchers: {
        broken: unreadable,
        good: newWatcherConfig(),
      },
    });

    const merged = mergeSettingsForSavePreservingProjects(draft, baseSettings(), draft);

    expect(Object.keys(merged.watchers ?? {}).sort()).toEqual(["broken", "good"]);
    expect(merged.watchers?.broken).toEqual(unreadable);
    expect(merged.watchers?.good).toEqual(newWatcherConfig());
  });

  it("leaves the map absent when nothing was ever configured", () => {
    const merged = mergeSettingsForSavePreservingProjects(
      baseSettings(),
      baseSettings(),
      baseSettings()
    );
    expect(merged.watchers).toBeUndefined();
  });
});

/**
 * #1171 - "All agents" and an empty "Selected" are opposites, and they have to stay
 * opposites all the way to the saved draft.
 *
 * `null` reaches every configured agent; `[]` reaches none. A plain multiselect cannot
 * express both, which is why the selector is a mode. The other tests pin the pure
 * functions; this one pins the payload that actually leaves the modal.
 */
describe("the watcher commands selector, through a real save (#1171)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  function watcherSettings(commands: WatcherConfig["commands"]): AppSettings {
    return baseSettings({
      watchers: { probe: { ...newWatcherConfig(), pattern: "Read", commands } },
    });
  }

  async function savedWatcher(
    initial: AppSettings,
    startsAs: "all" | "selected",
    pick: (root: HTMLElement) => void
  ): Promise<WatcherEntry | undefined> {
    const fake = new FakeTransport();
    fake.resolve("get_settings", initial);
    fake.resolve("get_web_server_status", false);
    fake.resolve("preview_watcher_reach", []);
    fake.resolve("preview_watcher_pattern", {
      compiles: true,
      error: null,
      sampled: false,
      matchedRows: 0,
      totalRows: 0,
      samples: [],
      capturesVolatile: false,
    });
    fake.resolve("save_settings_draft", undefined);

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="watchers" onClose={() => {}} />,
      fake
    );
    try {
      // Wait for THIS test's settings, not the seed the modal paints from
      // `settingsStore.current` while `get_settings` is still in flight.
      await waitFor(() =>
        expect(
          rendered.root.querySelector<HTMLSelectElement>(
            '[data-ac-testid="settings.watchers.selectorMode.probe"]'
          )?.value
        ).toBe(startsAs)
      );

      pick(rendered.root);

      rendered.root
        .querySelector<HTMLButtonElement>('[data-ac-testid="settings.save"]')!
        .click();

      await waitFor(() => expect(fake.lastCall("save_settings_draft")).toBeTruthy());
      const draft = fake.lastCall("save_settings_draft")!.args as { draft: AppSettings };
      return draft.draft.watchers?.probe;
    } finally {
      rendered.cleanup();
    }
  }

  function selectMode(root: HTMLElement, value: "all" | "selected") {
    const select = root.querySelector<HTMLSelectElement>(
      '[data-ac-testid="settings.watchers.selectorMode.probe"]'
    )!;
    select.value = value;
    select.dispatchEvent(new Event("change", { bubbles: true }));
  }

  it('saves null when the user picks "All agents"', async () => {
    const saved = await savedWatcher(watcherSettings(["claude"]), "selected", (root) =>
      selectMode(root, "all")
    );
    expect((saved as WatcherConfig).commands).toBeNull();
  });

  it('saves an empty list, not null, when "Selected" is left empty', async () => {
    const saved = await savedWatcher(watcherSettings(null), "all", (root) =>
      selectMode(root, "selected")
    );
    expect((saved as WatcherConfig).commands).toEqual([]);
  });

  // A malformed entry must cost one watcher, not the tab and not the file.
  it("renders the readable watchers, names the unreadable one, and edits neither by accident", async () => {
    const fake = new FakeTransport();
    fake.resolve(
      "get_settings",
      baseSettings({
        watchers: {
          broken: { mode: "State", commands: "claude" },
          good: { ...newWatcherConfig(), pattern: "Read" },
        },
      })
    );
    fake.resolve("get_web_server_status", false);
    fake.resolve("preview_watcher_reach", []);

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="watchers" onClose={() => {}} />,
      fake
    );
    try {
      await waitFor(() =>
        expect(
          rendered.root.querySelector('[data-ac-testid="settings.watchers.row.good"]')
        ).toBeTruthy()
      );

      expect(
        rendered.root.querySelector('[data-ac-testid="settings.watchers.row.broken"]')
      ).toBeNull();

      const notice = rendered.root.querySelector(
        '[data-ac-testid="settings.watchers.unreadable"]'
      );
      expect(notice?.textContent).toContain("broken");
    } finally {
      rendered.cleanup();
    }
  });
});

/** The debounce the section waits before asking the backend, mirrored from the modal. */
const REACH_DEBOUNCE_MS = 300;

const AGENT: AgentConfig = {
  id: "a1",
  label: "Claude",
  command: "claude",
  color: "#6366f1",
  envs: [],
  isolatedHome: false,
};

function reachRow(id: string, allocated = true): WatcherReachRow {
  return {
    id,
    entries: [
      { agentId: "a1", agentLabel: "Claude", commandStem: "claude", allocated },
    ],
  };
}

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason: unknown) => void;
} {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/**
 * #1171 test 58h. The reach answer belongs to exactly one REQUEST, and the guard is keyed on
 * that request rather than on "any change to the draft".
 *
 * The pair this replaces contradicted itself: an idempotence guard ("skip the call when the
 * serialized request is unchanged") next to a clear-on-any-change rule means a `pattern`
 * keystroke clears the answer and issues no call to replace it, leaving the row pending
 * forever. That case is the last assertion here and it must fail against the old pair.
 */
describe("the reach preview, keyed on the request fingerprint (#1171 test 58h)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  function threeRows(): AppSettings {
    return baseSettings({
      agents: [AGENT],
      watchers: {
        alpha: { ...newWatcherConfig(), pattern: "a" },
        beta: { ...newWatcherConfig(), pattern: "b" },
        gamma: { ...newWatcherConfig(), pattern: "c" },
      },
    });
  }

  function transport(
    reach: (args: Record<string, unknown>) => unknown = () =>
      [reachRow("alpha"), reachRow("beta"), reachRow("gamma")]
  ): FakeTransport {
    const fake = new FakeTransport();
    fake.resolve("get_settings", threeRows());
    fake.resolve("get_web_server_status", false);
    fake.resolve("save_settings_draft", undefined);
    fake.onInvoke("preview_watcher_reach", reach);
    fake.resolve("preview_watcher_pattern", {
      compiles: true,
      error: null,
      sampled: false,
      matchedRows: 0,
      totalRows: 0,
      samples: [],
      capturesVolatile: false,
    });
    return fake;
  }

  /** Mount, let the settings land, and let the first debounced round settle. */
  async function mounted(fake: FakeTransport) {
    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="watchers" onClose={() => {}} />,
      fake
    );
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(REACH_DEBOUNCE_MS);
    await vi.advanceTimersByTimeAsync(0);
    return rendered;
  }

  const reachText = (root: HTMLElement, id: string) =>
    root.querySelector(`[data-ac-testid="settings.watchers.reach.${id}"]`)?.textContent ?? "";

  const selectorMode = (root: HTMLElement, id: string, value: "all" | "selected") => {
    const select = root.querySelector<HTMLSelectElement>(
      `[data-ac-testid="settings.watchers.selectorMode.${id}"]`
    )!;
    select.value = value;
    select.dispatchEvent(new Event("change", { bubbles: true }));
  };

  it("sends one call for the whole draft, watchers and agents together", async () => {
    const fake = transport();
    const rendered = await mounted(fake);
    try {
      const call = fake.lastCall("preview_watcher_reach")!;
      const sent = call.args as {
        watchers: { id: string }[];
        agents: { id: string; command: string }[];
      };
      expect(sent.watchers.map((row) => row.id)).toEqual(["alpha", "beta", "gamma"]);
      expect(sent.agents).toEqual([{ id: "a1", label: "Claude", command: "claude" }]);
    } finally {
      rendered.cleanup();
    }
  });

  it("fires exactly one call for a selector edit, an add, a toggle and a delete", async () => {
    const fake = transport();
    const rendered = await mounted(fake);
    try {
      for (const act of [
        () => selectorMode(rendered.root, "alpha", "selected"),
        () =>
          click(
            rendered.root.querySelector('[data-ac-testid="settings.watchers.add"]')!
          ),
        () =>
          click(
            rendered.root.querySelector('[data-ac-testid="settings.watchers.enabled.alpha"]')!
          ),
        () =>
          click(
            rendered.root.querySelector('[data-ac-testid="settings.watchers.remove.gamma"]')!
          ),
      ]) {
        fake.clearCalls();
        act();
        await vi.advanceTimersByTimeAsync(REACH_DEBOUNCE_MS);
        await vi.advanceTimersByTimeAsync(0);
        expect(fake.callsFor("preview_watcher_reach")).toHaveLength(1);
      }
    } finally {
      rendered.cleanup();
    }
  });

  /**
   * The agent half of the draft, which is the fail-open the amendment closed. The modal edits
   * agents and watchers in ONE store that one Save writes together, so resolving against the
   * saved agent list answers about a state the user has already left -- and two of the three
   * agent edits over-report that way: deleting an agent leaves it named in a reach list it
   * will not be in, and changing an agent's `command` leaves a watcher reported as reaching it
   * under the old stem.
   */
  it("re-asks with the edited agent when the draft changes an agent's command", async () => {
    const fake = transport();
    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="agents" onClose={() => {}} />,
      fake
    );
    try {
      await vi.advanceTimersByTimeAsync(0);
      await vi.advanceTimersByTimeAsync(REACH_DEBOUNCE_MS);
      await vi.advanceTimersByTimeAsync(0);
      fake.clearCalls();

      // The command field only exists once the row's editor is open.
      click(rendered.root.querySelector('[data-ac-testid="settings.agentRow.0.toggle"]')!);
      await vi.advanceTimersByTimeAsync(0);
      input(
        rendered.root.querySelector<HTMLInputElement>(
          '[data-ac-testid="settings.agentRow.0.command"]'
        )!,
        "codex"
      );
      await vi.advanceTimersByTimeAsync(REACH_DEBOUNCE_MS);
      await vi.advanceTimersByTimeAsync(0);

      const calls = fake.callsFor("preview_watcher_reach");
      expect(calls).toHaveLength(1);
      expect((calls[0].args as { agents: { command: string }[] }).agents).toEqual([
        { id: "a1", label: "Claude", command: "codex" },
      ]);
    } finally {
      rendered.cleanup();
    }
  });

  /**
   * Emptying the pattern clears the preview, but clearing without advancing the generation
   * leaves an older answer entitled to paint: the row then reads "Compiles" over a pattern
   * that no longer exists.
   */
  it("discards a pattern preview in flight when the pattern is emptied", async () => {
    const pending = deferred<unknown>();
    const fake = transport();
    fake.onInvoke("preview_watcher_pattern", () => pending.promise);
    const rendered = await mounted(fake);
    try {
      expect(fake.callsFor("preview_watcher_pattern").length).toBeGreaterThan(0);

      input(
        rendered.root.querySelector<HTMLInputElement>(
          '[data-ac-testid="settings.watchers.pattern.alpha"]'
        )!,
        ""
      );
      await vi.advanceTimersByTimeAsync(0);

      pending.resolve({
        compiles: true,
        error: null,
        sampled: true,
        matchedRows: 30,
        totalRows: 30,
        samples: [],
        capturesVolatile: false,
      });
      await vi.advanceTimersByTimeAsync(REACH_DEBOUNCE_MS * 2);
      await vi.advanceTimersByTimeAsync(0);

      expect(
        rendered.root.querySelector('[data-ac-testid="settings.watchers.preview.alpha"]')
          ?.textContent
      ).not.toContain("Compiles");
    } finally {
      rendered.cleanup();
    }
  });

  /**
   * The permanent-pending case. A `pattern` keystroke changes the draft but NOT the request,
   * so it must change nothing at all: no call, and above all no clearing.
   */
  it("neither calls nor clears on a pattern keystroke", async () => {
    const fake = transport();
    const rendered = await mounted(fake);
    try {
      const before = reachText(rendered.root, "alpha");
      expect(before).toContain("Would reach 1 agent when enabled");

      fake.clearCalls();
      input(
        rendered.root.querySelector<HTMLInputElement>(
          '[data-ac-testid="settings.watchers.pattern.alpha"]'
        )!,
        "Read (.+)"
      );
      await vi.advanceTimersByTimeAsync(REACH_DEBOUNCE_MS * 4);
      await vi.advanceTimersByTimeAsync(0);

      expect(fake.callsFor("preview_watcher_reach")).toHaveLength(0);
      expect(reachText(rendered.root, "alpha")).toBe(before);
    } finally {
      rendered.cleanup();
    }
  });

  /**
   * Clearing has to be synchronous. A commit-time guard stops a stale answer from being
   * WRITTEN, not from being READ, and this indicator is what a user consults before pressing
   * Save.
   */
  it("clears the displayed answer at once and leaves it cleared until its own answer lands", async () => {
    const pending = deferred<WatcherReachRow[]>();
    let round = 0;
    const fake = transport(() => {
      round += 1;
      return round === 1
        ? [reachRow("alpha"), reachRow("beta"), reachRow("gamma")]
        : pending.promise;
    });
    const rendered = await mounted(fake);
    try {
      expect(reachText(rendered.root, "alpha")).toContain("Would reach 1 agent");

      selectorMode(rendered.root, "alpha", "selected");
      await vi.advanceTimersByTimeAsync(0);
      expect(reachText(rendered.root, "alpha")).toBe("Resolving reach...");

      // The call is out and still unanswered: nothing reappears in the meantime.
      await vi.advanceTimersByTimeAsync(REACH_DEBOUNCE_MS);
      await vi.advanceTimersByTimeAsync(0);
      expect(reachText(rendered.root, "alpha")).toBe("Resolving reach...");

      pending.resolve([reachRow("alpha"), reachRow("beta"), reachRow("gamma")]);
      await vi.advanceTimersByTimeAsync(0);
      expect(reachText(rendered.root, "alpha")).toContain("Would reach 1 agent");
    } finally {
      rendered.cleanup();
    }
  });

  it("re-requests A and settles on A after A to B and back, when A had already answered", async () => {
    const fake = transport();
    const rendered = await mounted(fake);
    try {
      fake.clearCalls();
      selectorMode(rendered.root, "alpha", "selected");
      await vi.advanceTimersByTimeAsync(REACH_DEBOUNCE_MS);
      await vi.advanceTimersByTimeAsync(0);

      selectorMode(rendered.root, "alpha", "all");
      await vi.advanceTimersByTimeAsync(REACH_DEBOUNCE_MS);
      await vi.advanceTimersByTimeAsync(0);

      // No answer cache: a shape that returns is asked again rather than restored, because a
      // cache would add an invalidation question to save one debounced call.
      const calls = fake.callsFor("preview_watcher_reach");
      expect(calls).toHaveLength(2);
      expect(
        (calls[1].args as { watchers: { commands: unknown }[] }).watchers[0].commands
      ).toBeNull();
      expect(reachText(rendered.root, "alpha")).toContain("Would reach 1 agent");
    } finally {
      rendered.cleanup();
    }
  });

  it("awaits A's own answer after A to B and back, when A was still in flight", async () => {
    const first = deferred<WatcherReachRow[]>();
    const second = deferred<WatcherReachRow[]>();
    const answers = [first.promise, second.promise];
    const fake = transport(() => answers.shift() ?? []);

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="watchers" onClose={() => {}} />,
      fake
    );
    try {
      await vi.advanceTimersByTimeAsync(0);
      // Round A is issued and left pending.
      await vi.advanceTimersByTimeAsync(REACH_DEBOUNCE_MS);
      await vi.advanceTimersByTimeAsync(0);
      expect(fake.callsFor("preview_watcher_reach")).toHaveLength(1);

      // To B, which issues its own round.
      selectorMode(rendered.root, "alpha", "selected");
      await vi.advanceTimersByTimeAsync(REACH_DEBOUNCE_MS);
      await vi.advanceTimersByTimeAsync(0);
      expect(fake.callsFor("preview_watcher_reach")).toHaveLength(2);

      // And back to A while A is STILL in flight: its own answer is the one to wait for.
      selectorMode(rendered.root, "alpha", "all");
      await vi.advanceTimersByTimeAsync(REACH_DEBOUNCE_MS * 2);
      await vi.advanceTimersByTimeAsync(0);
      expect(fake.callsFor("preview_watcher_reach")).toHaveLength(2);
      expect(reachText(rendered.root, "alpha")).toBe("Resolving reach...");

      first.resolve([reachRow("alpha"), reachRow("beta"), reachRow("gamma")]);
      await vi.advanceTimersByTimeAsync(0);
      expect(reachText(rendered.root, "alpha")).toContain("Would reach 1 agent");
    } finally {
      rendered.cleanup();
    }
  });

  it("renders an error rather than the previous answer when a call is rejected", async () => {
    let round = 0;
    const fake = transport(() => {
      round += 1;
      if (round === 1) return [reachRow("alpha"), reachRow("beta"), reachRow("gamma")];
      throw new Error("resolution failed");
    });
    const rendered = await mounted(fake);
    try {
      expect(reachText(rendered.root, "alpha")).toContain("Would reach 1 agent");

      selectorMode(rendered.root, "alpha", "selected");
      await vi.advanceTimersByTimeAsync(REACH_DEBOUNCE_MS);
      await vi.advanceTimersByTimeAsync(0);

      expect(reachText(rendered.root, "alpha")).toContain("Reach unknown");
      expect(reachText(rendered.root, "alpha")).not.toContain("Would reach 1 agent");
    } finally {
      rendered.cleanup();
    }
  });

  it("never lets a stale response overwrite a newer one", async () => {
    const stale = deferred<WatcherReachRow[]>();
    const fresh = deferred<WatcherReachRow[]>();
    const answers = [stale.promise, fresh.promise];
    const fake = transport(() => answers.shift() ?? []);

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="watchers" onClose={() => {}} />,
      fake
    );
    try {
      await vi.advanceTimersByTimeAsync(0);
      await vi.advanceTimersByTimeAsync(REACH_DEBOUNCE_MS);
      await vi.advanceTimersByTimeAsync(0);

      selectorMode(rendered.root, "alpha", "selected");
      await vi.advanceTimersByTimeAsync(REACH_DEBOUNCE_MS);
      await vi.advanceTimersByTimeAsync(0);

      // The newer round answers "reaches nobody"; the older one, arriving after it, claims an
      // agent and must be discarded.
      fresh.resolve([
        { id: "alpha", entries: [] },
        reachRow("beta"),
        reachRow("gamma"),
      ]);
      await vi.advanceTimersByTimeAsync(0);
      expect(reachText(rendered.root, "alpha")).toContain("Would reach 0 agents");

      stale.resolve([reachRow("alpha"), reachRow("beta"), reachRow("gamma")]);
      await vi.advanceTimersByTimeAsync(0);
      expect(reachText(rendered.root, "alpha")).toContain("Would reach 0 agents");
    } finally {
      rendered.cleanup();
    }
  });

  /**
   * #1171 test 58g, the editor half: a row the Rust decoder would reject is classified
   * unrecognised, is not sent, consumes no budget slot, and survives a save verbatim.
   */
  it("leaves rows the decoder would reject out of the request and intact in the save", async () => {
    // Written as the JSON the file holds, with no cast: `WatcherEntry` now admits any
    // `serde_json::Value`, which is exactly what Rust preserves and hands back.
    const raw = (over: Record<string, JsonValue>): WatcherEntry => ({
      enabled: false,
      mode: "occurrence",
      pattern: "",
      dedupe: "row",
      dedupeWindowMs: 2000,
      ...over,
    });
    const rejected: Record<string, WatcherEntry> = {
      badcommands: raw({ commands: [1] }),
      negative: raw({ dedupeWindowMs: -1 }),
      fractional: raw({ dedupeWindowMs: 1.5 }),
      huge: raw({ dedupeWindowMs: 1e30 }),
      unsafe: raw({ dedupeWindowMs: Number.MAX_SAFE_INTEGER + 2 }),
      // The non-object shapes, which the old mirror could not even express.
      scalar: "claude",
      nothing: null,
      listed: ["claude"],
    };
    const accepted = {
      boundary: { ...newWatcherConfig(), dedupeWindowMs: Number.MAX_SAFE_INTEGER },
      good: { ...newWatcherConfig(), pattern: "Read" },
    };

    const fake = transport();
    fake.resolve(
      "get_settings",
      baseSettings({ agents: [AGENT], watchers: { ...rejected, ...accepted } })
    );

    const rendered = await mounted(fake);
    try {
      const sent = fake.lastCall("preview_watcher_reach")!.args as {
        watchers: { id: string }[];
      };
      expect(sent.watchers.map((row) => row.id).sort()).toEqual(["boundary", "good"]);

      // Named, not hidden, and not offered for editing.
      const notice = rendered.root.querySelector(
        '[data-ac-testid="settings.watchers.unreadable"]'
      );
      for (const id of Object.keys(rejected)) {
        expect(notice?.textContent).toContain(id);
        expect(
          rendered.root.querySelector(`[data-ac-testid="settings.watchers.row.${id}"]`)
        ).toBeNull();
      }

      click(rendered.root.querySelector('[data-ac-testid="settings.save"]')!);
      await vi.advanceTimersByTimeAsync(0);
      const draft = (fake.lastCall("save_settings_draft")!.args as { draft: AppSettings })
        .draft;
      for (const [id, entry] of Object.entries(rejected)) {
        expect(draft.watchers?.[id]).toEqual(entry);
      }
    } finally {
      rendered.cleanup();
    }
  });
});

/**
 * #1171 tests 58i and 58k, through the rendered editor.
 *
 * An empty pattern is a valid regex that matches every row, so Add plus an accidental Save
 * would otherwise activate a watcher that matches everything on every agent, fill the caps,
 * turn the ring over and displace a useful watcher out of an agent's budget.
 */
describe("the birth state of a watcher row and how it reads (#1171)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  function transport(rows: WatcherReachRow[], watchers: Record<string, WatcherEntry>) {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings({ agents: [AGENT], watchers }));
    fake.resolve("get_web_server_status", false);
    fake.resolve("save_settings_draft", undefined);
    fake.resolve("preview_watcher_reach", rows);
    fake.resolve("preview_watcher_pattern", {
      compiles: true,
      error: null,
      sampled: false,
      matchedRows: 0,
      totalRows: 0,
      samples: [],
      capturesVolatile: false,
    });
    return fake;
  }

  it("adds a disabled row and refuses to enable it while it has no pattern", async () => {
    const fake = transport([reachRow("watcher-1")], {});
    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="watchers" onClose={() => {}} />,
      fake
    );
    try {
      await waitFor(() =>
        expect(
          rendered.root.querySelector('[data-ac-testid="settings.watchers.add"]')
        ).toBeTruthy()
      );
      click(rendered.root.querySelector('[data-ac-testid="settings.watchers.add"]')!);

      const checkbox = await waitForElement<HTMLInputElement>(
        rendered.root,
        '[data-ac-testid="settings.watchers.enabled.watcher-1"]'
      );
      expect(checkbox.checked).toBe(false);
      expect(checkbox.disabled).toBe(true);

      // Even forced, the change is refused rather than merely discouraged.
      checkbox.checked = true;
      checkbox.dispatchEvent(new Event("change", { bubbles: true }));
      expect(checkbox.checked).toBe(false);

      click(rendered.root.querySelector('[data-ac-testid="settings.save"]')!);
      await waitFor(() => expect(fake.lastCall("save_settings_draft")).toBeTruthy());
      const draft = (fake.lastCall("save_settings_draft")!.args as { draft: AppSettings })
        .draft;
      expect((draft.watchers?.["watcher-1"] as WatcherConfig).enabled).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });

  /**
   * The invariant is `enabled => pattern !== ""`, and it has to hold on EVERY edit.
   *
   * Gating only the checkbox leaves this sequence open: type a pattern, enable, then delete
   * the pattern. Save does not validate watchers, so it persists `enabled: true` with
   * `pattern: ""` -- the global regex that matches every row on every agent, which is the
   * flood this whole rule exists to prevent.
   */
  it("cannot be walked into an enabled watcher with an empty pattern", async () => {
    const fake = transport([reachRow("watcher-1")], {});
    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="watchers" onClose={() => {}} />,
      fake
    );
    try {
      await waitFor(() =>
        expect(
          rendered.root.querySelector('[data-ac-testid="settings.watchers.add"]')
        ).toBeTruthy()
      );
      click(rendered.root.querySelector('[data-ac-testid="settings.watchers.add"]')!);

      const pattern = await waitForElement<HTMLInputElement>(
        rendered.root,
        '[data-ac-testid="settings.watchers.pattern.watcher-1"]'
      );
      input(pattern, "Read");

      const checkbox = rendered.root.querySelector<HTMLInputElement>(
        '[data-ac-testid="settings.watchers.enabled.watcher-1"]'
      )!;
      await waitFor(() => expect(checkbox.disabled).toBe(false));
      click(checkbox);
      await waitFor(() =>
        expect(
          rendered.root
            .querySelector('[data-ac-testid="settings.watchers.row.watcher-1"]')
            ?.getAttribute("data-ac-state")
        ).toBe("enabled")
      );

      // And now the step the checkbox guard never saw.
      input(pattern, "");

      await waitFor(() =>
        expect(
          rendered.root
            .querySelector('[data-ac-testid="settings.watchers.row.watcher-1"]')
            ?.getAttribute("data-ac-state")
        ).toBe("disabled")
      );

      click(rendered.root.querySelector('[data-ac-testid="settings.save"]')!);
      await waitFor(() => expect(fake.lastCall("save_settings_draft")).toBeTruthy());
      const saved = (fake.lastCall("save_settings_draft")!.args as { draft: AppSettings })
        .draft.watchers?.["watcher-1"] as WatcherConfig;
      expect(saved.pattern).toBe("");
      expect(saved.enabled).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });

  it("disables a watcher loaded enabled the moment its pattern is emptied", async () => {
    const fake = transport([reachRow("probe")], {
      probe: { ...newWatcherConfig(), pattern: "Read", enabled: true },
    });
    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="watchers" onClose={() => {}} />,
      fake
    );
    try {
      const pattern = await waitForElement<HTMLInputElement>(
        rendered.root,
        '[data-ac-testid="settings.watchers.pattern.probe"]'
      );
      await waitFor(() =>
        expect(
          rendered.root
            .querySelector('[data-ac-testid="settings.watchers.row.probe"]')
            ?.getAttribute("data-ac-state")
        ).toBe("enabled")
      );

      input(pattern, "");

      await waitFor(() =>
        expect(
          rendered.root
            .querySelector('[data-ac-testid="settings.watchers.row.probe"]')
            ?.getAttribute("data-ac-state")
        ).toBe("disabled")
      );
      // Auto-disabling flips `enabled`, which is part of the reach request, so the answer is
      // cleared and re-asked. Once it lands, the line names the missing condition rather than
      // blaming the budget.
      await waitFor(() =>
        expect(
          rendered.root.querySelector('[data-ac-testid="settings.watchers.reach.probe"]')
            ?.textContent
        ).toContain("Add a pattern to enable it.")
      );
    } finally {
      rendered.cleanup();
    }
  });

  /**
   * Rename is the other writer, and it was a way out of the state that editing is not.
   *
   * A hand-written `{ enabled: true, pattern: "" }` stays as written until the editor touches
   * it, which is deliberate. But the id is what the 8-per-agent budget resolves in: a `zzz`
   * sitting outside the first eight, renamed to `aaa` from this control, walks into budget,
   * displaces a useful watcher and runs the global regex -- without anybody going near the
   * pattern. Renaming is delete plus create, so the row it creates owes what every created
   * row owes.
   */
  it("does not let Rename carry an enabled empty pattern into a new id", async () => {
    const fake = transport([reachRow("zzz")], {
      zzz: { ...newWatcherConfig(), pattern: "", enabled: true },
    });
    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="watchers" onClose={() => {}} />,
      fake
    );
    try {
      await waitFor(() =>
        expect(
          rendered.root
            .querySelector('[data-ac-testid="settings.watchers.row.zzz"]')
            ?.getAttribute("data-ac-state")
        ).toBe("enabled")
      );

      click(rendered.root.querySelector('[data-ac-testid="settings.watchers.renameStart.zzz"]')!);
      const field = await waitForElement<HTMLInputElement>(
        rendered.root,
        '[data-ac-testid="settings.watchers.renameInput.zzz"]'
      );
      input(field, "aaa");
      click(
        rendered.root.querySelector('[data-ac-testid="settings.watchers.renameConfirm.zzz"]')!
      );

      await waitFor(() =>
        expect(
          rendered.root.querySelector('[data-ac-testid="settings.watchers.row.aaa"]')
        ).toBeTruthy()
      );
      expect(
        rendered.root
          .querySelector('[data-ac-testid="settings.watchers.row.aaa"]')
          ?.getAttribute("data-ac-state")
      ).toBe("disabled");

      click(rendered.root.querySelector('[data-ac-testid="settings.save"]')!);
      await waitFor(() => expect(fake.lastCall("save_settings_draft")).toBeTruthy());
      const watchers = (fake.lastCall("save_settings_draft")!.args as { draft: AppSettings })
        .draft.watchers;
      expect(watchers?.zzz).toBeUndefined();
      expect((watchers?.aaa as WatcherConfig).enabled).toBe(false);
      expect((watchers?.aaa as WatcherConfig).pattern).toBe("");
    } finally {
      rendered.cleanup();
    }
  });

  it("lets the row be enabled once it has a pattern", async () => {
    const fake = transport([reachRow("probe")], {
      probe: { ...newWatcherConfig(), pattern: "Read" },
    });
    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="watchers" onClose={() => {}} />,
      fake
    );
    try {
      const checkbox = await waitForElement<HTMLInputElement>(
        rendered.root,
        '[data-ac-testid="settings.watchers.enabled.probe"]'
      );
      expect(checkbox.disabled).toBe(false);
      click(checkbox);
      await waitFor(() =>
        expect(
          rendered.root
            .querySelector('[data-ac-testid="settings.watchers.row.probe"]')
            ?.getAttribute("data-ac-state")
        ).toBe("enabled")
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("states reach in the present tense with a budget badge only when the row is enabled", async () => {
    const fake = transport([reachRow("probe", false)], {
      probe: { ...newWatcherConfig(), pattern: "Read", enabled: true },
    });
    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="watchers" onClose={() => {}} />,
      fake
    );
    try {
      await waitFor(() =>
        expect(
          rendered.root.querySelector('[data-ac-testid="settings.watchers.reach.probe"]')
            ?.textContent
        ).toContain("Reaches 1 agent.")
      );
      expect(
        rendered.root.querySelector('[data-ac-testid="settings.watchers.reach.probe"]')
          ?.textContent
      ).toContain("Not running on Claude (budget).");
    } finally {
      rendered.cleanup();
    }
  });

  it("states a disabled row conditionally and never blames the budget for it", async () => {
    const fake = transport([reachRow("probe", false)], {
      probe: { ...newWatcherConfig(), pattern: "Read" },
    });
    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="watchers" onClose={() => {}} />,
      fake
    );
    try {
      await waitFor(() =>
        expect(
          rendered.root.querySelector('[data-ac-testid="settings.watchers.reach.probe"]')
            ?.textContent
        ).toContain("Would reach 1 agent when enabled.")
      );
      const text =
        rendered.root.querySelector('[data-ac-testid="settings.watchers.reach.probe"]')
          ?.textContent ?? "";
      expect(text).not.toContain("budget");
      expect(text).not.toContain("Add a pattern");
    } finally {
      rendered.cleanup();
    }
  });

  /**
   * The other half of #1171 test 58g: the predicate rejects what serde rejects AND the editor
   * cannot produce it. Writing a negative window would reclassify this very row as
   * unrecognised, taking it out of the editor in the middle of being edited.
   */
  it("refuses to write a dedupe window the decoder would reject", async () => {
    const fake = transport([reachRow("probe")], {
      probe: { ...newWatcherConfig(), pattern: "Read", mode: "occurrence" },
    });
    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="watchers" onClose={() => {}} />,
      fake
    );
    try {
      const field = await waitForElement<HTMLInputElement>(
        rendered.root,
        '[data-ac-testid="settings.watchers.dedupeWindow.probe"]'
      );
      for (const rejected of ["-1", String(Number.MAX_SAFE_INTEGER + 2)]) {
        input(field, rejected);
        expect(
          rendered.root.querySelector('[data-ac-testid="settings.watchers.row.probe"]')
        ).toBeTruthy();
        expect(
          rendered.root.querySelector('[data-ac-testid="settings.watchers.unreadable"]')
        ).toBeNull();
      }

      input(field, "5000");
      click(rendered.root.querySelector('[data-ac-testid="settings.save"]')!);
      await waitFor(() => expect(fake.lastCall("save_settings_draft")).toBeTruthy());
      const draft = (fake.lastCall("save_settings_draft")!.args as { draft: AppSettings })
        .draft;
      expect((draft.watchers?.probe as WatcherConfig).dedupeWindowMs).toBe(5000);
    } finally {
      rendered.cleanup();
    }
  });

  it("names the missing pattern first on the state everyone sees after Add Watcher", async () => {
    const fake = transport([reachRow("probe", false)], { probe: newWatcherConfig() });
    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="watchers" onClose={() => {}} />,
      fake
    );
    try {
      await waitFor(() =>
        expect(
          rendered.root.querySelector('[data-ac-testid="settings.watchers.reach.probe"]')
            ?.textContent
        ).toBe("Would reach 1 agent when enabled. Add a pattern to enable it.")
      );
    } finally {
      rendered.cleanup();
    }
  });
});

async function waitForElement<T extends Element>(
  root: HTMLElement,
  selector: string
): Promise<T> {
  await waitFor(() => expect(root.querySelector(selector)).toBeTruthy());
  return root.querySelector<T>(selector)!;
}
