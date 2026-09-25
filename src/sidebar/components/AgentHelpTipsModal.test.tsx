// @vitest-environment jsdom
import { Show, createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentHelpEntry } from "../../shared/agent-help";
import AgentHelpTipsModal from "./AgentHelpTipsModal";

const ENTRY: AgentHelpEntry = {
  label: "Tool",
  tips: [
    { title: "First tip", body: "First body", link: { label: "Docs", url: "https://example.com/docs" } },
    { title: "Second tip", body: "Second body", link: { label: "More", url: "https://example.com/more" } },
  ],
};

function byTestId<T extends HTMLElement = HTMLElement>(testId: string): T | null {
  return document.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function tipSections(): NodeListOf<HTMLElement> {
  return document.querySelectorAll<HTMLElement>(".agent-help-tips-tip");
}

function stubSelection(text: () => string): void {
  vi.spyOn(window, "getSelection").mockImplementation(() => ({ toString: text }) as Selection);
}

function rightClickBody(x = 100, y = 120): MouseEvent {
  const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: x, clientY: y });
  document.querySelector(".agent-help-tips-body")!.dispatchEvent(event);
  return event;
}

async function flushClipboard(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

let dispose: (() => void) | null = null;

function mount(props: {
  entry: AgentHelpEntry | null;
  localError?: string | null;
  onClose?: () => void;
}): void {
  const root = document.createElement("div");
  document.body.appendChild(root);
  dispose = render(
    () => (
      <AgentHelpTipsModal
        title="Tool"
        entry={props.entry}
        localError={props.localError ?? null}
        onClose={props.onClose ?? (() => {})}
      />
    ),
    root,
  );
}

afterEach(() => {
  dispose?.();
  dispose = null;
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  document.body.replaceChildren();
});

describe("AgentHelpTipsModal (#2143)", () => {
  it("Escape closes the window", async () => {
    const [open, setOpen] = createSignal(true);
    const root = document.createElement("div");
    document.body.appendChild(root);
    dispose = render(
      () => (
        <Show when={open()}>
          <AgentHelpTipsModal title="Tool" entry={ENTRY} localError={null} onClose={() => setOpen(false)} />
        </Show>
      ),
      root,
    );
    expect(byTestId("agentHelpTips.modal")).toBeTruthy();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(open()).toBe(false);
    expect(byTestId("agentHelpTips.modal")).toBeNull();
  });

  it("the previously focused element is restored", async () => {
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    opener.focus();
    const [open, setOpen] = createSignal(true);
    const root = document.createElement("div");
    document.body.appendChild(root);
    dispose = render(
      () => (
        <Show when={open()}>
          <AgentHelpTipsModal title="Tool" entry={ENTRY} localError={null} onClose={() => setOpen(false)} />
        </Show>
      ),
      root,
    );
    await Promise.resolve();
    expect(document.activeElement).toBe(byTestId("agentHelpTips.close"));
    byTestId<HTMLButtonElement>("agentHelpTips.close")!.click();
    expect(byTestId("agentHelpTips.modal")).toBeNull();
    expect(document.activeElement).toBe(opener);
  });

  it("a local error is shown", () => {
    mount({ entry: ENTRY, localError: "line 3: expected value" });
    expect(byTestId("agentHelpTips.localError")?.textContent).toBe(
      "Your agent-help.local.json was ignored: line 3: expected value",
    );
    expect(tipSections()).toHaveLength(2);
  });

  it("a right-click with a selection opens the copy menu", () => {
    stubSelection(() => "First body");
    mount({ entry: ENTRY });
    const event = rightClickBody(100, 120);
    expect(event.defaultPrevented).toBe(true);
    const menu = byTestId("agentHelpTips.copyMenu");
    expect(menu).toBeTruthy();
    expect(menu!.style.left).toBe("100px");
    expect(menu!.style.top).toBe("120px");
    expect(byTestId("agentHelpTips.copyMenuItem")?.textContent).toBe("Copy");
  });

  it("a right-click with no selection opens nothing", () => {
    stubSelection(() => "");
    mount({ entry: ENTRY });
    const event = rightClickBody();
    expect(event.defaultPrevented).toBe(false);
    expect(byTestId("agentHelpTips.copyMenu")).toBeNull();
  });

  it("the copy item writes the text selected at open time", async () => {
    let selected = "First body";
    stubSelection(() => selected);
    const writeText = vi.fn((_text: string) => Promise.resolve());
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    mount({ entry: ENTRY });
    rightClickBody();
    // Pressing the item collapses the live selection.
    selected = "";
    byTestId<HTMLButtonElement>("agentHelpTips.copyMenuItem")!.click();
    await flushClipboard();
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(writeText.mock.calls[0][0]).toBe("First body");
    expect(byTestId("agentHelpTips.copyMenu")).toBeNull();
  });

  it("a clipboard rejection dismisses the menu and keeps the window open", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    stubSelection(() => "First body");
    const writeText = vi.fn(() => Promise.reject(new Error("denied")));
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    mount({ entry: ENTRY });
    rightClickBody();
    byTestId<HTMLButtonElement>("agentHelpTips.copyMenuItem")!.click();
    await flushClipboard();
    expect(error).toHaveBeenCalledTimes(1);
    expect(byTestId("agentHelpTips.modal")).toBeTruthy();
    expect(byTestId("agentHelpTips.copyMenu")).toBeNull();
  });

  it("Escape closes the menu first, then the window", () => {
    stubSelection(() => "First body");
    const onClose = vi.fn();
    mount({ entry: ENTRY, onClose });
    rightClickBody();
    expect(byTestId("agentHelpTips.copyMenu")).toBeTruthy();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    expect(byTestId("agentHelpTips.copyMenu")).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("Ctrl+C is not intercepted", () => {
    mount({ entry: ENTRY });
    const reached = vi.fn();
    document.addEventListener("keydown", reached);
    try {
      const event = new KeyboardEvent("keydown", { key: "c", ctrlKey: true, bubbles: true, cancelable: true });
      document.querySelector(".agent-help-tips-body")!.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(false);
      expect(reached).toHaveBeenCalledTimes(1);
    } finally {
      document.removeEventListener("keydown", reached);
    }
  });

  it("no tips renders the empty line", () => {
    mount({ entry: null });
    expect(byTestId("agentHelpTips.modal")?.textContent).toContain("No tips here yet.");
    expect(byTestId("agentHelpTips.localError")).toBeNull();
  });

  it("a multi-line tip body is rendered as text", () => {
    const body = "line one\nline two <b>x</b>";
    mount({ entry: { tips: [{ title: "Raw", body }] } });
    const rendered = document.querySelector(".agent-help-tips-tip-body");
    expect(rendered?.textContent).toBe(body);
    expect(document.querySelector(".agent-help-tips-tip-body b")).toBeNull();
    expect(byTestId("agentHelpTips.modal")?.innerHTML).not.toContain("<b>");
  });

  it("a junk tips value renders nothing and does not throw", () => {
    const junk: unknown[] = [null, [null], [{ title: 7, body: "numeric title" }]];
    for (const tips of junk) {
      expect(() => mount({ entry: { tips } as unknown as AgentHelpEntry })).not.toThrow();
      expect(tipSections()).toHaveLength(0);
      expect(byTestId("agentHelpTips.modal")?.textContent).toContain("No tips here yet.");
      dispose?.();
      dispose = null;
      document.body.replaceChildren();
    }
  });

  it("Tab wraps from the last control to the first tip link", async () => {
    mount({ entry: ENTRY });
    await Promise.resolve();
    const [link] = document.querySelectorAll<HTMLAnchorElement>(".agent-help-tips-tip-link");
    const close = byTestId<HTMLButtonElement>("agentHelpTips.close")!;
    expect(document.activeElement).toBe(close);

    const tab = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    document.dispatchEvent(tab);
    expect(tab.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(link);

    // Inside the list the browser moves focus itself; the trap stays out of the way.
    link.focus();
    const inner = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    document.dispatchEvent(inner);
    expect(inner.defaultPrevented).toBe(false);
  });

  it("Shift-Tab wraps from the first tip link to the last control", async () => {
    mount({ entry: ENTRY });
    await Promise.resolve();
    const [link, second] = document.querySelectorAll<HTMLAnchorElement>(".agent-help-tips-tip-link");
    const close = byTestId<HTMLButtonElement>("agentHelpTips.close")!;
    expect(second).toBeTruthy();

    link.focus();
    const back = new KeyboardEvent("keydown", {
      key: "Tab",
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    });
    document.dispatchEvent(back);
    expect(back.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(close);

    // Shift-Tab from the second link goes back to the first by default, not to Close.
    second.focus();
    const inner = new KeyboardEvent("keydown", {
      key: "Tab",
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    });
    document.dispatchEvent(inner);
    expect(inner.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(second);
  });
});
