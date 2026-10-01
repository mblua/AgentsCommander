import { vi } from "vitest";

// Geometry enables bridge dispatch in jsdom; it does not establish Windows
// visibility, hit-testing, physical keyboard/focus behavior or real IME coverage.
export function stubAutomationGeometry() {
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
    return { x: 100, y: 50, left: 100, top: 50, right: 200, bottom: 70,
      width: this.isConnected ? 100 : 0, height: this.isConnected ? 20 : 0, toJSON: () => ({}) } as DOMRect;
  });
  vi.spyOn(Element.prototype, "getClientRects").mockImplementation(function (this: Element) {
    const list = (this.isConnected ? [this.getBoundingClientRect()] : []) as unknown as DOMRectList;
    Object.defineProperty(list, "item", { value: (index: number) => list[index] ?? null });
    return list;
  });
}
