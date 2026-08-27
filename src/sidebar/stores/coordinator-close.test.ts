// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { __setTransportForTests } from "../../shared/ipc";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { session } from "../../shared/testing/ui-harness";
import { sessionsStore } from "./sessions";
import {
  requestCoordinatorClose,
  requestCoordinatorCloseById,
  confirmPendingCoordinatorClose,
  pendingCoordinatorClose,
  setPendingCoordinatorClose,
  registerCoordinatorCloseModalHost,
  __resetCoordinatorCloseModalHostForTests,
} from "./coordinator-close";

// #588 — the single by-id helper funnels all three close paths (ProjectPanel
// "X", SessionItem, keyboard shortcut). A non-coordinator is a plain destroy; a
// coordinator goes through close_coordinator and opens the busy-confirm modal
// (the module-level pendingCoordinatorClose signal) when the backend refuses.

const COORD_NAME = "wg-2-dev-team/dev-webpage-ui";

describe("coordinator-close helper (#588)", () => {
  let fake: FakeTransport;
  let restoreTransport: () => void;
  // Tests that expect the confirm modal must register a host (a window with a
  // mounted ProjectPanel); dispose it in afterEach so the ref-count resets.
  let disposeModalHost: (() => void) | null = null;

  beforeEach(() => {
    fake = new FakeTransport();
    restoreTransport = __setTransportForTests(fake);
    sessionsStore.setSessions([]);
    setPendingCoordinatorClose(null);
    disposeModalHost = null;
    // Deterministic host==0 precondition regardless of test order/shuffle.
    __resetCoordinatorCloseModalHostForTests();
  });

  afterEach(() => {
    disposeModalHost?.();
    disposeModalHost = null;
    restoreTransport();
    sessionsStore.setSessions([]);
    setPendingCoordinatorClose(null);
    vi.restoreAllMocks();
  });

  it("routes a non-coordinator close straight to destroy_session (no close_coordinator, no modal)", async () => {
    const s = session({ id: "s-plain", isCoordinator: false });
    sessionsStore.setSessions([s]);
    fake.resolve("destroy_session", undefined);

    await requestCoordinatorClose(s);

    expect(fake.callsFor("destroy_session").map((c) => c.args)).toEqual([{ id: "s-plain" }]);
    expect(fake.callsFor("close_coordinator")).toHaveLength(0);
    expect(pendingCoordinatorClose()).toBeNull();
  });

  it("closes an idle coordinator directly (close_coordinator confirmed:false -> closed:true, no modal)", async () => {
    const s = session({ id: "s-coord", name: COORD_NAME, isCoordinator: true });
    sessionsStore.setSessions([s]);
    fake.resolve("close_coordinator", { closed: true, workingCount: 0 });

    await requestCoordinatorClose(s);

    const calls = fake.callsFor("close_coordinator");
    expect(calls).toHaveLength(1);
    expect(calls[0].args).toEqual({ id: "s-coord", confirmed: false });
    expect(fake.callsFor("destroy_session")).toHaveLength(0);
    expect(pendingCoordinatorClose()).toBeNull();
  });

  it("opens the confirmation modal (named from the session) when the backend reports working members", async () => {
    disposeModalHost = registerCoordinatorCloseModalHost(); // a window with the modal host
    const s = session({ id: "s-coord", name: COORD_NAME, isCoordinator: true });
    sessionsStore.setSessions([s]);
    fake.resolve("close_coordinator", { closed: false, workingCount: 3 });

    await requestCoordinatorClose(s);

    const pending = pendingCoordinatorClose();
    expect(pending).not.toBeNull();
    expect(pending!.sessionId).toBe("s-coord");
    expect(pending!.name).toBe(COORD_NAME);
    expect(pending!.workingCount).toBe(3);
  });

  it("confirm calls close_coordinator(confirmed:true) and clears the modal", async () => {
    setPendingCoordinatorClose({ sessionId: "s-coord", name: "x", workingCount: 2 });
    fake.resolve("close_coordinator", { closed: true, workingCount: 0 });

    await confirmPendingCoordinatorClose();

    const calls = fake.callsFor("close_coordinator");
    expect(calls).toHaveLength(1);
    expect(calls[0].args).toEqual({ id: "s-coord", confirmed: true });
    expect(pendingCoordinatorClose()).toBeNull();
  });

  it("cancel (clearing the pending signal) destroys nothing and invokes no command", async () => {
    setPendingCoordinatorClose({ sessionId: "s-coord", name: "x", workingCount: 2 });
    setPendingCoordinatorClose(null);

    expect(pendingCoordinatorClose()).toBeNull();
    expect(fake.callsFor("close_coordinator")).toHaveLength(0);
    expect(fake.callsFor("destroy_session")).toHaveLength(0);
  });

  it("by-id with an unknown id still routes through close_coordinator and falls back to 'this orchestrator'", async () => {
    disposeModalHost = registerCoordinatorCloseModalHost(); // sidebar/web window
    // No session in the store for this id -> the helper cannot short-circuit to a
    // plain destroy; it lets the backend self-route. The modal name falls back to
    // the generic label (the id-only keyboard-shortcut path).
    fake.resolve("close_coordinator", { closed: false, workingCount: 1 });

    await requestCoordinatorCloseById("ghost-id");

    const calls = fake.callsFor("close_coordinator");
    expect(calls).toHaveLength(1);
    expect(calls[0].args).toEqual({ id: "ghost-id", confirmed: false });
    expect(pendingCoordinatorClose()?.name).toBe("this orchestrator");
  });

  it("falls back to a plain destroy (no modal, no cascade) on closed:false when NO modal host is mounted", async () => {
    // The detached terminal webview: registerShortcuts runs but no ProjectPanel
    // hosts the confirm modal. A busy coordinator close must NOT silently no-op —
    // it falls back to destroying JUST the coordinator.
    const s = session({ id: "s-coord", name: COORD_NAME, isCoordinator: true });
    sessionsStore.setSessions([s]);
    fake.resolve("close_coordinator", { closed: false, workingCount: 2 });
    fake.resolve("destroy_session", undefined);

    await requestCoordinatorCloseById("s-coord");

    // close_coordinator(confirmed:false) was attempted, came back closed:false,
    // then the helper fell back to destroy_session(id) — no pending modal.
    expect(fake.callsFor("close_coordinator")).toEqual([
      { cmd: "close_coordinator", args: { id: "s-coord", confirmed: false } },
    ]);
    expect(fake.callsFor("destroy_session").map((c) => c.args)).toEqual([{ id: "s-coord" }]);
    expect(pendingCoordinatorClose()).toBeNull();
  });

  it("catches a direct non-coordinator destroy failure exactly once", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const s = session({ id: "s-plain", isCoordinator: false });
    fake.reject("destroy_session", "destroy-failed");

    await requestCoordinatorClose(s);

    expect(fake.callsFor("destroy_session")).toHaveLength(1);
    expect(fake.callsFor("close_coordinator")).toHaveLength(0);
    expect(error).toHaveBeenCalledOnce();
    expect(pendingCoordinatorClose()).toBeNull();
  });

  it("catches an initial coordinator close failure without opening a modal or dispatching again", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const s = session({ id: "s-coord", isCoordinator: true });
    sessionsStore.setSessions([s]);
    fake.reject("close_coordinator", "close-failed");

    await requestCoordinatorClose(s);

    expect(fake.callsFor("close_coordinator")).toHaveLength(1);
    expect(fake.callsFor("destroy_session")).toHaveLength(0);
    expect(error).toHaveBeenCalledOnce();
    expect(pendingCoordinatorClose()).toBeNull();
  });

  it("consumes a confirmed cascade before catching its failure", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    setPendingCoordinatorClose({ sessionId: "s-coord", name: "x", workingCount: 2 });
    fake.reject("close_coordinator", "cascade-failed");

    await confirmPendingCoordinatorClose();

    expect(fake.callsFor("close_coordinator")).toHaveLength(1);
    expect(error).toHaveBeenCalledOnce();
    expect(pendingCoordinatorClose()).toBeNull();
  });
});
