// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import RestartPromptModal from "./RestartPromptModal";

function target<T extends HTMLElement = HTMLElement>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing test target: ${testId}`);
  return element;
}

function renderModal(
  overrides: Partial<{
    agentLabel: string;
    replicaName: string;
    error: string;
    busy: boolean;
    onRestart: () => void;
    onLater: () => void;
  }> = {},
) {
  const root = document.createElement("div");
  const onRestart = vi.fn();
  const onLater = vi.fn();
  document.body.append(root);
  const dispose = render(
    () => (
      <RestartPromptModal
        agentLabel={overrides.agentLabel ?? "Codex"}
        replicaName={overrides.replicaName ?? "dev-webpage-ui"}
        error={overrides.error}
        busy={overrides.busy}
        onRestart={overrides.onRestart ?? onRestart}
        onLater={overrides.onLater ?? onLater}
      />
    ),
    root,
  );
  return { dispose, onRestart, onLater };
}

describe("RestartPromptModal (#537)", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  it("names the assigned agent and replica in the prompt", () => {
    const { dispose } = renderModal({ agentLabel: "Claude Code", replicaName: "dev-rust" });
    const body = target("restartPrompt.modal").textContent ?? "";
    expect(body).toContain("Claude Code");
    expect(body).toContain("dev-rust");
    expect(body).toContain("Restart the session now to apply it?");
    dispose();
  });

  it("Restart now triggers the restart and not the Later callback", () => {
    const { dispose, onRestart, onLater } = renderModal();
    target<HTMLButtonElement>("restartPrompt.restart").click();
    expect(onRestart).toHaveBeenCalledTimes(1);
    expect(onLater).not.toHaveBeenCalled();
    dispose();
  });

  it("Later is a no-op for restart (only the Later callback fires)", () => {
    const { dispose, onRestart, onLater } = renderModal();
    target<HTMLButtonElement>("restartPrompt.later").click();
    expect(onLater).toHaveBeenCalledTimes(1);
    expect(onRestart).not.toHaveBeenCalled();
    dispose();
  });

  it("dismisses via overlay click (treated as Later) without restarting", () => {
    const { dispose, onRestart, onLater } = renderModal();
    target("restartPrompt.overlay").click();
    expect(onLater).toHaveBeenCalledTimes(1);
    expect(onRestart).not.toHaveBeenCalled();
    dispose();
  });

  it("does not bubble an inner-dialog click to the overlay dismiss", () => {
    const { dispose, onRestart, onLater } = renderModal();
    target("restartPrompt.modal").click();
    expect(onLater).not.toHaveBeenCalled();
    expect(onRestart).not.toHaveBeenCalled();
    dispose();
  });

  // A swallowed restart failure used to leave the old agent running while
  // the UI looked applied. The modal now surfaces the error and exposes a busy
  // state so the caller can keep it open for a retry.
  it("renders no error chip and an idle, enabled confirm button by default", () => {
    const { dispose } = renderModal();
    expect(document.querySelector('[data-ac-testid="restartPrompt.error"]')).toBeNull();
    const restart = target<HTMLButtonElement>("restartPrompt.restart");
    expect(restart.disabled).toBe(false);
    expect(restart.textContent?.trim()).toBe("Restart now");
    dispose();
  });

  it("surfaces a failed restart inline while leaving the buttons usable for retry", () => {
    const { dispose } = renderModal({
      error: "Resource Monitor cap reached (16/16). Close an agent or raise the limit in Settings > Resources.",
    });
    const chip = target("restartPrompt.error");
    expect(chip.textContent ?? "").toContain("Resource Monitor cap reached");
    // Not busy → the user can immediately retry from the still-open modal.
    expect(target<HTMLButtonElement>("restartPrompt.restart").disabled).toBe(false);
    expect(target<HTMLButtonElement>("restartPrompt.later").disabled).toBe(false);
    dispose();
  });

  it("disables both buttons and shows a progress label while busy", () => {
    const { dispose, onRestart, onLater } = renderModal({ busy: true });
    const restart = target<HTMLButtonElement>("restartPrompt.restart");
    const later = target<HTMLButtonElement>("restartPrompt.later");
    expect(restart.disabled).toBe(true);
    expect(later.disabled).toBe(true);
    expect(restart.textContent?.trim()).toBe("Restarting…");
    // A disabled control must not fire its callback (no double-restart).
    restart.click();
    later.click();
    expect(onRestart).not.toHaveBeenCalled();
    expect(onLater).not.toHaveBeenCalled();
    dispose();
  });

  // Escape on the dialog does exactly what a backdrop click does.
  it("focuses the dialog on mount and treats Escape like a backdrop click", async () => {
    const { dispose, onRestart, onLater } = renderModal({ busy: true });
    const dialog = target("restartPrompt.modal");
    await vi.waitFor(() => expect(document.activeElement).toBe(dialog));
    for (const [key, calls] of [["Enter", 0], ["Escape", 1]] as const) {
      dialog.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
      expect(onLater).toHaveBeenCalledTimes(calls);
    }
    target("restartPrompt.overlay").click();
    expect(onLater).toHaveBeenCalledTimes(2);
    expect(onRestart).not.toHaveBeenCalled();
    dispose();
  });
});

describe("RestartPromptModal accessibility characterization (#4945 :26)", () => {
  let windowClicks: EventTarget[] = [];
  const onWindowClick = (e: Event) => { windowClicks.push(e.target as EventTarget); };

  beforeEach(() => {
    windowClicks = [];
    window.addEventListener("click", onWindowClick);
  });

  afterEach(() => {
    window.removeEventListener("click", onWindowClick);
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  const bubbleClick = (el: Element) =>
    el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));

  it("autofocuses the dialog focus host on mount", async () => {
    const { dispose } = renderModal();
    const dialog = target("restartPrompt.modal");
    expect(dialog.classList.contains("ac-escape-focus-host")).toBe(true);
    expect(dialog.getAttribute("role")).toBe("dialog");
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(dialog.getAttribute("aria-labelledby")).toBe("restartPromptTitle");
    await vi.waitFor(() => expect(document.activeElement).toBe(dialog));
    dispose();
  });

  it("nested content clicks never reach window or Later", () => {
    const { dispose, onRestart, onLater } = renderModal();
    bubbleClick(target("restartPrompt.modal").querySelector("strong")!);
    bubbleClick(target("restartPrompt.modal"));
    expect(windowClicks).toHaveLength(0);
    expect(onLater).not.toHaveBeenCalled();
    expect(onRestart).not.toHaveBeenCalled();
    dispose();
  });

  it("bare backdrop click calls Later exactly once and reaches window", () => {
    const { dispose, onRestart, onLater } = renderModal();
    const overlay = target("restartPrompt.overlay");
    bubbleClick(overlay);
    expect(onLater).toHaveBeenCalledTimes(1);
    expect(onRestart).not.toHaveBeenCalled();
    // Baseline: the overlay handler does not stop propagation.
    expect(windowClicks).toEqual([overlay]);
    dispose();
  });

  it("Escape on the dialog calls Later exactly once", () => {
    const { dispose, onRestart, onLater } = renderModal();
    const event = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
    target("restartPrompt.modal").dispatchEvent(event);
    expect(onLater).toHaveBeenCalledTimes(1);
    expect(onRestart).not.toHaveBeenCalled();
    dispose();
  });

  it("Later and Restart buttons call only their own callback, shielded from window", () => {
    const { dispose, onRestart, onLater } = renderModal();
    bubbleClick(target("restartPrompt.later"));
    expect(onLater).toHaveBeenCalledTimes(1);
    expect(onRestart).not.toHaveBeenCalled();
    bubbleClick(target("restartPrompt.restart"));
    expect(onRestart).toHaveBeenCalledTimes(1);
    expect(onLater).toHaveBeenCalledTimes(1);
    expect(windowClicks).toHaveLength(0);
    dispose();
  });
});
