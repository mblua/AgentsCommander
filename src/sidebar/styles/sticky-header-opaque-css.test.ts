import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { declProps, declValue, escapeRe, scanRules } from "./css-test-helpers";

// #4821 - in 4 sidebar styles the sticky project header must stay opaque: each
// style's `background` shorthand must end in the `var(--sidebar-bg)` color layer.
// The sheet is CRLF on disk; every regex here is CRLF-safe (`[^}]*`, `m` flag).
const RAW = readFileSync(new URL("./sidebar.css", import.meta.url), "utf8");
const CSS = RAW.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\r\n]/g, " "));
const RULES = scanRules(CSS);
const STYLES = ["deep-space", "arctic-ops", "obsidian-mesh", "neon-circuit"];

/** The block whose line starts with exactly `selector {`. Throws unless exactly one. */
function block(selector: string): { body: string; index: number } {
  const re = new RegExp(`^${escapeRe(selector)} \\{([^}]*)\\}`, "gm");
  const hits = [...CSS.matchAll(re)];
  const hit = hits[0];
  if (hits.length !== 1 || hit === undefined) {
    throw new Error(`expected 1 block for ${selector}, got ${hits.length}`);
  }
  return { body: hit[1] ?? "", index: hit.index };
}

const ds = '[data-sidebar-style="deep-space"]';
const ao = '[data-sidebar-style="arctic-ops"]';
const om = '[data-sidebar-style="obsidian-mesh"]';
const nc = '[data-sidebar-style="neon-circuit"]';
const lt = "html.light-theme";
const HEADERS: Array<[string, string[]]> = [
  [`${ds} .project-header`, ["rgba(20, 30, 70, 0.6)", "rgba(10, 15, 45, 0.4)", "rgba(12, 18, 40, 0.7)"]],
  [`${ao} .project-header`, ["rgba(160, 200, 255, 0.05)", "rgba(180, 210, 240, 0.06)"]],
  [`${lt}${ao} .project-header`, ["rgba(200, 225, 255, 0.3)", "rgba(220, 235, 255, 0.5)"]],
  [`${om} .project-header`, ["rgba(40, 30, 15, 0.6)", "rgba(25, 20, 10, 0.4)"]],
  [`${lt}${om} .project-header`, ["rgba(240, 220, 170, 0.25)", "rgba(230, 210, 150, 0.15)"]],
  [`${nc} .project-header`, ["rgba(255, 0, 180, 0.03)"]],
  [`${lt}${nc} .project-header`, ["rgba(255, 0, 180, 0.04)"]],
];

describe("#4821 sticky project header opaque", () => {
  it.each(HEADERS)("%s background ends in var(--sidebar-bg) and keeps its stops", (sel, stops) => {
    const bg = declValue(block(sel).body, "background");
    expect(bg).toMatch(/var\(--sidebar-bg\)$/);
    for (const stop of stops) expect(bg).toContain(stop);
  });

  it("no later style header rule overrides background", () => {
    const first = Math.min(...HEADERS.map(([sel]) => block(sel).index));
    const later = RULES.filter((r) => r.index > first).filter((r) =>
      r.selectors.some(
        (s) => STYLES.some((st) => s.includes(`[data-sidebar-style="${st}"]`)) && s.endsWith(".project-header"),
      ),
    );
    const owned = new Set(HEADERS.map(([sel]) => block(sel).index));
    for (const r of later) {
      if (owned.has(r.index)) continue;
      expect(declProps(r.body)).not.toContain("background");
    }
  });

  it("neon panel line sits above the sticky header and ignores the pointer", () => {
    const body = block(`${nc} .project-panel::before`).body;
    expect(declValue(body, "z-index")).toBe("21");
    expect(declValue(body, "pointer-events")).toBe("none");
  });

  it("deep-space light error slot keeps today's background", () => {
    const body = block(`${lt}${ds} .project-filter-error-slot`).body;
    const bg = declValue(body, "background");
    expect(bg).toContain("rgba(20, 30, 70, 0.6)");
    expect(bg).toMatch(/var\(--sidebar-bg\)$/);
    expect(body).not.toContain("rgba(12, 18, 40, 0.7)");
  });
});
