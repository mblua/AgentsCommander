import { AutomationAPI } from "./ipc";
import { automationAttrs } from "./automation-hooks";

export const CSP_PROBE_TEST_ID = "csp-probe";
export type CspProbeOutcome = "pending" | "ran" | "blocked" | "error";
export type CspHeadStyle = "ok" | "blocked" | "absent";
type ProbeState = {
  total: number;
  scriptSrc: number;
  scriptSrcElem: number;
  inline: CspProbeOutcome;
  head: CspHeadStyle;
};

let state: ProbeState = freshState();
let listenerInstalled = false;
let probeElement: HTMLButtonElement | null = null;

function freshState(): ProbeState {
  return { total: 0, scriptSrc: 0, scriptSrcElem: 0, inline: "pending", head: "absent" };
}

export function formatCspProbeState(value: ProbeState): string {
  return `v=${value.total};src=${value.scriptSrc};elem=${value.scriptSrcElem};` +
    `inline=${value.inline};head=${value.head}`;
}

export function readHeadStyleFact(doc: Document): CspHeadStyle {
  try {
    const style = doc.head.querySelector("style");
    if (!style) return "absent";
    return style.sheet !== null && getComputedStyle(doc.documentElement).overflow === "hidden"
      ? "ok" : "blocked";
  } catch {
    return "blocked";
  }
}

function refreshState(): void {
  probeElement?.setAttribute("data-ac-state", formatCspProbeState(state));
}

function onViolation(event: Event): void {
  state.total += 1;
  const directive = (event as SecurityPolicyViolationEvent).effectiveDirective;
  if (directive === "script-src") state.scriptSrc += 1;
  if (directive === "script-src-elem") state.scriptSrcElem += 1;
  refreshState();
}

export function runCspProbe(): void {
  const probeWindow = window as Window & { __acCspInlineProbe?: number };
  try {
    delete probeWindow.__acCspInlineProbe;
    const script = document.createElement("script");
    script.textContent = "window.__acCspInlineProbe=1";
    document.head.append(script);
    script.remove();
    state.inline = probeWindow.__acCspInlineProbe === 1 ? "ran" : "blocked";
  } catch {
    state.inline = "error";
  }
  refreshState();
}

function createProbeElement(): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  const attrs = automationAttrs(CSP_PROBE_TEST_ID, "button", formatCspProbeState(state));
  for (const [name, value] of Object.entries(attrs)) button.setAttribute(name, value);
  button.setAttribute("aria-label", "CSP probe");
  button.style.cssText = "position:fixed;right:0;bottom:0;width:8px;height:8px;" +
    "opacity:0;z-index:2147483647;border:0;padding:0;margin:0";
  button.addEventListener("click", runCspProbe);
  document.body.append(button);
  return button;
}

export async function initCspProbe(): Promise<void> {
  if (!listenerInstalled) {
    document.addEventListener("securitypolicyviolation", onViolation, true);
    listenerInstalled = true;
    state.head = readHeadStyleFact(document);
  }
  const enabled = await AutomationAPI.enabled().catch(() => false);
  if (!enabled || probeElement) return;
  probeElement = createProbeElement();
}

export function resetCspProbeForTests(): void {
  document.removeEventListener("securitypolicyviolation", onViolation, true);
  probeElement?.remove();
  probeElement = null;
  listenerInstalled = false;
  state = freshState();
  delete (window as Window & { __acCspInlineProbe?: number }).__acCspInlineProbe;
}
