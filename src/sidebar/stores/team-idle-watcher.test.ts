// @vitest-environment jsdom
//
// Pure-helper tests for the focused-WG suppression + grace period
// behavior introduced in #254, plus the #2109 decision helpers. We
// exercise the helpers directly — no SolidJS root, no store mocks, no
// fake timers. The wiring inside `startTeamIdleWatcher`'s createEffect
// is covered by the integration case at the bottom of this file, which
// drives the real stores through a FakeTransport.
//
// jsdom is required because importing this module evaluates the
// file-scope `createSignal` and pulls in the sibling stores, which
// transitively touch `window.location` in `transport-ws.ts`.

import { afterEach, beforeEach, describe, it, expect, vi } from "vitest";

vi.hoisted(() => {
  class MockWebSocket {
    static readonly CONNECTING = 0;
    static readonly OPEN = 1;
    static readonly CLOSING = 2;
    static readonly CLOSED = 3;

    readonly url: string;
    binaryType: BinaryType = "blob";
    readyState = MockWebSocket.CLOSED;

    constructor(url: string) {
      this.url = url;
    }

    send(): void {}

    close(): void {
      this.readyState = MockWebSocket.CLOSED;
    }
  }

  Object.defineProperty(globalThis, "WebSocket", {
    configurable: true,
    writable: true,
    value: MockWebSocket,
  });
});

import {
  allSessionsIdle,
  beepIdleTransitions,
  GRACE_MS,
  hasBusyToIdleTransition,
  pruneExpiredGrace,
  shouldSuppressBeep,
  startTeamIdleWatcher,
  updateGraceOnFocusChange,
} from "./team-idle-watcher";
import { playTeamIdleBeep } from "../../shared/sound";
import { __setTransportForTests } from "../../shared/ipc";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { sessionsStore } from "./sessions";
import { projectStore } from "./project";
import { settingsStore } from "../../shared/stores/settings";

vi.mock("../../shared/sound", () => ({
  playTeamIdleBeep: vi.fn(),
  setSoundsEnabled: vi.fn(),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => {
    throw new Error("tauri unavailable in tests");
  },
}));

describe("shouldSuppressBeep (#254)", () => {
  it("suppresses the focused workgroup", () => {
    expect(shouldSuppressBeep("A", "A", new Map(), 0)).toBe(true);
  });

  it("does NOT suppress non-focused workgroups with no grace entry", () => {
    expect(shouldSuppressBeep("B", "A", new Map(), 0)).toBe(false);
  });

  it("suppresses inside an active grace window", () => {
    const grace = new Map<string, number>([["A", 4000]]);
    expect(shouldSuppressBeep("A", "B", grace, 2000)).toBe(true);
  });

  it("does NOT suppress once the grace window has expired", () => {
    const grace = new Map<string, number>([["A", 4000]]);
    expect(shouldSuppressBeep("A", "B", grace, 5000)).toBe(false);
  });

  it("does NOT suppress at the exact grace boundary (now === until)", () => {
    // The half-open window `now < until` means `now === until` is
    // already outside grace. Locks in the boundary explicitly so a
    // future refactor to `now <= until` would have to flip this test.
    const grace = new Map<string, number>([["A", 4000]]);
    expect(shouldSuppressBeep("A", "B", grace, 4000)).toBe(false);
  });
});

describe("updateGraceOnFocusChange (#254)", () => {
  it("fast switch A→B→A re-arms B's grace and suppresses B inside the window", () => {
    const grace = new Map<string, number>();
    let prev: string | null = null;

    // T=0: initial focus arrives on A. previousFocusedWg was null,
    // so no grace is armed yet.
    prev = updateGraceOnFocusChange(prev, "A", grace, 0, GRACE_MS);
    expect(prev).toBe("A");
    expect(grace.size).toBe(0);

    // T=0: A → B. previousFocusedWg = "A" is left behind: grace
    // armed for A until 0 + 4000 = 4000.
    prev = updateGraceOnFocusChange(prev, "B", grace, 0, GRACE_MS);
    expect(prev).toBe("B");
    expect(grace.get("A")).toBe(4000);

    // T=500: B → A. previousFocusedWg = "B" leaves: grace for B
    // until 500 + 4000 = 4500.
    prev = updateGraceOnFocusChange(prev, "A", grace, 500, GRACE_MS);
    expect(prev).toBe("A");
    expect(grace.get("B")).toBe(4500);

    // T=3000: B is idle, but B is non-focused and still inside its
    // grace window (until 4500) → suppressed.
    expect(shouldSuppressBeep("B", "A", grace, 3000)).toBe(true);

    // T=5000: past B's grace → would beep.
    expect(shouldSuppressBeep("B", "A", grace, 5000)).toBe(false);
  });

  it("alt-tab out (focus → null) arms grace for the previously focused WG", () => {
    const grace = new Map<string, number>();

    // Established focus on A.
    const afterTabOut = updateGraceOnFocusChange(
      "A",
      null,
      grace,
      0,
      GRACE_MS,
    );

    expect(afterTabOut).toBeNull();
    expect(grace.get("A")).toBe(4000);

    // T=2000: A becomes idle while alt-tabbed out → suppressed
    // (grace still active).
    expect(shouldSuppressBeep("A", null, grace, 2000)).toBe(true);
  });

  it("alt-tab back in (focus = A) restores focused-WG suppression for A", () => {
    const grace = new Map<string, number>([["A", 4000]]);

    // The previous tick left focus at null with A's grace armed.
    // Now the window regains focus on A.
    const afterTabIn = updateGraceOnFocusChange(
      null,
      "A",
      grace,
      6000,
      GRACE_MS,
    );

    expect(afterTabIn).toBe("A");
    // previousFocusedWg was null, so no grace was armed for the
    // "left" side — and the existing A grace entry is untouched.
    expect(grace.get("A")).toBe(4000);

    // T=6000: A is focused again → suppressed by the focused-WG rule
    // (independent of A's now-expired grace).
    expect(shouldSuppressBeep("A", "A", grace, 6000)).toBe(true);
  });

  it("no-op when focus does not change", () => {
    const grace = new Map<string, number>();
    const result = updateGraceOnFocusChange("A", "A", grace, 1000, GRACE_MS);
    expect(result).toBe("A");
    expect(grace.size).toBe(0);
  });

  it("background-open: first real tick after listener resolves does not arm grace", () => {
    // Regression guard for the bug where the snapshot tick seeded
    // `previousFocusedWg` from a still-default-true `osFocused`,
    // causing the first real tick (after the async listener
    // resolved `osFocused` to false) to fire a phantom grace
    // window for the active session's WG. The fix moves the
    // updateGraceOnFocusChange call past the snapshot early-return,
    // so the first real tick always sees `previousFocusedWg` of
    // null and the resolved `focusedWg` — null→null is a no-op.
    const grace = new Map<string, number>();
    const result = updateGraceOnFocusChange(null, null, grace, 1000, GRACE_MS);
    expect(result).toBeNull();
    expect(grace.size).toBe(0);
  });
});

describe("hasBusyToIdleTransition (#2109)", () => {
  it("true when a session that was busy is idle now", () => {
    expect(
      hasBusyToIdleTransition(new Map([["s1", true]]), new Map([["s1", false]])),
    ).toBe(true);
  });

  it("false when a busy session is still busy", () => {
    expect(
      hasBusyToIdleTransition(new Map([["s1", true]]), new Map([["s1", true]])),
    ).toBe(false);
  });

  it("false when an idle session stays idle", () => {
    expect(
      hasBusyToIdleTransition(new Map([["s1", false]]), new Map([["s1", false]])),
    ).toBe(false);
  });

  it("false when the previously busy key is absent from the current map", () => {
    expect(hasBusyToIdleTransition(new Map([["s1", true]]), new Map())).toBe(false);
  });
});

describe("allSessionsIdle (#2109)", () => {
  it("false when there are no sessions", () => {
    expect(allSessionsIdle(new Map())).toBe(false);
  });

  it("true when every session is idle", () => {
    expect(allSessionsIdle(new Map([["s1", false], ["s2", false]]))).toBe(true);
  });

  it("false when any session is busy", () => {
    expect(allSessionsIdle(new Map([["s1", false], ["s2", true]]))).toBe(false);
  });
});

describe("beepIdleTransitions (#2109)", () => {
  const beepSpy = vi.mocked(playTeamIdleBeep);

  beforeEach(() => beepSpy.mockClear());

  const perSession = (entries: [string, boolean][]) => new Map(entries);

  it("beeps once on a busy→idle transition when every session is idle", () => {
    beepIdleTransitions(
      new Map([["wg", perSession([["s1", false]])]]),
      new Map([["wg", perSession([["s1", true]])]]),
      null,
      new Map(),
      0,
    );
    expect(beepSpy).toHaveBeenCalledTimes(1);
  });

  it("skips the focused workgroup", () => {
    beepIdleTransitions(
      new Map([["wg", perSession([["s1", false]])]]),
      new Map([["wg", perSession([["s1", true]])]]),
      "wg",
      new Map(),
      0,
    );
    expect(beepSpy).not.toHaveBeenCalled();
  });

  it("skips inside an active grace window", () => {
    beepIdleTransitions(
      new Map([["wg", perSession([["s1", false]])]]),
      new Map([["wg", perSession([["s1", true]])]]),
      null,
      new Map([["wg", 5000]]),
      1000,
    );
    expect(beepSpy).not.toHaveBeenCalled();
  });

  it("skips while another session in the room is still busy", () => {
    beepIdleTransitions(
      new Map([["wg", perSession([["s1", false], ["s2", true]])]]),
      new Map([["wg", perSession([["s1", true], ["s2", true]])]]),
      null,
      new Map(),
      0,
    );
    expect(beepSpy).not.toHaveBeenCalled();
  });

  it("skips a workgroup with no previous snapshot", () => {
    beepIdleTransitions(
      new Map([["wg", perSession([["s1", false]])]]),
      new Map(),
      null,
      new Map(),
      0,
    );
    expect(beepSpy).not.toHaveBeenCalled();
  });
});

describe("pruneExpiredGrace (#2109)", () => {
  it("deletes only entries whose grace has expired", () => {
    const grace = new Map([
      ["expired", 200],
      ["boundary", 1000],
      ["active", 1001],
    ]);
    pruneExpiredGrace(grace, 1000);
    expect([...grace.keys()]).toEqual(["active"]);
  });
});

describe("startTeamIdleWatcher wiring (#2109)", () => {
  const beepSpy = vi.mocked(playTeamIdleBeep);
  const projectPath = "C:\\Project";
  const workgroupPath = `${projectPath}\\.ac\\wg-1-dev-team`;
  const hop = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

  beforeEach(() => beepSpy.mockClear());
  afterEach(() => {
    sessionsStore.setSessions([]);
    projectStore.clear();
  });

  it("beeps once on the first busy→idle transition and honours the enabled and focus gates", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.resolve(
      "discover_project",
      discovery({
        teams: [{ name: "dev-team", agents: ["architect"], coordinator: "architect" }],
        workgroups: [
          {
            name: "wg-1-dev-team",
            path: workgroupPath,
            task: null,
            taskTitle: null,
            teamName: "dev-team",
            agents: [
              {
                name: "architect",
                path: `${workgroupPath}\\__agent_architect`,
                repoPaths: [],
                isCoordinator: true,
              },
            ],
          },
        ],
      }),
    );
    fake.resolve("get_settings", baseSettings({ teamIdleBeepEnabled: true }));

    const restoreTransport = __setTransportForTests(fake);
    let dispose: (() => void) | null = null;
    try {
      await projectStore.createAndLoad(projectPath);
      await settingsStore.load();
      sessionsStore.setSessions([session()]);

      dispose = startTeamIdleWatcher();

      // (a) the first tick only seeds the snapshot and never beeps.
      await hop();
      expect(beepSpy).not.toHaveBeenCalled();

      // (b) a busy→idle transition beeps once.
      sessionsStore.setSessionWaiting("session-1", true);
      await waitFor(() => expect(beepSpy).toHaveBeenCalledTimes(1));

      // (c) the enabled=false gate suppresses the same transition.
      fake.resolve("get_settings", baseSettings({ teamIdleBeepEnabled: false }));
      await settingsStore.load();
      sessionsStore.setSessionWaiting("session-1", false);
      sessionsStore.setSessionWaiting("session-1", true);
      await hop();
      expect(beepSpy).toHaveBeenCalledTimes(1);

      // (d) a focused workgroup is suppressed even while enabled.
      fake.resolve("get_settings", baseSettings({ teamIdleBeepEnabled: true }));
      await settingsStore.load();
      sessionsStore.setVisibleActiveIdForTests("session-1");
      sessionsStore.setSessionWaiting("session-1", false);
      sessionsStore.setSessionWaiting("session-1", true);
      await hop();
      expect(beepSpy).toHaveBeenCalledTimes(1);
    } finally {
      dispose?.();
      restoreTransport();
    }
  });
});
