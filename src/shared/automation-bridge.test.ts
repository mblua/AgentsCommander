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
): UiAutomationRequest {
  return {
    requestId: `request-${action}-${selector}`,
    token: "token",
    window: "main",
    action,
    selector,
    value,
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
