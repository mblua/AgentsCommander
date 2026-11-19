// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import UnleashConfirmModal from "./UnleashConfirmModal";

// #4752 P05 - the Unleash confirmation dialog.
const byTestId = (id: string) => document.querySelector<HTMLElement>(`[data-ac-testid="${id}"]`);

describe("UnleashConfirmModal (#4752)", () => {
  let dispose: (() => void) | null = null;

  function mount(busy = false) {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    const root = document.createElement("div");
    document.body.appendChild(root);
    dispose = render(
      () => (
        <UnleashConfirmModal
          replicaName="dev-rust"
          busy={busy}
          onConfirm={onConfirm}
          onCancel={onCancel}
        />
      ),
      root
    );
    return { onConfirm, onCancel };
  }

  afterEach(() => {
    dispose?.();
    dispose = null;
    document.body.replaceChildren();
  });

  it("states the three required facts", () => {
    mount();
    const dialog = byTestId("unleashConfirm.modal")!;
    expect(dialog.tagName).toBe("DIALOG");
    expect(dialog.classList.contains("agent-modal")).toBe(true);
    expect(dialog.classList.contains("unleash-confirm")).toBe(true);
    expect(dialog.hasAttribute("open")).toBe(true);
    expect(dialog.getAttribute("aria-labelledby")).toBe("unleashConfirmTitle");
    expect(dialog.getAttribute("aria-describedby")).toBe("unleashConfirmDescription");
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    const text = dialog.textContent!.replace(/\s+/g, " ");
    expect(text).toContain("Unleash dev-rust?");
    expect(text).toContain("remove the workspace restrictions from the context it gives this agent");
    expect(text).toContain("The Git brakes are removed too");
    expect(text).toContain("The session will be restarted now.");
  });

  it("focuses Cancel after mount", async () => {
    mount();
    await new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0)));
    expect(document.activeElement).toBe(byTestId("unleashConfirm.cancel"));
  });

  it("cancels on Escape, overlay click and Cancel; confirms on the confirm button", () => {
    const { onConfirm, onCancel } = mount();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    byTestId("unleashConfirm.overlay")!.click();
    byTestId("unleashConfirm.modal")!.click();
    byTestId("unleashConfirm.cancel")!.click();
    expect(onCancel).toHaveBeenCalledTimes(3);
    byTestId("unleashConfirm.confirm")!.click();
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("disables confirm while busy", () => {
    mount(true);
    expect((byTestId("unleashConfirm.confirm") as HTMLButtonElement).disabled).toBe(true);
  });
});
