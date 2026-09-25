// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import "./sessions";

const saved = Object.getOwnPropertyDescriptor(globalThis, "document")!;
const appended: Node[] = [];

afterEach(() => {
  if (!("document" in globalThis)) Object.defineProperty(globalThis, "document", saved);
  vi.restoreAllMocks();
  for (const node of appended.splice(0)) node.parentNode?.removeChild(node);
});

it("menu-lock observer callback does not throw once document is gone (#2504)", async () => {
  const errors: unknown[] = [];
  const win = window;
  const body = document.body;
  win.addEventListener("error", (e) => {
    errors.push(e.error);
    e.preventDefault();
  });
  const disconnect = vi.spyOn(win.MutationObserver.prototype, "disconnect");
  try {
    appended.push(body.appendChild(win.document.createElement("div")));
    delete (globalThis as { document?: Document }).document;
    await Promise.resolve();
    await Promise.resolve();
  } finally {
    Object.defineProperty(globalThis, "document", saved);
  }
  expect(errors).toEqual([]);
  expect(disconnect).toHaveBeenCalledTimes(1);
});
