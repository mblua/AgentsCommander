// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { createSignal, type Accessor } from "solid-js";
import { render } from "solid-js/web";
import type { SidebarHistoryState } from "../context-history";

const history = vi.hoisted(() => ({
  state: (() => ({ status: "unavailable" })) as () => unknown,
  acquire: vi.fn(),
  release: vi.fn(),
  refresh: vi.fn(),
}));
vi.mock("../context-history", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../context-history")>();
  history.acquire.mockImplementation(() => ({
    state: () => history.state(),
    release: history.release,
  }));
  return {
    ...actual,
    acquireContextHistory: history.acquire,
    refreshContextHistoryOnHover: history.refresh,
  };
});

import ContextBadge, { CLOSE_DELAY_MS, CONTEXT_BADGE_TOOLTIP, rampColor } from "./ContextBadge";

const TESTID = "session.s1.contextBadge";

function target<T extends HTMLElement = HTMLElement>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing test target: ${testId}`);
  return element;
}

function renderBadge(percent: number | null | undefined) {
  const root = document.createElement("div");
  document.body.append(root);
  const dispose = render(() => <ContextBadge percent={percent} testId={TESTID} />, root);
  return { dispose };
}

describe("ContextBadge (#1033)", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("renders a reading as a meter carrying its value (a_reading_is_a_meter_with_a_value)", () => {
    renderBadge(42);

    const badge = target(TESTID);
    expect(badge.getAttribute("role")).toBe("meter");
    expect(badge.getAttribute("aria-valuenow")).toBe("42");
    expect(badge.getAttribute("aria-valuemin")).toBe("0");
    expect(badge.getAttribute("aria-valuemax")).toBe("100");
    expect(badge.getAttribute("aria-valuetext")).toBe("Context 42% used");
    expect(badge.textContent).toBe("CTX 42%");
    expect(badge.getAttribute("data-ac-state")).toBe("reading");
  });

  // A real 0 is a reading: it must be a meter like any other, not the N/A state.
  // Red if anyone writes `<Show when={props.percent}>`, since 0 is falsy.
  it("treats a real zero as a reading, not an absence (zero_is_a_meter_not_unavailable)", () => {
    renderBadge(0);

    const badge = target(TESTID);
    expect(badge.getAttribute("role")).toBe("meter");
    expect(badge.getAttribute("aria-valuenow")).toBe("0");
    expect(badge.textContent).toBe("CTX 0%");
    expect(badge.getAttribute("data-ac-state")).toBe("reading");
  });

  // Pins the ARIA rule: a meter REQUIRES aria-valuenow, and there is no valid
  // aria-valuenow for N/A, so the unavailable state must not be a meter at all.
  it("is not a meter when there is no reading (the_unavailable_state_is_not_a_meter)", () => {
    renderBadge(null);

    const badge = target(TESTID);
    expect(badge.hasAttribute("role")).toBe(false);
    expect(badge.hasAttribute("aria-valuenow")).toBe(false);
    expect(badge.hasAttribute("aria-valuemin")).toBe(false);
    expect(badge.hasAttribute("aria-valuemax")).toBe(false);
    expect(badge.hasAttribute("aria-valuetext")).toBe(false);
    expect(badge.textContent).toBe("CTX N/A");
    expect(badge.getAttribute("data-ac-state")).toBe("unavailable");
  });

  it("renders a missing key exactly like an explicit null (undefined_is_the_same_one_unavailable_state)", () => {
    renderBadge(undefined);

    const badge = target(TESTID);
    expect(badge.hasAttribute("role")).toBe(false);
    expect(badge.textContent).toBe("CTX N/A");
    expect(badge.getAttribute("data-ac-state")).toBe("unavailable");
  });

  // #1031's one hard rule, pinned at the DOM: the badge is a signal, never a control.
  it("is not a control in either state (the_badge_is_not_a_control)", () => {
    for (const percent of [42, null]) {
      document.body.innerHTML = "";
      renderBadge(percent);

      const badge = target(TESTID);
      expect(badge.tagName).toBe("SPAN");
      expect(badge.tagName).not.toBe("BUTTON");
      expect(badge.hasAttribute("onclick")).toBe(false);
      expect(badge.hasAttribute("tabindex")).toBe(false);
      expect(badge.hasAttribute("href")).toBe(false);
      expect(badge.closest("button")).toBeNull();
      expect(badge.closest("a")).toBeNull();
    }
  });

  it("carries the honesty tooltip in both states (both_states_carry_the_tooltip)", () => {
    for (const percent of [42, null]) {
      document.body.innerHTML = "";
      renderBadge(percent);

      expect(target(TESTID).getAttribute("title")).toBe(CONTEXT_BADGE_TOOLTIP);
    }
    // The requirement the tooltip exists to satisfy, not just that a string is set.
    expect(CONTEXT_BADGE_TOOLTIP).toContain("Best-effort");
    expect(CONTEXT_BADGE_TOOLTIP).toContain("unavailable, stale or absent");
    expect(CONTEXT_BADGE_TOOLTIP).toContain("does not");
  });

  // A 5s-cadence live region would interrupt a screen reader every 5 seconds,
  // forever, with a number that drives nothing.
  it("announces nothing on its own (no_live_region)", () => {
    for (const percent of [42, null]) {
      document.body.innerHTML = "";
      renderBadge(percent);

      const badge = target(TESTID);
      expect(badge.hasAttribute("aria-live")).toBe(false);
      expect(badge.hasAttribute("aria-atomic")).toBe(false);
      expect(badge.getAttribute("data-ac-role")).toBe("status");
    }
  });
});

// #3532 P11 - the scoped badge reads the P10 owner and colors only bg/border.
describe("ContextBadge history scope (#3532 P11)", () => {
  const SCOPE = {
    projectPath: "C:/proj",
    runtimeSessionId: "00000000-0000-4000-8000-000000000001",
  };
  const bucket = (requestedSessions: number, mean: number | null, count: number) => ({
    requestedSessions, recordsCount: count, positiveSum: 0, mean,
    sessionsCount: count, recordsPerSession: count === 0 ? null : 1, incomplete: false,
  });
  const readyWith = (sessionWindows: ReturnType<typeof bucket>[]): SidebarHistoryState => ({
    status: "ready",
    scope: { kind: "known", agentKey: "dev", teamIds: ["t1"] },
    generatedAtUtcMs: 1,
    sessionWindows,
  });
  const ready = (last: number | null, middle = 10): SidebarHistoryState =>
    readyWith([
      bucket(25, 90, 1),
      bucket(100, middle, 2),
      bucket(300, last, last === null ? 0 : 4),
    ]);
  const cardOf = (badge: HTMLElement) =>
    document.getElementById(badge.getAttribute("aria-describedby")!)!;

  function renderScoped(
    percent: number | null,
    initial: SidebarHistoryState,
    extra: { colorSessions?: number; scope?: Accessor<typeof SCOPE | undefined> } = {},
  ) {
    history.state = () => initial;
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () => (
        <ContextBadge
          percent={percent}
          testId={TESTID}
          historyScope={extra.scope ? extra.scope() : SCOPE}
          colorSessions={extra.colorSessions}
        />
      ),
      root,
    );
    return { dispose };
  }

  afterEach(() => {
    document.body.innerHTML = "";
    history.acquire.mockClear();
    history.release.mockClear();
    history.refresh.mockClear();
  });

  it("interpolates the ramp per segment on the unrounded mean", () => {
    expect(rampColor(0, "dark")).toBe("#22c55e");
    expect(rampColor(50, "dark")).toBe("#eab308");
    expect(rampColor(100, "dark")).toBe("#ef4444");
    expect(rampColor(0, "light")).toBe("#15803d");
    expect(rampColor(50, "light")).toBe("#8a6400");
    expect(rampColor(100, "light")).toBe("#b91c1c");
    expect(rampColor(25, "dark")).toBe("#86bc33");
    expect(rampColor(50.9, "dark")).not.toBe(rampColor(50, "dark"));
  });

  it("keeps the old markup when the owner has no history (Root/browser)", () => {
    renderScoped(42, { status: "unavailable" });
    const badge = target(TESTID);
    expect(history.acquire).toHaveBeenCalledWith(SCOPE);
    expect(badge.getAttribute("title")).toBe(CONTEXT_BADGE_TOOLTIP);
    expect(badge.hasAttribute("tabindex")).toBe(false);
    expect(badge.classList.contains("ctx-history")).toBe(false);
    expect(document.querySelector("[data-ac-testid='context-history.card']")).toBeNull();
  });

  it("colors from the saved bucket and keeps the live meter value", () => {
    renderScoped(82, ready(55), { colorSessions: 300 });
    const badge = target(TESTID);
    expect(badge.getAttribute("aria-valuenow")).toBe("82");
    expect(badge.textContent).toBe("CTX 82%");
    expect(badge.hasAttribute("title")).toBe(false);
    expect(badge.getAttribute("tabindex")).toBe("0");
    expect(badge.hasAttribute("role") && badge.getAttribute("role")).toBe("meter");
    expect(badge.classList.contains("toned")).toBe(true);
    expect(badge.style.getPropertyValue("--ctx-history-tone-dark")).toBe(rampColor(55, "dark"));
    expect(cardOf(badge).getAttribute("role")).toBe("tooltip");
  });

  it("gives the same color for the same mean whatever the live percent", () => {
    renderScoped(35, ready(55));
    const a = target(TESTID).style.getPropertyValue("--ctx-history-tone-dark");
    document.body.innerHTML = "";
    renderScoped(82, ready(55));
    expect(target(TESTID).style.getPropertyValue("--ctx-history-tone-dark")).toBe(a);
    document.body.innerHTML = "";
    renderScoped(82, ready(20));
    expect(target(TESTID).style.getPropertyValue("--ctx-history-tone-dark")).not.toBe(a);
  });

  it("selects the bucket of the saved session count", () => {
    renderScoped(10, ready(55, 10), { colorSessions: 100 });
    expect(target(TESTID).style.getPropertyValue("--ctx-history-tone-dark"))
      .toBe(rampColor(10, "dark"));
  });

  it("falls back to the last returned bucket for tone and card basis alike", () => {
    for (const colorSessions of [999, undefined]) {
      document.body.innerHTML = "";
      renderScoped(10, ready(55, 10), { colorSessions });
      const badge = target(TESTID);
      expect(badge.style.getPropertyValue("--ctx-history-tone-dark")).toBe(rampColor(55, "dark"));
      expect(cardOf(badge).textContent).toContain("Color basado en: últimas 300 sesiones");
    }
  });

  it("uses a custom [2,5] list and keeps an empty list neutral", () => {
    renderScoped(10, readyWith([bucket(2, 30, 2), bucket(5, 70, 5)]), { colorSessions: 2 });
    const badge = target(TESTID);
    expect(badge.style.getPropertyValue("--ctx-history-tone-dark")).toBe(rampColor(30, "dark"));
    expect(cardOf(badge).querySelectorAll(".ctx-history-row").length).toBe(2);
    document.body.innerHTML = "";
    renderScoped(10, readyWith([]), { colorSessions: 2 });
    expect(target(TESTID).classList.contains("toned")).toBe(false);
  });

  it("stays neutral without positive data, loading, error or ambiguity", () => {
    const states: SidebarHistoryState[] = [
      ready(null),
      { status: "loading" },
      { status: "error", message: "x" },
      {
        status: "ambiguous", scope: { kind: "ambiguous", agentKey: null, teamIds: [] },
        generatedAtUtcMs: 1, text: "",
      },
    ];
    for (const state of states) {
      document.body.innerHTML = "";
      renderScoped(0, state);
      const badge = target(TESTID);
      expect(badge.classList.contains("toned")).toBe(false);
      expect(badge.getAttribute("style") ?? "").not.toContain("--ctx-history-tone");
      expect(badge.getAttribute("aria-valuenow")).toBe("0");
    }
  });

  it("keeps the tint for N/A with history and no synthetic meter value", () => {
    renderScoped(null, ready(55));
    const badge = target(TESTID);
    expect(badge.textContent).toBe("CTX N/A");
    expect(badge.hasAttribute("role")).toBe(false);
    expect(badge.hasAttribute("aria-valuenow")).toBe(false);
    expect(badge.classList.contains("toned")).toBe(true);
    expect(badge.querySelector(".ctx-history-live")).not.toBeNull();
  });

  it("opens on hover/focus, closes on Escape/blur/leave/outside and never bubbles", () => {
    renderScoped(35, ready(55));
    const badge = target(TESTID);
    const bubbled = vi.fn();
    for (const type of ["click", "keydown", "pointerdown"]) {
      badge.parentElement!.addEventListener(type, bubbled);
    }
    expect(cardOf(badge).hidden).toBe(true);

    badge.dispatchEvent(new MouseEvent("mouseenter"));
    expect(cardOf(badge).hidden).toBe(false);
    expect(history.refresh).toHaveBeenCalledWith(SCOPE);
    vi.useFakeTimers();
    badge.dispatchEvent(new MouseEvent("mouseleave", { relatedTarget: document.body }));
    expect(cardOf(badge).hidden).toBe(false);
    vi.advanceTimersByTime(CLOSE_DELAY_MS);
    expect(cardOf(badge).hidden).toBe(true);
    vi.useRealTimers();

    badge.dispatchEvent(new FocusEvent("focus"));
    expect(cardOf(badge).hidden).toBe(false);
    badge.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(cardOf(badge).hidden).toBe(true);
    badge.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    badge.dispatchEvent(new FocusEvent("focus"));
    badge.dispatchEvent(new FocusEvent("blur"));
    expect(cardOf(badge).hidden).toBe(true);

    badge.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    badge.dispatchEvent(new FocusEvent("focus"));
    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    expect(cardOf(badge).hidden).toBe(true);
    expect(bubbled).not.toHaveBeenCalled();
  });

  it("survives the gap between badge and card and closes after leaving both", () => {
    vi.useFakeTimers();
    try {
      renderScoped(35, ready(55));
      const badge = target(TESTID);
      const card = cardOf(badge);
      badge.dispatchEvent(new MouseEvent("mouseenter"));
      // The pointer leaves the badge into the gap: relatedTarget is neither area.
      badge.dispatchEvent(new MouseEvent("mouseleave", { relatedTarget: document.body }));
      vi.advanceTimersByTime(CLOSE_DELAY_MS / 2);
      card.dispatchEvent(new MouseEvent("mouseenter"));
      vi.advanceTimersByTime(CLOSE_DELAY_MS * 4);
      expect(card.hidden).toBe(false);
      // Pressing inside the card blurs the badge without closing it.
      badge.dispatchEvent(new FocusEvent("blur"));
      expect(card.hidden).toBe(false);
      card.dispatchEvent(new MouseEvent("mouseleave", { relatedTarget: document.body }));
      expect(card.hidden).toBe(false);
      vi.advanceTimersByTime(CLOSE_DELAY_MS);
      expect(card.hidden).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it("updates the open card on a ready -> ready change", () => {
    const [state, setState] = createSignal<SidebarHistoryState>(ready(55));
    history.state = state;
    const root = document.createElement("div");
    document.body.append(root);
    render(() => <ContextBadge percent={35} testId={TESTID} historyScope={SCOPE} />, root);
    const badge = target(TESTID);
    badge.dispatchEvent(new MouseEvent("mouseenter"));
    expect(cardOf(badge).textContent).toContain("55.0%");
    setState({
      ...ready(20),
      scope: { kind: "known", agentKey: "other-agent", teamIds: ["t2"] },
    });
    expect(cardOf(badge).textContent).toContain("20.0%");
    expect(cardOf(badge).textContent).toContain("other-agent");
    expect(cardOf(badge).textContent).not.toContain("55.0%");
    expect(badge.style.getPropertyValue("--ctx-history-tone-dark")).toBe(rampColor(20, "dark"));
  });

  it("lets the keyboard always close it and never inherits the card hover", () => {
    renderScoped(35, ready(55));
    const badge = target(TESTID);
    const card = cardOf(badge);
    const other = document.createElement("button");
    document.body.append(other);

    // Focus on the badge, pointer on the card, Tab to another control: closes.
    badge.focus();
    card.dispatchEvent(new MouseEvent("mouseenter"));
    expect(card.hidden).toBe(false);
    other.focus();
    expect(document.activeElement).toBe(other);
    expect(card.hidden).toBe(true);

    // Escape closes from wherever focus is.
    badge.focus();
    card.dispatchEvent(new MouseEvent("mouseenter"));
    other.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(card.hidden).toBe(true);

    // Reopened without hover: a blur with no target (no pointer on the card) closes,
    // so the hover flag was reset by the previous close.
    badge.dispatchEvent(new FocusEvent("focus"));
    expect(card.hidden).toBe(false);
    badge.dispatchEvent(new FocusEvent("blur", { relatedTarget: null }));
    expect(card.hidden).toBe(true);

    // The scrollbar case still holds: pointer on the card, blur with no target.
    badge.dispatchEvent(new FocusEvent("focus"));
    card.dispatchEvent(new MouseEvent("mouseenter"));
    badge.dispatchEvent(new FocusEvent("blur", { relatedTarget: null }));
    expect(card.hidden).toBe(false);
  });

  it("toggles on touch activation", () => {
    renderScoped(35, ready(55));
    const badge = target(TESTID);
    const touch = () => {
      const e = new Event("pointerdown", { bubbles: true });
      Object.defineProperty(e, "pointerType", { value: "touch" });
      badge.dispatchEvent(e);
    };
    touch();
    expect(cardOf(badge).hidden).toBe(false);
    touch();
    expect(cardOf(badge).hidden).toBe(true);
  });

  it("releases the handle on dispose and before a new scope", () => {
    const [scope, setScope] = createSignal<typeof SCOPE | undefined>(SCOPE);
    const { dispose } = renderScoped(35, ready(55), { scope });
    expect(history.acquire).toHaveBeenCalledTimes(1);
    const next = "00000000-0000-4000-8000-000000000002";
    setScope({ ...SCOPE, runtimeSessionId: next });
    expect(history.release).toHaveBeenCalledTimes(1);
    expect(history.acquire).toHaveBeenCalledTimes(2);
    setScope({ ...SCOPE, runtimeSessionId: next });
    expect(history.acquire).toHaveBeenCalledTimes(2);
    dispose();
    expect(history.release).toHaveBeenCalledTimes(2);
  });
});
