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
});
