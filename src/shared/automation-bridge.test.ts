// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { executeAutomationRequest, resetAutomationBridgeForTests } from "./automation-bridge";
import type { UiAutomationAction, UiAutomationRequest, UiAutomationResponse } from "./types";

const terminalAutomation = vi.hoisted(() => ({
  execute: vi.fn(),
  reset: vi.fn(),
}));

vi.mock("./ipc", () => ({
  AutomationAPI: {
    complete: vi.fn(() => Promise.resolve()),
    enabled: vi.fn(() => Promise.resolve(false)),
    frontendReady: vi.fn(() => Promise.resolve()),
    executeTerminalController: terminalAutomation.execute,
    resetTerminalControllerForTests: terminalAutomation.reset,
  },
  onUiAutomationRequest: vi.fn(() => Promise.resolve(() => {})),
}));

let topmostElement: Element | null = null;

function request<A extends UiAutomationAction>(
  action: A,
  selector: string,
  value?: string,
  expiresAtUnixMs?: number | null,
): UiAutomationRequest<A> {
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

function addTerminalTarget(sessionId = "session-a"): HTMLDivElement {
  const target = addTarget("div", `terminal.session.${sessionId}`);
  target.setAttribute("data-ac-session-id", sessionId);
  target.setAttribute("data-ac-role", "surface");
  topmostElement = target;
  return target;
}

/** #944 - a visible automation target nested under `parent` (the chain tests need
 *  ancestors, and `addTarget` always appends to <body>). */
function nestTarget<K extends keyof HTMLElementTagNameMap>(
  parent: HTMLElement,
  tag: K,
  testId: string,
): HTMLElementTagNameMap[K] {
  const element = makeVisible(document.createElement(tag));
  element.setAttribute("data-ac-testid", testId);
  parent.append(element);
  return element;
}

/** #944 - the full ten-event superset: `pointermove` / `mousemove` are in here ON
 *  PURPOSE even though `hover` must never dispatch them, so that re-adding the move
 *  pair (plan R1) turns A1 / A3 / A12 red instead of passing silently. */
const HOVER_EVENTS = [
  "pointerout",
  "pointerleave",
  "mouseout",
  "mouseleave",
  "pointerover",
  "pointerenter",
  "mouseover",
  "mouseenter",
  "pointermove",
  "mousemove",
] as const;

const listenerCleanups: Array<() => void> = [];

/** Record dispatches in order. Listeners on document/body/html outlive
 *  `document.body.innerHTML = ""`, so every one is tracked and removed in afterEach. */
function recordOn(
  element: HTMLElement,
  log: string[],
  types: readonly string[] = HOVER_EVENTS,
  id?: string,
): void {
  for (const type of types) {
    const handler = () => log.push(id ? `${type}:${id}` : type);
    element.addEventListener(type, handler);
    listenerCleanups.push(() => element.removeEventListener(type, handler));
  }
}

function hoverOf(response: UiAutomationResponse) {
  if (!response.ok) throw new Error(`${response.error}: ${response.message}`);
  const hover = response.diagnostics?.hover;
  if (!hover) throw new Error("expected hover diagnostics");
  return hover;
}

describe("automation bridge", () => {
  beforeEach(() => {
    topmostElement = null;
    // #944 - LOAD-BEARING, not hygiene. The sticky pointer is module scope and
    // `document.body.innerHTML = ""` below detaches the hovered node without
    // clearing it, so without this the next test's first hover sees a detached
    // `from` and takes the staleFrom branch: A13 would fail on test ORDER alone.
    // beforeEach (not afterEach) because it is self-defending.
    resetAutomationBridgeForTests();
    terminalAutomation.execute.mockReset().mockReturnValue(null);
    terminalAutomation.reset.mockClear();
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => topmostElement),
    });
  });

  afterEach(() => {
    while (listenerCleanups.length > 0) listenerCleanups.pop()!();
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

  it("returns allow-listed data-ac metadata without value-like attributes", async () => {
    const source = addTarget("span", "settings.agentRow.0.envRow.0.source", "user");
    source.setAttribute("data-ac-role", "status");
    source.setAttribute("data-ac-env-source", "user");
    source.setAttribute("data-ac-agent-id", "codex");
    source.setAttribute("data-ac-agent-index", "0");
    source.setAttribute("data-ac-agent-command", "codex");
    source.setAttribute("data-ac-profile-letter", "B");
    source.setAttribute("data-ac-requested-profile", "B");
    source.setAttribute("data-ac-effective-profile", "A");
    source.setAttribute("data-ac-configured", "false");
    source.setAttribute("data-ac-value", "AIza123456789012345678901234567890");
    source.setAttribute("data-ac-token", "abcdefghijklmnopqrstuvwxyz1234567890");

    const response = await executeAutomationRequest(
      "main",
      request("query", "settings.agentRow.0.envRow.0.source"),
    );

    expect(response.ok).toBe(true);
    if (!response.ok) throw new Error(response.message);
    expect(response.target.text).toBe("");
    expect(response.target.metadata).toEqual({
      agentId: "codex",
      agentIndex: "0",
      agentCommand: "codex",
      profileLetter: "B",
      requestedProfile: "B",
      effectiveProfile: "A",
      configured: "false",
      envSource: "user",
    });
    expect(response.target.metadata).not.toHaveProperty("value");
    expect(response.target.metadata).not.toHaveProperty("token");
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

  it("types text through textareas for terminal automation input", async () => {
    const textarea = makeVisible(document.createElement("textarea"));
    textarea.setAttribute("data-ac-testid", "terminal.input");
    document.body.append(textarea);
    topmostElement = textarea;

    const response = await executeAutomationRequest(
      "terminal",
      request("typeText", "terminal.input", "status\n"),
    );

    expect(response.ok).toBe(true);
    if (!response.ok) throw new Error(response.message);
    expect(textarea.value).toBe("status\n");
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

  describe("terminal automation", () => {
    const metrics = {
      sessionId: "session-a",
      baseY: 40,
      viewportY: 0,
      length: 64,
      cols: 122,
      rows: 39,
      type: "normal" as const,
      atBottom: false,
    };

    it.each([
      ["query", { kind: "query" }],
      ["top", { kind: "top" }],
      ["bottom", { kind: "bottom" }],
      ["line:0", { kind: "line", value: 0 }],
      ["line:2147483647", { kind: "line", value: 2147483647 }],
      ["lines:-2147483648", { kind: "lines", value: -2147483648 }],
      ["lines:2147483647", { kind: "lines", value: 2147483647 }],
      ["pages:-2147483648", { kind: "pages", value: -2147483648 }],
      ["pages:2147483647", { kind: "pages", value: 2147483647 }],
    ] as const)("routes canonical operation %s and returns exact metrics", async (value, operation) => {
      const target = addTerminalTarget();
      terminalAutomation.execute.mockReturnValue({ ok: true, target: metrics });

      const response = await executeAutomationRequest(
        "main",
        request("terminal", "terminal.session.session-a", value),
      );

      expect(terminalAutomation.execute).toHaveBeenCalledWith({
        element: target,
        sessionId: "session-a",
        operation,
      });
      expect(response).toMatchObject({
        ok: true,
        action: "terminal",
        selector: "terminal.session.session-a",
        target: metrics,
      });
      target.remove();
      terminalAutomation.execute.mockReset();
    });

    it.each([
      null,
      "",
      "QUERY",
      " query",
      "query ",
      "line:",
      "line:+1",
      "line:-1",
      "line:01",
      "line:1.0",
      "line:1e2",
      "line:1:2",
      "line:2147483648",
      "lines:+1",
      "lines:01",
      "lines:-0",
      "lines:1.0",
      "lines:1e2",
      "lines:-2147483649",
      "pages:2147483648",
      "unknown",
    ])("rejects noncanonical terminal value %j before controller execution", async (value) => {
      addTerminalTarget();
      const terminalRequest = request(
        "terminal",
        "terminal.session.session-a",
        value ?? undefined,
      );
      if (value === null) terminalRequest.value = null;

      const response = await executeAutomationRequest("main", terminalRequest);

      expect(response.ok).toBe(false);
      if (response.ok) throw new Error("expected value_not_supported");
      expect(response.error).toBe("value_not_supported");
      expect(terminalAutomation.execute).not.toHaveBeenCalled();
    });

    it("checks expiry immediately before terminal controller execution", async () => {
      addTerminalTarget();
      const now = vi.spyOn(Date, "now");
      now.mockReturnValueOnce(100).mockReturnValueOnce(100).mockReturnValue(101);

      const response = await executeAutomationRequest(
        "main",
        request("terminal", "terminal.session.session-a", "top", 101),
      );

      expect(response.ok).toBe(false);
      if (response.ok) throw new Error("expected timeout");
      expect(response.error).toBe("timeout");
      expect(terminalAutomation.execute).not.toHaveBeenCalled();
    });

    it("reuses missing, duplicate, hidden, disabled, and obscured DOM failures", async () => {
      const missing = await executeAutomationRequest(
        "main",
        request("terminal", "terminal.session.session-a", "query"),
      );
      expect(missing.ok ? null : missing.error).toBe("missing_selector");

      const first = addTerminalTarget();
      addTerminalTarget();
      const duplicate = await executeAutomationRequest(
        "main",
        request("terminal", "terminal.session.session-a", "query"),
      );
      expect(duplicate.ok ? null : duplicate.error).toBe("duplicate_selector");

      document.body.innerHTML = "";
      const hiddenTarget = addTerminalTarget();
      hiddenTarget.style.visibility = "hidden";
      const hidden = await executeAutomationRequest(
        "main",
        request("terminal", "terminal.session.session-a", "query"),
      );
      expect(hidden.ok ? null : hidden.error).toBe("target_hidden");

      document.body.innerHTML = "";
      const disabledTarget = addTerminalTarget();
      disabledTarget.setAttribute("aria-disabled", "true");
      const disabled = await executeAutomationRequest(
        "main",
        request("terminal", "terminal.session.session-a", "query"),
      );
      expect(disabled.ok ? null : disabled.error).toBe("target_disabled");

      document.body.innerHTML = "";
      addTerminalTarget();
      topmostElement = addTarget("div", "blocking.target");
      const obscured = await executeAutomationRequest(
        "main",
        request("terminal", "terminal.session.session-a", "query"),
      );
      expect(obscured.ok ? null : obscured.error).toBe("target_obscured");
      expect(terminalAutomation.execute).not.toHaveBeenCalled();
      first.remove();
    });

    it.each([
      ["invalid prefix", "terminal.other.session-a", "session-a"],
      ["empty session", "terminal.session.", ""],
      ["control session", "terminal.session.bad\u0085", "bad\u0085"],
      ["mismatched metadata", "terminal.session.session-a", "session-b"],
    ])("rejects %s as terminal_target_mismatch", async (_label, selector, metadata) => {
      const target = addTarget("div", selector);
      target.setAttribute("data-ac-session-id", metadata);
      topmostElement = target;

      const response = await executeAutomationRequest(
        "main",
        request("terminal", selector, "query"),
      );

      expect(response.ok).toBe(false);
      if (response.ok) throw new Error("expected terminal_target_mismatch");
      expect(response.error).toBe("terminal_target_mismatch");
      expect(terminalAutomation.execute).not.toHaveBeenCalled();
    });

    it("rejects session ids longer than 256 UTF-8 bytes", async () => {
      const sessionId = "é".repeat(129);
      addTerminalTarget(sessionId);
      const response = await executeAutomationRequest(
        "main",
        request("terminal", `terminal.session.${sessionId}`, "query"),
      );
      expect(response.ok ? null : response.error).toBe("terminal_target_mismatch");
      expect(terminalAutomation.execute).not.toHaveBeenCalled();
    });

    it.each([
      "terminal_target_mismatch",
      "terminal_entry_stale",
      "terminal_session_not_visible",
    ] as const)("copies controller error %s unchanged", async (error) => {
      addTerminalTarget();
      terminalAutomation.execute.mockReturnValue({ ok: false, error, message: error });

      const response = await executeAutomationRequest(
        "main",
        request("terminal", "terminal.session.session-a", "query"),
      );

      expect(response.ok).toBe(false);
      if (response.ok) throw new Error(`expected ${error}`);
      expect(response.error).toBe(error);
      expect(response.message).toBe(error);
    });

    it("maps controller absence and thrown exceptions", async () => {
      addTerminalTarget();
      const absent = await executeAutomationRequest(
        "main",
        request("terminal", "terminal.session.session-a", "query"),
      );
      expect(absent.ok ? null : absent.error).toBe("terminal_controller_unavailable");

      terminalAutomation.execute.mockImplementation(() => {
        throw new Error("terminal exploded");
      });
      const thrown = await executeAutomationRequest(
        "main",
        request("terminal", "terminal.session.session-a", "query"),
      );
      expect(thrown.ok ? null : thrown.error).toBe("automation_bridge_exception");
      expect(thrown.ok ? "" : thrown.message).toBe("terminal exploded");
    });

    it("does not focus, click, dispatch DOM input, or settle a terminal operation", async () => {
      const target = addTerminalTarget();
      terminalAutomation.execute.mockReturnValue({ ok: true, target: metrics });
      const focus = vi.spyOn(HTMLElement.prototype, "focus");
      const click = vi.spyOn(HTMLElement.prototype, "click");
      const dispatch = vi.spyOn(target, "dispatchEvent");
      const timeout = vi.spyOn(window, "setTimeout");

      const response = await executeAutomationRequest(
        "main",
        request("terminal", "terminal.session.session-a", "top"),
      );

      expect(response.ok).toBe(true);
      expect(focus).not.toHaveBeenCalled();
      expect(click).not.toHaveBeenCalled();
      expect(dispatch).not.toHaveBeenCalled();
      expect(timeout).not.toHaveBeenCalled();
    });

    it("bridge reset clears the terminal controller seam", () => {
      terminalAutomation.reset.mockClear();
      resetAutomationBridgeForTests();
      expect(terminalAutomation.reset).toHaveBeenCalledTimes(1);
    });
  });

  // #944 - hover. Eight events, sticky, focus-free, click-free, move-free.
  describe("hover", () => {
    // A1
    it("dispatches the full boundary sequence, every leave before every enter", async () => {
      const log: string[] = [];
      const a = addTarget("button", "hover.a", "A");
      const b = addTarget("button", "hover.b", "B");
      const outs: MouseEvent[] = [];
      const overs: MouseEvent[] = [];
      a.addEventListener("mouseout", (event) => outs.push(event as MouseEvent));
      b.addEventListener("mouseover", (event) => overs.push(event as MouseEvent));
      recordOn(a, log);
      recordOn(b, log);

      topmostElement = a;
      await executeAutomationRequest("main", request("hover", "hover.a"));
      log.length = 0;

      topmostElement = b;
      const response = await executeAutomationRequest("main", request("hover", "hover.b"));

      const expected = [
        "pointerout",
        "pointerleave",
        "mouseout",
        "mouseleave",
        "pointerover",
        "pointerenter",
        "mouseover",
        "mouseenter",
      ];
      expect(hoverOf(response).events).toEqual(expected);
      expect(log).toEqual(expected); // what the DOM actually saw, in order
      expect(hoverOf(response)).toMatchObject({ from: "hover.a", to: "hover.b", changed: true });
      expect(outs[0]?.relatedTarget).toBe(b);
      expect(overs[0]?.relatedTarget).toBe(a);
    });

    // A2
    it("does not focus, does not click, and dispatches no move events", async () => {
      const button = addTarget("button", "hover.inert", "Inert");
      topmostElement = button;
      const onClick = vi.fn();
      const onMouseDown = vi.fn();
      const onPointerMove = vi.fn();
      const onMouseMove = vi.fn();
      const focus = vi.spyOn(button, "focus");
      button.addEventListener("click", onClick);
      button.addEventListener("mousedown", onMouseDown);
      button.addEventListener("pointermove", onPointerMove);
      button.addEventListener("mousemove", onMouseMove);

      const response = await executeAutomationRequest("main", request("hover", "hover.inert"));

      expect(response.ok).toBe(true);
      expect(onClick).not.toHaveBeenCalled();
      expect(onMouseDown).not.toHaveBeenCalled();
      expect(onPointerMove).not.toHaveBeenCalled();
      expect(onMouseMove).not.toHaveBeenCalled();
      expect(focus).not.toHaveBeenCalled();
      expect(document.activeElement).toBe(document.body);
    });

    // A3
    it("enters outermost-first and leaves innermost-first", async () => {
      const log: string[] = [];
      const outer = document.createElement("div");
      document.body.append(outer);
      const inner = document.createElement("div");
      outer.append(inner);
      const button = nestTarget(inner, "button", "hover.nested");

      const boundary = ["mouseenter", "mouseleave"] as const;
      recordOn(document.documentElement, log, boundary, "html");
      recordOn(document.body, log, boundary, "body");
      recordOn(outer, log, boundary, "outer");
      recordOn(inner, log, boundary, "inner");
      recordOn(button, log, boundary, "button");

      topmostElement = button;
      await executeAutomationRequest("main", request("hover", "hover.nested"));

      expect(log).toEqual([
        "mouseenter:html",
        "mouseenter:body",
        "mouseenter:outer",
        "mouseenter:inner",
        "mouseenter:button",
      ]);

      log.length = 0;
      await executeAutomationRequest("main", request("hover", "", "leave"));

      expect(log).toEqual([
        "mouseleave:button",
        "mouseleave:inner",
        "mouseleave:outer",
        "mouseleave:body",
        "mouseleave:html",
      ]);
    });

    // A4
    it("dispatches nothing when re-hovering the element it is already on", async () => {
      const log: string[] = [];
      const button = addTarget("button", "hover.same", "Same");
      topmostElement = button;
      await executeAutomationRequest("main", request("hover", "hover.same"));
      recordOn(button, log);

      const response = await executeAutomationRequest("main", request("hover", "hover.same"));

      const hover = hoverOf(response);
      expect(hover.events).toEqual([]);
      expect(hover.changed).toBe(false);
      expect(log).toEqual([]);
    });

    // A5
    it("--leave fires the chain up to <html> with a null relatedTarget and parks the pointer", async () => {
      const button = addTarget("button", "hover.leave", "Leave");
      const outs: MouseEvent[] = [];
      const htmlLeaves: MouseEvent[] = [];
      button.addEventListener("mouseout", (event) => outs.push(event as MouseEvent));
      const onHtmlLeave = (event: Event) => htmlLeaves.push(event as MouseEvent);
      document.documentElement.addEventListener("mouseleave", onHtmlLeave);
      listenerCleanups.push(() =>
        document.documentElement.removeEventListener("mouseleave", onHtmlLeave),
      );

      topmostElement = button;
      await executeAutomationRequest("main", request("hover", "hover.leave"));

      const left = await executeAutomationRequest("main", request("hover", "", "leave"));

      const leftHover = hoverOf(left);
      expect(leftHover).toMatchObject({ from: "hover.leave", to: null, changed: true });
      // button + body + html
      expect(leftHover.events).toEqual([
        "pointerout",
        "pointerleave",
        "pointerleave",
        "pointerleave",
        "mouseout",
        "mouseleave",
        "mouseleave",
        "mouseleave",
      ]);
      expect(outs[0]?.relatedTarget).toBeNull();
      expect(htmlLeaves).toHaveLength(1);

      // The pointer is parked nowhere, so the same element is a real transition
      // again rather than a same-element no-op.
      const again = await executeAutomationRequest("main", request("hover", "hover.leave"));
      expect(hoverOf(again).changed).toBe(true);
      expect(hoverOf(again).events).toContain("mouseenter");
    });

    // A6
    it("--leave with nothing hovered is an idempotent no-op", async () => {
      const response = await executeAutomationRequest("main", request("hover", "", "leave"));

      expect(response.ok).toBe(true);
      const hover = hoverOf(response);
      expect(hover).toMatchObject({ from: null, to: null, changed: false, reason: "not_hovered" });
      expect(hover.events).toEqual([]);
    });

    // A7
    it("--leave on a detached sticky element still leaves its connected ancestors", async () => {
      const log: string[] = [];
      const container = document.createElement("div");
      document.body.append(container);
      const button = nestTarget(container, "button", "hover.detached");
      recordOn(button, log, HOVER_EVENTS, "button");
      recordOn(container, log, HOVER_EVENTS, "container");

      topmostElement = button;
      await executeAutomationRequest("main", request("hover", "hover.detached"));
      log.length = 0;

      button.remove(); // exactly what <For> does to a row under a stationary cursor

      const response = await executeAutomationRequest("main", request("hover", "", "leave"));

      const hover = hoverOf(response);
      expect(hover).toMatchObject({ from: "hover.detached", to: null, changed: true, staleFrom: true });
      // The dead node gets nothing; container + body + html still get theirs.
      expect(hover.events).toEqual([
        "pointerleave",
        "pointerleave",
        "pointerleave",
        "mouseleave",
        "mouseleave",
        "mouseleave",
      ]);
      expect(log).toEqual(["pointerleave:container", "mouseleave:container"]);
    });

    // A8
    it("--leave never returns missing_selector, target_hidden or target_obscured", async () => {
      const button = addTarget("button", "hover.gauntlet", "Gauntlet");
      topmostElement = button;
      await executeAutomationRequest("main", request("hover", "hover.gauntlet"));

      button.remove(); // missing_selector, if the leave path ran the query
      topmostElement = null; // target_obscured, if it ran the hit test

      const response = await executeAutomationRequest("main", request("hover", "", "leave"));

      expect(response.ok).toBe(true);
      expect(hoverOf(response).changed).toBe(true);
    });

    // A9
    it("hovers a disabled target, and still refuses to click it", async () => {
      const button = addTarget("button", "hover.disabled", "Disabled");
      button.setAttribute("aria-disabled", "true");
      topmostElement = button;
      const onMouseEnter = vi.fn();
      button.addEventListener("mouseenter", onMouseEnter);

      const hovered = await executeAutomationRequest("main", request("hover", "hover.disabled"));

      expect(hovered.ok).toBe(true);
      expect(onMouseEnter).toHaveBeenCalledTimes(1);

      const clicked = await executeAutomationRequest("main", request("click", "hover.disabled"));

      expect(clicked.ok).toBe(false);
      if (clicked.ok) throw new Error("expected target_disabled");
      expect(clicked.error).toBe("target_disabled");
    });

    // A10
    it("refuses hidden and obscured targets, dispatching nothing", async () => {
      const log: string[] = [];
      const hidden = addTarget("button", "hover.hidden", "Hidden");
      hidden.style.visibility = "hidden";
      recordOn(hidden, log);

      const hiddenResponse = await executeAutomationRequest("main", request("hover", "hover.hidden"));

      expect(hiddenResponse.ok).toBe(false);
      if (hiddenResponse.ok) throw new Error("expected target_hidden");
      expect(hiddenResponse.error).toBe("target_hidden");

      const covered = addTarget("button", "hover.covered", "Covered");
      recordOn(covered, log);
      const blocker = addTarget("div", "hover.blocker", "Blocker");
      topmostElement = blocker;

      const obscured = await executeAutomationRequest("main", request("hover", "hover.covered"));

      expect(obscured.ok).toBe(false);
      if (obscured.ok) throw new Error("expected target_obscured");
      expect(obscured.error).toBe("target_obscured");
      expect(obscured.diagnostics?.topmost?.testId).toBe("hover.blocker");
      expect(log).toEqual([]);
    });

    // A11
    it("does not dispatch when the request expires before mutation", async () => {
      const log: string[] = [];
      const button = addTarget("button", "hover.expired", "Expired");
      topmostElement = button;
      recordOn(button, log);

      const response = await executeAutomationRequest(
        "main",
        request("hover", "hover.expired", undefined, Date.now() - 1),
      );

      expect(response.ok).toBe(false);
      if (response.ok) throw new Error("expected timeout");
      expect(response.error).toBe("timeout");
      expect(log).toEqual([]);
    });

    // A12
    it("skips out/leave on a detached previous target and still enters the new one", async () => {
      const log: string[] = [];
      const container = document.createElement("div");
      document.body.append(container);
      const first = nestTarget(container, "button", "hover.stale");
      const second = addTarget("button", "hover.fresh", "Fresh");
      recordOn(first, log, HOVER_EVENTS, "first");
      recordOn(container, log, HOVER_EVENTS, "container");
      recordOn(second, log, HOVER_EVENTS, "second");

      topmostElement = first;
      await executeAutomationRequest("main", request("hover", "hover.stale"));
      log.length = 0;
      first.remove();

      topmostElement = second;
      const response = await executeAutomationRequest("main", request("hover", "hover.fresh"));

      expect(hoverOf(response)).toMatchObject({
        from: "hover.stale",
        to: "hover.fresh",
        changed: true,
        staleFrom: true,
      });
      expect(hoverOf(response).events).toEqual([
        "pointerleave",
        "mouseleave",
        "pointerover",
        "pointerenter",
        "mouseover",
        "mouseenter",
      ]);
      expect(log).toEqual([
        "pointerleave:container",
        "mouseleave:container",
        "pointerover:second",
        "pointerenter:second",
        "mouseover:second",
        "mouseenter:second",
      ]);
    });

    // A13
    it("enters the chain from the document root on the first hover of a session", async () => {
      const log: string[] = [];
      recordOn(document.body, log, ["mouseenter"], "body");
      const button = addTarget("button", "hover.first", "First");
      topmostElement = button;

      const response = await executeAutomationRequest("main", request("hover", "hover.first"));

      const hover = hoverOf(response);
      expect(hover.from).toBeNull();
      expect(log).toEqual(["mouseenter:body"]);
      // html + body + button
      expect(hover.events).toEqual([
        "pointerover",
        "pointerenter",
        "pointerenter",
        "pointerenter",
        "mouseover",
        "mouseenter",
        "mouseenter",
        "mouseenter",
      ]);
    });

    // A14
    it("refuses an unrecognized value instead of silently hovering the target", async () => {
      const log: string[] = [];
      const button = addTarget("button", "hover.value", "Value");
      topmostElement = button;
      recordOn(button, log);

      for (const value of ["Leave", "true"]) {
        const response = await executeAutomationRequest(
          "main",
          request("hover", "hover.value", value),
        );

        expect(response.ok).toBe(false);
        if (response.ok) throw new Error("expected value_not_supported");
        expect(response.error).toBe("value_not_supported");
      }
      expect(log).toEqual([]);
    });

    // A16
    it("never fires into a node an earlier handler detached, and never reports it", async () => {
      const log: string[] = [];
      const outer = document.createElement("div");
      document.body.append(outer);
      const inner = document.createElement("div");
      outer.append(inner);
      const button = nestTarget(inner, "button", "hover.torn");
      recordOn(inner, log, HOVER_EVENTS, "inner");
      recordOn(button, log, HOVER_EVENTS, "button");

      // The enter chain runs outermost-first, so this ancestor handler fires BEFORE
      // the events for `inner` and for the target itself - and it kills both.
      outer.addEventListener("pointerenter", () => inner.remove());

      topmostElement = button;
      const response = await executeAutomationRequest("main", request("hover", "hover.torn"));

      // html + body + outer are alive and get everything. `inner` and the target are
      // dead from the moment outer's handler ran, so they get nothing more - and
      // `events` reports exactly what landed, not what we intended to send.
      expect(hoverOf(response).events).toEqual([
        "pointerover", // the target was still connected for this one
        "pointerenter",
        "pointerenter",
        "pointerenter",
        "mouseenter",
        "mouseenter",
        "mouseenter",
      ]);
      // pointerover BUBBLES, so `inner` hears the target's - it is still connected at
      // that instant. It hears nothing after outer's handler runs: no pointerenter, no
      // mouseover (which would have bubbled), no mouseenter.
      expect(log).toEqual(["pointerover:button", "pointerover:inner"]);
    });

    // A15
    it("carries pointerId / pointerType / isPrimary on the pointer events in jsdom", async () => {
      const button = addTarget("button", "hover.pointer", "Pointer");
      topmostElement = button;
      const pointerEvents: MouseEvent[] = [];
      const mouseEvents: MouseEvent[] = [];
      for (const type of ["pointerover", "pointerenter"]) {
        button.addEventListener(type, (event) => pointerEvents.push(event as MouseEvent));
      }
      for (const type of ["mouseover", "mouseenter"]) {
        button.addEventListener(type, (event) => mouseEvents.push(event as MouseEvent));
      }

      await executeAutomationRequest("main", request("hover", "hover.pointer"));

      expect(pointerEvents).toHaveLength(2);
      for (const event of pointerEvents) {
        // jsdom 25 has no PointerEvent ctor and MouseEventInit DROPS these three, so
        // they exist only because createHoverEvent defines them on the fallback event.
        const pointer = event as unknown as {
          pointerId: number;
          pointerType: string;
          isPrimary: boolean;
        };
        expect(pointer.pointerId).toBe(1);
        expect(pointer.pointerType).toBe("mouse");
        expect(pointer.isPrimary).toBe(true);
        expect(event.button).toBe(-1);
        expect(event.buttons).toBe(0);
      }

      expect(mouseEvents).toHaveLength(2);
      for (const event of mouseEvents) {
        expect(event.button).toBe(0);
        expect(event.buttons).toBe(0); // a hover presses nothing
      }
    });
  });
});
