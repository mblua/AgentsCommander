import { AutomationAPI, onUiAutomationRequest } from "./ipc";
import { isTauri } from "./platform";
import type {
  UiAutomationAction,
  UiAutomationDiagnostics,
  UiAutomationRequest,
  UiAutomationResponse,
  UiAutomationTarget,
  UiAutomationTargetRect,
} from "./types";

const MAX_AVAILABLE_TARGETS = 50;
const MAX_SNAPSHOT_TEXT = 120;
const REDACTED_TEXT = "[redacted]";

type UiAutomationErrorCode = Extract<UiAutomationResponse, { ok: false }>["error"];

let started = false;

async function resolveAutomationWindowLabel(explicit?: string): Promise<string> {
  if (explicit) return explicit;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow().label;
}

export async function initAutomationBridge(windowLabel?: string): Promise<void> {
  if (started || !isTauri) return;

  const enabled = await AutomationAPI.enabled().catch(() => false);
  if (!enabled) return;

  const resolvedWindowLabel = await resolveAutomationWindowLabel(windowLabel);
  await onUiAutomationRequest((request) => {
    if (request.window !== resolvedWindowLabel) return;

    void executeAutomationRequest(resolvedWindowLabel, request)
      .then((response) => AutomationAPI.complete(response))
      .catch((error) => {
        console.error("[automation] failed to complete request:", error);
      });
  });

  started = true;
  await AutomationAPI.frontendReady(resolvedWindowLabel).catch((error) => {
    console.error("[automation] frontend readiness failed:", error);
  });
}

export function resetAutomationBridgeForTests(): void {
  started = false;
}

export async function executeAutomationRequest(
  windowLabel: string,
  request: UiAutomationRequest,
): Promise<UiAutomationResponse> {
  try {
    return executeAutomationRequestInner(windowLabel, request);
  } catch (error) {
    return errorResponse(
      windowLabel,
      request,
      "automation_bridge_exception",
      error instanceof Error ? error.message : String(error),
    );
  }
}

function executeAutomationRequestInner(
  windowLabel: string,
  request: UiAutomationRequest,
): UiAutomationResponse {
  const matches = queryAutomationTargets(request.selector);
  const diagnostics = baseDiagnostics();

  if (matches.length === 0) {
    return errorResponse(
      windowLabel,
      request,
      "missing_selector",
      `No automation target matched data-ac-testid="${request.selector}" in window "${windowLabel}".`,
      availableTargets(),
      diagnostics,
    );
  }

  if (matches.length > 1) {
    return errorResponse(
      windowLabel,
      request,
      "duplicate_selector",
      `Multiple automation targets matched data-ac-testid="${request.selector}" in window "${windowLabel}".`,
      matches.map((element) => snapshotTarget(element)),
      diagnostics,
    );
  }

  const element = matches[0];
  if (!isElementVisible(element)) {
    return errorResponse(
      windowLabel,
      request,
      "target_hidden",
      `Automation target "${request.selector}" is hidden in window "${windowLabel}".`,
      availableTargets(),
      diagnostics,
    );
  }

  if (request.action === "query") {
    return successResponse(windowLabel, request, snapshotTarget(element), diagnostics);
  }

  if (isElementDisabled(element)) {
    return errorResponse(
      windowLabel,
      request,
      "target_disabled",
      `Automation target "${request.selector}" is disabled in window "${windowLabel}".`,
      availableTargets(),
      diagnostics,
    );
  }

  const topmost = topmostElementAtCenter(element);
  if (!topmost.ok) {
    return errorResponse(
      windowLabel,
      request,
      "target_obscured",
      `Automation target "${request.selector}" is obscured in window "${windowLabel}".`,
      availableTargets(),
      { ...diagnostics, topmost: topmost.element ? snapshotTarget(topmost.element) : null },
    );
  }

  if (request.action === "click") {
    element.focus();
    element.click();
    return successResponse(windowLabel, request, snapshotTarget(element), diagnostics);
  }

  if (request.action === "setValue") {
    return setElementValue(windowLabel, request, element, diagnostics);
  }

  return errorResponse(
    windowLabel,
    request,
    "unsupported_action",
    `Unsupported automation action "${request.action as UiAutomationAction}".`,
    availableTargets(),
    diagnostics,
  );
}

function setElementValue(
  windowLabel: string,
  request: UiAutomationRequest,
  element: HTMLElement,
  diagnostics: UiAutomationDiagnostics,
): UiAutomationResponse {
  if (
    !(element instanceof HTMLInputElement) &&
    !(element instanceof HTMLTextAreaElement) &&
    !(element instanceof HTMLSelectElement)
  ) {
    return errorResponse(
      windowLabel,
      request,
      "value_not_supported",
      `Automation target "${request.selector}" does not support setValue.`,
      availableTargets(),
      diagnostics,
    );
  }

  element.focus();
  element.value = request.value ?? "";
  element.dispatchEvent(new Event("input", { bubbles: true }));
  element.dispatchEvent(new Event("change", { bubbles: true }));
  return successResponse(windowLabel, request, snapshotTarget(element), diagnostics);
}

function successResponse(
  windowLabel: string,
  request: UiAutomationRequest,
  target: UiAutomationTarget,
  diagnostics?: UiAutomationDiagnostics,
): UiAutomationResponse {
  return {
    ok: true,
    requestId: request.requestId,
    window: windowLabel,
    action: request.action,
    selector: request.selector,
    target,
    diagnostics,
  };
}

function errorResponse(
  windowLabel: string,
  request: UiAutomationRequest,
  error: UiAutomationErrorCode,
  message: string,
  available?: UiAutomationTarget[],
  diagnostics?: UiAutomationDiagnostics,
): UiAutomationResponse {
  return {
    ok: false,
    requestId: request.requestId,
    window: windowLabel,
    action: request.action,
    selector: request.selector,
    error,
    message,
    available,
    diagnostics,
  };
}

function queryAutomationTargets(testId: string): HTMLElement[] {
  const selector = `[data-ac-testid="${cssEscape(testId)}"]`;
  return queryAcrossOpenRoots(selector);
}

function availableTargets(): UiAutomationTarget[] {
  return queryAcrossOpenRoots("[data-ac-testid]")
    .map((element) => snapshotTarget(element))
    .sort((a, b) => a.testId.localeCompare(b.testId))
    .slice(0, MAX_AVAILABLE_TARGETS);
}

function queryAcrossOpenRoots(selector: string): HTMLElement[] {
  const matches: HTMLElement[] = [];
  const visit = (root: Document | ShadowRoot) => {
    matches.push(...Array.from(root.querySelectorAll<HTMLElement>(selector)));
    for (const element of Array.from(root.querySelectorAll<HTMLElement>("*"))) {
      if (element.shadowRoot) {
        visit(element.shadowRoot);
      }
    }
  };

  visit(document);
  return matches;
}

function cssEscape(value: string): string {
  const css = globalThis.CSS as { escape?: (value: string) => string } | undefined;
  if (css?.escape) return css.escape(value);
  return value.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
}

function snapshotTarget(element: HTMLElement): UiAutomationTarget {
  return {
    testId: element.getAttribute("data-ac-testid") ?? "",
    role: element.getAttribute("data-ac-role") ?? element.getAttribute("role"),
    state: element.getAttribute("data-ac-state"),
    tag: element.tagName.toLowerCase(),
    text: snapshotText(element),
    visible: isElementVisible(element),
    disabled: isElementDisabled(element),
    checked: boolState(element, "checked"),
    selected: boolState(element, "selected"),
    pressed: ariaBool(element, "aria-pressed"),
    expanded: ariaBool(element, "aria-expanded"),
    rect: targetRect(element),
  };
}

function snapshotText(element: HTMLElement): string {
  if (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement) {
    return "";
  }

  const role = element.getAttribute("data-ac-role") ?? element.getAttribute("role") ?? "";
  const allowText =
    role === "agent-preset" ||
    role === "button" ||
    role === "checkbox" ||
    role === "menuitem" ||
    role === "tab" ||
    element instanceof HTMLButtonElement;

  if (!allowText) return "";

  const text = (element.textContent ?? "").replace(/\s+/g, " ").trim();
  return redactSnapshotText(text).slice(0, MAX_SNAPSHOT_TEXT);
}

function redactSnapshotText(text: string): string {
  return text
    .replace(/AIza[0-9A-Za-z_-]{20,}/g, REDACTED_TEXT)
    .replace(/\b[A-Za-z0-9_=-]{32,}\b/g, REDACTED_TEXT);
}

function boolState(element: HTMLElement, state: "checked" | "selected"): boolean | null {
  if (state === "checked") {
    if (element instanceof HTMLInputElement && ["checkbox", "radio"].includes(element.type)) {
      return element.checked;
    }
    return ariaBool(element, "aria-checked");
  }

  if (element instanceof HTMLOptionElement) {
    return element.selected;
  }
  return ariaBool(element, "aria-selected");
}

function ariaBool(element: HTMLElement, attr: string): boolean | null {
  const value = element.getAttribute(attr);
  if (value === "true") return true;
  if (value === "false") return false;
  return null;
}

function isElementDisabled(element: HTMLElement): boolean {
  if (
    (element instanceof HTMLButtonElement ||
      element instanceof HTMLInputElement ||
      element instanceof HTMLSelectElement ||
      element instanceof HTMLTextAreaElement) &&
    element.disabled
  ) {
    return true;
  }

  return element.getAttribute("aria-disabled") === "true" ||
    element.getAttribute("data-ac-state") === "disabled";
}

function isElementVisible(element: HTMLElement): boolean {
  if (isTreeHidden(element)) return false;
  const rects = element.getClientRects();
  if (rects.length === 0) return false;
  const rect = element.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0;
}

function isTreeHidden(element: HTMLElement): boolean {
  let current: HTMLElement | null = element;
  while (current) {
    if (
      current.hidden ||
      current.hasAttribute("inert") ||
      current.getAttribute("aria-hidden") === "true"
    ) {
      return true;
    }

    const style = window.getComputedStyle(current);
    if (style.display === "none" || style.visibility === "hidden") {
      return true;
    }

    current = parentElementOrHost(current);
  }

  return false;
}

function parentElementOrHost(element: HTMLElement): HTMLElement | null {
  if (element.parentElement) return element.parentElement;
  const root = element.getRootNode();
  if (root instanceof ShadowRoot && root.host instanceof HTMLElement) {
    return root.host;
  }
  return null;
}

function topmostElementAtCenter(element: HTMLElement): { ok: true } | { ok: false; element: HTMLElement | null } {
  if (typeof document.elementFromPoint !== "function") {
    return { ok: true };
  }

  const rect = element.getBoundingClientRect();
  const top = document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2);
  if (!top) {
    return { ok: false, element: null };
  }

  if (isSameOrComposedDescendant(element, top)) {
    return { ok: true };
  }

  return { ok: false, element: top instanceof HTMLElement ? top : null };
}

function isSameOrComposedDescendant(parent: HTMLElement, child: Element): boolean {
  let current: Node | null = child;
  while (current) {
    if (current === parent) return true;
    if (current.parentNode) {
      current = current.parentNode;
      continue;
    }
    const root = current.getRootNode();
    current = root instanceof ShadowRoot ? root.host : null;
  }
  return false;
}

function targetRect(element: HTMLElement): UiAutomationTargetRect | null {
  const rect = element.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return null;
  return {
    x: rect.x,
    y: rect.y,
    width: rect.width,
    height: rect.height,
  };
}

function baseDiagnostics(): UiAutomationDiagnostics {
  return {
    devicePixelRatio: window.devicePixelRatio || 1,
    viewport: {
      width: window.innerWidth,
      height: window.innerHeight,
    },
  };
}
