// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import WatchersApp from "./App";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../shared/testing/ui-harness";
import type { WatcherActivitySnapshot, WatcherMatchPayload } from "../shared/types";

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
  return fake;
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
});
