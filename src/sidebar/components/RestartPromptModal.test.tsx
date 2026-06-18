// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
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
});
