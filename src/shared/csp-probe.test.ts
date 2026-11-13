// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AutomationAPI } from "./ipc";
import {
  formatCspProbeState, initCspProbe, readHeadStyleFact, resetCspProbeForTests,
} from "./csp-probe";

vi.mock("./ipc", () => ({ AutomationAPI: { enabled: vi.fn() } }));
const enabled = vi.mocked(AutomationAPI.enabled);
const probe = () => document.querySelector<HTMLButtonElement>('[data-ac-testid="csp-probe"]');
const state = () => probe()?.getAttribute("data-ac-state");
function violation(effectiveDirective: string): void {
  document.dispatchEvent(
    Object.assign(new Event("securitypolicyviolation"), { effectiveDirective }),
  );
}

beforeEach(() => {
  document.head.replaceChildren();
  document.body.replaceChildren();
  enabled.mockReset().mockResolvedValue(false);
});
afterEach(() => {
  resetCspProbeForTests();
  vi.restoreAllMocks();
});

describe("CSP probe automation gate", () => {
  it("does not expose an action when disabled", async () => {
    await initCspProbe();
    expect(probe()).toBeNull();
  });
  it("treats a rejected gate as disabled", async () => {
    enabled.mockRejectedValue(new Error("unavailable"));
    await initCspProbe();
    expect(probe()).toBeNull();
  });
  it("creates one button and installs one listener across repeated initialization", async () => {
    enabled.mockResolvedValue(true);
    await initCspProbe();
    expect(state()).toBe("v=0;src=0;elem=0;inline=pending;head=absent");
    expect(probe()?.getAttribute("data-ac-role")).toBe("button");
    await initCspProbe();
    expect(document.querySelectorAll('[data-ac-testid="csp-probe"]')).toHaveLength(1);
    violation("img-src");
    expect(state()).toBe("v=1;src=0;elem=0;inline=pending;head=absent");
  });
  it("counts each directive without losing other violations", async () => {
    enabled.mockResolvedValue(true);
    await initCspProbe();
    violation("script-src-elem");
    violation("script-src");
    violation("img-src");
    expect(state()).toBe("v=3;src=1;elem=1;inline=pending;head=absent");
  });
  it("counts synchronously before the disabled gate resolves", async () => {
    const initialization = initCspProbe();
    violation("script-src");
    await initialization;
    enabled.mockResolvedValue(true);
    await initCspProbe();
    expect(state()).toBe("v=1;src=1;elem=0;inline=pending;head=absent");
  });
  it("reports whether the fixed inline script ran when clicked", async () => {
    enabled.mockResolvedValue(true);
    await initCspProbe();
    probe()?.click();
    const ran = (window as Window & { __acCspInlineProbe?: number }).__acCspInlineProbe === 1;
    expect(state()).toContain(`inline=${ran ? "ran" : "blocked"};`);
    expect(document.head.querySelector("script")).toBeNull();
  });
});

describe("CSP head style fact", () => {
  it("reports an absent head style", () => {
    expect(readHeadStyleFact(document)).toBe("absent");
  });
  it("recognizes the applied head style", () => {
    const style = document.createElement("style");
    style.textContent = "html { overflow: hidden; }";
    document.head.append(style);
    expect(readHeadStyleFact(document)).toBe("ok");
  });
  it("reports a style without a sheet as blocked", () => {
    const style = document.createElement("style");
    document.head.append(style);
    Object.defineProperty(style, "sheet", { value: null });
    expect(readHeadStyleFact(document)).toBe("blocked");
  });
  it("reports a style read failure as blocked", () => {
    vi.spyOn(document.head, "querySelector").mockImplementation(() => { throw new Error("DOM"); });
    expect(readHeadStyleFact(document)).toBe("blocked");
  });
});

it("formats exact zero and large counts", () => {
  expect(formatCspProbeState({ total: 0, scriptSrc: 0, scriptSrcElem: 0,
    inline: "pending", head: "absent" })).toBe("v=0;src=0;elem=0;inline=pending;head=absent");
  expect(formatCspProbeState({ total: 1234567, scriptSrc: 1000000, scriptSrcElem: 234567,
    inline: "blocked", head: "ok" }))
    .toBe("v=1234567;src=1000000;elem=234567;inline=blocked;head=ok");
});
