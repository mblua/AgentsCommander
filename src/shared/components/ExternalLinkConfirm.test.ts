// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import ExternalLinkConfirm from "./ExternalLinkConfirm";
import { WindowAPI } from "../ipc";

vi.mock("../ipc", () => ({
  WindowAPI: {
    openExternal: vi.fn(() => Promise.resolve()),
  },
}));

const externalUrl = "https://example.com/docs";

function buttonByText(text: string): HTMLButtonElement {
  const button = Array.from(document.querySelectorAll("button")).find(
    (candidate) => candidate.textContent?.trim() === text
  );
  if (!button) {
    throw new Error(`Button not found: ${text}`);
  }
  return button as HTMLButtonElement;
}

function copyButton(): HTMLButtonElement {
  const button = document.querySelector(
    'button[aria-label="Copy URL"], button[aria-label="Copied URL"]'
  );
  if (!button) {
    throw new Error("Copy button not found");
  }
  return button as HTMLButtonElement;
}

function addKeyboardActivation(button: HTMLButtonElement): void {
  button.addEventListener("keydown", (event) => {
    if (event.key === "Enter" || event.key === " ") {
      button.click();
    }
  });
}

async function flushPromises(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

async function openModal(): Promise<() => void> {
  const root = document.createElement("div");
  document.body.append(root);
  const dispose = render(() => ExternalLinkConfirm({}), root);
  const link = document.createElement("a");
  link.href = externalUrl;
  link.textContent = "external docs";
  root.append(link);

  link.dispatchEvent(
    new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 })
  );
  await flushPromises();

  expect(document.querySelector('[role="alertdialog"]')).not.toBeNull();
  return dispose;
}

describe("ExternalLinkConfirm", () => {
  beforeEach(() => {
    vi.mocked(WindowAPI.openExternal).mockClear();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: vi.fn(() => Promise.resolve()),
      },
    });
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("opens the pending external URL when Open anyway is activated with Enter", async () => {
    const dispose = await openModal();
    const openButton = buttonByText("Open anyway");
    addKeyboardActivation(openButton);
    openButton.focus();

    openButton.dispatchEvent(
      new KeyboardEvent("keydown", {
        bubbles: true,
        cancelable: true,
        key: "Enter",
      })
    );
    await flushPromises();

    expect(WindowAPI.openExternal).toHaveBeenCalledWith(externalUrl);
    expect(document.querySelector('[role="alertdialog"]')).toBeNull();

    dispose();
  });

  it("closes the modal when Cancel is activated with Space", async () => {
    const dispose = await openModal();
    const cancelButton = buttonByText("Cancel");
    addKeyboardActivation(cancelButton);
    cancelButton.focus();

    cancelButton.dispatchEvent(
      new KeyboardEvent("keydown", {
        bubbles: true,
        cancelable: true,
        key: " ",
      })
    );
    await flushPromises();

    expect(WindowAPI.openExternal).not.toHaveBeenCalled();
    expect(document.querySelector('[role="alertdialog"]')).toBeNull();

    dispose();
  });

  it("copies the pending external URL when Copy URL is activated with Enter", async () => {
    const dispose = await openModal();
    const copy = copyButton();
    addKeyboardActivation(copy);
    copy.focus();

    copy.dispatchEvent(
      new KeyboardEvent("keydown", {
        bubbles: true,
        cancelable: true,
        key: "Enter",
      })
    );
    await flushPromises();

    expect(navigator.clipboard.writeText).toHaveBeenCalledWith(externalUrl);
    expect(document.querySelector('[role="alertdialog"]')).not.toBeNull();

    dispose();
  });
});
