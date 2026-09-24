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
    { title: "Second tip", body: "Second body" },
  ],
};

function byTestId<T extends HTMLElement = HTMLElement>(testId: string): T | null {
  return document.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function tipSections(): NodeListOf<HTMLElement> {
  return document.querySelectorAll<HTMLElement>(".agent-help-tips-tip");
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

  it("the copy button writes the tips as text", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    mount({ entry: ENTRY });
    byTestId<HTMLButtonElement>("agentHelpTips.copy")!.click();
    await Promise.resolve();
    await Promise.resolve();
    expect(writeText).toHaveBeenCalledTimes(1);
    const text = (writeText.mock.calls[0] as unknown as [string])[0];
    expect(text).toContain("First tip");
    expect(text).toContain("Second tip");
    expect(byTestId("agentHelpTips.copy")?.textContent).toBe("Copied");
  });

  it("a clipboard rejection keeps the window open", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const writeText = vi.fn(() => Promise.reject(new Error("denied")));
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    mount({ entry: ENTRY });
    byTestId<HTMLButtonElement>("agentHelpTips.copy")!.click();
    await Promise.resolve();
    await Promise.resolve();
    expect(error).toHaveBeenCalledTimes(1);
    expect(byTestId("agentHelpTips.modal")).toBeTruthy();
    expect(byTestId("agentHelpTips.copy")?.textContent).toBe("Copy");
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
    const link = document.querySelector<HTMLAnchorElement>(".agent-help-tips-tip-link")!;
    const copy = byTestId<HTMLButtonElement>("agentHelpTips.copy")!;
    const close = byTestId<HTMLButtonElement>("agentHelpTips.close")!;
    expect(document.activeElement).toBe(close);

    const tab = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    document.dispatchEvent(tab);
    expect(tab.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(link);

    // Inside the list the browser moves focus itself; the trap stays out of the way.
    copy.focus();
    const inner = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    document.dispatchEvent(inner);
    expect(inner.defaultPrevented).toBe(false);
  });

  it("Shift-Tab wraps from the first tip link to the last control", async () => {
    mount({ entry: ENTRY });
    await Promise.resolve();
    const link = document.querySelector<HTMLAnchorElement>(".agent-help-tips-tip-link")!;
    const copy = byTestId<HTMLButtonElement>("agentHelpTips.copy")!;
    const close = byTestId<HTMLButtonElement>("agentHelpTips.close")!;

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

    // Shift-Tab from Copy goes back to the link by default, not to Close.
    copy.focus();
    const inner = new KeyboardEvent("keydown", {
      key: "Tab",
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    });
    document.dispatchEvent(inner);
    expect(inner.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(copy);
  });

  it("a copy that resolves after close arms no timer", async () => {
    let resolveWrite: () => void = () => {};
    const writeText = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveWrite = resolve;
        }),
    );
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    mount({ entry: ENTRY });
    const copy = byTestId<HTMLButtonElement>("agentHelpTips.copy")!;
    copy.click();
    expect(writeText).toHaveBeenCalledTimes(1);

    dispose?.();
    dispose = null;
    const setTimeoutSpy = vi.spyOn(globalThis, "setTimeout");
    resolveWrite();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    expect(setTimeoutSpy.mock.calls.some((call) => call[1] === 1500)).toBe(false);
    expect(copy.textContent).toBe("Copy");
  });
});
