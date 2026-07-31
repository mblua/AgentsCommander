// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import WatchersApp, { logicalGeometry, registerAll } from "./App";
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
   * Solid does not await the promise `onMount(async ...)` returns, so anything that escapes
   * its `catch` is an unhandled rejection rather than an error anyone sees. A setup that
   * fails has to stop the mount AND say so.
   */
  it("reports a failed setup instead of leaving an unhandled rejection behind", async () => {
    const fake = transportWith(snapshot());
    const realListen = fake.listen.bind(fake);
    fake.listen = ((event: string, callback: (payload: never) => void) =>
      event === "session_renamed"
        ? Promise.reject(new Error("transport closed mid-subscribe"))
        : realListen(event, callback)) as FakeTransport["listen"];

    const rendered = renderWithFakeTransport(() => <WatchersApp initialSessionId="s1" />, fake);
    try {
      await waitFor(() =>
        expect(
          rendered.root.querySelector('[data-ac-testid="watchers.error"]')?.textContent
        ).toContain("transport closed mid-subscribe")
      );
      // The mount stopped where it stood: nothing was fetched behind the failure.
      expect(fake.callsFor("get_watcher_activity")).toHaveLength(0);
    } finally {
      rendered.cleanup();
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
