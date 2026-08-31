// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import ToastHost from "./ToastHost";
import { toastStore, TOAST_EXIT_MS } from "../stores/toasts";

function q<T extends HTMLElement = HTMLElement>(testId: string): T | null {
  return document.body.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function qa(testId: string): HTMLElement[] {
  return Array.from(
    document.body.querySelectorAll<HTMLElement>(`[data-ac-testid="${testId}"]`),
  );
}

function renderHost(): () => void {
  const root = document.createElement("div");
  document.body.appendChild(root);
  const dispose = render(() => <ToastHost />, root);
  return () => {
    dispose();
    root.remove();
  };
}

describe("ToastHost (#574)", () => {
  let dispose: (() => void) | null = null;

  afterEach(() => {
    dispose?.();
    dispose = null;
    toastStore.clear();
    vi.useRealTimers();
    document.body.replaceChildren();
  });

  it("renders an error toast with kind, role=alert, and the message text", () => {
    dispose = renderHost();
    toastStore.error("boom");
    const item = q("toast.item");
    expect(item).toBeTruthy();
    expect(item?.getAttribute("data-ac-kind")).toBe("error");
    expect(item?.getAttribute("role")).toBe("alert");
    expect(item?.textContent ?? "").toContain("boom");
  });

  it("clicking the dismiss button fades then removes the item", async () => {
    vi.useFakeTimers();
    dispose = renderHost();
    toastStore.error("boom");
    expect(q("toast.item")).toBeTruthy();
    q("toast.item.dismiss")!.dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true }),
    );
    expect(q("toast.item")).toBeTruthy();
    expect(q("toast.item")?.classList.contains("toast-item--exiting")).toBe(true);
    await vi.advanceTimersByTimeAsync(TOAST_EXIT_MS);
    expect(q("toast.item")).toBeNull();
  });

  it("renders an action button that fires and dismisses the toast", async () => {
    vi.useFakeTimers();
    dispose = renderHost();
    const onClick = vi.fn();
    toastStore.push({
      message: "Interactive menu requires user input",
      durationMs: null,
      action: { label: "Resolved by user", onClick },
    });

    const action = q<HTMLButtonElement>("toast.item.action");
    expect(action?.textContent).toBe("Resolved by user");
    action!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));

    expect(onClick).toHaveBeenCalledTimes(1);
    expect(q("toast.item")?.classList.contains("toast-item--exiting")).toBe(true);
    await vi.advanceTimersByTimeAsync(TOAST_EXIT_MS);
    expect(q("toast.item")).toBeNull();
  });

  it("dismissByTag removes the matching toast", async () => {
    vi.useFakeTimers();
    dispose = renderHost();
    toastStore.push({ message: "blocked", durationMs: null, tag: "blockedMenu:one" });
    toastStore.push({ message: "other", durationMs: null, tag: "blockedMenu:two" });

    toastStore.dismissByTag("blockedMenu:one");

    expect(qa("toast.item")[0].classList.contains("toast-item--exiting")).toBe(true);
    expect(qa("toast.item")[1].classList.contains("toast-item--exiting")).toBe(false);
    await vi.advanceTimersByTimeAsync(TOAST_EXIT_MS);
    expect(qa("toast.item")).toHaveLength(1);
    expect(q("toast.item")?.textContent ?? "").toContain("other");
  });

  it("renders one node per toast", () => {
    dispose = renderHost();
    toastStore.error("a");
    toastStore.error("b");
    expect(qa("toast.item")).toHaveLength(2);
  });

  it("data-ac-kind reflects each kind", () => {
    dispose = renderHost();
    toastStore.error("e");
    toastStore.info("i");
    toastStore.success("s");
    const kinds = qa("toast.item").map((n) => n.getAttribute("data-ac-kind"));
    expect(kinds).toEqual(["error", "info", "success"]);
  });

  it("exposes data-ac-toast-id matching the returned id (§15.8/Q5)", () => {
    dispose = renderHost();
    const id = toastStore.error("x");
    expect(q("toast.item")?.getAttribute("data-ac-toast-id")).toBe(String(id));
  });

  it("a mousedown on the toast body does not reach a document listener (§15.6)", () => {
    dispose = renderHost();
    toastStore.error("boom");
    const message = document.body.querySelector<HTMLElement>(".toast-item__message");
    expect(message).toBeTruthy();

    // Mirrors App.tsx's native document `mousedown` listener (handleRaiseTerminal).
    const spy = vi.fn();
    document.addEventListener("mousedown", spy);
    try {
      message!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
      // The item's native on:mousedown stopPropagation keeps the event below
      // document, so the terminal-raise listener never fires.
      expect(spy).not.toHaveBeenCalled();

      // Control: a mousedown elsewhere DOES bubble to the document listener.
      document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
      expect(spy).toHaveBeenCalledTimes(1);
    } finally {
      document.removeEventListener("mousedown", spy);
    }
  });
});
