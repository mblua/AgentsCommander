// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import WatchersApp, {
  logicalGeometry,
  MOUNT_TIMEOUT_MS,
  POLL_FOCUSED_MS,
  POLL_TIMEOUT_MESSAGE,
  POLL_TIMEOUT_MS,
  POLL_UNFOCUSED_MS,
  registerAll,
  STARTUP_DEGRADED_MESSAGE,
  withDeadline,
} from "./App";
import { ALL_SESSIONS_LIMIT } from "./activity";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../shared/testing/ui-harness";
import type {
  Session,
  WatcherActivitySnapshot,
  WatcherMatchPayload,
} from "../shared/types";

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

const AGENT_SESSIONS = [
  session({
    id: "s1",
    name: "claude@repo",
    agentId: "a1",
    agentLabel: "Claude Sandbox",
    workingDirectory: "C:/p/.ac/wg-19-team/__agent_dev-rust",
  }),
  session({
    id: "s2",
    name: "codex@repo",
    agentId: "a2",
    agentLabel: "Codex",
    workingDirectory: "C:/p/.ac/wg-20-team/__agent_dev-ui",
  }),
];

function transportWith(snap: WatcherActivitySnapshot): FakeTransport {
  const fake = new FakeTransport();
  fake.resolve("list_sessions", AGENT_SESSIONS);
  fake.resolve("get_settings", { agents: [] });
  fake.resolve("get_watcher_activity", snap);
  // The authoritative scope, pulled after the subscribe. `null` is "nothing was requested",
  // which leaves the query parameter standing.
  fake.resolve("get_watchers_scope", null);
  return fake;
}

/** A promise this test resolves by hand, so an ordering can be pinned instead of raced. */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void; reject: (reason: unknown) => void } {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Let every already-resolved microtask run, without waiting on a condition. */
async function flush(): Promise<void> {
  for (let i = 0; i < 8; i += 1) await Promise.resolve();
}

/**
 * The persistent startup notice, mirroring `errorBanner` below.
 *
 * Module-scoped rather than local to one `describe`, because the two suites that assert it --
 * the rewritten subscribe-failure test in the #1171 block and the whole #1196 block -- are
 * different blocks and this is one helper, not two.
 */
const startupBanner = (root: HTMLElement): HTMLElement | null =>
  root.querySelector<HTMLElement>('[data-ac-testid="watchers.startupDegraded"]');

describe("the watcher activity window (#1171)", () => {
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

  // #1171 test 81, the four states told apart from snapshot values alone.
  it("shows the warming state before the engine has ticked, never the day-one message", async () => {
    const fake = transportWith(snapshot({ warmedUp: false }));
    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      await waitFor(() =>
        expect(
          rendered.root.querySelector('[data-ac-testid="watchers.empty.warming"]')
        ).toBeTruthy()
      );
      expect(
        rendered.root.querySelector('[data-ac-testid="watchers.empty.unconfigured"]')
      ).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("offers Configure watchers once warmed up with nothing reaching the agent", async () => {
    const fake = transportWith(snapshot({ warmedUp: true, activeWatchers: [] }));
    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      await waitFor(() =>
        expect(rendered.root.querySelector('[data-ac-testid="watchers.configure"]')).toBeTruthy()
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("lists what it is waiting for, with mode and a zero counter", async () => {
    const fake = transportWith(
      snapshot({
        activeWatchers: [
          { watcherId: "permission", mode: "state", count: 0, degraded: false },
        ],
      })
    );
    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      await waitFor(() =>
        expect(
          rendered.root.querySelector('[data-ac-testid="watchers.waiting.permission"]')
        ).toBeTruthy()
      );
      const entry = rendered.root.querySelector('[data-ac-testid="watchers.waiting.permission"]');
      expect(entry?.textContent).toContain("state");
    } finally {
      rendered.cleanup();
    }
  });

  // #1171 tests 82 and 83.
  it("keeps the truncated banner and the unsampled note as separate elements", async () => {
    const fake = transportWith(
      snapshot({ matches: [match()], truncated: true, possiblyMissedFrames: 3 })
    );
    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      await waitFor(() =>
        expect(rendered.root.querySelector('[data-ac-testid="watchers.truncated"]')).toBeTruthy()
      );
      const truncated = rendered.root.querySelector('[data-ac-testid="watchers.truncated"]');
      const missed = rendered.root.querySelector('[data-ac-testid="watchers.missedFrames"]');
      expect(missed).toBeTruthy();
      expect(truncated).not.toBe(missed);
      expect(truncated?.textContent).not.toBe(missed?.textContent);
    } finally {
      rendered.cleanup();
    }
  });

  it("shows the best-effort footer in every state", async () => {
    for (const snap of [
      snapshot({ warmedUp: false }),
      snapshot({ warmedUp: true }),
      snapshot({ matches: [match()] }),
    ]) {
      const fake = transportWith(snap);
      const rendered = renderWithFakeTransport(
        () => <WatchersApp initialSessionId="s1" />,
        fake
      );
      try {
        await waitFor(() =>
          expect(rendered.root.querySelector('[data-ac-testid="watchers.footer"]')).toBeTruthy()
        );
        expect(
          rendered.root.querySelector('[data-ac-testid="watchers.footer"]')?.textContent
        ).toContain("not an audit log");
      } finally {
        rendered.cleanup();
      }
    }
  });

  // #1171 test 84.
  it("renders a row containing HTML-looking text as text", async () => {
    const hostile = "<img src=x onerror=alert(1)> Read (<script>)";
    const fake = transportWith(
      snapshot({ matches: [match({ captures: [hostile], row: hostile })] })
    );
    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      await waitFor(() =>
        expect(rendered.root.querySelector('[data-ac-testid="watchers.table"]')).toBeTruthy()
      );
      expect(rendered.root.querySelector("img")).toBeNull();
      expect(rendered.root.querySelector("script")).toBeNull();
      expect(rendered.root.textContent).toContain(hostile);
    } finally {
      rendered.cleanup();
    }
  });

  // #1171 test 85.
  it("distinguishes a state row from an occurrence row", async () => {
    const fake = transportWith(
      snapshot({
        matches: [
          match({ seq: 1, mode: "occurrence", watcherId: "reads" }),
          match({ seq: 2, mode: "state", watcherId: "permission" }),
        ],
      })
    );
    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      await waitFor(() =>
        expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s1:1"]')).toBeTruthy()
      );
      const occurrence = rendered.root.querySelector('[data-ac-testid="watchers.row.s1:1"]');
      const state = rendered.root.querySelector('[data-ac-testid="watchers.row.s1:2"]');
      expect(occurrence?.getAttribute("data-ac-mode")).toBe("occurrence");
      expect(state?.getAttribute("data-ac-mode")).toBe("state");
      expect(occurrence?.textContent).not.toBe(state?.textContent);
    } finally {
      rendered.cleanup();
    }
  });

  // #1171 test 80.
  it("hides the Agent and Workgroup chips in single-session scope and shows them in All sessions", async () => {
    const fake = transportWith(snapshot({ matches: [match()] }));
    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      await waitFor(() =>
        expect(rendered.root.querySelector('[data-ac-testid="watchers.table"]')).toBeTruthy()
      );
      expect(rendered.root.querySelector('[data-ac-testid="watchers.filter.agent"]')).toBeNull();
      expect(
        rendered.root.querySelector('[data-ac-testid="watchers.filter.workgroup"]')
      ).toBeNull();

      const scope = rendered.root.querySelector<HTMLSelectElement>(
        '[data-ac-testid="watchers.scope"]'
      )!;
      scope.value = "all";
      scope.dispatchEvent(new Event("change", { bubbles: true }));

      await waitFor(() =>
        expect(
          rendered.root.querySelector('[data-ac-testid="watchers.filter.agent"]')
        ).toBeTruthy()
      );
      expect(
        rendered.root.querySelector('[data-ac-testid="watchers.filter.workgroup"]')
      ).toBeTruthy();
    } finally {
      rendered.cleanup();
    }
  });

  /**
   * #1193. The two labels name each other's concept, so both renames have to hold at once:
   * asserting all three strings in one test makes a half-applied swap -- which would leave
   * two controls labelled AGENT -- fail. The assertions read the source casing because
   * `text-transform: uppercase` is a paint-time rule jsdom does not apply to `textContent`.
   */
  it("labels the dropdown Agent and the chip group Coding-Agent", async () => {
    const fake = transportWith(snapshot({ matches: [match()] }));
    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      await waitFor(() =>
        expect(rendered.root.querySelector('[data-ac-testid="watchers.table"]')).toBeTruthy()
      );

      const scope = rendered.root.querySelector<HTMLSelectElement>(
        '[data-ac-testid="watchers.scope"]'
      )!;
      scope.value = "all";
      scope.dispatchEvent(new Event("change", { bubbles: true }));

      // The chip group only renders in All-agents scope.
      await waitFor(() =>
        expect(
          rendered.root.querySelector('[data-ac-testid="watchers.filter.agent"]')
        ).toBeTruthy()
      );

      // The dropdown is the control that selects among the user's agents.
      expect(
        scope.parentElement?.querySelector(".watchers-filter-label")?.textContent
      ).toBe("Agent");
      expect(
        rendered.root.querySelector('[data-ac-testid="watchers.scope"] option[value="all"]')
          ?.textContent
      ).toBe("All agents");
      // The chip group is the one that filters by the CLI behind the session.
      expect(
        rendered.root.querySelector(
          '[data-ac-testid="watchers.filter.agent"] .watchers-filter-label'
        )?.textContent
      ).toBe("Coding-Agent");
    } finally {
      rendered.cleanup();
    }
  });

  /**
   * #1171 test 79, the frontend half. The backend focuses the existing window and emits
   * `watchers_scope_request` instead of building a second one; what is left to prove here
   * is that the window actually changes scope when that event arrives.
   */
  it("re-scopes to the requested session when the window is already open", async () => {
    const fake = transportWith(snapshot({ matches: [match()] }));
    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      await waitFor(() =>
        expect(
          rendered.root.querySelector<HTMLSelectElement>('[data-ac-testid="watchers.scope"]')
            ?.value
        ).toBe("s1")
      );

      fake.emitFromBackend("watchers_scope_request", { sessionId: "s2" });

      await waitFor(() =>
        expect(
          rendered.root.querySelector<HTMLSelectElement>('[data-ac-testid="watchers.scope"]')
            ?.value
        ).toBe("s2")
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("shows a match that arrives on the event stream, exactly once", async () => {
    const fake = transportWith(snapshot({ matches: [match({ seq: 1 })] }));
    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      await waitFor(() =>
        expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s1:1"]')).toBeTruthy()
      );

      // The same match the snapshot already carried, plus a genuinely new one.
      fake.emitFromBackend("watcher_matches", {
        sessionId: "s1",
        matches: [match({ seq: 1 }), match({ seq: 2 })],
      });

      await waitFor(() =>
        expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s1:2"]')).toBeTruthy()
      );
      expect(
        rendered.root.querySelectorAll('[data-ac-testid="watchers.row.s1:1"]')
      ).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });

  it("ignores a batch for a session outside the current scope", async () => {
    const fake = transportWith(snapshot({ matches: [match()] }));
    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      await waitFor(() =>
        expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s1:1"]')).toBeTruthy()
      );

      fake.emitFromBackend("watcher_matches", {
        sessionId: "s2",
        matches: [match({ sessionId: "s2", seq: 9 })],
      });

      expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s2:9"]')).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  /**
   * #1171 test 77, the race the earlier version of this test did not run.
   *
   * Waiting for the snapshot row to be in the DOM and only then emitting proves nothing: it
   * passes even if the window fetched before it subscribed, or dropped whatever arrived
   * first. The overlap has to land while the invoke is still pending.
   */
  it("merges an overlap that arrives while the snapshot fetch is still in flight", async () => {
    const activity = deferred<WatcherActivitySnapshot>();
    const fake = transportWith(snapshot());
    fake.onInvoke("get_watcher_activity", () => activity.promise);

    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      // The fetch is out, so the subscribe that precedes it has completed.
      await waitFor(() => expect(fake.callsFor("get_watcher_activity")).toHaveLength(1));
      expect(rendered.root.querySelector('[data-ac-testid="watchers.table"]')).toBeNull();

      // seq 7 overlaps the snapshot; seq 8 is only on the stream.
      fake.emitFromBackend("watcher_matches", {
        sessionId: "s1",
        matches: [match({ seq: 7 }), match({ seq: 8 })],
      });

      activity.resolve(snapshot({ matches: [match({ seq: 6 }), match({ seq: 7 })], lastSeq: 7 }));

      await waitFor(() =>
        expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s1:6"]')).toBeTruthy()
      );
      expect(
        rendered.root.querySelectorAll('[data-ac-testid="watchers.row.s1:7"]')
      ).toHaveLength(1);
      expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s1:8"]')).toBeTruthy();
    } finally {
      rendered.cleanup();
    }
  });

  /**
   * #1171 test 79d, both orderings of the same race. They fail differently, which is why
   * neither one alone is enough.
   */
  describe("settling the scope on mount (#1171 test 79d)", () => {
    it("adopts the event and never fetches the scope it was about to leave", async () => {
      const pull = deferred<string | null>();
      const fake = transportWith(snapshot());
      fake.onInvoke("get_watchers_scope", () => pull.promise);

      const rendered = renderWithFakeTransport(
        () => <WatchersApp initialSessionId="s1" />,
        fake
      );
      try {
        await waitFor(() => expect(fake.callsFor("get_watchers_scope")).toHaveLength(1));
        // Nothing at all is fetched before the scope is settled.
        expect(fake.callsFor("get_watcher_activity")).toHaveLength(0);

        fake.emitFromBackend("watchers_scope_request", { sessionId: "s2" });
        // The pull answers a THIRD session, and it lost: an event was handled since it was
        // issued.
        pull.resolve("s1");

        await waitFor(() => expect(fake.callsFor("get_watcher_activity")).toHaveLength(1));
        expect(fake.callsFor("get_watcher_activity")[0].args.sessionId).toBe("s2");
      } finally {
        rendered.cleanup();
      }
    });

    it("re-scopes on an event that arrives during the first fetch, without waiting for the poll", async () => {
      const first = deferred<WatcherActivitySnapshot>();
      const fake = transportWith(snapshot());
      fake.onInvoke("get_watcher_activity", (args) =>
        args.sessionId === "s1" ? first.promise : snapshot({ matches: [match({ sessionId: "s2", seq: 4 })] })
      );

      const rendered = renderWithFakeTransport(
        () => <WatchersApp initialSessionId="s1" />,
        fake
      );
      try {
        await waitFor(() => expect(fake.callsFor("get_watcher_activity")).toHaveLength(1));

        fake.emitFromBackend("watchers_scope_request", { sessionId: "s2" });

        await waitFor(() =>
          expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s2:4"]')).toBeTruthy()
        );

        // The first scope's answer lands last and must change nothing.
        first.resolve(snapshot({ matches: [match({ sessionId: "s1", seq: 1 })] }));
        await flush();
        expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s1:1"]')).toBeNull();
      } finally {
        rendered.cleanup();
      }
    });
  });

  /**
   * #1171 test 79e, the content guard. Three assertions that fail independently, because a
   * correct selector over the previous session's rows is the failure this exists to prevent.
   */
  describe("guarding what is painted (#1171 test 79e)", () => {
    it("drops the previous scope's rows synchronously, before the new fetch resolves and when it rejects", async () => {
      const second = deferred<WatcherActivitySnapshot>();
      const fake = transportWith(snapshot());
      fake.onInvoke("get_watcher_activity", (args) =>
        args.sessionId === "s1"
          ? snapshot({ matches: [match({ sessionId: "s1", seq: 1 })] })
          : second.promise
      );

      const rendered = renderWithFakeTransport(
        () => <WatchersApp initialSessionId="s1" />,
        fake
      );
      try {
        await waitFor(() =>
          expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s1:1"]')).toBeTruthy()
        );

        const scope = rendered.root.querySelector<HTMLSelectElement>(
          '[data-ac-testid="watchers.scope"]'
        )!;
        scope.value = "s2";
        scope.dispatchEvent(new Event("change", { bubbles: true }));

        // Already gone, with the new answer still in flight.
        await waitFor(() =>
          expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s1:1"]')).toBeNull()
        );

        // And still gone once the new fetch fails outright.
        second.reject(new Error("backend said no"));
        await waitFor(() =>
          expect(rendered.root.querySelector('[data-ac-testid="watchers.error"]')).toBeTruthy()
        );
        expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s1:1"]')).toBeNull();
      } finally {
        rendered.cleanup();
      }
    });

    it("keeps the newer counters when two fetches of the SAME scope resolve out of order", async () => {
      // Both deferred answers are for s1, so only the request counter can tell them apart: a
      // generation keyed on the SCOPE sees one value and lets the older one commit its
      // `lastSeq`, `warmedUp`, `degraded` and counters over the newer ones, leaving the table
      // and the counters describing two different instants.
      const older = deferred<WatcherActivitySnapshot>();
      const newer = deferred<WatcherActivitySnapshot>();
      const forS1 = [older.promise, newer.promise];
      const fake = transportWith(snapshot());
      fake.onInvoke("get_watcher_activity", (args) =>
        args.sessionId === "s1" ? forS1.shift() ?? snapshot() : snapshot()
      );

      const rendered = renderWithFakeTransport(
        () => <WatchersApp initialSessionId="s1" />,
        fake
      );
      try {
        // Round one for s1, left in flight.
        await waitFor(() => expect(fake.callsFor("get_watcher_activity")).toHaveLength(1));

        fake.emitFromBackend("watchers_scope_request", { sessionId: "s2" });
        await waitFor(() => expect(fake.callsFor("get_watcher_activity")).toHaveLength(2));

        // Back to s1: round two for the same scope, also left in flight.
        fake.emitFromBackend("watchers_scope_request", { sessionId: "s1" });
        await waitFor(() => expect(fake.callsFor("get_watcher_activity")).toHaveLength(3));

        newer.resolve(
          snapshot({
            activeWatchers: [
              { watcherId: "reads", mode: "occurrence", count: 9, degraded: true },
            ],
          })
        );
        await waitFor(() =>
          expect(rendered.root.querySelector('[data-ac-testid="watchers.degraded"]')).toBeTruthy()
        );

        older.resolve(
          snapshot({
            activeWatchers: [
              { watcherId: "reads", mode: "occurrence", count: 0, degraded: false },
            ],
          })
        );
        await flush();
        expect(
          rendered.root.querySelector('[data-ac-testid="watchers.degraded"]')
        ).toBeTruthy();
      } finally {
        rendered.cleanup();
      }
    });
  });

  /**
   * #1171 test 79f. In "All sessions" the scope is a SET derived from the session list, and
   * the session listeners rewrite it without touching the selection. A generation keyed on
   * the selected session passes every other test here and fails this one.
   */
  it("refetches when a session enters the scope in All sessions, keeping its rows", async () => {
    const fake = new FakeTransport();
    let listed = [AGENT_SESSIONS[0]];
    fake.onInvoke("list_sessions", () => listed);
    fake.resolve("get_settings", { agents: [] });
    fake.resolve("get_watchers_scope", null);
    fake.onInvoke("get_watcher_activity", (args) =>
      snapshot({ matches: [match({ sessionId: String(args.sessionId), seq: 1 })] })
    );

    const rendered = renderWithFakeTransport(() => <WatchersApp />, fake);
    try {
      await waitFor(() =>
        expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s1:1"]')).toBeTruthy()
      );

      listed = AGENT_SESSIONS;
      fake.emitFromBackend("session_created", { sessionId: "s2" });

      await waitFor(() =>
        expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s2:1"]')).toBeTruthy()
      );
      expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s1:1"]')).toBeTruthy();
    } finally {
      rendered.cleanup();
    }
  });

  /**
   * A degraded watcher has by definition already emitted matches, so the marker that only
   * lived inside the "configured and waiting" branch could never be reached.
   */
  it("shows the degraded marker while the table is showing rows", async () => {
    const fake = transportWith(
      snapshot({
        matches: [match()],
        activeWatchers: [{ watcherId: "reads", mode: "occurrence", count: 40, degraded: true }],
      })
    );
    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      await waitFor(() =>
        expect(rendered.root.querySelector('[data-ac-testid="watchers.table"]')).toBeTruthy()
      );
      const marker = rendered.root.querySelector('[data-ac-testid="watchers.degraded"]');
      expect(marker?.textContent).toContain("reads");
    } finally {
      rendered.cleanup();
    }
  });

  /** "Nothing has matched yet" over activations a filter is hiding is a false statement. */
  it("says the filters are hiding the activations rather than that none exist", async () => {
    const fake = transportWith(
      snapshot({
        matches: [match()],
        activeWatchers: [{ watcherId: "reads", mode: "occurrence", count: 1, degraded: false }],
      })
    );
    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      await waitFor(() =>
        expect(rendered.root.querySelector('[data-ac-testid="watchers.table"]')).toBeTruthy()
      );

      const search = rendered.root.querySelector<HTMLInputElement>(
        '[data-ac-testid="watchers.filter.text"]'
      )!;
      input(search, "nothing will ever match this");

      await waitFor(() =>
        expect(
          rendered.root.querySelector('[data-ac-testid="watchers.empty.filtered"]')
        ).toBeTruthy()
      );
      expect(
        rendered.root.querySelector('[data-ac-testid="watchers.empty.waiting"]')
      ).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  /**
   * The synchronous drop has to be as wide as the fetch key that triggered it.
   *
   * Leaving a single session for "All sessions" narrows the per-session limit from 500 to
   * 100, and `keepSessions` alone keeps every one of the 500 painted for the whole round
   * trip -- and forever if the new fetch fails, which is exactly the "correct selector over
   * stale rows" this guard exists to prevent.
   */
  it("drops what the new scope's limit no longer allows, before the new fetch resolves", async () => {
    const held = snapshot({
      matches: Array.from({ length: 150 }, (_, i) => match({ seq: i })),
    });
    const second = deferred<WatcherActivitySnapshot>();
    let round = 0;
    const fake = transportWith(held);
    fake.onInvoke("get_watcher_activity", () => {
      round += 1;
      return round === 1 ? held : second.promise;
    });

    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      await waitFor(() =>
        expect(rendered.root.querySelectorAll("tr.watchers-row")).toHaveLength(150)
      );

      const scope = rendered.root.querySelector<HTMLSelectElement>(
        '[data-ac-testid="watchers.scope"]'
      )!;
      scope.value = "all";
      scope.dispatchEvent(new Event("change", { bubbles: true }));

      await waitFor(() =>
        expect(rendered.root.querySelectorAll("tr.watchers-row")).toHaveLength(
          ALL_SESSIONS_LIMIT
        )
      );
      // And what survived is the newest, not whatever the filter happened to reach first.
      expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s1:149"]')).toBeTruthy();
      expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s1:0"]')).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  /**
   * The three session listeners each fire their own `list_sessions`, and nothing polls the
   * list afterwards, so an answer that resolves out of order is not a flicker: it is wrong
   * permanently.
   */
  describe("reloading the session list (#1171)", () => {
    const optionValues = (root: HTMLElement) =>
      [
        ...root.querySelectorAll<HTMLOptionElement>(
          '[data-ac-testid="watchers.scope"] option'
        ),
      ].map((option) => option.value);

    it("does not let a stale list resurrect a session that was destroyed", async () => {
      const stale = deferred<Session[]>();
      const lists: (Session[] | Promise<Session[]>)[] = [
        [AGENT_SESSIONS[0]], // mount
        stale.promise, // the create's reload, left in flight
        [AGENT_SESSIONS[0]], // the destroy's reload, which answers first
      ];
      const fake = transportWith(snapshot());
      fake.onInvoke("list_sessions", () => lists.shift() ?? [AGENT_SESSIONS[0]]);

      const rendered = renderWithFakeTransport(() => <WatchersApp />, fake);
      try {
        await waitFor(() => expect(fake.callsFor("list_sessions")).toHaveLength(1));

        fake.emitFromBackend("session_created", AGENT_SESSIONS[1]);
        await waitFor(() => expect(fake.callsFor("list_sessions")).toHaveLength(2));

        fake.emitFromBackend("session_destroyed", { id: "s2" });
        await waitFor(() => expect(fake.callsFor("list_sessions")).toHaveLength(3));
        await waitFor(() => expect(optionValues(rendered.root)).toEqual(["all", "s1"]));

        // The create's answer arrives last, describing a world that no longer exists.
        stale.resolve(AGENT_SESSIONS);
        await flush();
        expect(optionValues(rendered.root)).toEqual(["all", "s1"]);
      } finally {
        rendered.cleanup();
      }
    });

    it("does not let a stale list remove a session that was just created", async () => {
      const stale = deferred<Session[]>();
      const lists: (Session[] | Promise<Session[]>)[] = [
        [AGENT_SESSIONS[0]], // mount
        stale.promise, // the rename's reload, left in flight
        AGENT_SESSIONS, // the create's reload, which answers first
      ];
      const fake = transportWith(snapshot());
      fake.onInvoke("list_sessions", () => lists.shift() ?? AGENT_SESSIONS);

      const rendered = renderWithFakeTransport(() => <WatchersApp />, fake);
      try {
        await waitFor(() => expect(fake.callsFor("list_sessions")).toHaveLength(1));

        fake.emitFromBackend("session_renamed", { id: "s1", name: "renamed" });
        await waitFor(() => expect(fake.callsFor("list_sessions")).toHaveLength(2));

        fake.emitFromBackend("session_created", AGENT_SESSIONS[1]);
        await waitFor(() => expect(fake.callsFor("list_sessions")).toHaveLength(3));
        await waitFor(() => expect(optionValues(rendered.root)).toEqual(["all", "s1", "s2"]));

        stale.resolve([AGENT_SESSIONS[0]]);
        await flush();
        expect(optionValues(rendered.root)).toEqual(["all", "s1", "s2"]);
      } finally {
        rendered.cleanup();
      }
    });
  });

  /**
   * #1196 REVERSES what this test used to pin, deliberately.
   *
   * It used to assert that a failed subscribe stopped the mount where it stood
   * (`get_watcher_activity` length 0). That invariant is the defect: the arming of the poll
   * sat behind every startup await, so a window that ended its mount was a window that never
   * polled. A degraded window that polls is strictly more useful than a dead one with a red
   * banner, so a failed step is now logged, raises the persistent startup notice, and the
   * chain continues to its arming point.
   *
   * The "say so" half of the old contract is kept and strengthened: the notice is persistent
   * where `loadError` is cleared by the scope effect and by every successful round. Solid
   * still does not await the promise `onMount(async ...)` returns, so the mount must also
   * still leave no unhandled rejection behind.
   */
  it("keeps starting up and reports it when a subscribe fails", async () => {
    const fake = transportWith(snapshot());
    const realListen = fake.listen.bind(fake);
    fake.listen = ((event: string, callback: (payload: never) => void) =>
      event === "session_renamed"
        ? Promise.reject(new Error("transport closed mid-subscribe"))
        : realListen(event, callback)) as FakeTransport["listen"];

    // Restored in the `finally`: this `describe`'s `afterEach` has no `vi.restoreAllMocks()`
    // and `vitest.config.ts` sets neither `restoreMocks` nor `clearMocks`, so an unrestored
    // spy would leak into every later test in this file.
    const errorSpy = vi.spyOn(console, "error");
    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      // Waiting on `watchers.error` is what the old version did, and under the new contract
      // that banner never appears, so a verbatim copy would time out after 1s.
      await waitFor(() => expect(startupBanner(rendered.root)).toBeTruthy());
      expect(startupBanner(rendered.root)?.textContent).toBe(STARTUP_DEGRADED_MESSAGE);

      // The assertion that inverts: the mount ran on past the failure and armed, so the scope
      // effect issued its first fetch.
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(1);

      // The original test's real value, preserved: a failed setup stays diagnosable. The raw
      // error goes to the console, never to the banner.
      expect(
        errorSpy.mock.calls.some((args) =>
          args.some(
            (arg) => arg instanceof Error && arg.message === "transport closed mid-subscribe"
          )
        )
      ).toBe(true);
    } finally {
      rendered.cleanup();
      errorSpy.mockRestore();
    }
  });

  /**
   * The P0 this suite itself used to expose: `onMount` crosses several awaits, and a window
   * closed inside one of them left the continuation registering listeners, fetching and
   * starting a poll against a component that no longer existed.
   */
  it("stops the mount where it is when the window closes mid-flight", async () => {
    const sessions = deferred<Session[]>();
    const fake = transportWith(snapshot());
    fake.onInvoke("list_sessions", () => sessions.promise);

    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    await waitFor(() => expect(fake.callsFor("list_sessions")).toHaveLength(1));

    rendered.cleanup();
    sessions.resolve(AGENT_SESSIONS);
    await flush();

    expect(fake.callsFor("get_watchers_scope")).toHaveLength(0);
    expect(fake.callsFor("get_watcher_activity")).toHaveLength(0);

    // And the listeners are gone with it: this must reach nobody rather than throw.
    fake.emitFromBackend("watcher_matches", { sessionId: "s1", matches: [match()] });
    await flush();
    expect(fake.callsFor("get_watcher_activity")).toHaveLength(0);
  });
});

/**
 * #1171 - the setup sequence behind the geometry listeners.
 *
 * Registering them one at a time and keeping each unlisten in its own local means a later
 * registration that REJECTS strands the earlier one: it never reaches the component's
 * cleanup, and the window is gone with a live move handler still attached to it.
 */
describe("registering a set of listeners as one unit (#1171)", () => {
  it("hands over a single release when every registration succeeds", async () => {
    const released: string[] = [];
    const release = await registerAll(
      [
        () => Promise.resolve(() => released.push("moved")),
        () => Promise.resolve(() => released.push("resized")),
      ],
      () => false
    );
    expect(release).toBeTruthy();
    release!();
    expect(released).toEqual(["moved", "resized"]);
  });

  it("releases what it already holds when a later registration rejects", async () => {
    const released: string[] = [];
    await expect(
      registerAll(
        [
          () => Promise.resolve(() => released.push("moved")),
          () => Promise.reject(new Error("the window is gone")),
        ],
        () => false
      )
    ).rejects.toThrow("the window is gone");
    expect(released).toEqual(["moved"]);
  });

  it("releases what it already holds when the window closes mid-sequence", async () => {
    const released: string[] = [];
    let closed = false;
    const release = await registerAll(
      [
        () =>
          Promise.resolve(() => released.push("moved")).finally(() => {
            closed = true;
          }),
        () => Promise.resolve(() => released.push("resized")),
      ],
      () => closed
    );
    expect(release).toBeNull();
    expect(released).toEqual(["moved"]);
  });

  it("releases a fully registered set when the window closed on the LAST await", async () => {
    const released: string[] = [];
    let closed = false;
    const release = await registerAll(
      [
        () =>
          Promise.resolve(() => released.push("moved")).finally(() => {
            closed = true;
          }),
      ],
      () => closed
    );
    expect(release).toBeNull();
    expect(released).toEqual(["moved"]);
  });
});

/**
 * #1171 - the pure conversion behind the geometry the activity window persists.
 *
 * `outerPosition()` and `innerSize()` answer in PHYSICAL pixels while the window builder's
 * `position` and `inner_size` take LOGICAL ones, so writing back what was read multiplies the
 * rect by the scale factor on every save-and-reopen cycle. The defect is invisible at a scale
 * factor of 1, which is the only one a headless run ever has, so it is pinned here.
 */
describe("persisting the activity window's geometry (#1171)", () => {
  it("converts a physical rect to the logical one the builder restores", () => {
    expect(logicalGeometry({ x: 300, y: 150, width: 2205, height: 1200 }, 1.5)).toEqual({
      x: 200,
      y: 100,
      width: 1470,
      height: 800,
    });
  });

  it("round-trips at 100%, where the two units coincide", () => {
    const rect = { x: 10, y: 20, width: 1470, height: 800 };
    expect(logicalGeometry(rect, 1)).toEqual(rect);
  });

  it("is stable under repeated save-and-reopen at a fractional factor", () => {
    // The reopened window reports the same physical rect the logical one asks for, so a
    // second save must produce the same numbers rather than shrink or grow them.
    const once = logicalGeometry({ x: 0, y: 0, width: 2205, height: 1200 }, 1.5);
    const twice = logicalGeometry(
      { x: 0, y: 0, width: once.width * 1.5, height: once.height * 1.5 },
      1.5
    );
    expect(twice).toEqual(once);
  });

  it("treats an impossible scale factor as 1 rather than producing Infinity", () => {
    const rect = { x: 1, y: 2, width: 3, height: 4 };
    expect(logicalGeometry(rect, 0)).toEqual(rect);
  });
});

/**
 * #1188 - the bound that turns "never settles" into "settles as a failure".
 *
 * The failure `withDeadline` exists for needs a promise that never settles, which no real IPC
 * call produces on demand, so it is tested here in isolation rather than through the window.
 */
describe("bounding one activity round (#1188)", () => {
  // Fake timers are required even where nothing is advanced: `vi.getTimerCount()` goes through
  // Vitest's `_checkFakeTimers()`, which throws outright when the timer APIs are not mocked.
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("resolves with the work's value and leaves no timer armed (T4)", async () => {
    expect(await withDeadline(Promise.resolve(7), 1_000, "m")).toBe(7);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("rejects with the message at the deadline and leaves no timer armed (T5)", async () => {
    const message = "the reply is not coming";
    const promise = withDeadline(new Promise<never>(() => {}), 1_000, message);
    // Attach the expectation BEFORE advancing the clock. Advancing first leaves the promise
    // rejected with nothing attached for a turn, which Vitest can report as an unhandled
    // rejection and turn into a flake in an otherwise correct test.
    const assertion = expect(promise).rejects.toThrow(new Error(message));
    await vi.advanceTimersByTimeAsync(1_000);
    await assertion;
    expect(vi.getTimerCount()).toBe(0);
  });

  it("reports a hung round within one period (T7)", () => {
    // The service level, and nothing more: a deadline at or above the period means a hung
    // round is reported later than one period after it hangs, which is the silence this issue
    // exists to end. It is NOT what stops rounds overlapping. Nothing can overlap here at any
    // value, because the chain arms the next round only inside `runPollRound`'s `finally`.
    expect(POLL_TIMEOUT_MS).toBeLessThan(POLL_FOCUSED_MS);
    // Not strict: this says only that an unfocused window must not poll MORE often than a
    // focused one. Equal cadences would be legitimate.
    expect(POLL_FOCUSED_MS).toBeLessThanOrEqual(POLL_UNFOCUSED_MS);
  });
});

/**
 * #1188 - the poll chain that could die, and the window that never said so.
 *
 * The only re-arm of `pollTimer` lived inside the `.then()` of the fetch the poll had just
 * issued, so a `get_watcher_activity` that never settled ended the chain for the life of the
 * window. It was silent twice over: push events kept painting, so the table was stale rather
 * than blank, and `loadError` is only ever written from a `catch`, which a promise that never
 * settles never reaches.
 */
describe("the activity poll chain (#1188)", () => {
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

  /**
   * Drive the mount with microtask turns, never `waitFor`.
   *
   * `waitFor` polls on `Date.now()` and a real `setTimeout` (`ui-harness.tsx:71-92`), both of
   * which `vi.useFakeTimers()` replaces, so under fake timers it does not even time out: it
   * hangs until Vitest kills the test. Every mount await here resolves through `FakeTransport`,
   * which is `async` but never timer-based, so microtasks alone settle it.
   *
   * 200 is a margin, not a measurement. The precondition assertion at every call site is what
   * makes an undercount safe: it fails loudly instead of passing vacuously. Do not delete it,
   * and if a future change outgrows the count, raise the count rather than reaching for
   * `waitFor`. This also relies on nothing on the watchers mount path waiting on a frame --
   * `installBrowserDomStubs` stubs `requestAnimationFrame` with a real `setTimeout`, which
   * under fake timers would never run. There is no `requestAnimationFrame` in `src/watchers/`.
   *
   * #1196 raised it from 50. Bounding all eight mount awaits adds microtask hops per step --
   * `Promise.race` attaches a `then`, `.finally()` is specified as a `then` whose handler
   * returns `Promise.resolve(x).then(...)`, and each `step` async frame adds its own -- which
   * puts the mount over the old 50 and failed T1, T2, T3 and T6 at their FIRST assertion,
   * before any timer was advanced. That is this count, not broken arming. 200 microtask turns
   * cost microseconds, so the margin is cheap.
   */
  const flushMount = async (): Promise<void> => {
    for (let i = 0; i < 200; i += 1) await Promise.resolve();
  };

  const errorBanner = (root: HTMLElement): HTMLElement | null =>
    root.querySelector<HTMLElement>('[data-ac-testid="watchers.error"]');

  /**
   * Fake timers must be installed BEFORE the render.
   *
   * `vi.useFakeTimers()` replaces the global timer functions; it does not convert timers that
   * are already armed. This window arms `pollTimer` during its own mount, so installing them
   * afterwards would leave the first period running on real time. The precedent this repo
   * already has, `ProjectPanel.restart-toast.test.tsx:211-251`, does the opposite for the same
   * underlying rule -- install fake timers before the timer under test is armed -- because
   * there the timer is armed by a click after the mount, so there is an interval to swap the
   * clock in. Here there is none. Copy the rule, not either ordering.
   *
   * `initialSessionId="s1"` is mandatory: `transportWith` registers two agent sessions, so a
   * render without it lands in "All sessions" and every round issues two calls instead of one,
   * which makes every exact call count below wrong.
   */
  const renderWithFakeClock = (fake: FakeTransport) => {
    vi.spyOn(document, "hasFocus").mockReturnValue(true);
    vi.useFakeTimers();
    return renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
  };

  /** Round 1 and rounds 3+ answer; round 2 is whatever the failure under test is. */
  function transportWithSecondRound(
    snap: WatcherActivitySnapshot,
    secondRound: () => unknown
  ): FakeTransport {
    const fake = transportWith(snap);
    let round = 0;
    fake.onInvoke("get_watcher_activity", () => {
      round += 1;
      return round === 2 ? secondRound() : snap;
    });
    return fake;
  }

  it("keeps polling after a fetch that never settles, and says the round failed (T1)", async () => {
    const snap = snapshot({ matches: [match()] });
    const fake = transportWithSecondRound(snap, () => new Promise<never>(() => {}));
    const rendered = renderWithFakeClock(fake);
    try {
      // 1. The mount settles. This first call is the SCOPE EFFECT's, not the poll's:
      //    `schedulePoll()` only arms a timer and issues nothing.
      await flushMount();
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(1);
      expect(errorBanner(rendered.root)).toBeNull();

      // 2. One period later round 2 has fired and is hung. Nothing on screen says so yet,
      //    which is exactly the state this issue reports.
      await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS);
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(2);
      expect(errorBanner(rendered.root)).toBeNull();

      // 3. The deadline was armed when round 2 was ISSUED, so it expires at
      //    POLL_FOCUSED_MS + POLL_TIMEOUT_MS, which is where this lands. That arithmetic holds
      //    for any positive timeout: while round 2 is pending, the chained design has no next
      //    poll timer armed at all. It does not depend on 8s being under 10s.
      await vi.advanceTimersByTimeAsync(POLL_TIMEOUT_MS);
      expect(errorBanner(rendered.root)?.textContent).toBe(POLL_TIMEOUT_MESSAGE);
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(2);

      // 4. And the chain survived it. On `f08b8241` the test never reaches this line: it
      //    fails at step 3 on the absent banner.
      await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS);
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(3);

      // 5. A good round leaves no trace of the bad one.
      await flushMount();
      expect(errorBanner(rendered.root)).toBeNull();
    } finally {
      rendered.cleanup();
      vi.useRealTimers();
      vi.restoreAllMocks();
    }
  });

  it("keeps the rows it already has while a round is timing out (T2)", async () => {
    const snap = snapshot({ matches: [match()] });
    const fake = transportWithSecondRound(snap, () => new Promise<never>(() => {}));
    const rendered = renderWithFakeClock(fake);
    try {
      await flushMount();
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(1);

      await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS);
      await vi.advanceTimersByTimeAsync(POLL_TIMEOUT_MS);
      expect(errorBanner(rendered.root)?.textContent).toBe(POLL_TIMEOUT_MESSAGE);

      // The banner says the list MAY be out of date, so blanking the table would destroy
      // information the user still wants. Guards against a future "clear on error".
      expect(
        rendered.root.querySelector('[data-ac-testid="watchers.row.s1:1"]')
      ).toBeTruthy();
    } finally {
      rendered.cleanup();
      vi.useRealTimers();
      vi.restoreAllMocks();
    }
  });

  it("cannot paint a round that was abandoned at the deadline (T3)", async () => {
    const snap = snapshot({ matches: [match()] });
    const late = deferred<WatcherActivitySnapshot>();
    const fake = transportWithSecondRound(snap, () => late.promise);
    const rendered = renderWithFakeClock(fake);
    try {
      await flushMount();
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(1);

      await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS);
      await vi.advanceTimersByTimeAsync(POLL_TIMEOUT_MS);
      expect(errorBanner(rendered.root)?.textContent).toBe(POLL_TIMEOUT_MESSAGE);

      // The ordering IS the test. Resolve while round 2 is still the newest request, so
      // `requestCounter` cannot supply the answer: a later round would have advanced it and
      // the commit guard would discard this response even if the continuation wrongly resumed.
      // With the counter unmoved, the only thing left standing between this snapshot and the
      // table is that `refresh()`'s await already threw at the deadline.
      late.resolve(snapshot({ matches: [match({ seq: 999 })] }));
      await flushMount();
      expect(rendered.root.querySelector('[data-ac-testid="watchers.row.s1:999"]')).toBeNull();

      // And the chain recovered regardless.
      await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS);
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(3);
    } finally {
      rendered.cleanup();
      vi.useRealTimers();
      vi.restoreAllMocks();
    }
  });

  /**
   * A fixation test, not a regression one: a REJECTED round already re-armed on `f08b8241`,
   * because `refresh()` catches the rejection itself and therefore fulfils, so the old
   * `.then()` ran. Expect it to pass against the baseline. Its job is to pin that behaviour so
   * the move from `.then()` to `finally` demonstrably does not lose it.
   */
  it("re-arms after a round the backend rejects, and shows its message (T6)", async () => {
    const snap = snapshot({ matches: [match()] });
    const fake = transportWithSecondRound(snap, () => {
      throw new Error("the ring buffer is gone");
    });
    const rendered = renderWithFakeClock(fake);
    try {
      await flushMount();
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(1);
      expect(errorBanner(rendered.root)).toBeNull();

      await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS);
      await flushMount();
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(2);
      expect(errorBanner(rendered.root)?.textContent).toBe("the ring buffer is gone");

      await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS);
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(3);
    } finally {
      rendered.cleanup();
      vi.useRealTimers();
      vi.restoreAllMocks();
    }
  });
});

/**
 * #1196 - the mount that could hang before it ever armed its poll.
 *
 * `schedulePoll()` was the SOLE entry into the poll chain and it sat behind eight sequential
 * IPC awaits, none of them bounded. A single reply that never arrived left the chain unarmed,
 * and it was silent by construction: both writers of `loadError` were unreachable, because a
 * promise that never settles never rejects and `refresh()` itself was gated on the arming. The
 * window sat on "Waiting for the first sample..." for as long as it was open.
 *
 * The remedy is #1188's, one level up: a deadline to turn "never settles" into "settles as a
 * failure", and a `finally` to make the arming unconditional once everything pending has been
 * made to settle. Neither half works alone.
 */
describe("the watcher window mount chain (#1196)", () => {
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

  /**
   * The #1188 driver, at its post-#1196 count.
   *
   * Copied rather than shared because AC2 makes `flushMount()`'s turn count the ONLY permitted
   * modification inside the #1188 block, and hoisting it to module scope would be a second
   * one. `waitFor` is still unusable here for the reason given in that block: it polls on
   * `Date.now()` and a real `setTimeout`, both of which `vi.useFakeTimers()` replaces.
   */
  const flushMount = async (): Promise<void> => {
    for (let i = 0; i < 200; i += 1) await Promise.resolve();
  };

  /**
   * Drain the zero-length deadlines the exhausted budget arms.
   *
   * Once the budget is spent, every remaining step races a `setTimeout(..., 0)` that is armed
   * only AFTER the previous step's rejection has propagated through several microtask turns.
   * The loop is what walks that chain; a single tick is not enough for more than one link.
   *
   * The tick is 1ms and not 0, which is measured rather than assumed. Vitest 4.1.5's vendored
   * fake timers do not re-scan for a timer armed during a tick, so an advance of length zero
   * cannot reach one: probed here, a `setTimeout(..., 0)` armed inside another timer's callback
   * survives `advanceTimersByTimeAsync(0)` with `getTimerCount() === 1` and fires only on
   * `advanceTimersByTimeAsync(1)`, and a three-deep promise-mediated chain of the shape this
   * mount produces drains one link under ten zero-length advances against all three under ten
   * one-millisecond ones. Ten ticks covers the seven steps that can inherit an exhausted
   * budget, and the 10ms it costs is far inside every poll-period assertion below.
   */
  const drainZeroTimers = async (): Promise<void> => {
    for (let i = 0; i < 10; i += 1) await vi.advanceTimersByTimeAsync(1);
  };

  /** Fake timers before the render, and `initialSessionId="s1"` so a round is one call. */
  const renderWithFakeClock = (fake: FakeTransport) => {
    vi.spyOn(document, "hasFocus").mockReturnValue(true);
    vi.useFakeTimers();
    return renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
  };

  it("arms the poll after a startup call that never settles, and asks only once (M1)", async () => {
    const fake = transportWith(snapshot());
    // Step #1. A promise that never settles is the failure this issue exists for, and no real
    // IPC call produces one on demand.
    fake.onInvoke("get_settings", () => new Promise(() => {}));
    const rendered = renderWithFakeClock(fake);
    try {
      // 1. Stuck at step #1, with nothing on screen saying so. This is the shipped behaviour
      //    the issue reports, reproduced.
      await flushMount();
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(0);
      expect(startupBanner(rendered.root)).toBeNull();

      // 2. Still stuck one millisecond short of the budget. This is what catches a per-await
      //    implementation at MOUNT_TIMEOUT_MS / 8, which M3 alone would let through.
      await vi.advanceTimersByTimeAsync(MOUNT_TIMEOUT_MS - 1);
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(0);

      // 3. The budget expires. Steps #2 to #8 inherit nothing and settle through their
      //    zero-length deadlines, and the `finally` arms.
      await vi.advanceTimersByTimeAsync(1);
      await drainZeroTimers();
      await flushMount();

      // 4. And the window says so, in the exact words it paints.
      expect(startupBanner(rendered.root)?.textContent).toBe(STARTUP_DEGRADED_MESSAGE);

      // 5. `scopeSettled` was set, so the scope effect issued its first fetch.
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(1);

      // 6. Real data reached the view. Asserted because a window that merely LOOKS alive is
      //    the failure mode here: "warming" is what a dead window shows too.
      expect(
        rendered.root.querySelector('[data-ac-testid="watchers.empty.warming"]')
      ).toBeNull();
      expect(
        rendered.root.querySelector('[data-ac-testid="watchers.empty.unconfigured"]')
      ).toBeTruthy();

      // 7. The chain survived the degraded start.
      await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS);
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(2);

      // 8. And arming did not turn one lost `get_settings` into one per period for the life of
      //    the window. Pairs with M8: this pins that the count does not grow while the command
      //    is hung, M8 that it does grow while it is healthy. Neither pin works alone.
      const before = fake.callsFor("get_watcher_activity").length;
      await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS * 3);
      expect(fake.callsFor("get_watcher_activity").length).toBeGreaterThan(before);
      expect(fake.callsFor("get_settings")).toHaveLength(1);
    } finally {
      rendered.cleanup();
      vi.useRealTimers();
      vi.restoreAllMocks();
    }
  });

  it("runs the steps after a hung one instead of jumping to the end (M2)", async () => {
    const fake = transportWith(snapshot());
    // Step #6 only. The five subscriptions are not interchangeable, and a chain that skipped
    // ahead would silently lose #7 and #8 as well.
    const realListen = fake.listen.bind(fake);
    fake.listen = ((event: string, callback: (payload: never) => void) =>
      event === "session_renamed"
        ? new Promise<never>(() => {})
        : realListen(event, callback)) as FakeTransport["listen"];

    const rendered = renderWithFakeClock(fake);
    try {
      await flushMount();
      expect(fake.callsFor("get_watchers_scope")).toHaveLength(0);

      await vi.advanceTimersByTimeAsync(MOUNT_TIMEOUT_MS);
      await drainZeroTimers();
      await flushMount();

      // Step #8 was still ISSUED. `work` is evaluated before the bound is applied, so this
      // holds whether or not its zero-budget race is won by the call, and the test
      // deliberately does not assert which side won.
      expect(fake.callsFor("get_watchers_scope")).toHaveLength(1);
      expect(startupBanner(rendered.root)?.textContent).toBe(STARTUP_DEGRADED_MESSAGE);
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(1);
    } finally {
      rendered.cleanup();
      vi.useRealTimers();
      vi.restoreAllMocks();
    }
  });

  it("spends one budget across the whole chain, not one per await (M3)", async () => {
    const fake = transportWith(snapshot());
    fake.onInvoke("get_settings", () => new Promise(() => {}));
    fake.onInvoke("list_sessions", () => new Promise(() => {}));
    const rendered = renderWithFakeClock(fake);
    try {
      await flushMount();
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(0);

      // ONE budget, for two hung steps. Under a per-await design at MOUNT_TIMEOUT_MS each,
      // step #7 would still be pending here and the window would still be unarmed.
      await vi.advanceTimersByTimeAsync(MOUNT_TIMEOUT_MS);
      await drainZeroTimers();
      await flushMount();

      expect(fake.callsFor("get_watcher_activity")).toHaveLength(1);
      expect(startupBanner(rendered.root)?.textContent).toBe(STARTUP_DEGRADED_MESSAGE);
    } finally {
      rendered.cleanup();
      vi.useRealTimers();
      vi.restoreAllMocks();
    }
  });

  it("keeps the mount budget between one round's allowance and one period (M4)", () => {
    // The floor is strict: the mount has eight calls to make where a poll round has one, so a
    // budget at or below one round's allowance would bound out a start that is merely slow.
    expect(MOUNT_TIMEOUT_MS).toBeGreaterThan(POLL_TIMEOUT_MS);

    // The ceiling is NOT strict, and equality is the intended value. `POLL_TIMEOUT_MS <
    // POLL_FOCUSED_MS` above is strict for a reason that does not transfer: rounds are chained,
    // so a round deadline at or above the period reports a hung round no sooner than the next
    // round would have been due. The mount has no cadence behind it -- nothing is due at
    // t=MOUNT_TIMEOUT_MS for the budget to collide with, because the first fetch is issued at
    // the arming, whenever that is. This ceiling says only how long a user may look at a
    // window that is doing nothing.
    expect(MOUNT_TIMEOUT_MS).toBeLessThanOrEqual(POLL_FOCUSED_MS);
  });

  /**
   * The test the `loadSessions`/`reloadSessions` split exists for.
   *
   * `reloadSessions` catches its own rejection and therefore FULFILS, so a mount that wrapped
   * it could not observe a rejecting `list_sessions` at all. In "All agents" that reproduces
   * the issue's exact symptom -- `scopeIds()` is `[]`, `Promise.all([])` resolves, and the
   * window paints "Waiting for the first sample..." -- with no banner of any kind.
   */
  it("raises the notice when list_sessions REJECTS, not only when it hangs (M6)", async () => {
    // `transportWith` is mandatory as the base: with a bare `new FakeTransport()`, steps #1 and
    // #8 have no handler and throw on their own, the notice would appear whatever step #7 did,
    // and this test would pass against the implementation it exists to reject. Here #1 resolves,
    // #2 to #6 register and #8 resolves, so step #7 is the ONLY possible source of the notice.
    const fake = transportWith(snapshot());
    fake.reject("list_sessions", "the session list is gone");
    // `renderWithFakeTransport`, not `renderWithFakeClock`: that helper hardcodes
    // `initialSessionId="s1"`, and this must render WITHOUT one so the scope is "All agents".
    // No fake timers either, because the rejection is immediate.
    const rendered = renderWithFakeTransport(() => <WatchersApp />, fake);
    try {
      await waitFor(() => expect(startupBanner(rendered.root)).toBeTruthy());
      expect(startupBanner(rendered.root)?.textContent).toBe(STARTUP_DEGRADED_MESSAGE);

      // The warming state may legitimately be up: with no session list there is no scope, so
      // there is nothing to fetch. What must not happen is the window being SILENT about it,
      // so the assertion is the notice rather than the absence of warming.
      expect(startupBanner(rendered.root)).toBeTruthy();
    } finally {
      rendered.cleanup();
    }
  });

  it("unlistens a subscription abandoned at the budget that answers late (M7)", async () => {
    const fake = transportWith(snapshot());
    const late = deferred<() => void>();
    const realListen = fake.listen.bind(fake);
    fake.listen = ((event: string, callback: (payload: never) => void) =>
      event === "session_created"
        ? late.promise
        : realListen(event, callback)) as FakeTransport["listen"];

    const rendered = renderWithFakeClock(fake);
    const unlisten = vi.fn();
    try {
      await flushMount();
      await vi.advanceTimersByTimeAsync(MOUNT_TIMEOUT_MS);
      await drainZeroTimers();
      await flushMount();
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(1);

      rendered.cleanup();

      // The reply lands after the window is gone. `register`'s continuation sees `disposed`,
      // unlistens on the spot and throws the sentinel, which the race's still-attached handler
      // absorbs instead of letting it escape as an unhandled rejection.
      late.resolve(unlisten);
      await flushMount();
      expect(unlisten).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
      vi.restoreAllMocks();
    }
  });

  /**
   * A fixation test, not a regression one: it passes against the baseline, and is excluded from
   * AC1 for that reason.
   *
   * Its target is not the defect but a wrong FIX for it. An implementer who deletes the poll's
   * settings refresh instead of routing it through the single-flight passes every other test
   * here while silently losing the behaviour `runPollRound` documents, where a watcher saved in
   * the Settings modal appears without reopening the window. Pairs with M1 step 8: that one
   * pins that the count does not grow while `get_settings` is hung, which a deleted refresh
   * also satisfies; this one pins that it does grow while it is healthy, which an unguarded
   * refresh also satisfies. Only together do they pin "guarded, and still there".
   */
  it("still refreshes the settings every period on a healthy window (M8)", async () => {
    const fake = transportWith(snapshot());
    const rendered = renderWithFakeClock(fake);
    try {
      await flushMount();
      expect(fake.callsFor("get_settings")).toHaveLength(1);

      await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS);
      await flushMount();
      expect(fake.callsFor("get_settings")).toHaveLength(2);

      await vi.advanceTimersByTimeAsync(POLL_FOCUSED_MS);
      await flushMount();
      expect(fake.callsFor("get_settings")).toHaveLength(3);

      // And a healthy start paints nothing new.
      expect(startupBanner(rendered.root)).toBeNull();
    } finally {
      rendered.cleanup();
      vi.useRealTimers();
      vi.restoreAllMocks();
    }
  });
});
