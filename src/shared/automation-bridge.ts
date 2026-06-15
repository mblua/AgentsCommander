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
const MISSING_SELECTOR_RETRY_MS = 250;
const MISSING_SELECTOR_RETRY_INTERVAL_MS = 25;
const REDACTED_TEXT = "[redacted]";

const SAFE_METADATA_ATTRIBUTES = [
  ["data-ac-agent-id", "agentId"],
  ["data-ac-agent-index", "agentIndex"],
  ["data-ac-agent-command", "agentCommand"],
  ["data-ac-command", "command"],
  ["data-ac-profile-letter", "profileLetter"],
  ["data-ac-requested-profile", "requestedProfile"],
  ["data-ac-effective-profile", "effectiveProfile"],
  ["data-ac-configured", "configured"],
  ["data-ac-env-source", "envSource"],
] as const;

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
    return await executeAutomationRequestInner(windowLabel, request);
  } catch (error) {
    return errorResponse(
      windowLabel,
      request,
      "automation_bridge_exception",
      error instanceof Error ? error.message : String(error),
    );
  }
}

async function executeAutomationRequestInner(
  windowLabel: string,
  request: UiAutomationRequest,
): Promise<UiAutomationResponse> {
  const diagnostics = baseDiagnostics();
  const expiredBeforeQuery = expiredRequestResponse(windowLabel, request, diagnostics);
  if (expiredBeforeQuery) return expiredBeforeQuery;

  const matches = await queryAutomationTargetsWithBriefRetry(request);
  const expiredAfterQuery = expiredRequestResponse(windowLabel, request, diagnostics);
  if (expiredAfterQuery) return expiredAfterQuery;

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
    const expired = expiredMutationResponse(windowLabel, request, diagnostics);
    if (expired) return expired;
    element.focus();
    element.click();
    await settleAfterDomMutation();
    return successResponse(windowLabel, request, snapshotTarget(element), diagnostics);
  }

  if (request.action === "contextClick") {
    const expired = expiredMutationResponse(windowLabel, request, diagnostics);
    if (expired) return expired;
    element.focus();
    element.dispatchEvent(createContextMenuEvent(element, elementCenterPoint(element)));
    await settleAfterDomMutation();
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

async function setElementValue(
  windowLabel: string,
  request: UiAutomationRequest,
  element: HTMLElement,
  diagnostics: UiAutomationDiagnostics,
): Promise<UiAutomationResponse> {
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

  const expired = expiredMutationResponse(windowLabel, request, diagnostics);
  if (expired) return expired;

  element.focus();
  element.value = request.value ?? "";
  element.dispatchEvent(new Event("input", { bubbles: true }));
  element.dispatchEvent(new Event("change", { bubbles: true }));
  await settleAfterDomMutation();
  return successResponse(windowLabel, request, snapshotTarget(element), diagnostics);
}

async function settleAfterDomMutation(): Promise<void> {
  await Promise.resolve();
  await new Promise<void>((resolve) => window.setTimeout(resolve, 0));
}

function expiredMutationResponse(
  windowLabel: string,
  request: UiAutomationRequest,
  diagnostics: UiAutomationDiagnostics,
): UiAutomationResponse | null {
  return expiredRequestResponse(windowLabel, request, diagnostics);
}

function expiredRequestResponse(
  windowLabel: string,
  request: UiAutomationRequest,
  diagnostics: UiAutomationDiagnostics,
): UiAutomationResponse | null {
  const nowUnixMs = Date.now();
  if (!requestExpired(request, nowUnixMs)) return null;

  return errorResponse(
    windowLabel,
    request,
    "timeout",
    `Automation request "${request.requestId}" expired before the frontend could complete "${request.action}".`,
    availableTargets(),
    {
      ...diagnostics,
      expiresAtUnixMs: request.expiresAtUnixMs ?? null,
      nowUnixMs,
    },
  );
}

function requestExpired(request: UiAutomationRequest, nowUnixMs: number): boolean {
  return typeof request.expiresAtUnixMs === "number" && request.expiresAtUnixMs <= nowUnixMs;
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

async function queryAutomationTargetsWithBriefRetry(
  request: UiAutomationRequest,
): Promise<HTMLElement[]> {
  let matches = queryAutomationTargets(request.selector);
  if (matches.length > 0) return matches;

  const retryMs = missingSelectorRetryBudgetMs(request);
  if (retryMs <= 0) return matches;

  const deadline = Date.now() + retryMs;
  while (Date.now() < deadline) {
    const nowUnixMs = Date.now();
    if (requestExpired(request, nowUnixMs)) return [];
    const remainingBudgetMs = deadline - nowUnixMs;
    const remainingExpiryMs = requestExpiryRemainingMs(request, nowUnixMs);
    const sleepMs = Math.min(
      MISSING_SELECTOR_RETRY_INTERVAL_MS,
      remainingBudgetMs,
      remainingExpiryMs ?? MISSING_SELECTOR_RETRY_INTERVAL_MS,
    );

    await new Promise<void>((resolve) =>
      window.setTimeout(resolve, sleepMs),
    );
    if (requestExpired(request, Date.now())) return [];

    matches = queryAutomationTargets(request.selector);
    if (matches.length > 0) return matches;
  }

  return matches;
}

function missingSelectorRetryBudgetMs(request: UiAutomationRequest): number {
  if (typeof request.expiresAtUnixMs !== "number") return MISSING_SELECTOR_RETRY_MS;
  return Math.max(0, Math.min(MISSING_SELECTOR_RETRY_MS, request.expiresAtUnixMs - Date.now()));
}

function requestExpiryRemainingMs(request: UiAutomationRequest, nowUnixMs: number): number | null {
  if (typeof request.expiresAtUnixMs !== "number") return null;
  return Math.max(0, request.expiresAtUnixMs - nowUnixMs);
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
    metadata: snapshotMetadata(element),
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

function snapshotMetadata(element: HTMLElement): Record<string, string> {
  const metadata: Record<string, string> = {};
  for (const [attribute, key] of SAFE_METADATA_ATTRIBUTES) {
    const raw = element.getAttribute(attribute);
    const value = sanitizeMetadataValue(raw);
    if (value !== null) {
      metadata[key] = value;
    }
  }
  return metadata;
}

function sanitizeMetadataValue(value: string | null): string | null {
  if (value === null) return null;
  const normalized = redactSnapshotText(value.replace(/\s+/g, " ").trim());
  if (!normalized) return null;
  return normalized.slice(0, MAX_SNAPSHOT_TEXT);
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

function elementCenterPoint(element: HTMLElement): { clientX: number; clientY: number } {
  const rect = element.getBoundingClientRect();
  return {
    clientX: rect.x + rect.width / 2,
    clientY: rect.y + rect.height / 2,
  };
}

function createContextMenuEvent(
  element: HTMLElement,
  point: { clientX: number; clientY: number },
): MouseEvent {
  const eventView = element.ownerDocument.defaultView ?? window;
  const eventInit: MouseEventInit = {
    bubbles: true,
    cancelable: true,
    composed: true,
    button: 2,
    buttons: 2,
    clientX: point.clientX,
    clientY: point.clientY,
    screenX: Math.round(point.clientX),
    screenY: Math.round(point.clientY),
  };

  try {
    return new eventView.MouseEvent("contextmenu", { ...eventInit, view: eventView });
  } catch (error) {
    if (!String(error).includes("member view is not of type Window")) throw error;
    return new eventView.MouseEvent("contextmenu", eventInit);
  }
}

function topmostElementAtCenter(element: HTMLElement): { ok: true } | { ok: false; element: HTMLElement | null } {
  if (typeof document.elementFromPoint !== "function") {
    return { ok: true };
  }

  const { clientX, clientY } = elementCenterPoint(element);
  const top = document.elementFromPoint(clientX, clientY);
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
