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
  // Generic, sanitized diagnostic detail surfaced on a target (e.g. the
  // project.loadStatus chip exposes boot/load failure info here).
  ["data-ac-detail", "detail"],
] as const;

type UiAutomationErrorCode = Extract<UiAutomationResponse, { ok: false }>["error"];

let started = false;

/** #944 - the pointer is sticky and per-WebView. Only `hover` moves it (plan R2).
 *  The CHAIN is stored, not recomputed: <For> re-mints rows (ProjectPanel :834-840,
 *  :3721-3725), so `hoveredElement` is routinely detached by the time we leave it,
 *  and a chain recomputed from a detached node stops at its detached root - which
 *  would strand `.sidebar-layout` in the pointer-inside state forever (plan C17).
 *
 *  ASSUMPTION, and the one hole `dispatchHoverEvent`'s isConnected guard cannot
 *  close: a node that is still CONNECTED has not been RE-PARENTED since we captured
 *  its chain. If it had, the old and the new ancestors would both be live, and we
 *  would fire the leave chain at the ancestors it no longer has while the ones it
 *  does have never hear a thing. Nothing detects that. It cannot happen today -
 *  Solid's <For> reorders rows within one parent and never moves a node across
 *  parents - so if you ever re-parent a live node under the pointer, this chain has
 *  to be recomputed, not filtered. */
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

  if (request.action === "hover") {
    // #944 - `value` is the leave flag and nothing else. Silently treating an
    // unrecognized value as a normal hover is a false green: the harness believes
    // the pointer moved OFF the target while it actually moved ON to it. `backend`
    // validates its own `value` the same way (ui_automation.rs:852-863).
    if (request.value != null && request.value !== "leave") {
      return errorResponse(
        windowLabel,
        request,
        "value_not_supported",
        `Automation action "hover" accepts no value other than "leave" (got "${request.value}").`,
        availableTargets(),
        diagnostics,
      );
    }

    // #944 - the leave form is TARGET-FREE and must not run the selector gauntlet:
    // the thing you want to release is normally gone (menu torn down) or re-minted
    // (<For>), and a cleanup step that fails when the thing it cleans up is missing
    // is not a cleanup step. It cannot return missing_selector / target_hidden /
    // target_obscured, by construction (plan R5).
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

  // #944 - a real pointer hovers disabled controls: a tooltip is exactly what you
  // hover a disabled thing for. Only MUTATING actions are refused on a disabled
  // target. (Load-bearing on `.session-context-option` having no `pointer-events:
  // none`; see plan §8.)
  if (request.action !== "hover" && isElementDisabled(element)) {
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

  if (request.action === "hover") {
    const expired = expiredMutationResponse(windowLabel, request, diagnostics);
    if (expired) return expired;

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

  if (request.action === "setValue" || request.action === "typeText") {
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

  // Defense-in-depth: an element that carries a value-bearing attribute must
  // never surface free text, regardless of role. #516 broadened the set of
  // text-allowed roles (status/metric/row/cell/...), so the absence of a role
  // from the allow-list can no longer be relied on to suppress a value-like
  // target's text — gate on the attributes explicitly instead.
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

/** #944 - eight events, and deliberately NOT pointermove/mousemove.
 *
 *  src/ has FOUR move listeners (an earlier version of this comment said three and
 *  called them all pointerdown-armed; two of those words were wrong):
 *
 *  - `main/App.tsx:78` (splitter) and `browser/App.tsx:101` (web-client splitter,
 *    at DOCUMENT level) attach theirs INSIDE the pointerdown / mousedown handler and
 *    remove it on release. No button, no listener.
 *  - `WorkgroupGroupRail.tsx:422` is always on (`window`), but `movePress` (:346)
 *    returns unless `reorderState()` is set, and only `startPress` - a pointerdown
 *    handler - sets it. Chromium gives a real mouse `pointerId: 1`, which is exactly
 *    what we would have hardcoded, so a synthetic move during a REAL user's group
 *    drag would cancel it or retarget it by our clientY, and their pointerup would
 *    commit that.
 *  - `screenshot-overlay/App.tsx:240` is always on and armed by NOTHING: it sets the
 *    crosshair from the event and repaints, on every move, with no button down. That
 *    window runs a bridge like any other (`main.tsx:26` inits one for every window
 *    root) and its canvas IS an automation target (`screenshotOverlay.canvas`), so a
 *    synthetic move there would drag the capture crosshair of a live overlay.
 *
 *  Nothing that consumes hover needs a move event; the flyouts are driven by
 *  enter/leave. Not dispatching one is what makes this action's inertness structural
 *  rather than lucky - the fourth listener is proof that "no consumer would notice"
 *  was never a safe bet. Plan R1. A2 pins it. */
function dispatchHoverEnter(to: HTMLElement): HoverDiagnostics {
  const from = hoveredElement;
  const events: string[] = [];

  if (from === to) {
    // A real pointer that has not moved fires nothing. `changed: false` is how a
    // harness sees that its hover was a no-op (plan C13).
    return { from: testIdOf(from), to: testIdOf(to), changed: false, events };
  }

  const staleFrom = !!from && !from.isConnected;
  const toChain = hoverChain(to);
  // Computed against the STORED chain: a detached `from` still shares body/html with
  // `to`, so we do not re-enter ancestors we never left. null (no `from`) means the
  // pointer arrived from outside the window: enter the whole chain, root first.
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

/** #944 - target-free (plan R5). Cannot fail, cannot be given a selector. */
function dispatchHoverLeave(): HoverDiagnostics {
  const from = hoveredElement;
  const events: string[] = [];
  if (!from) {
    return { from: null, to: null, changed: false, reason: "not_hovered", events };
  }

  const staleFrom = !from.isConnected;
  // to === null: the pointer leaves the window. The chain runs all the way up
  // (documentElement included) and relatedTarget is null. The whole STORED chain is
  // offered: a detached row is gone (dispatchHoverEvent drops it), but
  // `.sidebar-layout` above it is not, and it is still frozen (plan C17).
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

/** The enter chain runs OUTERMOST-FIRST, so an ancestor's handler runs before the
 *  events for its own descendants - including `to` itself. An `onMouseEnter` that
 *  tore down its own subtree would leave the rest of this function firing into dead
 *  nodes. No handler in src/ does that today, and it is `dispatchHoverEvent`'s
 *  isConnected guard, not that fact, that makes it safe. */
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

/** THE invariant, and the single place it is enforced: never dispatch into a node
 *  that has left the document, and never report an event that did not land.
 *  Dispatching into a removed node is a lie the DOM would not tell (plan §8.4), and
 *  `diagnostics.events` is the harness's only evidence of what happened - a phantom
 *  entry in it is worse than a missing one.
 *
 *  A node in a chain is dead by the time we reach it in two ways:
 *  - it was already detached when the chain was captured. The routine case: <For>
 *    re-mints a row under a stationary cursor, so `from` is gone by the time we
 *    leave it, while its ancestors are still there and still owe us their leave.
 *  - a handler EARLIER in the same dispatch detached it (see dispatchEnterGroup).
 *
 *  The chains are therefore passed WHOLE and filtered here, at dispatch time. A
 *  pre-filter cannot see the second case. */
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

/** [node, ...ancestors], innermost first, up to and including documentElement. */
function hoverChain(node: HTMLElement): HTMLElement[] {
  const chain: HTMLElement[] = [];
  let current: HTMLElement | null = node;
  while (current) {
    chain.push(current);
    current = parentElementOrHost(current);
  }
  return chain;
}

/** The first node of `fromChain` that is also on `toChain`. null = disjoint (a
 *  detached `from`, or a document-less node): the caller then leaves / enters the
 *  whole chain, which is what a pointer arriving from outside the window does. */
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
  return element?.getAttribute("data-ac-testid") ?? null;
}

/** `ok: true` requires a target, and a leave with nothing hovered has none. */
function emptyHoverTarget(): UiAutomationTarget {
  return {
    testId: "",
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

  // jsdom 25 has no PointerEvent, and MouseEventInit SILENTLY DROPS pointerId /
  // pointerType / isPrimary - so without this the suite would exercise
  // `pointerId: undefined` while the GUI ships `1`. Same trick as
  // WorkgroupGroupRail.test.tsx:120.
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
