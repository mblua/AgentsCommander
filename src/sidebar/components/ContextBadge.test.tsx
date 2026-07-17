// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import ContextBadge, { CONTEXT_BADGE_TOOLTIP } from "./ContextBadge";

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
