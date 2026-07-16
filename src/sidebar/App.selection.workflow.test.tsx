// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SidebarApp from "./App";
import { sessionsStore } from "./stores/sessions";
import { __setTransportForTests } from "../shared/ipc";
import { FakeTransport } from "../shared/testing/fake-transport";
import type { TransportConnectionState, UnlistenFn } from "../shared/transport";
import { voiceRecorder } from "../shared/voice-recorder";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../shared/testing/ui-harness";
import {
  dormantSelection,
  initialSelection,
  liveSelection,
  noneSelection,
  ROOT_SESSION,
  SESSION_A,
  SESSION_B,
  userLiveSelection,
} from "../shared/testing/session-selection";

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason: unknown) => void;
} {
  let resolve = (_value: T): void => undefined;
  let reject = (_reason: unknown): void => undefined;
  const promise = new Promise<T>((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });
  return { promise, resolve, reject };
}

class TrackingFakeTransport extends FakeTransport {
  readonly selectionUnlisten = vi.fn();
  readonly connectionUnlisten = vi.fn();
  selectionRegistrationGate: Promise<void> | null = null;

  override async listen<T>(
    event: string,
    callback: (payload: T) => void,
  ): Promise<UnlistenFn> {
    if (event === "session_switched" && this.selectionRegistrationGate) {
      await this.selectionRegistrationGate;
    }
    const unlisten = await super.listen(event, callback);
    return () => {
      if (event === "session_switched") this.selectionUnlisten();
      unlisten();
    };
  }

  override onConnectionState(
    callback: (state: TransportConnectionState) => void,
  ): UnlistenFn {
    const unlisten = super.onConnectionState(callback);
    return () => {
      this.connectionUnlisten();
      unlisten();
    };
  }
}

class RacingConnectionTransport extends FakeTransport {
  override onConnectionState(
    callback: (state: TransportConnectionState) => void,
  ): UnlistenFn {
    const unlisten = super.onConnectionState(callback);
    this.setConnectionState({ state: "disconnected", generation: 1 });
    this.setConnectionState({ state: "connected", generation: 2 });
    return unlisten;
  }
}

function setup(fake: FakeTransport, sessions: ReturnType<typeof session>[]): void {
  fake.resolve("get_settings", baseSettings({ projectPaths: [], projectPath: null }));
  fake.resolve("get_update_status", null);
  fake.resolve("search_repos", []);
  fake.resolve("list_sessions", sessions);
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
  fake.resolve("drain_session_warnings", []);
}

describe("SidebarApp authoritative selection workflow", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    vi.restoreAllMocks();
  });

  it("registers the event listener before hydration so revision 2 beats revision 1", async () => {
    const fake = new FakeTransport();
    const hydration = deferred<ReturnType<typeof liveSelection>>();
    const rows = [
      session({ id: SESSION_A, status: "running" }),
      session({ id: SESSION_B, status: "running" }),
    ];
    setup(fake, rows);
    fake.onInvoke("get_active_session", () => hydration.promise);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(fake.callsFor("get_active_session")).toHaveLength(1));
      fake.emitFromBackend("session_switched", userLiveSelection(SESSION_B, 2));
      hydration.resolve(liveSelection(SESSION_A, 1));
      await waitFor(() => expect(sessionsStore.activeId).toBe(SESSION_B));
      expect(sessionsStore.selectionRevision).toBe(2);
    } finally {
      rendered.cleanup();
    }
  });

  it("does not auto-select a first created row", async () => {
    const fake = new FakeTransport();
    setup(fake, []);
    fake.resolve("get_active_session", initialSelection());
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(sessionsStore.selectionRevision).toBe(0));
      fake.emitFromBackend("session_created", session({ id: SESSION_A, status: "running" }));
      await waitFor(() => expect(sessionsStore.sessions).toHaveLength(1));
      expect(sessionsStore.activeId).toBeNull();
      fake.emitFromBackend("session_switched", liveSelection(SESSION_A, 1));
      await waitFor(() => expect(sessionsStore.activeId).toBe(SESSION_A));
    } finally {
      rendered.cleanup();
    }
  });

  it("treats destroy as row removal and waits for the final selection", async () => {
    const fake = new FakeTransport();
    setup(fake, [session({ id: SESSION_A, status: "active" })]);
    fake.resolve("get_active_session", liveSelection(SESSION_A, 1));
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(sessionsStore.activeId).toBe(SESSION_A));
      fake.emitFromBackend("session_destroyed", { id: SESSION_A });
      await waitFor(() => expect(sessionsStore.sessions).toHaveLength(0));
      expect(sessionsStore.activeId).toBeNull();
      expect(sessionsStore.selection?.id).toBe(SESSION_A);
      fake.emitFromBackend("session_switched", noneSelection(2));
      await waitFor(() => expect(sessionsStore.selection?.mode).toBe("none"));
    } finally {
      rendered.cleanup();
    }
  });

  it("never re-promotes an exited Root between destroyed, row refresh, and dormant selection", async () => {
    const fake = new FakeTransport();
    setup(fake, [session({ id: ROOT_SESSION, status: "active", isRootAgent: true })]);
    fake.resolve("get_active_session", liveSelection(ROOT_SESSION, 1));
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(sessionsStore.activeId).toBe(ROOT_SESSION));
      fake.emitFromBackend("session_destroyed", { id: ROOT_SESSION });
      fake.emitFromBackend(
        "session_created",
        session({ id: ROOT_SESSION, status: { exited: 23 }, isRootAgent: true }),
      );
      await waitFor(() => expect(sessionsStore.sessions).toHaveLength(1));
      expect(sessionsStore.activeId).toBeNull();
      expect(sessionsStore.sessions[0].status).toEqual({ exited: 23 });
      fake.emitFromBackend("session_switched", dormantSelection(ROOT_SESSION, 2, 23));
      await waitFor(() => expect(sessionsStore.activeId).toBe(ROOT_SESSION));
      expect(sessionsStore.sessions[0].status).toEqual({ exited: 23 });
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps a non-Root first-exit row dormant until the authoritative final selection", async () => {
    const fake = new FakeTransport();
    setup(fake, [session({ id: SESSION_A, status: "active" })]);
    fake.resolve("get_active_session", liveSelection(SESSION_A, 1));
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(sessionsStore.activeId).toBe(SESSION_A));
      fake.emitFromBackend("session_destroyed", { id: SESSION_A });
      fake.emitFromBackend(
        "session_created",
        session({ id: SESSION_A, status: { exited: 31 } }),
      );
      await waitFor(() => expect(sessionsStore.sessions[0]?.status).toEqual({ exited: 31 }));
      expect(sessionsStore.activeId).toBeNull();
      fake.emitFromBackend("session_switched", noneSelection(2));
      await waitFor(() => expect(sessionsStore.selection?.mode).toBe("none"));
      expect(sessionsStore.sessions[0]?.status).toEqual({ exited: 31 });
    } finally {
      rendered.cleanup();
    }
  });

  it("highlights a missing-row live selection when its later created row arrives", async () => {
    const fake = new FakeTransport();
    setup(fake, []);
    fake.resolve("get_active_session", liveSelection(SESSION_A, 1));
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(sessionsStore.selection?.id).toBe(SESSION_A));
      expect(sessionsStore.activeId).toBeNull();
      fake.emitFromBackend(
        "session_created",
        session({ id: SESSION_A, status: "running" }),
      );
      await waitFor(() => expect(sessionsStore.activeId).toBe(SESSION_A));
      expect(sessionsStore.sessions[0]?.status).toBe("active");
    } finally {
      rendered.cleanup();
    }
  });

  it("revokes voice on disconnect, destroy, and an exited upsert", async () => {
    const revokeLive = vi.spyOn(voiceRecorder, "revokeLiveBinding").mockImplementation(() => undefined);
    const revokeSession = vi.spyOn(voiceRecorder, "revokeSession").mockImplementation(() => undefined);
    const fake = new FakeTransport();
    setup(fake, [session({ id: SESSION_A, status: "active" })]);
    fake.resolve("get_active_session", liveSelection(SESSION_A, 1));
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(sessionsStore.activeId).toBe(SESSION_A));
      revokeLive.mockClear();
      revokeSession.mockClear();
      fake.setConnectionState({ state: "disconnected", generation: 0 });
      expect(revokeLive).toHaveBeenCalledOnce();
      fake.emitFromBackend("session_destroyed", { id: SESSION_B });
      expect(revokeSession).toHaveBeenCalledWith(SESSION_B);
      revokeSession.mockClear();
      fake.emitFromBackend(
        "session_created",
        session({ id: SESSION_A, status: { exited: 9 } }),
      );
      expect(revokeSession).toHaveBeenCalledWith(SESSION_A);
    } finally {
      rendered.cleanup();
    }
  });

  it("retries exact busy hydration and stops retrying after an accepted event", async () => {
    const fake = new FakeTransport();
    setup(fake, [session({ id: SESSION_A, status: "running" })]);
    const hydration = deferred<ReturnType<typeof liveSelection>>();
    fake.onInvoke("get_active_session", () => hydration.promise);
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(fake.callsFor("get_active_session")).toHaveLength(1));
      fake.emitFromBackend("session_switched", userLiveSelection(SESSION_A, 2));
      hydration.reject("selectionCoordinatorBusy");
      await new Promise((resolve) => setTimeout(resolve, 80));
      expect(fake.callsFor("get_active_session")).toHaveLength(1);
      expect(sessionsStore.selectionRevision).toBe(2);
      expect(sessionsStore.activeId).toBe(SESSION_A);
    } finally {
      rendered.cleanup();
    }
  });

  it("restores the current sidebar snapshot when busy capacity becomes available", async () => {
    const fake = new FakeTransport();
    setup(fake, [session({ id: SESSION_A, status: "running" })]);
    let attempts = 0;
    fake.onInvoke("get_active_session", () => {
      attempts += 1;
      if (attempts === 1) throw "selectionCoordinatorBusy";
      return liveSelection(SESSION_A, 1);
    });
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(sessionsStore.activeId).toBe(SESSION_A));
      expect(fake.callsFor("get_active_session")).toHaveLength(2);
    } finally {
      rendered.cleanup();
    }
  });

  it("uses the raced connection snapshot without duplicate generation hydration", async () => {
    const fake = new RacingConnectionTransport();
    fake.setConnectionState({ state: "connected", generation: 0 });
    setup(fake, [session({ id: SESSION_A, status: "running" })]);
    fake.resolve("get_active_session", liveSelection(SESSION_A, 1));
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(sessionsStore.activeId).toBe(SESSION_A));
      expect(sessionsStore.connectionGeneration).toBe(2);
      expect(fake.callsFor("get_active_session")).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });

  it("disposes selection and connection listeners exactly once", async () => {
    const fake = new TrackingFakeTransport();
    setup(fake, []);
    fake.resolve("get_active_session", initialSelection());
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    await waitFor(() => expect(sessionsStore.selectionRevision).toBe(0));
    rendered.cleanup();
    expect(fake.selectionUnlisten).toHaveBeenCalledOnce();
    expect(fake.connectionUnlisten).toHaveBeenCalledOnce();
  });

  it("immediately disposes a selection listener that resolves after unmount", async () => {
    const fake = new TrackingFakeTransport();
    const gate = deferred<void>();
    fake.selectionRegistrationGate = gate.promise;
    setup(fake, []);
    fake.resolve("get_active_session", initialSelection());
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    rendered.cleanup();
    const restoreLateTransport = __setTransportForTests(fake);
    try {
      gate.resolve(undefined);
      await waitFor(() => expect(fake.selectionUnlisten).toHaveBeenCalledOnce());
      expect(fake.connectionUnlisten).not.toHaveBeenCalled();
      expect(fake.callsFor("get_active_session")).toHaveLength(0);
    } finally {
      restoreLateTransport();
    }
  });

  it("drops an initial list completion after unmount", async () => {
    const fake = new FakeTransport();
    const rows = deferred<ReturnType<typeof session>[]>();
    setup(fake, []);
    fake.onInvoke("list_sessions", () => rows.promise);
    fake.resolve("get_active_session", initialSelection());
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    await waitFor(() => expect(fake.callsFor("list_sessions")).toHaveLength(1));
    rendered.cleanup();
    rows.resolve([session({ id: SESSION_A, status: "active" })]);
    await Promise.resolve();
    await Promise.resolve();
    expect(sessionsStore.sessions).toHaveLength(0);
    expect(sessionsStore.activeId).toBeNull();
  });
});
