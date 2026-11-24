// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import AgentMatrixNoticeModal from "./AgentMatrixNoticeModal";
import { click, renderWithFakeTransport } from "../../shared/testing/ui-harness";

const matrixPath = "/fixture/.ac/_agent_alpha";

describe("AgentMatrixNoticeModal", () => {
  let dispose: (() => void) | undefined;

  afterEach(() => {
    dispose?.();
    dispose = undefined;
  });

  function mount() {
    const onClose = vi.fn();
    const { cleanup, root } = renderWithFakeTransport(() => (
      <AgentMatrixNoticeModal name="fixture/alpha" path={matrixPath} onClose={onClose} />
    ));
    dispose = cleanup;
    return {
      onClose,
      overlay: root.querySelector<HTMLElement>('[data-ac-testid="agentMatrixNotice.overlay"]')!,
      dialog: root.querySelector<HTMLElement>('[data-ac-testid="agentMatrixNotice.modal"]')!,
      close: root.querySelector<HTMLButtonElement>('[data-ac-testid="agentMatrixNotice.close"]')!,
      description: root.querySelector<HTMLElement>("#agentMatrixNoticeDescription")!,
    };
  }

  it("links the title and description and keeps Got it as its sole Tab stop", () => {
    const { overlay, dialog, close, description } = mount();
    expect(overlay.getAttribute("data-ac-role")).toBe("overlay");
    expect(dialog.getAttribute("data-ac-role")).toBe("dialog");
    expect(close.getAttribute("data-ac-role")).toBe("button");
    expect(dialog.getAttribute("role")).toBe("dialog");
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    const title = document.getElementById(dialog.getAttribute("aria-labelledby")!);
    expect(title?.textContent).toBe("Agent Matrix");
    expect(dialog.contains(title)).toBe(true);
    expect(dialog.getAttribute("aria-describedby")).toBe(description.id);
    expect(description.textContent).toContain("alpha");
    expect(description.textContent).toContain(matrixPath);
    expect(close.textContent?.trim()).toBe("Got it");
    expect(close.hasAttribute("autofocus")).toBe(true);
    const focusable = dialog.querySelectorAll<HTMLElement>(
      "a[href], button, input, select, textarea, [contenteditable], [tabindex]",
    );
    expect(Array.from(focusable).filter((el) => el.tabIndex >= 0)).toEqual([close]);
    close.focus();
    expect(document.activeElement).toBe(close);
  });

  it("calls close once per button or backdrop click while mounted", () => {
    const { overlay, dialog, close, onClose } = mount();
    click(close);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(dialog.isConnected).toBe(true);
    click(overlay);
    expect(onClose).toHaveBeenCalledTimes(2);
    expect(dialog.isConnected).toBe(true);
  });

  it("contains the nested description click without closing or bubbling to window", () => {
    const { description, dialog, onClose } = mount();
    const windowClick = vi.fn();
    window.addEventListener("click", windowClick);
    try {
      click(description.querySelector("strong")!);
      expect(windowClick).not.toHaveBeenCalled();
      expect(onClose).not.toHaveBeenCalled();
      expect(dialog.isConnected).toBe(true);
    } finally {
      window.removeEventListener("click", windowClick);
    }
  });

  it("handles Escape from inside and outside until unmount", () => {
    const { close, onClose } = mount();
    const escape = (from: EventTarget) => {
      from.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    };
    close.focus();
    expect(document.activeElement).toBe(close);
    escape(close);
    expect(onClose).toHaveBeenCalledTimes(1);
    const outside = document.createElement("button");
    document.body.append(outside);
    try {
      outside.focus();
      expect(document.activeElement).toBe(outside);
      escape(outside);
      expect(onClose).toHaveBeenCalledTimes(2);
      dispose?.();
      dispose = undefined;
      escape(outside);
      expect(onClose).toHaveBeenCalledTimes(2);
    } finally {
      outside.remove();
    }
  });
});
