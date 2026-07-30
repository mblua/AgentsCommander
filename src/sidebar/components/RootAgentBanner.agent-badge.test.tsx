// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import RootAgentBanner from "./RootAgentBanner";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { sessionsStore } from "../stores/sessions";
import type { SessionStatus } from "../../shared/types";

// #1167 - the ROOT AGENT badge must look exactly like a Coordinator row's badge:
// the same class pair, plus root-agent-badge for inline placement only. jsdom does
// not apply sidebar.css, so the pin is on the emitted classes and attributes.
describe("RootAgentBanner coding-agent badge is style-invariant (#1167)", () => {
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

  const CASES: { name: string; status: SessionStatus; label: string }[] = [
    { name: "live root", status: "running", label: "Codex" },
    { name: "dormant root", status: { exited: 0 }, label: "Codex" },
    { name: "custom label", status: "running", label: "Isolated Claude" },
  ];

  for (const testCase of CASES) {
    it(`renders the same badge markup: ${testCase.name}`, async () => {
      sessionsStore.setSessions([
        session({
          id: "root-1",
          name: "Agent's Commander",
          isRootAgent: true,
          status: testCase.status,
          agentLabel: testCase.label,
        }),
      ]);
      const rendered = renderWithFakeTransport(() => <RootAgentBanner />, new FakeTransport());
      try {
        await waitFor(() =>
          expect(rendered.root.querySelector(".ac-discovery-badge.agent")).not.toBeNull(),
        );
        const el = rendered.root.querySelector<HTMLElement>(".ac-discovery-badge.agent")!;
        expect(el.className).toBe("ac-discovery-badge agent root-agent-badge");
        expect(el.textContent).toBe(testCase.label);
        expect(el.hasAttribute("data-agent")).toBe(false);
        expect(el.classList.contains("running")).toBe(false);
        expect(rendered.root.querySelector(".agent-badge")).toBeNull();
      } finally {
        rendered.cleanup();
      }
    });
  }
});
