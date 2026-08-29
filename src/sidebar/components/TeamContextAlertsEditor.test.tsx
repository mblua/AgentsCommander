// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  click,
  input,
  installBrowserDomStubs,
  waitFor,
} from "../../shared/testing/ui-harness";
import { TeamContextAlertsEditor } from "./TeamContextAlertsEditor";
import type { ContextAlertThresholdDraft } from "./team-context-alerts";
import { validateContextAlertThresholdDrafts } from "./team-context-alerts";

function draft(id: number, raw: string): ContextAlertThresholdDraft {
  return { id, raw };
}

function button(root: ParentNode, label: string): HTMLButtonElement {
  const found = Array.from(root.querySelectorAll("button")).find(
    (candidate) => candidate.textContent?.trim() === label,
  );
  if (!(found instanceof HTMLButtonElement)) throw new Error(`Button not found: ${label}`);
  return found;
}

function removeButton(root: ParentNode, number: number): HTMLButtonElement {
  const found = root.querySelector(`[aria-label="Remove threshold ${number}"]`);
  if (!(found instanceof HTMLButtonElement)) throw new Error(`Remove button not found: ${number}`);
  return found;
}

function inputs(root: ParentNode): HTMLInputElement[] {
  return Array.from(root.querySelectorAll<HTMLInputElement>(".team-context-alert-input"));
}

function mountEditor(initial: ContextAlertThresholdDraft[], initiallyDisabled = false) {
  const root = document.createElement("div");
  document.body.appendChild(root);
  const [drafts, setDrafts] = createSignal(initial);
  const [disabled, setDisabled] = createSignal(initiallyDisabled);
  const dispose = render(
    () => (
      <TeamContextAlertsEditor
        idPrefix="test-context-alerts"
        drafts={drafts()}
        validation={validateContextAlertThresholdDrafts(drafts())}
        disabled={disabled()}
        onChange={setDrafts}
      />
    ),
    root,
  );
  return {
    root,
    drafts,
    setDrafts,
    setDisabled,
    cleanup: () => {
      dispose();
      root.remove();
    },
  };
}

describe("TeamContextAlertsEditor", () => {
  let restoreDom: (() => void) | null = null;
  let cleanupEditor: (() => void) | null = null;

  beforeEach(() => {
    restoreDom = installBrowserDomStubs();
  });

  afterEach(() => {
    cleanupEditor?.();
    cleanupEditor = null;
    restoreDom?.();
    restoreDom = null;
    document.body.replaceChildren();
  });

  it("renders the labelled empty state, count, scope, signal caveat, and no-action copy", () => {
    const mounted = mountEditor([]);
    cleanupEditor = mounted.cleanup;

    const section = mounted.root.querySelector("section");
    expect(section?.getAttribute("aria-labelledby")).toBe("test-context-alerts-heading");
    expect(mounted.root.textContent).toContain("Context usage alerts (optional)");
    expect(mounted.root.textContent).toContain("No context alerts configured.");
    expect(mounted.root.textContent).toContain("0 of 3 thresholds");
    expect(mounted.root.textContent).toContain("Applies to every room of this team.");
    expect(mounted.root.textContent).toContain("best-effort");
    expect(mounted.root.textContent).toContain("contextRegex");
    expect(mounted.root.textContent).toContain("No automatic action is taken");
    expect(button(mounted.root, "Add threshold").getAttribute("aria-describedby")).toBe(
      "test-context-alerts-count",
    );
  });

  it("adds a blank focused row while preserving its input node through raw updates", async () => {
    const mounted = mountEditor([]);
    cleanupEditor = mounted.cleanup;

    click(button(mounted.root, "Add threshold"));
    await waitFor(() => expect(inputs(mounted.root)).toHaveLength(1));
    const thresholdInput = inputs(mounted.root)[0];
    if (!thresholdInput) throw new Error("Threshold input missing");
    await waitFor(() => expect(document.activeElement).toBe(thresholdInput));

    expect(mounted.drafts()).toEqual([{ id: 1, raw: "" }]);
    expect(thresholdInput.getAttribute("aria-invalid")).toBe("true");
    expect(mounted.root.querySelector('[role="alert"][aria-label="Context alert threshold errors"]'))
      .toBeTruthy();

    for (const raw of ["+", "+50", "050"] as const) {
      input(thresholdInput, raw);
      expect(mounted.drafts()[0]?.raw).toBe(raw);
      expect(inputs(mounted.root)[0]).toBe(thresholdInput);
      expect(document.activeElement).toBe(thresholdInput);
    }
    expect(thresholdInput.getAttribute("aria-invalid")).toBeNull();
  });

  it("enforces the cap without hiding malformed over-cardinality rows", () => {
    const capped = mountEditor([draft(1, "10"), draft(2, "20"), draft(3, "30")]);
    cleanupEditor = capped.cleanup;
    expect(button(capped.root, "Add threshold").disabled).toBe(true);
    expect(capped.root.textContent).toContain("3 of 3 thresholds");

    capped.setDrafts([
      draft(1, "10"),
      draft(2, "20"),
      draft(3, "30"),
      draft(4, "40"),
    ]);
    expect(inputs(capped.root)).toHaveLength(4);
    expect(capped.root.textContent).toContain("4 of 3 thresholds");
    expect(button(capped.root, "Add threshold").disabled).toBe(true);
    expect(Array.from(capped.root.querySelectorAll<HTMLButtonElement>("[aria-label^='Remove threshold']")))
      .toHaveLength(4);
    expect(Array.from(capped.root.querySelectorAll<HTMLButtonElement>("[aria-label^='Remove threshold']"))
      .every((candidate) => !candidate.disabled)).toBe(true);
  });

  it("preserves surviving keyed nodes and focuses next, previous, then Add after removals", async () => {
    const mounted = mountEditor([draft(1, "10"), draft(2, "20"), draft(3, "30")]);
    cleanupEditor = mounted.cleanup;
    const [first, second, third] = inputs(mounted.root);
    if (!first || !second || !third) throw new Error("Expected three inputs");

    click(removeButton(mounted.root, 2));
    await waitFor(() => expect(document.activeElement).toBe(third));
    expect(inputs(mounted.root)).toEqual([first, third]);

    click(removeButton(mounted.root, 2));
    await waitFor(() => expect(document.activeElement).toBe(first));
    expect(inputs(mounted.root)).toEqual([first]);

    click(removeButton(mounted.root, 1));
    await waitFor(() => expect(document.activeElement).toBe(button(mounted.root, "Add threshold")));
    expect(inputs(mounted.root)).toEqual([]);
  });

  it("uses stable accessible text inputs and exposes exact raw spellings to the validator", () => {
    const mounted = mountEditor([draft(9, "50")]);
    cleanupEditor = mounted.cleanup;
    const thresholdInput = inputs(mounted.root)[0];
    if (!thresholdInput) throw new Error("Threshold input missing");

    expect(thresholdInput.id).toBe("test-context-alerts-threshold-9");
    expect(thresholdInput.labels?.[0]?.textContent?.trim()).toBe("Threshold 1 percentage");
    expect(thresholdInput.type).toBe("text");
    expect(thresholdInput.getAttribute("inputmode")).toBe("numeric");
    expect(thresholdInput.autocomplete).toBe("off");
    for (const forbiddenAttribute of ["pattern", "min", "max", "step"]) {
      expect(thresholdInput.hasAttribute(forbiddenAttribute)).toBe(false);
    }
    expect(thresholdInput.getAttribute("aria-describedby")).toBe("test-context-alerts-help");
    expect(mounted.root.querySelector(".team-context-alert-suffix")?.getAttribute("aria-hidden"))
      .toBe("true");

    for (const raw of ["+50", "-50", "5e1", "text", "50.5"] as const) {
      input(thresholdInput, raw);
      expect(mounted.drafts()[0]?.raw).toBe(raw);
      expect(thresholdInput.getAttribute("aria-invalid")).toBe("true");
      const describedIds = thresholdInput.getAttribute("aria-describedby")?.split(/\s+/) ?? [];
      expect(describedIds).toContain("test-context-alerts-help");
      expect(describedIds).toContain("test-context-alerts-threshold-9-error");
      expect(mounted.root.querySelector("#test-context-alerts-threshold-9-error")).toBeTruthy();
    }

    input(thresholdInput, "050");
    expect(mounted.drafts()[0]?.raw).toBe("050");
    expect(thresholdInput.getAttribute("aria-invalid")).toBeNull();
    expect(thresholdInput.getAttribute("aria-describedby")).toBe("test-context-alerts-help");
    expect(mounted.root.querySelector("#test-context-alerts-threshold-9-error")).toBeNull();
  });

  it("disables every editor control without changing controlled raw drafts", () => {
    const mounted = mountEditor([draft(1, "50")], true);
    cleanupEditor = mounted.cleanup;
    const thresholdInput = inputs(mounted.root)[0];
    if (!thresholdInput) throw new Error("Threshold input missing");

    expect(thresholdInput.disabled).toBe(true);
    expect(button(mounted.root, "Add threshold").disabled).toBe(true);
    expect(removeButton(mounted.root, 1).disabled).toBe(true);
    input(thresholdInput, "75");
    expect(mounted.drafts()).toEqual([draft(1, "50")]);

    mounted.setDisabled(false);
    expect(thresholdInput.disabled).toBe(false);
    expect(removeButton(mounted.root, 1).disabled).toBe(false);
  });
});
