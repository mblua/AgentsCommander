// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { blockContextMenu } from "./App";

function dispatchDocumentContextMenu(target: Element): MouseEvent {
  const event = new MouseEvent("contextmenu", {
    bubbles: true,
    cancelable: true,
    clientX: 80,
    clientY: 96,
  });

  document.addEventListener("contextmenu", blockContextMenu);
  try {
    target.dispatchEvent(event);
  } finally {
    document.removeEventListener("contextmenu", blockContextMenu);
  }

  return event;
}

describe("SidebarApp global context-menu blocker", () => {
  afterEach(() => {
    document.body.replaceChildren();
  });

  it("preserves the native context menu in the project regex filter input", () => {
    const row = document.createElement("div");
    row.className = "project-filter-row";
    const input = document.createElement("input");
    input.className = "project-filter-input";
    row.append(input);
    document.body.append(row);

    const event = dispatchDocumentContextMenu(input);

    expect(event.defaultPrevented).toBe(false);
  });

  it("still blocks non-exempt sidebar surfaces", () => {
    const surface = document.createElement("div");
    surface.className = "sidebar-scrollable";
    document.body.append(surface);

    const event = dispatchDocumentContextMenu(surface);

    expect(event.defaultPrevented).toBe(true);
  });
});
