import { AutomationAPI, onUiAutomationRequest } from "./ipc";
import { isTauri } from "./platform";
import type {
  UiAutomationAction,
  UiAutomationDiagnostics,
  UiAutomationListTarget,
  UiAutomationRequest,
  UiAutomationResponse,
  UiAutomationRole,
  UiAutomationTarget,
  UiAutomationTargetRect,
} from "./types";

const MAX_AVAILABLE_TARGETS = 50;
const MAX_SNAPSHOT_TEXT = 120;
const MISSING_SELECTOR_RETRY_MS = 250;
const MISSING_SELECTOR_RETRY_INTERVAL_MS = 25;
const REDACTED_TEXT = "[redacted]";
const MAX_TEST_ID_BYTES = 256;
const MAX_LIST_TARGETS = 50;
const MAX_SCAN_TARGETS = 1_000;
const MAX_SCAN_ELEMENTS = 20_000;
const MAX_OPEN_ROOTS = 64;
const PUBLIC_TEST_ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$/;
const SAFE_STATE_PATTERN = /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,63}$/;
const SUPPORTED_ROLES = new Set<UiAutomationRole>([
  "agent-preset", "alert", "button", "cell", "checkbox", "combobox", "dialog",
  "group", "input", "list", "menu", "menuitem", "metric", "overlay", "region",
  "row", "searchbox", "separator", "spinbutton", "status", "surface", "tab",
  "text", "textbox", "toolbar",
]);

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
  ["data-ac-detail", "detail"],
] as const;

type UiAutomationErrorCode = Extract<UiAutomationResponse, { ok: false }>["error"];

let started = false;

let hoveredElement: HTMLElement | null = null;
let hoveredChain: HTMLElement[] = [];

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
      .catch(() => {});
  });

  started = true;
  await AutomationAPI.frontendReady(resolvedWindowLabel).catch(() => {});
}

export function resetAutomationBridgeForTests(): void {
  started = false;
  hoveredElement = null;
  hoveredChain = [];
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

  if (request.action === "list") {
    return listResponse(windowLabel, request);
  }

  if (request.action === "hover") {
    if (request.value != null && request.value !== "leave") {
      return errorResponse(
        windowLabel,
        request,
        "value_not_supported",
        "Automation hover accepts no value other than the closed leave operation.",
        availableTargets(),
        diagnostics,
      );
    }

    if (request.value === "leave") {
      const target = hoveredElement ? snapshotTarget(hoveredElement) : emptyHoverTarget();
      const hover = dispatchHoverLeave();
      await settleAfterDomMutation();
      return successResponse(windowLabel, request, target, { ...diagnostics, hover });
    }
  }

  const matches = await queryAutomationTargetsWithBriefRetry(request);
  const expiredAfterQuery = expiredRequestResponse(windowLabel, request, diagnostics);
  if (expiredAfterQuery) return expiredAfterQuery;

  if (matches.length === 0) {
    return errorResponse(
      windowLabel,
      request,
      "missing_selector",
      "No public automation target matched the requested selector.",
      availableTargets(),
      diagnostics,
    );
  }

  if (matches.length > 1) {
    return errorResponse(
      windowLabel,
      request,
      "duplicate_selector",
      "Multiple public automation targets matched the requested selector.",
      availableTargets(matches),
      diagnostics,
    );
  }

  const element = matches[0];
  if (!isElementVisible(element)) {
    return errorResponse(
      windowLabel,
      request,
      "target_hidden",
      "The requested automation target is hidden.",
      availableTargets(),
      diagnostics,
    );
  }

  if (request.action === "query") {
    return successResponse(windowLabel, request, snapshotTarget(element), diagnostics);
  }

  if (request.action !== "hover" && isElementDisabled(element)) {
    return errorResponse(
      windowLabel,
      request,
      "target_disabled",
      "The requested automation target is disabled.",
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
      "The requested automation target is obscured.",
      availableTargets(),
      {
        ...diagnostics,
        topmost:
          topmost.element && publicAutomationTestId(topmost.element)
            ? snapshotTarget(topmost.element)
            : null,
      },
    );
  }

  if (request.action === "hover") {
    const expired = expiredMutationResponse(windowLabel, request, diagnostics);
    if (expired) return expired;
    if (!revalidateExactTarget(request, element)) {
      return errorResponse(
        windowLabel,
        request,
        "target_stale",
        "Automation target changed before hover.",
        availableTargets(),
        diagnostics,
      );
    }

    const hover = dispatchHoverEnter(element);
    await settleAfterDomMutation();
    return successResponse(windowLabel, request, snapshotTarget(element), {
      ...diagnostics,
      hover,
    });
  }

  if (request.action === "click") {
    const expired = expiredMutationResponse(windowLabel, request, diagnostics);
    if (expired) return expired;
    if (!revalidateExactTarget(request, element)) {
      return errorResponse(windowLabel, request, "target_stale", "Automation target changed before click.", availableTargets(), diagnostics);
    }
    element.focus();
    element.click();
    await settleAfterDomMutation();
    return successResponse(windowLabel, request, snapshotTarget(element), diagnostics);
  }

  if (request.action === "contextClick") {
    const expired = expiredMutationResponse(windowLabel, request, diagnostics);
    if (expired) return expired;
    if (!revalidateExactTarget(request, element)) {
      return errorResponse(windowLabel, request, "target_stale", "Automation target changed before context click.", availableTargets(), diagnostics);
    }
    element.focus();
    element.dispatchEvent(createContextMenuEvent(element, elementCenterPoint(element)));
    await settleAfterDomMutation();
    return successResponse(windowLabel, request, snapshotTarget(element), diagnostics);
  }

  if (request.action === "setValue" || request.action === "typeText") {
    return setElementValue(windowLabel, request, element, diagnostics);
  }

  if (request.action === "focus") {
    if (!isProgrammaticallyFocusable(element)) {
      return errorResponse(
        windowLabel,
        request,
        "target_not_focusable",
        "Automation target is not programmatically focusable.",
        availableTargets(),
        diagnostics,
      );
    }
    const expired = expiredMutationResponse(windowLabel, request, diagnostics);
    if (expired) return expired;
    if (!revalidateExactTarget(request, element)) {
      return errorResponse(windowLabel, request, "target_stale", "Automation target changed before focus.", availableTargets(), diagnostics);
    }
    element.focus({ preventScroll: true });
    await settleAfterDomMutation();
    if (!revalidateExactTarget(request, element)) {
      return errorResponse(windowLabel, request, "target_stale", "Automation target changed during focus.", availableTargets(), diagnostics);
    }
    if (deepestActiveElement() !== element) {
      return errorResponse(windowLabel, request, "focus_failed", "The browser did not retain focus on the automation target.", availableTargets(), diagnostics);
    }
    return successResponse(windowLabel, request, snapshotTarget(element), diagnostics);
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
      "The requested automation target does not support value mutation.",
      availableTargets(),
      diagnostics,
    );
  }

  const expired = expiredMutationResponse(windowLabel, request, diagnostics);
  if (expired) return expired;
  if (!revalidateExactTarget(request, element)) {
    return errorResponse(
      windowLabel,
      request,
      "target_stale",
      "Automation target changed before value mutation.",
      availableTargets(),
      diagnostics,
    );
  }

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
    "request_expired",
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
    action: request.action as Exclude<UiAutomationAction, "list">,
    selector: request.selector ?? "",
    target,
    activeTestId: activeTestId(),
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
    selector: request.selector ?? "",
    error,
    message,
    available,
    activeTestId: activeTestId(),
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
  let matches = queryAutomationTargets(request.selector ?? "");
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

    matches = queryAutomationTargets(request.selector ?? "");
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

function availableTargets(source?: HTMLElement[]): UiAutomationTarget[] {
  const elements = source ?? scanAutomationElements().elements.map(({ element }) => element);
  return elements
    .map((element, ordinal) => ({ element, ordinal, testId: publicAutomationTestId(element) }))
    .filter((candidate): candidate is { element: HTMLElement; ordinal: number; testId: string } =>
      candidate.testId !== null,
    )
    .sort((left, right) => compareCodeUnits(left.testId, right.testId) || left.ordinal - right.ordinal)
    .slice(0, MAX_AVAILABLE_TARGETS)
    .map(({ element, testId }) => snapshotTargetWithPublicId(element, testId));
}

function listResponse(windowLabel: string, request: UiAutomationRequest): UiAutomationResponse {
  const scanResult = scanAutomationElements();
  const prefix = request.prefix === undefined || request.prefix === null ? null : request.prefix;
  const requestedRole = supportedRole(request.role ?? null);
  const matches = scanResult.elements
    .map(({ element, ordinal }) => ({
      element,
      ordinal,
      testId: publicAutomationTestId(element),
      role: projectedRole(element),
    }))
    .filter(
      (candidate): candidate is {
        element: HTMLElement;
        ordinal: number;
        testId: string;
        role: UiAutomationRole | null;
      } =>
        candidate.testId !== null &&
        (prefix === null || candidate.testId.startsWith(prefix)) &&
        (request.role == null || candidate.role === requestedRole),
    )
    .sort(
      (left, right) =>
        compareCodeUnits(left.testId, right.testId) ||
        compareNullableCodeUnits(left.role, right.role) ||
        left.ordinal - right.ordinal,
    );
  const targets = matches
    .slice(0, MAX_LIST_TARGETS)
    .map(({ element, testId, role }) => listTarget(element, testId, role));
  const truncated = scanResult.scan.truncated || matches.length > MAX_LIST_TARGETS;
  return {
    ok: true,
    requestId: request.requestId,
    window: windowLabel,
    action: "list",
    filters: { prefix, role: requestedRole },
    targets,
    matchedCount: matches.length,
    matchedCountExact: !scanResult.scan.truncated,
    returnedCount: targets.length,
    limit: MAX_LIST_TARGETS,
    truncated,
    scan: scanResult.scan,
    activeTestId: activeTestId(),
  } as UiAutomationResponse;
}

function scanAutomationElements(): {
  elements: Array<{ element: HTMLElement; ordinal: number }>;
  scan: {
    elements: number;
    elementLimit: number;
    targets: number;
    targetLimit: number;
    openRoots: number;
    openRootLimit: number;
    truncated: boolean;
  };
} {
  const candidates: Array<{ element: HTMLElement; ordinal: number }> = [];
  const roots: Array<Document | ShadowRoot> = [document];
  let elements = 0;
  let targets = 0;
  let openRoots = 0;
  let ordinal = 0;
  let truncated = false;

  outer: while (roots.length > 0) {
    if (openRoots >= MAX_OPEN_ROOTS) {
      truncated = true;
      break;
    }
    const root = roots.shift()!;
    openRoots += 1;
    for (const element of Array.from(root.querySelectorAll<HTMLElement>("*"))) {
      if (elements >= MAX_SCAN_ELEMENTS) {
        truncated = true;
        break outer;
      }
      elements += 1;
      if (element.shadowRoot) roots.push(element.shadowRoot);
      if (publicAutomationTestId(element) === null) continue;
      if (targets >= MAX_SCAN_TARGETS) {
        truncated = true;
        break outer;
      }
      targets += 1;
      candidates.push({ element, ordinal: ordinal++ });
    }
  }

  return {
    elements: candidates,
    scan: {
      elements,
      elementLimit: MAX_SCAN_ELEMENTS,
      targets,
      targetLimit: MAX_SCAN_TARGETS,
      openRoots,
      openRootLimit: MAX_OPEN_ROOTS,
      truncated,
    },
  };
}

export function publicAutomationTestId(element: HTMLElement): string | null {
  if (element.hasAttribute("data-ac-testid-private")) return null;
  const testId = element.getAttribute("data-ac-testid");
  if (!testId || !PUBLIC_TEST_ID_PATTERN.test(testId)) return null;
  if (new TextEncoder().encode(testId).length > MAX_TEST_ID_BYTES) return null;
  return testId;
}

function compareCodeUnits(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}

function compareNullableCodeUnits(left: string | null, right: string | null): number {
  if (left === null) return right === null ? 0 : -1;
  if (right === null) return 1;
  return compareCodeUnits(left, right);
}

function supportedRole(value: string | null): UiAutomationRole | null {
  return value !== null && SUPPORTED_ROLES.has(value as UiAutomationRole)
    ? (value as UiAutomationRole)
    : null;
}

function projectedRole(element: HTMLElement): UiAutomationRole | null {
  return supportedRole(element.getAttribute("data-ac-role") ?? element.getAttribute("role"));
}

function projectedState(element: HTMLElement): string | null {
  const state = element.getAttribute("data-ac-state");
  return state !== null && SAFE_STATE_PATTERN.test(state) ? state : null;
}

function listTarget(
  element: HTMLElement,
  testId: string,
  role: UiAutomationRole | null,
): UiAutomationListTarget {
  return {
    testId,
    role,
    state: projectedState(element),
    visible: isElementVisible(element),
    disabled: isElementDisabled(element),
    checked: boolState(element, "checked"),
    selected: boolState(element, "selected"),
    pressed: ariaBool(element, "aria-pressed"),
    expanded: ariaBool(element, "aria-expanded"),
    focused: deepestActiveElement() === element,
  };
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
  return snapshotTargetWithPublicId(element, publicAutomationTestId(element));
}

function snapshotTargetWithPublicId(
  element: HTMLElement,
  testId: string | null,
): UiAutomationTarget {
  return {
    testId,
    role: projectedRole(element),
    state: projectedState(element),
    metadata: snapshotMetadata(element),
    tag: element.tagName.toLowerCase(),
    text: snapshotText(element),
    visible: isElementVisible(element),
    disabled: isElementDisabled(element),
    checked: boolState(element, "checked"),
    selected: boolState(element, "selected"),
    pressed: ariaBool(element, "aria-pressed"),
    expanded: ariaBool(element, "aria-expanded"),
    focused: deepestActiveElement() === element,
    rect: targetRect(element),
  };
}

function deepestActiveElement(): Element | null {
  let active: Element | null = document.activeElement;
  while (active instanceof HTMLElement && active.shadowRoot?.activeElement) {
    active = active.shadowRoot.activeElement;
  }
  return active;
}

function activeTestId(): string | null {
  const active = deepestActiveElement();
  if (!(active instanceof HTMLElement)) return null;
  const testId = publicAutomationTestId(active);
  if (testId === null) return null;
  const matches = queryAutomationTargets(testId).filter(
    (candidate) => publicAutomationTestId(candidate) === testId,
  );
  return matches.length === 1 && matches[0] === active ? testId : null;
}

function revalidateExactTarget(request: UiAutomationRequest, element: HTMLElement): boolean {
  if (!element.isConnected || !request.selector) return false;
  const matches = queryAutomationTargets(request.selector);
  return matches.length === 1 && matches[0] === element;
}

function isProgrammaticallyFocusable(element: HTMLElement): boolean {
  if (element instanceof HTMLButtonElement) return true;
  if (element instanceof HTMLInputElement) return element.type.toLowerCase() !== "hidden";
  if (element instanceof HTMLSelectElement || element instanceof HTMLTextAreaElement) return true;
  if (element instanceof HTMLAnchorElement || element instanceof HTMLAreaElement) {
    return element.hasAttribute("href");
  }
  if (
    element instanceof HTMLIFrameElement ||
    element instanceof HTMLObjectElement ||
    element instanceof HTMLEmbedElement
  ) {
    return true;
  }
  if (element instanceof HTMLAudioElement || element instanceof HTMLVideoElement) {
    return element.controls;
  }
  if (element instanceof HTMLElement && element.tagName.toLowerCase() === "summary") {
    const details = element.parentElement;
    if (details?.tagName.toLowerCase() === "details") {
      return Array.from(details.children).find((child) => child.tagName.toLowerCase() === "summary") === element;
    }
  }
  return element.hasAttribute("tabindex") || element.isContentEditable;
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

  if (element.hasAttribute("data-ac-value") || element.hasAttribute("data-ac-token")) {
    return "";
  }

  const role = element.getAttribute("data-ac-role") ?? element.getAttribute("role") ?? "";
  const allowText =
    role === "agent-preset" ||
    role === "button" ||
    role === "checkbox" ||
    role === "cell" ||
    role === "group" ||
    role === "metric" ||
    role === "menuitem" ||
    role === "row" ||
    role === "status" ||
    role === "tab" ||
    role === "text" ||
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

type HoverDiagnostics = NonNullable<UiAutomationDiagnostics["hover"]>;

const HOVER_POINTER_INIT = { pointerId: 1, pointerType: "mouse", isPrimary: true } as const;

function dispatchHoverEnter(to: HTMLElement): HoverDiagnostics {
  const from = hoveredElement;
  const events: string[] = [];

  if (from === to) {
    return { from: testIdOf(from), to: testIdOf(to), changed: false, events };
  }

  const staleFrom = !!from && !from.isConnected;
  const toChain = hoverChain(to);
  const common = from ? firstCommonAncestor(hoveredChain, toChain) : null;

  if (from) {
    dispatchLeaveGroup(from, to, takeUntil(hoveredChain, common), events);
  }
  dispatchEnterGroup(to, staleFrom ? null : from, takeUntil(toChain, common).reverse(), events);

  hoveredElement = to;
  hoveredChain = toChain;
  return {
    from: testIdOf(from),
    to: testIdOf(to),
    changed: true,
    ...(staleFrom ? { staleFrom: true } : {}),
    events,
  };
}

function dispatchHoverLeave(): HoverDiagnostics {
  const from = hoveredElement;
  const events: string[] = [];
  if (!from) {
    return { from: null, to: null, changed: false, reason: "not_hovered", events };
  }

  const staleFrom = !from.isConnected;
  dispatchLeaveGroup(from, null, hoveredChain, events);

  hoveredElement = null;
  hoveredChain = [];
  return {
    from: testIdOf(from),
    to: null,
    changed: true,
    ...(staleFrom ? { staleFrom: true } : {}),
    events,
  };
}

function dispatchLeaveGroup(
  from: HTMLElement,
  to: HTMLElement | null,
  chain: HTMLElement[], // innermost first; detached nodes are dropped on dispatch
  events: string[],
): void {
  const point = elementCenterPoint(to ?? from);
  dispatchHoverEvent(from, "pointerout", point, to, true, true, events);
  for (const node of chain) {
    dispatchHoverEvent(node, "pointerleave", point, to, false, false, events);
  }
  dispatchHoverEvent(from, "mouseout", point, to, true, true, events);
  for (const node of chain) {
    dispatchHoverEvent(node, "mouseleave", point, to, false, false, events);
  }
}

function dispatchEnterGroup(
  to: HTMLElement,
  from: HTMLElement | null,
  chain: HTMLElement[], // outermost first; detached nodes are dropped on dispatch
  events: string[],
): void {
  const point = elementCenterPoint(to);
  dispatchHoverEvent(to, "pointerover", point, from, true, true, events);
  for (const node of chain) {
    dispatchHoverEvent(node, "pointerenter", point, from, false, false, events);
  }
  dispatchHoverEvent(to, "mouseover", point, from, true, true, events);
  for (const node of chain) {
    dispatchHoverEvent(node, "mouseenter", point, from, false, false, events);
  }
}

function dispatchHoverEvent(
  target: HTMLElement,
  type: string,
  point: { clientX: number; clientY: number },
  related: HTMLElement | null,
  bubbles: boolean,
  cancelable: boolean,
  events: string[],
): void {
  if (!target.isConnected) return;
  target.dispatchEvent(createHoverEvent(target, type, point, related, bubbles, cancelable));
  events.push(type);
}

function hoverChain(node: HTMLElement): HTMLElement[] {
  const chain: HTMLElement[] = [];
  let current: HTMLElement | null = node;
  while (current) {
    chain.push(current);
    current = parentElementOrHost(current);
  }
  return chain;
}

function firstCommonAncestor(
  fromChain: HTMLElement[],
  toChain: HTMLElement[],
): HTMLElement | null {
  const toSet = new Set(toChain);
  return fromChain.find((node) => toSet.has(node)) ?? null;
}

function takeUntil(chain: HTMLElement[], stop: HTMLElement | null): HTMLElement[] {
  const index = stop ? chain.indexOf(stop) : -1;
  return index === -1 ? chain.slice() : chain.slice(0, index);
}

function testIdOf(element: HTMLElement | null): string | null {
  return element ? publicAutomationTestId(element) : null;
}

function emptyHoverTarget(): UiAutomationTarget {
  return {
    testId: null,
    role: null,
    state: null,
    metadata: {},
    tag: "",
    text: "",
    visible: false,
    disabled: false,
    checked: null,
    selected: null,
    pressed: null,
    expanded: null,
    focused: false,
    rect: null,
  };
}

function createHoverEvent(
  element: HTMLElement,
  type: string,
  point: { clientX: number; clientY: number },
  related: HTMLElement | null,
  bubbles: boolean,
  cancelable: boolean,
): MouseEvent {
  const eventView = element.ownerDocument.defaultView ?? window;
  const isPointer = type.startsWith("pointer");
  const view = eventView as unknown as { PointerEvent?: typeof MouseEvent };
  const hasPointerEvent = typeof view.PointerEvent === "function";
  const Ctor = isPointer && hasPointerEvent ? view.PointerEvent! : eventView.MouseEvent;

  const eventInit: MouseEventInit = {
    bubbles,
    cancelable,
    composed: true,
    button: isPointer ? -1 : 0,
    buttons: 0,
    relatedTarget: related,
    clientX: point.clientX,
    clientY: point.clientY,
    screenX: Math.round(point.clientX),
    screenY: Math.round(point.clientY),
    ...(isPointer ? HOVER_POINTER_INIT : {}),
  };

  let event: MouseEvent;
  try {
    event = new Ctor(type, { ...eventInit, view: eventView });
  } catch (error) {
    if (!String(error).includes("member view is not of type Window")) throw error;
    event = new Ctor(type, eventInit);
  }

  if (isPointer && !hasPointerEvent) {
    for (const [key, value] of Object.entries(HOVER_POINTER_INIT)) {
      Object.defineProperty(event, key, { value, configurable: true });
    }
  }
  return event;
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
