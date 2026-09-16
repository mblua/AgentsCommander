// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import ErrorModal from "./ErrorModal";
import {
  errorModalStore,
  __resetErrorModalStoreForTests,
} from "../stores/error-modal";
import type { ErrorLogEntry } from "../../shared/types";

const entry: ErrorLogEntry = {
  timestamp: "2026-06-13T00:00:00.000Z",
  level: "error",
  target: "test",
  message: "boom",
};

let cleanup: (() => void) | null = null;
const documentListeners: Array<() => void> = [];

const hop = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

afterEach(() => {
  cleanup?.();
  cleanup = null;
  for (const stop of documentListeners.splice(0)) stop();
  __resetErrorModalStoreForTests();
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

function mount(): void {
  const root = document.createElement("div");
  document.body.appendChild(root);
  cleanup = render(() => <ErrorModal />, root);
}

function keydown(key: string, shiftKey = false): KeyboardEvent {
  const e = new KeyboardEvent("keydown", {
    key,
    shiftKey,
    cancelable: true,
    bubbles: true,
  });
  document.dispatchEvent(e);
  return e;
}

/** A document listener registered after the modal's, so only the modal's
 *  `stopImmediatePropagation` decides whether it ever sees the key. */
function collectKeys(): string[] {
  const seen: string[] = [];
  const listener = (e: KeyboardEvent) => seen.push(e.key);
  document.addEventListener("keydown", listener, true);
  documentListeners.push(() => document.removeEventListener("keydown", listener, true));
  return seen;
}

describe("ErrorModal key routing", () => {
  it("does not intercept keys while closed", () => {
    mount();
    const seen = collectKeys();
    const e = keydown("Tab");
    expect(e.defaultPrevented).toBe(false);
    expect(seen).toEqual(["Tab"]);
  });

  it("Escape dismisses while open", async () => {
    mount();
    errorModalStore.enqueue([entry]);
    await hop();
    expect(errorModalStore.open).toBe(true);

    const e = keydown("Escape");
    expect(e.defaultPrevented).toBe(true);
    expect(errorModalStore.open).toBe(false);
  });

  it("stops immediate propagation for Tab and unrelated keys while open", async () => {
    mount();
    const seen = collectKeys();
    errorModalStore.enqueue([entry]);
    await hop();
    expect(errorModalStore.open).toBe(true);

    keydown("Tab");
    keydown("x");
    expect(seen).toEqual([]);
  });

  it("wraps Tab focus inside the modal", async () => {
    mount();
    errorModalStore.enqueue([entry]);
    await hop();

    const message = document.querySelector<HTMLElement>(".error-modal-message");
    const dismiss = document.querySelector<HTMLButtonElement>(".error-modal-btn-dismiss");
    if (!message || !dismiss) throw new Error("error modal did not render");

    dismiss.focus();
    const e = keydown("Tab");
    expect(e.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(message);
  });
});
