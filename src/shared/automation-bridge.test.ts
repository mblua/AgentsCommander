// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { executeAutomationRequest } from "./automation-bridge";
import type { UiAutomationAction, UiAutomationRequest } from "./types";

vi.mock("./ipc", () => ({
  AutomationAPI: {
    complete: vi.fn(() => Promise.resolve()),
    enabled: vi.fn(() => Promise.resolve(false)),
    frontendReady: vi.fn(() => Promise.resolve()),
  },
  onUiAutomationRequest: vi.fn(() => Promise.resolve(() => {})),
}));

let topmostElement: Element | null = null;

function request(
  action: UiAutomationAction,
  selector: string,
  value?: string,
  expiresAtUnixMs?: number | null,
): UiAutomationRequest {
  return {
    requestId: `request-${action}-${selector}`,
    token: "token",
    window: "main",
    action,
    selector,
    value,
    expiresAtUnixMs,
  };
}

function domRect(x = 10, y = 20, width = 120, height = 30): DOMRect {
  return {
    x,
    y,
    width,
    height,
    top: y,
    right: x + width,
    bottom: y + height,
    left: x,
    toJSON: () => ({}),
  } as DOMRect;
}

function domRectList(rect: DOMRect): DOMRectList {
  const list = [rect] as unknown as DOMRectList;
  Object.defineProperty(list, "item", {
    configurable: true,
    value: (index: number) => list[index] ?? null,
  });
  return list;
}

function makeVisible<T extends HTMLElement>(element: T, rect = domRect()): T {
  Object.defineProperty(element, "getBoundingClientRect", {
    configurable: true,
    value: () => rect,
  });
  Object.defineProperty(element, "getClientRects", {
    configurable: true,
    value: () => domRectList(rect),
  });
  return element;
}

function addTarget<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  testId: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  makeVisible(element);
  element.setAttribute("data-ac-testid", testId);
  if (text) element.textContent = text;
  document.body.append(element);
  return element;
}

describe("automation bridge", () => {
  beforeEach(() => {
    topmostElement = null;
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => topmostElement),
    });
  });

  afterEach(() => {
    document.body.innerHTML = "";
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("queries a visible button target", async () => {
    const button = addTarget("button", "onboarding.confirm", "Set up Coding Agent");
    button.setAttribute("data-ac-role", "button");
    button.setAttribute("data-ac-state", "ready");

    const response = await executeAutomationRequest(
      "main",
      request("query", "onboarding.confirm"),
    );

    expect(response.ok).toBe(true);
    if (!response.ok) throw new Error(response.message);
    expect(response.target).toMatchObject({
      testId: "onboarding.confirm",
      role: "button",
      state: "ready",
      text: "Set up Coding Agent",
      visible: true,
      disabled: false,
    });
  });

  it("clicks a visible enabled target", async () => {
    const button = addTarget("button", "onboarding.agentPreset.codex", "Codex");
    topmostElement = button;
    const onClick = vi.fn();
    button.addEventListener("click", onClick);

    const response = await executeAutomationRequest(
      "main",
      request("click", "onboarding.agentPreset.codex"),
    );

    expect(response.ok).toBe(true);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("dispatches contextmenu on a visible enabled target", async () => {
    const rect = domRect(40, 60, 200, 80);
    const header = addTarget("div", "project.loops.header.test", "Loops");
    makeVisible(header, rect);
    topmostElement = header;
    const onContextMenu = vi.fn((event: MouseEvent) => {
      event.preventDefault();
      addTarget("button", "loop.action.new.test", "New loop");
    });
    header.addEventListener("contextmenu", onContextMenu);

    const response = await executeAutomationRequest(
      "main",
      request("contextClick", "project.loops.header.test"),
    );
    const menuResponse = await executeAutomationRequest(
      "main",
      request("query", "loop.action.new.test"),
    );

    if (!response.ok) throw new Error(response.message);
    expect(response.ok).toBe(true);
    expect(onContextMenu).toHaveBeenCalledTimes(1);
    const event = onContextMenu.mock.calls[0]?.[0];
    if (!event) throw new Error("expected contextmenu event");
    expect(event).toBeInstanceOf(MouseEvent);
    expect(event.button).toBe(2);
    expect(event.buttons).toBe(2);
    expect(event.clientX).toBe(140);
    expect(event.clientY).toBe(100);
    expect(event.defaultPrevented).toBe(true);
    expect(menuResponse.ok).toBe(true);
  });

  it("dispatches contextmenu on a disabled loop row domain state", async () => {
    const row = addTarget("div", "loop.row.test.weekday-standup", "Weekday standup");
    row.setAttribute("data-ac-state", "loop-disabled");
    topmostElement = row;
    const onContextMenu = vi.fn((event: MouseEvent) => {
      event.preventDefault();
      addTarget("button", "loop.action.toggle.test.weekday-standup", "Enable");
    });
    row.addEventListener("contextmenu", onContextMenu);

    const response = await executeAutomationRequest(
      "main",
      request("contextClick", "loop.row.test.weekday-standup"),
    );
    const menuResponse = await executeAutomationRequest(
      "main",
      request("query", "loop.action.toggle.test.weekday-standup"),
    );

    if (!response.ok) throw new Error(response.message);
    expect(response.ok).toBe(true);
    expect(onContextMenu).toHaveBeenCalledTimes(1);
    expect(menuResponse.ok).toBe(true);
  });

  it("waits for click-driven dynamic targets before completing", async () => {
    const button = addTarget("button", "settings.agent.addCustom", "+ Custom Agent");
    topmostElement = button;
    button.addEventListener("click", () => {
      void Promise.resolve().then(() => {
        addTarget("div", "settings.agentRow.1", "New Agent");
        addTarget("input", "settings.agentRow.1.label");
        addTarget("button", "settings.agentRow.1.remove", "Remove agent");
      });
    });

    const clickResponse = await executeAutomationRequest(
      "main",
      request("click", "settings.agent.addCustom"),
    );
    const rowResponse = await executeAutomationRequest(
      "main",
      request("query", "settings.agentRow.1"),
    );
    const labelResponse = await executeAutomationRequest(
      "main",
      request("query", "settings.agentRow.1.label"),
    );
    const removeResponse = await executeAutomationRequest(
      "main",
      request("query", "settings.agentRow.1.remove"),
    );

    expect(clickResponse.ok).toBe(true);
    expect(rowResponse.ok).toBe(true);
    expect(labelResponse.ok).toBe(true);
    expect(removeResponse.ok).toBe(true);
  });

  it("queries settings row parents and disabled preset targets", async () => {
    const row = addTarget("div", "settings.agentRow.0");
    row.setAttribute("data-ac-role", "group");
    const preset = addTarget("button", "settings.agentPreset.codex", "+ Codex");
    preset.setAttribute("data-ac-role", "button");
    preset.setAttribute("data-ac-state", "disabled");
    preset.disabled = true;

    const rowResponse = await executeAutomationRequest(
      "main",
      request("query", "settings.agentRow.0"),
    );
    const presetResponse = await executeAutomationRequest(
      "main",
      request("query", "settings.agentPreset.codex"),
    );

    expect(rowResponse.ok).toBe(true);
    expect(presetResponse.ok).toBe(true);
    if (!presetResponse.ok) throw new Error(presetResponse.message);
    expect(presetResponse.target).toMatchObject({
      testId: "settings.agentPreset.codex",
      disabled: true,
      state: "disabled",
    });
  });

  it("briefly retries queries for asynchronously rendered targets", async () => {
    window.setTimeout(() => {
      addTarget("button", "settings.agent.addCustom", "+ Custom Agent");
    }, 10);

    const response = await executeAutomationRequest(
      "main",
      request("query", "settings.agent.addCustom"),
    );

    expect(response.ok).toBe(true);
    if (!response.ok) throw new Error(response.message);
    expect(response.target.testId).toBe("settings.agent.addCustom");
  });

  it("does not return a retry query success after the request expires", async () => {
    vi.useFakeTimers();
    window.setTimeout(() => {
      addTarget("button", "settings.agent.late", "Late target");
    }, 10);

    const responsePromise = executeAutomationRequest(
      "main",
      request("query", "settings.agent.late", undefined, Date.now() + 5),
    );

    await vi.advanceTimersByTimeAsync(5);
    const response = await responsePromise;

    expect(response.ok).toBe(false);
    if (response.ok) throw new Error("expected timeout");
    expect(response.error).toBe("timeout");

    await vi.advanceTimersByTimeAsync(5);
  });

  it("sets input values and dispatches input plus change", async () => {
    const input = addTarget("input", "onboarding.custom.command");
    topmostElement = input;
    const onInput = vi.fn();
    const onChange = vi.fn();
    input.addEventListener("input", onInput);
    input.addEventListener("change", onChange);

    const response = await executeAutomationRequest(
      "main",
      request("setValue", "onboarding.custom.command", "codex"),
    );

    expect(response.ok).toBe(true);
    expect(input.value).toBe("codex");
    expect(onInput).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it("does not click when the request expires before mutation", async () => {
    const button = addTarget("button", "expired.click", "Expired");
    topmostElement = button;
    const onClick = vi.fn();
    const focus = vi.spyOn(button, "focus");
    button.addEventListener("click", onClick);

    const response = await executeAutomationRequest(
      "main",
      request("click", "expired.click", undefined, Date.now() - 1),
    );

    expect(response.ok).toBe(false);
    if (response.ok) throw new Error("expected timeout");
    expect(response.error).toBe("timeout");
    expect(response.diagnostics?.expiresAtUnixMs).toBeLessThanOrEqual(Date.now());
    expect(response.available?.map((target) => target.testId)).toContain("expired.click");
    expect(onClick).not.toHaveBeenCalled();
    expect(focus).not.toHaveBeenCalled();
  });

  it("does not context-click when the request expires before mutation", async () => {
    const target = addTarget("div", "expired.context", "Expired");
    topmostElement = target;
    const onContextMenu = vi.fn();
    const focus = vi.spyOn(target, "focus");
    target.addEventListener("contextmenu", onContextMenu);

    const response = await executeAutomationRequest(
      "main",
      request("contextClick", "expired.context", undefined, Date.now() - 1),
    );

    expect(response.ok).toBe(false);
    if (response.ok) throw new Error("expected timeout");
    expect(response.error).toBe("timeout");
    expect(response.diagnostics?.expiresAtUnixMs).toBeLessThanOrEqual(Date.now());
    expect(response.available?.map((availableTarget) => availableTarget.testId)).toContain(
      "expired.context",
    );
    expect(onContextMenu).not.toHaveBeenCalled();
    expect(focus).not.toHaveBeenCalled();
  });

  it("does not set values when the request expires before mutation", async () => {
    const input = addTarget("input", "expired.set");
    topmostElement = input;
    const onInput = vi.fn();
    const onChange = vi.fn();
    const focus = vi.spyOn(input, "focus");
    input.value = "before";
    input.addEventListener("input", onInput);
    input.addEventListener("change", onChange);

    const response = await executeAutomationRequest(
      "main",
      request("setValue", "expired.set", "after", Date.now() - 1),
    );

    expect(response.ok).toBe(false);
    if (response.ok) throw new Error("expected timeout");
    expect(response.error).toBe("timeout");
    expect(response.diagnostics?.expiresAtUnixMs).toBeLessThanOrEqual(Date.now());
    expect(input.value).toBe("before");
    expect(onInput).not.toHaveBeenCalled();
    expect(onChange).not.toHaveBeenCalled();
    expect(focus).not.toHaveBeenCalled();
  });

  it("reports missing selectors with available targets", async () => {
    addTarget("button", "onboarding.modal", "Welcome");

    const response = await executeAutomationRequest(
      "main",
      request("query", "onboarding.agentPreset.codex"),
    );

    expect(response.ok).toBe(false);
    if (response.ok) throw new Error("expected missing_selector");
    expect(response.error).toBe("missing_selector");
    expect(response.available?.map((target) => target.testId)).toContain("onboarding.modal");
  });

  it("reports duplicate selectors and does not click either target", async () => {
    const first = addTarget("button", "duplicate.target", "First");
    const second = addTarget("button", "duplicate.target", "Second");
    topmostElement = first;
    const onFirstClick = vi.fn();
    const onSecondClick = vi.fn();
    first.addEventListener("click", onFirstClick);
    second.addEventListener("click", onSecondClick);

    const response = await executeAutomationRequest(
      "main",
      request("click", "duplicate.target"),
    );

    expect(response.ok).toBe(false);
    if (response.ok) throw new Error("expected duplicate_selector");
    expect(response.error).toBe("duplicate_selector");
    expect(response.available).toHaveLength(2);
    expect(onFirstClick).not.toHaveBeenCalled();
    expect(onSecondClick).not.toHaveBeenCalled();
  });

  it("reports hidden targets", async () => {
    const button = addTarget("button", "hidden.target", "Hidden");
    button.style.visibility = "hidden";

    const response = await executeAutomationRequest(
      "main",
      request("query", "hidden.target"),
    );

    expect(response.ok).toBe(false);
    if (response.ok) throw new Error("expected target_hidden");
    expect(response.error).toBe("target_hidden");
  });

  it("reports disabled action targets", async () => {
    const button = addTarget("button", "disabled.target", "Disabled");
    button.disabled = true;
    topmostElement = button;

    const response = await executeAutomationRequest(
      "main",
      request("click", "disabled.target"),
    );

    expect(response.ok).toBe(false);
    if (response.ok) throw new Error("expected target_disabled");
    expect(response.error).toBe("target_disabled");
  });

  it("reports data-ac-state disabled action targets", async () => {
    const button = addTarget("button", "state.disabled.target", "Disabled");
    button.setAttribute("data-ac-state", "disabled");
    topmostElement = button;

    const response = await executeAutomationRequest(
      "main",
      request("click", "state.disabled.target"),
    );

    expect(response.ok).toBe(false);
    if (response.ok) throw new Error("expected target_disabled");
    expect(response.error).toBe("target_disabled");
  });

  it("reports obscured action targets", async () => {
    addTarget("button", "covered.target", "Covered");
    const blocker = addTarget("div", "dialog.blocker", "Modal");
    blocker.setAttribute("data-ac-role", "dialog");
    topmostElement = blocker;

    const response = await executeAutomationRequest(
      "main",
      request("click", "covered.target"),
    );

    expect(response.ok).toBe(false);
    if (response.ok) throw new Error("expected target_obscured");
    expect(response.error).toBe("target_obscured");
    expect(response.diagnostics?.topmost?.testId).toBe("dialog.blocker");
  });

  it("finds targets outside #root for portal-rendered surfaces", async () => {
    const root = document.createElement("div");
    root.id = "root";
    document.body.append(root);
    const menuItem = addTarget("button", "menu.session.restart", "Restart Session");

    const response = await executeAutomationRequest(
      "main",
      request("query", "menu.session.restart"),
    );

    expect(response.ok).toBe(true);
    if (!response.ok) throw new Error(response.message);
    expect(response.target.testId).toBe("menu.session.restart");
    expect(root.contains(menuItem)).toBe(false);
  });

  it("includes text for telemetry roles used by automation assertions", async () => {
    const metric = addTarget("div", "resourceMonitor.summary.network", "Network Unknown");
    metric.setAttribute("data-ac-role", "metric");
    const group = addTarget("div", "resourceMonitor.group.session-1", "cap-one running");
    group.setAttribute("data-ac-role", "group");

    const response = await executeAutomationRequest(
      "resource-monitor",
      request("query", "resourceMonitor.summary.network"),
    );

    expect(response.ok).toBe(true);
    if (!response.ok) throw new Error(response.message);
    expect(response.target.text).toBe("Network Unknown");

    const groupResponse = await executeAutomationRequest(
      "resource-monitor",
      request("query", "resourceMonitor.group.session-1"),
    );

    expect(groupResponse.ok).toBe(true);
    if (!groupResponse.ok) throw new Error(groupResponse.message);
    expect(groupResponse.target.text).toBe("cap-one running");
  });

  it("traverses open shadow roots and redacts input text", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const shadow = host.attachShadow({ mode: "open" });
    const input = makeVisible(document.createElement("input"));
    input.setAttribute("data-ac-testid", "shadow.secret");
    input.value = "AIza123456789012345678901234567890";
    shadow.append(input);

    const response = await executeAutomationRequest(
      "main",
      request("query", "shadow.secret"),
    );

    expect(response.ok).toBe(true);
    if (!response.ok) throw new Error(response.message);
    expect(response.target.testId).toBe("shadow.secret");
    expect(response.target.text).toBe("");
  });
});
