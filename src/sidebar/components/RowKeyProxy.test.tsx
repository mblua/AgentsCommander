// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import { RowKeyProxy, onRowKey, ROW_KEY_PROXY_CLASS } from "./RowKeyProxy";

function key(el: Element, init: KeyboardEventInit): KeyboardEvent {
  const ev = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  el.dispatchEvent(ev);
  return ev;
}

function click(el: Element): void {
  el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
}

describe("RowKeyProxy", () => {
  let host: HTMLDivElement;
  let dispose: () => void;
  let spyClick: ReturnType<typeof vi.fn<() => void>>;
  let spyAct: ReturnType<typeof vi.fn<() => void>>;
  let row: HTMLElement;
  let proxy: HTMLElement;
  let nested: HTMLElement;

  beforeEach(() => {
    host = document.createElement("div");
    document.body.appendChild(host);
    spyClick = vi.fn<() => void>();
    spyAct = vi.fn<() => void>();
    dispose = render(
      () => (
        <div data-testid="row" onClick={() => spyClick()} onKeyDown={onRowKey(() => spyAct())}>
          <RowKeyProxy label="Toggle Row" />
          <button type="button" data-testid="nested">x</button>
        </div>
      ),
      host
    );
    row = host.querySelector<HTMLElement>('[data-testid="row"]')!;
    proxy = row.querySelector<HTMLElement>(`.${ROW_KEY_PROXY_CLASS}`)!;
    nested = row.querySelector<HTMLElement>('[data-testid="nested"]')!;
  });

  afterEach(() => {
    dispose();
    host.remove();
  });

  it("renders a native button as the row's first child", () => {
    expect(proxy.tagName).toBe("BUTTON");
    expect(proxy.getAttribute("type")).toBe("button");
    expect(proxy.getAttribute("aria-label")).toBe("Toggle Row");
    expect(proxy.classList.contains("ac-row-key-proxy")).toBe(true);
    expect(row.firstElementChild).toBe(proxy);
  });

  it("runs the action once on Enter and on Space", () => {
    const enter = key(proxy, { key: "Enter" });
    expect(spyAct).toHaveBeenCalledTimes(1);
    expect(enter.defaultPrevented).toBe(true);
    const space = key(proxy, { key: " " });
    expect(spyAct).toHaveBeenCalledTimes(2);
    expect(space.defaultPrevented).toBe(true);
  });

  it("ignores other keys", () => {
    key(proxy, { key: "a" });
    key(proxy, { key: "Escape" });
    expect(spyAct).not.toHaveBeenCalled();
  });

  it("does not re-fire on a held key (repeat)", () => {
    const ev = key(proxy, { key: "Enter", repeat: true });
    expect(spyAct).not.toHaveBeenCalled();
    expect(ev.defaultPrevented).toBe(true);
  });

  it("fires once when a keydown is followed by the synthesized click", () => {
    key(proxy, { key: "Enter" });
    click(proxy);
    expect(spyAct).toHaveBeenCalledTimes(1);
    expect(spyClick).not.toHaveBeenCalled();
  });

  it("ignores keys from a nested button", () => {
    key(nested, { key: "Enter" });
    expect(spyAct).not.toHaveBeenCalled();
  });

  it("swallows a click on the proxy", () => {
    click(proxy);
    expect(spyClick).not.toHaveBeenCalled();
  });

  it("leaves a mouse click on the row unchanged", () => {
    click(row);
    expect(spyClick).toHaveBeenCalledTimes(1);
    expect(spyAct).not.toHaveBeenCalled();
  });
});
