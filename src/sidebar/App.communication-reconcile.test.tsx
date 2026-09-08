// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SidebarApp from "./App";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
} from "../shared/testing/ui-harness";
import { initialSelection, SESSION_A } from "../shared/testing/session-selection";
import { sessionsStore } from "./stores/sessions";
import { isSessionWorking } from "../shared/session-activity";
import type { Session, SessionCommunication } from "../shared/types";

// #1856: the sidebar's view of session.communication is fed ONLY by the
// session_communication_changed event, so any missed edge (a backend clobber, a
// dropped event, a transport reconnect, a reload) latches a wrong notice forever
// because the backend does not republish while the screen is quiet. The five
// second profile-drift poll already fetches a listing that carries
// communication; these tests pin that the poll now reconciles from it.
//
// The clock is fully fake, with NO shouldAdvanceTime: the in-flight test below
// asserts an ORDER (the event lands, THEN the listing resolves), and a clock
// that also tracks real time lets the listing resolve first, which passes that
// test for free even with the guard deleted. That is also why the harness
// `waitFor` is not used here: it polls on Date.now, which a fake clock only
// moves when a test advances it.

const projectPath = "C:\\Project";
const agentPath = `${projectPath}\\.ac\\_agent_General`;
const updatedAt = "2026-09-07T06:00:00.000Z";

// Long enough for the mount's async wiring to settle, short of the 5s interval.
const MOUNT_MS = 500;
// The interval is 5000ms and handleWindowFocusDriftRefresh debounces by 250ms.
const POLL_MS = 5000 + 250 + 50;

function blockedMenu(message: string): SessionCommunication {
  return { kind: "blockedMenu", visible: true, updatedAt, message };
}

/** Deliberately NOT working: `isSessionWorking` must be false so the reconcile
 *  is proven to happen ABOVE the `continue` in refreshProfileOutdated. */
function idleSession(overrides: Partial<Session> = {}): Session {
  return session({
    id: SESSION_A,
    name: "General",
    workingDirectory: agentPath,
    status: "idle",
    agentId: "codex",
    agentLabel: "Codex",
    ...overrides,
  });
}

/** `communication` is `skip_serializing_if = "Option::is_none"` on the Rust side,
 *  so a session with no communication arrives with the key ABSENT. `session()`
 *  always writes the key, so the absent-key case has to be built by removing it. */
function withoutCommunicationKey(s: Session): Session {
  const clone: Session = { ...s };
  delete clone.communication;
  return clone;
}

function storedCommunication(id: string): SessionCommunication | null | undefined {
  return sessionsStore.sessions.find((r) => r.id === id)?.communication;
}

describe("SidebarApp communication reconcile from the polled listing (#1856)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;
  // Every call BUILDS a fresh session AND a fresh communication object. Solid's
  // setState MERGES an object written over an object, so the notice the store
  // holds is mutated in place; a fixture that reused one instance would hand the
  // poll back whatever the store just wrote, and tests 3 and 4 would pass for
  // free. Notices are compared by value against a freshly built expectation.
  let listing: () => Session[] = () => [];

  beforeEach(() => {
    vi.useFakeTimers();
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    listing = () => [];
  });

  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    vi.useRealTimers();
  });

  async function mount(
    listSessions: () => Session[] | Promise<Session[]> = () => listing(),
  ): Promise<FakeTransport> {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings({ projectPaths: [projectPath], projectPath }));
    fake.resolve("open_project", { path: projectPath, registered: true, created: false });
    fake.resolve(
      "discover_project",
      discovery({
        agents: [{ name: "General", path: agentPath, roleExists: true }],
        teams: [],
        workgroups: [],
      }),
    );
    fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
    fake.resolve("search_repos", []);
    fake.onInvoke("list_sessions", () => listSessions());
    fake.resolve("get_active_session", initialSelection());
    fake.resolve("list_detached_sessions", []);
    fake.resolve("telegram_list_bridges", []);
    fake.resolve("get_update_status", null);

    rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    await vi.advanceTimersByTimeAsync(MOUNT_MS);
    expect(fake.callsFor("list_sessions").length).toBe(1);
    return fake;
  }

  it("recovers a notice the store lost, from the next listing", async () => {
    const message = "Choose an option in the interactive menu";
    listing = () => [idleSession({ communication: blockedMenu(message) })];
    const fake = await mount();
    expect(storedCommunication(SESSION_A)).toEqual(blockedMenu(message));

    // Simulate the lost edge: the store is wrong, the backend listing is right.
    sessionsStore.setCommunication(SESSION_A, null);
    expect(storedCommunication(SESSION_A)).toBeNull();

    await vi.advanceTimersByTimeAsync(POLL_MS);
    expect(fake.callsFor("list_sessions").length).toBe(2);

    expect(storedCommunication(SESSION_A)).toEqual(blockedMenu(message));
  });

  it("clears a stale notice when the listing omits the communication key", async () => {
    const message = "Stale menu prompt";
    listing = () => [idleSession({ communication: blockedMenu(message) })];
    await mount();
    expect(storedCommunication(SESSION_A)).toEqual(blockedMenu(message));

    // The key is ABSENT, not null: that is what the backend actually sends.
    expect("communication" in withoutCommunicationKey(idleSession())).toBe(false);
    listing = () => [withoutCommunicationKey(idleSession())];

    await vi.advanceTimersByTimeAsync(POLL_MS);

    expect(storedCommunication(SESSION_A)).toBeNull();
  });

  it("reconciles a session that is not working", async () => {
    const message = "Menu on an idle session";
    // The regression pin for the placement rule: the new write must sit ABOVE
    // the `continue` that skips every session which is not working.
    expect(isSessionWorking(idleSession({ communication: blockedMenu(message) }))).toBe(false);

    listing = () => [idleSession({ communication: blockedMenu(message) })];
    await mount();
    expect(storedCommunication(SESSION_A)).toEqual(blockedMenu(message));

    sessionsStore.setCommunication(SESSION_A, null);
    await vi.advanceTimersByTimeAsync(POLL_MS);

    expect(storedCommunication(SESSION_A)).toEqual(blockedMenu(message));
  });

  it("lets an event that lands while the listing is in flight win", async () => {
    const listedMessage = "From the listing";
    const eventMessage = "From the event";
    listing = () => [idleSession({ communication: blockedMenu(listedMessage) })];

    // The gated listing parks on a timer, so the exact moment it resolves is
    // driven by the same fake clock the assertions below advance.
    const gate = { on: false };
    const IN_FLIGHT_MS = 1000;
    const fake = await mount(() =>
      gate.on
        ? new Promise<Session[]>((resolveList) => {
            setTimeout(() => resolveList(listing()), IN_FLIGHT_MS);
          })
        : listing(),
    );
    expect(storedCommunication(SESSION_A)).toEqual(blockedMenu(listedMessage));

    gate.on = true;
    await vi.advanceTimersByTimeAsync(POLL_MS);
    // The poll has asked for the listing and is provably still waiting on it.
    expect(fake.callsFor("list_sessions").length).toBe(2);

    // The edge lands while the listing is in flight; it must not be overwritten.
    sessionsStore.setCommunication(SESSION_A, blockedMenu(eventMessage));
    await vi.advanceTimersByTimeAsync(IN_FLIGHT_MS + 50);

    expect(storedCommunication(SESSION_A)).toEqual(blockedMenu(eventMessage));
  });

  it("completes a poll over an empty listing without touching any store entry", async () => {
    const message = "Untouched";
    listing = () => [idleSession({ communication: blockedMenu(message) })];
    const fake = await mount();
    expect(storedCommunication(SESSION_A)).toEqual(blockedMenu(message));

    listing = () => [];
    const generationBefore = sessionsStore.communicationGeneration;
    await vi.advanceTimersByTimeAsync(POLL_MS);
    expect(fake.callsFor("list_sessions").length).toBe(2);

    expect(sessionsStore.communicationGeneration).toBe(generationBefore);
    expect(storedCommunication(SESSION_A)).toEqual(blockedMenu(message));
  });
});
