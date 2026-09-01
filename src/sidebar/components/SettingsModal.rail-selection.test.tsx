// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SettingsModal from "./SettingsModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import type { AgentConfig } from "../../shared/types";

// #895 — the configured coding-agent list doubles as the rail picker. Use calls
// the same selection closure: row 0 targets the left/primary rail, later rows
// target the right/comparison rail, and opposite-rail occupants exchange places.

function agent(id: string, label: string, command: string): AgentConfig {
  return { id, label, command, color: "#334155", envs: [], isolatedHome: false };
}

const AGENTS = [
  agent("codex", "Codex", "codex"),
  agent("claude", "Claude Code", "claude"),
  agent("opencode", "OpenCode", "opencode"),
];

function renderAgents(agents: AgentConfig[] = AGENTS) {
  const fake = new FakeTransport();
  fake.resolve("get_settings", baseSettings({ agents }));
  fake.resolve("get_web_server_status", false);
  fake.resolve("get_coding_agent_catalog", []);
  fake.resolve("list_reseedable_agent_commands", []);
  return renderWithFakeTransport(() => <SettingsModal section="agents" onClose={() => {}} />, fake);
}

function byTestId<T extends Element = Element>(root: HTMLElement, testId: string): T | null {
  return root.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function click(root: HTMLElement, testId: string): void {
  const el = byTestId<HTMLElement>(root, testId);
  if (!el) throw new Error(`missing selector ${testId}`);
  el.click();
}

/** The agent pinned to a rail, or null when the rail is empty. */
function railAgent(root: HTMLElement, railIndex: 0 | 1): string | null {
  return byTestId(root, `settings.profileRail.${railIndex}`)?.getAttribute("data-ac-agent-id") ?? null;
}

/** Both rails at once, so a swap reads as one assertion. */
function rails(root: HTMLElement): [string | null, string | null] {
  return [railAgent(root, 0), railAgent(root, 1)];
}

function pills(root: HTMLElement): (string | null)[] {
  return [...root.querySelectorAll('[data-ac-testid^="settings.agentRow."][data-ac-rail]')].map((el) =>
    el.getAttribute("data-ac-rail"),
  );
}

function head(root: HTMLElement, i: number): HTMLElement {
  const el = byTestId<HTMLElement>(root, `settings.agentRow.${i}.select`);
  if (!el) throw new Error(`missing head ${i}`);
  return el;
}

async function ready(root: HTMLElement): Promise<void> {
  await waitFor(() => expect(byTestId(root, "settings.agentRow.0.select")).toBeTruthy());
}

/** Pin the left rail away from row 0 via the rail's own dropdown. */
async function pinLeft(root: HTMLElement, agentId: string): Promise<void> {
  const select = byTestId<HTMLSelectElement>(root, "settings.profileRail.0.agentSelect")!;
  select.value = agentId;
  select.dispatchEvent(new Event("change", { bubbles: true }));
  await waitFor(() => expect(railAgent(root, 0)).toBe(agentId));
}

/** Reveal the second comparison rail via the top-right dropdown. */
async function enterTwoRails(root: HTMLElement): Promise<void> {
  const sel = byTestId<HTMLSelectElement>(root, "settings.profiles.railCount")!;
  sel.value = "2";
  sel.dispatchEvent(new Event("change", { bubbles: true }));
  await waitFor(() => expect(railAgent(root, 1)).not.toBeNull());
}

function railCountAttr(root: HTMLElement): string | null {
  return byTestId(root, "settings.profiles.section")?.getAttribute("data-ac-rail-count") ?? null;
}

describe("SettingsModal coding-agent rail selection (#895)", () => {
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

  it("renders a complete Use contract on every configured agent row", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      await enterTwoRails(r.root);

      const titles = [
        "Already on the left comparison rail",
        "Already on the right comparison rail",
        "Show this agent's configuration in the right comparison rail",
      ];
      for (const [i, configuredAgent] of AGENTS.entries()) {
        const use = byTestId<HTMLButtonElement>(r.root, `settings.agentRow.${i}.use`)!;
        const title = titles[i]!;
        const actions = [
          ...r.root.querySelectorAll(`[data-ac-testid="settings.agentRow.${i}"] .settings-agent-row-actions button`),
        ].map((el) => el.getAttribute("data-ac-testid"));

        expect(use.textContent).toBe("Configuration");
        expect(use.getAttribute("data-ac-role")).toBe("button");
        expect(use.getAttribute("title")).toBe(title);
        expect(use.getAttribute("aria-label")).toBe(`${configuredAgent.label}: ${title}`);
        expect(actions).toEqual([
          `settings.agentRow.${i}.use`,
          `settings.agentRow.${i}.remove`,
          `settings.agentRow.${i}.toggle`,
        ]);

        if (i < 2) {
          expect(use.disabled).toBe(true);
          expect(use.matches(":disabled")).toBe(true);
        } else {
          expect(use.disabled).toBe(false);
          expect(use.matches(":disabled")).toBe(false);
        }
      }
      expect(r.root.querySelector('[data-ac-testid$=".unuse"]')).toBeNull();
      expect(byTestId(r.root, "settings.profileRail.1.clear")).toBeTruthy();
      expect(byTestId(r.root, "settings.profileRail.0.clear")).toBeNull();

      click(r.root, "settings.agents.swapRails");
      await waitFor(() => expect(rails(r.root)).toEqual(["claude", "codex"]));
      for (const [i, title] of [
        "Swap this agent onto the left comparison rail",
        "Swap this agent onto the right comparison rail",
      ].entries()) {
        const use = byTestId<HTMLButtonElement>(r.root, `settings.agentRow.${i}.use`)!;
        expect(use.getAttribute("title")).toBe(title);
        expect(use.getAttribute("aria-label")).toBe(`${AGENTS[i]!.label}: ${title}`);
        expect(use.disabled).toBe(false);
      }
    } finally {
      r.cleanup();
    }
  });

  for (const target of AGENTS) {
    it(`uses ${target.label} in one-rail view and clears a stale right id`, async () => {
      const r = renderAgents();
      try {
        await ready(r.root);
        await enterTwoRails(r.root);
        const staleRightId = target.id === "codex" ? "claude" : "codex";
        if (staleRightId === "codex") {
          click(r.root, "settings.agents.swapRails");
          await waitFor(() => expect(rails(r.root)).toEqual(["claude", "codex"]));
        }
        await pinLeft(r.root, staleRightId);
        expect(railCountAttr(r.root)).toBe("1");
        expect(rails(r.root)).toEqual([staleRightId, null]);

        const use = byTestId<HTMLButtonElement>(
          r.root,
          `settings.agentRow.${AGENTS.indexOf(target)}.use`,
        )!;
        expect(use.disabled).toBe(false);
        use.click();
        await waitFor(() => expect(rails(r.root)).toEqual([target.id, null]));
        expect(railCountAttr(r.root)).toBe("1");
        expect(byTestId(r.root, "settings.profileRail.1")).toBeNull();
      } finally {
        r.cleanup();
      }
    });
  }

  for (const [index, target] of AGENTS.entries()) {
    it(`uses ${target.label} in two-rail view to target the ${index === 0 ? "left" : "right"} rail`, async () => {
      const r = renderAgents();
      try {
        await ready(r.root);
        await enterTwoRails(r.root);
        if (index < 2) {
          // Row 0 holds right and row 1 holds left, proving both swap directions.
          click(r.root, "settings.agents.swapRails");
          await waitFor(() => expect(rails(r.root)).toEqual(["claude", "codex"]));
        }

        const use = byTestId<HTMLButtonElement>(r.root, `settings.agentRow.${index}.use`)!;
        expect(use.disabled).toBe(false);
        use.click();
        await waitFor(() =>
          expect(rails(r.root)).toEqual(index === 2 ? ["codex", "opencode"] : ["codex", "claude"]),
        );
      } finally {
        r.cleanup();
      }
    });
  }

  it("keeps enabled and disabled Use clicks out of the row header", async () => {
    const r = renderAgents();
    let observer: MutationObserver | null = null;
    let enabledHeader: HTMLElement | null = null;
    let disabledHeader: HTMLElement | null = null;
    let observeHeader: ((event: MouseEvent) => void) | null = null;
    let observeDocument: ((event: MouseEvent) => void) | null = null;
    try {
      await ready(r.root);
      await enterTwoRails(r.root);
      enabledHeader = head(r.root, 2);
      disabledHeader = head(r.root, 0);
      let headerClickCount = 0;
      const observedHeaderEvents = new WeakSet<MouseEvent>();
      observeHeader = (event) => {
        // Solid intentionally skips delegated handlers for disabled native
        // controls, even though jsdom dispatches this synthetic event to raw
        // native listeners. It cannot activate the selectable row header.
        if (event.target instanceof HTMLButtonElement && event.target.disabled) return;
        observedHeaderEvents.add(event);
      };
      observeDocument = (event) => {
        // Solid delegates JSX click handlers from document. This observer runs
        // after that delegate, while the event still exposes cancelBubble.
        if (observedHeaderEvents.has(event) && !event.cancelBubble) headerClickCount += 1;
      };
      enabledHeader.addEventListener("click", observeHeader);
      disabledHeader.addEventListener("click", observeHeader);
      document.addEventListener("click", observeDocument);

      let previousRails = rails(r.root);
      const railTransitions: [string | null, string | null][] = [];
      observer = new MutationObserver(() => {
        const nextRails = rails(r.root);
        if (nextRails[0] !== previousRails[0] || nextRails[1] !== previousRails[1]) {
          railTransitions.push(nextRails);
          previousRails = nextRails;
        }
      });
      observer.observe(r.root, { attributes: true, childList: true, subtree: true });

      const enabledUse = byTestId<HTMLButtonElement>(r.root, "settings.agentRow.2.use")!;
      enabledUse.click();
      await waitFor(() => expect(rails(r.root)).toEqual(["codex", "opencode"]));
      await waitFor(() => expect(railTransitions).toEqual([["codex", "opencode"]]));
      expect(headerClickCount).toBe(0);

      const disabledUse = byTestId<HTMLButtonElement>(r.root, "settings.agentRow.0.use")!;
      expect(disabledUse.disabled).toBe(true);
      disabledUse.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      expect(headerClickCount).toBe(0);
      expect(rails(r.root)).toEqual(["codex", "opencode"]);
      expect(railTransitions).toEqual([["codex", "opencode"]]);
    } finally {
      observer?.disconnect();
      if (enabledHeader && observeHeader) enabledHeader.removeEventListener("click", observeHeader);
      if (disabledHeader && observeHeader) disabledHeader.removeEventListener("click", observeHeader);
      if (observeDocument) document.removeEventListener("click", observeDocument);
      r.cleanup();
    }
  });

  it("keeps Use ordered, focusable, and isolated from row-header keyboard selection", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      await enterTwoRails(r.root);
      const rowHeader = head(r.root, 2);
      const use = byTestId<HTMLButtonElement>(r.root, "settings.agentRow.2.use")!;
      const remove = byTestId<HTMLButtonElement>(r.root, "settings.agentRow.2.remove")!;
      const toggle = byTestId<HTMLButtonElement>(r.root, "settings.agentRow.2.toggle")!;
      const actions = [
        ...rowHeader.querySelectorAll(".settings-agent-row-actions > button"),
      ];

      expect(rowHeader.contains(use)).toBe(true);
      expect(actions).toEqual([use, remove, toggle]);
      use.focus();
      expect(document.activeElement).toBe(use);

      use.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Enter" }));
      use.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: " " }));
      expect(rails(r.root)).toEqual(["codex", "claude"]);

      use.click();
      await waitFor(() => expect(rails(r.root)).toEqual(["codex", "opencode"]));
    } finally {
      r.cleanup();
    }
  });

  it("assigns the left rail when the first row is clicked", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      await pinLeft(r.root, "opencode");

      click(r.root, "settings.agentRow.0.select");
      await waitFor(() => expect(rails(r.root)).toEqual(["codex", null]));
      expect(byTestId(r.root, "settings.agentRow.0")?.getAttribute("data-ac-rail")).toBe("left");
    } finally {
      r.cleanup();
    }
  });

  it("assigns the right rail when a row after the first is clicked", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      await enterTwoRails(r.root);
      // 2-rail view: left=codex (agents[0]), right=claude (agents[1]). Assigning
      // a later row to the right rail is now a 2-rail-only behavior (#1098).
      expect(rails(r.root)).toEqual(["codex", "claude"]);

      click(r.root, "settings.agentRow.2.select");
      await waitFor(() => expect(rails(r.root)).toEqual(["codex", "opencode"]));
      expect(pills(r.root)).toEqual(["left", "available", "right"]);
    } finally {
      r.cleanup();
    }
  });

  // ── F1: a row at index >= 1 that holds the LEFT rail must swap, not no-op ──

  it("swaps rather than no-oping when a row after the first holds the left rail", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      await enterTwoRails(r.root);
      click(r.root, "settings.agents.swapRails");
      await waitFor(() => expect(rails(r.root)).toEqual(["claude", "codex"]));
      expect(pills(r.root)).toEqual(["right", "left", "available"]);

      // Row 1 (claude) now carries the LEFT pill. Clicking it targets the right
      // rail, so the rails exchange places instead of the click doing nothing.
      click(r.root, "settings.agentRow.1.select");
      await waitFor(() => expect(rails(r.root)).toEqual(["codex", "claude"]));
      expect(pills(r.root)).toEqual(["left", "right", "available"]);
    } finally {
      r.cleanup();
    }
  });

  it("keeps the single left rail when the left-rail's own row is clicked in 1-rail view", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      await pinLeft(r.root, "opencode"); // left=opencode, second rail hidden (1 rail)
      expect(rails(r.root)).toEqual(["opencode", null]);

      // Row 2 (opencode) already holds the single rail; clicking it stays a no-op
      // and must never populate a second rail (#1098).
      click(r.root, "settings.agentRow.2.select");
      await waitFor(() => expect(railCountAttr(r.root)).toBe("1"));
      expect(rails(r.root)).toEqual(["opencode", null]);
      expect(byTestId(r.root, "settings.profileRail.1")).toBeNull();
    } finally {
      r.cleanup();
    }
  });

  // ── F2: row 0 holding the RIGHT rail must swap, not stomp the pair ──

  it("swaps rather than clearing the comparison when the first row holds the right rail", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      await enterTwoRails(r.root);
      click(r.root, "settings.agents.swapRails");
      await waitFor(() => expect(rails(r.root)).toEqual(["claude", "codex"]));

      // Row 0 (codex) now carries the RIGHT pill. Clicking it targets the left
      // rail; claude is displaced onto the right rather than dropped.
      click(r.root, "settings.agentRow.0.select");
      await waitFor(() => expect(rails(r.root)).toEqual(["codex", "claude"]));
      expect(pills(r.root)).toEqual(["left", "right", "available"]);
    } finally {
      r.cleanup();
    }
  });

  it("leaves Swap Rails working after the first row reclaims the left rail", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      await enterTwoRails(r.root);
      click(r.root, "settings.agents.swapRails");
      await waitFor(() => expect(rails(r.root)).toEqual(["claude", "codex"]));
      click(r.root, "settings.agentRow.0.select");
      await waitFor(() => expect(rails(r.root)).toEqual(["codex", "claude"]));

      // A stale rightRailId (=== leftRailId) would wedge Swap Rails: it derives
      // the right rail as null and early-returns while still rendering enabled.
      expect(byTestId(r.root, "settings.agents.swapRails")?.getAttribute("data-ac-state")).toBe("enabled");
      click(r.root, "settings.agents.swapRails");
      await waitFor(() => expect(rails(r.root)).toEqual(["claude", "codex"]));
    } finally {
      r.cleanup();
    }
  });

  // ── Honest affordances: no row advertises an action it will not perform ──

  it("marks a row inert only when it already holds the rail its click targets", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      await enterTwoRails(r.root);
      // Resting: row 0 is the left rail, row 1 is the right rail. Clicking either
      // does nothing, and both say so.
      expect(head(r.root, 0).getAttribute("aria-disabled")).toBe("true");
      expect(head(r.root, 0).getAttribute("title")).toBe("Already on the left comparison rail");
      expect(head(r.root, 1).getAttribute("aria-disabled")).toBe("true");
      expect(head(r.root, 1).getAttribute("title")).toBe("Already on the right comparison rail");

      expect(head(r.root, 2).getAttribute("aria-disabled")).toBe("false");
      expect(head(r.root, 2).getAttribute("title")).toBe("Show this agent's configuration in the right comparison rail");

      // After a swap both rows become live, and each announces a swap.
      click(r.root, "settings.agents.swapRails");
      await waitFor(() => expect(rails(r.root)).toEqual(["claude", "codex"]));
      expect(head(r.root, 0).getAttribute("aria-disabled")).toBe("false");
      expect(head(r.root, 0).getAttribute("title")).toBe("Swap this agent onto the left comparison rail");
      expect(head(r.root, 1).getAttribute("aria-disabled")).toBe("false");
      expect(head(r.root, 1).getAttribute("title")).toBe("Swap this agent onto the right comparison rail");
    } finally {
      r.cleanup();
    }
  });

  // ── F5: the clear control is icon-only, so it must carry its own name ──

  it("names the icon-only clear control and still empties the rail", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      await enterTwoRails(r.root);
      const clear = byTestId<HTMLButtonElement>(r.root, "settings.profileRail.1.clear")!;

      // No text node names this button, so the aria-label is the only accessible
      // name it has. The glyph itself must stay out of the accessibility tree.
      expect(clear.textContent?.trim()).toBe("");
      expect(clear.getAttribute("aria-label")).toBe("Clear the comparison rail");
      expect(clear.getAttribute("title")).toBeTruthy();
      expect(clear.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");

      clear.click();
      await waitFor(() => expect(rails(r.root)).toEqual(["codex", null]));
    } finally {
      r.cleanup();
    }
  });

  // ── Delete and expand must never touch the rails ──

  it("deletes the agent from the trash button without assigning a rail", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      expect(rails(r.root)).toEqual(["codex", null]);

      // Row 2 (opencode) is 'available'. Its delete must not leak into the head
      // click that would otherwise move it onto the right rail.
      click(r.root, "settings.agentRow.2.remove");
      await waitFor(() => expect(byTestId(r.root, "settings.agentRow.2")).toBeNull());
      expect(rails(r.root)).toEqual(["codex", null]);
    } finally {
      r.cleanup();
    }
  });

  it("does not swap rails when the trash of a rail-holding row is clicked", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      await enterTwoRails(r.root);
      click(r.root, "settings.agents.swapRails");
      await waitFor(() => expect(rails(r.root)).toEqual(["claude", "codex"]));

      // Row 1 holds the LEFT rail, so a leaked head click would swap. Deleting
      // row 2 (untouched by either rail) must leave the pair exactly as it was.
      click(r.root, "settings.agentRow.2.remove");
      await waitFor(() => expect(byTestId(r.root, "settings.agentRow.2")).toBeNull());
      expect(rails(r.root)).toEqual(["claude", "codex"]);
    } finally {
      r.cleanup();
    }
  });

  it("expands the editor from the chevron without assigning a rail", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      expect(byTestId(r.root, "settings.agentRow.2.editor")).toBeNull();

      click(r.root, "settings.agentRow.2.toggle");
      await waitFor(() => expect(byTestId(r.root, "settings.agentRow.2.editor")).toBeTruthy());
      expect(rails(r.root)).toEqual(["codex", null]);

      click(r.root, "settings.agentRow.2.toggle");
      await waitFor(() => expect(byTestId(r.root, "settings.agentRow.2.editor")).toBeNull());
      expect(rails(r.root)).toEqual(["codex", null]);
    } finally {
      r.cleanup();
    }
  });

  // ── #1095: 1/2-rail layout toggle ──

  it("defaults to a single rail with the second rail hidden", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      expect(rails(r.root)).toEqual(["codex", null]);
      expect(byTestId(r.root, "settings.profileRail.1")).toBeNull();
      expect(railCountAttr(r.root)).toBe("1");
      expect(byTestId<HTMLSelectElement>(r.root, "settings.profiles.railCount")!.value).toBe("1");
    } finally {
      r.cleanup();
    }
  });

  it("dropdown reveals then hides the second rail", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      await enterTwoRails(r.root);
      expect(rails(r.root)).toEqual(["codex", "claude"]);
      expect(railCountAttr(r.root)).toBe("2");
      expect(byTestId(r.root, "settings.profileRail.1")).toBeTruthy();

      const sel = byTestId<HTMLSelectElement>(r.root, "settings.profiles.railCount")!;
      sel.value = "1";
      sel.dispatchEvent(new Event("change", { bubbles: true }));
      await waitFor(() => expect(rails(r.root)).toEqual(["codex", null]));
      expect(byTestId(r.root, "settings.profileRail.1")).toBeNull();
      expect(railCountAttr(r.root)).toBe("1");
    } finally {
      r.cleanup();
    }
  });

  it("right-rail Clear collapses back to one rail", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      await enterTwoRails(r.root);
      click(r.root, "settings.profileRail.1.clear");
      await waitFor(() => expect(byTestId(r.root, "settings.profileRail.1")).toBeNull());
      expect(railCountAttr(r.root)).toBe("1");
      expect(byTestId<HTMLSelectElement>(r.root, "settings.profiles.railCount")!.value).toBe("1");
    } finally {
      r.cleanup();
    }
  });

  it("disables the rail-count dropdown when fewer than two agents", async () => {
    const r = renderAgents([agent("codex", "Codex", "codex")]);
    try {
      await ready(r.root);
      expect(byTestId<HTMLSelectElement>(r.root, "settings.profiles.railCount")!.disabled).toBe(true);
      expect(byTestId(r.root, "settings.profileRail.1")).toBeNull();
    } finally {
      r.cleanup();
    }
  });

  it("clicking a comparison agent row in 1-rail view loads it into the single left rail", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      // Default 1-rail: left=codex, second rail hidden.
      click(r.root, "settings.agentRow.2.select");
      await waitFor(() => expect(rails(r.root)).toEqual(["opencode", null]));
      // No auto-switch: still one rail, second rail absent, dropdown still "1".
      expect(railCountAttr(r.root)).toBe("1");
      expect(byTestId(r.root, "settings.profileRail.1")).toBeNull();
      expect(byTestId<HTMLSelectElement>(r.root, "settings.profiles.railCount")!.value).toBe("1");
      // The clicked row now carries the left pill.
      expect(byTestId(r.root, "settings.agentRow.2")?.getAttribute("data-ac-rail")).toBe("left");
    } finally {
      r.cleanup();
    }
  });

  // ── #1098: repeated comparison-row clicks stay in the single left rail ──

  it("keeps loading the single left rail across repeated comparison-row clicks", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      click(r.root, "settings.agentRow.1.select"); // claude
      await waitFor(() => expect(rails(r.root)).toEqual(["claude", null]));
      expect(railCountAttr(r.root)).toBe("1");

      click(r.root, "settings.agentRow.2.select"); // opencode
      await waitFor(() => expect(rails(r.root)).toEqual(["opencode", null]));
      expect(railCountAttr(r.root)).toBe("1");
      expect(byTestId(r.root, "settings.profileRail.1")).toBeNull();
    } finally {
      r.cleanup();
    }
  });

  it("does not resurrect the second rail from a stale comparison id after collapsing to 1 rail", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      await enterTwoRails(r.root);          // left=codex, right=claude (2 rails)
      // Collapse to 1 rail via the LEFT per-rail select pointing at the right
      // agent: rightRailId stays "claude" (stale) while the view is 1-rail.
      await pinLeft(r.root, "claude");
      expect(railCountAttr(r.root)).toBe("1");
      expect(byTestId(r.root, "settings.profileRail.1")).toBeNull();

      // A comparison-row click loads the left rail AND clears the stale id; the
      // second rail must not re-reveal.
      click(r.root, "settings.agentRow.2.select"); // opencode
      await waitFor(() => expect(rails(r.root)).toEqual(["opencode", null]));
      expect(railCountAttr(r.root)).toBe("1");
      expect(byTestId(r.root, "settings.profileRail.1")).toBeNull();
    } finally {
      r.cleanup();
    }
  });
});
