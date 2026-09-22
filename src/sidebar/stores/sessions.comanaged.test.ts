// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createComponent } from "solid-js";
import SessionItem from "../components/SessionItem";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
} from "../../shared/testing/ui-harness";
import { settingsStore } from "../../shared/stores/settings";
import { sessionsStore } from "./sessions";

// #2271 phase 8 - the sidecar map and the refresh it must survive. A flag on
// Session would be wiped by projectStoredSelection's wholesale replacement, so
// the executable form of section 4.1 is here and not in a component test.

describe("sessionsStore comanaged sidecar (#2271)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    sessionsStore.resetComanagedForTests();
    resetUiStoresForTests();
  });

  afterEach(() => {
    sessionsStore.resetComanagedForTests();
    resetUiStoresForTests();
    cleanupDom?.();
    cleanupDom = null;
    document.body.replaceChildren();
  });

  it("sets on active:true, clears on active:false with a reason, and deletes the entry on removal (test 14)", () => {
    sessionsStore.setSessions([session({ id: "cm-1" })]);

    sessionsStore.setSessionComanaged("cm-1", true);
    expect(sessionsStore.comanagedBySessionId["cm-1"]).toBe(true);

    // session_comanaged_state { active: false, reason: "abstained" } -> the
    // store receives only `active`; the reason is event metadata.
    sessionsStore.setSessionComanaged("cm-1", false);
    expect("cm-1" in sessionsStore.comanagedBySessionId).toBe(false);

    sessionsStore.setSessionComanaged("cm-1", true);
    sessionsStore.setSessionComanaged("cm-other", true);
    expect(sessionsStore.comanagedBySessionId["cm-1"]).toBe(true);

    sessionsStore.removeSession("cm-1");
    // The removed session's entry is gone, so the event-fed map cannot grow
    // without bound; the neighbouring entry is untouched (no collateral clear).
    expect("cm-1" in sessionsStore.comanagedBySessionId).toBe(false);
    expect(sessionsStore.comanagedBySessionId["cm-other"]).toBe(true);
  });

  it("survives the list refresh and still renders comanaged (test 15)", async () => {
    sessionsStore.setSessions([
      session({ id: "cm-refresh", name: "wg-1-dev-team/architect", status: "running" }),
    ]);
    sessionsStore.setSessionComanaged("cm-refresh", true);

    // The refresh: a fresh set of backend rows replaces state.sessions wholesale,
    // exactly like the polled list. An event-only field on Session dies here.
    sessionsStore.setSessions([
      session({ id: "cm-refresh", name: "wg-1-dev-team/architect", status: "idle" }),
    ]);

    expect(sessionsStore.comanagedBySessionId["cm-refresh"]).toBe(true);
    const refreshed = sessionsStore.sessions.find((s) => s.id === "cm-refresh");
    expect(refreshed?.status).toBe("idle");

    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    const rendered = renderWithFakeTransport(
      // No JSX: this file is .ts (the phase's fixed path), so the component is
      // created through createComponent instead of a .tsx literal.
      () => createComponent(SessionItem, { session: refreshed!, isActive: false }),
      fake,
    );
    try {
      await settingsStore.load();
      const dot = rendered.root.querySelector(".session-item-status");
      if (!dot) throw new Error("status dot missing after refresh");
      // The refresh changed the runtime state to idle; the row keeps the real
      // idle colour and adds the Co-managed ring (#2408: "idle comanaged").
      expect(dot.classList.contains("comanaged")).toBe(true);
      expect(dot.classList.contains("idle")).toBe(true);
      expect(dot.getAttribute("data-ac-comanaged")).toBe("true");
    } finally {
      rendered.cleanup();
    }
  });
});
