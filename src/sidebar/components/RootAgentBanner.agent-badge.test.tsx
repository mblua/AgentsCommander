// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const acquire = vi.hoisted(() => vi.fn());
vi.mock("../context-history", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../context-history")>();
  return {
    ...actual,
    acquireContextHistory: (...args: Parameters<typeof actual.acquireContextHistory>) => {
      acquire(...args);
      return actual.acquireContextHistory(...args);
    },
  };
});

import RootAgentBanner from "./RootAgentBanner";
import { CONTEXT_BADGE_TOOLTIP } from "./ContextBadge";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { sessionsStore } from "../stores/sessions";
import { settingsStore } from "../../shared/stores/settings";
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

// #3532 P11 - the Root chip never carries a context history: no scope, no card.
describe("RootAgentBanner CTX badge stays without history (#3532 P11)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    acquire.mockClear();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("keeps the native title and never acquires a history", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings({
      agents: [{
        id: "claude", label: "Claude Code", command: "claude", color: "#d97757",
        envs: [], isolatedHome: false, contextRegex: String.raw`(\d+)%`,
      }],
    }));
    sessionsStore.setSessions([
      session({
        id: "00000000-0000-4000-8000-000000000009",
        name: "Agent's Commander",
        isRootAgent: true,
        status: "running",
        agentId: "claude",
      }),
    ]);
    const rendered = renderWithFakeTransport(() => <RootAgentBanner />, fake);
    try {
      await settingsStore.load();
      const sel = '[data-ac-testid="rootAgent.contextBadge"]';
      await waitFor(() => expect(rendered.root.querySelector(sel)).not.toBeNull());
      const badge = rendered.root.querySelector<HTMLElement>(sel)!;
      expect(badge.getAttribute("title")).toBe(CONTEXT_BADGE_TOOLTIP);
      expect(badge.hasAttribute("tabindex")).toBe(false);
      expect(badge.hasAttribute("aria-describedby")).toBe(false);
      expect(badge.classList.contains("ctx-history")).toBe(false);
      expect(acquire).not.toHaveBeenCalled();
      expect(fake.callsFor("get_context_history_sidebar_means")).toEqual([]);
    } finally {
      rendered.cleanup();
    }
  });
});
